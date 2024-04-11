#![no_std]
#![no_main]

use core::fmt::Write;
use core::str::from_utf8;

use cortex_m_rt::entry;
use defmt::*;
use embassy_executor::Executor;
use embassy_stm32::dma::NoDma;
use embassy_stm32::peripherals::SPI1;
use embassy_stm32::time::mhz;
use embassy_stm32::{spi, Config};
use embassy_stm32::time::Hertz;
use embedded_hal_bus::spi::ExclusiveDevice;
use heapless::String;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};
use embassy_stm32::gpio::{Level, Output, Speed, Input, Pull};
use embassy_time::{Delay, Duration, Ticker, Timer};
use embedded_hal::blocking::delay::DelayUs;
use embedded_hal::blocking::delay::DelayMs;
//use embedded_graphics_core::pixelcolor::BinaryColor;
use display_interface_spi::SPIInterface;
use embedded_graphics::{
    mono_font::{ascii::FONT_10X20, MonoTextStyle},
    pixelcolor::Rgb565,
    prelude::*,
    text::Text,
};
use mipidsi::Builder;
// Display
const W: i32 = 160;
const H: i32 = 128;


#[embassy_executor::task]
async fn main_task(mut spi: spi::Spi<'static, SPI1, NoDma, NoDma>,
        busy: Input<'static>,
        cs: Output<'static>,
        dc: Output<'static>,
        rst: Output<'static>,
        cs2: Output<'static>,
    ) {
    //let spidev = ExclusiveDevice::new(spi, cs, Delay);
    let mut delay = Delay {};

    //let mut epd = Epd2in9::new(&mut spi, cs, busy, dc, reset, &mut delay).expect("eink initalize error");
    //let mut spi = SpiDeviceDriver::new(spi, Option::<AnyIOPin>::None, &Config::new()).unwrap();

    // Initialise display controller
    //let mut ssd1680 = Ssd1680::new(&mut spi, cs, busy, dc, rst, &mut delay).unwrap();

    // SPI Display
    //let spi = Spi::new(Bus::Spi0, SlaveSelect::Ss1, 60_000_000_u32, Mode::Mode0).unwrap();
    let di = SPIInterface::new(spi, dc, cs);
    //let mut delay = Delay::new();
    let mut display = Builder::st7735s(di)
        // width and height are switched on purpose because of the orientation
        .with_display_size(H as u16, W as u16)
        // this orientation applies for the Display HAT Mini by Pimoroni
        .with_orientation(mipidsi::Orientation::LandscapeInverted(true))
        .with_invert_colors(mipidsi::ColorInversion::Inverted)
        .init(&mut Delay, Some(rst))
        .unwrap();
    // Text
    let char_w = 10;
    let char_h = 20;
    let mut text_style = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
    let text = "   Hello World ^_^;   ";
    let mut text_x = W;
    let mut text_y = H / 2;

    text_style.background_color = Some(Rgb565::CSS_BURLY_WOOD);
    // Alternating color
    let colors = [Rgb565::RED, Rgb565::GREEN, Rgb565::BLUE];

    // Clear the display initially
    display.clear(colors[0]).unwrap();

    //let start = std::time::Instant::now();
    //let mut last = std::time::Instant::now();
    let mut led_flags = 0b000;
    let mut counter = 0;
    loop {
        Timer::after_millis(100).await;
        //let elapsed = last.elapsed().as_secs_f64();
        //if elapsed < 0.125 {
        //    continue;
        //}
        //last = std::time::Instant::now();
        counter += 1;

        // X: move text up
        /*if button_x.is_low() {
            text_y -= char_h;
        }
        // Y: move text down
        if button_y.is_low() {
            text_y += char_h;
        }
        // A: change led color
        if button_a.is_low() {
            led_flags = (led_flags + 1) % 8;
        }
        // B: exit
        if button_b.is_low() {
            break;
        }*/

        // Fill the display with alternating colors every 8 frames
        //display.clear(colors[(counter / 8) % colors.len()]).unwrap();
        //let text_style = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE)
        //text_style.background_color = Some(colors[(counter / 8) % colors.len()]);

        // Draw text
        let right = Text::new(text, Point::new(text_x, text_y), text_style)
            .draw(&mut display)
            .unwrap();
        text_x = if right.x <= 0 { W } else { text_x - char_w };

        // Led
        /*
        let y = ((start.elapsed().as_secs_f64().sin() + 1.) * 50.).round() / 100.;
        led_r
            .set_pwm_frequency(50., if led_flags & 0b100 != 0 { y } else { 1. })
            .unwrap();
        led_g
            .set_pwm_frequency(50., if led_flags & 0b010 != 0 { y } else { 1. })
            .unwrap();
        led_b
            .set_pwm_frequency(50., if led_flags & 0b001 != 0 { y } else { 1. })
            .unwrap();
         */
    }

    // Turn off backlight and clear the display
    //backlight.set_low();
    display.clear(Rgb565::BLACK).unwrap();

    loop {
        println!("Finished tests - going to sleep");
        Timer::after_millis(1000).await;

    }

}

static EXECUTOR: StaticCell<Executor> = StaticCell::new();

#[entry]
fn main() -> ! {
    info!("Hello World!");
    let mut config = Config::default();
    {
        use embassy_stm32::rcc::*;
        config.rcc.hse = Some(Hse {
            freq: Hertz(8_000_000),
            mode: HseMode::Oscillator,
        });
        config.rcc.pll = Some(Pll {
            source: PllSource::HSE,
            prediv: PllPreDiv::DIV2,
            mul: PllMul::MUL85,
            divp: None,
            divq: Some(PllQDiv::DIV8), // 42.5 Mhz for fdcan.
            divr: Some(PllRDiv::DIV2), // Main system clock at 170 MHz
        });
        config.rcc.mux.fdcansel = mux::Fdcansel::PLL1_Q;
        config.rcc.sys = Sysclk::PLL1_R;
    }
    let p = embassy_stm32::init(config);



    let mut spi_config = spi::Config::default();
    spi_config.frequency = mhz(50);

    let busy = Input::new(p.PA1, Pull::Up);
    let cs = Output::new(p.PA2, Level::High, Speed::Low);
    let cs2 = Output::new(p.PA0, Level::High, Speed::Low);
    let dc = Output::new(p.PA3, Level::High, Speed::Low);
    let reset = Output::new(p.PA4, Level::High, Speed::Low);

    let spi = spi::Spi::new(p.SPI1, p.PA5, p.PA7, p.PA6, NoDma, NoDma, spi_config);

    let executor = EXECUTOR.init(Executor::new());

    executor.run(|spawner| {
        unwrap!(spawner.spawn(main_task(spi, busy, cs, dc, reset, cs2)));
    })
}
