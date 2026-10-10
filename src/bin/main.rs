#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use chaos_portal::InputEvent;
use chaos_portal::input_touch::run_touch;
use chaos_portal::rmt_rgb_strip::RgbStrip;
use defmt::info;
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, DynamicReceiver};
use embassy_time::{Delay, Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::i2c;
use esp_hal::i2c::master::I2c;
use esp_hal::riscv::singleton;
use esp_hal::rng::Rng;
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use panic_rtt_target as _;
use static_cell::StaticCell;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

static INPUT_STREAM: StaticCell<Channel<CriticalSectionRawMutex, InputEvent, 16>> =
    StaticCell::new();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // generator version: 1.4.0
    // generator parameters: -o esp32c6 -o unstable-hal -o embassy -o probe-rs -o defmt -o panic-rtt-target -o embedded-test -o cargo-embed -o vscode -o stable-x86_64-unknown-linux-gnu

    rtt_target::rtt_init_defmt!();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);
    info!("Embassy initialized!");

    let input_stream = INPUT_STREAM.init(Channel::new());

    spawner.spawn(
        run_touch(
            input_stream.dyn_sender(),
            peripherals.GPIO16.into(),
            peripherals.GPIO18.into(), // fl
            peripherals.GPIO20.into(), // fr
            peripherals.GPIO19.into(), // rl
            peripherals.GPIO17.into(), // rr
            peripherals.GPIO_SD,
            peripherals.TIMG1,
            peripherals.ETM,
        )
        .expect("run_touch spawn failed"),
    );

    spawner.spawn(print_inputs(input_stream.dyn_receiver()).unwrap());

    let _led = Output::new(peripherals.GPIO15, Level::High, OutputConfig::default());

    // let _cap_driver = Output::new(peripherals.GPIO16, Level::High, OutputConfig::default());
    // // let cap_sense = Input::new(peripherals.GPIO18, gpio::InputConfig::default());
    // let gpio_ext = Channels::new(peripherals.GPIO_SD);
    // let cap_sense_event = gpio_ext
    //     .channel0_event
    //     .rising_edge(peripherals.GPIO17, etm::InputConfig::default());
    // let cap_timer = TimerGroup::new(peripherals.TIMG1).timer0;
    // let cap_timer_task = cap_timer.cnt_stop();
    // let etm = Etm::new(peripherals.ETM);
    // let _cap_timer_etm_channel = etm.channel0.setup(&cap_sense_event, &cap_timer_task);

    let mut i2c = I2c::new(
        peripherals.I2C0,
        i2c::master::Config::default().with_frequency(Rate::from_khz(100)),
    )
    .expect("i2c setup")
    .with_sda(peripherals.GPIO22)
    .with_scl(peripherals.GPIO23)
    .into_async();
    let mut vl53l1_dev = vl53l1::Device::default();
    let mut delay = Delay {};
    info!("initializing vl53l1");
    while vl53l1::software_reset(&mut vl53l1_dev, &mut i2c, &mut delay)
        .await
        .is_err()
    {}
    vl53l1::data_init(&mut vl53l1_dev, &mut i2c)
        .await
        .expect("vl53l1 data init");
    vl53l1::static_init(&mut vl53l1_dev).expect("vl53l1 static init");
    vl53l1::set_measurement_timing_budget_micro_seconds(&mut vl53l1_dev, 20_000)
        .expect("vl53l1 timing budget");
    vl53l1::set_inter_measurement_period_milli_seconds(&mut vl53l1_dev, 50)
        .expect("vl53l1 measurement period");
    vl53l1::start_measurement(&mut vl53l1_dev, &mut i2c)
        .await
        .expect("vl53l1 start measurement");

    let mut display = RgbStrip::new(
        peripherals.RMT,
        Output::new(peripherals.GPIO21, Level::Low, OutputConfig::default()),
    );
    let mut display_buf = singleton!(: [u32; 64] = [0; 64])
        .expect("display buf init")
        .as_mut_slice();

    let _rng = Rng::new();

    // TODO: Spawn some tasks
    let _ = spawner;

    for p in 0..64 {
        display_buf[p] = if OCCLUDED_PIXELS_MASK[p] {
            u32::from_be_bytes([50, 50, 50, 0])
        } else {
            u32::from_be_bytes([255, 255, 255, 0])
        };
    }
    (display, display_buf) = display.transmit(display_buf).await;

    // loop {}

    let _was_hand_detected = false;
    let _number: usize = 0;

    loop {
        info!("Hello!");
        Timer::after(Duration::from_millis(1000)).await;
    }

    //     let start = Instant::now();
    //     let measurement = vl53l1::get_ranging_measurement_data(&mut vl53l1_dev, &mut i2c).await;
    //     let _delay = start.elapsed().as_micros();
    //     // info!("delay: {}", delay);

    //     let hand_detected = if let Ok(measurement) = measurement {
    //         measurement.range_status == RangeStatus::RANGE_VALID
    //             && measurement.range_milli_meter < 200
    //     } else {
    //         false
    //     };

    //     let new_hand_detection = hand_detected && !was_hand_detected;

    //     if new_hand_detection {
    //         number = (rng.random() % 20) as usize;
    //     }

    //     for p in 0..64 {
    //         if OCCLUDED_PIXELS_MASK[p] {
    //             let mut brightness: u8 = display_buf[p].to_be_bytes()[0];

    //             if hand_detected {
    //                 brightness = brightness.saturating_add(
    //                     (rng.random() % if NUMBER_MASKS[number][p] { 100 } else { 25 }) as u8,
    //                 );
    //                 if brightness > 50 {
    //                     brightness = 50;
    //                 }
    //             } else {
    //                 brightness = brightness.saturating_sub(
    //                     (rng.random() % if NUMBER_MASKS[number][p] { 2 } else { 10 }) as u8,
    //                 );
    //             }

    //             display_buf[p] = u32::from_be_bytes([brightness, brightness, brightness, 0])
    //         }
    //     }
    //     (display, display_buf) = display.transmit(display_buf).await;

    //     was_hand_detected = hand_detected;
    // }

    // // loop {
    // //     info!("Hello world!");
    // //     led.toggle();
    // //     critical_section::with(|_cs| {
    // //         cap_timer.start();
    // //         cap_timer.reset();
    // //         cap_driver.set_high();
    // //     });
    // //     ETimer::after(Duration::from_millis(100)).await;
    // //     info!(
    // //         "timer: {}",
    // //         cap_timer.now().duration_since_epoch().as_micros()
    // //     );
    // //     cap_driver.set_low();

    // //     // ETimer::after(Duration::from_secs(1)).await;
    // // }

    // // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.2.2/examples
}


#[embassy_executor::task]
pub async fn print_inputs(
    input_stream: DynamicReceiver<'static, InputEvent>,
) {
    loop {

            info!("Input: {}", input_stream.receive().await);
    }
}

#[rustfmt::skip]
const OCCLUDED_PIXELS_MASK: [bool; 64] = [
    false, true, true, true, true, true, false, false,
    true, true, true, true, true, true, true, true,
    true, true, true, true, true, true, true, true,
    true, true, true, true, true, true, true, true,
    true, true, true, true, false, true, true, true,
    true, true, true, true, true, true, true, true,
    true, true, true, true, true, true, true, true,
    false, false, true, true, true, true, false, false
];

#[rustfmt::skip]
const NUMBER_1_MASK: [bool; 64] = [
    false, false, false, false, false, false, false, false,
    false, false, false, true,  false, false, false, false,
    false, false, false, true,  false, false, false, false,
    false, false, false, true,  false, false, false, false,
    false, false, false, true,  false, false, false, false,
    false, false, false, true,  false, false, false, false,
    false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false
];

#[rustfmt::skip]
const NUMBER_2_MASK: [bool; 64] = [
    false, false, false, false, false, false, false, false,
    false, false, true,  true,  true,  false, false, false,
    false, false, false, false, true,  false, false, false,
    false, false, true,  true,  true,  false, false, false,
    false, false, true,  false, false, false, false, false,
    false, false, true,  true,  true,  false, false, false,
    false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false
];

#[rustfmt::skip]
const NUMBER_3_MASK: [bool; 64] = [
    false, false, false, false, false, false, false, false,
    false, false, true,  true,  true,  false, false, false,
    false, false, false, false, true,  false, false, false,
    false, false, true,  true,  true,  false, false, false,
    false, false, false, false, true,  false, false, false,
    false, false, true,  true,  true,  false, false, false,
    false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false
];

#[rustfmt::skip]
const NUMBER_4_MASK: [bool; 64] = [
    false, false, false, false, false, false, false, false,
    false, false, true,  false, true,  false, false, false,
    false, false, true,  false, true,  false, false, false,
    false, false, true,  true,  true,  false, false, false,
    false, false, false, false, true,  false, false, false,
    false, false, false, false, true,  false, false, false,
    false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false
];

#[rustfmt::skip]
const NUMBER_5_MASK: [bool; 64] = [
    false, false, false, false, false, false, false, false,
    false, false, true,  true,  true,  false, false, false,
    false, false, true,  false, false, false, false, false,
    false, false, true,  true,  true,  false, false, false,
    false, false, false, false, true,  false, false, false,
    false, false, true,  true,  true,  false, false, false,
    false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false
];

#[rustfmt::skip]
const NUMBER_6_MASK: [bool; 64] = [
    false, false, false, false, false, false, false, false,
    false, false, true,  true,  true,  false, false, false,
    false, false, true,  false, false, false, false, false,
    false, false, true,  true,  true,  false, false, false,
    false, false, true,  false, true,  false, false, false,
    false, false, true,  true,  true,  false, false, false,
    false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false
];

#[rustfmt::skip]
const NUMBER_7_MASK: [bool; 64] = [
    false, false, false, false, false, false, false, false,
    false, false, true,  true,  true,  false, false, false,
    false, false, false, false, true,  false, false, false,
    false, false, false, false, true,  false, false, false,
    false, false, false, false, true,  false, false, false,
    false, false, false, false, true,  false, false, false,
    false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false
];

#[rustfmt::skip]
const NUMBER_8_MASK: [bool; 64] = [
    false, false, false, false, false, false, false, false,
    false, false, true,  true,  true,  false, false, false,
    false, false, true,  false, true,  false, false, false,
    false, false, true,  true,  true,  false, false, false,
    false, false, true,  false, true,  false, false, false,
    false, false, true,  true,  true,  false, false, false,
    false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false
];

#[rustfmt::skip]
const NUMBER_9_MASK: [bool; 64] = [
    false, false, false, false, false, false, false, false,
    false, false, true,  true,  true,  false, false, false,
    false, false, true,  false, true,  false, false, false,
    false, false, true,  true,  true,  false, false, false,
    false, false, false, false, true,  false, false, false,
    false, false, true,  true,  true,  false, false, false,
    false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false
];

#[rustfmt::skip]
const NUMBER_10_MASK: [bool; 64] = [
    false, false, false, false, false, false, false, false,
    false, false, true,  false, true,  true,  true, false,
    false, false, true,  false, true,  false, true, false,
    false, false, true,  false, true,  false, true, false,
    false, false, true,  false, true,  false, true, false,
    false, false, true,  false, true,  true,  true, false,
    false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false
];

#[rustfmt::skip]
const NUMBER_11_MASK: [bool; 64] = [
    false, false, false, false, false, false, false, false,
    false, false, true,  false, true,  false, false, false,
    false, false, true,  false, true,  false, false, false,
    false, false, true,  false, true,  false, false, false,
    false, false, true,  false, true,  false, false, false,
    false, false, true,  false, true,  false, false, false,
    false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false
];

#[rustfmt::skip]
const NUMBER_12_MASK: [bool; 64] = [
    false, false, false,  false, false, false, false,false,
    false, false, true,   false, true,  true,  true,false,
    false, false, true,   false, false, false, true,false,
    false, false, true,   false, true,  true,  true,false,
    false, false, true,   false, true,  false, false,false,
    false, false, true,   false, true,  true,  true,false,
    false, false, false,  false, false, false, false,false,
    false, false, false,  false, false, false, false,false
];

#[rustfmt::skip]
const NUMBER_13_MASK: [bool; 64] = [
    false, false, false, false, false, false, false,false, 
    false, false, true, false, true,  true,  true,false, 
    false, false, true, false, false, false, true,false, 
    false, false, true, false, true,  true,  true,false, 
    false, false, true, false, false, false, true,false, 
    false, false, true, false, true,  true,  true,false, 
    false, false, false, false, false, false, false,false, 
    false, false, false, false, false, false, false, false
];

#[rustfmt::skip]
const NUMBER_14_MASK: [bool; 64] = [
    false, false, false, false, false, false, false,false, 
    false, false, true, false, true,  false, true,false, 
    false, false, true, false, true,  false, true,false, 
    false, false, true, false, true,  true,  true,false, 
    false, false, true, false, false, false, true,false, 
    false, false, true, false, false, false, true,false, 
    false, false, false, false, false, false, false,false, 
    false, false, false, false, false, false, false,false
];

#[rustfmt::skip]
const NUMBER_15_MASK: [bool; 64] = [
    false, false, false, false, false, false, false,false, 
    false, false, true, false, true,  true,  true,false, 
    false, false, true, false, true,  false, false,false, 
    false, false, true, false, true,  true,  true,false, 
    false, false, true, false, false, false, true,false, 
    false, false, true, false, true,  true,  true,false, 
    false, false, false, false, false, false, false,false, 
    false, false, false, false, false, false, false,false, 
];

#[rustfmt::skip]
const NUMBER_16_MASK: [bool; 64] = [
    false, false, false, false, false, false, false,false, 
    false, false, true, false, true,  true,  true,false, 
    false, false, true, false, true,  false, false,false, 
    false, false, true, false, true,  true,  true,false, 
    false, false, true, false, true,  false, true,false, 
    false, false, true, false, true,  true,  true,false, 
    false, false, false, false, false, false, false,false, 
    false, false, false, false, false, false, false,false
];

#[rustfmt::skip]
const NUMBER_17_MASK: [bool; 64] = [
    false, false, false, false, false, false, false,false, 
    false, false, true, false, true,  true,  true,false, 
    false, false, true, false, false, false, true,false, 
    false, false, true, false, false, false, true,false, 
    false, false, true, false, false, false, true,false, 
    false, false, true, false, false, false, true,false, 
    false, false, false, false, false, false, false,false, 
    false, false, false, false, false, false, false,false, 
];

#[rustfmt::skip]
const NUMBER_18_MASK: [bool; 64] = [
    false, false, false, false, false, false, false,false, 
    false, false, true, false, true,  true,  true,false, 
    false, false, true, false, true,  false, true,false, 
    false, false, true, false, true,  true,  true,false, 
    false, false, true, false, true,  false, true,false, 
    false, false, true, false, true,  true,  true,false, 
    false, false, false, false, false, false, false,false, 
    false, false, false, false, false, false, false,false, 
];

#[rustfmt::skip]
const NUMBER_19_MASK: [bool; 64] = [
    false, false, false, false, false, false, false,false, 
    false, false, true, false, true,  true,  true,false, 
    false, false, true, false, true,  false, true,false, 
    false, false, true, false, true,  true,  true,false, 
    false, false, true, false, false, false, true,false, 
    false, false, true, false, true,  true,  true,false, 
    false, false, false, false, false, false, false,false, 
    false, false, false, false, false, false, false,false, 
];

#[rustfmt::skip]
const NUMBER_20_MASK: [bool; 64] = [
    false, false, false, false, false, false, false, false,
    false, true,  true,  true, false, true,  true,  true,
    false, false, false, true, false, true,  false, true,
    false, true,  true,  true, false, true,  false, true,
    false, true,  false, false, false, true,  false, true,
    false, true,  true,  true, false, true,  true,  true,
    false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false
];

const NUMBER_MASKS: [&[bool; 64]; 20] = [
    &NUMBER_1_MASK,
    &NUMBER_2_MASK,
    &NUMBER_3_MASK,
    &NUMBER_4_MASK,
    &NUMBER_5_MASK,
    &NUMBER_6_MASK,
    &NUMBER_7_MASK,
    &NUMBER_8_MASK,
    &NUMBER_9_MASK,
    &NUMBER_10_MASK,
    &NUMBER_11_MASK,
    &NUMBER_12_MASK,
    &NUMBER_13_MASK,
    &NUMBER_14_MASK,
    &NUMBER_15_MASK,
    &NUMBER_16_MASK,
    &NUMBER_17_MASK,
    &NUMBER_18_MASK,
    &NUMBER_19_MASK,
    &NUMBER_20_MASK,
];
