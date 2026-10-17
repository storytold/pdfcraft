//! Explicit document-font discovery. Interface fallbacks never enter this catalogue.
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct FontFile {
    pub path: PathBuf,
    pub family: String,
    pub face: String,
}

/// Standard user-installed font folders; discovery is performed only on request.
pub fn installed_directories() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        let mut dirs = vec![PathBuf::from("/Library/Fonts")];
        if let Some(home) = home {
            dirs.push(home.join("Library/Fonts"));
        }
        dirs
    } else if cfg!(windows) {
        let mut dirs = std::env::var_os("WINDIR").map(|p| PathBuf::from(p).join("Fonts")).into_iter().collect::<Vec<_>>();
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Microsoft/Windows/Fonts"));
        }
        dirs
    } else {
        let mut dirs = vec![PathBuf::from("/usr/share/fonts"), PathBuf::from("/usr/local/share/fonts")];
        if let Some(home) = home {
            dirs.extend([home.join(".local/share/fonts"), home.join(".fonts")]);
        }
        dirs
    }
}

/// Bounded, symlink-free scan. Only standalone, openly licensed, embeddable faces are listed.
/// The caller controls roots (automation resolves confinement before calling this).
pub fn discover(roots: &[PathBuf]) -> Vec<FontFile> {
    use std::io::Read;
    let mut pending: Vec<_> = roots.iter().take(16).map(|r| (r.clone(), 0usize)).collect();
    let mut files = Vec::new();
    let mut entries = 0usize;
    let mut seen = std::collections::HashSet::new();
    while let Some((path, depth)) = pending.pop() {
        if entries >= 4096 || files.len() >= 500 {
            break;
        }
        entries += 1;
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        if meta.file_type().is_symlink() || !seen.insert(path.clone()) {
            continue;
        }
        if meta.is_dir() && depth < 4 {
            if let Ok(children) = std::fs::read_dir(&path) {
                let remaining = 4096usize.saturating_sub(entries.saturating_add(pending.len()));
                pending.extend(children.take(remaining).filter_map(Result::ok).map(|p| (p.path(), depth + 1)));
            }
        } else if meta.is_file() && meta.len() <= (16 << 20) && font_extension(&path) {
            let mut bytes = Vec::new();
            let read = std::fs::File::open(&path).and_then(|f| f.take((16 << 20) + 1).read_to_end(&mut bytes));
            if read.is_err() {
                continue;
            }
            if let Ok(bytes) = read.map(|_| bytes) {
                if pdfcraft_fonts::embedding::validate_document_input(&bytes).is_err() {
                    continue;
                }
                if let Ok((family, face)) = pdfcraft_fonts::embedding::names(&bytes) {
                    files.push(FontFile { path, family, face });
                }
            }
        }
    }
    files.sort_by(|a, b| a.face.cmp(&b.face).then(a.path.cmp(&b.path)));
    files.dedup_by(|a, b| a.face == b.face);
    files
}
fn font_extension(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("ttf") || e.eq_ignore_ascii_case("otf"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_installed_directories() {
        let dirs = installed_directories();
        assert!(!dirs.is_empty());
    }
}
