cfg_rt! {
    pub(crate) mod current_thread;
    pub(crate) use current_thread::CurrentThread;

    mod defer;
    use defer::Defer;

    pub(crate) mod inject;
    pub(crate) use inject::Inject;

    use crate::runtime::TaskHooks;
}

cfg_rt_multi_thread! {
    mod block_in_place;
    pub(crate) use block_in_place::block_in_place;

    mod lock;
    use lock::Lock;

    pub(crate) mod multi_thread;
    pub(crate) use multi_thread::MultiThread;
}

use crate::runtime::driver;

#[derive(Debug, Clone)]
pub(crate) enum SchedulerHandleEnum {
    #[cfg(feature = "rt")]
    CurrentThread(Arc<current_thread::CurrentThreadSchedulerHandle>),

    #[cfg(feature = "rt-multi-thread")]
    MultiThread(Arc<multi_thread::MultiThreadSchedulerHandle>),
}

#[cfg(feature = "rt")]
pub(super) enum Context {
    CurrentThread(current_thread::Context),

    #[cfg(feature = "rt-multi-thread")]
    MultiThread(multi_thread::Context),
}

impl SchedulerHandleEnum {
    #[cfg_attr(not(feature = "full"), allow(dead_code))]
    pub(crate) fn driver(&self) -> &driver::DriverHandle {
        match *self {
            #[cfg(feature = "rt")]
            SchedulerHandleEnum::CurrentThread(ref h) => &h.driverHandle,
            #[cfg(feature = "rt-multi-thread")]
            SchedulerHandleEnum::MultiThread(ref h) => &h.driverHandle,
        }
    }
}

cfg_rt! {
    use crate::future::Future;
    use crate::loom::sync::Arc;
    use crate::runtime::{blocking, task::Id};
    use crate::runtime::context;
    use crate::task::JoinHandle;
    use crate::util::RngSeedGenerator;
    use std::task::Waker;

    macro_rules! match_flavor {
        ($self:expr, $ty:ident($h:ident) => $e:expr) => {
            match $self {
                $ty::CurrentThread($h) => $e,

                #[cfg(feature = "rt-multi-thread")]
                $ty::MultiThread($h) => $e,

                #[cfg(all(tokio_unstable, feature = "rt-multi-thread"))]
                $ty::MultiThreadAlt($h) => $e,
            }
        }
    }

    impl SchedulerHandleEnum {
        #[track_caller]
        pub(crate) fn current() -> SchedulerHandleEnum {
            match context::with_current(Clone::clone) {
                Ok(schedulerHandleEnum) => schedulerHandleEnum,
                Err(e) => panic!("{}", e),
            }
        }

        pub(crate) fn getBlockingSpawner(&self) -> &blocking::Spawner {
            match self {
                SchedulerHandleEnum::CurrentThread(h) => &h.blocking_spawner,
                #[cfg(feature = "rt-multi-thread")]
                SchedulerHandleEnum::MultiThread(h) => &h.blocking_spawner,
            }
        }

        pub(crate) fn spawn<F>(&self, future: F, id: Id) -> JoinHandle<F::Output>
        where
            F: Future + Send + 'static,
            F::Output: Send + 'static,
        {
            match self {
                SchedulerHandleEnum::CurrentThread(h) => current_thread::CurrentThreadSchedulerHandle::spawn(h, future, id),
                #[cfg(feature = "rt-multi-thread")]
                SchedulerHandleEnum::MultiThread(h) => multi_thread::MultiThreadSchedulerHandle::spawn(h, future, id),
            }
        }

        pub(crate) fn shutdown(&self) {
            match *self {
                SchedulerHandleEnum::CurrentThread(_) => {},
                #[cfg(feature = "rt-multi-thread")]
                SchedulerHandleEnum::MultiThread(ref h) => h.shutdown(),
            }
        }

        pub(crate) fn seed_generator(&self) -> &RngSeedGenerator {
            match_flavor!(self, SchedulerHandleEnum(h) => &h.seed_generator)
        }

        pub(crate) fn as_current_thread(&self) -> &Arc<current_thread::CurrentThreadSchedulerHandle> {
            match self {
                SchedulerHandleEnum::CurrentThread(handle) => handle,
                #[cfg(feature = "rt-multi-thread")]
                _ => panic!("not a CurrentThread handle"),
            }
        }

        pub(crate) fn hooks(&self) -> &TaskHooks {
            match self {
                SchedulerHandleEnum::CurrentThread(h) => &h.task_hooks,
                #[cfg(feature = "rt-multi-thread")]
                SchedulerHandleEnum::MultiThread(h) => &h.task_hooks,
            }
        }
    }

    impl SchedulerHandleEnum {
        pub(crate) fn num_workers(&self) -> usize {
            match self {
                SchedulerHandleEnum::CurrentThread(_) => 1,
                #[cfg(feature = "rt-multi-thread")]
                SchedulerHandleEnum::MultiThread(handle) => handle.num_workers(),
            }
        }

        pub(crate) fn num_alive_tasks(&self) -> usize {
            match_flavor!(self, SchedulerHandleEnum(handle) => handle.num_alive_tasks())
        }
    }

    impl Context {
        #[track_caller]
        pub(crate) fn expect_current_thread(&self) -> &current_thread::Context {
            match self {
                Context::CurrentThread(context) => context,
                #[cfg(feature = "rt-multi-thread")]
                _ => panic!("expected `CurrentThread::Context`")
            }
        }

        pub(crate) fn defer(&self, waker: &Waker) {
            match_flavor!(self, Context(context) => context.defer(waker));
        }

        cfg_rt_multi_thread! {
            #[track_caller]
            pub(crate) fn expect_multi_thread(&self) -> &multi_thread::Context {
                match self {
                    Context::MultiThread(context) => context,
                    _ => panic!("expected `MultiThread::Context`")
                }
            }
        }
    }
}

cfg_not_rt! {
    #[cfg(any(
        feature = "net",
        all(unix, feature = "process"),
        all(unix, feature = "signal"),
        feature = "time",
    ))]
    impl SchedulerHandleEnum {
        #[track_caller]
        pub(crate) fn current() -> SchedulerHandleEnum {
            panic!("{}", crate::util::error::CONTEXT_MISSING_ERROR)
        }
    }
}
