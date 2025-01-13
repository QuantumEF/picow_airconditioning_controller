use defmt::*;
use embassy_rp::gpio::Output;
use embassy_time::{Duration, Instant};

#[derive(Debug, PartialEq, Format, Clone, Copy)]
pub enum ControllerState {
    Idle,
    Running { starttime: Instant },
    Cooldown { starttime: Instant },
}

#[derive(Debug, Clone, Copy)]
pub struct TempControllerConfig {
    pub threshold_temperature: i8,
    pub minimum_runtime: Duration,
    pub cooldown_time: Duration,
}

pub struct TempController<'a> {
    state: ControllerState,
    relay_output: Output<'a>,
    config: TempControllerConfig,
}

impl<'a> TempController<'a> {
    /// Creates a new temperature controller, starts off in Cooldown mode
    pub fn new(config: TempControllerConfig, relay_output: Output<'a>) -> TempController<'a> {
        TempController {
            state: ControllerState::Cooldown {
                starttime: Instant::now(),
            },
            relay_output,
            config,
        }
    }

    pub fn update(&mut self, current_temperature: i8) {
        let current_time = Instant::now();

        let controller_state_change = match self.state {
            ControllerState::Idle => {
                if current_temperature > self.config.threshold_temperature {
                    self.state = ControllerState::Running {
                        starttime: Instant::now(),
                    };
                    true
                } else {
                    false
                }
            }
            ControllerState::Running { starttime } => {
                if current_time > (starttime + self.config.minimum_runtime) {
                    self.state = ControllerState::Cooldown {
                        starttime: Instant::now(),
                    };
                    true
                } else {
                    false
                }
            }
            ControllerState::Cooldown { starttime } => {
                if current_time > (starttime + self.config.cooldown_time) {
                    self.state = ControllerState::Idle;
                    true
                } else {
                    false
                }
            }
        };

        if controller_state_change && self.is_running() {
            debug!("Setting Controller Relay");
            self.relay_output.set_high();
        } else if controller_state_change && self.is_cooldown() {
            debug!("Unsetting Controller Relay");
            self.relay_output.set_low();
        };
    }

    pub fn update_config(&mut self, config: TempControllerConfig) {
        self.config = config;
    }

    pub fn get_config(&self) -> TempControllerConfig {
        self.config
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
