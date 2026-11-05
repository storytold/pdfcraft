//! Open a path the operating system delivered in one Unicode normalization form from the file
//! named in another (macOS open-documents events, #73).
//!
//! A Finder double-click, Open With or a Dock drop hands the app an 'odoc' Apple event whose
//! file URL decodes through `NSURL.path` in NFD (decomposed) form, while APFS is byte-exact and
//! keeps each name in whatever form it was created in — commonly NFC (precomposed), like names
//! imported from Windows. A byte-exact lookup then misses, and the file does not open. Command
//! lines, drag-drop and the file picker deliver the exact bytes and never hit this.
//!
//! [`resolve`] walks the delivered path component by component, and where a component names no
//! entry, tries its NFC and NFD forms. Both directions are covered — NFD delivered with NFC
//! stored, the reverse, and any mix along the chain, so per-component resolution is required
//! where one form for the whole path would not do. A path that exists as delivered is returned
//! untouched, keeping the exact bytes the caller got.

use std::path::{Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

/// The existing file or directory `delivered` names, with each component tried in its NFC and
/// NFD forms where the delivered form matches nothing (see the module docs). `None` when no
/// variant of the path exists: the caller keeps the delivered path, so the error it reports
/// names what the operating system actually sent.
pub(crate) fn resolve(delivered: &str) -> Option<String> {
    let path = Path::new(delivered);
    if path.exists() {
        return Some(delivered.to_owned());
    }
    let mut resolved = PathBuf::new();
    for component in path.components() {
        // Roots and prefixes (`/`, `C:`) carry no name to vary; `components` folds `.` away.
        // A non-UTF-8 component has no Unicode form to try either.
        let name = component.as_os_str().to_str()?;
        resolved = existing_form(&resolved, name)?;
    }
    // Only the empty path walks no components, and it names nothing to open.
    if resolved.as_os_str().is_empty() {
        return None;
    }
    Some(resolved.to_string_lossy().into_owned())
}

/// `dir`'s entry `name` in the form it exists in: as delivered, else its NFC form, else its
/// NFD form (an ASCII name has neither). NFC first is arbitrary but fixed, so a directory
/// holding both forms of one name resolves the same way every time.
fn existing_form(dir: &Path, name: &str) -> Option<PathBuf> {
    let delivered = dir.join(name);
    if delivered.exists() {
        return Some(delivered);
    }
    if name.is_ascii() {
        return None;
    }
    for form in [name.nfc().collect::<String>(), name.nfd().collect::<String>()] {
        if form != name {
            let candidate = dir.join(form);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::resolve;

    /// "Área": NFC has `Á` as one code point, NFD as `A` plus a combining acute.
    const NFC: &str = "\u{00c1}rea";
    const NFD: &str = "A\u{0301}rea";

    /// A fresh directory to create names in, removed when the test ends.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("pdfcraft-unicode-path-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&mut self.0);
        }
    }

    /// Whether the filesystem under `dir` answers a lookup in one form for a name stored in
    /// the other. APFS's default volume does (normalization-insensitive): there the delivered
    /// name opens the file as it arrives, and the walk this module exists for has nothing to
    /// do. Linux, Windows and case-sensitive APFS tell the forms apart, and the tests then
    /// demand the resolution; everywhere they demand only that the file opens.
    fn tells_forms_apart(dir: &std::path::Path) -> bool {
        std::fs::write(dir.join(NFC), b"").unwrap();
        let apart = !dir.join(NFD).exists();
        let _ = std::fs::remove_file(dir.join(NFC));
        let _ = std::fs::remove_file(dir.join(NFD));
        apart
    }

    #[test]
    fn an_existing_path_is_returned_unchanged() {
        let dir = TempDir::new("exact");
        std::fs::write(dir.0.join(NFC), b"pdf").unwrap();
        let delivered = dir.0.join(NFC).to_string_lossy().into_owned();
        assert_eq!(resolve(&delivered).as_deref(), Some(delivered.as_str()), "no re-encoded copy of an existing path");
    }

    #[test]
    fn a_delivered_name_opens_a_file_stored_in_the_other_form() {
        for (stored, delivered, tag) in [(NFC, NFD, "nfc-stored"), (NFD, NFC, "nfd-stored")] {
            let dir = TempDir::new(tag);
            let apart = tells_forms_apart(&dir.0);
            std::fs::write(dir.0.join(stored), b"pdf").unwrap();
            let delivered = dir.0.join(delivered).to_str().unwrap().to_owned();
            let found = resolve(&delivered).expect("a path that exists");
            assert!(matches!(std::fs::read(&found), Ok(bytes) if bytes == b"pdf"), "the found path opens the file");
            if apart {
                assert_eq!(found, dir.0.join(stored).to_string_lossy().into_owned(), "resolved to the stored form");
            } else {
                assert_eq!(found, delivered, "a volume that folds the forms keeps the delivered path");
            }
        }
    }

    #[test]
    fn mixed_forms_along_the_chain_resolve_component_by_component() {
        let dir = TempDir::new("mixed");
        let apart = tells_forms_apart(&dir.0);
        // NFC directory, NFD file inside it — what a Windows-imported folder with a macOS
        // renamed file inside looks like on APFS.
        let stored = dir.0.join(NFC).join(NFD);
        std::fs::create_dir_all(dir.0.join(NFC)).unwrap();
        std::fs::write(&stored, b"pdf").unwrap();
        // Delivered the other way round: NFD directory, NFC file.
        let delivered = dir.0.join(NFD).join(NFC).to_str().unwrap().to_owned();
        let found = resolve(&delivered).expect("a path that exists");
        assert!(matches!(std::fs::read(&found), Ok(bytes) if bytes == b"pdf"), "the found path opens the file");
        if apart {
            assert_eq!(found, stored.to_string_lossy().into_owned(), "each component resolved to its stored form");
        } else {
            assert_eq!(found, delivered, "a volume that folds the forms keeps the delivered path");
        }
    }

    #[test]
    fn a_name_that_exists_in_no_form_resolves_to_nothing() {
        let dir = TempDir::new("missing");
        std::fs::write(dir.0.join(NFC), b"pdf").unwrap();
        // No "B\u{0300}se" (grave; NFC "B\u{00c0}se") anywhere, in either form: keep nothing.
        assert_eq!(resolve(dir.0.join("B\u{0300}se").to_str().unwrap()), None);
        // An ASCII miss says nothing more for the resolver to try.
        assert_eq!(resolve(dir.0.join("missing.pdf").to_str().unwrap()), None);
        // A component that matches in no form stops the walk right there.
        assert_eq!(resolve(dir.0.join("B\u{0300}se").join("gone.pdf").to_str().unwrap()), None);
        // The empty path names nothing.
        assert_eq!(resolve(""), None);
    }
}
