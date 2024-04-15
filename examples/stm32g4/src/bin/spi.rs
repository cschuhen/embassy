#![no_std]
#![no_main]

use core::fmt::Write;
use core::str::from_utf8;

use cortex_m_rt::entry;
use defmt::*;
//use embedded_graphics_core::pixelcolor::BinaryColor;
use display_interface_spi::SPIInterface;
use embassy_executor::Executor;
use embassy_stm32::dma::NoDma;
use embassy_stm32::gpio::{Input, Level, Output, Pull, Speed};
use embassy_stm32::peripherals::SPI1;
use embassy_stm32::time::{mhz, Hertz};
use embassy_stm32::{spi, Config};
use embassy_time::{Delay, Duration, Ticker, Timer};
use embedded_graphics::mono_font::ascii::FONT_10X20;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::{BinaryColor, Rgb565};
use embedded_graphics::prelude::*;
use embedded_graphics::text::Text;
use embedded_graphics_core::draw_target::DrawTarget;
use embedded_hal::blocking::delay::{DelayMs, DelayUs};
use embedded_hal_bus::spi::ExclusiveDevice;
use heapless::String;
//use mipidsi::Builder;
// Display
use ssd1309::mode::graphics::*;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};
#[embassy_executor::task]
async fn main_task(
    mut spi: spi::Spi<'static, SPI1, NoDma, NoDma>,
    busy: Input<'static>,
    cs: Output<'static>,
    dc: Output<'static>,
    rst: Output<'static>,
    cs2: Output<'static>,
) {
    //let spidev = ExclusiveDevice::new(spi, cs, Delay);
    let mut delay = Delay {};

    use core::cell::RefCell;

    use embassy_embedded_hal::shared_bus::blocking::spi::SpiDeviceWithConfig;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use embassy_sync::blocking_mutex::Mutex;
    use mipidsi::options::{
        ColorInversion, ColorOrder, HorizontalRefreshOrder, Orientation, RefreshOrder, VerticalRefreshOrder,
    };
    use mipidsi::Builder;

    let spi_bus: Mutex<NoopRawMutex, _> = Mutex::new(RefCell::new(spi));

    let mut display_config = spi::Config::default();
    display_config.frequency = mhz(1);
    //display_config.phase = spi::Phase::CaptureOnSecondTransition;
    //display_config.polarity = spi::Polarity::IdleHigh;

    let display_spi = SpiDeviceWithConfig::new(&spi_bus, cs, display_config);
    use display_interface_spi::SPIInterface;

    let di = SPIInterface::new(display_spi, dc);

    #[cfg(feature = "st7735s")]
    let (mut display, W, H) = {
        const W: i32 = 128;
        const H: i32 = 160;

        let display = Builder::new(mipidsi::models::ST7735s, di)
            .reset_pin(rst)
            //.refresh_order(RefreshOrder::new(
            //    VerticalRefreshOrder::BottomToTop,
            //    HorizontalRefreshOrder::RightToLeft,
            //))
            .invert_colors(ColorInversion::Inverted)
            .color_order(ColorOrder::Bgr)
            .display_size(W as u16, H as u16) // w, h
            .init(&mut Delay)
            .unwrap();
        (display, W, H)
    };

    #[cfg(feature = "st7789")]
    let (mut display, W, H) = {
        const W: i32 = 240;
        const H: i32 = 320;

        let display = Builder::new(mipidsi::models::ST7789, di)
            .reset_pin(rst)
            //.refresh_order(RefreshOrder::new(
            //    VerticalRefreshOrder::BottomToTop,
            //    HorizontalRefreshOrder::RightToLeft,
            //))
            .invert_colors(ColorInversion::Inverted)
            .color_order(ColorOrder::Bgr)
            .display_size(W as u16, H as u16) // w, h
            .init(&mut Delay)
            .unwrap();
        (display, W, H)
    };

    #[cfg(feature = "ssd1309")]
    let (mut display, W, H) = {
        const W: i32 = 128;
        const H: i32 = 64;
        let mut display: GraphicsMode<_> = ssd1309::Builder::new().connect(di).into();
        let mut rst = rst;

        _ = display.reset(&mut rst, &mut Delay);
        display.init().unwrap();
        display.flush().unwrap();
        (display, W, H)
    };

    // Text
    let char_w = 10;
    let char_h = 20;
    let text = "   Hello World ^_^;   ";
    let mut text_x = W;
    let mut text_y = H / 2;

    // Alternating color

    #[cfg(any(feature = "ssd1309"))]
    {
        let mut text_style = MonoTextStyle::new(&FONT_10X20, BinaryColor::On);
        text_style.background_color = Some(BinaryColor::Off);
        loop {
            Timer::after_millis(100).await;

            // Draw text
            let right = Text::new(text, Point::new(text_x, text_y), text_style)
                .draw(&mut display)
                .unwrap();
            //println!("T {} {}", text_x, text_y);
            text_x = if right.x <= 0 { W } else { text_x - char_w };
            display.flush().unwrap();
        }

        // Turn off backlight and clear the display
        //backlight.set_low();
        //display.clear(BinaryColor::Off).unwrap();
    }

    #[cfg(any(feature = "st7789", feature = "st7735s"))]
    {
        let colors = [Rgb565::RED, Rgb565::GREEN, Rgb565::BLUE];
        let mut text_style = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
        text_style.background_color = Some(Rgb565::CSS_BURLY_WOOD);
        // Clear the display initially
        display.clear(colors[0]).unwrap();

        let mut led_flags = 0b000;
        let mut counter = 0;
        loop {
            Timer::after_millis(100).await;
            counter += 1;

            // Draw text
            let right = Text::new(text, Point::new(text_x, text_y), text_style)
                .draw(&mut display)
                .unwrap();
            text_x = if right.x <= 0 { W } else { text_x - char_w };
        }

        // Turn off backlight and clear the display
        //backlight.set_low();
        display.clear(Rgb565::BLACK).unwrap();
    }

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

    let spi = spi::Spi::new(p.SPI1, p.PA5, p.PA7, p.PA6, NoDma, NoDma, spi_config);

    let executor = EXECUTOR.init(Executor::new());

    executor.run(|spawner| {
        unwrap!(spawner.spawn(main_task(spi, busy, cs, dc, reset, cs2)));
    })
}
