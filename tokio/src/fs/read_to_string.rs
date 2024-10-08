use crate::fs::asyncify;

use std::{io, path::Path};

/// This is the async equivalent of [`std::fs::read_to_string`][std].
pub async fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    let path = path.as_ref().to_owned();
    asyncify(move || std::fs::read_to_string(path)).await
}
