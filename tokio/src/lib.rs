#![allow(unknown_lints, unexpected_cfgs)]
#![allow(
    clippy::cognitive_complexity,
    clippy::large_enum_variant,
    clippy::module_inception,
    clippy::needless_doctest_main
)]
#![warn(
    missing_debug_implementations,
    missing_docs,
    rust_2018_idioms,
    unreachable_pub
)]
#![deny(unused_must_use)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
#![allow(non_snake_case, unused_variables)]


// Includes re-exports used by macros.
//
// This module is not intended to be part of the public API. In general, any
// `doc(hidden)` code is not part of Tokio's public and stable API.
#[macro_use]
#[doc(hidden)]
pub mod macros;

cfg_fs! {
    pub mod fs;
}

mod future;

pub mod io;
pub mod net;

mod loom;

cfg_process! {
    pub mod process;
}

#[cfg(any(
    feature = "fs",
    feature = "io-std",
    feature = "net",
    all(windows, feature = "process"),
))]
mod blocking;

cfg_rt! {
    pub mod runtime;
}
cfg_not_rt! {
    pub(crate) mod runtime;
}

cfg_signal! {
    pub mod signal;
}

cfg_signal_internal! {
    #[cfg(not(feature = "signal"))]
    #[allow(dead_code)]
    #[allow(unreachable_pub)]
    pub(crate) mod signal;
}

cfg_sync! {
    pub mod sync;
}
cfg_not_sync! {
    mod sync;
}

pub mod task;
cfg_rt! {
    pub use task::spawn;
}

cfg_time! {
    pub mod time;
}

mod trace {
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    cfg_not_taskdump! {
        #[inline(always)]
        #[allow(dead_code)]
        pub(crate) fn trace_leaf(_: &mut Context<'_>) -> std::task::Poll<()> {
            Poll::Ready(())
        }
    }

    #[cfg_attr(not(feature = "sync"), allow(dead_code))]
    pub(crate) fn async_trace_leaf() -> impl Future<Output=()> {
        struct Trace;

        impl Future for Trace {
            type Output = ();

            #[inline(always)]
            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
                trace_leaf(cx)
            }
        }

        Trace
    }
}

mod util;

/// Due to the `Stream` trait's inclusion in `std` landing later than Tokio's 1.0
/// release, most of the Tokio stream utilities have been moved into the [`tokio-stream`]
/// crate.
///
/// # Why was `Stream` not included in Tokio 1.0?
///
/// Originally, we had planned to ship Tokio 1.0 with a stable `Stream` type
/// but unfortunately the [RFC] had not been merged in time for `Stream` to
/// reach `std` on a stable compiler in time for the 1.0 release of Tokio. For
/// this reason, the team has decided to move all `Stream` based utilities to
/// the [`tokio-stream`] crate. While this is not ideal, once `Stream` has made
/// it into the standard library and the `MSRV` period has passed, we will implement
/// stream for our different types.
///
/// While this may seem unfortunate, not all is lost as you can get much of the
/// `Stream` support with `async/await` and `while let` loops. It is also possible
/// to create a `impl Stream` from `async fn` using the [`async-stream`] crate.
///
/// [`tokio-stream`]: https://docs.rs/tokio-stream
/// [`async-stream`]: https://docs.rs/async-stream
/// [RFC]: https://github.com/rust-lang/rfcs/pull/2996
///
/// # Example
///
/// Convert a [`sync::mpsc::Receiver`] to an `impl Stream`.
///
/// ```rust,no_run
/// use tokio::sync::mpsc;
///
/// let (tx, mut rx) = mpsc::channel::<usize>(16);
///
/// let stream = async_stream::stream! {
///     while let Some(item) = rx.recv().await {
///         yield item;
///     }
/// };
/// ```
pub mod stream {}

// local re-exports of platform specific things, allowing for decent
// documentation to be shimmed in on docs.rs

cfg_macros! {
    /// Implementation detail of the `select!` macro. This macro is **not**
    /// intended to be used as part of the public API and is permitted to
    /// change.
    #[doc(hidden)]
    pub use tokio_macros::select_priv_declare_output_enum;

    /// Implementation detail of the `select!` macro. This macro is **not**
    /// intended to be used as part of the public API and is permitted to
    /// change.
    #[doc(hidden)]
    pub use tokio_macros::select_priv_clean_pattern;

    cfg_rt! {
        #[cfg(feature = "rt-multi-thread")]
        #[cfg_attr(docsrs, doc(cfg(feature = "macros")))]
        #[doc(inline)]
        pub use tokio_macros::main;

        #[cfg(feature = "rt-multi-thread")]
        #[cfg_attr(docsrs, doc(cfg(feature = "macros")))]
        #[doc(inline)]
        pub use tokio_macros::test;

        cfg_not_rt_multi_thread! {
            #[doc(inline)]
            pub use tokio_macros::main_rt as main;

            #[doc(inline)]
            pub use tokio_macros::test_rt as test;
        }
    }

    // Always fail if rt is not enabled.
    cfg_not_rt! {
        #[doc(inline)]
        pub use tokio_macros::main_fail as main;

        #[doc(inline)]
        pub use tokio_macros::test_fail as test;
    }
}

// TODO: rm
#[cfg(feature = "io-util")]
#[cfg(test)]
fn is_unpin<T: Unpin>() {}
