use crate::runtime::RuntimeHandle;

/// Handle to the runtime's metrics.
///
/// This handle is internally reference-counted and can be freely cloned. A
/// `RuntimeMetrics` handle is obtained using the [`Runtime::metrics`] method.
///
/// [`Runtime::metrics`]: crate::runtime::Runtime::metrics()
#[derive(Clone, Debug)]
pub struct RuntimeMetrics {
    handle: RuntimeHandle,
}

impl RuntimeMetrics {
    pub(crate) fn new(handle: RuntimeHandle) -> RuntimeMetrics {
        RuntimeMetrics { handle }
    }

    /// Returns the number of worker threads used by the runtime.
    ///
    /// The number of workers is set by configuring `worker_threads` on
    /// `runtime::Builder`. When using the `current_thread` runtime, the return
    /// value is always `1`.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokio::runtime::RuntimeHandle;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let metrics = RuntimeHandle::current().metrics();
    ///
    ///     let n = metrics.num_workers();
    ///     println!("Runtime is using {} workers", n);
    /// }
    /// ```
    pub fn num_workers(&self) -> usize {
        self.handle.schedulerHandleEnum.num_workers()
    }

    /// Returns the current number of alive tasks in the runtime.
    ///
    /// This counter increases when a task is spawned and decreases when a
    /// task exits.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokio::runtime::RuntimeHandle;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///    let metrics = RuntimeHandle::current().metrics();
    ///
    ///     let n = metrics.num_alive_tasks();
    ///     println!("Runtime has {} alive tasks", n);
    /// }
    /// ```
    pub fn num_alive_tasks(&self) -> usize {
        self.handle.schedulerHandleEnum.num_alive_tasks()
    }
}
