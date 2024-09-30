pub(crate) mod context;

pub(crate) mod coop;

pub(crate) mod park;

mod driver;

pub(crate) mod scheduler;

cfg_io_driver_impl! {
    pub(crate) mod io;
}

cfg_process_driver! {
    mod process;
}

cfg_time! {
    pub(crate) mod time;
}

cfg_signal_internal_and_unix! {
    pub(crate) mod signal;
}

cfg_rt! {
    pub(crate) mod task;

    mod config;
    use config::Config;

    mod blocking;
    #[cfg_attr(target_os = "wasi", allow(unused_imports))]
    pub(crate) use blocking::spawn_blocking;

    cfg_fs! {
        pub(crate) use blocking::spawn_mandatory_blocking;
    }

    mod builder;
    pub use self::builder::Builder;

    mod task_hooks;
    pub(crate) use task_hooks::{TaskHooks, TaskCallback};
    #[cfg(not(tokio_unstable))]
    pub(crate) use task_hooks::TaskMeta;

    mod handle;
    pub use handle::{EnterGuard, RuntimeHandle, TryCurrentError};

    mod runtime;
    pub use runtime::{Runtime, RuntimeFlavor};

    /// Boundary value to prevent stack overflow caused by a large-sized Future being placed in the stack.
    pub(crate) const BOX_FUTURE_THRESHOLD: usize = 2048;

    mod thread_id;
    pub(crate) use thread_id::ThreadId;

    /// After thread starts / before thread stops
    type Callback = std::sync::Arc<dyn Fn() + Send + Sync>;
}
