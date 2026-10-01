#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use defmt::info;
use embassy_executor::Spawner;
use embassy_time::{Delay, Duration, Timer as ETimer};
use esp_hal::clock::CpuClock;
use esp_hal::etm::Etm;
use esp_hal::gpio::etm::{self, Channels};
use esp_hal::gpio::{self, Input, Level, Output, OutputConfig};
use esp_hal::i2c;
use esp_hal::i2c::master::I2c;
use esp_hal::time::Rate;
use esp_hal::timer::{
    Timer,
    timg::{TimerGroup, etm::Tasks},
};
use panic_rtt_target as _;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

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

    let mut led = Output::new(peripherals.GPIO15, Level::High, OutputConfig::default());

    let mut cap_driver = Output::new(peripherals.GPIO16, Level::High, OutputConfig::default());
    // let cap_sense = Input::new(peripherals.GPIO18, gpio::InputConfig::default());
    let gpio_ext = Channels::new(peripherals.GPIO_SD);
    let cap_sense_event = gpio_ext
        .channel0_event
        .rising_edge(peripherals.GPIO17, etm::InputConfig::default());
    let cap_timer = TimerGroup::new(peripherals.TIMG1).timer0;
    let cap_timer_task = cap_timer.cnt_stop();
    let etm = Etm::new(peripherals.ETM);
    let _cap_timer_etm_channel = etm.channel0.setup(&cap_sense_event, &cap_timer_task);

    let mut i2c = I2c::new(
        peripherals.I2C0,
        i2c::master::Config::default().with_frequency(Rate::from_khz(100)),
    )
    .expect("i2c setup")
    .with_sda(peripherals.GPIO22)
    .with_scl(peripherals.GPIO23);
    let mut vl53l1_dev = vl53l1::Device::default();
    let mut delay = Delay {};
    vl53l1::software_reset(&mut vl53l1_dev, &mut i2c, &mut delay).expect("vl53l1 software reset");
    vl53l1::data_init(&mut vl53l1_dev, &mut i2c).expect("vl53l1 data init");
    vl53l1::static_init(&mut vl53l1_dev).expect("vl53l1 static init");
    vl53l1::set_measurement_timing_budget_micro_seconds(&mut vl53l1_dev, 20_000)
        .expect("vl53l1 timing budget");
    vl53l1::set_inter_measurement_period_milli_seconds(&mut vl53l1_dev, 50)
        .expect("vl53l1 measurement period");
    vl53l1::start_measurement(&mut vl53l1_dev, &mut i2c).expect("vl53l1 start measurement");

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    info!("Embassy initialized!");

    // TODO: Spawn some tasks
    let _ = spawner;

    loop {
        info!(
            "{}",
            defmt::Debug2Format(&vl53l1::get_ranging_measurement_data(
                &mut vl53l1_dev,
                &mut i2c
            ))
        )
    }

    // loop {
    //     info!("Hello world!");
    //     led.toggle();
    //     critical_section::with(|_cs| {
    //         cap_timer.start();
    //         cap_timer.reset();
    //         cap_driver.set_high();
    //     });
    //     ETimer::after(Duration::from_millis(100)).await;
    //     info!(
    //         "timer: {}",
    //         cap_timer.now().duration_since_epoch().as_micros()
    //     );
    //     cap_driver.set_low();

    //     // ETimer::after(Duration::from_secs(1)).await;
    // }

    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.2.2/examples
}
