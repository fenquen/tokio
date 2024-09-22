#![cfg_attr(not(feature = "rt"), allow(dead_code))]

//! Source of time abstraction.
//!
//! By default, `std::time::Instant::now()` is used. However, when the
//! `test-util` feature flag is enabled, the values returned for `now()` are
//! configurable.

cfg_not_test_util! {
    use crate::time::{Instant};

    #[derive(Debug, Clone)]
    pub(crate) struct Clock {}

    pub(crate) fn now() -> Instant {
        Instant::from_std(std::time::Instant::now())
    }

    impl Clock {
        pub(crate) fn new(_enable_pausing: bool, _start_paused: bool) -> Clock {
            Clock {}
        }

        pub(crate) fn now(&self) -> Instant {
            now()
        }
    }
}