use crate::future::Future;
use crate::runtime::task::core::{Cell, Core, Header, Trailer};
use crate::runtime::task::state::{Snapshot, State};
use crate::runtime::task::waker::buildWakerRef;
use crate::runtime::task::{Id, JoinError, NotifiedTask, RawTask, Schedule, Task};

use crate::runtime::TaskMeta;
use std::any::Any;
use std::mem;
use std::mem::ManuallyDrop;
use std::panic;
use std::ptr::NonNull;
use std::task::{Context, Poll, Waker};

/// Typed raw task handle.
pub(super) struct Harness<T: Future, S: 'static> {
    cell: NonNull<Cell<T, S>>,
}

impl<T: Future, S: 'static> Harness<T, S> {
    pub(super) unsafe fn from_raw(headerPtr: NonNull<Header>) -> Harness<T, S> {
        Harness {
            cell: headerPtr.cast::<Cell<T, S>>(),
        }
    }

    fn header_ptr(&self) -> NonNull<Header> {
        self.cell.cast()
    }

    fn header(&self) -> &Header {
        unsafe { &*self.header_ptr().as_ptr() }
    }

    fn state(&self) -> &State {
        &self.header().state
    }

    fn trailer(&self) -> &Trailer {
        unsafe { &self.cell.as_ref().trailer }
    }

    fn core(&self) -> &Core<T, S> {
        unsafe { &self.cell.as_ref().core }
    }
}

/// Task operations that can be implemented without being generic over the
/// scheduler or task. Only one version of these methods should exist in the final binary.
impl RawTask {
    pub(super) fn dropRef(self) {
        if self.state().ref_dec() {
            self.dealloc();
        }
    }

    /// This call consumes a ref-count and notifies the task. This will create a
    /// new Notified and submit it if necessary.
    ///
    /// The caller does not need to hold a ref-count besides the one that was
    /// passed to this call.
    pub(super) fn wakeByVal(&self) {
        use super::state::TransitionToNotifiedByVal;

        match self.state().transition_to_notified_by_val() {
            TransitionToNotifiedByVal::Submit => {
                // The caller has given us a ref-count, and the transition has
                // created a new ref-count, so we now hold two. We turn the new
                // ref-count Notified and pass it to the call to `schedule`.
                //
                // The old ref-count is retained for now to ensure that the task
                // is not dropped during the call to `schedule` if the call
                // drops the task it was given.
                self.schedule();

                // Now that we have completed the call to schedule, we can release our ref-count.
                self.dropRef();
            }
            TransitionToNotifiedByVal::Dealloc => self.dealloc(),
            TransitionToNotifiedByVal::DoNothing => {}
        }
    }

    /// This call notifies the task. It will not consume any ref-counts, but the
    /// caller should hold a ref-count.  This will create a new Notified and submit it if necessary.
    pub(super) fn wakeByRef(&self) {
        use super::state::TransitionToNotifiedByRef;

        match self.state().transition_to_notified_by_ref() {
            TransitionToNotifiedByRef::Submit => {
                // The transition above incremented the ref-count for a new task
                // and the caller also holds a ref-count. The caller's ref-count
                // ensures that the task is not destroyed even if the new task
                // is dropped before `schedule` returns.
                self.schedule();
            }
            TransitionToNotifiedByRef::DoNothing => {}
        }
    }

    /// Remotely aborts the task.
    ///
    /// The caller should hold a ref-count, but we do not consume it.
    ///
    /// This is similar to `shutdown` except that it asks the runtime to perform
    /// the shutdown. This is necessary to avoid the shutdown happening in the
    /// wrong thread for non-Send tasks.
    pub(super) fn remote_abort(&self) {
        if self.state().transition_to_notified_and_cancel() {
            // The transition has created a new ref-count, which we turn into
            // a Notified and pass to the task.
            //
            // Since the caller holds a ref-count, the task cannot be destroyed
            // before the call to `schedule` returns even if the call drops the
            // `Notified` internally.
            self.schedule();
        }
    }

    /// Try to set the waker notified when the task is complete. Returns true if
    /// the task has already completed. If this call returns false, then the
    /// waker will not be notified.
    pub(super) fn try_set_join_waker(&self, waker: &Waker) -> bool {
        can_read_output(self.header(), self.trailer(), waker)
    }
}

impl<T: Future, S: Schedule> Harness<T, S> {
    pub(super) fn drop_reference(self) {
        if self.state().ref_dec() {
            self.dealloc();
        }
    }

    /// Polls the inner future. A ref-count is consumed.
    ///
    /// All necessary state checks and transitions are performed.
    /// Panics raised while polling the future are handled.
    pub(super) fn poll(self) {
        // We pass our ref-count to `poll_inner`.
        match self.poll_inner() {
            PollFuture::Notified => { // 将已经wake的task变idle
                // The `poll_inner` call has given us two ref-counts back.
                // We give one of them to a new task and call `yield_now`.
                self.core().scheduler.yield_now(NotifiedTask(self.get_new_task()));

                // The remaining ref-count is now dropped. We kept the extra
                // ref-count until now to ensure that even if the `yield_now`
                // call drops the provided task, the task isn't deallocated before after `yield_now` returns.
                self.drop_reference();
            }
            PollFuture::Complete => self.complete(), // Poll::Ready 对应
            PollFuture::Dealloc => self.dealloc(),
            PollFuture::DoNothing => (),
        }
    }

    /// Polls the task and cancel it if necessary. This takes ownership of a ref-count.
    ///
    /// If the return value is Notified, the caller is given ownership of two ref-counts.
    ///
    /// If the return value is Complete, the caller is given ownership of a single ref-count, which should be passed on to `complete`.
    ///
    /// If the return value is `Dealloc`, then this call consumed the last ref-count and the caller should call `dealloc`.
    ///
    /// Otherwise the ref-count is consumed and the caller should not access `self` again.
    fn poll_inner(&self) -> PollFuture {
        use super::state::{TransitionToIdle, TransitionToRunning};

        // transition2Running 可能调用 unset_notified()
        match self.state().transition2Running() {
            TransitionToRunning::Success => {
                let header_ptr = self.header_ptr();
                let wakerRef = buildWakerRef::<S>(&header_ptr);
                let context = Context::from_waker(&wakerRef);

                // the future completed. Move on to complete the task.
                if poll_future(self.core(), context) == Poll::Ready(()) {
                    return PollFuture::Complete;
                }

                match self.state().transition2Idle() {
                    TransitionToIdle::Ok => PollFuture::DoNothing,
                    TransitionToIdle::OkNotified => PollFuture::Notified,// transition2Idle时候发现已是notified了
                    TransitionToIdle::OkDealloc => PollFuture::Dealloc,
                    TransitionToIdle::Cancelled => {
                        // the transition to idle failed because the task was cancelled during the poll
                        cancel_task(self.core());
                        PollFuture::Complete
                    }
                }
            }
            TransitionToRunning::Cancelled => {
                cancel_task(self.core());
                PollFuture::Complete
            }
            TransitionToRunning::Failed => PollFuture::DoNothing,
            TransitionToRunning::Dealloc => PollFuture::Dealloc,
        }
    }

    /// Forcibly shuts down the task.
    ///
    /// Attempt to transition to `Running` in order to forcibly shutdown the
    /// task. If the task is currently running or in a state of completion, then
    /// there is nothing further to do. When the task completes running, it will
    /// notice the `CANCELLED` bit and finalize the task.
    pub(super) fn shutdown(self) {
        if !self.state().transition_to_shutdown() {
            // The task is concurrently running. No further work needed.
            self.drop_reference();
            return;
        }

        // By transitioning the lifecycle to `Running`, we have permission to
        // drop the future.
        cancel_task(self.core());
        self.complete();
    }

    pub(super) fn dealloc(self) {
        // Observe that we expect to have mutable access to these objects
        // because we are going to drop them. This only matters when running under loom
        self.trailer().waker.with_mut(|_| ());
        self.core().coreStage.with_mut(|_| ());

        // Safety: The caller of this method just transitioned our ref-count to
        // zero, so it is our responsibility to release the allocation.
        //
        // We don't hold any references into the allocation at this point, but
        // it is possible for another thread to still hold a `&State` into the
        // allocation if that other thread has decremented its last ref-count,
        // but has not yet returned from the relevant method on `State`.
        //
        // However, the `State` type consists of just an `AtomicUsize`, and an
        // `AtomicUsize` wraps the entirety of its contents in an `UnsafeCell`.
        // As explained in the documentation for `UnsafeCell`, such references
        // are allowed to be dangling after their last use, even if the
        // reference has not yet gone out of scope.
        unsafe {
            drop(Box::from_raw(self.cell.as_ptr()));
        }
    }

    // ===== join handle =====

    /// Read the task output into `dst`.
    pub(super) fn try_read_output(self, dst: &mut Poll<super::Result<T::Output>>, waker: &Waker) {
        if can_read_output(self.header(), self.trailer(), waker) {
            *dst = Poll::Ready(self.core().take_output());
        }
    }

    pub(super) fn drop_join_handle_slow(self) {
        // Try to unset `JOIN_INTEREST`. This must be done as a first step in
        // case the task concurrently completed.
        if self.state().unset_join_interested().is_err() {
            // It is our responsibility to drop the output. This is critical as
            // the task output may not be `Send` and as such must remain with
            // the scheduler or `JoinHandle`. i.e. if the output remains in the
            // task structure until the task is deallocated, it may be dropped
            // by a Waker on any arbitrary thread.
            //
            // Panics are delivered to the user via the `JoinHandle`. Given that
            // they are dropping the `JoinHandle`, we assume they are not
            // interested in the panic and swallow it.
            let _ = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                self.core().drop_future_or_output();
            }));
        }

        // Drop the `JoinHandle` reference, possibly deallocating the task
        self.drop_reference();
    }

    // ====== internal ======

    /// Completes the task. This method assumes that the state is RUNNING.
    fn complete(self) {
        // The future has completed and its output has been written to the task
        // stage. We transition from running to complete.

        let snapshot = self.state().transition2Complete();

        // We catch panics here in case dropping the future or waking the
        // JoinHandle panics.
        let _ = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            if !snapshot.is_join_interested() {
                // The `JoinHandle` is not interested in the output of
                // this task. It is our responsibility to drop the
                // output.
                self.core().drop_future_or_output();
            } else if snapshot.is_join_waker_set() {
                // Notify the waker. Reading the waker field is safe per rule 4
                // in task/mod.rs, since the JOIN_WAKER bit is set and the call
                // to transition_to_complete() above set the COMPLETE bit.
                self.trailer().wake_join();
            }
        }));

        // We catch panics here in case invoking a hook panics.
        //
        // We call this in a separate block so that it runs after the task appears to have
        // completed and will still run if the destructor panics.
        if let Some(f) = self.trailer().hooks.task_terminate_callback.as_ref() {
            let _ = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                f(&TaskMeta {
                    _phantom: Default::default(),
                })
            }));
        }

        // The task has completed execution and will no longer be scheduled.
        let num_release = self.release();

        if self.state().transition_to_terminal(num_release) {
            self.dealloc();
        }
    }

    /// Releases the task from the scheduler. Returns the number of ref-counts
    /// that should be decremented.
    fn release(&self) -> usize {
        // We don't actually increment the ref-count here, but the new task is
        // never destroyed, so that's ok.
        let me = ManuallyDrop::new(self.get_new_task());

        if let Some(task) = self.core().scheduler.release(&me) {
            mem::forget(task);
            2
        } else {
            1
        }
    }

    /// Creates a new task that holds its own ref-count.
    ///
    /// # Safety
    ///
    /// Any use of `self` after this call must ensure that a ref-count to the
    /// task holds the task alive until after the use of `self`. Passing the
    /// returned Task to any method on `self` is unsound if dropping the Task
    /// could drop `self` before the call on `self` returned.
    fn get_new_task(&self) -> Task<S> {
        // safety: The header is at the beginning of the cell, so this cast is safe
        unsafe { Task::from_raw(self.cell.cast()) }
    }
}

fn can_read_output(header: &Header, trailer: &Trailer, waker: &Waker) -> bool {
    // Load a snapshot of the current task state
    let snapshot = header.state.load();

    debug_assert!(snapshot.is_join_interested());

    if !snapshot.is_complete() {
        // If the task is not complete, try storing the provided waker in the task's waker field.

        let res = if snapshot.is_join_waker_set() {
            // If JOIN_WAKER is set, then JoinHandle has previously stored a
            // waker in the waker field per step (iii) of rule 5 in task/mod.rs.

            // Optimization: if the stored waker and the provided waker wake the
            // same task, then return without touching the waker field. (Reading
            // the waker field below is safe per rule 3 in task/mod.rs.)
            if unsafe { trailer.will_wake(waker) } {
                return false;
            }

            // Otherwise swap the stored waker with the provided waker by
            // following the rule 5 in task/mod.rs.
            header.state.unset_waker().and_then(|snapshot| set_join_waker(header, trailer, waker.clone(), snapshot))
        } else {
            // If JOIN_WAKER is unset, then JoinHandle has mutable access to the
            // waker field per rule 2 in task/mod.rs; therefore, skip step (i)
            // of rule 5 and try to store the provided waker in the waker field.
            set_join_waker(header, trailer, waker.clone(), snapshot)
        };

        match res {
            Ok(_) => return false,
            Err(snapshot) => {
                assert!(snapshot.is_complete());
            }
        }
    }

    true
}

fn set_join_waker(header: &Header,
                  trailer: &Trailer,
                  waker: Waker,
                  snapshot: Snapshot) -> Result<Snapshot, Snapshot> {
    assert!(snapshot.is_join_interested());
    assert!(!snapshot.is_join_waker_set());

    // Safety: Only the `JoinHandle` may set the `waker` field. When
    // `JOIN_INTEREST` is **not** set, nothing else will touch the field.
    unsafe {
        trailer.set_waker(Some(waker));
    }

    // Update the `JoinWaker` state accordingly
    let res = header.state.set_join_waker();

    // If the state could not be updated, then clear the join waker
    if res.is_err() {
        unsafe {
            trailer.set_waker(None);
        }
    }

    res
}

enum PollFuture {
    Complete,
    /// 发生在transition2Idle时候发现已是notified了
    Notified,
    DoNothing,
    Dealloc,
}

/// Cancels the task and store the appropriate error in the stage field.
fn cancel_task<T: Future, S: Schedule>(core: &Core<T, S>) {
    // Drop the future from a panic guard.
    let res = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        core.drop_future_or_output();
    }));

    core.store_output(Err(panic_result_to_join_error(core.taskId, res)));
}

fn panic_result_to_join_error(task_id: Id, res: Result<(), Box<dyn Any + Send + 'static>>) -> JoinError {
    match res {
        Ok(()) => JoinError::cancelled(task_id),
        Err(panic) => JoinError::panic(task_id, panic),
    }
}

/// Polls the future. If the future completes, the output is written to the stage field.
fn poll_future<T: Future, S: Schedule>(core: &Core<T, S>, context: Context<'_>) -> Poll<()> {
    let output = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        struct Guard<'a, T: Future, S: Schedule> {
            core: &'a Core<T, S>,
        }

        impl<'a, T: Future, S: Schedule> Drop for Guard<'a, T, S> {
            fn drop(&mut self) {
                // If the future panics on poll, we drop it inside the panic guard.
                self.core.drop_future_or_output();
            }
        }

        let guard = Guard { core };

        let res = guard.core.poll(context);

        // 到了这边说明上边的guard.core.poll(cx)成功调用了 自然是用不到为了应对panic的drop了
        mem::forget(guard);

        res
    }));

    // Prepare output for being placed in the core stage.
    let output = match output {
        Ok(Poll::Pending) => return Poll::Pending,
        Ok(Poll::Ready(output)) => Ok(output),
        Err(panic) => Err(panic_to_error(&core.scheduler, core.taskId, panic)),
    };

    // Catch and ignore panics if the future panics on drop.
    let res = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        core.store_output(output);
    }));

    if res.is_err() {
        core.scheduler.unhandled_panic();
    }

    Poll::Ready(())
}

#[cold]
fn panic_to_error<S: Schedule>(scheduler: &S, task_id: Id, panic: Box<dyn Any + Send + 'static>) -> JoinError {
    scheduler.unhandled_panic();
    JoinError::panic(task_id, panic)
}
