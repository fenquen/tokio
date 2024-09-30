use crate::io::util::flush::{flush, Flush};
use crate::io::util::shutdown::{shutdown, Shutdown};
use crate::io::util::write::{write, Write};
use crate::io::util::write_all::{write_all, WriteAll};
use crate::io::util::write_all_buf::{write_all_buf, WriteAllBuf};
use crate::io::util::write_buf::{write_buf, WriteBuf};
use crate::io::util::write_int::{WriteF32, WriteF32Le, WriteF64, WriteF64Le};
use crate::io::util::write_int::{
    WriteI128, WriteI128Le, WriteI16, WriteI16Le, WriteI32, WriteI32Le, WriteI64, WriteI64Le,
    WriteI8,
};
use crate::io::util::write_int::{
    WriteU128, WriteU128Le, WriteU16, WriteU16Le, WriteU32, WriteU32Le, WriteU64, WriteU64Le,
    WriteU8,
};
use crate::io::util::write_vectored::{write_vectored, WriteVectored};
use crate::io::AsyncWrite;
use std::io::IoSlice;

use bytes::Buf;

cfg_io_util! {
    /// Defines numeric writer.
    macro_rules! write_impl {
        (
            $(
                $(#[$outer:meta])*
                fn $name:ident(&mut self, n: $ty:ty) -> $($fut:ident)*;
            )*
        ) => {
            $(
                $(#[$outer])*
                fn $name(&mut self, n: $ty) -> $($fut)*<&mut Self> where Self: Unpin {
                    $($fut)*::new(self, n)
                }
            )*
        }
    }

    /// Writes bytes to a sink.
    ///
    /// Implemented as an extension trait, adding utility methods to all
    /// [`AsyncWrite`] types. Callers will tend to import this trait instead of
    /// [`AsyncWrite`].
    ///
    /// ```no_run
    /// use tokio::io::{self, AsyncWriteExt};
    /// use tokio::fs::File;
    ///
    /// #[tokio::main]
    /// async fn main() -> io::Result<()> {
    ///     let data = b"some bytes";
    ///
    ///     let mut pos = 0;
    ///     let mut buffer = File::create("foo.txt").await?;
    ///
    ///     while pos < data.len() {
    ///         let bytes_written = buffer.write(&data[pos..]).await?;
    ///         pos += bytes_written;
    ///     }
    ///
    ///     Ok(())
    /// }
    /// ```
    ///
    /// See [module][crate::io] documentation for more details.
    ///
    /// [`AsyncWrite`]: AsyncWrite
    pub trait AsyncWriteExt: AsyncWrite {
        fn write<'a>(&'a mut self, src: &'a [u8]) -> Write<'a, Self>
        where
            Self: Unpin,
        {
            write(self, src)
        }

        /// Like [`write`], except that it writes from a slice of buffers.
        ///
        /// Equivalent to:
        ///
        /// ```ignore
        /// async fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize>;
        /// ```
        ///
        /// See [`AsyncWrite::poll_write_vectored`] for more details.
        ///
        /// # Cancel safety
        ///
        /// This method is cancellation safe in the sense that if it is used as
        /// the event in a [`tokio::select!`](crate::select) statement and some
        /// other branch completes first, then it is guaranteed that no data was
        /// written to this `AsyncWrite`.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// use tokio::io::{self, AsyncWriteExt};
        /// use tokio::fs::File;
        /// use std::io::IoSlice;
        ///
        /// #[tokio::main]
        /// async fn main() -> io::Result<()> {
        ///     let mut file = File::create("foo.txt").await?;
        ///
        ///     let bufs: &[_] = &[
        ///         IoSlice::new(b"hello"),
        ///         IoSlice::new(b" "),
        ///         IoSlice::new(b"world"),
        ///     ];
        ///
        ///     file.write_vectored(&bufs).await?;
        ///     file.flush().await?;
        ///
        ///     Ok(())
        /// }
        /// ```
        ///
        /// [`write`]: AsyncWriteExt::write
        fn write_vectored<'a, 'b>(&'a mut self, bufs: &'a [IoSlice<'b>]) -> WriteVectored<'a, 'b, Self>
        where
            Self: Unpin,
        {
            write_vectored(self, bufs)
        }

        fn write_buf<'a, B>(&'a mut self, src: &'a mut B) -> WriteBuf<'a, Self, B>
        where
            Self: Sized + Unpin,
            B: Buf,
        {
            write_buf(self, src)
        }

        fn write_all_buf<'a, B>(&'a mut self, src: &'a mut B) -> WriteAllBuf<'a, Self, B>
        where
            Self: Sized + Unpin,
            B: Buf,
        {
            write_all_buf(self, src)
        }


        fn write_all<'a>(&'a mut self, src: &'a [u8]) -> WriteAll<'a, Self>
        where
            Self: Unpin,
        {
            write_all(self, src)
        }

        write_impl! {
            fn write_u8(&mut self, n: u8) -> WriteU8;

            fn write_i8(&mut self, n: i8) -> WriteI8;

            fn write_u16(&mut self, n: u16) -> WriteU16;

            fn write_i16(&mut self, n: i16) -> WriteI16;

            fn write_u32(&mut self, n: u32) -> WriteU32;

            fn write_i32(&mut self, n: i32) -> WriteI32;

            fn write_u64(&mut self, n: u64) -> WriteU64;

            fn write_i64(&mut self, n: i64) -> WriteI64;

            fn write_u128(&mut self, n: u128) -> WriteU128;

            fn write_i128(&mut self, n: i128) -> WriteI128;

            fn write_f32(&mut self, n: f32) -> WriteF32;

            fn write_f64(&mut self, n: f64) -> WriteF64;

            fn write_u16_le(&mut self, n: u16) -> WriteU16Le;

            fn write_i16_le(&mut self, n: i16) -> WriteI16Le;

            fn write_u32_le(&mut self, n: u32) -> WriteU32Le;

            fn write_i32_le(&mut self, n: i32) -> WriteI32Le;

            fn write_u64_le(&mut self, n: u64) -> WriteU64Le;

            fn write_i64_le(&mut self, n: i64) -> WriteI64Le;

            fn write_u128_le(&mut self, n: u128) -> WriteU128Le;

            fn write_i128_le(&mut self, n: i128) -> WriteI128Le;

            fn write_f32_le(&mut self, n: f32) -> WriteF32Le;

            fn write_f64_le(&mut self, n: f64) -> WriteF64Le;
        }

        fn flush(&mut self) -> Flush<'_, Self>
        where
            Self: Unpin,
        {
            flush(self)
        }

        fn shutdown(&mut self) -> Shutdown<'_, Self>
        where
            Self: Unpin,
        {
            shutdown(self)
        }
    }
}

impl<W: AsyncWrite + ?Sized> AsyncWriteExt for W {}
