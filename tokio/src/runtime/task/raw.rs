use crate::future::Future;
use crate::runtime::task::core::{Core, Trailer};
use crate::runtime::task::{Cell, Harness, Header, Id, Schedule, State};

use std::ptr::NonNull;
use std::task::{Poll, Waker};
use crate::runtime::task;

/// Raw task handle
#[derive(Clone)]
pub(crate) struct RawTask {
    headerPtr: NonNull<Header>,
}

pub(super) struct Vtable {
    /// Polls the future.
    pub(super) poll: unsafe fn(NonNull<Header>),

    /// Schedules the task for execution on the runtime.
    pub(super) schedule: unsafe fn(NonNull<Header>),

    /// Deallocates the memory.
    pub(super) dealloc: unsafe fn(NonNull<Header>),

    /// Reads the task output, if complete.
    pub(super) try_read_output: unsafe fn(NonNull<Header>, *mut (), &Waker),

    /// The join handle has been dropped.
    pub(super) drop_join_handle_slow: unsafe fn(NonNull<Header>),

    /// An abort handle has been dropped.
    pub(super) drop_abort_handle: unsafe fn(NonNull<Header>),

    /// Scheduler is being shutdown.
    pub(super) shutdown: unsafe fn(NonNull<Header>),

    /// The number of bytes that the `trailer` field is offset from the header.
    pub(super) trailer_offset: usize,

    /// The number of bytes that the `scheduler` field is offset from the header.
    pub(super) scheduler_offset: usize,

    /// The number of bytes that the `id` field is offset from the header.
    pub(super) id_offset: usize,
}

/// Get the vtable for the requested `T` and `S` generics.
pub(super) fn vtable<T: Future, S: Schedule>() -> &'static Vtable {
    &Vtable {
        poll: poll::<T, S>,
        schedule: schedule::<S>,
        dealloc: dealloc::<T, S>,
        try_read_output: try_read_output::<T, S>,
        drop_join_handle_slow: drop_join_handle_slow::<T, S>,
        drop_abort_handle: drop_abort_handle::<T, S>,
        shutdown: shutdown::<T, S>,
        trailer_offset: OffsetHelper::<T, S>::TRAILER_OFFSET,
        scheduler_offset: OffsetHelper::<T, S>::SCHEDULER_OFFSET,
        id_offset: OffsetHelper::<T, S>::ID_OFFSET,
    }
}

/// Calling `get_trailer_offset` directly in vtable doesn't work because it
/// prevents the vtable from being promoted to a static reference.
///
/// See this thread for more info:
/// <https://users.rust-lang.org/t/custom-vtables-with-integers/78508>
struct OffsetHelper<T, S>(T, S);

impl<T: Future, S: Schedule> OffsetHelper<T, S> {
    // Pass `size_of`/`align_of` as arguments rather than calling them directly
    // inside `get_trailer_offset` because trait bounds on generic parameters
    // of const fn are unstable on our MSRV.
    const TRAILER_OFFSET: usize = get_trailer_offset(
        size_of::<Header>(),
        align_of::<Core<T, S>>(),
        size_of::<Core<T, S>>(),
        align_of::<Trailer>(),
    );

    // scheduler是core里的, the first field of `Core`, so it has the same offset as `Core`.
    const SCHEDULER_OFFSET: usize = get_core_offset(size_of::<Header>(), align_of::<Core<T, S>>());

    const ID_OFFSET: usize = get_id_offset(
        size_of::<Header>(),
        align_of::<Core<T, S>>(),
        size_of::<S>(),
        align_of::<Id>(),
    );
}

/// Compute the offset of the `Trailer` field in `Cell<T, S>` using the `#[repr(C)]` algorithm.
///
/// Pseudo-code for the `#[repr(C)]` algorithm can be found here:
/// <https://doc.rust-lang.org/reference/type-layout.html#reprc-structs>
const fn get_trailer_offset(header_size: usize,
                            core_align: usize,
                            core_size: usize,
                            trailer_align: usize) -> usize {
    let mut offset = header_size;

    let core_misalignment = offset % core_align;
    if core_misalignment > 0 {
        offset += core_align - core_misalignment;
    }

    offset += core_size;

    let trailer_misalign = offset % trailer_align;
    if trailer_misalign > 0 {
        offset += trailer_align - trailer_misalign;
    }

    offset
}

/// Compute the offset of the `Core<T, S>` field in `Cell<T, S>` using the
/// `#[repr(C)]` algorithm.
///
/// Pseudo-code for the `#[repr(C)]` algorithm can be found here:
/// <https://doc.rust-lang.org/reference/type-layout.html#reprc-structs>
const fn get_core_offset(header_size: usize, core_align: usize) -> usize {
    let mut offset = header_size;

    let core_misalign = offset % core_align;
    if core_misalign > 0 {
        offset += core_align - core_misalign;
    }

    offset
}

/// Compute the offset of the `Id` field in `Cell<T, S>` using the
/// `#[repr(C)]` algorithm.
///
/// Pseudo-code for the `#[repr(C)]` algorithm can be found here:
/// <https://doc.rust-lang.org/reference/type-layout.html#reprc-structs>
const fn get_id_offset(header_size: usize,
                       core_align: usize,
                       scheduler_size: usize,
                       id_align: usize) -> usize {
    let mut offset = get_core_offset(header_size, core_align);
    offset += scheduler_size;

    let id_misalign = offset % id_align;
    if id_misalign > 0 {
        offset += id_align - id_misalign;
    }

    offset
}

impl RawTask {
    pub(super) fn new<T: Future, S: Schedule>(task: T, scheduler: S, taskId: Id) -> RawTask {
        let cellPtr = Box::into_raw(Cell::<_, S>::new(task, scheduler, State::new(), taskId));
        RawTask { headerPtr: unsafe { NonNull::new_unchecked(cellPtr.cast()) } }
    }

    pub(super) unsafe fn fromHeaderPtr(headerPtr: NonNull<Header>) -> RawTask {
        RawTask { headerPtr }
    }

    pub(super) fn header_ptr(&self) -> NonNull<Header> {
        self.headerPtr
    }

    pub(super) fn trailer_ptr(&self) -> NonNull<Trailer> {
        unsafe { Header::get_trailer(self.headerPtr) }
    }

    /// Returns a reference to the task's header.
    pub(super) fn header(&self) -> &Header {
        unsafe { self.headerPtr.as_ref() }
    }

    /// Returns a reference to the task's trailer.
    pub(super) fn trailer(&self) -> &Trailer {
        unsafe { &*self.trailer_ptr().as_ptr() }
    }

    /// Returns a reference to the task's state.
    pub(super) fn state(&self) -> &State {
        &self.header().state
    }

    /// Safety: mutual exclusion is required to call this function.
    pub(crate) fn poll(self) {
        unsafe { (self.header().vtable.poll)(self.headerPtr) }
    }

    pub(super) fn schedule(self) {
        unsafe { (self.header().vtable.schedule)(self.headerPtr) }
    }

    pub(super) fn dealloc(self) {
        unsafe { (self.header().vtable.dealloc)(self.headerPtr); }
    }

    /// Safety: `dst` must be a `*mut Poll<super::Result<T::Output>>` where `T`
    /// is the future stored by the task.
    pub(super) unsafe fn try_read_output(self, dst: *mut (), waker: &Waker) {
        (self.header().vtable.try_read_output)(self.headerPtr, dst, waker);
    }

    pub(super) fn drop_join_handle_slow(self) {
        unsafe { (self.header().vtable.drop_join_handle_slow)(self.headerPtr) }
    }

    pub(super) fn drop_abort_handle(self) {
        unsafe { (self.header().vtable.drop_abort_handle)(self.headerPtr) }
    }

    pub(super) fn shutdown(self) {
        unsafe { (self.header().vtable.shutdown)(self.headerPtr) }
    }

    /// Increment the task's reference count.
    ///
    /// Currently, this is used only when creating an `AbortHandle`.
    pub(super) fn ref_inc(self) {
        self.header().state.ref_inc();
    }

    /// Get the queue-next pointer
    ///
    /// This is for usage by the injection queue
    ///
    /// Safety: make sure only one queue uses this and access is synchronized.
    pub(crate) unsafe fn get_queue_next(self) -> Option<RawTask> {
        self.header().queue_next.with(|ptr| *ptr).map(|p| RawTask::fromHeaderPtr(p))
    }

    /// Sets the queue-next pointer
    ///
    /// This is for usage by the injection queue
    ///
    /// Safety: make sure only one queue uses this and access is synchronized.
    pub(crate) unsafe fn set_queue_next(self, val: Option<RawTask>) {
        self.header().set_next(val.map(|task| task.headerPtr));
    }
}

impl Copy for RawTask {}

unsafe fn poll<T: Future, S: Schedule>(headerPtr: NonNull<Header>) {
    Harness::<T, S>::from_raw(headerPtr).poll();
}

unsafe fn schedule<S: Schedule>(headerPtr: NonNull<Header>) {
    use crate::runtime::task::{Notified, Task};

    let scheduler = Header::get_scheduler::<S>(headerPtr);
    scheduler.as_ref().schedule(Notified(Task::from_raw(headerPtr.cast())));
}

unsafe fn dealloc<T: Future, S: Schedule>(headerPtr: NonNull<Header>) {
    Harness::<T, S>::from_raw(headerPtr).dealloc();
}

unsafe fn try_read_output<T: Future, S: Schedule>(
    ptr: NonNull<Header>,
    dst: *mut (),
    waker: &Waker,
) {
    let out = &mut *(dst as *mut Poll<super::Result<T::Output>>);

    let harness = Harness::<T, S>::from_raw(ptr);
    harness.try_read_output(out, waker);
}

unsafe fn drop_join_handle_slow<T: Future, S: Schedule>(ptr: NonNull<Header>) {
    let harness = Harness::<T, S>::from_raw(ptr);
    harness.drop_join_handle_slow();
}

unsafe fn drop_abort_handle<T: Future, S: Schedule>(ptr: NonNull<Header>) {
    let harness = Harness::<T, S>::from_raw(ptr);
    harness.drop_reference();
}

unsafe fn shutdown<T: Future, S: Schedule>(ptr: NonNull<Header>) {
    let harness = Harness::<T, S>::from_raw(ptr);
    harness.shutdown();
}
