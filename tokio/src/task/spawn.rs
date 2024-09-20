use crate::runtime::BOX_FUTURE_THRESHOLD;
use crate::task::JoinHandle;

use std::future::Future;

#[cfg(feature = "rt")]
#[track_caller]
pub fn spawn<F: Future<Output: Send + 'static> + Send + 'static>(future: F) -> JoinHandle<F::Output> {
    // preventing stack overflows on debug mode, by quickly sending the task to the heap.
    if cfg!(debug_assertions) && size_of::<F>() > BOX_FUTURE_THRESHOLD {
        spawn_inner(Box::pin(future), None)
    } else {
        spawn_inner(future, None)
    }
}

#[cfg(feature = "rt")]
#[track_caller]
pub(super) fn spawn_inner<T: Future<Output: Send + 'static> + Send + 'static>(future: T, name: Option<&str>) -> JoinHandle<T::Output> {
    use crate::runtime::{context, task};

    match context::withCurrentSchedulerHandleEnum(|schedulerHandleEnum| schedulerHandleEnum.spawn(future, task::Id::next())) {
        Ok(join_handle) => join_handle,
        Err(e) => panic!("{}", e),
    }
}

