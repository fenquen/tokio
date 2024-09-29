// Signal handling
cfg_signal_internal_and_unix! {
    mod signal;
}

use crate::io::interest::Interest;
use crate::io::ready::Ready;
use crate::loom::sync::Mutex;
use crate::runtime::driver;
use crate::runtime::io::registration_set;
use crate::runtime::io::{RegistrationSet, ScheduledIo};

use mio::event::Source;
use std::fmt;
use std::io;
use std::sync::Arc;
use std::time::Duration;

/// I/O driver, backed by Mio.
pub(crate) struct IODriver {
    /// True when an event with the signal token is received
    signal_ready: bool,

    /// Reuse the `mio::Events` value across calls to poll.
    mioEvents: mio::Events,

    /// The system event queue.
    mioPoll: mio::Poll,
}

/// A reference to an I/O driver.
pub(crate) struct IODriverHandle {
    /// Registers I/O resources.
    mioRegistry: mio::Registry,

    /// Tracks all registrations
    registrationSet: RegistrationSet,

    /// State that should be synchronized
    synced: Mutex<registration_set::Synced>,

    /// Used to wake up the reactor from a call to `turn`.
    /// Not supported on `Wasi` due to lack of threading support.
    #[cfg(not(target_os = "wasi"))]
    mioWaker: mio::Waker,
}

#[derive(Debug)]
pub(crate) struct ReadyEvent {
    pub(super) tick: u8,
    pub(crate) ready: Ready,
    pub(super) is_shutdown: bool,
}

cfg_net_unix!(
    impl ReadyEvent {
        pub(crate) fn with_ready(&self, ready: Ready) -> Self {
            Self {
                ready,
                tick: self.tick,
                is_shutdown: self.is_shutdown,
            }
        }
    }
);

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub(super) enum Direction {
    Read,
    Write,
}

pub(super) enum Tick {
    Set,
    Clear(u8),
}

const TOKEN_WAKEUP: mio::Token = mio::Token(0);
const TOKEN_SIGNAL: mio::Token = mio::Token(1);

fn _assert_kinds() {
    fn _assert<T: Send + Sync>() {}

    _assert::<IODriverHandle>();
}

impl IODriver {
    /// Creates a new event loop, returning any error that happened during the creation.
    pub(crate) fn new(eventCount: usize) -> io::Result<(IODriver, IODriverHandle)> {
        let mioPoll = mio::Poll::new()?;

        #[cfg(not(target_os = "wasi"))]
        let mioWaker = mio::Waker::new(mioPoll.registry(), TOKEN_WAKEUP)?;
        let mioRegistry = mioPoll.registry().try_clone()?;

        let ioDriver = IODriver {
            signal_ready: false,
            mioEvents: mio::Events::with_capacity(eventCount),
            mioPoll,
        };

        let (registrations, synced) = RegistrationSet::new();

        let ioDriverHandle = IODriverHandle {
            mioRegistry,
            registrationSet: registrations,
            synced: Mutex::new(synced),
            mioWaker,
        };

        Ok((ioDriver, ioDriverHandle))
    }

    pub(crate) fn shutdown(&mut self, rt_handle: &driver::DriverHandle) {
        let handle = rt_handle.io();
        let ios = handle.registrationSet.shutdown(&mut handle.synced.lock());

        // `shutdown()` must be called without holding the lock.
        for io in ios {
            io.shutdown();
        }
    }

    pub fn turn(&mut self, ioDriverHandle: &IODriverHandle, maxWaitDuration: Option<Duration>) {
        debug_assert!(!ioDriverHandle.registrationSet.is_shutdown(&ioDriverHandle.synced.lock()));

        ioDriverHandle.release_pending_registrations();

        let mioEvents = &mut self.mioEvents;

        // Block waiting for an event to happen, peeling out how many events happened.
        match self.mioPoll.poll(mioEvents, maxWaitDuration) {
            Ok(()) => {}
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => panic!("unexpected error when polling the I/O driver: {:?}", e),
        }

        // Process all the events that came in, dispatching appropriately
        let mut ready_count = 0;

        for mioEvent in mioEvents.iter() {
            let token = mioEvent.token();

            if token == TOKEN_WAKEUP {
                // Nothing to do, the event is used to unblock the I/O driver
            } else if token == TOKEN_SIGNAL {
                self.signal_ready = true;
            } else {
                let ready = Ready::from_mio(mioEvent);

                // use std::ptr::from_exposed_addr when stable
                // epoll的attachment
                let scheduledIOPtr: *const ScheduledIo = token.0 as *const _;

                // Safety: we ensure that the pointers used as tokens are not freed
                // until they are both deregistered from mio **and** we know the I/O
                // driver is not concurrently polling. The I/O driver holds ownership of
                // an `Arc<ScheduledIo>` so we can safely cast this to a ref.
                let scheduleInfo = unsafe { &*scheduledIOPtr };

                scheduleInfo.set_readiness(Tick::Set, |currentReady| currentReady | ready);

                scheduleInfo.wake(ready);

                ready_count += 1;
            }
        }
    }
}

impl fmt::Debug for IODriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Driver")
    }
}

impl IODriverHandle {
    /// Forces a reactor blocked in a call to `turn` to wakeup, or otherwise
    /// makes the next call to `turn` return immediately.
    ///
    /// This method is intended to be used in situations where a notification
    /// needs to otherwise be sent to the main reactor. If the reactor is
    /// currently blocked inside of `turn` then it will wake up and soon return
    /// after this method has been called. If the reactor is not currently
    /// blocked in `turn`, then the next call to `turn` will not block and
    /// return immediately.
    pub(crate) fn unpark(&self) {
        #[cfg(not(target_os = "wasi"))]
        self.mioWaker.wake().expect("failed to wake I/O driver");
    }

    /// Registers an I/O resource with the reactor for a given `mio::Ready` state.
    ///
    /// The registration token is returned.
    pub(super) fn registerSource(&self, source: &mut impl Source, interest: Interest) -> io::Result<Arc<ScheduledIo>> {
        let scheduled_io = self.registrationSet.allocate(&mut self.synced.lock())?;
        let token = scheduled_io.token();

        // we should remove the `scheduled_io` from the `registrations` set if registering
        // the `source` with the OS fails. Otherwise it will leak the `scheduled_io`.
        if let Err(e) = self.mioRegistry.register(source, token, interest.to_mio()) {
            // safety: `scheduled_io` is part of the `registrations` set.
            unsafe { self.registrationSet.remove(&mut self.synced.lock(), &scheduled_io) };
            return Err(e);
        }

        Ok(scheduled_io)
    }

    /// Deregisters an I/O resource from the reactor.
    pub(super) fn deregister_source(&self, registration: &Arc<ScheduledIo>, source: &mut impl Source) -> io::Result<()> {
        // Deregister the source with the OS poller **first**
        self.mioRegistry.deregister(source)?;

        if self.registrationSet.deregister(&mut self.synced.lock(), registration) {
            self.unpark();
        }

        Ok(())
    }

    fn release_pending_registrations(&self) {
        if self.registrationSet.needs_release() {
            self.registrationSet.release(&mut self.synced.lock());
        }
    }
}

impl fmt::Debug for IODriverHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Handle")
    }
}

impl Direction {
    pub(super) fn mask(self) -> Ready {
        match self {
            Direction::Read => Ready::READABLE | Ready::READ_CLOSED,
            Direction::Write => Ready::WRITABLE | Ready::WRITE_CLOSED,
        }
    }
}
