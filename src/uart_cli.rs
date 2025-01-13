use core::fmt::Write;
use cyw43::NetDriver;
use defmt::{debug, error};
use embassy_net::Stack;
use embassy_rp::{
    peripherals::UART0,
    uart::{self, Async, Uart, UartTx},
};
use embassy_time::{Duration, Instant};
use embedded_cli::{
    cli::{CliBuilder, CliHandle},
    Command,
};
use embedded_io::ErrorType;

use crate::{
    temp_controller::{ControllerState, TempControllerConfig},
    CONTROLLER_CURRENT_STATUS, CONTROLLER_UPDATE_CONFIG, DHT11_WATCH,
};

#[derive(Debug, Command)]
enum BaseCommand {
    Temp,
    Addr,
    Status,
    GetConfig,
    SetConfig {
        set_temp: Option<i8>,
        min_runtime_secs: Option<u64>,
        min_cooldown_secs: Option<u64>,
    },
}

/// Wrapper around usart so we can impl embedded_io::Write
/// which is required for cli
struct Writer(UartTx<'static, UART0, Async>);

impl ErrorType for Writer {
    type Error = uart::Error;
}

impl embedded_io::Write for Writer {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.0.blocking_write(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.0.blocking_flush()?;
        Ok(())
    }
}

#[embassy_executor::task]
pub async fn uart_cli(
    uart: Uart<'static, UART0, Async>,
    network_stack: &'static Stack<NetDriver<'static>>,
) -> ! {
    let (command_buffer, history_buffer) = unsafe {
        static mut COMMAND_BUFFER: [u8; 32] = [0; 32];
        static mut HISTORY_BUFFER: [u8; 32] = [0; 32];
        (COMMAND_BUFFER.as_mut(), HISTORY_BUFFER.as_mut())
    };

    let (tx, mut rx) = uart.split();

    let term_writer = Writer(tx);
    let mut cli = CliBuilder::default()
        .writer(term_writer)
        .command_buffer(command_buffer)
        .history_buffer(history_buffer)
        .build()
        .ok()
        .unwrap();

    let mut controller_state = CONTROLLER_CURRENT_STATUS.wait().await;

    let mut dht11_monitor = DHT11_WATCH.receiver().unwrap();

    loop {
        let mut buffer = [0; 1];

        let (temperature, humidity) = dht11_monitor.get().await;
        match rx.read(&mut buffer).await {
            Ok(()) => {
                for byte in buffer {
                    let _ = cli.process_byte::<BaseCommand, _>(
                        byte,
                        &mut BaseCommand::processor(
                            |cli: &mut CliHandle<'_, Writer, uart::Error>, command| match command {
                                BaseCommand::Temp => {
                                    write!(
                                        cli.writer(),
                                        "Temp: {}°C\nHumidity: {}%",
                                        temperature,
                                        humidity,
                                    )
                                    .unwrap();
                                    Ok(())
                                }
                                BaseCommand::Addr => {
                                    match network_stack.config_v4().map(|x| x.address) {
                                        Some(addr) => write!(cli.writer(), "{}", addr).unwrap(),
                                        None => {
                                            write!(cli.writer(), "No Address Assigned").unwrap()
                                        }
                                    }
                                    Ok(())
                                }
                                BaseCommand::Status => {
                                    if let Some(changed_state) =
                                        CONTROLLER_CURRENT_STATUS.try_take()
                                    {
                                        controller_state = changed_state;
                                    };

                                    match controller_state.0 {
                                        ControllerState::Idle => {
                                            write!(cli.writer(), "Status: Idle",).unwrap()
                                        }
                                        ControllerState::Running { starttime } => {
                                            let time_remaining = controller_state.1.minimum_runtime
                                                - (Instant::now() - starttime);
                                            write!(
                                                cli.writer(),
                                                "Status: Running - Remaining: {}s",
                                                time_remaining.as_secs()
                                            )
                                            .unwrap()
                                        }
                                        ControllerState::Cooldown { starttime } => {
                                            let time_remaining = controller_state.1.cooldown_time
                                                - (Instant::now() - starttime);
                                            write!(
                                                cli.writer(),
                                                "Status: Cooldown - Remaining: {}s",
                                                time_remaining.as_secs()
                                            )
                                            .unwrap()
                                        }
                                    }
                                    Ok(())
                                }
                                BaseCommand::GetConfig => {
                                    if let Some(changed_state) =
                                    CONTROLLER_CURRENT_STATUS.try_take()
                                {
                                    controller_state = changed_state;
                                };
                                    let config = controller_state.1;
                                    write!(
                                        cli.writer(),
                                        "Threshold Temp: {}°C\nMin Runtime: {}s\nCooldown Time: {}s",
                                        config.threshold_temperature,
                                        config.minimum_runtime.as_secs(),
                                        config.cooldown_time.as_secs(),
                                    )
                                    .unwrap();
                                    Ok(())
                                }
                                BaseCommand::SetConfig {
                                    set_temp,
                                    min_runtime_secs,
                                    min_cooldown_secs,
                                } => {
                                    let new_config = TempControllerConfig {
                                        threshold_temperature: set_temp
                                            .unwrap_or(controller_state.1.threshold_temperature),
                                        minimum_runtime: Duration::from_secs(
                                            min_runtime_secs.unwrap_or(
                                                controller_state.1.minimum_runtime.as_secs(),
                                            ),
                                        ),
                                        cooldown_time: Duration::from_secs(
                                            min_cooldown_secs.unwrap_or(
                                                controller_state.1.cooldown_time.as_secs(),
                                            ),
                                        ),
                                    };
                                    CONTROLLER_UPDATE_CONFIG.signal(new_config);
                                    Ok(())
                                }
                            },
                        ),
                    );
                }
            }
            Err(err) => error!("UART Error: {:?}", err),
        }
        debug!("Byte: {}", buffer);
    }
}
