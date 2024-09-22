use crate::runtime::time::TimerEntry;
use crate::time::{error::Error, Duration, Instant};
use crate::util::trace;

use pin_project_lite::pin_project;
use std::future::Future;
use std::panic::Location;
use std::pin::Pin;
use std::task::{self, ready, Poll};

#[track_caller]
pub fn sleep_until(deadline: Instant) -> Sleep {
    Sleep::new_timeout(deadline, trace::caller_location())
}

#[track_caller]
pub fn sleep(duration: Duration) -> Sleep {
    let location = trace::caller_location();

    match Instant::now().checked_add(duration) {
        Some(deadline) => Sleep::new_timeout(deadline, location),
        None => Sleep::new_timeout(Instant::far_future(), location),
    }
}

pin_project! {
    #[derive(Debug)]
    pub struct Sleep {
        inner: Inner,

        // The link between the `Sleep` instance and the timer that drives it.
        #[pin]
        entry: TimerEntry,
    }
}

cfg_not_trace! {
    #[derive(Debug)]
    struct Inner {
    }
}

impl Sleep {
    #[cfg_attr(not(all(tokio_unstable, feature = "tracing")), allow(unused_variables))]
    #[track_caller]
    pub(crate) fn new_timeout(deadline: Instant,
                              location: Option<&'static Location<'static>>) -> Sleep {
        use crate::runtime::scheduler;
        let handle = scheduler::SchedulerHandleEnum::current();
        let entry = TimerEntry::new(handle, deadline);

        #[cfg(not(all(tokio_unstable, feature = "tracing")))]
        let inner = Inner {};

        Sleep { inner, entry }
    }

    pub(crate) fn far_future(location: Option<&'static Location<'static>>) -> Sleep {
        Self::new_timeout(Instant::far_future(), location)
    }

    /// Returns the instant at which the future will complete.
    pub fn deadline(&self) -> Instant {
        self.entry.deadline()
    }

    /// Returns `true` if `Sleep` has elapsed.
    ///
    /// A `Sleep` instance is elapsed when the requested duration has elapsed.
    pub fn is_elapsed(&self) -> bool {
        self.entry.is_elapsed()
    }

    /// Resets the `Sleep` instance to a new deadline.
    pub fn reset(self: Pin<&mut Self>, deadline: Instant) {
        self.reset_inner(deadline);
    }

    /// Resets the `Sleep` instance to a new deadline without reregistering it
    /// to be woken up.
    ///
    /// Calling this function allows changing the instant at which the `Sleep`
    /// future completes without having to create new associated state and
    /// without having it registered. This is required in e.g. the
    /// [`crate::time::Interval`] where we want to reset the internal [Sleep]
    /// without having it wake up the last task that polled it.
    pub(crate) fn reset_without_reregister(self: Pin<&mut Self>, deadline: Instant) {
        let mut me = self.project();
        me.entry.as_mut().reset(deadline, false);
    }

    fn reset_inner(self: Pin<&mut Self>, deadline: Instant) {
        let mut me = self.project();
        me.entry.as_mut().reset(deadline, true);
    }

    fn poll_elapsed(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Result<(), Error>> {
        let me = self.project();

        ready!(crate::trace::trace_leaf(cx));

        #[cfg(any(not(tokio_unstable), not(feature = "tracing")))]
        let coop = ready!(crate::runtime::coop::poll_proceed(cx));

        let result = me.entry.poll_elapsed(cx).map(move |r| {
            coop.made_progress();
            r
        });

        #[cfg(any(not(tokio_unstable), not(feature = "tracing")))]
        return result;
    }
}

impl Future for Sleep {
    type Output = ();

    // `poll_elapsed` can return an error in two cases:
    //
    // - AtCapacity: this is a pathological case where far too many
    //   sleep instances have been scheduled.
    // - Shutdown: No timer has been setup, which is a mis-use error.
    //
    // Both cases are extremely rare, and pretty accurately fit into
    // "logic errors", so we just panic in this case. A user couldn't
    // really do much better if we passed the error onwards.
    fn poll(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        match ready!(self.as_mut().poll_elapsed(cx)) {
            Ok(()) => Poll::Ready(()),
            Err(e) => panic!("timer error: {}", e),
        }
    }
}
