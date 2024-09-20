use super::{Context, CONTEXT};

use crate::runtime::{scheduler, TryCurrentError};
use crate::util::markers::SyncNotSend;

use std::cell::{Cell, RefCell};
use std::marker::PhantomData;

#[derive(Debug)]
#[must_use]
pub(crate) struct SetCurrentGuard {
    prevSchedulerHandleEnumCell: Option<scheduler::SchedulerHandleEnum>,

    // The depth for this guard
    depth: usize,

    // Don't let the type move across threads.
    _p: PhantomData<SyncNotSend>,
}

impl Drop for SetCurrentGuard {
    fn drop(&mut self) {
        CONTEXT.with(|ctx| {
            let depth = ctx.currentSchedulerHandleEnumCell.depth.get();

            if depth != self.depth {
                if !std::thread::panicking() {
                    panic!(
                        "`EnterGuard` values dropped out of order. Guards returned by \
                         `tokio::runtime::Handle::enter()` must be dropped in the reverse order as they were acquired."
                    );
                }

                return;
            }

            *ctx.currentSchedulerHandleEnumCell.schedulerHandleEnum.borrow_mut() = self.prevSchedulerHandleEnumCell.take();
            ctx.currentSchedulerHandleEnumCell.depth.set(depth - 1);
        });
    }
}

pub(super) struct SchedulerHandleEnumCell {
    schedulerHandleEnum: RefCell<Option<scheduler::SchedulerHandleEnum>>,

    /// Tracks the number of nested calls to `try_set_current`.
    depth: Cell<usize>,
}

impl SchedulerHandleEnumCell {
    pub(super) const fn new() -> SchedulerHandleEnumCell {
        SchedulerHandleEnumCell {
            schedulerHandleEnum: RefCell::new(None),
            depth: Cell::new(0),
        }
    }
}

pub(crate) fn trySetCurrentSchedulerHandleEnum(schedulerHandleEnum: &scheduler::SchedulerHandleEnum) -> Option<SetCurrentGuard> {
    CONTEXT.try_with(|ctx| ctx.set_current(schedulerHandleEnum)).ok()
}

pub(crate) fn withCurrentSchedulerHandleEnum<F, R>(f: F) -> Result<R, TryCurrentError>
where
    F: FnOnce(&scheduler::SchedulerHandleEnum) -> R,
{
    match CONTEXT.try_with(|ctx| ctx.currentSchedulerHandleEnumCell.schedulerHandleEnum.borrow().as_ref().map(f)) {
        Ok(Some(ret)) => Ok(ret),
        Ok(None) => Err(TryCurrentError::new_no_context()),
        Err(_access_error) => Err(TryCurrentError::new_thread_local_destroyed()),
    }
}

impl Context {
    pub(super) fn set_current(&self, schedulerHandleEnum: &scheduler::SchedulerHandleEnum) -> SetCurrentGuard {
        let old_handle = self.currentSchedulerHandleEnumCell.schedulerHandleEnum.borrow_mut().replace(schedulerHandleEnum.clone());
        let depth = self.currentSchedulerHandleEnumCell.depth.get();

        assert_ne!(depth, usize::MAX, "reached max `enter` depth");

        let depth = depth + 1;
        self.currentSchedulerHandleEnumCell.depth.set(depth);

        SetCurrentGuard {
            prevSchedulerHandleEnumCell: old_handle,
            depth,
            _p: PhantomData,
        }
    }
}
