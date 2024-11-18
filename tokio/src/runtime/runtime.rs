use super::BOX_FUTURE_THRESHOLD;
use crate::runtime::blocking::BlockingPool;
use crate::runtime::scheduler::CurrentThread;
use crate::runtime::{context, EnterGuard, RuntimeHandle};
use crate::task::JoinHandle;

use std::future::Future;
use std::time::Duration;

cfg_rt_multi_thread! {
    use crate::runtime::Builder;
    use crate::runtime::scheduler::MultiThread;
}

/// The Tokio runtime
#[derive(Debug)]
pub struct Runtime {
    /// Task scheduler
    schedulerEnum: SchedulerEnum,

    /// Handle to runtime, also contains driver handles
    runtimeHandle: RuntimeHandle,

    /// Blocking pool handle, used to signal shutdown
    blocking_pool: BlockingPool,
}

/// The flavor of a `Runtime`.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeFlavor {
    /// The flavor that executes all tasks on the current thread.
    CurrentThread,
    /// The flavor that executes tasks across multiple threads.
    MultiThread,
}

#[derive(Debug)]
pub(super) enum SchedulerEnum {
    /// Execute all tasks on the current-thread.
    CurrentThread(CurrentThread),

    /// Execute tasks across multiple threads.
    #[cfg(feature = "rt-multi-thread")]
    MultiThread(MultiThread),

}

impl Runtime {
    pub(super) fn from_parts(schedulerEnum: SchedulerEnum,
                             runtimeHandle: RuntimeHandle,
                             blocking_pool: BlockingPool) -> Runtime {
        Runtime {
            schedulerEnum,
            runtimeHandle,
            blocking_pool,
        }
    }

    #[cfg(feature = "rt-multi-thread")]
    pub fn new() -> std::io::Result<Runtime> {
        Builder::new_multi_thread().enable_all().build()
    }

    pub fn handle(&self) -> &RuntimeHandle {
        &self.runtimeHandle
    }

    /// Spawns a future onto the Tokio runtime.
    #[track_caller]
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        if cfg!(debug_assertions) && std::mem::size_of::<F>() > BOX_FUTURE_THRESHOLD {
            self.runtimeHandle.spawn_named(Box::pin(future), None)
        } else {
            self.runtimeHandle.spawn_named(future, None)
        }
    }

    /// Runs the provided function on an executor dedicated to blocking operations.
    #[track_caller]
    pub fn spawn_blocking<F, R>(&self, func: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.runtimeHandle.spawn_blocking(func)
    }

    /// Runs a future to completion on the Tokio runtime. This is the runtime's entry point.
    ///
    /// [handle]: fn@RuntimeHandle::block_on
    #[track_caller]
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        if cfg!(debug_assertions) && size_of::<F>() > BOX_FUTURE_THRESHOLD {
            self.block_on_inner(Box::pin(future))
        } else {
            self.block_on_inner(future)
        }
    }

    #[track_caller]
    fn block_on_inner<F: Future>(&self, future: F) -> F::Output {
        let _enter = self.enter();

        match &self.schedulerEnum {
            SchedulerEnum::CurrentThread(exec) => exec.block_on(&self.runtimeHandle.schedulerHandleEnum, future),
            #[cfg(feature = "rt-multi-thread")]
            SchedulerEnum::MultiThread(exec) => exec.block_on(&self.runtimeHandle.schedulerHandleEnum, future),
        }
    }

    /// Enters the runtime context.
    ///
    /// This allows you to construct types that must have an executor
    /// available on creation such as [`Sleep`] or [`TcpStream`]. It will
    /// also allow you to call methods such as [`tokio::spawn`].
    ///
    /// [`Sleep`]: struct@crate::time::Sleep
    /// [`TcpStream`]: struct@crate::net::TcpStream
    /// [`tokio::spawn`]: fn@crate::spawn
    ///
    /// # Example
    ///
    /// ```
    /// use tokio::runtime::Runtime;
    /// use tokio::task::JoinHandle;
    ///
    /// fn function_that_spawns(msg: String) -> JoinHandle<()> {
    ///     // Had we not used `rt.enter` below, this would panic.
    ///     tokio::spawn(async move {
    ///         println!("{}", msg);
    ///     })
    /// }
    ///
    /// fn main() {
    ///     let rt = Runtime::new().unwrap();
    ///
    ///     let s = "Hello World!".to_string();
    ///
    ///     // By entering the context, we tie `tokio::spawn` to this executor.
    ///     let _guard = rt.enter();
    ///     let handle = function_that_spawns(s);
    ///
    ///     // Wait for the task before we end the test.
    ///     rt.block_on(handle).unwrap();
    /// }
    /// ```
    pub fn enter(&self) -> EnterGuard<'_> {
        self.runtimeHandle.enter()
    }

    /// Shuts down the runtime, waiting for at most `duration` for all spawned work to stop
    pub fn shutdown_timeout(mut self, duration: Duration) {
        // Wakeup and shutdown all the worker threads
        self.runtimeHandle.schedulerHandleEnum.shutdown();
        self.blocking_pool.shutdown(Some(duration));
    }

    /// Shuts down the runtime, without waiting for any spawned work to stop.
    ///
    /// This can be useful if you want to drop a runtime from within another runtime.
    /// Normally, dropping a runtime will block indefinitely for spawned blocking tasks
    /// to complete, which would normally not be permitted within an asynchronous context.
    /// By calling `shutdown_background()`, you can drop the runtime from such a context.
    ///
    /// Note however, that because we do not wait for any blocking tasks to complete, this
    /// may result in a resource leak (in that any blocking tasks are still running until they
    /// return.
    ///
    /// See the [struct level documentation](Runtime#shutdown) for more details.
    ///
    /// This function is equivalent to calling `shutdown_timeout(Duration::from_nanos(0))`.
    ///
    /// ```
    /// use tokio::runtime::Runtime;
    ///
    /// fn main() {
    ///    let runtime = Runtime::new().unwrap();
    ///
    ///    runtime.block_on(async move {
    ///        let inner_runtime = Runtime::new().unwrap();
    ///        // ...
    ///        inner_runtime.shutdown_background();
    ///    });
    /// }
    /// ```
    pub fn shutdown_background(self) {
        self.shutdown_timeout(Duration::from_nanos(0));
    }
}

#[allow(clippy::single_match)]
impl Drop for Runtime {
    fn drop(&mut self) {
        match &mut self.schedulerEnum {
            SchedulerEnum::CurrentThread(current_thread) => {
                // This ensures that tasks spawned on the current-thread
                // runtime are dropped inside the runtime's context.
                let _guard = context::trySetCurrentSchedulerHandleEnum(&self.runtimeHandle.schedulerHandleEnum);
                current_thread.shutdown(&self.runtimeHandle.schedulerHandleEnum);
            }
            #[cfg(feature = "rt-multi-thread")]
            SchedulerEnum::MultiThread(multi_thread) => {
                // The threaded scheduler drops its tasks on its worker threads, which is already in the runtime's context.
                multi_thread.shutdown(&self.runtimeHandle.schedulerHandleEnum);
            }
        }
    }
}

impl std::panic::UnwindSafe for Runtime {}

impl std::panic::RefUnwindSafe for Runtime {}
