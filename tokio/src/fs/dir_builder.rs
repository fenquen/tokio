use crate::fs::asyncify;

use std::io;
use std::path::Path;

/// A builder for creating directories in various manners.
///
/// This is a specialized version of [`std::fs::DirBuilder`] for usage on
/// the Tokio runtime.
#[derive(Debug, Default)]
pub struct DirBuilder {
    /// Indicates whether to create parent directories if they are missing.
    recursive: bool,

    /// Sets the Unix mode for newly created directories.
    #[cfg(unix)]
    pub(super) mode: Option<u32>,
}

impl DirBuilder {
    pub fn new() -> Self {
        DirBuilder::default()
    }

    /// Indicates whether to create directories recursively (including all parent directories).
    pub fn recursive(&mut self, recursive: bool) -> &mut Self {
        self.recursive = recursive;
        self
    }

    /// Creates the specified directory with the configured options.
    ///
    /// It is considered an error if the directory already exists unless
    /// recursive mode is enabled.
    ///
    /// This is an async version of [`std::fs::DirBuilder::create`].
    ///
    /// # Errors
    ///
    /// An error will be returned under the following circumstances:
    ///
    /// * Path already points to an existing file.
    /// * Path already points to an existing directory and the mode is
    ///   non-recursive.
    /// * The calling process doesn't have permissions to create the directory
    ///   or its missing parents.
    /// * Other I/O error occurred.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use tokio::fs::DirBuilder;
    /// use std::io;
    ///
    /// #[tokio::main]
    /// async fn main() -> io::Result<()> {
    ///     DirBuilder::new()
    ///         .recursive(true)
    ///         .create("/tmp/foo/bar/baz")
    ///         .await?;
    ///
    ///     Ok(())
    /// }
    /// ```
    pub async fn create(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = path.as_ref().to_owned();
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(self.recursive);

        #[cfg(unix)]
        {
            if let Some(mode) = self.mode {
                std::os::unix::fs::DirBuilderExt::mode(&mut builder, mode);
            }
        }

        asyncify(move || builder.create(path)).await
    }
}


#[cfg(unix)]
impl DirBuilder {
    /// set the mode to create new directories with.
    pub fn mode(&mut self, mode: u32) -> &mut Self {
        self.mode = Some(mode);
        self
    }
}

