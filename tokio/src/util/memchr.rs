//! Search for a byte in a byte array using libc.
//!
//! When nothing pulls in libc, then just use a trivial implementation. Note
//! that we only depend on libc on unix.

#[cfg(all(unix, feature = "libc"))]
pub(crate) fn memchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    let start = haystack.as_ptr();

    // SAFETY: `start` is valid for `haystack.len()` bytes.
    let ptr = unsafe { libc::memchr(start.cast(), needle as _, haystack.len()) };

    if ptr.is_null() {
        None
    } else {
        Some(ptr as usize - start as usize)
    }
}
