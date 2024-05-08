use defmt::*;
use embassy_rp::gpio::{Level, Output, Pin};
use embassy_time::{Duration, Instant, Timer};

use crate::dht11::DHT11;

pub static mut SHARED_TEMP_HUMID: (i32, i32) = (0, 0);

#[derive(PartialEq, Format)]
enum ControllerState {
    Idle,
    Running,
    Cooldown,
}

#[embassy_executor::task]
pub async fn temp_controller_task(
    mut dht11_ctl: DHT11,
    threshold_temperature: i8,
    relay_pin: impl Pin,
    minimum_runtime: Duration,
    cooldown_time: Duration,
    // mut _shared_temp_var: (i32, i32),
) -> ! {
    let mut machine_state = ControllerState::Idle;
    let mut runtime_start = Instant::from_secs(0);
    let mut cooldown_starttime = Instant::from_secs(0);

    let mut relay_output = Output::new(relay_pin, Level::Low);

    loop {
        let (temperature, humidity) = dht11_ctl.get_temperature_humidity();
        let current_time = Instant::now();

        unsafe { SHARED_TEMP_HUMID = (temperature as i32, humidity as i32) };

        info!(
            "Machine State: {}, Time: {}, Temp: {}",
            machine_state, current_time, temperature
        );

        if (machine_state == ControllerState::Idle) && (temperature > threshold_temperature) {
            machine_state = ControllerState::Running;
            runtime_start = Instant::now();
            relay_output.set_high();
        } else if (machine_state == ControllerState::Running)
            && (current_time > (runtime_start + minimum_runtime))
        {
            machine_state = ControllerState::Cooldown;
            cooldown_starttime = Instant::now();
            relay_output.set_low();
        } else if (machine_state == ControllerState::Cooldown)
            && (current_time > (cooldown_starttime + cooldown_time))
        {
            machine_state = ControllerState::Idle;
        }
        Timer::after_secs(1).await
    }
}
