//! A scheduler is initialized with a fixed number of workers. Each worker is
//! driven by a thread. Each worker has a "core" which contains data such as the
//! run queue and other state. When `block_in_place` is called, the worker's
//! "core" is handed off to a new thread allowing the scheduler to continue to
//! make progress while the originating thread blocks.
//!
//! # Shutdown
//!
//! Shutting down the runtime involves the following steps:
//!
//!  1. The Shared::close method is called. This closes the inject queue and
//!     `OwnedTasks` instance and wakes up all worker threads.
//!
//!  2. Each worker thread observes the close signal next time it runs
//!     Core::maintenance by checking whether the inject queue is closed.
//!     The `Core::is_shutdown` flag is set to true.
//!
//!  3. The worker thread calls `pre_shutdown` in parallel. Here, the worker
//!     will keep removing tasks from `OwnedTasks` until it is empty. No new
//!     tasks can be pushed to the `OwnedTasks` during or after this step as it
//!     was closed in step 1.
//!
//!  5. The workers call Shared::shutdown to enter the single-threaded phase of
//!     shutdown. These calls will push their core to `Shared::shutdown_cores`,
//!     and the last thread to push its core will finish the shutdown procedure.
//!
//!  6. The local run queue of each core is emptied, then the inject queue is
//!     emptied.
//!
//! At this point, shutdown has completed. It is not possible for any of the
//! collections to contain any tasks at this point, as each collection was
//! closed first, then emptied afterwards.
//!
//! ## Spawns during shutdown
//!
//! When spawning tasks during shutdown, there are two cases:
//!
//!  * The spawner observes the `OwnedTasks` being open, and the inject queue is
//!    closed.
//!  * The spawner observes the `OwnedTasks` being closed and doesn't check the
//!    inject queue.
//!
//! The first case can only happen if the `OwnedTasks::bind` call happens before
//! or during step 1 of shutdown. In this case, the runtime will clean up the
//! task in step 3 of shutdown.
//!
//! In the latter case, the task was not spawned and the task is immediately
//! cancelled by the spawner.
//!
//! The correctness of shutdown requires both the inject queue and `OwnedTasks`
//! collection to have a closed bit. With a close bit on only the inject queue,
//! spawning could run in to a situation where a task is successfully bound long
//! after the runtime has shut down. With a close bit on only the `OwnedTasks`,
//! the first spawning situation could result in the notification being pushed
//! to the inject queue after step 6 of shutdown, which would leave a task in
//! the inject queue indefinitely. This would be a ref-count cycle and a memory
//! leak.

use crate::loom::sync::{Arc, Mutex};
use crate::runtime;
use crate::runtime::scheduler::multi_thread::{
    idle, queue, Counters, Idle, MultiThreadSchedulerHandle, Overflow, Parker, Stats, UnParker,
};
use crate::runtime::scheduler::{inject, Defer, Lock};
use crate::runtime::task::{OwnedTasks, TaskHarnessScheduleHooks};
use crate::runtime::{
    blocking, coop, driver, scheduler, task, Config,
};
use crate::runtime::{context, TaskHooks};
use crate::util::atomic_cell::AtomicCell;
use crate::util::rand::{FastRand, RngSeedGenerator};

use std::cell::RefCell;
use std::task::Waker;
use std::time::Duration;

/// A scheduler worker
pub(super) struct Worker {
    /// Reference to scheduler's handle
    multiThreadSchedulerHandle: Arc<MultiThreadSchedulerHandle>,

    /// Index holding this worker's remote state
    index: usize,

    /// Used to hand-off a worker's core to another thread.
    core: AtomicCell<Core>,
}

struct Core {
    /// Used to schedule bookkeeping tasks every so often.
    tick: u32,

    /// When a task is scheduled from a worker, it is stored in this slot. The
    /// worker will check this slot for a task **before** checking the run queue.
    /// This effectively results in the **last** scheduled task to be run
    /// next (LIFO). This is an optimization for improving locality which
    /// benefits message passing patterns and helps to reduce latency.
    lifoSlot: Option<Notified>,

    /// When `true`, locally scheduled tasks go to the LIFO slot. When `false`,
    /// they go to the back of the `run_queue`.
    lifoEnabled: bool,

    /// The worker-local run queue.
    runQueue: queue::Local<Arc<MultiThreadSchedulerHandle>>,

    /// True if the worker is currently searching for more work. Searching
    /// involves attempting to steal from other workers.
    is_searching: bool,

    /// True if the scheduler is being shutdown
    is_shutdown: bool,

    /// Parker
    ///
    /// Stored in an `Option` as the parker is added / removed to make the
    /// borrow checker happy.
    park: Option<Parker>,

    /// Per-worker runtime stats
    stats: Stats,

    /// How often to check the global queue
    checkGlobalQueueInterval: u32,

    /// Fast random number generator.
    rand: FastRand,
}

/// State shared across all workers
pub(crate) struct WorkerSharedState {
    /// Per-worker remote state. All other workers have access to this and is how they communicate between each other.
    remotes: Box<[Remote]>,

    /// Global task queue used for:
    ///  1. Submit work to the scheduler while **not** currently on a worker thread.
    ///  2. Submit work to the scheduler when a worker run queue is saturated
    pub(super) injectShared: inject::Shared<Arc<MultiThreadSchedulerHandle>>,

    /// Coordinates idle workers
    idle: Idle,

    /// Collection of all active tasks spawned onto this executor.
    pub(crate) ownedTasks: OwnedTasks<Arc<MultiThreadSchedulerHandle>>,

    /// Data synchronized by the scheduler mutex
    pub(super) synced: Mutex<Synced>,

    /// Cores that have observed the shutdown signal
    ///
    /// The core is **not** placed back in the worker to avoid it from being
    /// stolen by a thread that was spawned as part of `block_in_place`.
    #[allow(clippy::vec_box)] // we're moving an already-boxed value
    shutdown_cores: Mutex<Vec<Box<Core>>>,

    /// Scheduler configuration options
    config: Config,
}

/// Data synchronized by the scheduler mutex
pub(crate) struct Synced {
    /// Synchronized state for `Idle`.
    pub(super) idleSyncState: idle::IdleSyncState,

    /// Synchronized state for `Inject`.
    pub(crate) injectSyncState: inject::InjectSyncState,
}

/// Used to communicate with a worker from other threads.
struct Remote {
    /// Steals tasks from this worker.
    pub(super) steal: queue::Steal<Arc<MultiThreadSchedulerHandle>>,

    /// Unparks the associated worker thread
    unParker: UnParker,
}

/// Thread-local context
pub(crate) struct MultiThreadThreadLocalContext {
    worker: Arc<Worker>,

    core: RefCell<Option<Box<Core>>>,

    /// Tasks to wake after resource drivers are polled. This is mostly to handle yielded tasks.
    pub(crate) defer: Defer,
}

/// Starts the workers
pub(crate) struct Launcher(Vec<Arc<Worker>>);

impl Launcher {
    pub(crate) fn launch(mut self) {
        for worker in self.0.drain(..) {
            runtime::spawn_blocking(move || run(worker));
        }
    }
}

/// Running a task may consume the core. If the core is still available when
/// running the task completes, it is returned. Otherwise, the worker will need
/// to stop processing.
type RunResult = Result<Box<Core>, ()>;

/// A notified task handle
type Notified = task::Notified<Arc<MultiThreadSchedulerHandle>>;

/// Value picked out of thin-air. Running the LIFO slot a handful of times
/// seems sufficient to benefit from locality. More than 3 times probably is
/// overweighing. The value can be tuned in the future with data that shows
/// improvements.
const MAX_LIFO_POLLS_PER_TICK: usize = 3;

pub(super) fn create(workerCount: usize,
                     parker: Parker,
                     driverHandle: driver::DriverHandle,
                     blocking_spawner: blocking::Spawner,
                     seed_generator: RngSeedGenerator,
                     config: Config) -> (Arc<MultiThreadSchedulerHandle>, Launcher) {
    let mut cores = Vec::with_capacity(workerCount);
    let mut remotes = Vec::with_capacity(workerCount);

    // Create the local queues
    for _ in 0..workerCount {
        let (steal, run_queue) = queue::local();

        let parker = parker.clone();
        let unParker = parker.unpark();
        let stats = Stats::new();

        cores.push(Box::new(Core {
            tick: 0,
            lifoSlot: None,
            lifoEnabled: !config.disable_lifo_slot,
            runQueue: run_queue,
            is_searching: false,
            is_shutdown: false,
            park: Some(parker),
            checkGlobalQueueInterval: stats.tuned_global_queue_interval(&config),
            stats,
            rand: FastRand::from_seed(config.seed_generator.next_seed()),
        }));

        remotes.push(Remote { steal, unParker });
    }

    let (idle, idle_synced) = Idle::new(workerCount);
    let (inject, inject_synced) = inject::Shared::new();

    let remotes_len = remotes.len();

    let multiThreadSchedulerHandle = Arc::new(MultiThreadSchedulerHandle {
        taskHooks: TaskHooks {
            task_spawn_callback: config.before_spawn.clone(),
            task_terminate_callback: config.after_termination.clone(),
        },
        workerSharedState: WorkerSharedState {
            remotes: remotes.into_boxed_slice(),
            injectShared: inject,
            idle,
            ownedTasks: OwnedTasks::new(workerCount),
            synced: Mutex::new(Synced {
                idleSyncState: idle_synced,
                injectSyncState: inject_synced,
            }),
            shutdown_cores: Mutex::new(vec![]),
            config,
        },
        driverHandle,
        blockingSpawner: blocking_spawner,
        rngSeedGenerator: seed_generator,
    });

    let mut launcher = Launcher(vec![]);

    for (index, core) in cores.drain(..).enumerate() {
        launcher.0.push(
            Arc::new(Worker {
                multiThreadSchedulerHandle: multiThreadSchedulerHandle.clone(),
                index,
                core: AtomicCell::new(Some(core)),
            }));
    }

    (multiThreadSchedulerHandle, launcher)
}

#[track_caller]
pub(crate) fn block_in_place<F: FnOnce() -> R, R>(f: F) -> R {
    // Try to steal the worker core back
    struct Reset {
        take_core: bool,
        budget: coop::Budget,
    }

    impl Drop for Reset {
        fn drop(&mut self) {
            with_current(|maybe_cx| {
                if let Some(cx) = maybe_cx {
                    if self.take_core {
                        let core = cx.worker.core.take();

                        let mut cx_core = cx.core.borrow_mut();
                        assert!(cx_core.is_none());
                        *cx_core = core;
                    }

                    // Reset the task budget as we are re-entering the runtime.
                    coop::set(self.budget);
                }
            });
        }
    }

    let mut had_entered = false;
    let mut take_core = false;

    let setup_result = with_current(|maybe_cx| {
        match (context::current_enter_context(), maybe_cx.is_some()) {
            (context::EnterRuntime::Entered { .. }, true) => {
                // We are on a thread pool runtime thread, so we just need to set up blocking.
                had_entered = true;
            }
            (context::EnterRuntime::Entered { allow_block_in_place, }, false) => {
                // We are on an executor, but _not_ on the thread pool.  That is
                // _only_ okay if we are in a thread pool runtime's block_on method:
                if allow_block_in_place {
                    had_entered = true;
                    return Ok(());
                }

                // This probably means we are on the current_thread runtime or in a
                // LocalSet, where it is _not_ okay to block.
                return Err("can call blocking only when running on the multi-threaded runtime");
            }
            (context::EnterRuntime::NotEntered, true) => {
                // This is a nested call to block_in_place (we already exited).
                // All the necessary setup has already been done.
                return Ok(());
            }
            (context::EnterRuntime::NotEntered, false) => {
                // We are outside of the tokio runtime, so blocking is fine.
                // We can also skip all of the thread pool blocking setup steps.
                return Ok(());
            }
        }

        let cx = maybe_cx.expect("no .is_some() == false cases above should lead here");

        // Get the worker core. If none is set, then blocking is fine!
        let mut core = match cx.core.borrow_mut().take() {
            Some(core) => core,
            None => return Ok(()),
        };

        // If we heavily call `spawn_blocking`, there might be no available thread to
        // run this core. Except for the task in the lifo_slot, all tasks can be
        // stolen, so we move the task out of the lifo_slot to the run_queue.
        if let Some(task) = core.lifoSlot.take() {
            core.runQueue.push_back_or_overflow(task, &*cx.worker.multiThreadSchedulerHandle);
        }

        // We are taking the core from the context and sending it to another
        // thread.
        take_core = true;

        // The parker should be set here
        assert!(core.park.is_some());

        // In order to block, the core must be sent to another thread for
        // execution.
        //
        // First, move the core back into the worker's shared core slot.
        cx.worker.core.set(core);

        // Next, clone the worker handle and send it to a new thread for
        // processing.
        //
        // Once the blocking task is done executing, we will attempt to
        // steal the core back.
        let worker = cx.worker.clone();
        runtime::spawn_blocking(move || run(worker));
        Ok(())
    });

    if let Err(panic_message) = setup_result {
        panic!("{}", panic_message);
    }

    if had_entered {
        // Unset the current task's budget. Blocking sections are not
        // constrained by task budgets.
        let _reset = Reset {
            take_core,
            budget: coop::stop(),
        };

        context::exit_runtime(f)
    } else {
        f()
    }
}

fn run(worker: Arc<Worker>) {
    #[allow(dead_code)]
    struct AbortOnPanic;

    impl Drop for AbortOnPanic {
        fn drop(&mut self) {
            if std::thread::panicking() {
                eprintln!("worker thread panicking; aborting process");
                std::process::abort();
            }
        }
    }

    // Catching panics on worker threads in tests is quite tricky. Instead, when
    // debug assertions are enabled, we just abort the process.
    #[cfg(debug_assertions)]
    let _abort_on_panic = AbortOnPanic;

    // Acquire a core. If this fails, then another thread is running this worker and there is nothing further to do.
    let core = match worker.core.take() {
        Some(core) => core,
        None => return,
    };

    let schedulerHandleEnum = scheduler::SchedulerHandleEnum::MultiThread(worker.multiThreadSchedulerHandle.clone());

    context::enter_runtime(&schedulerHandleEnum, true, |_| {
        // Set the worker context.
        let threadLocalContextEnum = scheduler::ThreadLocalContextEnum::MultiThread(MultiThreadThreadLocalContext {
            worker,
            core: RefCell::new(None),
            defer: Defer::new(),
        });

        context::set_scheduler(&threadLocalContextEnum, || {
            let multiThreadThreadLocalContext = threadLocalContextEnum.expect_multi_thread();

            // This should always be an error. It only returns a `Result` to support
            // using `?` to short circuit.
            assert!(multiThreadThreadLocalContext.run(core).is_err());

            // Check if there are any deferred tasks to notify. This can happen when
            // the worker core is lost due to `block_in_place()` being called from
            // within the task.
            multiThreadThreadLocalContext.defer.wake();
        });
    });
}

impl MultiThreadThreadLocalContext {
    fn run(&self, mut core: Box<Core>) -> RunResult {
        // Reset `lifo_enabled` here in case the core was previously stolen from
        // a task that had the LIFO slot disabled.
        self.reset_lifo_enabled(&mut core);

        // Start as "processing" tasks as polling tasks from the local queue
        // will be one of the first things we do.
        core.stats.start_processing_scheduled_tasks();

        while !core.is_shutdown {
            self.assert_lifo_enabled_is_correct(&core);

            // Increment the tick
            core.tick();

            // Run maintenance, if needed
            core = self.maintenance(core);

            // First, check work available to the current worker.
            if let Some(notified) = core.next_task(&self.worker) {
                core = self.run_task(notified, core)?;
                continue;
            }

            // We consumed all work in the queues and will start searching for work.
            core.stats.end_processing_scheduled_tasks();

            // There is no more **local** work to process, try to steal work from other workers.
            if let Some(notified) = core.steal_work(&self.worker) {
                // Found work, switch back to processing
                core.stats.start_processing_scheduled_tasks();
                core = self.run_task(notified, core)?;
            } else {
                // Wait for work
                core = if !self.defer.is_empty() {
                    self.park_timeout(core, Some(Duration::from_millis(0)))
                } else {
                    self.park(core)
                };

                core.stats.start_processing_scheduled_tasks();
            }
        }

        core.pre_shutdown(&self.worker);

        // Signal shutdown
        self.worker.multiThreadSchedulerHandle.shutdown_core(core);

        Err(())
    }

    fn run_task(&self, task: Notified, mut core: Box<Core>) -> RunResult {
        let localNotified = self.worker.multiThreadSchedulerHandle.workerSharedState.ownedTasks.assert_owner(task);

        // Make sure the worker is not in the **searching** state. This enables
        // another idle worker to try to steal work.
        core.transition_from_searching(&self.worker);

        self.assert_lifo_enabled_is_correct(&core);

        // Measure the poll start time. Note that we may end up polling other
        // tasks under this measurement. In this case, the tasks came from the
        // LIFO slot and are considered part of the current task for scheduling
        // purposes. These tasks inherent the "parent"'s limits.
        core.stats.start_poll();

        // Make the core available to the runtime context
        *self.core.borrow_mut() = Some(core);

        // Run the task
        coop::budget(|| {
            localNotified.run();

            let mut lifo_polls = 0;

            // As long as there is budget remaining and a task exists in the `lifo_slot`, then keep running.
            loop {
                // Check if we still have the core. If not, the core was stolen by another worker.
                let mut core = match self.core.borrow_mut().take() {
                    Some(core) => core,
                    None => {
                        // In this case, we cannot call `reset_lifo_enabled()`
                        // because the core was stolen. The stealer will handle that at the top of `Context::run`
                        return Err(());
                    }
                };

                // Check for a task in the LIFO slot
                let task = match core.lifoSlot.take() {
                    Some(task) => task,
                    None => {
                        self.reset_lifo_enabled(&mut core);
                        return Ok(core);
                    }
                };

                if !coop::has_budget_remaining() {
                    // Not enough budget left to run the LIFO task, push it to the back of the queue and return.
                    core.runQueue.push_back_or_overflow(task, &*self.worker.multiThreadSchedulerHandle);

                    // If we hit this point, the LIFO slot should be enabled.
                    // There is no need to reset it.
                    debug_assert!(core.lifoEnabled);
                    return Ok(core);
                }

                // Track that we are about to run a task from the LIFO slot.
                lifo_polls += 1;

                // Disable the LIFO slot if we reach our limit
                //
                // In ping-ping style workloads where task A notifies task B,
                // which notifies task A again, continuously prioritizing the
                // LIFO slot can cause starvation as these two tasks will
                // repeatedly schedule the other. To mitigate this, we limit the
                // number of times the LIFO slot is prioritized.
                if lifo_polls >= MAX_LIFO_POLLS_PER_TICK {
                    core.lifoEnabled = false;
                }

                // Run the LIFO task, then loop
                *self.core.borrow_mut() = Some(core);
                let task = self.worker.multiThreadSchedulerHandle.workerSharedState.ownedTasks.assert_owner(task);
                task.run();
            }
        })
    }

    fn reset_lifo_enabled(&self, core: &mut Core) {
        core.lifoEnabled = !self.worker.multiThreadSchedulerHandle.workerSharedState.config.disable_lifo_slot;
    }

    fn assert_lifo_enabled_is_correct(&self, core: &Core) {
        debug_assert_eq!(core.lifoEnabled, !self.worker.multiThreadSchedulerHandle.workerSharedState.config.disable_lifo_slot);
    }

    fn maintenance(&self, mut core: Box<Core>) -> Box<Core> {
        if core.tick % self.worker.multiThreadSchedulerHandle.workerSharedState.config.event_interval == 0 {
            core.stats.end_processing_scheduled_tasks();

            // Call `park` with a 0 timeout. This enables the I/O driver, timer, ...
            // to run without actually putting the thread to sleep.
            core = self.park_timeout(core, Some(Duration::from_millis(0)));

            // Run regularly scheduled maintenance
            core.maintenance(&self.worker);

            core.stats.start_processing_scheduled_tasks();
        }

        core
    }

    /// Parks the worker thread while waiting for tasks to execute.
    ///
    /// This function checks if indeed there's no more work left to be done before parking.
    /// Also important to notice that, before parking, the worker thread will try to take
    /// ownership of the Driver (IO/Time) and dispatch any events that might have fired.
    /// Whenever a worker thread executes the Driver loop, all waken tasks are scheduled
    /// in its own local queue until the queue saturates (ntasks > `LOCAL_QUEUE_CAPACITY`).
    /// When the local queue is saturated, the overflow tasks are added to the injection queue
    /// from where other workers can pick them up.
    /// Also, we rely on the workstealing algorithm to spread the tasks amongst workers
    /// after all the IOs get dispatched
    fn park(&self, mut core: Box<Core>) -> Box<Core> {
        if let Some(f) = &self.worker.multiThreadSchedulerHandle.workerSharedState.config.before_park {
            f();
        }

        if core.transition_to_parked(&self.worker) {
            while !core.is_shutdown  {
                core = self.park_timeout(core, None);

                // Run regularly scheduled maintenance
                core.maintenance(&self.worker);

                if core.transition_from_parked(&self.worker) {
                    break;
                }
            }
        }

        if let Some(f) = &self.worker.multiThreadSchedulerHandle.workerSharedState.config.after_unpark {
            f();
        }

        core
    }

    fn park_timeout(&self, mut core: Box<Core>, duration: Option<Duration>) -> Box<Core> {
        self.assert_lifo_enabled_is_correct(&core);

        // Take the parker out of core
        let mut park = core.park.take().expect("park missing");

        // Store `core` in context
        *self.core.borrow_mut() = Some(core);

        // Park thread
        if let Some(timeout) = duration {
            park.park_timeout(&self.worker.multiThreadSchedulerHandle.driverHandle, timeout);
        } else {
            park.park(&self.worker.multiThreadSchedulerHandle.driverHandle);
        }

        self.defer.wake();

        // Remove `core` from context
        core = self.core.borrow_mut().take().expect("core missing");

        // Place `park` back in `core`
        core.park = Some(park);

        if core.should_notify_others() {
            self.worker.multiThreadSchedulerHandle.notify_parked_local();
        }

        core
    }

    pub(crate) fn defer(&self, waker: &Waker) {
        self.defer.defer(waker);
    }

    #[allow(dead_code)]
    pub(crate) fn get_worker_index(&self) -> usize {
        self.worker.index
    }
}

impl Core {
    /// Increment the tick
    fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    /// Return the next notified task available to this worker.
    fn next_task(&mut self, worker: &Worker) -> Option<Notified> {
        if self.tick % self.checkGlobalQueueInterval == 0 {
            // Update the global queue interval, if needed
            self.tune_global_queue_interval(worker);

            worker.multiThreadSchedulerHandle.next_remote_task().or_else(|| self.next_local_task())
        } else {
            let notified = self.next_local_task();

            // local上空掉了
            if notified.is_some() {
                return notified;
            }

            // 到不是local的上边看看 要是有的话搬到local
            if worker.multiThreadSchedulerHandle.workerSharedState.injectShared.is_empty() {
                return None;
            }

            // Other threads can only **remove** tasks from the current worker's
            // `run_queue`. So, we can be confident that by the time we call
            // `run_queue.push_back` below, there will be *at least* `cap`
            // available slots in the queue.
            let cap = usize::min(self.runQueue.remaining_slots(), self.runQueue.max_capacity() / 2);

            // The worker is currently idle, pull a batch of work from the
            // injection queue. We don't want to pull *all* the work so other
            // workers can also get some.
            let n = usize::min(worker.multiThreadSchedulerHandle.workerSharedState.injectShared.len() / worker.multiThreadSchedulerHandle.workerSharedState.remotes.len() + 1, cap);

            // Take at least one task since the first task is returned directly and not pushed onto the local queue.
            let n = usize::max(1, n);

            let mut synced = worker.multiThreadSchedulerHandle.workerSharedState.synced.lock();

            // safety: passing in the correct `inject::Synced`.
            let mut tasks = unsafe {
                worker.multiThreadSchedulerHandle.workerSharedState.injectShared.pop_n(&mut synced.injectSyncState, n)
            };

            // Pop the first task to return immediately
            let ret = tasks.next();

            // Push the rest of the on the run queue
            self.runQueue.push_back(tasks);

            ret
        }
    }

    fn next_local_task(&mut self) -> Option<Notified> {
        self.lifoSlot.take().or_else(|| self.runQueue.pop())
    }

    /// Function responsible for stealing tasks from another worker
    ///
    /// Note: Only if less than half the workers are searching for tasks to steal
    /// a new worker will actually try to steal. The idea is to make sure not all
    /// workers will be trying to steal at the same time.
    fn steal_work(&mut self, worker: &Worker) -> Option<Notified> {
        if !self.transition_to_searching(worker) {
            return None;
        }

        let remoteLen = worker.multiThreadSchedulerHandle.workerSharedState.remotes.len();

        // 通过随机数来确定由哪个remote起始来steal
        let start = self.rand.fastrand_n(remoteLen as u32) as usize;

        for i in 0..remoteLen {
            let i = (start + i) % remoteLen;

            // Don't steal from ourself 这是当前的worker
            if i == worker.index {
                continue;
            }

            let targetRemote = &worker.multiThreadSchedulerHandle.workerSharedState.remotes[i];

            if let Some(task) = targetRemote.steal.steal_into(&mut self.runQueue) {
                return Some(task);
            }
        }

        // Fallback on checking the global queue
        worker.multiThreadSchedulerHandle.next_remote_task()
    }

    fn transition_to_searching(&mut self, worker: &Worker) -> bool {
        if !self.is_searching {
            self.is_searching = worker.multiThreadSchedulerHandle.workerSharedState.idle.transition_worker_to_searching();
        }

        self.is_searching
    }

    fn transition_from_searching(&mut self, worker: &Worker) {
        if !self.is_searching {
            return;
        }

        self.is_searching = false;
        worker.multiThreadSchedulerHandle.transition_worker_from_searching();
    }

    fn has_tasks(&self) -> bool {
        self.lifoSlot.is_some() || self.runQueue.has_tasks()
    }

    fn should_notify_others(&self) -> bool {
        // If there are tasks available to steal, but this worker is not
        // looking for tasks to steal, notify another worker.
        if self.is_searching {
            return false;
        }
        self.lifoSlot.is_some() as usize + self.runQueue.len() > 1
    }

    /// Prepares the worker state for parking.
    ///
    /// Returns true if the transition happened, false if there is work to do first.
    fn transition_to_parked(&mut self, worker: &Worker) -> bool {
        // Workers should not park if they have work to do
        if self.has_tasks() {
            return false;
        }

        // When the final worker transitions **out** of searching to parked, it
        // must check all the queues one last time in case work materialized
        // between the last work scan and transitioning out of searching.
        let is_last_searcher = worker.multiThreadSchedulerHandle.workerSharedState.idle.transition_worker_to_parked(
            &worker.multiThreadSchedulerHandle.workerSharedState,
            worker.index,
            self.is_searching,
        );

        // The worker is no longer searching. Setting this is the local cache
        // only.
        self.is_searching = false;

        if is_last_searcher {
            worker.multiThreadSchedulerHandle.notify_if_work_pending();
        }

        true
    }

    /// Returns `true` if the transition happened.
    fn transition_from_parked(&mut self, worker: &Worker) -> bool {
        // If a task is in the lifo slot/run queue, then we must unpark regardless of being notified
        if self.has_tasks() {
            // When a worker wakes, it should only transition to the "searching"
            // state when the wake originates from another worker *or* a new task
            // is pushed. We do *not* want the worker to transition to "searching"
            // when it wakes when the I/O driver receives new events.
            self.is_searching = !worker.multiThreadSchedulerHandle.workerSharedState.idle.unpark_worker_by_id(&worker.multiThreadSchedulerHandle.workerSharedState, worker.index);
            return true;
        }

        if worker.multiThreadSchedulerHandle.workerSharedState.idle.is_parked(&worker.multiThreadSchedulerHandle.workerSharedState, worker.index) {
            return false;
        }

        // When unparked, the worker is in the searching state.
        self.is_searching = true;
        true
    }

    /// Runs maintenance work such as checking the pool's state.
    fn maintenance(&mut self, worker: &Worker) {
        if !self.is_shutdown {
            // Check if the scheduler has been shutdown
            let synced = worker.multiThreadSchedulerHandle.workerSharedState.synced.lock();
            self.is_shutdown = worker.multiThreadSchedulerHandle.workerSharedState.injectShared.is_closed(&synced.injectSyncState);
        }
    }

    /// Signals all tasks to shut down, and waits for them to complete. Must run
    /// before we enter the single-threaded phase of shutdown processing.
    fn pre_shutdown(&mut self, worker: &Worker) {
        // Start from a random inner list
        let start = self.rand.fastrand_n(worker.multiThreadSchedulerHandle.workerSharedState.ownedTasks.get_shard_size() as u32);

        // Signal to all tasks to shut down.
        worker.multiThreadSchedulerHandle.workerSharedState.ownedTasks.close_and_shutdown_all(start as usize);
    }

    /// Shuts down the core.
    fn shutdown(&mut self, handle: &MultiThreadSchedulerHandle) {
        // Take the core
        let mut park = self.park.take().expect("park missing");

        // Drain the queue
        while self.next_local_task().is_some() {}

        park.shutdown(&handle.driverHandle);
    }

    fn tune_global_queue_interval(&mut self, worker: &Worker) {
        let next = self.stats.tuned_global_queue_interval(&worker.multiThreadSchedulerHandle.workerSharedState.config);

        // Smooth out jitter
        if u32::abs_diff(self.checkGlobalQueueInterval, next) > 2 {
            self.checkGlobalQueueInterval = next;
        }
    }
}

// TODO: Move `Handle` impls into handle.rs
impl task::Schedule for Arc<MultiThreadSchedulerHandle> {
    fn release(&self, task: &task::Task<Arc<MultiThreadSchedulerHandle>>) -> Option<task::Task<Arc<MultiThreadSchedulerHandle>>> {
        self.workerSharedState.ownedTasks.remove(task)
    }

    fn schedule(&self, task: Notified) {
        self.scheduleTask(task, false);
    }

    fn hooks(&self) -> TaskHarnessScheduleHooks {
        TaskHarnessScheduleHooks {
            task_terminate_callback: self.taskHooks.task_terminate_callback.clone(),
        }
    }

    fn yield_now(&self, task: Notified) {
        self.scheduleTask(task, true);
    }
}

impl MultiThreadSchedulerHandle {
    pub(super) fn scheduleTask(&self, task: Notified, is_yield: bool) {
        with_current(|multiThreadThreadLocalContext| {
            if let Some(multiThreadThreadLocalContext) = multiThreadThreadLocalContext {
                // Make sure the task is part of the **current** scheduler.
                if self.ptr_eq(&multiThreadThreadLocalContext.worker.multiThreadSchedulerHandle) {
                    // And the current thread still holds a core
                    if let Some(core) = multiThreadThreadLocalContext.core.borrow_mut().as_mut() {
                        self.scheduleLocal(core, task, is_yield);
                        return;
                    }
                }
            }

            // otherwise, use the inject queue.
            self.push_remote_task(task);
            self.notify_parked_remote();
        });
    }

    pub(super) fn schedule_option_task_without_yield(&self, task: Option<Notified>) {
        if let Some(task) = task {
            self.scheduleTask(task, false);
        }
    }

    fn scheduleLocal(&self, core: &mut Core, task: Notified, is_yield: bool) {
        // Spawning from the worker thread. If scheduling a "yield" then the
        // task must always be pushed to the back of the queue, enabling other
        // tasks to be executed. If **not** a yield, then there is more
        // flexibility and the task may go to the front of the queue.
        let should_notify = if is_yield || !core.lifoEnabled {
            core.runQueue.push_back_or_overflow(task, self);
            true
        } else {
            // Push to the LIFO slot
            let prev = core.lifoSlot.take();
            let ret = prev.is_some();

            if let Some(prev) = prev {
                core.runQueue.push_back_or_overflow(prev, self);
            }

            core.lifoSlot = Some(task);

            ret
        };

        // Only notify if not currently parked. If `park` is `None`, then the
        // scheduling is from a resource driver. As notifications often come in
        // batches, the notification is delayed until the park is complete.
        if should_notify && core.park.is_some() {
            self.notify_parked_local();
        }
    }

    fn next_remote_task(&self) -> Option<Notified> {
        if self.workerSharedState.injectShared.is_empty() {
            return None;
        }

        let mut synced = self.workerSharedState.synced.lock();

        // safety: passing in correct `idle::Synced`
        unsafe { self.workerSharedState.injectShared.pop(&mut synced.injectSyncState) }
    }

    fn push_remote_task(&self, task: Notified) {
        let mut synced = self.workerSharedState.synced.lock();
        // safety: passing in correct `idle::Synced`
        unsafe {
            self.workerSharedState.injectShared.push(&mut synced.injectSyncState, task);
        }
    }

    pub(super) fn close(&self) {
        if self.workerSharedState.injectShared.close(&mut self.workerSharedState.synced.lock().injectSyncState) {
            self.notify_all();
        }
    }

    fn notify_parked_local(&self) {
        if let Some(index) = self.workerSharedState.idle.worker_to_notify(&self.workerSharedState) {
            self.workerSharedState.remotes[index].unParker.unpark(&self.driverHandle);
        }
    }

    fn notify_parked_remote(&self) {
        if let Some(index) = self.workerSharedState.idle.worker_to_notify(&self.workerSharedState) {
            self.workerSharedState.remotes[index].unParker.unpark(&self.driverHandle);
        }
    }

    pub(super) fn notify_all(&self) {
        for remote in &self.workerSharedState.remotes[..] {
            remote.unParker.unpark(&self.driverHandle);
        }
    }

    fn notify_if_work_pending(&self) {
        for remote in &self.workerSharedState.remotes[..] {
            if !remote.steal.is_empty() {
                self.notify_parked_local();
                return;
            }
        }

        if !self.workerSharedState.injectShared.is_empty() {
            self.notify_parked_local();
        }
    }

    fn transition_worker_from_searching(&self) {
        if self.workerSharedState.idle.transition_worker_from_searching() {
            // We are the final searching worker. Because work was found, we need to notify another worker.
            self.notify_parked_local();
        }
    }

    /// Signals that a worker has observed the shutdown signal and has replaced
    /// its core back into its handle.
    ///
    /// If all workers have reached this point, the final cleanup is performed.
    fn shutdown_core(&self, core: Box<Core>) {
        let mut cores = self.workerSharedState.shutdown_cores.lock();
        cores.push(core);

        if cores.len() != self.workerSharedState.remotes.len() {
            return;
        }

        debug_assert!(self.workerSharedState.ownedTasks.is_empty());

        for mut core in cores.drain(..) {
            core.shutdown(self);
        }

        // Drain the injection queue
        //
        // We already shut down every task, so we can simply drop the tasks.
        while let Some(task) = self.next_remote_task() {
            drop(task);
        }
    }

    fn ptr_eq(&self, other: &MultiThreadSchedulerHandle) -> bool {
        std::ptr::eq(self, other)
    }
}

impl Overflow<Arc<MultiThreadSchedulerHandle>> for MultiThreadSchedulerHandle {
    fn push(&self, task: task::Notified<Arc<MultiThreadSchedulerHandle>>) {
        self.push_remote_task(task);
    }

    fn push_batch<I>(&self, iter: I)
    where
        I: Iterator<Item=task::Notified<Arc<MultiThreadSchedulerHandle>>>,
    {
        unsafe {
            self.workerSharedState.injectShared.push_batch(self, iter);
        }
    }
}

pub(crate) struct InjectGuard<'a> {
    lock: crate::loom::sync::MutexGuard<'a, Synced>,
}

impl<'a> AsMut<inject::InjectSyncState> for InjectGuard<'a> {
    fn as_mut(&mut self) -> &mut inject::InjectSyncState {
        &mut self.lock.injectSyncState
    }
}

impl<'a> Lock<inject::InjectSyncState> for &'a MultiThreadSchedulerHandle {
    type Handle = InjectGuard<'a>;

    fn lock(self) -> Self::Handle {
        InjectGuard {
            lock: self.workerSharedState.synced.lock(),
        }
    }
}

#[track_caller]
fn with_current<R>(f: impl FnOnce(Option<&MultiThreadThreadLocalContext>) -> R) -> R {
    use scheduler::ThreadLocalContextEnum::MultiThread;

    context::with_scheduler(|ctx| match ctx {
        Some(MultiThread(ctx)) => f(Some(ctx)),
        _ => f(None),
    })
}
