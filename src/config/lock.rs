use std::fs::{self, File};
use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::error::{TreehouseError, Result};

pub struct FileLock {
    _file: File,
    path: PathBuf,
}

impl FileLock {
    /// Acquire an exclusive lock on a file. Creates parent dirs and file if needed.
    pub fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let file = File::create(path)?;
        file.try_lock_exclusive().map_err(|e| {
            TreehouseError::LockFailed(format!("Could not acquire lock on {}: {e}", path.display()))
        })?;

        Ok(Self {
            _file: file,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

// Lock is released automatically when File is dropped (fs2 behavior)
