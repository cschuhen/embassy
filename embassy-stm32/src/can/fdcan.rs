use core::cell::{RefCell, RefMut};
use core::future::poll_fn;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::task::Poll;

use cfg_if::cfg_if;
use embassy_hal_internal::{into_ref, PeripheralRef};
use fdcan;
pub use fdcan::frame::{FrameFormat, RxFrameInfo, TxFrameHeader};
pub use fdcan::id::{ExtendedId, Id, StandardId};
use fdcan::message_ram::RegisterBlock;
use fdcan::LastErrorCode;
pub use fdcan::{config, filter};

use crate::gpio::sealed::AFType;
use crate::interrupt::typelevel::Interrupt;
use crate::rcc::RccPeripheral;
use crate::{interrupt, peripherals, Peripheral};

pub struct RxFrame {
    pub header: RxFrameInfo,
    pub data: Data,
}

pub struct TxFrame {
    pub header: TxFrameHeader,
    pub data: Data,
}

impl TxFrame {
    pub fn new(header: TxFrameHeader, data: &[u8]) -> Option<Self> {
        if data.len() < header.len as usize {
            return None;
        }

        let Some(data) = Data::new(data) else { return None };

        Some(TxFrame { header, data })
    }

    fn from_preserved(header: TxFrameHeader, data32: &[u32]) -> Option<Self> {
        let mut data = [0u8; 64];

        for i in 0..data32.len() {
            data[4 * i..][..4].copy_from_slice(&data32[i].to_le_bytes());
        }

        let Some(data) = Data::new(&data) else { return None };

        Some(TxFrame { header, data })
    }

    pub fn data(&self) -> &[u8] {
        &self.data.bytes[..(self.header.len as usize)]
    }
}
impl RxFrame {
    pub(crate) fn new(header: RxFrameInfo, data: &[u8]) -> Self {
        let data = Data::new(&data).unwrap_or_else(|| Data::empty());

        RxFrame { header, data }
    }

    pub fn data(&self) -> &[u8] {
        &self.data.bytes[..(self.header.len as usize)]
    }
}

/// Payload of a (FD)CAN data frame.
///
/// Contains 0 to 64 Bytes of data.
#[derive(Debug, Copy, Clone)]
pub struct Data {
    pub(crate) bytes: [u8; 64],
}

impl Data {
    /// Creates a data payload from a raw byte slice.
    ///
    /// Returns `None` if `data` is more than 64 bytes (which is the maximum) or
    /// cannot be represented with an FDCAN DLC.
    pub fn new(data: &[u8]) -> Option<Self> {
        if !Data::is_valid_len(data.len()) {
            return None;
        }

        let mut bytes = [0; 64];
        bytes[..data.len()].copy_from_slice(data);

        Some(Self { bytes })
    }

    pub fn raw(&self) -> &[u8] {
        &self.bytes
    }

    /// Checks if the length can be encoded in FDCAN DLC field.
    pub const fn is_valid_len(len: usize) -> bool {
        match len {
            0..=8 => true,
            12 => true,
            16 => true,
            20 => true,
            24 => true,
            32 => true,
            48 => true,
            64 => true,
            _ => false,
        }
    }

    /// Creates an empty data payload containing 0 bytes.
    #[inline]
    pub const fn empty() -> Self {
        Self { bytes: [0; 64] }
    }
}

/// Interrupt handler.
pub struct IT0InterruptHandler<T: Instance> {
    _phantom: PhantomData<T>,
}

// We use IT0 for everything currently
impl<T: Instance> interrupt::typelevel::Handler<T::IT0Interrupt> for IT0InterruptHandler<T> {
    unsafe fn on_interrupt() {
        let regs = T::regs();

        let ir = regs.ir().read();

        if ir.tc() {
            regs.ir().write(|w| w.set_tc(true));
            T::state().tx_waker.wake();
        }

        if ir.tefn() {
            regs.ir().write(|w| w.set_tefn(true));
            T::state().tx_waker.wake();
        }

        if ir.ped() || ir.pea() {
            regs.ir().write(|w| {
                w.set_ped(true);
                w.set_pea(true);
            });
        }

        if ir.rfn(0) {
            regs.ir().write(|w| w.set_rfn(0, true));
            T::state().rx_waker.wake();
        }

        if ir.rfn(1) {
            regs.ir().write(|w| w.set_rfn(1, true));
            T::state().rx_waker.wake();
        }
    }
}

pub struct IT1InterruptHandler<T: Instance> {
    _phantom: PhantomData<T>,
}

impl<T: Instance> interrupt::typelevel::Handler<T::IT1Interrupt> for IT1InterruptHandler<T> {
    unsafe fn on_interrupt() {}
}

#[derive(Debug)]
pub enum BusError {
    Stuff,
    Form,
    Acknowledge,
    BitRecessive,
    BitDominant,
    Crc,
    Software,
    BusOff,
    BusPassive,
    BusWarning,
}

impl BusError {
    fn try_from(lec: LastErrorCode) -> Option<BusError> {
        match lec {
            LastErrorCode::AckError => Some(BusError::Acknowledge),
            LastErrorCode::Bit0Error => Some(BusError::BitRecessive), // TODO: verify
            LastErrorCode::Bit1Error => Some(BusError::BitDominant),  // TODO: verify
            LastErrorCode::CRCError => Some(BusError::Crc),
            LastErrorCode::FormError => Some(BusError::Form),
            LastErrorCode::StuffError => Some(BusError::Stuff),
            _ => None,
        }
    }
}

pub trait FdcanOperatingMode {}
impl FdcanOperatingMode for fdcan::PoweredDownMode {}
impl FdcanOperatingMode for fdcan::ConfigMode {}
impl FdcanOperatingMode for fdcan::InternalLoopbackMode {}
impl FdcanOperatingMode for fdcan::ExternalLoopbackMode {}
impl FdcanOperatingMode for fdcan::NormalOperationMode {}
impl FdcanOperatingMode for fdcan::RestrictedOperationMode {}
impl FdcanOperatingMode for fdcan::BusMonitoringMode {}
impl FdcanOperatingMode for fdcan::TestMode {}

fn calc_fdcan_timings(periph_clock: crate::time::Hertz, can_bitrate: u32) -> Option<config::NominalBitTiming> {
    const BS1_MAX: u8 = 16;
    const BS2_MAX: u8 = 8;
    const MAX_SAMPLE_POINT_PERMILL: u16 = 900;

    let periph_clock = periph_clock.0;

    if can_bitrate < 1000 {
        return None;
    }

    // Ref. "Automatic Baudrate Detection in CANopen Networks", U. Koppe, MicroControl GmbH & Co. KG
    //      CAN in Automation, 2003
    //
    // According to the source, optimal quanta per bit are:
    //   Bitrate        Optimal Maximum
    //   1000 kbps      8       10
    //   500  kbps      16      17
    //   250  kbps      16      17
    //   125  kbps      16      17
    let max_quanta_per_bit: u8 = if can_bitrate >= 1_000_000 { 10 } else { 17 };

    // Computing (prescaler * BS):
    //   BITRATE = 1 / (PRESCALER * (1 / PCLK) * (1 + BS1 + BS2))       -- See the Reference Manual
    //   BITRATE = PCLK / (PRESCALER * (1 + BS1 + BS2))                 -- Simplified
    // let:
    //   BS = 1 + BS1 + BS2                                             -- Number of time quanta per bit
    //   PRESCALER_BS = PRESCALER * BS
    // ==>
    //   PRESCALER_BS = PCLK / BITRATE
    let prescaler_bs = periph_clock / can_bitrate;

    // Searching for such prescaler value so that the number of quanta per bit is highest.
    let mut bs1_bs2_sum = max_quanta_per_bit - 1;
    while (prescaler_bs % (1 + bs1_bs2_sum) as u32) != 0 {
        if bs1_bs2_sum <= 2 {
            return None; // No solution
        }
        bs1_bs2_sum -= 1;
    }

    let prescaler = prescaler_bs / (1 + bs1_bs2_sum) as u32;
    if (prescaler < 1) || (prescaler > 1024) {
        return None; // No solution
    }

    // Now we have a constraint: (BS1 + BS2) == bs1_bs2_sum.
    // We need to find such values so that the sample point is as close as possible to the optimal value,
    // which is 87.5%, which is 7/8.
    //
    //   Solve[(1 + bs1)/(1 + bs1 + bs2) == 7/8, bs2]  (* Where 7/8 is 0.875, the recommended sample point location *)
    //   {{bs2 -> (1 + bs1)/7}}
    //
    // Hence:
    //   bs2 = (1 + bs1) / 7
    //   bs1 = (7 * bs1_bs2_sum - 1) / 8
    //
    // Sample point location can be computed as follows:
    //   Sample point location = (1 + bs1) / (1 + bs1 + bs2)
    //
    // Since the optimal solution is so close to the maximum, we prepare two solutions, and then pick the best one:
    //   - With rounding to nearest
    //   - With rounding to zero
    let mut bs1 = ((7 * bs1_bs2_sum - 1) + 4) / 8; // Trying rounding to nearest first
    let mut bs2 = bs1_bs2_sum - bs1;
    core::assert!(bs1_bs2_sum > bs1);

    let sample_point_permill = 1000 * ((1 + bs1) / (1 + bs1 + bs2)) as u16;
    if sample_point_permill > MAX_SAMPLE_POINT_PERMILL {
        // Nope, too far; now rounding to zero
        bs1 = (7 * bs1_bs2_sum - 1) / 8;
        bs2 = bs1_bs2_sum - bs1;
    }

    // Check is BS1 and BS2 are in range
    if (bs1 < 1) || (bs1 > BS1_MAX) || (bs2 < 1) || (bs2 > BS2_MAX) {
        return None;
    }

    // Check if final bitrate matches the requested
    if can_bitrate != (periph_clock / (prescaler * (1 + bs1 + bs2) as u32)) {
        return None;
    }

    // One is recommended by DS-015, CANOpen, and DeviceNet
    let sync_jump_width = core::num::NonZeroU8::new(1)?;

    let seg1 = core::num::NonZeroU8::new(bs1)?;
    let seg2 = core::num::NonZeroU8::new(bs2)?;
    let nz_prescaler = core::num::NonZeroU16::new(prescaler as u16)?;

    Some(config::NominalBitTiming {
        sync_jump_width,
        prescaler: nz_prescaler,
        seg1,
        seg2,
    })
}

pub struct Fdcan<'d, T: Instance, M: FdcanOperatingMode> {
    pub can: RefCell<fdcan::FdCan<FdcanInstance<'d, T>, M>>,
}

impl<'d, T: Instance> Fdcan<'d, T, fdcan::ConfigMode> {
    /// Creates a new Fdcan instance, keeping the peripheral in sleep mode.
    /// You must call [Fdcan::enable_non_blocking] to use the peripheral.
    pub fn new(
        peri: impl Peripheral<P = T> + 'd,
        rx: impl Peripheral<P = impl RxPin<T>> + 'd,
        tx: impl Peripheral<P = impl TxPin<T>> + 'd,
        _irqs: impl interrupt::typelevel::Binding<T::IT0Interrupt, IT0InterruptHandler<T>>
            + interrupt::typelevel::Binding<T::IT1Interrupt, IT1InterruptHandler<T>>
            + 'd,
    ) -> Fdcan<'d, T, fdcan::ConfigMode> {
        into_ref!(peri, rx, tx);

        rx.set_as_af(rx.af_num(), AFType::Input);
        tx.set_as_af(tx.af_num(), AFType::OutputPushPull);

        T::enable_and_reset();

        rx.set_as_af(rx.af_num(), AFType::Input);
        tx.set_as_af(tx.af_num(), AFType::OutputPushPull);

        let mut can = fdcan::FdCan::new(FdcanInstance(peri)).into_config_mode();

        unsafe {
            T::configure_msg_ram();

            T::IT0Interrupt::unpend();
            T::IT0Interrupt::enable();

            T::IT1Interrupt::unpend();
            T::IT1Interrupt::enable();

            // this isn't really documented in the reference manual
            // but corresponding txbtie bit has to be set for the TC (TxComplete) interrupt to fire
            T::regs().txbtie().write(|w| w.0 = 0xffff_ffff);
        }

        can.enable_interrupt(fdcan::interrupt::Interrupt::RxFifo0NewMsg);
        can.enable_interrupt(fdcan::interrupt::Interrupt::TxComplete);
        can.enable_interrupt_line(fdcan::interrupt::InterruptLine::_0, true);
        can.enable_interrupt_line(fdcan::interrupt::InterruptLine::_1, true);

        let can_ref_cell = RefCell::new(can);
        Self { can: can_ref_cell }
    }

    /// Configures the bit timings calculated from supplied bitrate.
    pub fn set_bitrate(&mut self, bitrate: u32) {
        let bit_timing = calc_fdcan_timings(T::frequency(), bitrate).unwrap();
        self.can.borrow_mut().set_nominal_bit_timing(bit_timing);
    }
}

macro_rules! impl_transition {
    ($from_mode:ident, $to_mode:ident, $name:ident, $func: ident) => {
        impl<'d, T: Instance> Fdcan<'d, T, fdcan::$from_mode> {
            pub fn $name(self) -> Fdcan<'d, T, fdcan::$to_mode> {
                Fdcan {
                    can: RefCell::new(self.can.into_inner().$func()),
                }
            }
        }
    };
}

impl_transition!(PoweredDownMode, ConfigMode, into_config_mode, into_config_mode);
impl_transition!(InternalLoopbackMode, ConfigMode, into_config_mode, into_config_mode);

impl_transition!(ConfigMode, NormalOperationMode, into_normal_mode, into_normal);
impl_transition!(
    ConfigMode,
    ExternalLoopbackMode,
    into_external_loopback_mode,
    into_external_loopback
);
impl_transition!(
    ConfigMode,
    InternalLoopbackMode,
    into_internal_loopback_mode,
    into_internal_loopback
);

impl<'d, T: Instance, M: FdcanOperatingMode> Fdcan<'d, T, M> {
    pub fn as_mut(&self) -> RefMut<'_, fdcan::FdCan<FdcanInstance<'d, T>, M>> {
        self.can.borrow_mut()
    }
}

impl<'d, T: Instance, M: FdcanOperatingMode> Fdcan<'d, T, M>
where
    M: fdcan::Transmit,
    M: fdcan::Receive,
{
    /// Queues the message to be sent but exerts backpressure.  If a lower-priority
    /// frame is dropped from the mailbox, it is returned.  If no lower-priority frames
    /// can be replaced, this call asynchronously waits for a frame to be successfully
    /// transmitted, then tries again.
    pub async fn write(&mut self, frame: &TxFrame) -> Option<TxFrame> {
        poll_fn(|cx| {
            T::state().tx_waker.register(cx.waker());
            if let Ok(dropped) =
                self.can
                    .borrow_mut()
                    .transmit_preserve(frame.header, &frame.data.bytes, &mut |_, hdr, data32| {
                        TxFrame::from_preserved(hdr, data32)
                    })
            {
                return Poll::Ready(dropped.flatten());
            }

            // Couldn't replace any lower priority frames.  Need to wait for some mailboxes
            // to clear.
            Poll::Pending
        })
        .await
    }

    pub async fn flush(&self, mb: fdcan::Mailbox) {
        poll_fn(|cx| {
            T::state().tx_waker.register(cx.waker());

            let idx: u8 = mb.into();
            let idx = 1 << idx;
            if !T::regs().txbrp().read().trp(idx) {
                return Poll::Ready(());
            }

            Poll::Pending
        })
        .await;
    }

    /// Returns the next received message frame
    pub async fn read(&mut self) -> Result<RxFrame, BusError> {
        poll_fn(|cx| {
            T::state().err_waker.register(cx.waker());
            T::state().rx_waker.register(cx.waker());

            // TODO: handle fifo0 AND fifo1
            let mut buffer: [u8; 64] = [0; 64];
            if let Ok(rx) = self.can.borrow_mut().receive0(&mut buffer) {
                // rx: fdcan::ReceiveOverrun<RxFrameInfo>
                // TODO: report overrun?
                //  for now we just drop it
                let frame: RxFrame = RxFrame::new(rx.unwrap(), &buffer);
                return Poll::Ready(Ok(frame));
            } else if let Some(err) = self.curr_error() {
                // TODO: this is probably wrong
                return Poll::Ready(Err(err));
            }

            Poll::Pending
        })
        .await
    }
    fn curr_error(&self) -> Option<BusError> {
        let err = { T::regs().psr().read() };
        if err.bo() {
            return Some(BusError::BusOff);
        } else if err.ep() {
            return Some(BusError::BusPassive);
        } else if err.ew() {
            return Some(BusError::BusWarning);
        } else {
            cfg_if! {
                if #[cfg(stm32h7)] {
                    let lec = err.lec();
                } else {
                    let lec = err.lec().to_bits();
                }
            }
            if let Ok(err) = LastErrorCode::try_from(lec) {
                return BusError::try_from(err);
            }
        }
        None
    }

    pub fn split<'c>(&'c self) -> (FdcanTx<'c, 'd, T, M>, FdcanRx<'c, 'd, T, M>) {
        (FdcanTx { can: &self.can }, FdcanRx { can: &self.can })
    }
}
pub struct FdcanTx<'c, 'd, T: Instance, M: fdcan::Transmit> {
    can: &'c RefCell<fdcan::FdCan<FdcanInstance<'d, T>, M>>,
}

impl<'c, 'd, T: Instance, M: fdcan::Transmit> FdcanTx<'c, 'd, T, M> {
    /// Queues the message to be sent but exerts backpressure.  If a lower-priority
    /// frame is dropped from the mailbox, it is returned.  If no lower-priority frames
    /// can be replaced, this call asynchronously waits for a frame to be successfully
    /// transmitted, then tries again.
    pub async fn write(&mut self, frame: &TxFrame) -> Option<TxFrame> {
        poll_fn(|cx| {
            T::state().tx_waker.register(cx.waker());
            if let Ok(dropped) =
                self.can
                    .borrow_mut()
                    .transmit_preserve(frame.header, &frame.data.bytes, &mut |_, hdr, data32| {
                        TxFrame::from_preserved(hdr, data32)
                    })
            {
                return Poll::Ready(dropped.flatten());
            }

            // Couldn't replace any lower priority frames.  Need to wait for some mailboxes
            // to clear.
            Poll::Pending
        })
        .await
    }
}

#[allow(dead_code)]
pub struct FdcanRx<'c, 'd, T: Instance, M: fdcan::Receive> {
    can: &'c RefCell<fdcan::FdCan<FdcanInstance<'d, T>, M>>,
}

impl<'c, 'd, T: Instance, M: fdcan::Receive> FdcanRx<'c, 'd, T, M> {
    /// Returns the next received message frame
    pub async fn read(&mut self) -> Result<RxFrame, BusError> {
        poll_fn(|cx| {
            T::state().err_waker.register(cx.waker());
            T::state().rx_waker.register(cx.waker());

            // TODO: handle fifo0 AND fifo1
            let mut buffer: [u8; 64] = [0; 64];
            if let Ok(rx) = self.can.borrow_mut().receive0(&mut buffer) {
                // rx: fdcan::ReceiveOverrun<RxFrameInfo>
                // TODO: report overrun?
                //  for now we just drop it
                let frame: RxFrame = RxFrame::new(rx.unwrap(), &buffer);
                return Poll::Ready(Ok(frame));
            } else if let Some(err) = self.curr_error() {
                // TODO: this is probably wrong
                return Poll::Ready(Err(err));
            }

            Poll::Pending
        })
        .await
    }

    fn curr_error(&self) -> Option<BusError> {
        let err = { T::regs().psr().read() };
        if err.bo() {
            return Some(BusError::BusOff);
        } else if err.ep() {
            return Some(BusError::BusPassive);
        } else if err.ew() {
            return Some(BusError::BusWarning);
        } else {
            cfg_if! {
                if #[cfg(stm32h7)] {
                    let lec = err.lec();
                } else {
                    let lec = err.lec().to_bits();
                }
            }
            if let Ok(err) = LastErrorCode::try_from(lec) {
                return BusError::try_from(err);
            }
        }
        None
    }
}
impl<'d, T: Instance, M: FdcanOperatingMode> Deref for Fdcan<'d, T, M> {
    type Target = RefCell<fdcan::FdCan<FdcanInstance<'d, T>, M>>;

    fn deref(&self) -> &Self::Target {
        &self.can
    }
}

impl<'d, T: Instance, M: FdcanOperatingMode> DerefMut for Fdcan<'d, T, M> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.can
    }
}

pub(crate) mod sealed {
    use embassy_sync::waitqueue::AtomicWaker;

    pub struct State {
        pub tx_waker: AtomicWaker,
        pub err_waker: AtomicWaker,
        pub rx_waker: AtomicWaker,
    }

    impl State {
        pub const fn new() -> Self {
            Self {
                tx_waker: AtomicWaker::new(),
                err_waker: AtomicWaker::new(),
                rx_waker: AtomicWaker::new(),
            }
        }
    }

    pub trait Instance {
        const REGISTERS: *mut fdcan::RegisterBlock;
        const MSG_RAM: *mut fdcan::message_ram::RegisterBlock;
        const MSG_RAM_OFFSET: usize;

        fn regs() -> &'static crate::pac::can::Fdcan;
        fn state() -> &'static State;

        #[cfg(not(stm32h7))]
        fn configure_msg_ram() {}

        #[cfg(stm32h7)]
        fn configure_msg_ram() {
            let r = Self::regs();

            use fdcan::message_ram::*;
            let mut offset_words = Self::MSG_RAM_OFFSET as u16;

            // 11-bit filter
            r.sidfc().modify(|w| w.set_flssa(offset_words));
            offset_words += STANDARD_FILTER_MAX as u16;

            // 29-bit filter
            r.xidfc().modify(|w| w.set_flesa(offset_words));
            offset_words += 2 * EXTENDED_FILTER_MAX as u16;

            // Rx FIFO 0 and 1
            for i in 0..=1 {
                r.rxfc(i).modify(|w| {
                    w.set_fsa(offset_words);
                    w.set_fs(RX_FIFO_MAX);
                    w.set_fwm(RX_FIFO_MAX);
                });
                offset_words += 18 * RX_FIFO_MAX as u16;
            }

            // Rx buffer - see below
            // Tx event FIFO
            r.txefc().modify(|w| {
                w.set_efsa(offset_words);
                w.set_efs(TX_EVENT_MAX);
                w.set_efwm(TX_EVENT_MAX);
            });
            offset_words += 2 * TX_EVENT_MAX as u16;

            // Tx buffers
            r.txbc().modify(|w| {
                w.set_tbsa(offset_words);
                w.set_tfqs(TX_FIFO_MAX);
            });
            offset_words += 18 * TX_FIFO_MAX as u16;

            // Rx Buffer - not used
            r.rxbc().modify(|w| {
                w.set_rbsa(offset_words);
            });

            // TX event FIFO?
            // Trigger memory?

            // Set the element sizes to 16 bytes
            r.rxesc().modify(|w| {
                w.set_rbds(0b111);
                for i in 0..=1 {
                    w.set_fds(i, 0b111);
                }
            });
            r.txesc().modify(|w| {
                w.set_tbds(0b111);
            })
        }
    }
}

pub trait IT0Instance {
    type IT0Interrupt: crate::interrupt::typelevel::Interrupt;
}

pub trait IT1Instance {
    type IT1Interrupt: crate::interrupt::typelevel::Interrupt;
}

pub trait InterruptableInstance: IT0Instance + IT1Instance {}
pub trait Instance: sealed::Instance + RccPeripheral + InterruptableInstance + 'static {}

pub struct FdcanInstance<'a, T>(PeripheralRef<'a, T>);

unsafe impl<'d, T: Instance> fdcan::message_ram::Instance for FdcanInstance<'d, T> {
    const MSG_RAM: *mut RegisterBlock = T::MSG_RAM;
}

unsafe impl<'d, T: Instance> fdcan::Instance for FdcanInstance<'d, T>
where
    FdcanInstance<'d, T>: fdcan::message_ram::Instance,
{
    const REGISTERS: *mut fdcan::RegisterBlock = T::REGISTERS;
}

macro_rules! impl_fdcan {
    ($inst:ident, $msg_ram_inst:ident, $msg_ram_offset:literal) => {
        impl sealed::Instance for peripherals::$inst {
            const REGISTERS: *mut fdcan::RegisterBlock = crate::pac::$inst.as_ptr() as *mut _;
            const MSG_RAM: *mut fdcan::message_ram::RegisterBlock = crate::pac::$msg_ram_inst.as_ptr() as *mut _;
            const MSG_RAM_OFFSET: usize = $msg_ram_offset;

            fn regs() -> &'static crate::pac::can::Fdcan {
                &crate::pac::$inst
            }

            fn state() -> &'static sealed::State {
                static STATE: sealed::State = sealed::State::new();
                &STATE
            }
        }

        impl Instance for peripherals::$inst {}

        foreach_interrupt!(
            ($inst,can,FDCAN,IT0,$irq:ident) => {
                impl IT0Instance for peripherals::$inst {
                    type IT0Interrupt = crate::interrupt::typelevel::$irq;
                }
            };
            ($inst,can,FDCAN,IT1,$irq:ident) => {
                impl IT1Instance for peripherals::$inst {
                    type IT1Interrupt = crate::interrupt::typelevel::$irq;
                }
            };
        );

        impl InterruptableInstance for peripherals::$inst {}
    };

    ($inst:ident, $msg_ram_inst:ident) => {
        impl_fdcan!($inst, $msg_ram_inst, 0);
    };
}

#[cfg(not(stm32h7))]
foreach_peripheral!(
    (can, FDCAN) => { impl_fdcan!(FDCAN, FDCANRAM); };
    (can, FDCAN1) => { impl_fdcan!(FDCAN1, FDCANRAM1); };
    (can, FDCAN2) => { impl_fdcan!(FDCAN2, FDCANRAM2); };
    (can, FDCAN3) => { impl_fdcan!(FDCAN3, FDCANRAM3); };
);

#[cfg(stm32h7)]
foreach_peripheral!(
    (can, FDCAN1) => { impl_fdcan!(FDCAN1, FDCANRAM, 0x0000); };
    (can, FDCAN2) => { impl_fdcan!(FDCAN2, FDCANRAM, 0x0C00); };
    (can, FDCAN3) => { impl_fdcan!(FDCAN3, FDCANRAM, 0x1800); };
);

pin_trait!(RxPin, Instance);
pin_trait!(TxPin, Instance);
