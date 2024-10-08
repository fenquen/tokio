use crate::io::interest::Interest;
use crate::runtime::io::Registration;

use mio::event::Source;
use std::fmt;
use std::io;
use std::ops::Deref;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::task::ready;
use crate::runtime::scheduler::SchedulerHandleEnum;

/// Associates an I/O resource that implements the [`std::io::Read`] and/or
/// [`std::io::Write`] traits with the reactor that drives it.
///
/// `PollEvented` uses [`Registration`] internally to take a type that
/// implements [`mio::event::Source`] as well as [`std::io::Read`] and/or
/// [`std::io::Write`] and associate it with a reactor that will drive it.
///
/// Once the [`mio::event::Source`] type is wrapped by `PollEvented`, it can be
/// used from within the future's execution model. As such, the
/// `PollEvented` type provides [`AsyncRead`] and [`AsyncWrite`]
/// implementations using the underlying I/O resource as well as readiness
/// events provided by the reactor.
///
/// **Note**: While `PollEvented` is `Sync` (if the underlying I/O type is
/// `Sync`), the caller must ensure that there are at most two tasks that
/// use a `PollEvented` instance concurrently. One for reading and one for
/// writing. While violating this requirement is "safe" from a Rust memory
/// model point of view, it will result in unexpected behavior in the form
/// of lost notifications and tasks hanging.
///
/// ## Readiness events
///
/// Besides just providing [`AsyncRead`] and [`AsyncWrite`] implementations,
/// this type also supports access to the underlying readiness event stream.
/// While similar in function to what [`Registration`] provides, the
/// semantics are a bit different.
///
/// Two functions are provided to access the readiness events:
/// [`poll_read_ready`] and [`poll_write_ready`]. These functions return the
/// current readiness state of the `PollEvented` instance. If
/// [`poll_read_ready`] indicates read readiness, immediately calling
/// [`poll_read_ready`] again will also indicate read readiness.
///
/// When the operation is attempted and is unable to succeed due to the I/O
/// resource not being ready, the caller must call [`clear_readiness`].
/// This clears the readiness state until a new readiness event is received.
///
/// This allows the caller to implement additional functions. For example,
/// [`TcpListener`] implements `poll_accept` by using [`poll_read_ready`] and
/// [`clear_readiness`].
///
/// ## Platform-specific events
///
/// `PollEvented` also allows receiving platform-specific `mio::Ready` events.
/// These events are included as part of the read readiness event stream. The
/// write readiness event stream is only for `Ready::writable()` events.
///
/// [`AsyncRead`]: crate::io::AsyncRead
/// [`AsyncWrite`]: crate::io::AsyncWrite
/// [`TcpListener`]: crate::net::TcpListener
/// [`clear_readiness`]: Registration::clear_readiness
/// [`poll_read_ready`]: Registration::pollReadReady
/// [`poll_write_ready`]: Registration::poll_write_ready
#[cfg(any(feature = "net", all(unix, feature = "process"), all(unix, feature = "signal"), ))]
pub(crate) struct PollEvented<E: Source> {
    mioSource: Option<E>,
    pub registration: Registration,
}

impl<E: Source> PollEvented<E> {
    #[track_caller]
    #[cfg_attr(feature = "signal", allow(unused))]
    pub(crate) fn new(mioSource: E) -> io::Result<PollEvented<E>> {
        PollEvented::new_with_interest_and_handle(mioSource,
                                                  Interest::READABLE | Interest::WRITABLE,
                                                  SchedulerHandleEnum::current())
    }

    #[track_caller]
    #[cfg_attr(feature = "signal", allow(unused))]
    pub(crate) fn new_with_interest(mioSource: E, interest: Interest) -> io::Result<PollEvented<E>> {
        Self::new_with_interest_and_handle(mioSource, interest, SchedulerHandleEnum::current())
    }

    #[track_caller]
    pub(crate) fn new_with_interest_and_handle(mut mioSource: E,
                                               interest: Interest,
                                               schedulerHandleEnum: SchedulerHandleEnum) -> io::Result<Self> {
        let registration = Registration::register(&mut mioSource, interest, schedulerHandleEnum)?;
        Ok(PollEvented {
            mioSource: Some(mioSource),
            registration,
        })
    }

    /// Returns a reference to the registration.
    #[cfg(feature = "net")]
    pub(crate) fn registration(&self) -> &Registration {
        &self.registration
    }

    /// Deregisters the inner io from the registration and returns a Result containing the inner io.
    #[cfg(any(feature = "net", feature = "process"))]
    pub(crate) fn into_inner(mut self) -> io::Result<E> {
        let mut inner = self.mioSource.take().unwrap(); // As io shouldn't ever be None, just unwrap here.
        self.registration.deregister(&mut inner)?;
        Ok(inner)
    }

    #[cfg(all(feature = "process", target_os = "linux"))]
    pub(crate) fn poll_read_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.registration.pollReadReady(cx).map_err(io::Error::from).map_ok(|_| ())
    }

    /// Re-register under new runtime with `interest`.
    #[cfg(all(feature = "process", target_os = "linux"))]
    pub(crate) fn reregister(&mut self, interest: Interest) -> io::Result<()> {
        let io = self.mioSource.as_mut().unwrap(); // As io shouldn't ever be None, just unwrap here.
        let _ = self.registration.deregister(io);
        self.registration = Registration::register(io, interest, SchedulerHandleEnum::current())?;

        Ok(())
    }
}

#[cfg(any(feature = "net", all(unix, feature = "process")))]
use crate::io::ReadBuf;
#[cfg(any(feature = "net", all(unix, feature = "process")))]
use std::task::{Context, Poll};

#[cfg(any(feature = "net", all(unix, feature = "process")))]
impl<E: Source> PollEvented<E> {
    // Safety: The caller must ensure that `E` can read into uninitialized memory
    pub(crate) unsafe fn pollRead<'a>(&'a self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>>
    where
        &'a E: std::io::Read + 'a,
    {
        use std::io::Read;

        loop {
            let readyEvent = ready!(self.registration.pollReadReady(cx))?;

            let b = &mut *(buf.unfilled_mut() as *mut [std::mem::MaybeUninit<u8>] as *mut [u8]);

            // used only when the cfgs below apply
            #[allow(unused_variables)]
            let len = b.len();

            match self.mioSource.as_ref().unwrap().read(b) {
                Ok(n) => {
                    // When mio is using the epoll or kqueue selector, reading a partially full
                    // buffer is sufficient to show that the socket buffer has been drained.
                    //
                    // This optimization does not work for level-triggered selectors such as windows or when poll is used.
                    //
                    // Read more: https://github.com/tokio-rs/tokio/issues/5866
                    #[cfg(all(
                        not(mio_unsupported_force_poll_poll),
                        any(
                            // epoll
                            target_os = "android",
                            target_os = "illumos",
                            target_os = "linux",
                            target_os = "redox",
                            // kqueue
                            target_os = "dragonfly",
                            target_os = "freebsd",
                            target_os = "ios",
                            target_os = "macos",
                            target_os = "netbsd",
                            target_os = "openbsd",
                            target_os = "tvos",
                            target_os = "visionos",
                            target_os = "watchos",
                        )
                    ))]
                    if 0 < n && n < len {
                        self.registration.clear_readiness(readyEvent);
                    }

                    // Safety: We trust `TcpStream::read` to have filled up `n` bytes in the buffer.
                    buf.assume_init(n);
                    buf.advance(n);
                    return Poll::Ready(Ok(()));
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => self.registration.clear_readiness(readyEvent),
                Err(e) => return Poll::Ready(Err(e)),
            }
        }
    }

    pub(crate) fn poll_write<'a>(&'a self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>>
    where
        &'a E: io::Write + 'a,
    {
        use std::io::Write;

        loop {
            let evt = ready!(self.registration.poll_write_ready(cx))?;

            match self.mioSource.as_ref().unwrap().write(buf) {
                Ok(n) => {
                    // if we write only part of our buffer, this is sufficient on unix to show
                    // that the socket buffer is full.  Unfortunately this assumption
                    // fails for level-triggered selectors (like on Windows or poll even for
                    // UNIX): https://github.com/tokio-rs/tokio/issues/5866
                    if n > 0 && (!cfg!(windows) && !cfg!(mio_unsupported_force_poll_poll) && n < buf.len()) {
                        self.registration.clear_readiness(evt);
                    }

                    return Poll::Ready(Ok(n));
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    self.registration.clear_readiness(evt);
                }
                Err(e) => return Poll::Ready(Err(e)),
            }
        }
    }

    #[cfg(any(feature = "net", feature = "process"))]
    pub(crate) fn poll_write_vectored<'a>(&'a self, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>>
    where
        &'a E: io::Write + 'a,
    {
        use std::io::Write;
        self.registration.poll_write_io(cx, || self.mioSource.as_ref().unwrap().write_vectored(bufs))
    }
}


impl<E: Source> UnwindSafe for PollEvented<E> {}

impl<E: Source> RefUnwindSafe for PollEvented<E> {}

impl<E: Source> Deref for PollEvented<E> {
    type Target = E;

    fn deref(&self) -> &E {
        self.mioSource.as_ref().unwrap()
    }
}

impl<E: Source + fmt::Debug> fmt::Debug for PollEvented<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PollEvented").field("io", &self.mioSource).finish()
    }
}

impl<E: Source> Drop for PollEvented<E> {
    fn drop(&mut self) {
        if let Some(mut io) = self.mioSource.take() {
            // Ignore errors
            let _ = self.registration.deregister(&mut io);
        }
    }
}
