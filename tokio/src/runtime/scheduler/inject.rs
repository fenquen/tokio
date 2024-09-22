//! Inject queue used to send wakeups to a work-stealing scheduler

use crate::loom::sync::Mutex;
use crate::runtime::task;

mod pop;
pub(crate) use pop::Pop;

mod shared;
pub(crate) use shared::Shared;

mod synced;
pub(crate) use synced::InjectSyncState;

cfg_rt_multi_thread! {
    mod rt_multi_thread;
}

/// Growable, MPMC queue used to inject new tasks into the scheduler and as an
/// overflow queue when the local, fixed-size, array queue overflows.
pub(crate) struct Inject<T: 'static> {
    shared: Shared<T>,
    synced: Mutex<InjectSyncState>,
}

impl<T: 'static> Inject<T> {
    pub(crate) fn new() -> Inject<T> {
        let (shared, synced) = Shared::new();

        Inject {
            shared,
            synced: Mutex::new(synced),
        }
    }

    /// Closes the injection queue, returns `true` if the queue is open when the
    /// transition is made.
    pub(crate) fn close(&self) -> bool {
        let mut synced = self.synced.lock();
        self.shared.close(&mut synced)
    }

    /// Pushes a value into the queue.
    ///
    /// This does nothing if the queue is closed.
    pub(crate) fn push(&self, task: task::Notified<T>) {
        let mut synced = self.synced.lock();
        // safety: passing correct `Synced`
        unsafe { self.shared.push(&mut synced, task) }
    }

    pub(crate) fn pop(&self) -> Option<task::Notified<T>> {
        if self.shared.is_empty() {
            return None;
        }

        let mut synced = self.synced.lock();
        // safety: passing correct `Synced`
        unsafe { self.shared.pop(&mut synced) }
    }
}
