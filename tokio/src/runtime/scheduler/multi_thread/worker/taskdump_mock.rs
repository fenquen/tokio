use super::{Core, MultiThreadSchedulerHandle};

impl MultiThreadSchedulerHandle {
    pub(super) fn trace_core(&self, core: Box<Core>) -> Box<Core> {
        core
    }
}
