cfg_io_blocking! {
    pub(crate) mod blocking;
}

mod async_buf_read;
pub use self::async_buf_read::AsyncBufRead;

mod async_read;
pub use self::async_read::AsyncRead;

mod async_seek;
pub use self::async_seek::AsyncSeek;

mod async_write;
pub use self::async_write::AsyncWrite;

mod read_buf;
pub use self::read_buf::ReadBuf;

// Re-export some types from `std::io` so that users don't have to deal
// with conflicts when `use`ing `tokio::io` and `std::io`.
#[doc(no_inline)]
pub use std::io::{Error, ErrorKind, Result, SeekFrom};

cfg_io_driver_impl! {
    pub(crate) mod interest;
    pub(crate) mod ready;

    cfg_net! {
        pub use interest::Interest;
        pub use ready::Ready;
    }

    #[cfg_attr(target_os = "wasi", allow(unused_imports))]
    mod poll_evented;

    #[cfg(not(loom))]
    #[cfg_attr(target_os = "wasi", allow(unused_imports))]
    pub(crate) use poll_evented::PollEvented;
}

cfg_aio! {
    /// BSD-specific I/O types.
    pub mod bsd {
        mod poll_aio;

        pub use poll_aio::{Aio, AioEvent, AioSource};
    }
}

cfg_net_unix! {
    mod async_fd;

    pub mod unix {
        //! Asynchronous IO structures specific to Unix-like operating systems.
        pub use super::async_fd::{AsyncFd, AsyncFdTryNewError, AsyncFdReadyGuard, AsyncFdReadyMutGuard, TryIoError};
    }
}

cfg_io_std! {
    mod stdio_common;

    mod stderr;
    pub use stderr::{stderr, Stderr};

    mod stdin;
    pub use stdin::{stdin, Stdin};

    mod stdout;
    pub use stdout::{stdout, Stdout};
}

cfg_io_util! {
    mod split;
    pub use split::{split, ReadHalf, WriteHalf};
    mod join;
    pub use join::{join, Join};

    pub(crate) mod seek;
    pub(crate) mod util;
    pub use util::{
        copy, copy_bidirectional, copy_bidirectional_with_sizes, copy_buf, duplex, empty, repeat, sink, simplex, AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, AsyncWriteExt,
        BufReader, BufStream, BufWriter, DuplexStream, Empty, Lines, Repeat, Sink, Split, Take, SimplexStream,
    };
}

cfg_not_io_util! {
    cfg_process! {
        pub(crate) mod util;
    }
}

cfg_io_blocking! {
    /// Types in this module can be mocked out in tests.
    mod sys {
        // TODO: don't rename
        pub(crate) use crate::blocking::spawn_blocking as run;
        pub(crate) use crate::blocking::JoinHandle as Blocking;
    }
}
