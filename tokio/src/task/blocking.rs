use crate::task::JoinHandle;

#[cfg(feature = "rt-multi-thread")]
/// Runs the provided blocking function on the current thread without blocking the executor.
#[track_caller]
pub fn block_in_place<F: FnOnce() -> R, R>(f: F) -> R {
    crate::runtime::scheduler::block_in_place(f)
}


#[cfg(feature = "rt")]
/// Runs the provided closure on a thread where blocking is acceptable.
#[track_caller]
pub fn spawn_blocking<F: FnOnce() -> R + Send + 'static, R: Send + 'static>(f: F) -> JoinHandle<R> {
    crate::runtime::spawn_blocking(f)
}

