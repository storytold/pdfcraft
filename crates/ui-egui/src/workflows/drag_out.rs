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

/// Whether the pointer, still holding the primary button, has left the window.
pub(crate) fn left_window(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.pointer.primary_down() && i.pointer.latest_pos().is_none_or(|p| !i.viewport_rect().contains(p)))
}

/// The offered file's name: the document's, with the pages (sorted, 0-based) as Extract Pages
/// names a single page: "Plan (page 3).pdf", "Plan (pages 3-5).pdf", "Plan (4 pages).pdf".
pub(crate) fn file_name(document: &str, pages: &[usize]) -> String {
    let stem = super::files::file_stem_from_user(document);
    let stem = if stem.is_empty() { "Pages".to_string() } else { stem };
    let what = match pages {
        [page] => format!("page {}", page.saturating_add(1)),
        [first, .., last] if pages.windows(2).all(|w| w[0].checked_add(1) == Some(w[1])) => {
            format!("pages {}-{}", first.saturating_add(1), last.saturating_add(1))
        }
        _ => format!("{} pages", pages.len()),
    };
    format!("{stem} ({what}).pdf")
}

/// Write `bytes` as `name` in a folder of its own under [`FOLDER`], first removing the folders
/// of drags older than [`KEEP`].
pub(crate) fn write_offered(root: &Path, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    remove_old(root, SystemTime::now());
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = root.join(format!("{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(name);
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// Remove the folders under `root` last changed more than [`KEEP`] before `now`.
fn remove_old(root: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let changed = entry.metadata().and_then(|m| m.modified()).ok();
        if changed.and_then(|t| now.duration_since(t).ok()).is_some_and(|age| age > KEEP) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
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
        pages.sort_unstable();
        pages.dedup();
        let document = self.session.get(id).map(|d| super::files::strip_pdf(&d.name).to_string()).unwrap_or_default();
        let offered = self
            .session
            .extract(id, &pages)
            .map_err(|e| e.to_string())
            .and_then(|bytes| write_offered(&offered_root(), &file_name(&document, &pages), &bytes).map_err(|e| e.to_string()));
        let path = match offered {
            Ok(path) => path,
            Err(e) => {
                self.notify_fmt("Couldn't extract pages: {e}", &[("e", &e)]);
                return;
            }
        };
        self.dragged_out = Some(path.clone());
        if let Err(e) = start_system_drag(window, path) {
            self.notify_fmt("Couldn't extract pages: {e}", &[("e", &e)]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_say_which_pages_the_file_holds() {
        assert_eq!(file_name("Plan", &[2]), "Plan (page 3).pdf");
        assert_eq!(file_name("Plan", &[2, 3, 4]), "Plan (pages 3-5).pdf");
        assert_eq!(file_name("Plan", &[0, 2, 3, 9]), "Plan (4 pages).pdf");
        assert_eq!(file_name("A/B: C?", &[0]), "A_B_ C_ (page 1).pdf");
        assert_eq!(file_name("", &[0, 1]), "Pages (pages 1-2).pdf");
    }

    #[test]
    fn old_offered_files_are_removed_and_new_ones_kept() {
        let root = std::env::temp_dir().join(format!("pdfcraft-drag-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let first = write_offered(&root, "a.pdf", b"%PDF").unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), b"%PDF");
        let dir = first.parent().unwrap().to_path_buf();
        remove_old(&root, SystemTime::now() + KEEP / 2);
        assert!(dir.exists(), "a drag's file is kept while it may still be copied");
        remove_old(&root, SystemTime::now() + KEEP * 2);
        assert!(!dir.exists(), "an old drag's folder is removed");
        let _ = std::fs::remove_dir_all(&root);
    }
}
