//! Open history: one line per file the user opens, with the local time, appended to
//! `open-history.tsv` beside the recovery folder. Unlike the recent-files list it keeps every
//! open (no cap, no de-duplication), so how often and when a file was opened can be read back.
//! Files the app reopens from the last session at startup and switching to a tab that is already
//! open are not opens and are not written. Desktop only.

use std::collections::HashSet;
use std::path::PathBuf;

/// Where opens are written, and the files being reopened from the last session.
#[derive(Default)]
pub struct OpenHistory {
    /// `None` keeps no history (tests, the web build, no home folder).
    pub file: Option<PathBuf>,
    /// Absolute paths reopened at startup; each is skipped once when it finishes opening.
    restoring: HashSet<String>,
}

impl OpenHistory {
    /// `open-history.tsv` in the portable data folder in portable mode, otherwise beside the
    /// recovery folder (`~/Library/Application Support/PdfCraft` on macOS).
    pub fn default_file() -> Option<PathBuf> {
        if let Some(dir) = crate::settings::portable_dir() {
            return Some(dir.join("open-history.tsv"));
        }
        crate::recovery::default_dir().and_then(|d| d.parent().map(|p| p.join("open-history.tsv")))
    }

    /// The next open of `path` is a session restore, not the user's.
    pub(crate) fn restoring(&mut self, path: &str) {
        self.restoring.insert(absolute(path));
    }

    /// `path` finished opening: append `time<TAB>path` unless it was a session restore.
    pub(crate) fn opened(&mut self, path: &str) {
        let path = absolute(path);
        if self.restoring.remove(&path) {
            return;
        }
        let Some(file) = &self.file else { return };
        let time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S %z");
        // Tabs and line breaks in a path would split the line.
        let clean: String = path.chars().map(|c| if c == '\t' || c == '\n' || c == '\r' { ' ' } else { c }).collect();
        let line = format!("{time}\t{clean}\n");
        let result = (|| {
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir)?;
            }
            use std::io::Write;
            std::fs::OpenOptions::new().create(true).append(true).open(file)?.write_all(line.as_bytes())
        })();
        if let Err(e) = result {
            log::warn!("could not write the open history to {}: {e}", file.display());
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn absolute(path: &str) -> String {
    std::path::absolute(path).map_or_else(|_| path.to_string(), |p| p.to_string_lossy().into_owned())
}

#[cfg(target_arch = "wasm32")]
fn absolute(path: &str) -> String {
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_are_appended_but_session_restores_are_not() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-open-history-{}", std::process::id()));
        let file = dir.join("open-history.tsv");
        let _ = std::fs::remove_file(&file);
        let mut h = OpenHistory { file: Some(file.clone()), ..Default::default() };
        h.restoring("/docs/restored.pdf");
        h.opened("/docs/restored.pdf");
        h.opened("/docs/a.pdf");
        h.opened("/docs/restored.pdf");
        let text = std::fs::read_to_string(&file).unwrap();
        let paths: Vec<&str> = text.lines().filter_map(|l| l.split('\t').nth(1)).collect();
        assert_eq!(paths, [absolute("/docs/a.pdf"), absolute("/docs/restored.pdf")]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
