//! A small file of the app's own in its settings folder (the plug-ins' settings, for one), at a
//! path the app chose: never one a document, a tool argument or an agent names. Read with a size
//! cap, written through a staging file ([`crate::staging::write_atomically`]).

use std::io::Read as _;
use std::path::{Path, PathBuf};

use crate::staging::{StagingName, write_atomically};

/// A settings file at a path the app chose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsFile(PathBuf);

impl SettingsFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        SettingsFile(path.into())
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Its bytes, `None` when there is no such file yet; an error past `max` bytes.
    pub fn read(&self, max: u64) -> std::io::Result<Option<Vec<u8>>> {
        let file = match std::fs::File::open(&self.0) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut bytes = Vec::new();
        file.take(max.saturating_add(1)).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > max {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("larger than {max} bytes")));
        }
        Ok(Some(bytes))
    }

    /// Replace it with `bytes`, creating its folder when needed.
    pub fn write(&self, bytes: &[u8]) -> std::io::Result<()> {
        if let Some(dir) = self.0.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        write_atomically(&self.0, bytes, StagingName::SuffixThenTag)
    }
}

#[cfg(test)]
mod tests {
    use super::SettingsFile;

    #[test]
    fn a_settings_file_is_written_whole_and_read_back_within_its_cap() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-settings-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let f = SettingsFile::new(dir.join("sub").join("s.json"));
        assert_eq!(f.read(10).unwrap(), None);
        f.write(b"{\"a\":1}").unwrap();
        assert_eq!(f.read(10).unwrap().as_deref(), Some(&b"{\"a\":1}"[..]));
        assert!(f.read(3).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
