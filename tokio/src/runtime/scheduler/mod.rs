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
use crate::runtime::scheduler::current_thread::CurrentThreadSchedulerHandle;
use crate::runtime::scheduler::multi_thread::MultiThreadSchedulerHandle;

#[derive(Debug, Clone)]
pub(crate) enum SchedulerHandleEnum {
    #[cfg(feature = "rt")]
    CurrentThread(Arc<CurrentThreadSchedulerHandle>),
    #[cfg(feature = "rt-multi-thread")]
    MultiThread(Arc<MultiThreadSchedulerHandle>),
}

impl SchedulerHandleEnum {
    pub(crate) fn driver(&self) -> &driver::DriverHandle {
        match *self {
            #[cfg(feature = "rt")]
            SchedulerHandleEnum::CurrentThread(ref h) => &h.driverHandle,
            #[cfg(feature = "rt-multi-thread")]
            SchedulerHandleEnum::MultiThread(ref h) => &h.driverHandle,
        }
    }
}

#[cfg(feature = "rt")]
pub(super) enum ThreadLocalContextEnum {
    CurrentThread(current_thread::Context),
    #[cfg(feature = "rt-multi-thread")]
    MultiThread(multi_thread::MultiThreadThreadLocalContext),
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
            }
        }
    }

    impl SchedulerHandleEnum {
        #[track_caller]
        pub(crate) fn current() -> SchedulerHandleEnum {
            match context::withCurrentSchedulerHandleEnum(Clone::clone) {
                Ok(schedulerHandleEnum) => schedulerHandleEnum,
                Err(e) => panic!("{}", e),
            }
        }

        pub(crate) fn getBlockingSpawner(&self) -> &blocking::Spawner {
            match self {
                SchedulerHandleEnum::CurrentThread(h) => &h.blocking_spawner,
                #[cfg(feature = "rt-multi-thread")]
                SchedulerHandleEnum::MultiThread(h) => &h.blockingSpawner,
            }
        }

        pub(crate) fn spawn<F:Future<Output: Send + 'static> + Send + 'static>(&self, future: F, id: Id) -> JoinHandle<F::Output> {
            match self {
                SchedulerHandleEnum::CurrentThread(h) => CurrentThreadSchedulerHandle::spawn(h, future, id),
                #[cfg(feature = "rt-multi-thread")]
                SchedulerHandleEnum::MultiThread(h) => MultiThreadSchedulerHandle::spawn(h, future, id),
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
            match self {
                SchedulerHandleEnum::CurrentThread(h) => &h.rngSeedGenerator,
                #[cfg(feature = "rt-multi-thread")]
                SchedulerHandleEnum::MultiThread(h) => &h.rngSeedGenerator,
            }
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
                SchedulerHandleEnum::MultiThread(h) => &h.taskHooks,
            }
        }
    }

    impl SchedulerHandleEnum {
        pub(crate) fn num_alive_tasks(&self) -> usize {
            match_flavor!(self, SchedulerHandleEnum(handle) => handle.num_alive_tasks())
        }
    }

    impl ThreadLocalContextEnum {
        #[track_caller]
        pub(crate) fn expect_current_thread(&self) -> &current_thread::Context {
            match self {
                ThreadLocalContextEnum::CurrentThread(context) => context,
                #[cfg(feature = "rt-multi-thread")]
                _ => panic!("expected `CurrentThread::Context`")
            }
        }

        pub(crate) fn defer(&self, waker: &Waker) {
            match self {
                ThreadLocalContextEnum::CurrentThread(context) => context. defer(waker),
                #[cfg(feature = "rt-multi-thread")]
                ThreadLocalContextEnum::MultiThread(context) => context. defer(waker),         }
        }

         #[cfg(feature = "rt-multi-thread")]
            #[track_caller]
            pub(crate) fn expect_multi_thread(&self) -> &multi_thread::MultiThreadThreadLocalContext {
                match self {
                    ThreadLocalContextEnum::MultiThread(multiThreadThreadLocalContext) => multiThreadThreadLocalContext,
                    _ => panic!("expected `MultiThread::Context`")
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
