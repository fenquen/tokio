#![cfg_attr(not(feature = "rt"), allow(dead_code))]

//! Signal driver

use crate::runtime::{driver, io};
use crate::signal::registry::globals;

use mio::net::UnixStream;
use std::io::{self as std_io, Read};
use std::sync::{Arc, Weak};
use std::time::Duration;

/// Responsible for registering wakeups when an OS signal is received, and
/// subsequently dispatching notifications to any signal listeners as appropriate.
///
/// Note: this driver relies on having an enabled IO driver in order to listen to
/// pipe write wakeups.
#[derive(Debug)]
pub(crate) struct SignalDriver {
    /// Thread parker. The `Driver` park implementation delegates to this.
    ioDriver: io::IODriver,

    /// A pipe for receiving wake events from the signal handler
    receiver: UnixStream,

    /// Shared state. The driver keeps a strong ref and the handle keeps a weak
    /// ref. The weak ref is used to check if the driver is still active before
    /// trying to register a signal handler.
    inner: Arc<()>,
}

#[derive(Debug, Default)]
pub(crate) struct SignalDriverHandle {
    /// Paired w/ the `Arc` above and is used to check if the driver is still
    /// around before attempting to register a signal handler.
    inner: Weak<()>,
}

impl SignalDriver {
    /// creates a new signal `Driver` instance that delegates wakeups to `park`.
    pub(crate) fn new(io: io::IODriver, io_handle: &io::IODriverHandle) -> std_io::Result<Self> {
        use std::mem::ManuallyDrop;
        use std::os::unix::io::{AsRawFd, FromRawFd};

        // NB: We give each driver a "fresh" receiver file descriptor to avoid
        // the issues described in alexcrichton/tokio-process#42.
        //
        // In the past we would reuse the actual receiver file descriptor and
        // swallow any errors around double registration of the same descriptor.
        // I'm not sure if the second (failed) registration simply doesn't end
        // up receiving wake up notifications, or there could be some race
        // condition when consuming readiness events, but having distinct
        // descriptors appears to mitigate this.
        //
        // Unfortunately we cannot just use a single global UnixStream instance
        // either, since we can't assume they will always be registered with the
        // exact same reactor.
        //
        // Mio 0.7 removed `try_clone()` as an API due to unexpected behavior
        // with registering dups with the same reactor. In this case, duping is
        // safe as each dup is registered with separate reactors **and** we
        // only expect at least one dup to receive the notification.

        // Manually drop as we don't actually own this instance of UnixStream.
        let receiver_fd = globals().receiver.as_raw_fd();

        // safety: there is nothing unsafe about this, but the `from_raw_fd` fn is marked as unsafe.
        let original = ManuallyDrop::new(unsafe { std::os::unix::net::UnixStream::from_raw_fd(receiver_fd) });
        let mut receiver = UnixStream::from_std(original.try_clone()?);

        io_handle.register_signal_receiver(&mut receiver)?;

        Ok(Self {
            ioDriver: io,
            receiver,
            inner: Arc::new(()),
        })
    }

    /// Returns a handle to this event loop which can be sent across threads
    /// and can be used as a proxy to the event loop itself.
    pub(crate) fn handle(&self) -> SignalDriverHandle {
        SignalDriverHandle {
            inner: Arc::downgrade(&self.inner),
        }
    }

    pub(crate) fn park(&mut self, handle: &driver::DriverHandle) {
        self.ioDriver.park(handle);
        self.process();
    }

    pub(crate) fn park_timeout(&mut self, handle: &driver::DriverHandle, duration: Duration) {
        self.ioDriver.park_timeout(handle, duration);
        self.process();
    }

    pub(crate) fn shutdown(&mut self, handle: &driver::DriverHandle) {
        self.ioDriver.shutdown(handle);
    }

    fn process(&mut self) {
        // If the signal pipe has not received a readiness event, then there is nothing else to do.
        if !self.ioDriver.consume_signal_ready() {
            return;
        }

        // Drain the pipe completely so we can receive a new readiness event
        // if another signal has come in.
        let mut buf = [0; 128];
        #[allow(clippy::unused_io_amount)]
        loop {
            match self.receiver.read(&mut buf) {
                Ok(0) => panic!("EOF on self-pipe"),
                Ok(_) => continue, // Keep reading
                Err(e) if e.kind() == std_io::ErrorKind::WouldBlock => break,
                Err(e) => panic!("Bad read on self-pipe: {}", e),
            }
        }

        // Broadcast any signals which were received
        globals().broadcast();
    }
}

// ===== impl Handle =====

impl SignalDriverHandle {
    pub(crate) fn check_inner(&self) -> std_io::Result<()> {
        if self.inner.strong_count() > 0 {
            Ok(())
        } else {
            Err(std_io::Error::new(
                std_io::ErrorKind::Other,
                "signal driver gone",
            ))
        }
    }
}
