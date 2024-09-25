//! Abstracts out the entire chain of runtime sub-drivers into common types.

// Eventually, this file will see significant refactoring / cleanup. For now, we
// don't need to worry much about dead code with certain feature permutations.
#![cfg_attr(
    any(not(all(tokio_unstable, feature = "full")), target_family = "wasm"),
    allow(dead_code)
)]

use crate::runtime::park::{ParkThread, UnparkThread};

use std::io;
use std::time::Duration;
use crate::runtime::process::ProcessDriver;
use crate::runtime::time::TimeDriverHandle;

#[derive(Debug)]
pub(crate) struct Driver {
    timeDriver: TimeDriver,
}

#[derive(Debug)]
pub(crate) struct DriverHandle {
    /// IO driver handle
    pub(crate) ioHandleEnum: IOHandleEnum,

    /// Signal driver handle
    pub(crate) signalDriverHandle: Option<crate::runtime::signal::SignalDriverHandle>,

    /// Time driver handle
    pub(crate) timeDriverHandle: Option<TimeDriverHandle>,

    /// Source of `Instant::now()`
    #[cfg_attr(not(all(feature = "time", feature = "test-util")), allow(dead_code))]
    pub(crate) clock: Clock,
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
        let (io_stack, io_handle, signal_handle) = create_io_stack(driverConfig.enable_io, driverConfig.nevents)?;

        let clock = create_clock(driverConfig.enable_pause_time, driverConfig.start_paused);

        let (time_driver, time_handle) = create_time_driver(driverConfig.enable_time, io_stack, &clock, driverConfig.workerThreadCount);

        Ok((
            Self { timeDriver: time_driver },
            DriverHandle {
                ioHandleEnum: io_handle,
                signalDriverHandle: signal_handle,
                timeDriverHandle: time_handle,
                clock,
            },
        ))
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.timeDriver.is_enabled()
    }

    pub(crate) fn park(&mut self, handle: &DriverHandle) {
        self.timeDriver.park(handle);
    }

    pub(crate) fn park_timeout(&mut self, handle: &DriverHandle, duration: Duration) {
        self.timeDriver.park_timeout(handle, duration);
    }

    pub(crate) fn shutdown(&mut self, handle: &DriverHandle) {
        self.timeDriver.shutdown(handle);
    }
}

impl DriverHandle {
    pub(crate) fn unpark(&self) {
        #[cfg(feature = "time")]
        if let Some(timeDriverHandle) = &self.timeDriverHandle {
            timeDriverHandle.unpark();
        }

        self.ioHandleEnum.unpark();
    }

    cfg_io_driver! {
        #[track_caller]
        pub(crate) fn io(&self) -> &crate::runtime::io::IODriverHandle {
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
        pub(crate) fn time(&self) -> &crate::runtime::time::TimeDriverHandle {
            self.timeDriverHandle.as_ref().expect("A Tokio 1.x context was found, but timers are disabled. Call `enable_time` on the runtime builder to enable timers.")
        }

        pub(crate) fn clock(&self) -> &Clock {
            &self.clock
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
    Enabled(crate::runtime::io::IODriverHandle),
    Disabled(UnparkThread),
}

#[cfg(any(feature = "net", all(unix, feature = "process"), all(unix, feature = "signal")))]
fn create_io_stack(enableIO: bool, eventCount: usize) -> io::Result<(IoStackEnum, IOHandleEnum, Option<crate::runtime::signal::SignalDriverHandle>)> {
    let ret = if enableIO {
        let (ioDriver, ioDriverHandle) = crate::runtime::io::IODriver::new(eventCount)?;

        let (signalDriver, signalDriverHandle) = create_signal_driver(ioDriver, &ioDriverHandle)?;

        let process_driver = create_process_driver(signalDriver);

        (IoStackEnum::Enabled(process_driver), IOHandleEnum::Enabled(ioDriverHandle), signalDriverHandle)
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

    pub(crate) fn park(&mut self, handle: &DriverHandle) {
        match self {
            IoStackEnum::Enabled(v) => v.park(handle),
            IoStackEnum::Disabled(v) => v.park(),
        }
    }

    pub(crate) fn park_timeout(&mut self, driverHandle: &DriverHandle, duration: Duration) {
        match self {
            IoStackEnum::Enabled(processDriver) => processDriver.park_timeout(driverHandle, duration),
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
            IOHandleEnum::Enabled(handle) => handle.unpark(),
            IOHandleEnum::Disabled(handle) => handle.unpark(),
        }
    }

    pub(crate) fn as_ref(&self) -> Option<&crate::runtime::io::IODriverHandle> {
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

// ===== signal driver =====

#[cfg(any(feature = "signal", all(unix, feature = "process")))]
#[cfg(not(loom))]
fn create_signal_driver(ioDriver: crate::runtime::io::IODriver, ioDriverHandle: &crate::runtime::io::IODriverHandle) -> io::Result<(crate::runtime::signal::SignalDriver, Option<crate::runtime::signal::SignalDriverHandle>)> {
    let driver = crate::runtime::signal::SignalDriver::new(ioDriver, ioDriverHandle)?;
    let handle = driver.handle();
    Ok((driver, Some(handle)))
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

// ===== process driver =====

#[cfg(feature = "process")]
#[cfg_attr(docsrs, doc(cfg(feature = "process")))]
#[cfg(not(loom))]
#[cfg(not(target_os = "wasi"))]
fn create_process_driver(signal_driver: crate::runtime::signal::SignalDriver) -> crate::runtime::process::ProcessDriver {
    ProcessDriver::new(signal_driver)
}


cfg_not_process_driver! {
    cfg_io_driver! {
        type ProcessDriver = SignalDriver;

        fn create_process_driver(signal_driver: SignalDriver) -> ProcessDriver {
            signal_driver
        }
    }
}

// ===== time driver =====

cfg_time! {
    #[derive(Debug)]
    pub(crate) enum TimeDriver {
        Enabled {driver: crate::runtime::time::TimeDriver},
        Disabled(IoStackEnum),
    }

    pub(crate) type Clock = crate::time::Clock;
    pub(crate) type TimeHandle = Option<TimeDriverHandle>;

    fn create_clock(enable_pausing: bool, start_paused: bool) -> Clock {
        Clock::new(enable_pausing, start_paused)
    }

    fn create_time_driver(
        enable: bool,
        io_stack: IoStackEnum,
        clock: &Clock,
        workers: usize,
    ) -> (TimeDriver, TimeHandle) {
        if enable {
            let (driver, handle) = crate::runtime::time::TimeDriver::new(io_stack, clock, workers as u32);

            (TimeDriver::Enabled { driver }, Some(handle))
        } else {
            (TimeDriver::Disabled(io_stack), None)
        }
    }

    impl TimeDriver {
        pub(crate) fn is_enabled(&self) -> bool {
            match self {
                TimeDriver::Enabled { .. } => true,
                TimeDriver::Disabled(inner) => inner.is_enabled(),
            }
        }

        pub(crate) fn park(&mut self, handle: &DriverHandle) {
            match self {
                TimeDriver::Enabled { driver, .. } => driver.park(handle),
                TimeDriver::Disabled(v) => v.park(handle),
            }
        }

        pub(crate) fn park_timeout(&mut self, handle: &DriverHandle, duration: Duration) {
            match self {
                TimeDriver::Enabled { driver } => driver.park_timeout(handle, duration),
                TimeDriver::Disabled(v) => v.park_timeout(handle, duration),
            }
        }

        pub(crate) fn shutdown(&mut self, handle: &DriverHandle) {
            match self {
                TimeDriver::Enabled { driver } => driver.shutdown(handle),
                TimeDriver::Disabled(v) => v.shutdown(handle),
            }
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
    ) -> (TimeDriver, TimeHandle) {
        (io_stack, ())
    }
}
