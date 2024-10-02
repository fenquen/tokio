use crate::io::{AsyncRead, ReadBuf};

use pin_project_lite::pin_project;
use std::future::Future;
use std::io;
use std::marker::PhantomPinned;
use std::marker::Unpin;
use std::pin::Pin;
use std::task::{ready, Context, Poll};

pub(crate) fn read<'a, R: AsyncRead + Unpin + ?Sized>(asyncRead: &'a mut R, buf: &'a mut [u8]) -> Read<'a, R> {
    Read {
        asyncRead,
        buf,
        _pin: PhantomPinned,
    }
}

pin_project! {
    #[derive(Debug)]
    #[must_use = "futures do nothing unless you `.await` or poll them"]
    pub struct Read<'a, R: ?Sized> {
        asyncRead: &'a mut R,
        buf: &'a mut [u8],
        // Make this future `!Unpin` for compatibility with async trait methods.
        #[pin]
        _pin: PhantomPinned,
    }
}

impl<R: AsyncRead + Unpin + ?Sized> Future for Read<'_, R> {
    type Output = io::Result<usize>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<usize>> {
        let me = self.project();
        let mut buf = ReadBuf::new(me.buf);
        ready!(Pin::new(me.asyncRead).poll_read(context, &mut buf))?;
        Poll::Ready(Ok(buf.filled().len()))
    }
}
