use defmt::info;
use embassy_rp::{
    peripherals::PIO1,
    pio::{Config, Pio, PioPin, ShiftDirection, StateMachine},
};
use fixed::traits::ToFixed;

pub struct DHT11 {
    state_machine: StateMachine<'static, PIO1, 0>,
    config: Config<'static, PIO1>,
}

impl DHT11 {
    pub fn new<T: PioPin>(pio: Pio<'static, PIO1>, pin: T) -> Self {
        let prg = pio_proc::pio_file!("src/dht11.pio");

        let Pio {
            mut common, sm0, ..
        } = pio;

        let mut cfg = Config::default();
        cfg.use_program(&common.load_program(&prg.program), &[]);
        let mut data_pin = common.make_pio_pin(pin);
        data_pin.set_pull(embassy_rp::gpio::Pull::Up);
        cfg.set_set_pins(&[&data_pin]);
        cfg.set_in_pins(&[&data_pin]);
        cfg.set_jmp_pin(&data_pin);
        // x&y are set to 31 in the pio program, this helps initialize a loop of 1024*100 cycles which needs to add up to ~20ms according to whoever wrote it.
        // Cursory examination indicated a system clock frequency of 125MHz
        // 102400 cycles / 20ms = 5.12 MHz -> 125MHz/5.12MHz = 24.414, thus a clock divider of near to 24.414
        // cfg.clock_divider = 82.to_fixed();
        cfg.clock_divider = 125.to_fixed();
        cfg.shift_in.auto_fill = true;
        cfg.shift_in.threshold = 8;
        cfg.shift_in.direction = ShiftDirection::Left;

        DHT11 {
            state_machine: sm0,
            config: cfg,
        }
    }

    pub fn get_temperature(&mut self) -> i8 {
        self.state_machine.set_config(&self.config);
        self.state_machine.set_enable(true);
        // Timer::after_micros(5).await;

        let mut dht11_data_buf: [u32; 5] = [0; 5];
        for item in &mut dht11_data_buf {
            *item = self.state_machine.rx().pull();
        }
        info!(
            "Temperature {}°C, Humidity: {}%",
            dht11_data_buf[2], dht11_data_buf[0]
        );
        self.state_machine.restart();
        dht11_data_buf[2] as i8
    }
}
