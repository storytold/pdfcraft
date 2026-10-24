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
    let mut budget = 64u64 << 20;
    let mut seen = std::collections::HashSet::new();
    let mut seen_face_content = std::collections::HashSet::new();
    while let Some((path, depth)) = pending.pop() {
        if entries >= 4096 || files.len() >= 500 || budget == 0 {
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
                let mut sorted_children: Vec<_> = children.take(remaining).filter_map(Result::ok).map(|p| p.path()).collect();
                sorted_children.sort();
                pending.extend(sorted_children.into_iter().rev().map(|p| (p, depth + 1)));
            }
        } else if meta.is_file() && meta.len() <= (16 << 20) && font_extension(&path) {
            let mut bytes = Vec::new();
            let read = std::fs::File::open(&path).and_then(|f| f.take((16 << 20) + 1).read_to_end(&mut bytes));
            if read.is_err() {
                continue;
            }
            if pdfcraft_fonts::embedding::validate_document_input(&bytes).is_err() {
                continue;
            }
            if meta.len() > budget {
                continue;
            }
            budget = budget.saturating_sub(meta.len());
            if let Ok((family, face)) = pdfcraft_fonts::embedding::names(&bytes) {
                let key = font_content_key(&bytes);
                if seen_face_content.insert((face.clone(), key)) {
                    files.push(FontFile { path, family, face });
                }
            }
        }
    }
    files.sort_by(|a, b| a.face.cmp(&b.face).then(a.path.cmp(&b.path)));
    files
}

fn font_content_key(bytes: &[u8]) -> (usize, u64, u64) {
    use std::hash::{DefaultHasher, Hasher};
    let mut h1 = DefaultHasher::new();
    h1.write(bytes);
    let mut h2 = DefaultHasher::new();
    h2.write_u8(0xff);
    h2.write(bytes);
    (bytes.len(), h1.finish(), h2.finish())
}
fn font_extension(path: &Path) -> bool {
    path.extension().and_then(std::ffi::OsStr::to_str).is_some_and(|e| e.eq_ignore_ascii_case("ttf") || e.eq_ignore_ascii_case("otf"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_installed_directories() {
        let dirs = installed_directories();
        assert!(!dirs.is_empty());
    }

    #[test]
    fn test_font_extension() {
        assert!(font_extension(Path::new("Arial.ttf")));
        assert!(font_extension(Path::new("Noto.otf")));
        assert!(font_extension(Path::new("CUSTOM.TTF")));
        assert!(font_extension(Path::new("ROBOTO.OTF")));
        assert!(!font_extension(Path::new("document.pdf")));
        assert!(!font_extension(Path::new("font.woff")));
        assert!(!font_extension(Path::new("font.woff2")));
        assert!(!font_extension(Path::new("no_extension")));
    }

    #[test]
    fn test_font_content_key() {
        let empty = font_content_key(&[]);
        assert_eq!(empty.0, 0);

        let a = font_content_key(b"TrueType font sample data bytes");
        let b = font_content_key(b"TrueType font sample data bytes");
        let c = font_content_key(b"Different font sample data bytes");

        assert_eq!(a, b, "identical bytes must produce identical keys");
        assert_ne!(a, c, "differing bytes must produce distinct keys");
        assert_eq!(a.0, b"TrueType font sample data bytes".len());
    }

    #[test]
    fn test_discover_empty_and_nonexistent_roots() {
        let empty = discover(&[]);
        assert!(empty.is_empty());

        let nonexistent = discover(&[PathBuf::from("Z:\\nonexistent_font_path_xyz_12345")]);
        assert!(nonexistent.is_empty());
    }

    #[test]
    fn test_discover_deduplicates_non_adjacent_same_content() {
        let root = std::env::temp_dir().join(format!("pdfcraft-font-dedup-test-{}", std::process::id()));
        let dir_a = root.join("a");
        let dir_b = root.join("b");
        let dir_c = root.join("c");
        let _ = std::fs::create_dir_all(&dir_a);
        let _ = std::fs::create_dir_all(&dir_b);
        let _ = std::fs::create_dir_all(&dir_c);

        let inter_reg = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts/Inter-Regular.ttf");
        let inter_med = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts/Inter-Medium.ttf");

        if inter_reg.exists() && inter_med.exists() {
            let _ = std::fs::copy(&inter_reg, dir_a.join("FontA.ttf"));
            let _ = std::fs::copy(&inter_med, dir_b.join("FontB.ttf"));
            let _ = std::fs::copy(&inter_reg, dir_c.join("FontC.ttf"));

            let discovered = discover(std::slice::from_ref(&root));
            let _ = std::fs::remove_dir_all(&root);

            assert_eq!(discovered.len(), 2, "identical content in non-adjacent paths is deduplicated");
            let faces: Vec<&str> = discovered.iter().map(|f| f.face.as_str()).collect();
            assert!(faces.contains(&"Inter-Regular"));
            assert!(faces.contains(&"Inter-Medium"));
        } else {
            let _ = std::fs::remove_dir_all(&root);
        }
    }
}
