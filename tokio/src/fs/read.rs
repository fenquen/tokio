use crate::fs::asyncify;

use std::{io, path::Path};

/// Reads the entire contents of a file into a bytes vector
pub async fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let path = path.as_ref().to_owned();
    asyncify(move || std::fs::read(path)).await
}
