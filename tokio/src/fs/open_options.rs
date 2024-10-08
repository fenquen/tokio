use crate::fs::{asyncify, File};

use std::io;
use std::path::Path;

#[cfg(not(test))]
use std::fs::OpenOptions as StdOpenOptions;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

#[derive(Clone, Debug)]
pub struct OpenOptions(StdOpenOptions);

impl OpenOptions {
    pub fn new() -> OpenOptions {
        OpenOptions(StdOpenOptions::new())
    }

    /// Sets the option for read access.
    pub fn read(&mut self, read: bool) -> &mut OpenOptions {
        self.0.read(read);
        self
    }

    /// Sets the option for write access.
    pub fn write(&mut self, write: bool) -> &mut OpenOptions {
        self.0.write(write);
        self
    }

    /// Sets the option for the append mode.
    pub fn append(&mut self, append: bool) -> &mut OpenOptions {
        self.0.append(append);
        self
    }

    /// Sets the option for truncating a previous file.
    pub fn truncate(&mut self, truncate: bool) -> &mut OpenOptions {
        self.0.truncate(truncate);
        self
    }

    /// Sets the option for creating a new file.
    pub fn create(&mut self, create: bool) -> &mut OpenOptions {
        self.0.create(create);
        self
    }

    /// Sets the option to always create a new file.
    ///
    /// This option indicates whether a new file will be created.  No file is
    /// allowed to exist at the target location, also no (dangling) symlink.
    ///
    /// This option is useful because it is atomic. Otherwise between checking
    /// whether a file exists and creating a new one, the file may have been
    /// created by another process (a TOCTOU race condition / attack).
    pub fn create_new(&mut self, create_new: bool) -> &mut OpenOptions {
        self.0.create_new(create_new);
        self
    }

    /// Opens a file at `path` with the options specified by `self`.
    pub async fn open(&self, path: impl AsRef<Path>) -> io::Result<File> {
        let path = path.as_ref().to_owned();
        let opts = self.0.clone();

        let std = asyncify(move || opts.open(path)).await?;
        Ok(File::from_std(std))
    }

    /// Returns a mutable reference to the underlying `std::fs::OpenOptions`
    pub(super) fn as_inner_mut(&mut self) -> &mut StdOpenOptions {
        &mut self.0
    }
}


#[cfg(unix)]
impl OpenOptions {
    /// Sets the mode bits that a new file will be created with.
    pub fn mode(&mut self, mode: u32) -> &mut OpenOptions {
        self.as_inner_mut().mode(mode);
        self
    }

    /// Passes custom flags to the `flags` argument of `open`.
    ///
    /// The bits that define the access mode are masked out with `O_ACCMODE`, to
    /// ensure they do not interfere with the access mode set by Rusts options.
    ///
    /// Custom flags can only set flags, not remove flags set by Rusts options.
    /// This options overwrites any previously set custom flags.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use tokio::fs::OpenOptions;
    /// use std::io;
    ///
    /// #[tokio::main]
    /// async fn main() -> io::Result<()> {
    ///     let mut options = OpenOptions::new();
    ///     options.write(true);
    ///     if cfg!(unix) {
    ///         options.custom_flags(libc::O_NOFOLLOW);
    ///     }
    ///     let file = options.open("foo.txt").await?;
    ///
    ///     Ok(())
    /// }
    /// ```
    pub fn custom_flags(&mut self, flags: i32) -> &mut OpenOptions {
        self.as_inner_mut().custom_flags(flags);
        self
    }
}


impl From<StdOpenOptions> for OpenOptions {
    fn from(options: StdOpenOptions) -> OpenOptions {
        OpenOptions(options)
    }
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self::new()
    }
}
