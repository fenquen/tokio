#![cfg_attr(any(not(all(tokio_unstable, feature = "full")), target_family = "wasm"), allow(dead_code))]

use crate::runtime::task;

pub(crate) struct InjectSyncState {
    /// True if the queue is closed.
    pub is_closed: bool,

    /// Linked-list head.
    pub(super) head: Option<task::RawTask>,

    /// Linked-list tail.
    pub(super) tail: Option<task::RawTask>,
}

unsafe impl Send for InjectSyncState {}
unsafe impl Sync for InjectSyncState {}

impl InjectSyncState {
    pub(super) fn pop<T: 'static>(&mut self) -> Option<task::NotifiedTask<T>> {
        let rawTask = self.head?;

        self.head = unsafe { rawTask.get_queue_next() };

        if self.head.is_none() {
            self.tail = None;
        }

        unsafe { rawTask.set_queue_next(None) };

        // safety: a `Notified` is pushed into the queue and now it is popped!
        Some(unsafe { task::NotifiedTask::fromRaw(rawTask) })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.head.is_none()
    }
}
