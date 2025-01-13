//! This example uses the RP Pico W board Wifi chip (cyw43).
//! Connects to specified Wifi network and creates a TCP endpoint on port 1234.

#![no_std]
#![no_main]
#![allow(async_fn_in_trait)]
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_sync::watch::Watch;
use heapless::String;

use cyw43_pio::PioSpi;
use defmt::*;
use embassy_executor::Spawner;
use embassy_net::tcp::TcpSocket;
use embassy_net::{Config as IPConfig, Stack, StackResources};
use embassy_rp::clocks::clk_sys_freq;
use embassy_rp::gpio::{Level, Output, Pin};
use embassy_rp::peripherals::{DMA_CH0, PIO0, PIO1, UART0};
use embassy_rp::pio::{InterruptHandler as PIOInterruptHandler, Pio};
use embassy_rp::{
    bind_interrupts,
    uart::{self, InterruptHandler as UARTInterruptHandler},
};
use embassy_time::{Duration, Timer};
use embedded_io_async::Write;

use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

mod dht11;
mod temp_controller;
use dht11::DHT11;
use temp_controller::{ControllerState, TempController, TempControllerConfig};
mod uart_cli;
use uart_cli::uart_cli;

bind_interrupts!(struct PIOIrqs {
    PIO0_IRQ_0 => PIOInterruptHandler<PIO0>;
    PIO1_IRQ_0 => PIOInterruptHandler<PIO1>;
});

bind_interrupts!(struct UARTIrqs {
    UART0_IRQ  => UARTInterruptHandler<UART0>;
});

const WIFI_NETWORK: &str = include_str!("wifi_network");
const WIFI_PASSWORD: &str = include_str!("wifi_password");

static DHT11_WATCH: Watch<CriticalSectionRawMutex, (i8, i8), 4> = Watch::new();

static CONTROLLER_UPDATE_CONFIG: Signal<CriticalSectionRawMutex, TempControllerConfig> =
    Signal::new();
static CONTROLLER_CURRENT_STATUS: Signal<
    CriticalSectionRawMutex,
    (ControllerState, TempControllerConfig),
> = Signal::new();

#[embassy_executor::task]
async fn wifi_task(
    runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(stack: &'static Stack<cyw43::NetDriver<'static>>) -> ! {
    stack.run().await
}

#[embassy_executor::task]
async fn temp_monitor_task(mut dht11_ctl: DHT11) {
    let dht11_monitor = DHT11_WATCH.sender();

    // Since I think the first few readings are garbage, let's just throw them away
    let _ = dht11_ctl.get_temperature_humidity();
    Timer::after_secs(1).await;
    let _ = dht11_ctl.get_temperature_humidity();

    loop {
        Timer::after_secs(1).await;
        let temp_humid = dht11_ctl.get_temperature_humidity();

        dht11_monitor.send(temp_humid);
    }
}

#[embassy_executor::task]
async fn temp_controller(relay_pin: impl Pin) {
    let mut dht11_controller_reciever = DHT11_WATCH.receiver().unwrap();

    let mut controller = TempController::new(
        temp_controller::TempControllerConfig {
            threshold_temperature: 20,
            minimum_runtime: Duration::from_secs(10),
            cooldown_time: Duration::from_secs(10),
        },
        Output::new(relay_pin, Level::Low),
    );

    loop {
        let (temperature, _) = dht11_controller_reciever.get().await;
        controller.update(temperature);

        CONTROLLER_CURRENT_STATUS.signal((controller.get_state(), controller.get_config()));

        if let Some(new_config) = CONTROLLER_UPDATE_CONFIG.try_take() {
            controller.update_config(new_config);
        }
        Timer::after_secs(1).await;
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    info!("Hello World! {}", clk_sys_freq());

    let p = embassy_rp::init(Default::default());

    let mut dht11_tcp_reciever = DHT11_WATCH.receiver().unwrap();

    let config = uart::Config::default();
    let uart = uart::Uart::new(
        p.UART0, p.PIN_0, p.PIN_1, UARTIrqs, p.DMA_CH1, p.DMA_CH2, config,
    );

    // let fw = include_bytes!("../cyw43-firmware/43439A0.bin");
    // let clm = include_bytes!("../cyw43-firmware/43439A0_clm.bin");

    // To make flashing faster for development, you may want to flash the firmwares independently
    // at hardcoded addresses, instead of baking them into the program with `include_bytes!`:
    //     probe-rs download 43439A0.bin --format bin --chip RP2040 --base-address 0x10100000
    //     probe-rs download 43439A0_clm.bin --format bin --chip RP2040 --base-address 0x10140000
    let fw = unsafe { core::slice::from_raw_parts(0x10100000 as *const u8, 230321) };
    let clm = unsafe { core::slice::from_raw_parts(0x10140000 as *const u8, 4752) };

    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs = Output::new(p.PIN_25, Level::High);
    let pio1 = Pio::new(p.PIO1, PIOIrqs);

    let mut pio0 = Pio::new(p.PIO0, PIOIrqs);
    let spi = PioSpi::new(
        &mut pio0.common,
        pio0.sm0,
        pio0.irq0,
        cs,
        p.PIN_24,
        p.PIN_29,
        p.DMA_CH0,
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());
    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw).await;
    unwrap!(spawner.spawn(wifi_task(runner)));

    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;

    let config = IPConfig::dhcpv4(Default::default());
    //let config = embassy_net::Config::ipv4_static(embassy_net::StaticConfigV4 {
    //    address: Ipv4Cidr::new(Ipv4Address::new(192, 168, 69, 2), 24),
    //    dns_servers: Vec::new(),
    //    gateway: Some(Ipv4Address::new(192, 168, 69, 1)),
    //});

    // Generate random seed
    let seed = 0x0123_4567_89ab_cdef; // chosen by fair dice roll. guarenteed to be random.

    // Init network stack
    static STACK: StaticCell<Stack<cyw43::NetDriver<'static>>> = StaticCell::new();
    static RESOURCES: StaticCell<StackResources<2>> = StaticCell::new();
    let stack = &*STACK.init(Stack::new(
        net_device,
        config,
        RESOURCES.init(StackResources::<2>::new()),
        seed,
    ));

    unwrap!(spawner.spawn(uart_cli(uart, stack)));

    unwrap!(spawner.spawn(net_task(stack)));

    loop {
        //control.join_open(WIFI_NETWORK).await;
        match control.join_wpa2(WIFI_NETWORK, WIFI_PASSWORD).await {
            Ok(_) => break,
            Err(err) => {
                info!("join failed with status={}", err.status);
            }
        }
    }

    // Wait for DHCP, not necessary when using static IP
    info!("waiting for DHCP...");
    while !stack.is_config_up() {
        Timer::after_millis(100).await;
    }
    info!("DHCP is now up!");

    // And now we can use it!

    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];
    let mut buf = [0; 4096];

    let mut output_string = String::<64>::new();
    let mut temperature_buffer = itoa::Buffer::new();
    let mut humidity_buffer = itoa::Buffer::new();

    unwrap!(spawner.spawn(temp_controller(p.PIN_13)));

    let dht11_ctl = DHT11::new(pio1, p.PIN_15);
    unwrap!(spawner.spawn(temp_monitor_task(dht11_ctl)));
    info!("DHT11 initialized");

    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(Duration::from_secs(10)));

        control.gpio_set(0, false).await;
        info!("Listening on TCP:1234...");
        if let Err(e) = socket.accept(1234).await {
            warn!("accept error: {:?}", e);
            continue;
        }

        info!("Received connection from {:?}", socket.remote_endpoint());
        control.gpio_set(0, true).await;

        loop {
            let _ = match socket.read(&mut buf).await {
                Ok(0) => {
                    warn!("read EOF");
                    break;
                }
                Ok(n) => n,
                Err(e) => {
                    warn!("read error: {:?}", e);
                    break;
                }
            };

            let (temperature, humidity) = dht11_tcp_reciever.get().await;
            let temperature_str = temperature_buffer.format(temperature);
            let humidity_str = humidity_buffer.format(humidity);
            output_string.clear();
            let _ = output_string.push_str(temperature_str);
            let _ = output_string.push(',');
            let _ = output_string.push_str(humidity_str);
            let _ = output_string.push('\n');

            match socket.write_all(output_string.as_bytes()).await {
                Ok(()) => {}
                Err(e) => {
                    warn!("write error: {:?}", e);
                    break;
                }
            };
        }
    }
}
