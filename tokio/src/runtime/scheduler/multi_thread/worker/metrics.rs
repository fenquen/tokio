use super::workerSharedState;

impl workerSharedState {
    pub(crate) fn injection_queue_depth(&self) -> usize {
        self.injectShared.len()
    }

    pub(crate) fn worker_local_queue_depth(&self, worker: usize) -> usize {
        self.remotes[worker].steal.len()
    }
}
