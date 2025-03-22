#![no_std]
#![no_main]


use cortex_m_rt::entry;
use defmt::*;
use embassy_executor::Executor;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::time::{Hertz};
use embassy_stm32::{spi, Config};
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};
use embedded_storage_async::nor_flash::NorFlash;

pub type Spi = spi::Spi<'static, embassy_stm32::mode::Async>;

use w25q32jv::W25q32jv;
pub type OutputPin = Output<'static>;

pub type SpiDevice =
    embedded_hal_bus::spi::ExclusiveDevice<Spi, OutputPin, embedded_hal_bus::spi::NoDelay>;
pub type Flash = W25q32jv<SpiDevice, OutputPin, OutputPin>;


#[embassy_executor::task]
async fn main_task(mut flash: Flash,
) {

    flash.erase(0x00, 0x100).await;
    let mut data = [0x00; 10];
	//async fn write(      &mut self,      offset: u32,   bytes: &[u8]) -> Result<(), Self::Error>;
    //async fn write_async(&mut self, mut address: u32, mut buf: &[u8]) -> Result<(), Error<S, P>> {

    let ret = flash.write_async(0x00, &data).await;
    let ret = flash.write(0x00, &data).await;


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

    let flash = {
        let cs = Output::new(p.PB12, Level::High, Speed::Low);
        let wp = Output::new(p.PC6, Level::High, Speed::Low);
        let reset = Output::new(p.PB2, Level::High, Speed::Low);
        let sck = p.PB13;
        let miso = p.PB14;
        let mosi = p.PB15;
        let mut config = spi::Config::default();
        config.frequency = Hertz(16_000_000);

        let spi = embassy_stm32::spi::Spi::new(
            p.SPI2, sck, mosi, miso, p.DMA2_CH1, p.DMA2_CH2, config,
        );
        let spi: SpiDevice = embedded_hal_bus::spi::ExclusiveDevice::new_no_delay(spi, cs);

        // Create the flash driver instance
        W25q32jv::new(spi, reset, wp).unwrap()
    };


    let executor = EXECUTOR.init(Executor::new());

    executor.run(|spawner| {
        unwrap!(spawner.spawn(main_task(flash)));
    })
}
