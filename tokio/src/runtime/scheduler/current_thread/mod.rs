use crate::future::poll_fn;
use crate::loom::sync::atomic::AtomicBool;
use crate::loom::sync::Arc;
use crate::runtime::driver::{self, Driver};
use crate::runtime::scheduler::{self, Defer, Inject};
use crate::runtime::task::{
    self, JoinHandle, OwnedTasks, Schedule, Task, TaskHarnessScheduleHooks,
};
use crate::runtime::{
    blocking, context, Config, TaskHooks, TaskMeta,
};
use crate::sync::notify::Notify;
use crate::util::atomic_cell::AtomicCell;
use crate::util::{waker_ref, RngSeedGenerator, Wake, WakerRef};

use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::sync::atomic::Ordering::{AcqRel, Release};
use std::task::Poll::{Pending, Ready};
use std::task::Waker;
use std::time::Duration;
use std::fmt;

/// Executes tasks on the current thread
pub(crate) struct CurrentThread {
    /// Core scheduler data is acquired by a thread entering `block_on`.
    core: AtomicCell<Core>,

    /// Notifier for waking up other threads to steal the driver.
    notify: Notify,
}

pub(crate) struct CurrentThreadSchedulerHandle {
    /// Scheduler state shared across threads
    shared: Shared,

    /// Resource driver handles
    pub(crate) driverHandle: driver::DriverHandle,

    /// Blocking pool spawner
    pub(crate) blocking_spawner: blocking::Spawner,

    /// Current random number generator seed
    pub(crate) rngSeedGenerator: RngSeedGenerator,

    /// User-supplied hooks to invoke for things
    pub(crate) task_hooks: TaskHooks,
}

/// Data required for executing the scheduler. The struct is passed around to
/// a function that will perform the scheduling work and acts as a capability token.
struct Core {
    /// Scheduler run queue
    tasks: VecDeque<Notified>,

    /// Current tick
    tick: u32,

    /// Runtime driver
    ///
    /// The driver is removed before starting to park the thread
    driver: Option<Driver>,

    /// How often to check the global queue
    global_queue_interval: u32,

    /// True if a task panicked without being handled and the runtime is
    /// configured to shutdown on unhandled panic.
    unhandled_panic: bool,
}

/// Scheduler state shared between threads.
struct Shared {
    /// Remote run queue
    inject: Inject<Arc<CurrentThreadSchedulerHandle>>,

    /// Collection of all active tasks spawned onto this executor.
    owned: OwnedTasks<Arc<CurrentThreadSchedulerHandle>>,

    /// Indicates whether the blocked on thread was woken.
    woken: AtomicBool,

    /// Scheduler configuration options
    config: Config,
}

/// Thread-local context.
///
/// pub(crate) to store in `runtime::context`.
pub(crate) struct Context {
    /// Scheduler handle
    handle: Arc<CurrentThreadSchedulerHandle>,

    /// Scheduler core, enabling the holder of `Context` to execute the
    /// scheduler.
    core: RefCell<Option<Box<Core>>>,

    /// Deferred tasks, usually ones that called `task::yield_now()`.
    pub(crate) defer: Defer,
}

type Notified = task::Notified<Arc<CurrentThreadSchedulerHandle>>;

/// Initial queue capacity.
const INITIAL_CAPACITY: usize = 64;

/// Used if none is specified. This is a temporary constant and will be removed
/// as we unify tuning logic between the multi-thread and current-thread
/// schedulers.
const DEFAULT_GLOBAL_QUEUE_INTERVAL: u32 = 31;

impl CurrentThread {
    pub(crate) fn new(
        driver: Driver,
        driver_handle: driver::DriverHandle,
        blocking_spawner: blocking::Spawner,
        seed_generator: RngSeedGenerator,
        config: Config,
    ) -> (CurrentThread, Arc<CurrentThreadSchedulerHandle>) {
        // Get the configured global queue interval, or use the default.
        let global_queue_interval = config
            .global_queue_interval
            .unwrap_or(DEFAULT_GLOBAL_QUEUE_INTERVAL);

        let handle = Arc::new(CurrentThreadSchedulerHandle {
            task_hooks: TaskHooks {
                task_spawn_callback: config.before_spawn.clone(),
                task_terminate_callback: config.after_termination.clone(),
            },
            shared: Shared {
                inject: Inject::new(),
                owned: OwnedTasks::new(1),
                woken: AtomicBool::new(false),
                config,
            },
            driverHandle: driver_handle,
            blocking_spawner,
            rngSeedGenerator: seed_generator,
        });

        let core = AtomicCell::new(Some(Box::new(Core {
            tasks: VecDeque::with_capacity(INITIAL_CAPACITY),
            tick: 0,
            driver: Some(driver),
            global_queue_interval,
            unhandled_panic: false,
        })));

        let scheduler = CurrentThread {
            core,
            notify: Notify::new(),
        };

        (scheduler, handle)
    }

    #[track_caller]
    pub(crate) fn block_on<F: Future>(&self, handle: &scheduler::SchedulerHandleEnum, future: F) -> F::Output {
        pin!(future);

        crate::runtime::context::enter_runtime(handle, false, |blocking| {
            let handle = handle.as_current_thread();

            // Attempt to steal the scheduler core and block_on the future if we can
            // there, otherwise, lets select on a notification that the core is
            // available or the future is complete.
            loop {
                if let Some(core) = self.take_core(handle) {
                    return core.block_on(future);
                } else {
                    let notified = self.notify.notified();
                    pin!(notified);

                    if let Some(out) = blocking
                        .block_on(poll_fn(|cx| {
                            if notified.as_mut().poll(cx).is_ready() {
                                return Ready(None);
                            }

                            if let Ready(out) = future.as_mut().poll(cx) {
                                return Ready(Some(out));
                            }

                            Pending
                        }))
                        .expect("Failed to `Enter::block_on`")
                    {
                        return out;
                    }
                }
            }
        })
    }

    fn take_core(&self, handle: &Arc<CurrentThreadSchedulerHandle>) -> Option<CoreGuard<'_>> {
        let core = self.core.take()?;

        Some(CoreGuard {
            context: scheduler::ThreadLocalContextEnum::CurrentThread(Context {
                handle: handle.clone(),
                core: RefCell::new(Some(core)),
                defer: Defer::new(),
            }),
            scheduler: self,
        })
    }

    pub(crate) fn shutdown(&mut self, handle: &scheduler::SchedulerHandleEnum) {
        let handle = handle.as_current_thread();

        // Avoid a double panic if we are currently panicking and
        // the lock may be poisoned.

        let core = match self.take_core(handle) {
            Some(core) => core,
            None if std::thread::panicking() => return,
            None => panic!("Oh no! We never placed the Core back, this is a bug!"),
        };

        // Check that the thread-local is not being destroyed
        let tls_available = context::withCurrentSchedulerHandleEnum(|_| ()).is_ok();

        if tls_available {
            core.enter(|core, _context| {
                let core = shutdown2(core, handle);
                (core, ())
            });
        } else {
            // Shutdown without setting the context. `tokio::spawn` calls will
            // fail, but those will fail either way because the thread-local is
            // not available anymore.
            let context = core.context.expect_current_thread();
            let core = context.core.borrow_mut().take().unwrap();

            let core = shutdown2(core, handle);
            *context.core.borrow_mut() = Some(core);
        }
    }
}

fn shutdown2(mut core: Box<Core>, handle: &CurrentThreadSchedulerHandle) -> Box<Core> {
    // Drain the OwnedTasks collection. This call also closes the
    // collection, ensuring that no tasks are ever pushed after this
    // call returns.
    handle.shared.owned.close_and_shutdown_all(0);

    // Drain local queue
    // We already shut down every task, so we just need to drop the task.
    while let Some(task) = core.next_local_task(handle) {
        drop(task);
    }

    // Close the injection queue
    handle.shared.inject.close();

    // Drain remote queue
    while let Some(task) = handle.shared.inject.pop() {
        drop(task);
    }

    assert!(handle.shared.owned.is_empty());

    // Shutdown the resource drivers
    if let Some(driver) = core.driver.as_mut() {
        driver.shutdown(&handle.driverHandle);
    }

    core
}

impl fmt::Debug for CurrentThread {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("CurrentThread").finish()
    }
}

// ===== impl Core =====

impl Core {
    /// Get and increment the current tick
    fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    fn next_task(&mut self, handle: &CurrentThreadSchedulerHandle) -> Option<Notified> {
        if self.tick % self.global_queue_interval == 0 {
            handle
                .next_remote_task()
                .or_else(|| self.next_local_task(handle))
        } else {
            self.next_local_task(handle)
                .or_else(|| handle.next_remote_task())
        }
    }

    fn next_local_task(&mut self, handle: &CurrentThreadSchedulerHandle) -> Option<Notified> {
        let ret = self.tasks.pop_front();
        ret
    }

    fn push_task(&mut self, handle: &CurrentThreadSchedulerHandle, task: Notified) {
        self.tasks.push_back(task);
    }
}

// ===== impl Context =====

impl Context {
    /// Execute the closure with the given scheduler core stored in the thread-local context.
    fn run_task<R>(&self, core: Box<Core>, f: impl FnOnce() -> R) -> (Box<Core>, R) {
       self.enter(core, || crate::runtime::coop::budget(f))
    }

    /// Blocks the current thread until an event is received by the driver,
    /// including I/O events, timer events, ...
    fn park(&self, mut core: Box<Core>, handle: &CurrentThreadSchedulerHandle) -> Box<Core> {
        let mut driver = core.driver.take().expect("driver missing");

        if let Some(f) = &handle.shared.config.before_park {
            let (c, ()) = self.enter(core, || f());
            core = c;
        }

        // This check will fail if `before_park` spawns a task for us to run
        // instead of parking the thread
        if core.tasks.is_empty() {
            let (c, ()) = self.enter(core, || {
                driver.park_timeout(&handle.driverHandle,None);
                self.defer.wake();
            });

            core = c;
        }

        if let Some(f) = &handle.shared.config.after_unpark {
            let (c, ()) = self.enter(core, || f());
            core = c;
        }

        core.driver = Some(driver);
        core
    }

    /// Checks the driver for new events without blocking the thread.
    fn park_yield(&self, mut core: Box<Core>, handle: &CurrentThreadSchedulerHandle) -> Box<Core> {
        let mut driver = core.driver.take().expect("driver missing");

        let (mut core, ()) = self.enter(core, || {
            driver.park_timeout(&handle.driverHandle, Some(Duration::from_millis(0)));
            self.defer.wake();
        });

        core.driver = Some(driver);
        core
    }

    fn enter<R>(&self, core: Box<Core>, f: impl FnOnce() -> R) -> (Box<Core>, R) {
        // Store the scheduler core in the thread-local context
        //
        // A drop-guard is employed at a higher level.
        *self.core.borrow_mut() = Some(core);

        // Execute the closure while tracking the execution budget
        let ret = f();

        // Take the scheduler core back
        let core = self.core.borrow_mut().take().expect("core missing");
        (core, ret)
    }

    pub(crate) fn defer(&self, waker: &Waker) {
        self.defer.defer(waker);
    }
}

// ===== impl Handle =====

impl CurrentThreadSchedulerHandle {
    /// Spawns a future onto the `CurrentThread` scheduler
    pub(crate) fn spawn<F>(me: &Arc<Self>,
                           future: F,
                           id: task::Id) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let (handle, notified) = me.shared.owned.bind(future, me.clone(), id);

        me.task_hooks.spawn(&TaskMeta {
            _phantom: Default::default(),
        });

        if let Some(notified) = notified {
            me.schedule(notified);
        }

        handle
    }

    fn next_remote_task(&self) -> Option<Notified> {
        self.shared.inject.pop()
    }

    fn waker_ref(me: &Arc<Self>) -> WakerRef<'_> {
        // Set woken to true when enter block_on, ensure outer future
        // be polled for the first time when enter loop
        me.shared.woken.store(true, Release);
        waker_ref(me)
    }

    // reset woken to false and return original value
    pub(crate) fn reset_woken(&self) -> bool {
        self.shared.woken.swap(false, AcqRel)
    }

    pub(crate) fn num_alive_tasks(&self) -> usize {
        self.shared.owned.num_alive_tasks()
    }
}

impl fmt::Debug for CurrentThreadSchedulerHandle {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("current_thread::Handle { ... }").finish()
    }
}

// ===== impl Shared =====

impl Schedule for Arc<CurrentThreadSchedulerHandle> {
    fn release(&self, task: &Task<Self>) -> Option<Task<Self>> {
        self.shared.owned.remove(task)
    }

    fn schedule(&self, task: task::Notified<Self>) {
        use scheduler::ThreadLocalContextEnum::CurrentThread;

        context::withThreadLocalContextEnum(|maybe_cx| match maybe_cx {
            Some(CurrentThread(cx)) if Arc::ptr_eq(self, &cx.handle) => {
                let mut core = cx.core.borrow_mut();

                // If `None`, the runtime is shutting down, so there is no need
                // to schedule the task.
                if let Some(core) = core.as_mut() {
                    core.push_task(self, task);
                }
            }
            _ => {
                // Schedule the task
                self.shared.inject.push(task);
                self.driverHandle.unpark();
            }
        });
    }

    fn hooks(&self) -> TaskHarnessScheduleHooks {
        TaskHarnessScheduleHooks {
            task_terminate_callback: self.task_hooks.task_terminate_callback.clone(),
        }
    }
}

impl Wake for CurrentThreadSchedulerHandle {
    fn wake(arc_self: Arc<Self>) {
        Wake::wake_by_ref(&arc_self);
    }

    /// Wake by reference
    fn wake_by_ref(arc_self: &Arc<Self>) {
        arc_self.shared.woken.store(true, Release);
        arc_self.driverHandle.unpark();
    }
}

// ===== CoreGuard =====

/// Used to ensure we always place the `Core` value back into its slot in
/// `CurrentThread`, even if the future panics.
struct CoreGuard<'a> {
    context: scheduler::ThreadLocalContextEnum,
    scheduler: &'a CurrentThread,
}

impl CoreGuard<'_> {
    #[track_caller]
    fn block_on<F: Future>(self, future: F) -> F::Output {
        let ret = self.enter(|mut core, context| {
            let waker = CurrentThreadSchedulerHandle::waker_ref(&context.handle);
            let mut cx = std::task::Context::from_waker(&waker);

            pin!(future);

            'outer: loop {
                let handle = &context.handle;

                if handle.reset_woken() {
                    let (c, res) = context.enter(core, || {
                        crate::runtime::coop::budget(|| future.as_mut().poll(&mut cx))
                    });

                    core = c;

                    if let Ready(v) = res {
                        return (core, Some(v));
                    }
                }

                for _ in 0..handle.shared.config.event_interval {
                    // Make sure we didn't hit an unhandled_panic
                    if core.unhandled_panic {
                        return (core, None);
                    }

                    core.tick();

                    let entry = core.next_task(handle);

                    let task = match entry {
                        Some(entry) => entry,
                        None => {
                            core = if !context.defer.is_empty() {
                                context.park_yield(core, handle)
                            } else {
                                context.park(core, handle)
                            };

                            // Try polling the `block_on` future next
                            continue 'outer;
                        }
                    };

                    let task = context.handle.shared.owned.assert_owner(task);

                    let (c, ()) = context.run_task(core, || {
                        task.run();
                    });

                    core = c;
                }

                // Yield to the driver, this drives the timer and pulls any
                // pending I/O events.
                core = context.park_yield(core, handle);
            }
        });

        match ret {
            Some(ret) => ret,
            None => {
                // `block_on` panicked.
                panic!("a spawned task panicked and the runtime is configured to shut down on unhandled panic");
            }
        }
    }

    /// Enters the scheduler context. This sets the queue and other necessary
    /// scheduler state in the thread-local.
    fn enter<F, R>(self, f: F) -> R
    where
        F: FnOnce(Box<Core>, &Context) -> (Box<Core>, R),
    {
        let context = self.context.expect_current_thread();

        // Remove `core` from `context` to pass into the closure.
        let core = context.core.borrow_mut().take().expect("core missing");

        // Call the closure and place `core` back
        let (core, ret) = context::set_scheduler(&self.context, || f(core, context));

        *context.core.borrow_mut() = Some(core);

        ret
    }
}

impl Drop for CoreGuard<'_> {
    fn drop(&mut self) {
        let context = self.context.expect_current_thread();

        if let Some(core) = context.core.borrow_mut().take() {
            // Replace old scheduler back into the state to allow
            // other threads to pick it up and drive it.
            self.scheduler.core.set(core);

            // Wake up other possible threads that could steal the driver.
            self.scheduler.notify.notify_one();
        }
    }
}
