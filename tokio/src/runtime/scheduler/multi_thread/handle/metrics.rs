use super::MultiThreadSchedulerHandle;

impl MultiThreadSchedulerHandle {
    pub(crate) fn num_alive_tasks(&self) -> usize {
        self.workerSharedState.ownedTasks.num_alive_tasks()
    }
}
