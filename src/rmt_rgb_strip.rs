//! Simple driver for the RMT peripheral that allows transmitting WS2812B data pulsetrains.

use core::cell::RefCell;
use core::panic;

use critical_section::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use esp_hal::gpio::{Level, Output};
use esp_hal::interrupt::{InterruptHandler, Priority};
use esp_hal::peripherals::{Interrupt, PCR, RMT};
use esp_hal::rmt::PulseCode;
use esp_hal::{handler, interrupt};
use esp_metadata_generated::property;
use panic_rtt_target as _;

const HALF_RAM_SIZE_U32: usize = 2 * property!("rmt.channel_ram_size"); // We're using the RAM for all 4 channels

const NS_PER_CLOCK: u16 = 50;

// static DEBUG_PINS: Mutex<RefCell<Option<(Output, Output)>>> = Mutex::new(RefCell::new(None));
static TRANSMISSION: Mutex<RefCell<Option<Transmission>>> = Mutex::new(RefCell::new(None));
static ISR_DONE: Signal<CriticalSectionRawMutex, Transmission> = Signal::new();

pub struct RgbStrip {
    _rmt: RMT<'static>, // Unused but helps enforce that we're a singleton and no one else can use the RMT.
}

impl RgbStrip {
    pub fn new(
        rmt: RMT<'static>,
        output: Output<'static>,
        // debug_pins: (Output<'static>, Output<'static>),
    ) -> Self {
        // critical_section::with(|cs| *DEBUG_PINS.borrow_ref_mut(cs) = Some(debug_pins));

        // Initialize clocks
        PCR::regs().rmt_sclk_conf().modify(|_, w| {
            unsafe { w.sclk_sel().bits(1) }; // PLL_F80M_CLK // TODO this was done later as a separate write
            unsafe { w.sclk_div_num().bits(3) }; // + 1 = overall divisor of 4 => 20MHz clock => NS_PER_CLOCK = 50
            unsafe { w.sclk_div_a().bits(0) };
            unsafe { w.sclk_div_b().bits(0) }; // 0 => fractional divisor of 256 (w/numerator 0) XXX: this might have been referring to only the tx channel divider?
            w.sclk_en().bit(true)
        });
        PCR::regs()
            .rmt_conf()
            .modify(|_, w| w.rmt_clk_en().bit(true));

        // Disable FIFO mode
        RMT::regs()
            .sys_conf()
            .modify(|_, w| w.apb_fifo_mask().bit(true)); // 1: Access memory directly. FIFO mode seems poorly documented and has "gotchas"?

        // Configure TX0
        RMT::regs().ch0_tx_lim().modify(|_, w| unsafe {
            w.tx_lim().bits(48 * 2);
            w.loop_count_reset().set_bit();
            w.tx_loop_cnt_en().clear_bit();
            w.tx_loop_num().bits(0)
        });
        RMT::regs().ch0_tx_conf0().modify(|_, w| {
            unsafe { w.div_cnt().bits(1) };
            w.carrier_en().bit(false);
            w.carrier_eff_en().set_bit(); // TODO Probably not needed
            w.carrier_out_lv().bit(false); // TODO Probably not needed
            w.idle_out_en().bit(true);
            w.idle_out_lv().bit(false);
            unsafe { w.mem_size().bits(4) };
            w.tx_conti_mode().bit(false);
            w.mem_tx_wrap_en().bit(true);
            w.conf_update().set_bit()
        });
        esp_hal::gpio::OutputSignal::RMT_SIG_0.connect_to(&output);

        // Configure and enable interrupts
        RMT::regs().int_ena().write(|w| {
            w.ch0_tx_thr_event().bit(true);
            w.ch0_tx_end().bit(true);
            w.ch0_tx_err().bit(true)
        });
        unsafe {
            interrupt::bind_handler(
                Interrupt::RMT,
                InterruptHandler::new(rmt_handler, Priority::Priority1),
            )
        };
        interrupt::enable(Interrupt::RMT, Priority::Priority1);

        // Return
        Self { _rmt: rmt }
    }

    /// Data is one word per LED in GRB_ format.
    /// The provided buffer is returned upon successful completion of the transmission.
    // SAFETY: consume Self to ensure no races in RMT transmissions, even if the future were to be dropped.
    // SAFETY: require data to be static to ensure that it remains valid while the ISR reads from it even if the future were to be dropped.
    pub async fn transmit(self, data: &'static mut [u32]) -> (Self, &'static mut [u32]) {
        // Begin buffering data into the RMT RAM
        let mut transmission = Transmission {
            write_upper_half_next: false,
            pulses: PulseIter::new(data),
        };
        transmission.write_next_half_of_rmt_ram();
        transmission.write_next_half_of_rmt_ram();
        critical_section::with(|cs| TRANSMISSION.borrow_ref_mut(cs).replace(transmission));

        // Begin transmission
        RMT::regs().ch0_tx_conf0().modify(|_, w| {
            w.mem_rd_rst().set_bit();
            w.apb_mem_rst().set_bit(); // Probably not necessary? for FIFO mode
            w.tx_start().set_bit();
            w.conf_update().set_bit()
        });

        // Wait for transmission to complete, data will continue to be buffered into the RMT RAM by the ISR.
        (self, ISR_DONE.wait().await.pulses.data.reclaim())
    }
}

/// Slice iterator that we can reclaim the underlying buffer from.
struct DataIter {
    data: &'static mut [u32],
    index: usize,
}

impl DataIter {
    fn new(data: &'static mut [u32]) -> Self {
        DataIter { data, index: 0 }
    }

    fn reclaim(self) -> &'static mut [u32] {
        return self.data;
    }
}

impl Iterator for DataIter {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        let val = self.data.get(self.index).copied();
        if self.index < self.data.len() {
            self.index += 1;
        }
        val
    }
}

/// Generate a sequence of pulses from LED data.
#[derive(PartialEq, Eq)]
enum PulseIterState {
    /// Always provide a low period to ensure LEDs are reset
    LedReset,
    /// Transmit data
    Data {
        current_word: u32,
        next_mask: u32, // Single bit mask iterating from most to least significant bit, stopping short of the least significant byte.
    },
    /// Terminate with a 0 [PulseCode] to end the transmission.
    RmtEndTransmission,
    /// No more pulses to copy.
    Done,
}

struct PulseIter {
    data: DataIter,
    state: PulseIterState,
}

impl PulseIter {
    const INIT_MASK: u32 = 1 << 31;

    fn new(data: &'static mut [u32]) -> Self {
        Self {
            data: DataIter::new(data),
            state: PulseIterState::LedReset,
        }
    }

    fn is_empty(&self) -> bool {
        self.state == PulseIterState::Done
    }
}

// This would be much lovelier with https://github.com/rust-lang/rfcs/blob/master/text/2071-impl-trait-existential-types.md to just make an iterator with chaining and store the whole unnamed/unnameable type in static memory.
impl<'a> Iterator for PulseIter {
    type Item = PulseCode;

    fn next(&mut self) -> Option<Self::Item> {
        match self.state {
            PulseIterState::LedReset => {
                self.state = if let Some(w) = self.data.next() {
                    PulseIterState::Data {
                        current_word: w,
                        next_mask: Self::INIT_MASK,
                    }
                } else {
                    PulseIterState::RmtEndTransmission
                };
                // > 50us in total.
                Some(PulseCode::new(
                    Level::Low,
                    30000 / NS_PER_CLOCK,
                    Level::Low,
                    30000 / NS_PER_CLOCK,
                ))
            }
            PulseIterState::Data {
                current_word,
                next_mask,
            } => {
                self.state = if next_mask != (1 << 8) {
                    PulseIterState::Data {
                        current_word,
                        next_mask: next_mask >> 1,
                    }
                } else {
                    if let Some(w) = self.data.next() {
                        PulseIterState::Data {
                            current_word: w,
                            next_mask: Self::INIT_MASK,
                        }
                    } else {
                        PulseIterState::RmtEndTransmission
                    }
                };

                // 1.2us < T = 1.3us
                Some(if (current_word & next_mask) == 0 {
                    // 0.2us < T0H = 0.3us < 0.4us
                    PulseCode::new(
                        Level::High,
                        400 / NS_PER_CLOCK,
                        Level::Low,
                        850 / NS_PER_CLOCK,
                    )
                } else {
                    // 0.65us < T1H = 0.8us < 1us
                    PulseCode::new(
                        Level::High,
                        800 / NS_PER_CLOCK,
                        Level::Low,
                        450 / NS_PER_CLOCK,
                    )
                })
            }
            PulseIterState::RmtEndTransmission => {
                self.state = PulseIterState::Done;

                Some(PulseCode::end_marker())
            }
            PulseIterState::Done => None,
        }
    }
}

struct Transmission {
    write_upper_half_next: bool,
    pulses: PulseIter,
}

impl Transmission {
    fn write_next_half_of_rmt_ram(&mut self) {
        if self.pulses.is_empty() {
            return;
        };

        let mut ram_ptr = property!("rmt.ram_start") as *mut u32;

        if self.write_upper_half_next {
            ram_ptr = unsafe { ram_ptr.add(HALF_RAM_SIZE_U32) }
        }

        for pulse in (&mut self.pulses).take(HALF_RAM_SIZE_U32) {
            unsafe { ram_ptr.write_volatile(pulse.into()) };
            ram_ptr = unsafe { ram_ptr.add(1) };
        }

        self.write_upper_half_next = !self.write_upper_half_next;
    }
}

extern "C" fn rmt_handler() {
    critical_section::with(|cs| {
        // let mut debug_pins = DEBUG_PINS.borrow_ref_mut(cs);
        // let (pin0, pin1) = debug_pins
        //     .as_mut()
        //     .expect("rmt isr called before debug pin init");
        // pin0.set_high();

        let status = RMT::regs().int_st().read();

        // ERR
        if status.ch0_tx_err().bit() {
            panic!("rmt error");
        }
        // THR_EVENT
        if status.ch0_tx_thr_event().bit() {
            RMT::regs()
                .int_clr()
                .write(|w| w.ch0_tx_thr_event().bit(true));

            let mut t = TRANSMISSION.borrow_ref_mut(cs);
            let tu = t
                .as_mut()
                .expect("rmt isr thr without ongoing transmission");
            // pin1.set_high();
            tu.write_next_half_of_rmt_ram();
            // pin1.set_low();
        }
        // END
        if status.ch0_tx_end().bit() {
            RMT::regs().int_clr().write(|w| w.ch0_tx_end().bit(true));

            ISR_DONE.signal(
                TRANSMISSION
                    .borrow_ref_mut(cs)
                    .take()
                    .expect("rmt isr done without ongoing transmission"),
            );
        }

        // pin0.set_low();
    });
}
