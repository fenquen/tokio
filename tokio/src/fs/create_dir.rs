use crate::fs::asyncify;

use std::io;
use std::path::Path;

pub async fn create_dir(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    asyncify(move || std::fs::create_dir(path)).await
}
