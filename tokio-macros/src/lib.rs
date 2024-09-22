#![allow(unknown_lints, unexpected_cfgs)]
#![allow(clippy::needless_doctest_main)]
#![warn(
    missing_debug_implementations,
    missing_docs,
    rust_2018_idioms,
    unreachable_pub
)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Macros for use with Tokio

// This `extern` is required for older `rustc` versions but newer `rustc`
// versions warn about the unused `extern crate`.
#[allow(unused_extern_crates)]
extern crate proc_macro;

mod entry;
mod select;

use proc_macro::TokenStream;

/// # Multi-threaded runtime
///
/// To use the multi-threaded runtime, the macro can be configured using
///
/// ```
/// #[tokio::main(flavor = "multi_thread", worker_threads = 10)]
/// # async fn main() {}
/// ```
///
/// The `worker_threads` option configures the number of worker threads, and
/// defaults to the number of cpus on the system. This is the default flavor.
///
/// Note: The multi-threaded runtime requires the `rt-multi-thread` feature
/// flag.
///
/// # Current thread runtime
///
/// ```
/// #[tokio::main(flavor = "current_thread")]
/// # async fn main() {}
/// ```
///
/// ### Using current thread runtime
///
/// The basic scheduler is single-threaded.
///
/// ```rust
/// #[tokio::main(flavor = "current_thread")]
/// async fn main() {
///     println!("Hello world");
/// }
/// ```
///
/// Equivalent code
///
/// ```rust
/// fn main() {
///     tokio::runtime::Builder::new_current_thread()
///         .enable_all()
///         .build()
///         .unwrap()
///         .block_on(async {
///             println!("Hello world");
///         })
/// }
/// ```
#[proc_macro_attribute]
pub fn main(args: TokenStream, item: TokenStream) -> TokenStream {
    entry::main(args.into(), item.into(), true).into()
}

/// Marks async function to be executed by selected runtime. This macro helps set up a `Runtime`
/// without requiring the user to use [Runtime](../tokio/runtime/struct.Runtime.html) or
/// [Builder](../tokio/runtime/struct.Builder.html) directly.
///
/// ## Function arguments:
///
/// Arguments are allowed for any functions aside from `main` which is special
///
/// ## Usage
///
/// ### Using default
///
/// ```rust
/// #[tokio::main(flavor = "current_thread")]
/// async fn main() {
///     println!("Hello world");
/// }
/// ```
///
/// Equivalent code not using `#[tokio::main]`
///
/// ```rust
/// fn main() {
///     tokio::runtime::Builder::new_current_thread()
///         .enable_all()
///         .build()
///         .unwrap()
///         .block_on(async {
///             println!("Hello world");
///         })
/// }
/// ```
///
/// ### Rename package
///
/// ```rust
/// use tokio as tokio1;
///
/// #[tokio1::main(crate = "tokio1")]
/// async fn main() {
///     println!("Hello world");
/// }
/// ```
///
/// Equivalent code not using `#[tokio::main]`
///
/// ```rust
/// use tokio as tokio1;
///
/// fn main() {
///     tokio1::runtime::Builder::new_multi_thread()
///         .enable_all()
///         .build()
///         .unwrap()
///         .block_on(async {
///             println!("Hello world");
///         })
/// }
/// ```
#[proc_macro_attribute]
pub fn main_rt(args: TokenStream, item: TokenStream) -> TokenStream {
    entry::main(args.into(), item.into(), false).into()
}

#[proc_macro_attribute]
pub fn test(args: TokenStream, item: TokenStream) -> TokenStream {
    entry::test(args.into(), item.into(), true).into()
}

/// Marks async function to be executed by runtime, suitable to test environment
///
/// ## Usage
///
/// ```no_run
/// #[tokio::test]
/// async fn my_test() {
///     assert!(true);
/// }
/// ```
#[proc_macro_attribute]
pub fn test_rt(args: TokenStream, item: TokenStream) -> TokenStream {
    entry::test(args.into(), item.into(), false).into()
}

/// Always fails with the error message below.
/// ```text
/// The #[tokio::main] macro requires rt or rt-multi-thread.
/// ```
#[proc_macro_attribute]
pub fn main_fail(_args: TokenStream, _item: TokenStream) -> TokenStream {
    syn::Error::new(
        proc_macro2::Span::call_site(),
        "The #[tokio::main] macro requires rt or rt-multi-thread.",
    )
    .to_compile_error()
    .into()
}

/// Always fails with the error message below.
/// ```text
/// The #[tokio::test] macro requires rt or rt-multi-thread.
/// ```
#[proc_macro_attribute]
pub fn test_fail(_args: TokenStream, _item: TokenStream) -> TokenStream {
    syn::Error::new(
        proc_macro2::Span::call_site(),
        "The #[tokio::test] macro requires rt or rt-multi-thread.",
    )
    .to_compile_error()
    .into()
}

/// Implementation detail of the `select!` macro. This macro is **not** intended
/// to be used as part of the public API and is permitted to change.
#[proc_macro]
#[doc(hidden)]
pub fn select_priv_declare_output_enum(input: TokenStream) -> TokenStream {
    select::declare_output_enum(input)
}

/// Implementation detail of the `select!` macro. This macro is **not** intended
/// to be used as part of the public API and is permitted to change.
#[proc_macro]
#[doc(hidden)]
pub fn select_priv_clean_pattern(input: TokenStream) -> TokenStream {
    select::clean_pattern_macro(input)
}
