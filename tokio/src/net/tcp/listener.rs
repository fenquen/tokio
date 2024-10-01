use crate::io::{Interest, PollEvented};
use crate::net::tcp::TcpStream;

cfg_not_wasi! {
    use crate::net::{to_socket_addrs, ToSocketAddrs};
}

use std::fmt;
use std::io;
use std::net::{self, SocketAddr};
use std::task::{ready, Context, Poll};


#[cfg(feature = "net")]
/// let listener = TcpListener::bind("127.0.0.1:8080").await?;
pub struct TcpListener {
    pollEvented: PollEvented<mio::net::TcpListener>,
}


impl TcpListener {
    #[cfg(not(target_os = "wasi"))]
    pub async fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<TcpListener> {
        let addrs = to_socket_addrs(addr).await?;

        let mut last_err = None;

        for addr in addrs {
            match TcpListener::bind_addr(addr) {
                Ok(listener) => return Ok(listener),
                Err(e) => last_err = Some(e),
            }
        }

        Err(last_err.unwrap_or_else(|| { io::Error::new(io::ErrorKind::InvalidInput, "could not resolve to any address") }))
    }

    #[cfg(not(target_os = "wasi"))]
    fn bind_addr(addr: SocketAddr) -> io::Result<TcpListener> {
        TcpListener::fromMio(mio::net::TcpListener::bind(addr)?)
    }

    pub async fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let (mioTcpStream, socketAddr) = self.pollEvented.registration.async_io(Interest::READABLE, || self.pollEvented.accept()).await?;
        let tcpStream = TcpStream::fromMio(mioTcpStream)?;
        Ok((tcpStream, socketAddr))
    }

    /// Polls to accept a new incoming connection to this listener.
    ///
    /// If there is no connection to accept, `Poll::Pending` is returned and the
    /// current task will be notified by a waker.  Note that on multiple calls
    /// to `poll_accept`, only the `Waker` from the `Context` passed to the most
    /// recent call is scheduled to receive a wakeup.
    pub fn poll_accept(&self, cx: &mut Context<'_>) -> Poll<io::Result<(TcpStream, SocketAddr)>> {
        loop {
            let ev = ready!(self.pollEvented.registration().poll_read_ready(cx))?;

            match self.pollEvented.accept() {
                Ok((io, addr)) => {
                    let io = TcpStream::fromMio(io)?;
                    return Poll::Ready(Ok((io, addr)));
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    self.pollEvented.registration().clear_readiness(ev);
                }
                Err(e) => return Poll::Ready(Err(e)),
            }
        }
    }

    #[track_caller]
    pub fn from_std(listener: net::TcpListener) -> io::Result<TcpListener> {
        let pollEvented = PollEvented::new(mio::net::TcpListener::from_std(listener))?;
        Ok(TcpListener { pollEvented })
    }

    pub fn into_std(self) -> io::Result<net::TcpListener> {
        #[cfg(unix)]
        {
            use std::os::unix::io::{FromRawFd, IntoRawFd};
            self.pollEvented.into_inner().map(IntoRawFd::into_raw_fd).map(|raw_fd| unsafe { std::net::TcpListener::from_raw_fd(raw_fd) })
        }
    }

    #[cfg(not(target_os = "wasi"))]
    pub(crate) fn fromMio(mioTcpListener: mio::net::TcpListener) -> io::Result<TcpListener> {
        Ok(TcpListener { pollEvented: PollEvented::new(mioTcpListener)? })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.pollEvented.local_addr()
    }

    pub fn ttl(&self) -> io::Result<u32> {
        self.pollEvented.ttl()
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.pollEvented.set_ttl(ttl)
    }
}

impl TryFrom<net::TcpListener> for TcpListener {
    type Error = io::Error;

    /// Consumes stream, returning the tokio I/O object.
    ///
    /// This is equivalent to
    /// [`TcpListener::from_std(stream)`](TcpListener::from_std).
    fn try_from(stream: net::TcpListener) -> Result<Self, Self::Error> {
        Self::from_std(stream)
    }
}

impl fmt::Debug for TcpListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.pollEvented.fmt(f)
    }
}

#[cfg(unix)]
mod sys {
    use super::TcpListener;
    use std::os::unix::prelude::*;

    impl AsRawFd for TcpListener {
        fn as_raw_fd(&self) -> RawFd {
            self.pollEvented.as_raw_fd()
        }
    }

    impl AsFd for TcpListener {
        fn as_fd(&self) -> BorrowedFd<'_> {
            unsafe { BorrowedFd::borrow_raw(self.as_raw_fd()) }
        }
    }
}