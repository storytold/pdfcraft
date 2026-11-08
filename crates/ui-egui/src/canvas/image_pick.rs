//! Select tool: click a picture on the page to select it, then ⌘C / Ctrl+C (or the right-click
//! menu) copies it in its original format: a JPEG image as the JPEG file stored in the PDF, any
//! other image as a lossless PNG (with its transparency). The clipboard gets the file, as when a
//! file is copied in Finder or Explorer; Copy Image copies the pixels instead.

use egui::{Color32, CornerRadius, Stroke};

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
/// it, even with text drawn over it (scans with an OCR layer, captions), with `pick` (a press
/// nobody holds: a link's click follows the link only); dragging or a double click still selects
/// text, and clears the picture.
#[allow(clippy::too_many_arguments)]
pub(crate) fn page_input(
    resp: &egui::Response,
    xf: &PageXform,
    page: usize,
    info: &DocInfo,
    doc: &pdfcraft_engine::Document,
    ptr: &crate::page_pointer::PagePointer,
    pick: bool,
    view: &mut DocView,
) {
    let Some(info_page) = info.pages.get(page) else { return };
    let generation = doc.edit_generation();
    if let Some(p) = ptr.pos.filter(|p| xf.rect.contains(*p)) {
        if ptr.drag_started || resp.double_clicked() || resp.triple_clicked() {
            view.picked_image = None;
            view.pending_pick = None;
            return;
        }
        if resp.clicked() {
            // A link's click picks nothing, and takes back an earlier click still being looked into.
            view.pending_pick = None;
            if pick {
                let (vx, vy) = xf.screen_to_view(p);
                let u = info_page.view_to_user(vx, vy);
                view.picked_image = None;
                view.pending_pick = Some((page, [f64::from(u[0]), f64::from(u[1])], generation));
            }
        }
    }
    // The click's picture, as soon as the page's pictures are known: the window stays live until then.
    let Some((_, at, clicked_in)) = view.pending_pick.filter(|pending| pending.0 == page) else { return };
    if clicked_in != generation {
        view.pending_pick = None;
    } else if let Some(pictures) = view.pictures.get(&resp.ctx, doc, page) {
        // Where the click was on the page now: the view may have moved while the page was read.
        let point = xf.user_rect_f64(info, page, [at[0], at[1], at[0], at[1]]).center();
        view.picked_image = pdfcraft_engine::picture_at(pictures, at).and_then(|(index, i)| {
            let screen_rect = crate::canvas::image_screen_rect(i, xf, info, page);
            (screen_rect.is_positive() && screen_rect.contains(point)).then(|| PickedImage {
                page,
                index: *index,
                rect: crate::canvas::image_selectable_bounds(i),
                generation,
            })
        });
        view.pending_pick = None;
    }
}

/// Page and document generation.
type PictureKey = (usize, u64);

/// The pictures of the page last asked about, found on their own thread: the search reads every
/// drawing operator on the page, seconds on a dense one. A small file answers on the spot, so its
/// menu and selection don't flicker. The web build has no threads, so there every search runs on
/// the spot, as before.
///
/// One search at a time: a page left while it is read is read to the end before the next one
/// starts (the read can't be stopped part-way).
#[derive(Default)]
pub(crate) struct PagePictures {
    /// The pictures of the key's page.
    ready: Option<(PictureKey, Vec<pdfcraft_engine::PagePicture>)>,
    /// The search under way. One at a time: each is a full read of a page.
    job: Option<(PictureKey, std::sync::mpsc::Receiver<Vec<pdfcraft_engine::PagePicture>>)>,
}

impl PagePictures {
    /// A file smaller than this has its pictures read on the spot.
    const SMALL_FILE: usize = 8 * 1024 * 1024;

    /// The pictures on `page`, or `None` while they are being found (a repaint follows the answer).
    pub(crate) fn get(&mut self, ctx: &egui::Context, doc: &pdfcraft_engine::Document, page: usize) -> Option<&[pdfcraft_engine::PagePicture]> {
        let key = (page, doc.edit_generation());
        self.poll();
        if self.job.is_none() && !self.ready.as_ref().is_some_and(|(k, _)| *k == key) {
            match doc.page_pictures_job(page) {
                Some(work) if doc.bytes.len() < Self::SMALL_FILE => self.ready = Some((key, work())),
                Some(work) => self.start(ctx, key, work),
                None => self.ready = Some((key, Vec::new())),
            }
            self.poll();
        }
        self.ready.as_ref().filter(|(k, _)| *k == key).map(|(_, pictures)| pictures.as_slice())
    }

    fn start(&mut self, ctx: &egui::Context, key: PictureKey, work: impl FnOnce() -> Vec<pdfcraft_engine::PagePicture> + Send + 'static) {
        let (send, answer) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        let run = move || {
            let _ = send.send(work());
            ctx.request_repaint();
        };
        self.job = Some((key, answer));
        #[cfg(target_arch = "wasm32")]
        run();
        #[cfg(not(target_arch = "wasm32"))]
        if let Err(e) = std::thread::Builder::new().name("page-pictures".into()).spawn(run) {
            log::warn!("page pictures thread: {e}");
            self.job = None;
            self.ready = Some((key, Vec::new()));
        }
    }

    /// Take the answer if it has come.
    fn poll(&mut self) {
        use std::sync::mpsc::TryRecvError;
        let Some((key, answer)) = &self.job else { return };
        let got = match answer.try_recv() {
            Ok(pictures) => Some(pictures),
            Err(TryRecvError::Empty) => None,
            // The search failed: this page offers no pictures.
            Err(TryRecvError::Disconnected) => Some(Vec::new()),
        };
        if let Some(pictures) = got {
            self.ready = Some((*key, pictures));
            self.job = None;
        }
    }
}

#[cfg(test)]
mod picture_tests {
    use super::*;

    /// A search that takes its time doesn't hold the window: starting and polling it return before
    /// the search is let go, and the answer is picked up when it comes.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn a_slow_page_does_not_hold_up_the_window() {
        let (release, wait) = std::sync::mpsc::channel::<()>();
        // Lets the search go in the end, should it run on this thread or be waited for.
        let late = release.clone();
        let let_go = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watchdog = let_go.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(2));
            watchdog.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = late.send(());
        });
        let mut pictures = PagePictures::default();
        let caller = std::thread::current().id();
        let (ran_on, ran) = std::sync::mpsc::channel();
        pictures.start(&egui::Context::default(), (3, 1), move || {
            let _ = ran_on.send(std::thread::current().id());
            let _ = wait.recv();
            Vec::new()
        });
        pictures.poll();
        assert!(!let_go.load(std::sync::atomic::Ordering::SeqCst), "starting and polling the search must not wait for it");
        // A search that never ran (its thread couldn't start) fails the test rather than hanging it.
        let ran_on = ran.recv_timeout(std::time::Duration::from_secs(10)).expect("the search ran");
        assert_ne!(ran_on, caller, "the search must not run on the calling thread");
        let _ = release.send(());
        for _ in 0..500 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            pictures.poll();
            if pictures.ready.is_some() {
                break;
            }
        }
        assert_eq!(pictures.ready, Some(((3, 1), Vec::new())));
    }

    /// Escape takes back a click whose picture isn't known yet: it isn't selected when it is.
    #[test]
    fn escape_cancels_a_click_on_a_page_still_being_read() {
        let mut app = PdfCraftApp::new();
        app.open_bytes("notes.txt", None, b"hello".to_vec()).unwrap();
        let (i, _) = app.active_ids().unwrap();
        app.views[i].pending_pick = Some((0, [1.0, 1.0], 0));
        let escape = egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() };
        egui::Context::default()
            .run_ui(egui::RawInput { events: vec![escape], ..Default::default() }, |ui| app.image_pick_keys(i, ui.ctx()))
            .drop_without_applying_deltas();
        assert_eq!(app.views[i].pending_pick, None);
    }

    /// A page (200 × 300) with a square comment over 20..80 × 20..80 and a link over 120..180 ×
    /// 220..280.
    fn page_with_a_comment_and_a_link() -> Vec<u8> {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
            "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
            "<< /Type /Annot /Subtype /Square /Rect [20 20 80 80] /C [1 0 0] /Border [0 0 2] >>",
            "<< /Type /Annot /Subtype /Link /Rect [120 220 180 280] /Border [0 0 0] /A << /S /URI /URI (https://example.org/) >> >>",
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    /// The page open with the Select tool, once it is on screen, and where a point on it is.
    fn open_page() -> (egui_kittest::Harness<'static, PdfCraftApp>, impl Fn(f32, f32) -> egui::Pos2) {
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
            let mut app = PdfCraftApp::new();
            app.open_bytes("marks.pdf", None, page_with_a_comment_and_a_link()).unwrap();
            app
        });
        let mut on_screen = None;
        for _ in 0..600 {
            h.run_steps(2);
            on_screen = h.state().views[0].page_screen_rect(0);
            if on_screen.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let r = on_screen.expect("the page is on screen");
        let k = r.width() / 200.0;
        (h, move |x, y| r.min + egui::vec2(x * k, (300.0 - y) * k))
    }

    /// An earlier click on the page whose pictures are still being found, as on a dense drawing:
    /// they don't come while the returned sender lives.
    fn click_still_being_looked_into(
        h: &mut egui_kittest::Harness<'static, PdfCraftApp>,
    ) -> std::sync::mpsc::Sender<Vec<pdfcraft_engine::PagePicture>> {
        let id = h.state().views[0].id;
        let generation = h.state().session.get(id).expect("the document").edit_generation();
        let (send, answer) = std::sync::mpsc::channel();
        let view = &mut h.state_mut().views[0];
        view.pictures = PagePictures { ready: None, job: Some(((0, generation), answer)) };
        view.pending_pick = Some((0, [150.0, 150.0], generation));
        send
    }

    fn click(h: &mut egui_kittest::Harness<'static, PdfCraftApp>, p: egui::Pos2) {
        h.hover_at(p);
        h.run_steps(1);
        h.drag_at(p);
        h.run_steps(1);
        h.drop_at(p);
        h.run_steps(4);
    }

    /// Another tool lets go of the click, even while its page isn't drawn (Organize pages).
    #[test]
    fn another_tool_takes_back_a_click_on_a_page_not_drawn() {
        let (mut h, _) = open_page();
        let _keep = click_still_being_looked_into(&mut h);
        assert!(h.state_mut().execute("page.organize"));
        h.run_steps(2);
        assert!(h.state().views[0].pending_pick.is_some(), "the click waits while the page isn't drawn");
        h.state_mut().quick_tool = crate::QuickTool::Hand;
        h.run_steps(1);
        h.state_mut().quick_tool = crate::QuickTool::Select;
        h.run_steps(1);
        assert_eq!(h.state().views[0].pending_pick, None);
    }

    /// A click a comment takes lets go of the earlier click: its picture isn't picked after the
    /// comment is selected.
    #[test]
    fn a_click_on_a_comment_takes_back_the_earlier_click() {
        let (mut h, at) = open_page();
        let _keep = click_still_being_looked_into(&mut h);
        click(&mut h, at(50.0, 50.0));
        assert_eq!(h.state().views[0].comments.selected, Some((0, 0)), "the comment is selected");
        assert_eq!(h.state().views[0].pending_pick, None);
    }

    /// A click on a link follows it and lets go of the earlier click too.
    #[test]
    fn a_click_on_a_link_takes_back_the_earlier_click() {
        let (mut h, at) = open_page();
        let _keep = click_still_being_looked_into(&mut h);
        click(&mut h, at(150.0, 250.0));
        assert!(h.state().pending_link.is_some(), "the link is followed");
        assert_eq!(h.state().views[0].pending_pick, None);
    }
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
pub(crate) fn safe_name(s: &str) -> String {
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
    /// ⌘C / Ctrl+C copies the selected picture in its original format; Escape deselects it, or
    /// takes back a click whose picture isn't known yet.
    pub(crate) fn image_pick_keys(&mut self, i: usize, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if ctx.input(|inp| inp.key_pressed(egui::Key::Escape)) {
            if let Some(view) = self.views.get_mut(i) {
                view.picked_image = None;
                view.pending_pick = None;
            }
            return;
        }
        let Some(picked) = self.views.get(i).and_then(|v| v.picked_image) else { return };
        let copy = ctx.input(|inp| inp.events.iter().any(|e| matches!(e, egui::Event::Copy)));
        if !copy || self.views[i].selected_text().is_some() {
            return;
        }
        // The document may have changed since the click: the same picture must still be there.
        let current =
            self.session.get(self.views[i].id).and_then(|d| d.page_images(picked.page).get(picked.index).map(crate::canvas::image_selectable_bounds));
        if current == Some(picked.rect) {
            self.copy_page_image_file(picked.page, picked.index);
        } else {
            self.views[i].picked_image = None;
            self.notify_tr("The page has changed; select the image again");
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
            self.last_copied_image = None;
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
        crate::os_clipboard::with(|c| c.set().file_list(&[path])).map_err(|e| e.to_string())
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
    use super::{is_copied_image_name, write_clipboard_file};

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
