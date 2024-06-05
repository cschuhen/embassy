#![no_std]
#![no_main]

use core::fmt::Write;
use core::str::from_utf8;

use cortex_m_rt::entry;
use defmt::*;
use embassy_executor::Executor;
use embassy_stm32::dma::NoDma;
use embassy_stm32::gpio::{Input, Level, Output, Pull, Speed};
use embassy_stm32::peripherals::SPI1;
use embassy_stm32::time::{mhz, Hertz};
use embassy_stm32::{spi, Config};
use embassy_time::{Delay, Duration, Ticker, Timer};
use embedded_graphics::fonts::{Font6x8, Text};
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Circle, Line, Rectangle};
use embedded_graphics::style::PrimitiveStyle;
use embedded_graphics::text_style;
use embedded_hal::blocking::delay::{DelayMs, DelayUs};
use embedded_hal_bus::spi::ExclusiveDevice;
use heapless::String;
use ssd1680::color::{Black, Red, White};
//use embedded_graphics_core::pixelcolor::BinaryColor;
use ssd1680::prelude::*;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

fn draw_rotation_and_rulers(display: &mut Display2in13) {
    display.set_rotation(DisplayRotation::Rotate0);
    draw_text(display, "rotation 0", 25, 25);
    draw_ruler(display);

    display.set_rotation(DisplayRotation::Rotate90);
    draw_text(display, "rotation 90", 25, 25);
    draw_ruler(display);

    display.set_rotation(DisplayRotation::Rotate180);
    draw_text(display, "rotation 180", 25, 25);
    draw_ruler(display);

    display.set_rotation(DisplayRotation::Rotate270);
    draw_text(display, "rotation 270", 25, 25);
    draw_ruler(display);
}

fn draw_ruler(display: &mut Display2in13) {
    for col in 1..ssd1680::WIDTH {
        if col % 25 == 0 {
            Line::new(Point::new(col as i32, 0), Point::new(col as i32, 10))
                .into_styled(PrimitiveStyle::with_stroke(Black, 1))
                .draw(display)
                .unwrap();
        }

        if col % 50 == 0 {
            //let label = col.to_string();

            draw_text(display, &"XX", col as i32, 12);
        }
    }
}

fn draw_text(display: &mut Display2in13, text: &str, x: i32, y: i32) {
    let _ = Text::new(text, Point::new(x, y))
        .into_styled(text_style!(
            font = Font6x8,
            text_color = Black,
            background_color = White
        ))
        .draw(display);
}

#[embassy_executor::task]
async fn main_task(
    mut spi: spi::Spi<'static, embassy_stm32::mode::Async>,
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
    let mut ssd1680 = Ssd1680::new(&mut spi, cs, busy, dc, rst, &mut delay).unwrap();

    // Clear frames on the display driver
    ssd1680.clear_red_frame(&mut spi).unwrap();
    ssd1680.clear_bw_frame(&mut spi).unwrap();

    // Create buffer for black and white
    let mut display_bw = Display2in13::bw();

    draw_rotation_and_rulers(&mut display_bw);

    display_bw.set_rotation(DisplayRotation::Rotate0);
    Rectangle::new(Point::new(60, 60), Point::new(100, 100))
        .into_styled(PrimitiveStyle::with_fill(Black))
        .draw(&mut display_bw)
        .unwrap();

    info!("Send bw frame to display");
    ssd1680.update_bw_frame(&mut spi, display_bw.buffer()).unwrap();

    // Draw red color
    let mut display_red = Display2in13::red();

    Circle::new(Point::new(100, 100), 20)
        .into_styled(PrimitiveStyle::with_fill(Red))
        .draw(&mut display_red)
        .unwrap();

    // println!("Send red frame to display");
    ssd1680.update_red_frame(&mut spi, display_red.buffer()).unwrap();

    info!("Update display");
    ssd1680.display_frame(&mut spi, &mut Delay).unwrap();

    info!("Done");

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
    spi_config.frequency = mhz(1);

    let busy = Input::new(p.PA1, Pull::Up);
    let cs = Output::new(p.PA2, Level::High, Speed::Low);
    let cs2 = Output::new(p.PA0, Level::High, Speed::Low);
    let dc = Output::new(p.PA3, Level::High, Speed::Low);
    let reset = Output::new(p.PA4, Level::High, Speed::Low);

    let spi = spi::Spi::new(p.SPI1, p.PA5, p.PA7, p.PA6, p.DMA1_CH1, p.DMA1_CH2, spi_config);

    let executor = EXECUTOR.init(Executor::new());

    executor.run(|spawner| {
        unwrap!(spawner.spawn(main_task(spi, busy, cs, dc, reset, cs2)));
    })
}
