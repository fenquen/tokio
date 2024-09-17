#![cfg_attr(not(feature = "rt"), allow(dead_code))]

use crate::process::unix::GlobalOrphanQueue;
use crate::runtime::driver;
use crate::runtime::signal::{SignalDriver, SignalDriverHandle};

use std::time::Duration;

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

    pub(crate) fn park(&mut self, handle: &driver::DriverHandle) {
        self.signalDriver.park(handle);
        GlobalOrphanQueue::reap_orphans(&self.signalDriverHandle);
    }

    pub(crate) fn park_timeout(&mut self, driverHandle: &driver::DriverHandle, duration: Duration) {
        self.signalDriver.park_timeout(driverHandle, duration);
        GlobalOrphanQueue::reap_orphans(&self.signalDriverHandle);
    }

    pub(crate) fn shutdown(&mut self, handle: &driver::DriverHandle) {
        self.signalDriver.shutdown(handle);
    }
}
