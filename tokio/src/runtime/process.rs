use crate::process::unix::GlobalOrphanQueue;
use crate::runtime::driver;
use crate::runtime::signal::{SignalDriver, SignalDriverHandle};

use std::time::Duration;
use crate::runtime::driver::DriverHandle;

/// Responsible for cleaning up orphaned child processes on Unix platforms.
#[derive(Debug)]
pub(crate) struct ProcessDriver {
    signalDriver: SignalDriver,
    signalDriverHandle: SignalDriverHandle,
}

impl ProcessDriver {
    /// Creates a new signal `Driver` instance that delegates wakeups to `park`.
    pub(crate) fn new(signalDriver: SignalDriver) -> Self {
        let signalDriverHandle = signalDriver.handle();

        Self {
            signalDriver,
            signalDriverHandle,
        }
    }

    pub(crate) fn park_timeout(&mut self, driverHandle: &DriverHandle, duration: Option<Duration>) {
        self.signalDriver.ioDriver.turn(driverHandle.io(), duration);
        self.signalDriver.process();
        GlobalOrphanQueue::reap_orphans(&self.signalDriverHandle);
    }

    pub(crate) fn shutdown(&mut self, handle: &driver::DriverHandle) {
        self.signalDriver.shutdown(handle);
    }
}
