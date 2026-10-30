//! Select tool: click a picture on the page to select it, then ⌘C / Ctrl+C (or the right-click
//! menu) copies it in its original format: a JPEG image as the JPEG file stored in the PDF, any
//! other image as a lossless PNG (with its transparency). The clipboard gets the file, as when a
//! file is copied in Finder or Explorer; Copy Image copies the pixels instead.

use egui::{Color32, CornerRadius, Pos2, Stroke};

use crate::PdfCraftApp;
use crate::canvas::{DocView, PageXform};
use pdfcraft_render::DocInfo;

/// The selected picture: page, index in `page_images` and its box in user space, and the
/// document's edit generation when it was picked (any later edit may have removed or renumbered it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickedImage {
    pub page: usize,
    pub index: usize,
    pub rect: [f64; 4],
    pub generation: u64,
}

const BLUE: Color32 = Color32::from_rgb(0x3A, 0x7B, 0xF0);

/// A click on page `page` (Select tool, not taken by anything else) selects the picture under
/// it, even with text drawn over it (scans with an OCR layer, captions); dragging or a double
/// click still selects text, and clears the picture.
pub(crate) fn page_input(
    resp: &egui::Response,
    xf: &PageXform,
    page: usize,
    info: &DocInfo,
    doc: &pdfcraft_engine::Document,
    pointer: Option<Pos2>,
    view: &mut DocView,
) {
    let Some(p) = pointer.filter(|p| xf.rect.contains(*p)) else { return };
    if resp.drag_started() || resp.double_clicked() || resp.triple_clicked() {
        view.picked_image = None;
        return;
    }
    if !resp.clicked() {
        return;
    }
    let Some(info_page) = info.pages.get(page) else { return };
    let (vx, vy) = xf.screen_to_view(p);
    let u = info_page.view_to_user(vx, vy);
    let at = [f64::from(u[0]), f64::from(u[1])];
    let generation = doc.edit_generation();
    view.picked_image =
        doc.page_image_at(page, at).and_then(|index| doc.page_images(page).get(index).map(|i| PickedImage { page, index, rect: i.rect, generation }));
}

/// Forget the selected picture once the document has been edited since it was picked: Edit text &
/// images (or Undo) may have deleted or moved it, and its highlight would stay behind on an empty
/// spot of the page.
pub(crate) fn drop_stale(doc: &pdfcraft_engine::Document, view: &mut DocView) {
    if view.picked_image.is_some_and(|p| p.generation != doc.edit_generation()) {
        view.picked_image = None;
    }
}

/// The selected picture's highlight on page `page`.
pub(crate) fn paint(painter: &egui::Painter, xf: &PageXform, page: usize, info: &DocInfo, view: &DocView) {
    let Some(picked) = view.picked_image.filter(|p| p.page == page) else { return };
    if info.pages.get(page).is_none() || !picked.rect.iter().all(|v| v.is_finite()) {
        return;
    }
    let r = xf.user_rect(info, page, picked.rect.map(|v| v as f32));
    painter.rect_filled(r, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(0x3A, 0x7B, 0xF0, 50));
    painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(2.0, BLUE), egui::StrokeKind::Outside);
}

/// A file name without characters file systems refuse.
fn safe_name(s: &str) -> String {
    let cleaned: String = s.chars().map(|c| if c.is_control() || r#"/\:*?"<>|"#.contains(c) { '_' } else { c }).take(80).collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() { "PdfCraft".into() } else { cleaned }
}

/// The per-user folder that holds the file of the last Copy Image in Original Format: `Clipboard`
/// in the portable data folder in portable mode ([`crate::portable`]), otherwise the platform's
/// per-user cache folder: `~/Library/Caches/PdfCraft/Clipboard` (macOS),
/// `%LOCALAPPDATA%\PdfCraft\Clipboard` (Windows), or `$XDG_CACHE_HOME/pdfcraft/clipboard` /
/// `~/.cache/pdfcraft/clipboard` (others). Never the shared temporary folder, where another user
/// could plant a link or a folder of their own.
#[cfg(not(target_arch = "wasm32"))]
fn clipboard_dir() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    if let Some(dir) = crate::portable::data_dir() {
        return Some(dir.join("Clipboard"));
    }
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from).filter(|p| p.is_absolute());
    if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Caches/PdfCraft/Clipboard"))
    } else if cfg!(windows) {
        env("LOCALAPPDATA").map(|d| d.join("PdfCraft").join("Clipboard"))
    } else {
        env("XDG_CACHE_HOME").or_else(|| env("HOME").map(|h| h.join(".cache"))).map(|d| d.join("pdfcraft/clipboard"))
    }
}

/// Whether `name` is a file Copy Image in Original Format writes: `<stem> p<page> img<index>.<ext>`
/// with a JPEG or PNG extension. Only such files are ever removed from the clipboard folder.
#[cfg(not(target_arch = "wasm32"))]
fn is_copied_image_name(name: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let Some((rest, ext)) = name.rsplit_once('.') else { return false };
    if !matches!(ext, "jpg" | "png") {
        return false;
    }
    let Some((rest, index)) = rest.rsplit_once(" img") else { return false };
    let Some((stem, page)) = rest.rsplit_once(" p") else { return false };
    !stem.is_empty() && digits(index) && digits(page)
}

/// Whether `files` (the clipboard's file list) is the one file `name` Copy Image in Original
/// Format wrote into `dir` with `bytes`, and not files copied since in another application or in
/// another PdfCraft window (whose copy replaces ours in the same folder, under the same name when
/// it is the same picture position of a document with the same name). Read once per ⌘V.
#[cfg(not(target_arch = "wasm32"))]
fn is_own_copy(files: &[std::path::PathBuf], dir: &std::path::Path, name: &str, bytes: &[u8]) -> bool {
    let [file] = files else { return false };
    // arboard puts the resolved path on the clipboard (links, and on Windows junctions and case),
    // so the folders are compared resolved, or as they are when either can't be resolved.
    let same_dir = match (file.parent().and_then(|p| p.canonicalize().ok()), dir.canonicalize().ok()) {
        (Some(a), Some(b)) => a == b,
        _ => file.parent() == Some(dir),
    };
    let same_content = || std::fs::metadata(file).is_ok_and(|m| m.len() == bytes.len() as u64) && std::fs::read(file).is_ok_and(|b| b == bytes);
    is_copied_image_name(name) && same_dir && file.file_name().and_then(|n| n.to_str()) == Some(name) && same_content()
}

/// Makes `dir` (a real folder, not a link, owned by us and private to us on Unix), removes the
/// earlier copies PdfCraft wrote there, and writes `bytes` as a new file `name`. Links are never
/// followed and nothing else in the folder is touched.
#[cfg(not(target_arch = "wasm32"))]
fn write_clipboard_file(dir: &std::path::Path, name: &str, bytes: &[u8]) -> Result<std::path::PathBuf, String> {
    use std::io::Write;
    if !is_copied_image_name(name) || name.contains(['/', '\\']) {
        return Err("the file name is not valid".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let meta = std::fs::symlink_metadata(dir).map_err(|e| e.to_string())?;
    if !meta.is_dir() {
        return Err(format!("{} is not a folder", dir.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Only the owner may change a folder's permissions, so this also refuses a folder that
        // belongs to someone else.
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    // Only the latest copy needs to exist; older ones are removed (best effort). Only regular
    // files with the copy's name pattern: links and anything else are left alone.
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let ours = e.file_name().to_str().is_some_and(is_copied_image_name);
            if ours && e.file_type().is_ok_and(|t| t.is_file()) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let path = dir.join(name);
    // `create_new` fails on anything already there, a link included, instead of following it.
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| e.to_string())?;
    if let Err(e) = file.write_all(bytes) {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err(e.to_string());
    }
    Ok(path)
}

impl PdfCraftApp {
    /// ⌘C / Ctrl+C copies the selected picture in its original format; Escape deselects it.
    pub(crate) fn image_pick_keys(&mut self, i: usize, ctx: &egui::Context) {
        let Some(picked) = self.views.get(i).and_then(|v| v.picked_image) else { return };
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if ctx.input(|inp| inp.key_pressed(egui::Key::Escape)) {
            self.views[i].picked_image = None;
            return;
        }
        let copy = ctx.input(|inp| inp.events.iter().any(|e| matches!(e, egui::Event::Copy)));
        if !copy || self.views[i].selected_text().is_some() {
            return;
        }
        // The document may have changed since the click: the same picture must still be there.
        let current = self.session.get(self.views[i].id).and_then(|d| d.page_images(picked.page).get(picked.index).map(|im| im.rect));
        if current == Some(picked.rect) {
            self.copy_page_image_file(picked.page, picked.index);
        } else {
            self.views[i].picked_image = None;
            self.notify_tr("The page has changed; select the image again");
        }
    }

    /// Edit text & images: ⌘C / Ctrl+C copies the selected image in its original format. ⌘V
    /// pastes it through [`Self::paste_image_key`], the one paste shortcut on the page.
    pub(crate) fn image_edit_keys(&mut self, i: usize, ctx: &egui::Context) {
        let Some(view) = self.views.get(i) else { return };
        if self.quick_tool != crate::QuickTool::EditText || view.organize || self.modal_open() || ctx.egui_wants_keyboard_input() {
            return;
        }
        let copy = ctx.input(|inp| inp.events.iter().any(|e| matches!(e, egui::Event::Copy)));
        if copy && let Some((page, index)) = view.image_selection.as_ref().map(|s| (s.page, s.index)) {
            self.copy_page_image_file(page, index);
        }
    }

    /// Copy Image in Original Format: image `index` on `page` as a file on the clipboard (JPEG
    /// as stored in the PDF, otherwise PNG).
    pub(crate) fn copy_page_image_file(&mut self, page: usize, index: usize) {
        // A newer copy wins over a whole-document copy still reading its pages.
        self.document_copy = None;
        let Some((_, id)) = self.active_ids() else { return };
        if !self.may_copy(id) {
            return;
        }
        let Some(doc) = self.session.get(id) else { return };
        let stem = safe_name(doc.name.rsplit_once('.').map_or(doc.name.as_str(), |(s, _)| s));
        let file = doc.page_image_copy_file(page, index);
        let copied = file.and_then(|(ext, bytes)| {
            let name = format!("{stem} p{} img{}.{ext}", page.saturating_add(1), index.saturating_add(1));
            self.put_file_on_clipboard(&name, &bytes)?;
            self.copied_image = Some((name.clone(), std::sync::Arc::new(bytes.clone())));
            self.last_copied_file = Some((name, bytes));
            Ok(ext)
        });
        match copied {
            Ok(ext) => self.notify_fmt("The image has been copied as a {ext} file", &[("ext", &ext.to_uppercase())]),
            Err(e) => self.notify_fmt("Couldn't copy the image: {e}", &[("e", &e)]),
        }
    }

    /// Writes `bytes` to PdfCraft's private clipboard folder ([`clipboard_dir`]), replacing the
    /// previous copy, and puts that file on the system clipboard.
    #[cfg(not(target_arch = "wasm32"))]
    fn put_file_on_clipboard(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        if !self.system_clipboard {
            return Ok(());
        }
        let dir = clipboard_dir().ok_or("there is no folder for the copied file")?;
        let path = write_clipboard_file(&dir, name, bytes)?;
        arboard::Clipboard::new().and_then(|mut c| c.set().file_list(&[path])).map_err(|e| e.to_string())
    }

    /// Whether the system clipboard still holds the file the last Copy Image in Original Format
    /// put there, so ⌘V may paste PdfCraft's own copy. Without the system clipboard (tests) that
    /// copy is the clipboard's.
    pub(crate) fn clipboard_holds_copied_image(&self) -> bool {
        let files = match self.clipboard_files_override.clone() {
            Some(files) => files,
            None if !self.system_clipboard => return true,
            #[cfg(not(target_arch = "wasm32"))]
            None => match arboard::Clipboard::new().and_then(|mut c| c.get().file_list()) {
                Ok(files) => files,
                Err(_) => return false,
            },
            #[cfg(target_arch = "wasm32")]
            None => return false,
        };
        #[cfg(not(target_arch = "wasm32"))]
        let own = match (&self.copied_image, self.clipboard_folder_override.clone().or_else(clipboard_dir)) {
            (Some((name, bytes)), Some(dir)) => is_own_copy(&files, &dir, name, bytes),
            _ => false,
        };
        #[cfg(target_arch = "wasm32")]
        let own = {
            let _ = files;
            false
        };
        own
    }

    #[cfg(target_arch = "wasm32")]
    fn put_file_on_clipboard(&self, _name: &str, _bytes: &[u8]) -> Result<(), String> {
        if self.system_clipboard {
            return Err("the web version can't put files on the clipboard yet; use Save Image As".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::safe_name;
    #[cfg(not(target_arch = "wasm32"))]
    use super::{is_copied_image_name, is_own_copy, write_clipboard_file};

    #[test]
    fn file_names_are_safe() {
        assert_eq!(safe_name("a/b:c"), "a_b_c");
        assert_eq!(safe_name("  ..  "), "PdfCraft");
        assert_eq!(safe_name("Relazione 2026"), "Relazione 2026");
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn copied_image_names_are_recognised() {
        assert!(is_copied_image_name("Report p1 img2.jpg"));
        assert!(is_copied_image_name("a p12 img3.png"));
        assert!(!is_copied_image_name("notes.txt"));
        assert!(!is_copied_image_name("Report p1 img2.pdf"));
        assert!(!is_copied_image_name(" p1 img2.png"));
        assert!(!is_copied_image_name("Report px img2.png"));
    }

    /// arboard puts the resolved path on the clipboard, so a clipboard folder reached through a
    /// link (a home folder behind one, `/tmp` on macOS) still matches its copy.
    #[test]
    #[cfg(unix)]
    fn the_copy_is_recognised_through_a_linked_clipboard_folder() {
        let root = std::env::temp_dir().join(format!("pdfcraft-clipboard-link-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let real = root.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let name = "Report p1 img2.jpg";
        std::fs::write(real.join(name), b"x").unwrap();
        let on_clipboard = real.canonicalize().unwrap().join(name);
        assert!(is_own_copy(&[on_clipboard], &link, name, b"x"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn only_the_file_copy_image_wrote_counts_as_its_copy() {
        let root = std::env::temp_dir().join(format!("pdfcraft-clipboard-own-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (dir, elsewhere) = (root.join("Clipboard"), root.join("elsewhere"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        let (name, bytes) = ("Report p1 img2.jpg", b"ours".as_slice());
        for (path, content) in
            [(dir.join(name), bytes), (elsewhere.join(name), bytes), (dir.join("Other p1 img1.png"), bytes), (dir.join("notes.txt"), bytes)]
        {
            std::fs::write(path, content).unwrap();
        }
        assert!(is_own_copy(&[dir.join(name)], &dir, name, bytes));
        assert!(!is_own_copy(&[], &dir, name, bytes), "nothing on the clipboard");
        assert!(!is_own_copy(&[elsewhere.join(name)], &dir, name, bytes), "another folder");
        assert!(!is_own_copy(&[dir.join("Other p1 img1.png")], &dir, name, bytes), "another window's copy");
        assert!(!is_own_copy(&[dir.join("notes.txt")], &dir, "notes.txt", bytes), "not a name Copy Image writes");
        assert!(!is_own_copy(&[dir.join(name), dir.join("Report p1 img3.jpg")], &dir, name, bytes), "more than one file");
        // Another window copied the same picture position of a document with the same name.
        assert!(!is_own_copy(&[dir.join(name)], &dir, name, b"them"), "same name, another picture");
        assert!(!is_own_copy(&[dir.join(name)], &dir, name, b"ours, longer"), "same name, another length");
        assert!(!is_own_copy(&[dir.join("Gone p1 img1.png")], &dir, "Gone p1 img1.png", bytes), "a file that can't be read");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn clipboard_folder_only_replaces_our_own_copies() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-clipboard-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("keep.txt"), b"other").unwrap();
        std::fs::write(dir.join("Old p1 img1.png"), b"old").unwrap();
        let path = write_clipboard_file(&dir, "New p2 img1.jpg", b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert!(dir.join("keep.txt").exists(), "files PdfCraft didn't write are kept");
        assert!(!dir.join("Old p1 img1.png").exists(), "the previous copy is removed");
        assert!(write_clipboard_file(&dir, "../escape p1 img1.png", b"x").is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
            // A link with a copy's name is neither followed nor removed.
            let target = dir.join("keep.txt");
            let link = dir.join("Link p1 img1.png");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            write_clipboard_file(&dir, "Again p1 img1.png", b"again").unwrap();
            assert!(std::fs::symlink_metadata(&link).is_ok(), "links are left alone");
            assert_eq!(std::fs::read(&target).unwrap(), b"other");
            assert!(write_clipboard_file(&dir, "Link p1 img1.png", b"x").is_err(), "writing never follows a link");
            assert_eq!(std::fs::read(&target).unwrap(), b"other");
            // A link in place of the folder is refused.
            let linked = std::env::temp_dir().join(format!("pdfcraft-clipboard-test-link-{}", std::process::id()));
            let _ = std::fs::remove_file(&linked);
            std::os::unix::fs::symlink(&dir, &linked).unwrap();
            assert!(write_clipboard_file(&linked, "X p1 img1.png", b"x").is_err());
            let _ = std::fs::remove_file(&linked);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
