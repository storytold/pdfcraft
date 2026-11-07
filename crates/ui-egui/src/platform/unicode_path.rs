//! Open a path the operating system delivered in one Unicode normalization form from the file
//! named in another (macOS open-documents events, #73).
//!
//! A path can reach the app spelled in another normalization form than the name on disk:
//! Foundation spells the path of a file URL it makes from a path decomposed (NFD), whatever
//! form it was given, while names written elsewhere, such as those imported from Windows, are
//! often precomposed (NFC). On macOS 13.3, the APFS and HFS+ volumes it makes, case-sensitive or
//! not, fold the two forms in lookups, so there such a path opens as it arrives. Where a volume's
//! lookups tell the forms apart, a component in the other form names no entry and the file would
//! not open. Paths come here through `OsEvent::Open`: macOS 'odoc' events (a Finder double-click,
//! Open With, a Dock drop) and, on Windows, Linux and the BSDs, the files a second launch hands
//! to the running app. Drag-drop and the file picker deliver the exact bytes and do not.
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
    resolve_with(delivered, Path::exists)
}

/// [`resolve`] on the volume `exists` answers for, so the tests can hold names byte for byte on
/// any volume, including one that folds the forms.
fn resolve_with(delivered: &str, exists: impl Fn(&Path) -> bool) -> Option<String> {
    let path = Path::new(delivered);
    if exists(path) {
        return Some(delivered.to_owned());
    }
    let mut resolved = PathBuf::new();
    for component in path.components() {
        // Roots and prefixes (`/`, `C:`) carry no name to vary; `components` folds `.` away.
        // A non-UTF-8 component has no Unicode form to try either.
        let name = component.as_os_str().to_str()?;
        resolved = existing_form(&resolved, name, &exists)?;
    }
    // Only the empty path walks no components, and it names nothing to open.
    if resolved.as_os_str().is_empty() {
        return None;
    }
    Some(resolved.to_string_lossy().into_owned())
}

/// `dir`'s entry `name` in the form `exists` finds it in: as delivered, else its NFC form, else
/// its NFD form (an ASCII name has neither). NFC first is arbitrary but fixed, so a directory
/// holding both forms of one name resolves the same way every time.
fn existing_form(dir: &Path, name: &str, exists: &impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let delivered = dir.join(name);
    if exists(&delivered) {
        return Some(delivered);
    }
    if name.is_ascii() {
        return None;
    }
    for form in [name.nfc().collect::<String>(), name.nfd().collect::<String>()] {
        if form != name {
            let candidate = dir.join(form);
            if exists(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::HashSet;
    use std::path::{MAIN_SEPARATOR_STR, Path, PathBuf};

    use super::{resolve, resolve_with};

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

    /// Whether the filesystem under `dir` tells the forms apart: a lookup in one form misses a
    /// name stored in the other. The APFS and HFS+ volumes macOS 13.3 makes, case-sensitive or
    /// not, fold the forms instead, and there a delivered name opens the file as it arrives; the
    /// tests on the real filesystem then demand only that the file opens, and on a volume that
    /// tells the forms apart, the resolution itself. The walk is checked on every volume by the
    /// tests on [`Volume`].
    fn tells_forms_apart(dir: &std::path::Path) -> bool {
        std::fs::write(dir.join(NFC), b"").unwrap();
        let apart = !dir.join(NFD).exists();
        let _ = std::fs::remove_file(dir.join(NFC));
        let _ = std::fs::remove_file(dir.join(NFD));
        apart
    }

    /// A volume that tells the forms apart, whatever the one the tests run on does: it holds
    /// `paths` and the folders they are in, byte for byte, and counts the lookups made in it.
    struct Volume {
        entries: HashSet<PathBuf>,
        probes: Cell<usize>,
    }

    impl Volume {
        fn holding(paths: &[PathBuf]) -> Self {
            let entries = paths.iter().flat_map(|p| p.ancestors()).map(Path::to_path_buf).collect();
            Self { entries, probes: Cell::new(0) }
        }

        fn resolve(&self, delivered: &Path) -> Option<String> {
            resolve_with(delivered.to_str().unwrap(), |path| {
                self.probes.set(self.probes.get() + 1);
                self.entries.contains(path)
            })
        }
    }

    /// The root the in-memory paths hang from, so they read the same on every platform.
    fn root() -> PathBuf {
        PathBuf::from(MAIN_SEPARATOR_STR)
    }

    /// A path that exists as delivered comes back byte for byte: the resolver never hands the
    /// caller a re-encoded copy of a path it could have used exactly, so command lines,
    /// drag-drop and picker paths keep the bytes they arrived with.
    #[test]
    fn an_existing_path_is_returned_unchanged() {
        let dir = TempDir::new("exact");
        std::fs::write(dir.0.join(NFC), b"pdf").unwrap();
        let delivered = dir.0.join(NFC).to_string_lossy().into_owned();
        assert_eq!(resolve(&delivered).as_deref(), Some(delivered.as_str()), "no re-encoded copy of an existing path");
    }

    /// The one name the operating system got wrong is still opened: a delivered form that
    /// matches no entry resolves to the stored form, in both directions (NFD delivered with
    /// NFC stored — the 'odoc' case — and the reverse). On a volume that folds the forms the
    /// delivered path already opens the file, and the resolver says so by returning it.
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

    /// Forms can differ per component — a Windows-imported folder with a macOS-renamed file
    /// inside — so resolving the whole path in one form would miss: each component is tried
    /// in its own NFC and NFD forms, and the chain resolves however the forms are mixed.
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

    /// A name that matches in no form resolves to `None`, never to an invented path, so the
    /// caller keeps what the operating system sent and the error names it. An ASCII miss has
    /// no other form to try, a component that matches nothing ends the walk there, and the
    /// empty path names nothing to open.
    #[test]
    fn a_name_that_exists_in_no_form_resolves_to_nothing() {
        let dir = TempDir::new("missing");
        std::fs::write(dir.0.join(NFC), b"pdf").unwrap();
        // No "B\u{0300}se" anywhere; B with a combining grave has no precomposed form, so its NFC
        // and NFD forms are itself and nothing else is tried.
        assert_eq!(resolve(dir.0.join("B\u{0300}se").to_str().unwrap()), None);
        // An ASCII miss says nothing more for the resolver to try.
        assert_eq!(resolve(dir.0.join("missing.pdf").to_str().unwrap()), None);
        // A component that matches in no form stops the walk right there.
        assert_eq!(resolve(dir.0.join("B\u{0300}se").join("gone.pdf").to_str().unwrap()), None);
        // The empty path names nothing.
        assert_eq!(resolve(""), None);
    }

    /// On a volume that tells the forms apart, a name delivered in one form is found in the
    /// form it is stored in, both ways round: NFD delivered for an NFC name (the 'odoc' case)
    /// needs the NFC form tried, and the reverse the NFD form. Each form is looked up once.
    #[test]
    fn the_walk_finds_a_name_stored_in_the_other_form() {
        for (stored, delivered) in [(NFC, NFD), (NFD, NFC)] {
            let volume = Volume::holding(&[root().join(stored).join("doc.pdf")]);
            let found = volume.resolve(&root().join(delivered).join("doc.pdf"));
            assert_eq!(found, Some(root().join(stored).join("doc.pdf").to_string_lossy().into_owned()), "{stored:?} stored");
            assert_eq!(volume.probes.get(), 5, "the path as delivered, the root, the folder's two forms, the file");
        }
    }

    /// Where no form of a name exists, the walk finds nothing rather than inventing a path: a
    /// missing file below a folder found in its other form, and a name whose NFC and NFD forms
    /// differ but neither exists.
    #[test]
    fn the_walk_finds_nothing_where_no_form_exists() {
        let volume = Volume::holding(&[root().join(NFC).join("doc.pdf")]);
        assert_eq!(volume.resolve(&root().join(NFD).join("other.pdf")), None);
        assert_eq!(volume.resolve(&root().join("\u{00c9}t\u{00e9}")), None);
    }

    /// Each component is resolved in its own form: an NFC folder holding an NFD file, delivered
    /// as an NFD folder holding an NFC file, matches neither form of the whole path.
    #[test]
    fn the_walk_resolves_mixed_forms_component_by_component() {
        let stored = root().join(NFC).join(NFD);
        let volume = Volume::holding(std::slice::from_ref(&stored));
        assert_eq!(volume.resolve(&root().join(NFD).join(NFC)), Some(stored.to_string_lossy().into_owned()));
    }

    /// A path that exists as delivered comes back as it was spelled, after one lookup and no
    /// walk: a doubled separator, which the walk would drop, is still there.
    #[test]
    fn a_path_that_exists_as_delivered_is_not_walked() {
        let volume = Volume::holding(&[root().join(NFD).join("doc.pdf")]);
        let delivered = format!("{}{MAIN_SEPARATOR_STR}{MAIN_SEPARATOR_STR}doc.pdf", root().join(NFD).display());
        assert_eq!(volume.resolve(Path::new(&delivered)).as_deref(), Some(delivered.as_str()));
        assert_eq!(volume.probes.get(), 1, "one lookup, of the path as delivered");
    }

    /// Forms are tried in a fixed order — as delivered, NFC, NFD — so a folder holding more than
    /// one of them resolves the same way every time. "a" with an acute and a dot below, marks
    /// out of canonical order, is neither its NFC nor its NFD form.
    #[test]
    fn forms_are_tried_nfc_before_nfd() {
        let delivered = "a\u{0301}\u{0323}";
        let (nfc, nfd) = ("\u{1ea1}\u{0301}", "a\u{0323}\u{0301}");
        let volume = Volume::holding(&[root().join(nfc), root().join(nfd)]);
        assert_eq!(volume.resolve(&root().join(delivered)), Some(root().join(nfc).to_string_lossy().into_owned()));
    }
}
