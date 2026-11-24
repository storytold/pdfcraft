//! The canary: a file outside every allowed path, which a confined launcher must fail to read.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::Result;

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A fresh directory in the system's temporary directory holding one file. The launcher's
/// enforcement check must be denied reading the file and writing next to it. Both are removed
/// when the canary is dropped.
#[derive(Debug)]
pub struct Canary {
    dir: PathBuf,
    file: PathBuf,
}

impl Canary {
    pub fn create() -> Result<Canary> {
        let base = std::env::temp_dir().canonicalize()?;
        // `create_dir` (not `create_dir_all`) fails if the name exists, so the directory found is
        // new; a name another process left behind is skipped.
        let mut tries = 0;
        let dir = loop {
            let dir = base.join(format!("gridcraft-sandbox-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match std::fs::create_dir(&dir) {
                Ok(()) => break dir,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && tries < 100 => tries += 1,
                Err(e) => return Err(e.into()),
            }
        };
        let file = dir.join("canary");
        if let Err(e) = std::fs::write(&file, b"gridcraft sandbox canary\n") {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e.into());
        }
        Ok(Canary { dir, file })
    }

    pub fn file(&self) -> &Path {
        &self.file
    }
}

impl Drop for Canary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
