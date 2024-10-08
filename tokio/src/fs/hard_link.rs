use crate::fs::asyncify;

use std::io;
use std::path::Path;

/// Creates a new hard link on the filesystem.
pub async fn hard_link(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> io::Result<()> {
    let src = src.as_ref().to_owned();
    let dst = dst.as_ref().to_owned();

    asyncify(move || std::fs::hard_link(src, dst)).await
}
