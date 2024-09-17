use crate::fs::asyncify;

use std::io;
use std::path::{Path, PathBuf};

pub async fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = path.as_ref().to_owned();
    asyncify(move || std::fs::canonicalize(path)).await
}
