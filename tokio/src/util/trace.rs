cfg_time! {
    #[track_caller]
    pub(crate) fn caller_location() -> Option<&'static std::panic::Location<'static>> {
        #[cfg(all(tokio_unstable, feature = "tracing"))]
        return Some(std::panic::Location::caller());
        #[cfg(not(all(tokio_unstable, feature = "tracing")))]
        None
    }
}

cfg_not_trace! {
    cfg_rt! {
        #[inline]
        pub(crate) fn task<F>(task: F, _: &'static str, _name: Option<&str>, _: u64) -> F {
            // nop
            task
        }
    }
}
