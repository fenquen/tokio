use core::mem::MaybeUninit;
use core::ptr;
use std::task::Waker;

const NUM_WAKERS: usize = 32;

pub(crate) struct WakerList {
    inner: [MaybeUninit<Waker>; NUM_WAKERS],
    currentCount: usize,
}

impl WakerList {
    pub(crate) fn new() -> Self {
        const UNINIT_WAKER: MaybeUninit<Waker> = MaybeUninit::uninit();

        Self {
            inner: [UNINIT_WAKER; NUM_WAKERS],
            currentCount: 0,
        }
    }

    #[inline]
    pub(crate) fn can_push(&self) -> bool {
        self.currentCount < NUM_WAKERS
    }

    pub(crate) fn push(&mut self, val: Waker) {
        debug_assert!(self.can_push());

        self.inner[self.currentCount] = MaybeUninit::new(val);
        self.currentCount += 1;
    }

    pub(crate) fn wake_all(&mut self) {
        struct DropGuard {
            start: *mut Waker,
            end: *mut Waker,
        }

        impl Drop for DropGuard {
            fn drop(&mut self) {
                let len = unsafe { self.end.offset_from(self.start) } as usize;
                let slice = ptr::slice_from_raw_parts_mut(self.start, len);
                unsafe { ptr::drop_in_place(slice) };
            }
        }

        debug_assert!(self.currentCount <= NUM_WAKERS);

        let mut dropGuard = {
            let start = self.inner.as_mut_ptr().cast::<Waker>();
            let end = unsafe { start.add(self.currentCount) };

            // Transfer ownership of the wakers in `inner` to `DropGuard`.
            self.currentCount = 0;

            DropGuard { start, end }
        };

        while !ptr::eq(dropGuard.start, dropGuard.end) {
            let waker = unsafe { ptr::read(dropGuard.start) };
            dropGuard.start = unsafe { dropGuard.start.add(1) };
            waker.wake();
        }
    }
}

impl Drop for WakerList {
    fn drop(&mut self) {
        let slice = ptr::slice_from_raw_parts_mut(self.inner.as_mut_ptr().cast::<Waker>(), self.currentCount);
        unsafe { ptr::drop_in_place(slice) };
    }
}
