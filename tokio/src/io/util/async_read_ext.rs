use crate::io::util::chain::{chain, Chain};
use crate::io::util::read::{read, Read};
use crate::io::util::read_buf::{read_buf, ReadBuf};
use crate::io::util::read_exact::{read_exact, ReadExact};
use crate::io::util::read_int::{ReadF32, ReadF32Le, ReadF64, ReadF64Le};
use crate::io::util::read_int::{
    ReadI128, ReadI128Le, ReadI16, ReadI16Le, ReadI32, ReadI32Le, ReadI64, ReadI64Le, ReadI8,
};
use crate::io::util::read_int::{
    ReadU128, ReadU128Le, ReadU16, ReadU16Le, ReadU32, ReadU32Le, ReadU64, ReadU64Le, ReadU8,
};
use crate::io::util::read_to_end::{read_to_end, ReadToEnd};
use crate::io::util::read_to_string::{read_to_string, ReadToString};
use crate::io::util::take::{take, Take};
use crate::io::AsyncRead;

use bytes::BufMut;

cfg_io_util! {
    /// Defines numeric reader
    macro_rules! read_impl {
        (
            $(
                $(#[$outer:meta])*
                fn $name:ident(&mut self) -> $($fut:ident)*;
            )*
        ) => {
            $(
                $(#[$outer])*
                fn $name(&mut self) -> $($fut)*<&mut Self> where Self: Unpin {
                    $($fut)*::new(self)
                }
            )*
        }
    }

    pub trait AsyncReadExt: AsyncRead {
        /// creates a new `AsyncRead` instance that chains this stream with `next`.
        ///
        /// The returned `AsyncRead` instance will first read all bytes from this object
        /// until EOF is encountered. Afterwards the output is equivalent to the
        /// output of `next`.
        ///
        /// # Examples
        ///
        /// [`File`][crate::fs::File]s implement `AsyncRead`:
        ///
        /// ```no_run
        /// use tokio::fs::File;
        /// use tokio::io::{self, AsyncReadExt};
        ///
        /// #[tokio::main]
        /// async fn main() -> io::Result<()> {
        ///     let f1 = File::open("foo.txt").await?;
        ///     let f2 = File::open("bar.txt").await?;
        ///
        ///     let mut handle = f1.chain(f2);
        ///     let mut buffer = String::new();
        ///
        ///     // read the value into a String. We could use any AsyncRead
        ///     // method here, this is just one example.
        ///     handle.read_to_string(&mut buffer).await?;
        ///     Ok(())
        /// }
        /// ```
        fn chain<R>(self, next: R) -> Chain<Self, R>
        where
            Self: Sized,
            R: AsyncRead,
        {
            chain(self, next)
        }

        fn read<'a>(&'a mut self, buf: &'a mut [u8]) -> Read<'a, Self>
        where
            Self: Unpin,
        {
            read(self, buf)
        }

        fn read_buf<'a, B>(&'a mut self, buf: &'a mut B) -> ReadBuf<'a, Self, B>
        where
            Self: Unpin,
            B: BufMut + ?Sized,
        {
            read_buf(self, buf)
        }

        fn read_exact<'a>(&'a mut self, buf: &'a mut [u8]) -> ReadExact<'a, Self>
        where
            Self: Unpin,
        {
            read_exact(self, buf)
        }

        read_impl! {
            fn read_u8(&mut self) -> ReadU8;

            fn read_i8(&mut self) -> ReadI8;

            fn read_u16(&mut self) -> ReadU16;

            fn read_i16(&mut self) -> ReadI16;

            fn read_u32(&mut self) -> ReadU32;

            fn read_i32(&mut self) -> ReadI32;

            fn read_u64(&mut self) -> ReadU64;

            fn read_i64(&mut self) -> ReadI64;

            fn read_u128(&mut self) -> ReadU128;

            fn read_i128(&mut self) -> ReadI128;

            fn read_f32(&mut self) -> ReadF32;

            fn read_f64(&mut self) -> ReadF64;

            fn read_u16_le(&mut self) -> ReadU16Le;

            fn read_i16_le(&mut self) -> ReadI16Le;

            fn read_u32_le(&mut self) -> ReadU32Le;

            fn read_i32_le(&mut self) -> ReadI32Le;

            fn read_u64_le(&mut self) -> ReadU64Le;

            fn read_i64_le(&mut self) -> ReadI64Le;

            fn read_u128_le(&mut self) -> ReadU128Le;

            fn read_i128_le(&mut self) -> ReadI128Le;

            fn read_f32_le(&mut self) -> ReadF32Le;

            fn read_f64_le(&mut self) -> ReadF64Le;
        }

        fn read_to_end<'a>(&'a mut self, buf: &'a mut Vec<u8>) -> ReadToEnd<'a, Self>
        where
            Self: Unpin,
        {
            read_to_end(self, buf)
        }

        fn read_to_string<'a>(&'a mut self, dst: &'a mut String) -> ReadToString<'a, Self>
        where
            Self: Unpin,
        {
            read_to_string(self, dst)
        }

        /// Creates an adaptor which reads at most `limit` bytes from it.
        ///
        /// This function returns a new instance of `AsyncRead` which will read
        /// at most `limit` bytes, after which it will always return EOF
        /// (`Ok(0)`). Any read errors will not count towards the number of
        /// bytes read and future calls to [`read()`] may succeed.
        ///
        /// [`read()`]: fn@crate::io::AsyncReadExt::read
        ///
        /// [read]: AsyncReadExt::read
        ///
        /// # Examples
        ///
        /// [`File`][crate::fs::File]s implement `Read`:
        ///
        /// ```no_run
        /// use tokio::io::{self, AsyncReadExt};
        /// use tokio::fs::File;
        ///
        /// #[tokio::main]
        /// async fn main() -> io::Result<()> {
        ///     let f = File::open("foo.txt").await?;
        ///     let mut buffer = [0; 5];
        ///
        ///     // read at most five bytes
        ///     let mut handle = f.take(5);
        ///
        ///     handle.read(&mut buffer).await?;
        ///     Ok(())
        /// }
        /// ```
        fn take(self, limit: u64) -> Take<Self>
        where
            Self: Sized,
        {
            take(self, limit)
        }
    }
}

impl<R: AsyncRead + ?Sized> AsyncReadExt for R {}
