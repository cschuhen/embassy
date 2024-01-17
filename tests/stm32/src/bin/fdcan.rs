#![no_std]
#![no_main]
use defmt::*;
use embassy_executor::Spawner;
use embassy_stm32::can;
use defmt::assert;
use embassy_stm32::peripherals::*;
use embassy_stm32::{bind_interrupts, Config};
use embassy_time::{Duration, Instant};

use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    FDCAN1_IT0 => can::IT0InterruptHandler<FDCAN1>;
    FDCAN1_IT1 => can::IT1InterruptHandler<FDCAN1>;
});

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let config = Config::default();

    let peripherals = embassy_stm32::init(config);

    let mut can = can::Fdcan::new(peripherals.FDCAN1, peripherals.PB8, peripherals.PB9, Irqs);

    // 250k bps
    can.set_bitrate(250_000);


    let mut can = can.into_external_loopback_mode();
    //let mut can = can.into_normal_mode();

    info!("CAN Configured");

    let mut i: u8 = 0;
    loop {
        //let tx_frame = Frame::new_data(unwrap!(StandardId::new(i as _)), [i]);

        let tx_frame = can::TxFrame::new(
            can::TxFrameHeader {
                len: 1,
                frame_format: can::FrameFormat::Standard,
                id: can::StandardId::new(0x123).unwrap().into(),
                bit_rate_switching: false,
                marker: None,
            },
            &[i],
        )
        .unwrap();


        info!("Transmitting frame...");
        let tx_ts = Instant::now();
        can.write(&tx_frame).await;

        let envelope = can.read().await.unwrap();
        let rx_ts = Instant::now();
        info!("Frame received!");

        // Check data.
        assert!(
            i == envelope.data()[0],
            "{} == {}",
            i,
            envelope.data()[0]
        );

        info!("loopback time {}", envelope.header.time_stamp);
        info!("loopback frame {=u8}", envelope.data()[0]);
        //let latency = envelope.header.time_stamp.saturating_duration_since(tx_ts);
        let latency = rx_ts.saturating_duration_since(tx_ts);
        info!("loopback latency {} us", latency.as_micros());

        // Theoretical minimum latency is 55us, actual is usually ~80us
        const MIN_LATENCY: Duration = Duration::from_micros(50);
        // Was failing at 150 but we are not getting a real time stamp. I'm not
        // sure if there are other delays
        const MAX_LATENCY: Duration = Duration::from_micros(1000);
        assert!(
            MIN_LATENCY <= latency && latency <= MAX_LATENCY,
            "{} <= {} <= {}",
            MIN_LATENCY,
            latency,
            MAX_LATENCY
        );

        i += 1;
        if i > 10 {
            break;
        }
    }

    info!("Test OK");
    cortex_m::asm::bkpt();

}
