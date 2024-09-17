use crate::fs::asyncify;
use std::path::Path;

pub async fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<u64, std::io::Error> {
    let from = from.as_ref().to_owned();
    let to = to.as_ref().to_owned();
    asyncify(|| std::fs::copy(from, to)).await
}
