use crate::net::{TcpListener, TcpStream};

use std::fmt;
use std::io;
use std::net::SocketAddr;

#[cfg(unix)]
use std::os::unix::io::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, RawFd};
use std::time::Duration;

cfg_net! {
    /// A TCP socket that has not yet been converted to a `TcpStream` or
    /// `TcpListener`.
    ///
    /// `TcpSocket` wraps an operating system socket and enables the caller to
    /// configure the socket before establishing a TCP connection or accepting
    /// inbound connections. The caller is able to set socket option and explicitly
    /// bind the socket with a socket address.
    ///
    /// The underlying socket is closed when the `TcpSocket` value is dropped.
    ///
    /// `TcpSocket` should only be used directly if the default configuration used
    /// by `TcpStream::connect` and `TcpListener::bind` does not meet the required
    /// use case.
    ///
    /// Calling `TcpStream::connect("127.0.0.1:8080")` is equivalent to:
    ///
    /// ```no_run
    /// use tokio::net::TcpSocket;
    ///
    /// use std::io;
    ///
    /// #[tokio::main]
    /// async fn main() -> io::Result<()> {
    ///     let addr = "127.0.0.1:8080".parse().unwrap();
    ///
    ///     let socket = TcpSocket::new_v4()?;
    ///     let stream = socket.connect(addr).await?;
    /// # drop(stream);
    ///
    ///     Ok(())
    /// }
    /// ```
    ///
    /// Calling `TcpListener::bind("127.0.0.1:8080")` is equivalent to:
    ///
    /// ```no_run
    /// use tokio::net::TcpSocket;
    ///
    /// use std::io;
    ///
    /// #[tokio::main]
    /// async fn main() -> io::Result<()> {
    ///     let addr = "127.0.0.1:8080".parse().unwrap();
    ///
    ///     let socket = TcpSocket::new_v4()?;
    ///     // On platforms with Berkeley-derived sockets, this allows to quickly
    ///     // rebind a socket, without needing to wait for the OS to clean up the
    ///     // previous one.
    ///     //
    ///     // On Windows, this allows rebinding sockets which are actively in use,
    ///     // which allows “socket hijacking”, so we explicitly don't set it here.
    ///     // https://docs.microsoft.com/en-us/windows/win32/winsock/using-so-reuseaddr-and-so-exclusiveaddruse
    ///     socket.set_reuseaddr(true)?;
    ///     socket.bind(addr)?;
    ///
    ///     let listener = socket.listen(1024)?;
    /// # drop(listener);
    ///
    ///     Ok(())
    /// }
    /// ```
    #[cfg_attr(docsrs, doc(alias = "connect_std"))]
    pub struct TcpSocket {
        inner: socket2::Socket,
    }
}

impl TcpSocket {
    pub fn new_v4() -> io::Result<TcpSocket> {
        TcpSocket::new(socket2::Domain::IPV4)
    }

    pub fn new_v6() -> io::Result<TcpSocket> {
        TcpSocket::new(socket2::Domain::IPV6)
    }

    fn new(domain: socket2::Domain) -> io::Result<TcpSocket> {
        let ty = socket2::Type::STREAM;

        #[cfg(any(
            target_os = "android",
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "fuchsia",
            target_os = "illumos",
            target_os = "linux",
            target_os = "netbsd",
            target_os = "openbsd"
        ))]
        let ty = ty.nonblocking();

        let inner = socket2::Socket::new(domain, ty, Some(socket2::Protocol::TCP))?;

        #[cfg(not(any(
            target_os = "android",
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "fuchsia",
            target_os = "illumos",
            target_os = "linux",
            target_os = "netbsd",
            target_os = "openbsd"
        )))]
        inner.set_nonblocking(true)?;

        Ok(TcpSocket { inner })
    }

    pub fn set_keepalive(&self, keepalive: bool) -> io::Result<()> {
        self.inner.set_keepalive(keepalive)
    }

    pub fn keepalive(&self) -> io::Result<bool> {
        self.inner.keepalive()
    }

    /// Allows the socket to bind to an in-use address.
    pub fn set_reuseaddr(&self, reuseaddr: bool) -> io::Result<()> {
        self.inner.set_reuse_address(reuseaddr)
    }

    pub fn reuseaddr(&self) -> io::Result<bool> {
        self.inner.reuse_address()
    }

    /// Allows the socket to bind to an in-use port. Only available for unix systems (excluding Solaris & Illumos).
    #[cfg(all(unix, not(target_os = "solaris"), not(target_os = "illumos")))]
    pub fn set_reuseport(&self, reuseport: bool) -> io::Result<()> {
        self.inner.set_reuse_port(reuseport)
    }

    pub fn reuseport(&self) -> io::Result<bool> {
        self.inner.reuse_port()
    }

    /// On most operating systems, this sets the `SO_SNDBUF` socket option.
    pub fn set_send_buffer_size(&self, size: u32) -> io::Result<()> {
        self.inner.set_send_buffer_size(size as usize)
    }

    pub fn send_buffer_size(&self) -> io::Result<u32> {
        self.inner.send_buffer_size().map(|n| n as u32)
    }

    /// On most operating systems, this sets the `SO_RCVBUF` socket option.
    pub fn set_recv_buffer_size(&self, size: u32) -> io::Result<()> {
        self.inner.set_recv_buffer_size(size as usize)
    }

    pub fn recv_buffer_size(&self) -> io::Result<u32> {
        self.inner.recv_buffer_size().map(|n| n as u32)
    }

    /// Sets the linger duration of this socket by setting the `SO_LINGER` option.
    ///
    /// This option controls the action taken when a stream has unsent messages and the stream is
    /// closed. If `SO_LINGER` is set, the system shall block the process until it can transmit the
    /// data or until the time expires.
    ///
    /// If `SO_LINGER` is not specified, and the socket is closed, the system handles the call in a
    /// way that allows the process to continue as quickly as possible.
    pub fn set_linger(&self, dur: Option<Duration>) -> io::Result<()> {
        self.inner.set_linger(dur)
    }

    pub fn linger(&self) -> io::Result<Option<Duration>> {
        self.inner.linger()
    }

    /// Sets the value of the `TCP_NODELAY` option on this socket.
    ///
    /// If set, this option disables the Nagle algorithm. This means that segments are always
    /// sent as soon as possible, even if there is only a small amount of data. When not set,
    /// data is buffered until there is a sufficient amount to send out, thereby avoiding
    /// the frequent sending of small packet
    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        self.inner.set_nodelay(nodelay)
    }

    pub fn nodelay(&self) -> io::Result<bool> {
        self.inner.nodelay()
    }


    /// Sets the value for the `IP_TOS` option on this socket.
    ///
    /// This value sets the type-of-service field that is used in every packet
    /// sent from this socket.
    ///
    /// **NOTE:** On Windows, `IP_TOS` is only supported on [Windows 8+ or
    /// Windows Server 2012+.](https://docs.microsoft.com/en-us/windows/win32/winsock/ipproto-ip-socket-options)
    // https://docs.rs/socket2/0.5.3/src/socket2/socket.rs.html#1446
    #[cfg(not(any(
        target_os = "fuchsia",
        target_os = "redox",
        target_os = "solaris",
        target_os = "illumos",
    )))]
    #[cfg_attr(
        docsrs,
        doc(cfg(not(any(
            target_os = "fuchsia",
            target_os = "redox",
            target_os = "solaris",
            target_os = "illumos",
        ))))
    )]
    pub fn set_tos(&self, tos: u32) -> io::Result<()> {
        self.inner.set_tos(tos)
    }

    #[cfg(not(any(
        target_os = "fuchsia",
        target_os = "redox",
        target_os = "solaris",
        target_os = "illumos",
    )))]
    #[cfg_attr(
        docsrs,
        doc(cfg(not(any(
            target_os = "fuchsia",
            target_os = "redox",
            target_os = "solaris",
            target_os = "illumos",
        ))))
    )]
    pub fn tos(&self) -> io::Result<u32> {
        self.inner.tos()
    }

    /// Gets the value for the `SO_BINDTODEVICE` option on this socket
    ///
    /// This value gets the socket binded device's interface name.
    #[cfg(any(target_os = "android", target_os = "fuchsia", target_os = "linux",))]
    #[cfg_attr(
        docsrs,
        doc(cfg(any(target_os = "android", target_os = "fuchsia", target_os = "linux",)))
    )]
    pub fn device(&self) -> io::Result<Option<Vec<u8>>> {
        self.inner.device()
    }

    /// Sets the value for the `SO_BINDTODEVICE` option on this socket
    ///
    /// If a socket is bound to an interface, only packets received from that
    /// particular interface are processed by the socket. Note that this only
    /// works for some socket types, particularly `AF_INET` sockets.
    ///
    /// If `interface` is `None` or an empty string it removes the binding.
    #[cfg(any(target_os = "android", target_os = "fuchsia", target_os = "linux"))]
    pub fn bind_device(&self, interface: Option<&[u8]>) -> io::Result<()> {
        self.inner.bind_device(interface)
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr().and_then(convert_address)
    }

    /// Returns the value of the `SO_ERROR` option.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.inner.take_error()
    }

    pub fn bind(&self, addr: SocketAddr) -> io::Result<()> {
        self.inner.bind(&addr.into())
    }

    ///  a TCP connection with a peer at the specified socket address.
    pub async fn connect(self, addr: SocketAddr) -> io::Result<TcpStream> {
        if let Err(err) = self.inner.connect(&addr.into()) {
            #[cfg(unix)]
            if err.raw_os_error() != Some(libc::EINPROGRESS) {
                return Err(err);
            }
        }
        #[cfg(unix)]
        let mio = {
            use std::os::unix::io::{FromRawFd, IntoRawFd};

            let raw_fd = self.inner.into_raw_fd();
            unsafe { mio::net::TcpStream::from_raw_fd(raw_fd) }
        };

        TcpStream::connect_mio(mio).await
    }

    /// converts the socket into a `TcpListener`.
    pub fn listen(self, backlog: u32) -> io::Result<TcpListener> {
        self.inner.listen(backlog as i32)?;
        #[cfg(unix)]
        let mio = {
            use std::os::unix::io::{FromRawFd, IntoRawFd};

            let raw_fd = self.inner.into_raw_fd();
            unsafe { mio::net::TcpListener::from_raw_fd(raw_fd) }
        };

        TcpListener::fromMio(mio)
    }

    pub fn from_std_stream(std_stream: std::net::TcpStream) -> TcpSocket {
        #[cfg(unix)]
        {
            use std::os::unix::io::{FromRawFd, IntoRawFd};

            let raw_fd = std_stream.into_raw_fd();
            unsafe { TcpSocket::from_raw_fd(raw_fd) }
        }
    }
}

fn convert_address(address: socket2::SockAddr) -> io::Result<SocketAddr> {
    match address.as_socket() {
        Some(address) => Ok(address),
        None => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid address family (not IPv4 or IPv6)",
        )),
    }
}

impl fmt::Debug for TcpSocket {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(fmt)
    }
}

cfg_unix! {
    impl AsRawFd for TcpSocket {
        fn as_raw_fd(&self) -> RawFd {
            self.inner.as_raw_fd()
        }
    }

    impl AsFd for TcpSocket {
        fn as_fd(&self) -> BorrowedFd<'_> {
            unsafe { BorrowedFd::borrow_raw(self.as_raw_fd()) }
        }
    }

    impl FromRawFd for TcpSocket {
        /// Converts a `RawFd` to a `TcpSocket`.
        ///
        /// The caller is responsible for ensuring that the socket is in non-blocking mode.
        unsafe fn from_raw_fd(fd: RawFd) -> TcpSocket {
            let inner = socket2::Socket::from_raw_fd(fd);
            TcpSocket { inner }
        }
    }

    impl IntoRawFd for TcpSocket {
        fn into_raw_fd(self) -> RawFd {
            self.inner.into_raw_fd()
        }
    }
}