use crate::future::Future;
use crate::loom::sync::Arc;
use crate::runtime::scheduler::multi_thread::worker;
use crate::runtime::{
    blocking, driver,
    task::{self, JoinHandle},
    TaskHooks, TaskMeta,
};
use crate::util::RngSeedGenerator;

use std::fmt;

mod metrics;

pub(crate) struct MultiThreadSchedulerHandle {
    /// Task spawner
    pub(super) workerSharedState: worker::WorkerSharedState,

    /// Resource driver handles
    pub(crate) driverHandle: driver::DriverHandle,

    /// Blocking pool spawner
    pub(crate) blockingSpawner: blocking::Spawner,

    /// Current random number generator seed
    pub(crate) rngSeedGenerator: RngSeedGenerator,

    /// User-supplied hooks to invoke for things
    pub(crate) taskHooks: TaskHooks,
}

impl MultiThreadSchedulerHandle {
    /// Spawns a future onto the thread pool
    pub(crate) fn spawn<F: Future<Output: Send + 'static> + Send + 'static>(me: &Arc<Self>, future: F, id: task::Id) -> JoinHandle<F::Output> {
        let (handle, notified) = me.workerSharedState.ownedTasks.bind(future, me.clone(), id);

        me.taskHooks.spawn(&TaskMeta {
            _phantom: Default::default(),
        });

        if let Some(task) = notified {
            me.scheduleTask(task, false);
        }

        handle
    }

    pub(crate) fn shutdown(&self) {
        self.close();
    }
}

impl fmt::Debug for MultiThreadSchedulerHandle {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("multi_thread::Handle { ... }").finish()
    }
}
