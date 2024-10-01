#![cfg_attr(not(feature = "net"), allow(unreachable_pub))]

use crate::io::interest::Interest;

use std::fmt;
use std::ops;

const READABLE: usize = 0b0_01;
const WRITABLE: usize = 0b0_10;
const READ_CLOSED: usize = 0b0_0100;
const WRITE_CLOSED: usize = 0b0_1000;
#[cfg(any(target_os = "linux", target_os = "android"))]
const PRIORITY: usize = 0b1_0000;
const ERROR: usize = 0b10_0000;

/// 包含 高位的tick 和 低位的ready
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ready(usize);

impl Ready {
    /// Returns the empty `Ready` set.
    pub const EMPTY: Ready = Ready(0);

    /// Returns a `Ready` representing readable readiness.
    pub const READABLE: Ready = Ready(READABLE);

    /// Returns a `Ready` representing writable readiness.
    pub const WRITABLE: Ready = Ready(WRITABLE);

    /// Returns a `Ready` representing read closed readiness.
    pub const READ_CLOSED: Ready = Ready(READ_CLOSED);

    /// Returns a `Ready` representing write closed readiness.
    pub const WRITE_CLOSED: Ready = Ready(WRITE_CLOSED);

    /// Returns a `Ready` representing priority readiness.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[cfg_attr(docsrs, doc(cfg(any(target_os = "linux", target_os = "android"))))]
    pub const PRIORITY: Ready = Ready(PRIORITY);

    /// Returns a `Ready` representing error readiness.
    pub const ERROR: Ready = Ready(ERROR);

    /// Returns a `Ready` representing readiness for all operations.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub const ALL: Ready = Ready(READABLE | WRITABLE | READ_CLOSED | WRITE_CLOSED | ERROR | PRIORITY);

    /// Returns a `Ready` representing readiness for all operations.
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    pub const ALL: Ready = Ready(READABLE | WRITABLE | READ_CLOSED | WRITE_CLOSED | ERROR);

    // Must remain crate-private to avoid adding a public dependency on Mio.
    pub(crate) fn from_mio(event: &mio::event::Event) -> Ready {
        let mut ready = Ready::EMPTY;

        #[cfg(all(target_os = "freebsd", feature = "net"))]
        {
            if event.is_aio() {
                ready |= Ready::READABLE;
            }

            if event.is_lio() {
                ready |= Ready::READABLE;
            }
        }

        if event.is_readable() {
            ready |= Ready::READABLE;
        }

        if event.is_writable() {
            ready |= Ready::WRITABLE;
        }

        if event.is_read_closed() {
            ready |= Ready::READ_CLOSED;
        }

        if event.is_write_closed() {
            ready |= Ready::WRITE_CLOSED;
        }

        if event.is_error() {
            ready |= Ready::ERROR;
        }

        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            if event.is_priority() {
                ready |= Ready::PRIORITY;
            }
        }

        ready
    }

    pub fn is_empty(self) -> bool {
        self == Ready::EMPTY
    }

    pub fn is_readable(self) -> bool {
        self.contains(Ready::READABLE) || self.is_read_closed()
    }

    pub fn is_writable(self) -> bool {
        self.contains(Ready::WRITABLE) || self.is_write_closed()
    }

    pub fn is_read_closed(self) -> bool {
        self.contains(Ready::READ_CLOSED)
    }

    pub fn is_write_closed(self) -> bool {
        self.contains(Ready::WRITE_CLOSED)
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[cfg_attr(docsrs, doc(cfg(any(target_os = "linux", target_os = "android"))))]
    pub fn is_priority(self) -> bool {
        self.contains(Ready::PRIORITY)
    }

    pub fn is_error(self) -> bool {
        self.contains(Ready::ERROR)
    }

    pub(crate) fn contains<T: Into<Self>>(self, other: T) -> bool {
        let other = other.into();
        (self & other) == other
    }

    /// Creates a `Ready` instance using the given `usize` representation.
    ///
    /// The `usize` representation must have been obtained from a call to
    /// `Readiness::as_usize`.
    ///
    /// This function is mainly provided to allow the caller to get a
    /// readiness value from an `AtomicUsize`.
    pub(crate) fn from_usize(val: usize) -> Ready {
        Ready(val & Ready::ALL.as_usize())
    }

    pub(crate) fn as_usize(self) -> usize {
        self.0
    }

    pub(crate) fn from_interest(interest: Interest) -> Ready {
        let mut ready = Ready::EMPTY;

        if interest.is_readable() {
            ready |= Ready::READABLE;
            ready |= Ready::READ_CLOSED;
        }

        if interest.is_writable() {
            ready |= Ready::WRITABLE;
            ready |= Ready::WRITE_CLOSED;
        }

        #[cfg(any(target_os = "linux", target_os = "android"))]
        if interest.is_priority() {
            ready |= Ready::PRIORITY;
            ready |= Ready::READ_CLOSED;
        }

        if interest.is_error() {
            ready |= Ready::ERROR;
        }

        ready
    }

    pub(crate) fn intersection(self, interest: Interest) -> Ready {
        Ready(self.0 & Ready::from_interest(interest).0)
    }

    pub(crate) fn satisfies(self, interest: Interest) -> bool {
        self.0 & Ready::from_interest(interest).0 != 0
    }
}

impl ops::BitOr<Ready> for Ready {
    type Output = Ready;

    #[inline]
    fn bitor(self, other: Ready) -> Ready {
        Ready(self.0 | other.0)
    }
}

impl ops::BitOrAssign<Ready> for Ready {
    #[inline]
    fn bitor_assign(&mut self, other: Ready) {
        self.0 |= other.0;
    }
}

impl ops::BitAnd<Ready> for Ready {
    type Output = Ready;

    #[inline]
    fn bitand(self, other: Ready) -> Ready {
        Ready(self.0 & other.0)
    }
}

impl ops::Sub<Ready> for Ready {
    type Output = Ready;

    #[inline]
    fn sub(self, other: Ready) -> Ready {
        Ready(self.0 & !other.0)
    }
}

impl fmt::Debug for Ready {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut fmt = fmt.debug_struct("Ready");

        fmt.field("is_readable", &self.is_readable())
            .field("is_writable", &self.is_writable())
            .field("is_read_closed", &self.is_read_closed())
            .field("is_write_closed", &self.is_write_closed())
            .field("is_error", &self.is_error());

        #[cfg(any(target_os = "linux", target_os = "android"))]
        fmt.field("is_priority", &self.is_priority());

        fmt.finish()
    }
}
