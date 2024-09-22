#[allow(dead_code)]
pub(crate) struct SyncNotSend(#[allow(dead_code)] *mut ());

unsafe impl Sync for SyncNotSend {}

#[cfg(feature = "rt")]
pub(crate) struct NotSendOrSync(#[allow(dead_code)] *mut ());

