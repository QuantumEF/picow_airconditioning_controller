use crate::dht11::DHT11;
use core::sync::atomic::AtomicI8;
use defmt::*;
use embassy_rp::gpio::{Level, Output, Pin};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, mutex::Mutex};
use embassy_time::{Duration, Instant, Timer};
use portable_atomic::Ordering;

pub static SHARED_TEMP: AtomicI8 = AtomicI8::new(0);
pub static SHARED_HUMID: AtomicI8 = AtomicI8::new(0);

#[derive(Debug, PartialEq, Format, Clone, Copy)]
pub enum ControllerState {
    Idle,
    Running { starttime: Instant },
    Cooldown { starttime: Instant },
}

#[derive(Debug, Clone, Copy)]
pub struct TempController {
    state: ControllerState,
    threshold_temperature: i8,
    minimum_runtime: Duration,
    cooldown_time: Duration,
}

impl TempController {
    /// Creates a new temperature controller, starts off in Cooldown mode
    pub fn new(
        threshold_temperature: i8,
        minimum_runtime: Duration,
        cooldown_time: Duration,
    ) -> TempController {
        TempController {
            state: ControllerState::Cooldown {
                starttime: Instant::now(),
            },
            threshold_temperature,
            minimum_runtime,
            cooldown_time,
        }
    }

    pub fn update(&mut self, current_temperature: i8) -> bool {
        let current_time = Instant::now();

        match self.state {
            ControllerState::Idle => {
                if current_temperature > self.threshold_temperature {
                    self.state = ControllerState::Running {
                        starttime: Instant::now(),
                    };
                    true
                } else {
                    false
                }
            }
            ControllerState::Running { starttime } => {
                if current_time > (starttime + self.minimum_runtime) {
                    self.state = ControllerState::Cooldown {
                        starttime: Instant::now(),
                    };
                    true
                } else {
                    false
                }
            }
            ControllerState::Cooldown { starttime } => {
                if current_time > (starttime + self.cooldown_time) {
                    self.state = ControllerState::Idle;
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.state, ControllerState::Running { starttime: _ })
    }

    pub fn _is_idle(&self) -> bool {
        self.state == ControllerState::Idle
    }

    pub fn is_cooldown(&self) -> bool {
        matches!(self.state, ControllerState::Cooldown { starttime: _ })
    }

    pub fn get_state(&self) -> ControllerState {
        self.state
    }
}

#[embassy_executor::task]
pub async fn temp_controller_task(
    mut dht11_ctl: DHT11,
    controller: Mutex<NoopRawMutex, TempController>,
    relay_pin: impl Pin,
) -> ! {
    let mut relay_output = Output::new(relay_pin, Level::Low);

    loop {
        let (temperature, humidity) = dht11_ctl.get_temperature_humidity();

        SHARED_TEMP.store(temperature, Ordering::Relaxed);
        SHARED_HUMID.store(humidity, Ordering::Relaxed);

        let mut controller = controller.lock().await;
        let controller_state_change = controller.update(temperature);
        if controller_state_change && controller.is_running() {
            debug!("Setting Controller Relay");
            relay_output.set_high();
        } else if controller_state_change && controller.is_cooldown() {
            debug!("Unsetting Controller Relay");
            relay_output.set_low();
        }

        Timer::after_secs(1).await
    }
}
