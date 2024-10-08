use crate::fs::asyncify;

use std::fs::Permissions;
use std::io;
use std::path::Path;

/// This is an async version of [`std::fs::set_permissions`]
pub async fn set_permissions(path: impl AsRef<Path>, perm: Permissions) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    asyncify(|| std::fs::set_permissions(path, perm)).await
}
