#![cfg_attr(not(feature = "net"), allow(dead_code))]

use crate::io::interest::Interest;
use crate::runtime::io::{Direction, IODriverHandle, ReadyEvent, ScheduledIO};
use crate::runtime::{coop, scheduler};

use mio::event::Source;
use std::io;
use std::sync::Arc;
use std::task::{ready, Context, Poll};
use crate::runtime::scheduler::SchedulerHandleEnum;

#[cfg(any(feature = "net", all(unix, feature = "process"), all(unix, feature = "signal"), ))]
#[derive(Debug)]
pub(crate) struct Registration {
    schedulerHandleEnum: SchedulerHandleEnum,

    /// Reference to state stored by the driver.
    scheduledIo: Arc<ScheduledIO>,
}

unsafe impl Send for Registration {}
unsafe impl Sync for Registration {}

impl Registration {
    /// Registers the I/O resource with the reactor for the provided handle, for
    /// a specific `Interest`. This does not add `hup` or `error` so if you are
    /// interested in those states, you will need to add them to the readiness
    /// state passed to this function.
    ///
    /// # Return
    ///
    /// - `Ok` if the registration happened successfully
    /// - `Err` if an error was encountered during registration
    #[track_caller]
    pub(crate) fn register(mioSource: &mut impl Source,
                           interest: Interest,
                           schedulerHandleEnum: SchedulerHandleEnum) -> io::Result<Registration> {
        let scheduledIo = schedulerHandleEnum.driver().io().register(mioSource, interest)?;
        Ok(Registration { schedulerHandleEnum, scheduledIo })
    }

    /// Deregisters the I/O resource from the reactor it is associated with.
    ///
    /// This function must be called before the I/O resource associated with the
    /// registration is dropped.
    ///
    /// Note that deregistering does not guarantee that the I/O resource can be
    /// registered with a different reactor. Some I/O resource types can only be
    /// associated with a single reactor instance for their lifetime.
    ///
    /// # Return
    ///
    /// If the deregistration was successful, `Ok` is returned. Any calls to
    /// `Reactor::turn` that happen after a successful call to `deregister` will
    /// no longer result in notifications getting sent for this registration.
    ///
    /// `Err` is returned if an error is encountered.
    pub(crate) fn deregister(&mut self, io: &mut impl Source) -> io::Result<()> {
        self.handle().deregister(&self.scheduledIo, io)
    }

    pub(crate) fn clear_readiness(&self, event: ReadyEvent) {
        self.scheduledIo.clear_readiness(event);
    }

    // Uses the poll path, requiring the caller to ensure mutual exclusion for
    // correctness. Only the last task to call this function is notified.
    pub(crate) fn poll_read_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<ReadyEvent>> {
        self.poll_ready(cx, Direction::Read)
    }

    // Uses the poll path, requiring the caller to ensure mutual exclusion for
    // correctness. Only the last task to call this function is notified.
    pub(crate) fn poll_write_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<ReadyEvent>> {
        self.poll_ready(cx, Direction::Write)
    }

    // Uses the poll path, requiring the caller to ensure mutual exclusion for
    // correctness. Only the last task to call this function is notified.
    #[cfg(not(target_os = "wasi"))]
    pub(crate) fn poll_read_io<R>(
        &self,
        cx: &mut Context<'_>,
        f: impl FnMut() -> io::Result<R>,
    ) -> Poll<io::Result<R>> {
        self.poll_io(cx, Direction::Read, f)
    }

    // Uses the poll path, requiring the caller to ensure mutual exclusion for
    // correctness. Only the last task to call this function is notified.
    pub(crate) fn poll_write_io<R>(
        &self,
        cx: &mut Context<'_>,
        f: impl FnMut() -> io::Result<R>,
    ) -> Poll<io::Result<R>> {
        self.poll_io(cx, Direction::Write, f)
    }

    /// Polls for events on the I/O resource's `direction` readiness stream.
    ///
    /// If called with a task context, notify the task when a new event is
    /// received.
    fn poll_ready(
        &self,
        cx: &mut Context<'_>,
        direction: Direction,
    ) -> Poll<io::Result<ReadyEvent>> {
        ready!(crate::trace::trace_leaf(cx));
        // Keep track of task budget
        let coop = ready!(crate::runtime::coop::poll_proceed(cx));
        let ev = ready!(self.scheduledIo.poll_readiness(cx, direction));

        if ev.isShutdown {
            return Poll::Ready(Err(gone()));
        }

        coop.made_progress();
        Poll::Ready(Ok(ev))
    }

    fn poll_io<R>(
        &self,
        cx: &mut Context<'_>,
        direction: Direction,
        mut f: impl FnMut() -> io::Result<R>,
    ) -> Poll<io::Result<R>> {
        loop {
            let ev = ready!(self.poll_ready(cx, direction))?;

            match f() {
                Ok(ret) => {
                    return Poll::Ready(Ok(ret));
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    self.clear_readiness(ev);
                }
                Err(e) => return Poll::Ready(Err(e)),
            }
        }
    }

    pub(crate) fn try_io<R>(
        &self,
        interest: Interest,
        f: impl FnOnce() -> io::Result<R>,
    ) -> io::Result<R> {
        let ev = self.scheduledIo.ready_event(interest);

        // Don't attempt the operation if the resource is not ready.
        if ev.ready.is_empty() {
            return Err(io::ErrorKind::WouldBlock.into());
        }

        match f() {
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.clear_readiness(ev);
                Err(io::ErrorKind::WouldBlock.into())
            }
            res => res,
        }
    }

    pub(crate) async fn pollReadinessAsync(&self, interest: Interest) -> io::Result<ReadyEvent> {
        let readyEvent = self.scheduledIo.pollReadinessAsync(interest).await;

        if readyEvent.isShutdown {
            return Err(gone());
        }

        Ok(readyEvent)
    }

    pub(crate) async fn async_io<R>(&self, interest: Interest, mut f: impl FnMut() -> io::Result<R>) -> io::Result<R> {
        loop {
            let readyEvent = self.pollReadinessAsync(interest).await?;

            let coop = crate::future::poll_fn(coop::poll_proceed).await;

            match f() {
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    self.clear_readiness(readyEvent);
                }
                x => {
                    coop.made_progress();
                    return x;
                }
            }
        }
    }

    fn handle(&self) -> &IODriverHandle {
        self.schedulerHandleEnum.driver().io()
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        // It is possible for a cycle to be created between wakers stored in
        // `ScheduledIo` instances and `Arc<driver::Inner>`. To break this
        // cycle, wakers are cleared. This is an imperfect solution as it is
        // possible to store a `Registration` in a waker. In this case, the
        // cycle would remain.
        //
        // See tokio-rs/tokio#3481 for more details.
        self.scheduledIo.clear_wakers();
    }
}

fn gone() -> io::Error {
    io::Error::new(io::ErrorKind::Other, crate::util::error::RUNTIME_SHUTTING_DOWN_ERROR)
}
