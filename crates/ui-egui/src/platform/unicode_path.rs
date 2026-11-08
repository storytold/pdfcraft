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
//! untouched, keeping the exact bytes the caller got. One that ends in a separator, or in a `.`
//! after one, is not walked: only a folder satisfies such a path, and the walk rebuilds the path
//! from its components, which drop that ending, so it could return a file of that name instead.
//! Where a later component matches in no form, the walk goes back and tries the next form of the
//! one before. It makes at most three times as many lookups as the path has components, after
//! the one for the path as delivered; past that, the path counts as not found.

use std::path::{Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

/// The existing file or directory `delivered` names, with each component tried in its NFC and
/// NFD forms where the delivered form matches nothing, or leads to no match further down (see
/// the module docs). `None` when no variant of the path is found, or the path is one the walk
/// leaves alone: the caller keeps the delivered path, so the error it reports names what the
/// operating system actually sent.
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
    // Roots and prefixes (`/`, `C:`) carry no name to vary, nor does a leading `.`, the one `.`
    // that `components` keeps.
    // `delivered` is a `&str`, so every component is UTF-8 and this `?` never returns: it keeps
    // the function total without an `expect`.
    let names = path.components().map(|c| c.as_os_str().to_str()).collect::<Option<Vec<_>>>()?;
    // Only the empty path has no components, and it names nothing to open.
    let last = names.last()?;
    // `components` drops a separator or `.` at the end, so where the path does not end in its
    // last name, the path rebuilt from `names` can be a file where only a folder will do:
    // `doc.pdf/` fails where `doc.pdf` is a file, and the walk would return that file.
    if !delivered.ends_with(last) {
        return None;
    }
    // Three lookups for each component, shared by the whole walk: the most a walk keeping the
    // first form that exists could make, so the path that walk finds is always reached, and
    // trying other forms never makes the number of lookups more than linear in the path's length.
    let mut probes = names.len().saturating_mul(3);
    walk(&names, &mut probes, &exists).map(|found| found.to_string_lossy().into_owned())
}

/// The first existing path naming each of `names` in one of its [`forms`], tried in that order:
/// where a later component matches in no form, the walk goes back and tries the next form of the
/// one before. `None` when no such path exists or once `probes` runs out. The walk keeps its
/// place on the heap, not on the call stack, so no path is deep enough to overflow it, and it
/// keeps the form each component matched so far was found in rather than a path to each, so its
/// memory grows with the path's length and not with its length times its depth.
fn walk(names: &[&str], probes: &mut usize, exists: &impl Fn(&Path) -> bool) -> Option<PathBuf> {
    // The form each component matched so far exists in, and for each of those components and the
    // one after them, the forms still to try.
    let mut matched: Vec<String> = Vec::new();
    let mut untried = vec![forms(names.first()?).into_iter()];
    while let Some(next) = untried.last_mut() {
        let Some(form) = next.next() else {
            untried.pop();
            matched.pop();
            continue;
        };
        *probes = probes.checked_sub(1)?;
        let candidate: PathBuf = matched.iter().chain([&form]).collect();
        if exists(&candidate) {
            matched.push(form);
            match names.get(matched.len()) {
                Some(name) => untried.push(forms(name).into_iter()),
                None => return Some(candidate),
            }
        }
    }
    None
}

/// `name` as delivered, then its NFC form, then its NFD form, each once (an ASCII name has no
/// other). NFC first is arbitrary but fixed, so a directory holding both forms of one name
/// resolves the same way every time.
fn forms(name: &str) -> Vec<String> {
    let mut forms = vec![name.to_owned()];
    if !name.is_ascii() {
        for form in [name.nfc().collect::<String>(), name.nfd().collect::<String>()] {
            if !forms.contains(&form) {
                forms.push(form);
            }
        }
    }
    forms
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
    /// real-filesystem tests that call it then demand only that the file opens, and on a volume
    /// that tells the forms apart, the resolution itself. The walk is checked on every volume by
    /// the tests on [`Volume`].
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
    /// no other form to try, a component that matches in no form, below folders that have no
    /// other form, leaves the walk nothing to go back to, and the empty path names nothing to
    /// open.
    #[test]
    fn a_name_that_exists_in_no_form_resolves_to_nothing() {
        let dir = TempDir::new("missing");
        std::fs::write(dir.0.join(NFC), b"pdf").unwrap();
        // No "B\u{0300}se" anywhere; B with a combining grave has no precomposed form, so its NFC
        // and NFD forms are itself and nothing else is tried.
        assert_eq!(resolve(dir.0.join("B\u{0300}se").to_str().unwrap()), None);
        // An ASCII miss says nothing more for the resolver to try.
        assert_eq!(resolve(dir.0.join("missing.pdf").to_str().unwrap()), None);
        // A component that matches in no form, below folders with no other form, ends the walk.
        assert_eq!(resolve(dir.0.join("B\u{0300}se").join("gone.pdf").to_str().unwrap()), None);
        // The empty path names nothing.
        assert_eq!(resolve(""), None);
    }

    /// A file with `/`, `//` or `/.` after its name resolves to `None`, in either form of the
    /// name, on any Unix volume (POSIX gives `ENOTDIR` there; run here on APFS): a regular file
    /// does not exist under that path, and the file without the ending is not offered instead. A
    /// folder with `/` after its name exists as delivered and comes back unchanged. Unix only:
    /// what Windows does with such a path was not checked.
    #[cfg(unix)]
    #[test]
    fn a_file_with_a_separator_after_its_name_resolves_to_nothing() {
        let dir = TempDir::new("trailing");
        std::fs::write(dir.0.join("doc.pdf"), b"pdf").unwrap();
        std::fs::write(dir.0.join(format!("{NFC}.pdf")), b"pdf").unwrap();
        std::fs::create_dir(dir.0.join(NFC)).unwrap();
        let base = dir.0.to_str().unwrap();
        for name in ["doc.pdf".to_owned(), format!("{NFC}.pdf"), format!("{NFD}.pdf")] {
            for ending in ["/", "//", "/."] {
                let delivered = format!("{base}/{name}{ending}");
                assert_eq!(resolve(&delivered), None, "{delivered:?}");
            }
        }
        let folder = format!("{base}/{NFC}/");
        assert_eq!(resolve(&folder).as_deref(), Some(folder.as_str()), "a folder that exists as delivered");
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

    /// A path ending in a separator, or in a `.` after one, is not walked, though the same path
    /// without that ending is found in the other form: the walk rebuilds the path from its
    /// components, which drop the ending, and would return a file where only a folder will do.
    /// One lookup, of the path as delivered. A name that itself ends in a `.` ends in its last
    /// component and is still walked.
    #[test]
    fn a_path_ending_in_a_separator_is_not_walked() {
        let volume = Volume::holding(&[root().join(NFC)]);
        assert_eq!(volume.resolve(&root().join(NFD)), Some(root().join(NFC).to_string_lossy().into_owned()));
        for ending in [MAIN_SEPARATOR_STR.to_owned(), format!("{MAIN_SEPARATOR_STR}.")] {
            let delivered = format!("{}{ending}", root().join(NFD).display());
            volume.probes.set(0);
            assert_eq!(volume.resolve(Path::new(&delivered)), None, "{delivered:?}");
            assert_eq!(volume.probes.get(), 1, "one lookup, of the path as delivered");
        }
        // A name that ends in a `.` ends in its last component, and is walked.
        let dotted = Volume::holding(&[root().join(format!("{NFC}."))]);
        let found = dotted.resolve(&root().join(format!("{NFD}.")));
        assert_eq!(found, Some(root().join(format!("{NFC}.")).to_string_lossy().into_owned()), "a name ending in a `.`");
    }

    /// Where both forms of a folder name exist side by side, a file that only the second form
    /// holds is still found: the walk goes back to the folder and tries its next form instead of
    /// keeping the first one that exists.
    #[test]
    fn a_file_only_the_other_form_of_its_folder_holds_is_found() {
        for (first, second) in [(NFC, NFD), (NFD, NFC)] {
            let file = root().join(second).join("doc.pdf");
            let volume = Volume::holding(&[root().join(first), file.clone()]);
            assert_eq!(volume.resolve(&root().join(first).join("doc.pdf")), Some(file.to_string_lossy().into_owned()), "{first:?} delivered");
        }
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

    /// Trying other forms is bounded: past three times as many lookups as the path has
    /// components, the path counts as not found. Sixteen folders each held in both forms, with
    /// the file only under the last way through them, take 196,608 lookups to search
    /// exhaustively.
    #[test]
    fn the_walk_gives_up_after_three_lookups_a_component() {
        const DEPTH: usize = 16;
        let probes = Cell::new(0);
        // Every folder below the root exists in both forms; `doc.pdf` only below NFD ones.
        let exists = |path: &Path| {
            probes.set(probes.get() + 1);
            let names: Vec<_> = path.components().skip(1).map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            let (folders, file) = match names.split_last() {
                Some((last, folders)) if last == "doc.pdf" => (folders, true),
                _ => (names.as_slice(), false),
            };
            folders.len() <= DEPTH
                && folders.iter().all(|f| f == NFC || f == NFD)
                && (!file || (folders.len() == DEPTH && folders.iter().all(|f| f == NFD)))
        };
        let mut delivered = root();
        for _ in 0..DEPTH {
            delivered.push(NFC);
        }
        delivered.push("doc.pdf");
        let found = resolve_with(delivered.to_str().unwrap(), exists);
        let components = DEPTH + 2;
        assert!(probes.get() <= 3 * components + 1, "{} lookups for {components} components", probes.get());
        assert_eq!(found, None, "past the budget the path counts as not found");
    }

    /// The budget always covers the path the walk reaches without going back: a name whose
    /// delivered, NFC and NFD forms all differ, stored in the last of them at every level, takes
    /// all three lookups at each. With a root, the root's one lookup leaves two of its three
    /// unused, so only the relative path spends the whole budget and fails if it is cut by one;
    /// production paths are normally absolute, the relative one is here to pin the budget exactly.
    #[test]
    fn a_path_needing_every_form_of_each_name_is_within_the_budget() {
        let (delivered, nfd) = ("a\u{0301}\u{0323}", "a\u{0323}\u{0301}");
        let stored = root().join(nfd).join(nfd).join(nfd);
        let volume = Volume::holding(std::slice::from_ref(&stored));
        let found = volume.resolve(&root().join(delivered).join(delivered).join(delivered));
        assert_eq!(found, Some(stored.to_string_lossy().into_owned()));
        assert_eq!(volume.probes.get(), 1 + 1 + 3 * 3, "the path as delivered, the root, then three forms a name");
        let stored = Path::new(nfd).join(nfd).join(nfd);
        let volume = Volume::holding(std::slice::from_ref(&stored));
        let found = volume.resolve(&Path::new(delivered).join(delivered).join(delivered));
        assert_eq!(found, Some(stored.to_string_lossy().into_owned()), "a relative path");
        assert_eq!(volume.probes.get(), 1 + 3 * 3, "the path as delivered, then three forms a name");
    }
}
