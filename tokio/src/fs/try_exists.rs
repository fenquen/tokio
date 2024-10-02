use crate::fs::asyncify;

use std::io;
use std::path::Path;

/// Returns `Ok(true)` if the path points at an existing entity.
pub async fn try_exists(path: impl AsRef<Path>) -> io::Result<bool> {
    let path = path.as_ref().to_owned();
    asyncify(move || path.try_exists()).await
}
