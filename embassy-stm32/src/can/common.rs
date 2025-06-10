use embassy_sync::channel::{SendDynamicReceiver, SendDynamicSender};

use super::enums::*;
use super::frame::*;

pub(crate) struct ClassicBufferedRxInner {
    pub rx_sender: SendDynamicSender<'static, Result<Envelope, BusError>>,
}
pub(crate) struct ClassicBufferedTxInner {
    pub tx_receiver: SendDynamicReceiver<'static, Frame>,
}

#[cfg(any(can_fdcan_v1, can_fdcan_h7))]

pub(crate) struct FdBufferedRxInner {
    pub rx_sender: SendDynamicSender<'static, Result<FdEnvelope, BusError>>,
}

#[cfg(any(can_fdcan_v1, can_fdcan_h7))]
pub(crate) struct FdBufferedTxInner {
    pub tx_receiver: SendDynamicReceiver<'static, FdFrame>,
}

/// Sender that can be used for sending CAN frames.
pub struct BufferedSender<'ch, FRAME> {
    pub(crate) tx_buf: embassy_sync::channel::SendDynamicSender<'ch, FRAME>,
    pub(crate) tx_guard: TxGuard,
}

impl<'ch, FRAME> BufferedSender<'ch, FRAME> {
    /// Async write frame to TX buffer.
    pub fn try_write(&mut self, frame: FRAME) -> Result<(), embassy_sync::channel::TrySendError<FRAME>> {
        self.tx_buf.try_send(frame)?;
        (self.tx_guard.info().tx_waker)();
        Ok(())
    }

    /// Async write frame to TX buffer.
    pub async fn write(&mut self, frame: FRAME) {
        self.tx_buf.send(frame).await;
        (self.tx_guard.info().tx_waker)();
    }

    /// Allows a poll_fn to poll until the channel is ready to write
    pub fn poll_ready_to_send(&self, cx: &mut core::task::Context<'_>) -> core::task::Poll<()> {
        self.tx_buf.poll_ready_to_send(cx)
    }
}

impl<'ch, FRAME> Clone for BufferedSender<'ch, FRAME> {
    fn clone(&self) -> Self {
        Self {
            tx_buf: self.tx_buf,
            tx_guard: TxGuard::new(self.tx_guard.info()),
        }
    }
}

/// Sender that can be used for sending Classic CAN frames.
pub type BufferedCanSender = BufferedSender<'static, Frame>;

/// Receiver that can be used for receiving CAN frames. Note, each CAN frame will only be received by one receiver.
pub struct BufferedReceiver<'ch, ENVELOPE> {
    pub(crate) rx_buf: embassy_sync::channel::SendDynamicReceiver<'ch, Result<ENVELOPE, BusError>>,
    pub(crate) rx_guard: RxGuard,
}

impl<'ch, ENVELOPE> BufferedReceiver<'ch, ENVELOPE> {
    /// Receive the next frame.
    ///
    /// See [`Channel::receive()`].
    pub fn receive(&self) -> embassy_sync::channel::DynamicReceiveFuture<'_, Result<ENVELOPE, BusError>> {
        self.rx_buf.receive()
    }

    /// Attempt to immediately receive the next frame.
    ///
    /// See [`Channel::try_receive()`]
    pub fn try_receive(&self) -> Result<Result<ENVELOPE, BusError>, embassy_sync::channel::TryReceiveError> {
        self.rx_buf.try_receive()
    }

    /// Allows a poll_fn to poll until the channel is ready to receive
    ///
    /// See [`Channel::poll_ready_to_receive()`]
    pub fn poll_ready_to_receive(&self, cx: &mut core::task::Context<'_>) -> core::task::Poll<()> {
        self.rx_buf.poll_ready_to_receive(cx)
    }

    /// Poll the channel for the next frame
    ///
    /// See [`Channel::poll_receive()`]
    pub fn poll_receive(&self, cx: &mut core::task::Context<'_>) -> core::task::Poll<Result<ENVELOPE, BusError>> {
        self.rx_buf.poll_receive(cx)
    }
}

impl<'ch, ENVELOPE> Clone for BufferedReceiver<'ch, ENVELOPE> {
    fn clone(&self) -> Self {
        Self {
            rx_buf: self.rx_buf,
            rx_guard: RxGuard::new(self.rx_guard.info()),
        }
    }
}

/// A BufferedCanReceiver for Classic CAN frames.
pub type BufferedCanReceiver = BufferedReceiver<'static, Envelope>;

/// Implements RAII for the internal reference counting (TX side). Each TX type should contain one
/// of these. The new method and the Drop impl will automatically call the reference counting
/// function. Like this, the reference counting function does not need to be called manually for
/// each TX type. Transceiver types (TX and RX) should contain one TxGuard and one RxGuard.
pub(crate) struct TxGuard {
    //internal_operation: fn(InternalOperation),
    info: &'static super::Info,
}
impl TxGuard {
    pub(crate) fn new(info: &'static super::Info) -> Self {
        (info.internal_operation)(InternalOperation::NotifySenderCreated);
        Self { info }
    }
    pub(crate) fn info(&self) -> &'static super::Info {
        self.info
    }
}
impl Drop for TxGuard {
    fn drop(&mut self) {
        (self.info.internal_operation)(InternalOperation::NotifySenderDestroyed);
    }
}

/// Implements RAII for the internal reference counting (RX side). See TxGuard for further doc.
pub(crate) struct RxGuard {
    info: &'static super::Info,
}
impl RxGuard {
    pub(crate) fn new(info: &'static super::Info) -> Self {
        (info.internal_operation)(InternalOperation::NotifyReceiverCreated);
        Self { info }
    }
    pub(crate) fn info(&self) -> &'static super::Info {
        self.info
    }
}
impl Drop for RxGuard {
    fn drop(&mut self) {
        (self.info.internal_operation)(InternalOperation::NotifyReceiverDestroyed);
    }
}

pub(crate) struct Guards {
    pub tx: TxGuard,
    pub _rx: RxGuard,
}

impl Guards {
    pub(crate) fn new(info: &'static super::Info) -> Self {
        Self {
            tx: TxGuard::new(info),
            _rx: RxGuard::new(info),
        }
    }
}

impl core::ops::Deref for Guards {
    type Target = &'static super::Info;

    fn deref(&self) -> &Self::Target {
        &self.tx.info
    }
}
