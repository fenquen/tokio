use crate::runtime::task::{Header, RawTask, Schedule};

use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::ops;
use std::ptr::NonNull;
use std::task::{RawWaker, RawWakerVTable, Waker};

pub(super) struct WakerRef<'a, S: 'static> {
    waker: ManuallyDrop<Waker>,
    _p: PhantomData<(&'a Header, S)>,
}

/// Returns a `WakerRef` which avoids having to preemptively increase the
/// refcount if there is no need to do so.
pub(super) fn waker_ref<S: Schedule>(headerPtr: &NonNull<Header>) -> WakerRef<'_, S> {
    // `Waker::will_wake` uses the VTABLE pointer as part of the check. This
    // means that `will_wake` will always return false when using the current
    // task's waker. (discussion at rust-lang/rust#66281).
    //
    // To fix this, we use a single vtable. Since we pass in a reference at this
    // point and not an *owned* waker, we must ensure that `drop` is never
    // called on this waker instance. This is done by wrapping it with
    // `ManuallyDrop` and then never calling drop.
    let waker = unsafe { ManuallyDrop::new(Waker::from_raw(buildRawWaker(*headerPtr))) };

    WakerRef {
        waker,
        _p: PhantomData,
    }
}

impl<S> ops::Deref for WakerRef<'_, S> {
    type Target = Waker;

    fn deref(&self) -> &Waker {
        &self.waker
    }
}

unsafe fn clone_waker(ptr: *const ()) -> RawWaker {
    let header = NonNull::new_unchecked(ptr as *mut Header);
    header.as_ref().state.ref_inc();

    buildRawWaker(header)
}

unsafe fn drop_waker(ptr: *const ()) {
    RawTask::fromHeaderPtr(NonNull::new_unchecked(ptr as *mut Header)).drop_reference();
}

unsafe fn wake_by_val(ptr: *const ()) {
    RawTask::fromHeaderPtr(NonNull::new_unchecked(ptr as *mut Header)).wake_by_val();
}

// Wake without consuming the waker
unsafe fn wake_by_ref(ptr: *const ()) {
    RawTask::fromHeaderPtr(NonNull::new_unchecked(ptr as *mut Header)).wake_by_ref();
}

static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(clone_waker, wake_by_val, wake_by_ref, drop_waker);

fn buildRawWaker(headerPtr: NonNull<Header>) -> RawWaker {
    RawWaker::new(headerPtr.as_ptr() as *const (), &WAKER_VTABLE)
}
