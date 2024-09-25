use core::{fmt::Write, sync::atomic::Ordering};
use cyw43::NetDriver;
use defmt::{debug, error};
use embassy_net::Stack;
use embassy_rp::{
    peripherals::UART0,
    uart::{self, Async, Uart, UartTx},
};
use embassy_time::Instant;
use embedded_cli::{
    cli::{CliBuilder, CliHandle},
    Command,
};
use embedded_io::ErrorType;

use crate::temp_controller::{ControllerState, TempController, SHARED_HUMID, SHARED_TEMP};

#[derive(Debug, Command)]
enum BaseCommand {
    Temp,
    Addr,
    Status,
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
    controller: *mut TempController,
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

    // Safety: I don't care about race conditions.
    let controller = unsafe {
        controller
            .as_mut()
            .expect("This pointer should be initialized")
    };

    loop {
        let mut buffer = [0; 1];
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
                                        SHARED_TEMP.load(Ordering::Relaxed),
                                        SHARED_HUMID.load(Ordering::Relaxed)
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
                                    match controller.get_state() {
                                        ControllerState::Idle => {
                                            write!(cli.writer(), "Status: Idle",).unwrap()
                                        }
                                        ControllerState::Running { starttime } => {
                                            let time_remaining = controller.minimum_runtime
                                                - (Instant::now() - starttime);
                                            write!(
                                                cli.writer(),
                                                "Status: Running - Remaining: {}s",
                                                time_remaining.as_secs()
                                            )
                                            .unwrap()
                                        }
                                        ControllerState::Cooldown { starttime } => {
                                            let time_remaining = controller.cooldown_time
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
