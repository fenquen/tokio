#[cfg(feature = "test-util")]
use crate::runtime::scheduler;
use crate::runtime::task::{self, Task, TaskHarnessScheduleHooks};
use crate::runtime::RuntimeHandle;

/// `task::Schedule` implementation that does nothing (except some bookkeeping
/// in test-util builds). This is unique to the blocking scheduler as tasks
/// scheduled are not really futures but blocking operations.
///
/// We avoid storing the task by forgetting it in `bind` and re-materializing it
/// in `release`.
pub(crate) struct BlockingSchedule {
    #[cfg(feature = "test-util")]
    handle: RuntimeHandle,
    hooks: TaskHarnessScheduleHooks,
}

impl BlockingSchedule {
    #[cfg_attr(not(feature = "test-util"), allow(unused_variables))]
    pub(crate) fn new(runtimeHandle: &RuntimeHandle) -> Self {
        #[cfg(feature = "test-util")]
        {
            match &runtimeHandle.schedulerHandleEnum {
                scheduler::SchedulerHandleEnum::CurrentThread(handle) => {
                    handle.driverHandle.clock.inhibit_auto_advance();
                }
                #[cfg(feature = "rt-multi-thread")]
                scheduler::SchedulerHandleEnum::MultiThread(_) => {}
            }
        }

        BlockingSchedule {
            #[cfg(feature = "test-util")]
            handle: runtimeHandle.clone(),
            hooks: TaskHarnessScheduleHooks {
                task_terminate_callback: runtimeHandle.schedulerHandleEnum.hooks().task_terminate_callback.clone(),
            },
        }
    }
}

impl task::Schedule for BlockingSchedule {
    fn release(&self, _task: &Task<Self>) -> Option<Task<Self>> {
        #[cfg(feature = "test-util")]
        {
            match &self.handle.schedulerHandleEnum {
                scheduler::SchedulerHandleEnum::CurrentThread(handle) => {
                    handle.driverHandle.clock.allow_auto_advance();
                    handle.driverHandle.unpark();
                }
                #[cfg(feature = "rt-multi-thread")]
                scheduler::SchedulerHandleEnum::MultiThread(_) => {}
                #[cfg(all(tokio_unstable, feature = "rt-multi-thread"))]
                scheduler::SchedulerHandleEnum::MultiThreadAlt(_) => {}
            }
        }
        None
    }

    fn schedule(&self, _task: task::NotifiedTask<Self>) {
        unreachable!();
    }

    fn hooks(&self) -> TaskHarnessScheduleHooks {
        TaskHarnessScheduleHooks {
            task_terminate_callback: self.hooks.task_terminate_callback.clone(),
        }
    }
}
