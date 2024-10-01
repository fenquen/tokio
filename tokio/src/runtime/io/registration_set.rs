use crate::loom::sync::atomic::AtomicUsize;
use crate::runtime::io::ScheduledIO;
use crate::util::linked_list::{self, LinkedList};

use std::io;
use std::ptr::NonNull;
use std::sync::atomic::Ordering::{Acquire, Release};
use std::sync::Arc;

pub(super) struct RegistrationSet {
    num_pending_release: AtomicUsize,
}

pub(super) struct Synced {
    // True when the I/O driver shutdown. At this point, no more registrations should be added to the set.
    is_shutdown: bool,

    // List of all registrations tracked by the set
    registeredScheduledIOList: LinkedList<Arc<ScheduledIO>, ScheduledIO>,

    // Registrations that are pending drop. When a `Registration` is dropped, it
    // stores its `ScheduledIo` in this list. The I/O driver is responsible for
    // dropping it. This ensures the `ScheduledIo` is not freed while it can
    // still be included in an I/O event.
    pendingReleaseScheduledIOs: Vec<Arc<ScheduledIO>>,
}

impl RegistrationSet {
    pub(super) fn new() -> (RegistrationSet, Synced) {
        let set = RegistrationSet {
            num_pending_release: AtomicUsize::new(0),
        };

        let synced = Synced {
            is_shutdown: false,
            registeredScheduledIOList: LinkedList::new(),
            pendingReleaseScheduledIOs: Vec::with_capacity(16),
        };

        (set, synced)
    }

    pub(super) fn is_shutdown(&self, synced: &Synced) -> bool {
        synced.is_shutdown
    }

    /// Returns `true` if there are registrations that need to be released
    pub(super) fn needs_release(&self) -> bool {
        self.num_pending_release.load(Acquire) != 0
    }

    pub(super) fn register(&self, synced: &mut Synced) -> io::Result<Arc<ScheduledIO>> {
        if synced.is_shutdown {
            return Err(io::Error::new(io::ErrorKind::Other, crate::util::error::RUNTIME_SHUTTING_DOWN_ERROR));
        }

        let scheduledIO = Arc::new(ScheduledIO::default());

        // Push a ref into the list of all resources.
        synced.registeredScheduledIOList.push_front(scheduledIO.clone());

        Ok(scheduledIO)
    }

    // returns `true` if the caller should unblock the I/O driver to purge registrations pending release
    pub(super) fn deregister(&self, synced: &mut Synced, registration: &Arc<ScheduledIO>) -> bool {
        // arbitrary, but buffering 16 `ScheduledIo`s doesn't seem like much
        const NOTIFY_AFTER: usize = 16;

        synced.pendingReleaseScheduledIOs.push(registration.clone());

        let len = synced.pendingReleaseScheduledIOs.len();
        self.num_pending_release.store(len, Release);

        len == NOTIFY_AFTER
    }

    pub(super) fn shutdown(&self, synced: &mut Synced) -> Vec<Arc<ScheduledIO>> {
        if synced.is_shutdown {
            return vec![];
        }

        synced.is_shutdown = true;
        synced.pendingReleaseScheduledIOs.clear();

        // Building a vec of all outstanding I/O handles could be expensive, but
        // this is the shutdown operation. In theory, shutdowns should be
        // "clean" with no outstanding I/O resources. Even if it is slow, we
        // aren't optimizing for shutdown.
        let mut ret = vec![];

        while let Some(io) = synced.registeredScheduledIOList.pop_back() {
            ret.push(io);
        }

        ret
    }

    pub(super) fn release(&self, synced: &mut Synced) {
        let pending = std::mem::take(&mut synced.pendingReleaseScheduledIOs);

        for io in pending {
            // safety: the registration is part of our list
            unsafe { self.remove(synced, io.as_ref()) }
        }

        self.num_pending_release.store(0, Release);
    }

    // marked as unsafe, because the caller must make sure that `io` is part of the registration set.
    pub(super) unsafe fn remove(&self, synced: &mut Synced, io: &ScheduledIO) {
        let _ = synced.registeredScheduledIOList.remove(io.into());
    }
}

// Safety: `Arc` pins the inner data
unsafe impl linked_list::Link for Arc<ScheduledIO> {
    type Handle = Arc<ScheduledIO>;
    type Target = ScheduledIO;

    fn as_raw(handle: &Self::Handle) -> NonNull<ScheduledIO> {
        // safety: Arc::as_ptr never returns null
        unsafe { NonNull::new_unchecked(Arc::as_ptr(handle) as *mut _) }
    }

    unsafe fn from_raw(ptr: NonNull<Self::Target>) -> Arc<ScheduledIO> {
        // safety: the linked list currently owns a ref count
        unsafe { Arc::from_raw(ptr.as_ptr() as *const _) }
    }

    unsafe fn pointers(target: NonNull<Self::Target>) -> NonNull<linked_list::Pointers<ScheduledIO>> {
        NonNull::new_unchecked(target.as_ref().linked_list_pointers.get())
    }
}
