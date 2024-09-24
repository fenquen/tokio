//! Multi-threaded runtime

mod counters;
use counters::Counters;

mod handle;
pub(crate) use handle::MultiThreadSchedulerHandle;

mod overflow;
pub(crate) use overflow::Overflow;

mod idle;
use self::idle::Idle;

mod stats;
pub(crate) use stats::Stats;

mod park;
pub(crate) use park::{Parker, UnParker};

pub(crate) mod queue;

mod worker;
pub(crate) use worker::{MultiThreadThreadLocalContext, Launcher, WorkerSharedState};

pub(crate) use worker::block_in_place;

use crate::loom::sync::Arc;
use crate::runtime::{
    blocking,
    driver::{self, Driver},
    scheduler, Config,
};
use crate::util::RngSeedGenerator;

use std::fmt;
use std::future::Future;

/// Work-stealing based thread pool for executing futures.
pub(crate) struct MultiThread;

impl MultiThread {
    pub(crate) fn new(coreThreadCount: usize,
                      driver: Driver,
                      driver_handle: driver::DriverHandle,
                      blocking_spawner: blocking::Spawner,
                      seed_generator: RngSeedGenerator,
                      config: Config) -> (MultiThread, Arc<MultiThreadSchedulerHandle>, Launcher) {
        let parker = Parker::new(driver);

        let (multiThreadSchedulerHandle, launcher) =
            worker::create(coreThreadCount,
                           parker,
                           driver_handle,
                           blocking_spawner,
                           seed_generator,
                           config);

        (MultiThread, multiThreadSchedulerHandle, launcher)
    }

    /// Blocks the current thread waiting for the future to complete.
    ///
    /// The future will execute on the current thread, but all spawned tasks
    /// will be executed on the thread pool.
    pub(crate) fn block_on<F: Future>(&self, handle: &scheduler::SchedulerHandleEnum, future: F) -> F::Output {
        crate::runtime::context::enter_runtime(handle, true, |blocking| {
            blocking.block_on(future).expect("failed to park thread")
        })
    }

    pub(crate) fn shutdown(&mut self, handle: &scheduler::SchedulerHandleEnum) {
        match handle {
            scheduler::SchedulerHandleEnum::MultiThread(handle) => handle.shutdown(),
            _ => panic!("expected MultiThread scheduler"),
        }
    }
}

impl fmt::Debug for MultiThread {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("MultiThread").finish()
    }
}
