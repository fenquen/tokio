#![cfg_attr(loom, allow(unused_imports))]

use crate::runtime::handle::RuntimeHandle;
use crate::runtime::{blocking, driver, Callback, Runtime, TaskCallback};
use crate::util::rand::{RngSeed, RngSeedGenerator};

use crate::runtime::blocking::BlockingPool;
use crate::runtime::scheduler::SchedulerHandleEnum;
use std::fmt;
use std::io;
use std::time::Duration;

/// ```
/// use tokio::runtime::Builder;
///
/// fn main() {
///     // build runtime
///     let runtime = Builder::new_multi_thread()
///         .worker_threads(4)
///         .thread_name("my-custom-name")
///         .thread_stack_size(3 * 1024 * 1024)
///         .build()
///         .unwrap();
///
///     // use runtime ...
/// }
/// ```
pub struct Builder {
    /// Runtime type
    kind: Kind,

    /// Whether or not to enable the I/O driver
    enable_io: bool,
    nevents: usize,

    /// Whether or not to enable the time driver
    enable_time: bool,

    /// Whether or not the clock should start paused.
    start_paused: bool,

    /// The number of worker threads, used by Runtime.
    ///
    /// Only used when not using the current-thread executor.
    worker_threads: Option<usize>,

    /// Cap on thread usage.
    max_blocking_threads: usize,

    /// Name fn used for threads spawned by the runtime.
    pub(super) thread_name: ThreadNameFn,

    /// Stack size used for threads spawned by the runtime.
    pub(super) thread_stack_size: Option<usize>,

    /// Callback to run after each thread starts.
    pub(super) after_start: Option<Callback>,

    /// To run before each worker thread stops
    pub(super) before_stop: Option<Callback>,

    /// To run before each worker thread is parked.
    pub(super) before_park: Option<Callback>,

    /// To run after each thread is unparked.
    pub(super) after_unpark: Option<Callback>,

    /// To run before each task is spawned.
    pub(super) before_spawn: Option<TaskCallback>,

    /// To run after each task is terminated.
    pub(super) after_termination: Option<TaskCallback>,

    /// Customizable keep alive timeout for `BlockingPool`
    pub(super) keep_alive: Option<Duration>,

    /// How many ticks before pulling a task from the global/remote queue?
    ///
    /// When `None`, the value is unspecified and behavior details are left to
    /// the scheduler. Each scheduler flavor could choose to either pick its own
    /// default value or use some other strategy to decide when to poll from the
    /// global queue. For example, the multi-threaded scheduler uses a
    /// self-tuning strategy based on mean task poll times.
    pub(super) global_queue_interval: Option<u32>,

    /// How many ticks before yielding to the driver for timer and I/O events?
    pub(super) event_interval: u32,

    pub(super) local_queue_capacity: usize,

    /// When true, the multi-threade scheduler LIFO slot should not be used.
    ///
    /// This option should only be exposed as unstable.
    pub(super) disable_lifo_slot: bool,

    /// Specify a random number generator seed to provide deterministic results
    pub(super) seed_generator: RngSeedGenerator,

    /// When true, enables task poll count histogram instrumentation.
    pub(super) metrics_poll_count_histogram_enable: bool,
}

pub(crate) type ThreadNameFn = std::sync::Arc<dyn Fn() -> String + Send + Sync + 'static>;

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    CurrentThread,
    #[cfg(feature = "rt-multi-thread")]
    MultiThread,
}

impl Builder {
    /// Returns a new builder with the current thread scheduler selected.
    ///
    /// Configuration methods can be chained on the return value.
    ///
    /// To spawn non-`Send` tasks on the resulting runtime, combine it with a
    /// [`LocalSet`].
    ///
    /// [`LocalSet`]: crate::task::LocalSet
    pub fn new_current_thread() -> Builder {
        #[cfg(loom)]
        const EVENT_INTERVAL: u32 = 4;
        // The number `61` is fairly arbitrary. I believe this value was copied from golang.
        #[cfg(not(loom))]
        const EVENT_INTERVAL: u32 = 61;

        Builder::new(Kind::CurrentThread, EVENT_INTERVAL)
    }

    /// Returns a new builder with the multi thread scheduler selected.
    ///
    /// Configuration methods can be chained on the return value.
    #[cfg(feature = "rt-multi-thread")]
    #[cfg_attr(docsrs, doc(cfg(feature = "rt-multi-thread")))]
    pub fn new_multi_thread() -> Builder {
        // The number `61` is fairly arbitrary. I believe this value was copied from golang.
        Builder::new(Kind::MultiThread, 61)
    }

    /// Returns a new runtime builder initialized with default configuration
    /// values.
    ///
    /// Configuration methods can be chained on the return value.
    pub(crate) fn new(kind: Kind, event_interval: u32) -> Builder {
        Builder {
            kind,

            // I/O defaults to "off"
            enable_io: false,
            nevents: 1024,

            // Time defaults to "off"
            enable_time: false,

            // The clock starts not-paused
            start_paused: false,

            // Read from environment variable first in multi-threaded mode.
            // Default to lazy auto-detection (one thread per CPU core)
            worker_threads: None,

            max_blocking_threads: 512,

            // Default thread name
            thread_name: std::sync::Arc::new(|| "tokio-runtime-worker".into()),

            // Do not set a stack size by default
            thread_stack_size: None,

            // No worker thread callbacks
            after_start: None,
            before_stop: None,
            before_park: None,
            after_unpark: None,

            before_spawn: None,
            after_termination: None,

            keep_alive: None,

            // Defaults for these values depend on the scheduler kind, so we get them
            // as parameters.
            global_queue_interval: None,
            event_interval,

            #[cfg(not(loom))]
            local_queue_capacity: 256,

            #[cfg(loom)]
            local_queue_capacity: 4,

            seed_generator: RngSeedGenerator::new(RngSeed::new()),

            metrics_poll_count_histogram_enable: false,

            disable_lifo_slot: false,
        }
    }

    /// Enables both I/O and time drivers.
    pub fn enable_all(&mut self) -> &mut Self {
        #[cfg(any(feature = "net", all(unix, feature = "process"), all(unix, feature = "signal")))]
        self.enable_io();

        #[cfg(feature = "time")]
        self.enable_time();

        self
    }

    /// Sets the number of worker threads the `Runtime` will use.
    #[track_caller]
    pub fn worker_threads(&mut self, val: usize) -> &mut Self {
        assert!(val > 0, "Worker threads cannot be set to 0");
        self.worker_threads = Some(val);
        self
    }

    /// Specifies the limit for additional threads spawned by the Runtime.
    #[track_caller]
    #[cfg_attr(docsrs, doc(alias = "max_threads"))]
    pub fn max_blocking_threads(&mut self, val: usize) -> &mut Self {
        assert!(val > 0, "Max blocking threads cannot be set to 0");
        self.max_blocking_threads = val;
        self
    }

    /// Sets name of threads spawned by the `Runtime`'s thread pool.
    pub fn thread_name(&mut self, val: impl Into<String>) -> &mut Self {
        let val = val.into();
        self.thread_name = std::sync::Arc::new(move || val.clone());
        self
    }

    /// Sets a function used to generate the name of threads spawned by the `Runtime`'s thread pool.
    pub fn thread_name_fn<F: Fn() -> String + Send + Sync + 'static>(&mut self, f: F) -> &mut Self {
        self.thread_name = std::sync::Arc::new(f);
        self
    }

    /// Sets the stack size (in bytes) for worker threads.
    pub fn thread_stack_size(&mut self, val: usize) -> &mut Self {
        self.thread_stack_size = Some(val);
        self
    }

    #[cfg(not(loom))]
    pub fn on_thread_start<F: Fn() + Send + Sync + 'static>(&mut self, f: F) -> &mut Self {
        self.after_start = Some(std::sync::Arc::new(f));
        self
    }

    #[cfg(not(loom))]
    pub fn on_thread_stop<F: Fn() + Send + Sync + 'static>(&mut self, f: F) -> &mut Self {
        self.before_stop = Some(std::sync::Arc::new(f));
        self
    }

    /// Executes function `f` just before a thread is parked (goes idle).
    /// `f` is called within the Tokio context, so functions like [`tokio::spawn`](crate::spawn)
    /// can be called, and may result in this thread being unparked immediately.
    ///
    /// This can be used to start work only when the executor is idle, or for bookkeeping
    /// and monitoring purposes.
    ///
    /// Note: There can only be one park callback for a runtime; calling this function
    /// more than once replaces the last callback defined, rather than adding to it.
    ///
    /// # Examples
    ///
    /// ## Multithreaded executor
    /// ```
    /// # use std::sync::Arc;
    /// # use std::sync::atomic::{AtomicBool, Ordering};
    /// # use tokio::runtime;
    /// # use tokio::sync::Barrier;
    /// # pub fn main() {
    /// let once = AtomicBool::new(true);
    /// let barrier = Arc::new(Barrier::new(2));
    ///
    /// let runtime = runtime::Builder::new_multi_thread()
    ///     .worker_threads(1)
    ///     .on_thread_park({
    ///         let barrier = barrier.clone();
    ///         move || {
    ///             let barrier = barrier.clone();
    ///             if once.swap(false, Ordering::Relaxed) {
    ///                 tokio::spawn(async move { barrier.wait().await; });
    ///            }
    ///         }
    ///     })
    ///     .build()
    ///     .unwrap();
    ///
    /// runtime.block_on(async {
    ///    barrier.wait().await;
    /// })
    /// # }
    /// ```
    /// ## Current thread executor
    /// ```
    /// # use std::sync::Arc;
    /// # use std::sync::atomic::{AtomicBool, Ordering};
    /// # use tokio::runtime;
    /// # use tokio::sync::Barrier;
    /// # pub fn main() {
    /// let once = AtomicBool::new(true);
    /// let barrier = Arc::new(Barrier::new(2));
    ///
    /// let runtime = runtime::Builder::new_current_thread()
    ///     .on_thread_park({
    ///         let barrier = barrier.clone();
    ///         move || {
    ///             let barrier = barrier.clone();
    ///             if once.swap(false, Ordering::Relaxed) {
    ///                 tokio::spawn(async move { barrier.wait().await; });
    ///            }
    ///         }
    ///     })
    ///     .build()
    ///     .unwrap();
    ///
    /// runtime.block_on(async {
    ///    barrier.wait().await;
    /// })
    /// # }
    /// ```
    #[cfg(not(loom))]
    pub fn on_thread_park<F: Fn() + Send + Sync + 'static>(&mut self, f: F) -> &mut Self {
        self.before_park = Some(std::sync::Arc::new(f));
        self
    }

    /// Executes function `f` just after a thread unparks (starts executing tasks).
    ///
    /// This is intended for bookkeeping and monitoring use cases; note that work
    /// in this callback will increase latencies when the application has allowed one or
    /// more runtime threads to go idle.
    ///
    /// Note: There can only be one unpark callback for a runtime; calling this function
    /// more than once replaces the last callback defined, rather than adding to it.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tokio::runtime;
    /// # pub fn main() {
    /// let runtime = runtime::Builder::new_multi_thread()
    ///     .on_thread_unpark(|| {
    ///         println!("thread unparking");
    ///     })
    ///     .build();
    ///
    /// runtime.unwrap().block_on(async {
    ///    tokio::task::yield_now().await;
    ///    println!("Hello from Tokio!");
    /// })
    /// # }
    /// ```
    #[cfg(not(loom))]
    pub fn on_thread_unpark<F: Fn() + Send + Sync + 'static>(&mut self, f: F) -> &mut Self {
        self.after_unpark = Some(std::sync::Arc::new(f));
        self
    }

    pub fn build(&mut self) -> io::Result<Runtime> {
        match &self.kind {
            Kind::CurrentThread => self.build_current_thread_runtime(),
            #[cfg(feature = "rt-multi-thread")]
            Kind::MultiThread => self.build_threaded_runtime(),
        }
    }

    fn get_cfg(&self, workerThreadCount: usize) -> driver::DriverConfig {
        driver::DriverConfig {
            enable_pause_time: match self.kind {
                Kind::CurrentThread => true,
                #[cfg(feature = "rt-multi-thread")]
                Kind::MultiThread => false,
            },
            enable_io: self.enable_io,
            enable_time: self.enable_time,
            start_paused: self.start_paused,
            nevents: self.nevents,
            workerThreadCount,
        }
    }

    /// Sets a custom timeout for a thread in the blocking pool.
    ///
    /// By default, the timeout for a thread is set to 10 seconds. This can
    /// be overridden using `.thread_keep_alive()`.
    pub fn thread_keep_alive(&mut self, duration: Duration) -> &mut Self {
        self.keep_alive = Some(duration);
        self
    }

    /// Sets the number of scheduler ticks after which the scheduler will poll the global
    /// task queue.
    ///
    /// A scheduler "tick" roughly corresponds to one `poll` invocation on a task.
    ///
    /// By default the global queue interval is 31 for the current-thread scheduler. Please see
    /// [the module documentation] for the default behavior of the multi-thread scheduler.
    ///
    /// Schedulers have a local queue of already-claimed tasks, and a global queue of incoming
    /// tasks. Setting the interval to a smaller value increases the fairness of the scheduler,
    /// at the cost of more synchronization overhead. That can be beneficial for prioritizing
    /// getting started on new work, especially if tasks frequently yield rather than complete
    /// or await on further I/O. Conversely, a higher value prioritizes existing work, and
    /// is a good choice when most tasks quickly complete polling.
    ///
    /// [the module documentation]: crate::runtime#multi-threaded-runtime-behavior-at-the-time-of-writing
    #[track_caller]
    pub fn global_queue_interval(&mut self, val: u32) -> &mut Self {
        assert!(val > 0, "global_queue_interval must be greater than 0");
        self.global_queue_interval = Some(val);
        self
    }

    /// Sets the number of scheduler ticks after which the scheduler will poll for
    /// external events (timers, I/O, and so on).
    ///
    /// A scheduler "tick" roughly corresponds to one `poll` invocation on a task.
    ///
    /// By default, the event interval is `61` for all scheduler types.
    ///
    /// Setting the event interval determines the effective "priority" of delivering
    /// these external events (which may wake up additional tasks), compared to
    /// executing tasks that are currently ready to run. A smaller value is useful
    /// when tasks frequently spend a long time in polling, or frequently yield,
    /// which can result in overly long delays picking up I/O events. Conversely,
    /// picking up new events requires extra synchronization and syscall overhead,
    /// so if tasks generally complete their polling quickly, a higher event interval
    /// will minimize that overhead while still keeping the scheduler responsive to
    /// events.
    pub fn event_interval(&mut self, val: u32) -> &mut Self {
        self.event_interval = val;
        self
    }

    fn build_current_thread_runtime(&mut self) -> io::Result<Runtime> {
        use crate::runtime::scheduler::{self, CurrentThread};
        use crate::runtime::{runtime::SchedulerEnum, Config};

        let (driver, driver_handle) = driver::Driver::new(self.get_cfg(1))?;

        // Blocking pool
        let blocking_pool = blocking::create_blocking_pool(self, self.max_blocking_threads);
        let blocking_spawner = blocking_pool.spawner().clone();

        // Generate a rng seed for this runtime.
        let seed_generator_1 = self.seed_generator.next_generator();
        let seed_generator_2 = self.seed_generator.next_generator();

        // And now put a single-threaded scheduler on top of the timer. When
        // there are no futures ready to do something, it'll let the timer or
        // the reactor to generate some new stimuli for the futures to continue
        // in their life.
        let (scheduler, handle) = CurrentThread::new(
            driver,
            driver_handle,
            blocking_spawner,
            seed_generator_2,
            Config {
                before_park: self.before_park.clone(),
                after_unpark: self.after_unpark.clone(),
                before_spawn: self.before_spawn.clone(),
                after_termination: self.after_termination.clone(),
                global_queue_interval: self.global_queue_interval,
                event_interval: self.event_interval,
                local_queue_capacity: self.local_queue_capacity,
                disable_lifo_slot: self.disable_lifo_slot,
                seed_generator: seed_generator_1,
            },
        );

        let handle = RuntimeHandle {
            schedulerHandleEnum: SchedulerHandleEnum::CurrentThread(handle),
        };

        Ok(Runtime::from_parts(
            SchedulerEnum::CurrentThread(scheduler),
            handle,
            blocking_pool,
        ))
    }
}

#[cfg(any(feature = "net", all(unix, feature = "process"), all(unix, feature = "signal"), ))]
impl Builder {
    /// Enables the I/O driver.
    pub fn enable_io(&mut self) -> &mut Self {
        self.enable_io = true;
        self
    }

    /// Enables the I/O driver and configures the max number of events to be processed per tick.
    pub fn max_io_events_per_tick(&mut self, capacity: usize) -> &mut Self {
        self.nevents = capacity;
        self
    }
}

#[cfg(feature = "time")]
#[cfg_attr(docsrs, doc(cfg(feature = "time")))]
impl Builder {
    pub fn enable_time(&mut self) -> &mut Self {
        self.enable_time = true;
        self
    }
}

#[cfg(feature = "rt-multi-thread")]
impl Builder {
    fn build_threaded_runtime(&mut self) -> io::Result<Runtime> {
        use crate::loom::sys::num_cpus;
        use crate::runtime::{runtime::SchedulerEnum, Config};
        use crate::runtime::scheduler::MultiThread;

        let workerThreadCount = self.worker_threads.unwrap_or_else(num_cpus);

        let (driver, driverHandle) = driver::Driver::new(self.get_cfg(workerThreadCount))?;

        // create the blocking pool
        let blockingPool = BlockingPool::new(self, self.max_blocking_threads + workerThreadCount);
        let blockingSpawner = blockingPool.spawner().clone();

        // Generate a rng seed for this runtime.
        let seed_generator_1 = self.seed_generator.next_generator();
        let seed_generator_2 = self.seed_generator.next_generator();

        let (multiThread, multiThreadSchedulerHandle, launcher) =
            MultiThread::new(
                workerThreadCount,
                driver,
                driverHandle,
                blockingSpawner,
                seed_generator_2,
                Config {
                    before_park: self.before_park.clone(),
                    after_unpark: self.after_unpark.clone(),
                    before_spawn: self.before_spawn.clone(),
                    after_termination: self.after_termination.clone(),
                    global_queue_interval: self.global_queue_interval,
                    event_interval: self.event_interval,
                    local_queue_capacity: self.local_queue_capacity,
                    disable_lifo_slot: self.disable_lifo_slot,
                    seed_generator: seed_generator_1,
                },
            );

        let runtimeHandle = RuntimeHandle {
            schedulerHandleEnum: SchedulerHandleEnum::MultiThread(multiThreadSchedulerHandle)
        };

        // Spawn the thread pool workers
        let enter = runtimeHandle.enter();

        launcher.launch();

        Ok(Runtime::from_parts(SchedulerEnum::MultiThread(multiThread), runtimeHandle, blockingPool))
    }
}


impl fmt::Debug for Builder {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("Builder")
            .field("worker_threads", &self.worker_threads)
            .field("max_blocking_threads", &self.max_blocking_threads)
            .field("thread_name", &"<dyn Fn() -> String + Send + Sync + 'static>")
            .field("thread_stack_size", &self.thread_stack_size)
            .field("after_start", &self.after_start.as_ref().map(|_| "..."))
            .field("before_stop", &self.before_stop.as_ref().map(|_| "..."))
            .field("before_park", &self.before_park.as_ref().map(|_| "..."))
            .field("after_unpark", &self.after_unpark.as_ref().map(|_| "..."))
            .finish()
    }
}
