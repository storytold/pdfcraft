//! Dragging pages out of the window: a page drag from the Pages panel or the Organize grid that
//! leaves the window offers the dragged pages as a new PDF, which a folder, the desktop or
//! another program takes as a file, as Extract Pages and Save would. Windows only for now;
//! elsewhere the drag stays inside the window as before.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The folder, under the system's temporary folder, holding the files that drags offer.
const FOLDER: &str = "PdfCraft drags";
/// Files offered by earlier drags are removed once this old: the place they were dropped has
/// its own copy by then.
const KEEP: Duration = Duration::from_secs(60 * 60);
/// The empty file a drag's folder holds beside its PDF, which marks the folder as a drag's.
const MARK: &str = ".pdfcraft-drag";
/// The longest path the Windows shell takes for a dragged file, in UTF-16 units without the
/// terminating null (`MAX_PATH` less one).
const MAX_PATH_UNITS: usize = 259;

/// Whether the pointer, still holding the primary button, has left the window.
pub(crate) fn left_window(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.pointer.primary_down() && i.pointer.latest_pos().is_none_or(|p| !i.viewport_rect().contains(p)))
}

/// The offered file's name: the document's, with the pages (sorted, 0-based) as Extract Pages
/// names a single page: "Plan (page 3).pdf", "Plan (pages 3-5).pdf", "Plan (4 pages).pdf".
/// The document's part is shortened to keep the name within `limit` UTF-16 units; `None` when
/// not even one character of it fits.
pub(crate) fn file_name(document: &str, pages: &[usize], limit: usize) -> Option<String> {
    let stem = super::files::file_stem_from_user(document);
    let mut stem = if stem.is_empty() { "Pages".to_string() } else { stem };
    let what = match pages {
        [page] => format!("page {}", page.saturating_add(1)),
        [first, .., last] if pages.windows(2).all(|w| w[0].checked_add(1) == Some(w[1])) => {
            format!("pages {}-{}", first.saturating_add(1), last.saturating_add(1))
        }
        _ => format!("{} pages", pages.len()),
    };
    let rest = format!(" ({what}).pdf");
    let units = |s: &str| s.encode_utf16().count();
    while !stem.is_empty() && units(&stem).saturating_add(units(&rest)) > limit {
        stem.pop();
        let kept = stem.trim_end_matches([' ', '.']).len();
        stem.truncate(kept);
    }
    (!stem.is_empty()).then(|| format!("{stem}{rest}"))
}

/// Write `bytes` in a folder of its own under `root`, named as [`file_name`] names the `pages`
/// of `document` within the shell's path limit, and marked as a drag's with [`MARK`]. First
/// removes the folders of drags older than [`KEEP`].
pub(crate) fn write_offered(root: &Path, document: &str, pages: &[usize], bytes: &[u8]) -> std::io::Result<PathBuf> {
    remove_old(root, SystemTime::now());
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = root.join(format!("{}-{stamp}", std::process::id()));
    // The folder, a separator, then the name.
    let limit = MAX_PATH_UNITS.saturating_sub(wide_len(&dir).saturating_add(1));
    let name =
        file_name(document, pages, limit).ok_or_else(|| std::io::Error::other("the temporary folder's path is too long for a dragged file"))?;
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(MARK), b"")?;
    let path = dir.join(name);
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// A path's length in UTF-16 units, as the Windows shell counts it.
fn wide_len(path: &Path) -> usize {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str().encode_wide().count()
    }
    #[cfg(not(windows))]
    {
        path.to_string_lossy().encode_utf16().count()
    }
}

/// Remove the folders under `root` that drags made (named `<process>-<time>` and holding
/// [`MARK`], see [`write_offered`]) and last changed more than [`KEEP`] before `now`. A `root`
/// that is a link or junction (to somewhere a drag didn't make) is left alone, and so is
/// anything in it that isn't a drag's own folder, whatever its name.
fn remove_old(root: &Path, now: SystemTime) {
    let real_folder = |p: &Path| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink());
    if !real_folder(root) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let marked = std::fs::symlink_metadata(path.join(MARK)).is_ok_and(|m| m.is_file());
        if !is_drag_folder_name(&entry.file_name().to_string_lossy()) || !real_folder(&path) || !marked {
            continue;
        }
        let changed = entry.metadata().and_then(|m| m.modified()).ok();
        if changed.and_then(|t| now.duration_since(t).ok()).is_some_and(|age| age > KEEP) {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

/// Whether `name` is a drag's folder name: digits, a hyphen, digits.
fn is_drag_folder_name(name: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    name.split_once('-').is_some_and(|(process, time)| digits(process) && digits(time))
}

/// The folder drags offer their files from.
pub(crate) fn offered_root() -> PathBuf {
    std::env::temp_dir().join(FOLDER)
}

/// Hand `path` to the system's drag loop, which returns once it is dropped or the drag is
/// cancelled.
#[cfg(windows)]
fn start_system_drag(window: &eframe::Frame, path: PathBuf) -> Result<(), String> {
    drag::start_drag(window, drag::DragItem::Files(vec![path]), drag::Image::Raw(Vec::new()), |_, _| {}, drag::Options::default())
        .map_err(|e| e.to_string())
}

#[cfg(windows)]
impl crate::PdfCraftApp {
    /// A page drag that has left the window becomes a drag of the dragged pages as a file. The
    /// page drag ends here, so nothing moves in the document.
    pub(crate) fn drag_pages_out(&mut self, ctx: &egui::Context, window: &eframe::Frame) {
        let Some((i, id)) = self.active_ids() else { return };
        let Some(view) = self.views.get_mut(i) else { return };
        let Some(mut pages) = view.panel_drag.clone().or_else(|| view.org_drag.clone()) else { return };
        if !left_window(ctx) {
            return;
        }
        view.panel_drag = None;
        view.org_drag = None;
        // The system's drag loop keeps the button's release from egui.
        crate::platform::window_controls::release_after_system_drag(ctx);
        // What's typed in a form field is part of the document, as for Extract Pages (#166).
        // Typing the field won't take offers nothing.
        if !self.commit_form_typing() {
            return;
        }
        pages.sort_unstable();
        pages.dedup();
        let document = self.session.get(id).map(|d| super::files::strip_pdf(&d.name).to_string()).unwrap_or_default();
        let root = self.drag_root_override.clone().unwrap_or_else(offered_root);
        let offered = self
            .session
            .extract(id, &pages)
            .map_err(|e| e.to_string())
            .and_then(|bytes| write_offered(&root, &document, &pages, &bytes).map_err(|e| e.to_string()));
        let path = match offered {
            Ok(path) => path,
            Err(e) => {
                self.notify_fmt("Couldn't extract pages: {e}", &[("e", &e)]);
                return;
            }
        };
        self.dragged_out = Some(path.clone());
        if let Err(e) = start_system_drag(window, path) {
            // The file is written by now: what failed is the system's drag.
            self.notify_fmt("Couldn't drag the pages out: {e}", &[("e", &e)]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_say_which_pages_the_file_holds() {
        let name = |document: &str, pages: &[usize]| file_name(document, pages, usize::MAX).unwrap();
        assert_eq!(name("Plan", &[2]), "Plan (page 3).pdf");
        assert_eq!(name("Plan", &[2, 3, 4]), "Plan (pages 3-5).pdf");
        assert_eq!(name("Plan", &[0, 2, 3, 9]), "Plan (4 pages).pdf");
        assert_eq!(name("A/B: C?", &[0]), "A_B_ C_ (page 1).pdf");
        assert_eq!(name("", &[0, 1]), "Pages (pages 1-2).pdf");
    }

    #[test]
    fn names_are_shortened_to_fit_the_shells_path_limit() {
        // " (page 3).pdf" is 13 units.
        assert_eq!(file_name("Site plan", &[2], 18).as_deref(), Some("Site (page 3).pdf"), "a trailing space goes too");
        assert_eq!(file_name("Plan", &[2], 14).as_deref(), Some("P (page 3).pdf"));
        assert_eq!(file_name("Plan", &[2], 13), None, "not even one character fits");
        // The clef is two UTF-16 units.
        assert_eq!(file_name("a\u{1D11E}", &[0], 15).as_deref(), Some("a (page 1).pdf"));
        let root = std::env::temp_dir().join(format!("pdfcraft-drag-test-long-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let path = write_offered(&root, &"x".repeat(240), &[0], b"%PDF").unwrap();
        assert!(wide_len(&path) <= MAX_PATH_UNITS, "{}", path.display());
        assert!(path.file_name().unwrap().to_string_lossy().ends_with("x (page 1).pdf"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn old_offered_files_are_removed_and_new_ones_kept() {
        let root = std::env::temp_dir().join(format!("pdfcraft-drag-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let first = write_offered(&root, "a", &[0], b"%PDF").unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), b"%PDF");
        let dir = first.parent().unwrap().to_path_buf();
        remove_old(&root, SystemTime::now() + KEEP / 2);
        assert!(dir.exists(), "a drag's file is kept while it may still be copied");
        remove_old(&root, SystemTime::now() + KEEP * 2);
        assert!(!dir.exists(), "an old drag's folder is removed");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cleanup_leaves_what_drags_did_not_make() {
        let root = std::env::temp_dir().join(format!("pdfcraft-drag-test-others-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mine = write_offered(&root, "a", &[0], b"%PDF").unwrap().parent().unwrap().to_path_buf();
        let other = root.join("photos");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(root.join("notes.txt"), b"keep").unwrap();
        // Named as a drag names its folder, but not marked as one.
        let lookalike = root.join("1234-5678");
        std::fs::create_dir_all(&lookalike).unwrap();
        remove_old(&root, SystemTime::now() + KEEP * 2);
        assert!(!mine.exists(), "a drag's old folder goes");
        assert!(other.exists() && root.join("notes.txt").exists(), "what drags didn't make stays");
        assert!(lookalike.exists(), "a folder only named like a drag's stays");
        assert!(is_drag_folder_name("1234-5678") && !is_drag_folder_name("12-") && !is_drag_folder_name("a-1"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    fn cleanup_through_a_junction_removes_nothing() {
        let base = std::env::temp_dir().join(format!("pdfcraft-drag-test-junction-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let target = base.join("target");
        let drag = write_offered(&target, "a", &[0], b"%PDF").unwrap().parent().unwrap().to_path_buf();
        let junction = base.join("junction");
        let made =
            std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&junction).arg(&target).output().is_ok_and(|o| o.status.success());
        assert!(made, "mklink /J made the junction");
        remove_old(&junction, SystemTime::now() + KEEP * 2);
        assert!(drag.exists(), "a drag's folder reached through a junction stays");
        let _ = std::fs::remove_dir(&junction);
        let _ = std::fs::remove_dir_all(&base);
    }
}
