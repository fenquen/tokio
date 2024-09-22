//! Extra utilities for spawning tasks

mod spawn_pinned;
pub use spawn_pinned::LocalPoolHandle;

pub mod task_tracker;
pub use task_tracker::TaskTracker;

mod abort_on_drop;
pub use abort_on_drop::AbortOnDropHandle;
