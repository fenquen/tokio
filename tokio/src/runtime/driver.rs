//! Abstracts out the entire chain of runtime sub-drivers into common types.

// Eventually, this file will see significant refactoring / cleanup. For now, we
// don't need to worry much about dead code with certain feature permutations.
#![cfg_attr(any(not(all(tokio_unstable, feature = "full")), target_family = "wasm"), allow(dead_code))]

use crate::runtime::park::{ParkThread, UnparkThread};

use std::io;
use std::time::Duration;
use crate::runtime::io::{IODriver, IODriverHandle};
use crate::runtime::process::ProcessDriver;
use crate::runtime::signal::{SignalDriver, SignalDriverHandle};
use crate::runtime::time::{TimeDriver, TimeDriverHandle};

#[derive(Debug)]
pub(crate) struct Driver {
    timeDriverEnum: TimeDriverEnum,
}

#[derive(Debug)]
pub(crate) struct DriverHandle {
    /// IO driver handle
    pub(crate) ioHandleEnum: IOHandleEnum,

    /// Signal driver handle
    pub(crate) signalDriverHandle: Option<SignalDriverHandle>,

    /// Time driver handle
    pub(crate) timeDriverHandle: Option<TimeDriverHandle>,
}

pub(crate) struct DriverConfig {
    pub(crate) enable_io: bool,
    pub(crate) enable_time: bool,
    pub(crate) enable_pause_time: bool,
    pub(crate) start_paused: bool,
    pub(crate) nevents: usize,
    pub(crate) workerThreadCount: usize,
}

impl Driver {
    pub(crate) fn new(driverConfig: DriverConfig) -> io::Result<(Self, DriverHandle)> {
        let (ioStackEnum, ioHandleEnum, signalDriverHandle) = create_io_stack(driverConfig.enable_io, driverConfig.nevents)?;

        let (timeDriverEnum, timeDriverHandle) = createTimeDriver(driverConfig.enable_time, ioStackEnum, driverConfig.workerThreadCount);

        Ok((
            Driver { timeDriverEnum },
            DriverHandle {
                ioHandleEnum,
                signalDriverHandle,
                timeDriverHandle,
            },
        ))
    }

    pub(crate) fn park_timeout(&mut self, handle: &DriverHandle, duration: Option<Duration>) {
        self.timeDriverEnum.park_timeout(handle, duration);
    }

    pub(crate) fn shutdown(&mut self, handle: &DriverHandle) {
        self.timeDriverEnum.shutdown(handle);
    }
}

impl DriverHandle {
    pub(crate) fn unpark(&self) {
        self.ioHandleEnum.unpark();
    }

    cfg_io_driver! {
        #[track_caller]
        pub(crate) fn io(&self) -> &IODriverHandle {
            self.ioHandleEnum.as_ref().expect("A Tokio 1.x context was found, but IO is disabled. Call `enable_io` on the runtime builder to enable IO.")
        }
    }

    cfg_signal_internal_and_unix! {
        #[track_caller]
        pub(crate) fn signal(&self) -> &crate::runtime::signal::SignalDriverHandle {
            self.signalDriverHandle.as_ref().expect("there is no signal driver running, must be called from the context of Tokio runtime")
        }
    }

    cfg_time! {
        /// Returns a reference to the time driver handle.
        ///
        /// Panics if no time driver is present.
        #[track_caller]
        pub(crate) fn time(&self) -> &TimeDriverHandle {
            self.timeDriverHandle.as_ref().expect("A Tokio 1.x context was found, but timers are disabled. Call `enable_time` on the runtime builder to enable timers.")
        }
    }
}

#[cfg(any(feature = "net", all(unix, feature = "process"), all(unix, feature = "signal")))]
#[derive(Debug)]
pub(crate) enum IoStackEnum {
    Enabled(ProcessDriver),
    Disabled(ParkThread),
}

#[cfg(any(feature = "net", all(unix, feature = "process"), all(unix, feature = "signal")))]
#[derive(Debug)]
pub(crate) enum IOHandleEnum {
    Enabled(IODriverHandle),
    Disabled(UnparkThread),
}

#[cfg(any(feature = "net", all(unix, feature = "process"), all(unix, feature = "signal")))]
fn create_io_stack(enableIO: bool, eventCount: usize) -> io::Result<(IoStackEnum, IOHandleEnum, Option<SignalDriverHandle>)> {
    let ret = if enableIO {
        let (ioDriver, ioDriverHandle) = IODriver::new(eventCount)?;

        let (signalDriver, signalDriverHandle) = createSignalDriver(ioDriver, &ioDriverHandle)?;

        let processDriver = ProcessDriver::new(signalDriver);

        (IoStackEnum::Enabled(processDriver), IOHandleEnum::Enabled(ioDriverHandle), signalDriverHandle)
    } else {
        let park_thread = ParkThread::new();
        let unpark_thread = park_thread.unpark();
        (IoStackEnum::Disabled(park_thread), IOHandleEnum::Disabled(unpark_thread), Default::default())
    };

    Ok(ret)
}

#[cfg(any(feature = "net", all(unix, feature = "process"), all(unix, feature = "signal")))]
impl IoStackEnum {
    pub(crate) fn is_enabled(&self) -> bool {
        match self {
            IoStackEnum::Enabled(..) => true,
            IoStackEnum::Disabled(..) => false,
        }
    }

    pub(crate) fn park(&mut self, driverHandle: &DriverHandle) {
        match self {
            IoStackEnum::Enabled(processDriver) => processDriver.park_timeout(driverHandle, None),
            IoStackEnum::Disabled(parkThread) => parkThread.park(),
        }
    }

    pub(crate) fn park_timeout(&mut self, driverHandle: &DriverHandle, duration: Duration) {
        match self {
            IoStackEnum::Enabled(processDriver) => processDriver.park_timeout(driverHandle, Some(duration)),
            IoStackEnum::Disabled(parkThread) => parkThread.park_timeout(duration),
        }
    }

    pub(crate) fn shutdown(&mut self, handle: &DriverHandle) {
        match self {
            IoStackEnum::Enabled(v) => v.shutdown(handle),
            IoStackEnum::Disabled(v) => v.shutdown(),
        }
    }
}

#[cfg(any(feature = "net", all(unix, feature = "process"), all(unix, feature = "signal")))]
impl IOHandleEnum {
    pub(crate) fn unpark(&self) {
        match self {
            IOHandleEnum::Enabled(ioDriverHandle) => ioDriverHandle.unpark(),
            IOHandleEnum::Disabled(handle) => handle.unpark(),
        }
    }

    pub(crate) fn as_ref(&self) -> Option<&IODriverHandle> {
        match self {
            IOHandleEnum::Enabled(v) => Some(v),
            IOHandleEnum::Disabled(..) => None,
        }
    }
}


cfg_not_io_driver! {
    pub(crate) type IoHandle = UnparkThread;

    #[derive(Debug)]
    pub(crate) struct IoStack(ParkThread);

    fn create_io_stack(_enabled: bool, _nevents: usize) -> io::Result<(IoStackEnum, IOHandleEnum, Option<crate::runtime::signal::SignalDriverHandle>)> {
        let park_thread = ParkThread::new();
        let unpark_thread = park_thread.unpark();
        Ok((IoStack(park_thread), unpark_thread, Default::default()))
    }

    impl IoStackEnum {
        pub(crate) fn park(&mut self, _handle: &DriverHandle) {
            self.0.park();
        }

        pub(crate) fn park_timeout(&mut self, _handle: &DriverHandle, duration: Duration) {
            self.0.park_timeout(duration);
        }

        pub(crate) fn shutdown(&mut self, _handle: &DriverHandle) {
            self.0.shutdown();
        }

        /// This is not a "real" driver, so it is not considered enabled.
        pub(crate) fn is_enabled(&self) -> bool {
            false
        }
    }
}

#[cfg(any(feature = "signal", all(unix, feature = "process")))]
#[cfg(not(loom))]
fn createSignalDriver(ioDriver: IODriver,
                      ioDriverHandle: &IODriverHandle) -> io::Result<(SignalDriver, Option<SignalDriverHandle>)> {
    let signalDriver = SignalDriver::new(ioDriver, ioDriverHandle)?;
    let signalDriverHandle = signalDriver.handle();
    Ok((signalDriver, Some(signalDriverHandle)))
}


cfg_not_signal_internal! {
    pub(crate) type SignalHandle = ();

    cfg_io_driver! {
        type SignalDriver = IoDriver;

        fn create_signal_driver(io_driver: IoDriver, _io_handle: &crate::runtime::io::Handle) -> io::Result<(SignalDriver, SignalHandle)> {
            Ok((io_driver, ()))
        }
    }
}

cfg_not_process_driver! {
    cfg_io_driver! {
        type ProcessDriver = SignalDriver;

        fn create_process_driver(signal_driver: SignalDriver) -> ProcessDriver {
            signal_driver
        }
    }
}

#[cfg(feature = "time")]
#[derive(Debug)]
pub(crate) enum TimeDriverEnum {
    Enabled { timeDriver: TimeDriver },
    Disabled(IoStackEnum),
}

#[cfg(feature = "time")]
pub(crate) type Clock = crate::time::Clock;

#[cfg(feature = "time")]
fn createTimeDriver(enable: bool, ioStackEnum: IoStackEnum, workerThreadCount: usize) -> (TimeDriverEnum, Option<TimeDriverHandle>) {
    if enable {
        let (timeDriver, timeDriverHandle) = TimeDriver::new(ioStackEnum, workerThreadCount as u32);
        (TimeDriverEnum::Enabled { timeDriver }, Some(timeDriverHandle))
    } else {
        (TimeDriverEnum::Disabled(ioStackEnum), None)
    }
}

#[cfg(feature = "time")]
impl TimeDriverEnum {
    pub(crate) fn is_enabled(&self) -> bool {
        match self {
            TimeDriverEnum::Enabled { .. } => true,
            TimeDriverEnum::Disabled(inner) => inner.is_enabled(),
        }
    }

    pub(crate) fn park_timeout(&mut self, driverHandle: &DriverHandle, duration: Option<Duration>) {
        match self {
            TimeDriverEnum::Enabled { timeDriver, .. } => timeDriver.park_internal(driverHandle, duration),
            TimeDriverEnum::Disabled(ioStackEnum) => {
                if let Some(duration) = duration {
                    ioStackEnum.park_timeout(driverHandle, duration);
                } else {
                    ioStackEnum.park(driverHandle);
                }
            }
        }
    }

    pub(crate) fn shutdown(&mut self, handle: &DriverHandle) {
        match self {
            TimeDriverEnum::Enabled { timeDriver } => timeDriver.shutdown(handle),
            TimeDriverEnum::Disabled(v) => v.shutdown(handle),
        }
    }
}


cfg_not_time! {
    type TimeDriver = IoStackEnum;

    pub(crate) type Clock = ();
    pub(crate) type TimeHandle = ();

    fn create_clock(_enable_pausing: bool, _start_paused: bool) -> Clock {
        ()
    }

    fn create_time_driver(
        _enable: bool,
        io_stack: IoStackEnum,
        _clock: &Clock,
        _workers: usize,
    ) -> (TimeDriverEnum, Option<TimeDriverHandle>) {
        (io_stack, ())
    }
}
