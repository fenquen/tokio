#[cfg(tokio_unstable)]
use crate::runtime;
use crate::runtime::{context, scheduler, RuntimeFlavor};

/// Handle to the runtime.
///
/// The handle is internally reference-counted and can be freely cloned. A handle can be
/// obtained using the [`Runtime::handle`] method.
///
/// [`Runtime::handle`]: crate::runtime::Runtime::handle()
#[derive(Debug, Clone)]
// When the `rt` feature is *not* enabled, this type is still defined, but not
// included in the public API.
pub struct RuntimeHandle {
    pub(crate) schedulerHandleEnum: SchedulerHandleEnum,
}

use crate::runtime::task::JoinHandle;
use crate::runtime::BOX_FUTURE_THRESHOLD;
use crate::util::error::{CONTEXT_MISSING_ERROR, THREAD_LOCAL_DESTROYED_ERROR};

use std::future::Future;
use std::marker::PhantomData;
use std::{error, fmt};
use crate::runtime::scheduler::SchedulerHandleEnum;

/// Runtime context guard.
///
/// Returned by [`Runtime::enter`] and [`RuntimeHandle::enter`], the context guard exits
/// the runtime context on drop.
///
/// [`Runtime::enter`]: fn@crate::runtime::Runtime::enter
#[derive(Debug)]
#[must_use = "Creating and dropping a guard does nothing"]
pub struct EnterGuard<'a> {
    _guard: context::SetCurrentGuard,
    _handle_lifetime: PhantomData<&'a RuntimeHandle>,
}

impl RuntimeHandle {
    /// Enters the runtime context. This allows you to construct types that must
    /// have an executor available on creation such as [`Sleep`] or
    /// [`TcpStream`]. It will also allow you to call methods such as
    /// [`tokio::spawn`] and [`RuntimeHandle::current`] without panicking.
    ///
    /// # Panics
    ///
    /// When calling `Handle::enter` multiple times, the returned guards
    /// **must** be dropped in the reverse order that they were acquired.
    /// Failure to do so will result in a panic and possible memory leaks.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokio::runtime::Runtime;
    ///
    /// let rt = Runtime::new().unwrap();
    ///
    /// let _guard = rt.enter();
    /// tokio::spawn(async {
    ///     println!("Hello world!");
    /// });
    /// ```
    ///
    /// Do **not** do the following, this shows a scenario that will result in a panic and possible memory leak.
    ///
    /// ```should_panic
    /// use tokio::runtime::Runtime;
    ///
    /// let rt1 = Runtime::new().unwrap();
    /// let rt2 = Runtime::new().unwrap();
    ///
    /// let enter1 = rt1.enter();
    /// let enter2 = rt2.enter();
    ///
    /// drop(enter1);
    /// drop(enter2);
    /// ```
    ///
    /// [`Sleep`]: struct@crate::time::Sleep
    /// [`TcpStream`]: struct@crate::net::TcpStream
    /// [`tokio::spawn`]: fn@crate::spawn
    pub fn enter(&self) -> EnterGuard<'_> {
        EnterGuard {
            _guard: match context::trySetCurrentSchedulerHandleEnum(&self.schedulerHandleEnum) {
                Some(guard) => guard,
                None => panic!("{}", THREAD_LOCAL_DESTROYED_ERROR),
            },
            _handle_lifetime: PhantomData,
        }
    }

    #[track_caller]
    pub fn current() -> Self {
        RuntimeHandle {
            schedulerHandleEnum: SchedulerHandleEnum::current(),
        }
    }

    /// Returns a Handle view over the currently running Runtime
    ///
    /// Returns an error if no Runtime has been started
    ///
    /// Contrary to `current`, this never panics
    pub fn try_current() -> Result<Self, TryCurrentError> {
        context::withCurrentSchedulerHandleEnum(|inner| RuntimeHandle {
            schedulerHandleEnum: inner.clone(),
        })
    }

    /// Spawns a future onto the Tokio runtime.
    #[track_caller]
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        if cfg!(debug_assertions) && std::mem::size_of::<F>() > BOX_FUTURE_THRESHOLD {
            self.spawn_named(Box::pin(future), None)
        } else {
            self.spawn_named(future, None)
        }
    }

    /// Runs the provided function on an executor dedicated to blocking operations.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokio::runtime::Runtime;
    ///
    /// # fn dox() {
    /// // Create the runtime
    /// let rt = Runtime::new().unwrap();
    /// // Get a handle from this runtime
    /// let handle = rt.handle();
    ///
    /// // Spawn a blocking function onto the runtime using the handle
    /// handle.spawn_blocking(|| {
    ///     println!("now running on a worker thread");
    /// });
    /// # }
    #[track_caller]
    pub fn spawn_blocking<F, R>(&self, func: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.schedulerHandleEnum.getBlockingSpawner().spawnBlocking(self, func)
    }

    /// Runs a future to completion on this `Handle`'s associated `Runtime`.
    ///
    /// This runs the given future on the current thread, blocking until it is
    /// complete, and yielding its resolved result. Any tasks or timers which
    /// the future spawns internally will be executed on the runtime.
    ///
    /// When this is used on a `current_thread` runtime, only the
    /// [`Runtime::block_on`] method can drive the IO and timer drivers, but the
    /// `Handle::block_on` method cannot drive them. This means that, when using
    /// this method on a `current_thread` runtime, anything that relies on IO or
    /// timers will not work unless there is another thread currently calling
    /// [`Runtime::block_on`] on the same runtime.
    ///
    /// # If the runtime has been shut down
    ///
    /// If the `Handle`'s associated `Runtime` has been shut down (through
    /// [`Runtime::shutdown_background`], [`Runtime::shutdown_timeout`], or by
    /// dropping it) and `Handle::block_on` is used it might return an error or
    /// panic. Specifically IO resources will return an error and timers will
    /// panic. Runtime independent futures will run as normal.
    ///
    /// # Panics
    ///
    /// This function panics if the provided future panics, if called within an
    /// asynchronous execution context, or if a timer future is executed on a
    /// runtime that has been shut down.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokio::runtime::Runtime;
    ///
    /// // Create the runtime
    /// let rt  = Runtime::new().unwrap();
    ///
    /// // Get a handle from this runtime
    /// let handle = rt.handle();
    ///
    /// // Execute the future, blocking the current thread until completion
    /// handle.block_on(async {
    ///     println!("hello");
    /// });
    /// ```
    ///
    /// Or using `Handle::current`:
    ///
    /// ```
    /// use tokio::runtime::RuntimeHandle;
    ///
    /// #[tokio::main]
    /// async fn main () {
    ///     let handle = RuntimeHandle::current();
    ///     std::thread::spawn(move || {
    ///         // Using Handle::block_on to run async code in the new thread.
    ///         handle.block_on(async {
    ///             println!("hello");
    ///         });
    ///     });
    /// }
    /// ```
    ///
    /// [`JoinError`]: struct@crate::task::JoinError
    /// [`JoinHandle`]: struct@crate::task::JoinHandle
    /// [`Runtime::block_on`]: fn@crate::runtime::Runtime::block_on
    /// [`Runtime::shutdown_background`]: fn@crate::runtime::Runtime::shutdown_background
    /// [`Runtime::shutdown_timeout`]: fn@crate::runtime::Runtime::shutdown_timeout
    /// [`spawn_blocking`]: crate::task::spawn_blocking
    /// [`tokio::fs`]: crate::fs
    /// [`tokio::net`]: crate::net
    /// [`tokio::time`]: crate::time
    #[track_caller]
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        if cfg!(debug_assertions) && std::mem::size_of::<F>() > BOX_FUTURE_THRESHOLD {
            self.block_on_inner(Box::pin(future))
        } else {
            self.block_on_inner(future)
        }
    }

    #[track_caller]
    fn block_on_inner<F: Future>(&self, future: F) -> F::Output {
        // Enter the runtime context. This sets the current driver handles and
        // prevents blocking an existing runtime.
        context::enter_runtime(&self.schedulerHandleEnum, true, |blocking| {
            blocking.block_on(future).expect("failed to park thread")
        })
    }

    #[track_caller]
    pub(crate) fn spawn_named<F>(&self, future: F, _name: Option<&str>) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let id = crate::runtime::task::Id::next();
        self.schedulerHandleEnum.spawn(future, id)
    }

    /// Returns the flavor of the current `Runtime`.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokio::runtime::{RuntimeHandle, RuntimeFlavor};
    ///
    /// #[tokio::main(flavor = "current_thread")]
    /// async fn main() {
    ///   assert_eq!(RuntimeFlavor::CurrentThread, RuntimeHandle::current().runtime_flavor());
    /// }
    /// ```
    ///
    /// ```
    /// use tokio::runtime::{RuntimeHandle, RuntimeFlavor};
    ///
    /// #[tokio::main(flavor = "multi_thread", worker_threads = 4)]
    /// async fn main() {
    ///   assert_eq!(RuntimeFlavor::MultiThread, RuntimeHandle::current().runtime_flavor());
    /// }
    /// ```
    pub fn runtime_flavor(&self) -> RuntimeFlavor {
        match self.schedulerHandleEnum {
            scheduler::SchedulerHandleEnum::CurrentThread(_) => RuntimeFlavor::CurrentThread,
            #[cfg(feature = "rt-multi-thread")]
            scheduler::SchedulerHandleEnum::MultiThread(_) => RuntimeFlavor::MultiThread,
        }
    }
}

/// Error returned by `try_current` when no Runtime has been started
#[derive(Debug)]
pub struct TryCurrentError {
    kind: TryCurrentErrorKind,
}

impl TryCurrentError {
    pub(crate) fn new_no_context() -> Self {
        Self {
            kind: TryCurrentErrorKind::NoContext,
        }
    }

    pub(crate) fn new_thread_local_destroyed() -> Self {
        Self {
            kind: TryCurrentErrorKind::ThreadLocalDestroyed,
        }
    }

    /// Returns true if the call failed because there is currently no runtime in
    /// the Tokio context.
    pub fn is_missing_context(&self) -> bool {
        matches!(self.kind, TryCurrentErrorKind::NoContext)
    }

    /// Returns true if the call failed because the Tokio context thread-local
    /// had been destroyed. This can usually only happen if in the destructor of
    /// other thread-locals.
    pub fn is_thread_local_destroyed(&self) -> bool {
        matches!(self.kind, TryCurrentErrorKind::ThreadLocalDestroyed)
    }
}

enum TryCurrentErrorKind {
    NoContext,
    ThreadLocalDestroyed,
}

impl fmt::Debug for TryCurrentErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TryCurrentErrorKind::NoContext => f.write_str("NoContext"),
            TryCurrentErrorKind::ThreadLocalDestroyed => f.write_str("ThreadLocalDestroyed"),
        }
    }
}

impl fmt::Display for TryCurrentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use TryCurrentErrorKind as E;
        match self.kind {
            E::NoContext => f.write_str(CONTEXT_MISSING_ERROR),
            E::ThreadLocalDestroyed => f.write_str(THREAD_LOCAL_DESTROYED_ERROR),
        }
    }
}

impl error::Error for TryCurrentError {}
