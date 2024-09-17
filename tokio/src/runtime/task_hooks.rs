use std::marker::PhantomData;

impl TaskHooks {
    pub(crate) fn spawn(&self, meta: &TaskMeta<'_>) {
        if let Some(f) = self.task_spawn_callback.as_ref() {
            f(meta)
        }
    }
}

#[derive(Clone)]
pub(crate) struct TaskHooks {
    pub(crate) task_spawn_callback: Option<TaskCallback>,
    pub(crate) task_terminate_callback: Option<TaskCallback>,
}

/// Task metadata supplied to user-provided hooks for task events.
#[allow(missing_debug_implementations)]
#[cfg_attr(not(tokio_unstable), allow(unreachable_pub))]
pub struct TaskMeta<'a> {
    pub(crate) _phantom: PhantomData<&'a ()>,
}

impl<'a> TaskMeta<'a> {
}

/// Runs on specific task-related events
pub(crate) type TaskCallback = std::sync::Arc<dyn Fn(&TaskMeta<'_>) + Send + Sync>;
