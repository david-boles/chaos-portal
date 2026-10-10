use defmt::{info, warn};
use embassy_sync::channel::DynamicSender;
use embassy_time::{Duration, Instant, Ticker, Timer};
use esp_hal::{
    etm::{Etm, EtmConfiguredChannel, EtmEvent, EtmTask},
    gpio::{
        AnyPin, Level, Output, OutputConfig,
        etm::{Channels, InputConfig},
    },
    peripherals::{ETM, GPIO_SD, TIMG1},
    timer::{
        Timer as HalTimer,
        timg::{TimerGroup, etm::Tasks},
    },
};

use crate::InputEvent::{self, FlTouch, FrTouch, RlTouch, RrTouch};

/// Minimum time it takes to charge a cap sense pin to consider it touched.
const CAP_SENSE_PERIOD: Duration = Duration::from_micros(50);
/// Time between subsequent measurements of the inputs.
const POLL_PERIOD: Duration = Duration::from_millis(1);
/// Time required for a pin to continually not be touched in order for a new touch to register.
const DEBOUNCE_PERIOD: Duration = Duration::from_millis(100);

struct CapPin<'a> {
    channel: &'a mut dyn TogglableEtmChannel,
    input_event: InputEvent,
    was_touched: bool,
    time_last_touched: Option<Instant>,
}

impl<'a> CapPin<'a> {
    fn new(channel: &'a mut dyn TogglableEtmChannel, input_event: InputEvent) -> Self {
        Self {
            channel,
            input_event,
            was_touched: false,
            time_last_touched: None,
        }
    }
}

#[embassy_executor::task]
pub async fn run_touch(
    input_stream: DynamicSender<'static, InputEvent>,
    driver_pin: AnyPin<'static>,
    fl_pin: AnyPin<'static>,
    fr_pin: AnyPin<'static>,
    rl_pin: AnyPin<'static>,
    rr_pin: AnyPin<'static>,
    sd: GPIO_SD<'static>,
    timer: TIMG1<'static>,
    etm: ETM<'static>,
) {
    info!("input_touch_setup");
    // === SETUP ===
    let mut driver = Output::new(driver_pin, Level::High, OutputConfig::default());

    let gpio_ext = Channels::new(sd);
    let fl_sense_event = gpio_ext
        .channel0_event
        .rising_edge(fl_pin, InputConfig::default());
    let fr_sense_event = gpio_ext
        .channel1_event
        .rising_edge(fr_pin, InputConfig::default());
    let rl_sense_event = gpio_ext
        .channel2_event
        .rising_edge(rl_pin, InputConfig::default());
    let rr_sense_event = gpio_ext
        .channel3_event
        .rising_edge(rr_pin, InputConfig::default());

    let timer = TimerGroup::new(timer).timer0;
    let timer_stop_task = timer.cnt_stop();

    let etm = Etm::new(etm);
    let mut fl_etm_channel = etm.channel0.setup(&fl_sense_event, &timer_stop_task);
    let mut fr_etm_channel = etm.channel1.setup(&fr_sense_event, &timer_stop_task);
    let mut rl_etm_channel = etm.channel2.setup(&rl_sense_event, &timer_stop_task);
    let mut rr_etm_channel = etm.channel3.setup(&rr_sense_event, &timer_stop_task);

    let mut cap_pins = [
        CapPin::new(&mut fl_etm_channel, FlTouch),
        CapPin::new(&mut fr_etm_channel, FrTouch),
        CapPin::new(&mut rl_etm_channel, RlTouch),
        CapPin::new(&mut rr_etm_channel, RrTouch),
    ];
    for cap_pin in cap_pins.iter_mut() {
        cap_pin.channel.disable();
    }

    let mut ticker = Ticker::every(POLL_PERIOD);
    assert!(POLL_PERIOD >= (20 * CAP_SENSE_PERIOD)); // Technically minimum is 3*4 = 12x, but there's additional overhead.

    info!("input_touch_setup complete");

    // === EXECUTE ===
    loop {
        for cap_pin in cap_pins.iter_mut() {
            // Discharge.
            driver.set_low();
            Timer::after(2 * CAP_SENSE_PERIOD).await;

            // Time charging to detect touch, try to ensure deterministic start.
            critical_section::with(|_cs| {
                timer.start();
                timer.reset();
                cap_pin.channel.enable();
                driver.set_high();
            });
            Timer::after(CAP_SENSE_PERIOD).await;
            cap_pin.channel.disable();
            // if cap_pin.input_event == FrTouch {
            //     info!("{}", timer.now().duration_since_epoch().as_micros());
            // }
            let is_touched =
                timer.now().duration_since_epoch().as_micros() >= CAP_SENSE_PERIOD.as_micros();

            // Perform debounce
            let now = Instant::now();

            if is_touched
                && !cap_pin.was_touched
                && cap_pin.time_last_touched.is_none_or(|time_last_touched| {
                    now.saturating_duration_since(time_last_touched) > DEBOUNCE_PERIOD
                })
                && let Err(_) = input_stream.try_send(cap_pin.input_event)
            {
                warn!("Input stream overflow! Dropped: {}", cap_pin.input_event)
            }

            cap_pin.was_touched = is_touched;
            if is_touched {
                cap_pin.time_last_touched = Some(now);
            }
        }

        ticker.next().await;
    }
}

trait TogglableEtmChannel {
    fn enable(&mut self) -> ();
    fn disable(&mut self) -> ();
}

impl<'a, E: EtmEvent, T: EtmTask, const C: u8> TogglableEtmChannel
    for EtmConfiguredChannel<'a, E, T, C>
{
    fn enable(&mut self) {
        if C < 32 {
            ETM::regs()
                .ch_ena_ad0_set()
                .write(|w| w.ch_set(C).set_bit());
        } else {
            ETM::regs()
                .ch_ena_ad1_set()
                .write(|w| w.ch_set(C - 32).set_bit());
        }
    }

    fn disable(&mut self) {
        if C < 32 {
            ETM::regs()
                .ch_ena_ad0_clr()
                .write(|w| w.ch_clr(C).set_bit());
        } else {
            ETM::regs()
                .ch_ena_ad1_clr()
                .write(|w| w.ch_clr(C - 32).set_bit());
        }
    }
}
