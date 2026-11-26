//! Select tool: click a picture on the page to select it, then ⌘C / Ctrl+C (or the right-click
//! menu) copies it in its original format: a JPEG image as the JPEG file stored in the PDF, any
//! other image as a lossless PNG (with its transparency). The clipboard gets the file, as when a
//! file is copied in Finder or Explorer; Copy Image copies the pixels instead.

use std::sync::Arc;

use craft::behavior::kurbo::BezPath;
use craft::host::queue::{JobQueue, Priority, QueueOptions};
use craft::host::{Jobs, Wake};
use craft::pages::ui::PageBody;
#[cfg(not(target_arch = "wasm32"))]
use craft::platform::clipboard::Flavor;
use craft::ui::canvas_controls::SelectionOutline;
use craft::ui::contract::Show as _;

use crate::PdfCraftApp;
use crate::canvas::{PageGeometry, PageSession, PageXform};
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

/// The picture a click asked for ([`PageSession::pending_pick`]), once the page's pictures are known: the window stays live
/// until then. A click made before an edit asks for nothing.
pub(crate) fn resolve_pending(
    session: &mut PageSession,
    geo: PageGeometry<'_>,
    doc: &pdfcraft_engine::Document,
    info: &DocInfo,
    jobs: &Arc<dyn Jobs>,
    wake: &Wake,
) {
    let Some((page, at, clicked_in)) = session.pending_pick else { return };
    let generation = doc.edit_generation();
    if clicked_in != generation {
        session.pending_pick = None;
        return;
    }
    // A page not drawn keeps the click waiting.
    let Some(xf) = session.page_xform(page, geo) else { return };
    let answer = session.pictures.get(jobs, wake, doc, page).map(|pictures| {
        // Where the click was on the page now: the view may have moved while the page was read.
        let point = xf.user_rect_f64(info, page, [at[0], at[1], at[0], at[1]]).center();
        pdfcraft_engine::picture_at(pictures, at).and_then(|(index, i)| {
            let screen_rect = crate::canvas::image_screen_rect(i, &xf, info, page);
            (screen_rect.is_positive() && screen_rect.contains(point)).then(|| PickedImage {
                page,
                index: *index,
                rect: crate::canvas::image_selectable_bounds(i),
                generation,
            })
        })
    });
    let Some(picked) = answer else { return };
    if picked.is_some() {
        session.text.selection = None;
    }
    session.picked_image = picked;
    session.pending_pick = None;
}

/// Page and document generation.
type PictureKey = (usize, u64);

/// The pictures of the page last asked about, found on a job queue: the search reads every drawing operator on the
/// page, seconds on a dense one. A small file answers on the spot, so its menu and selection don't flicker. On the web
/// the queue has no lanes and each pass runs one search inline, so a dense page never holds a whole frame.
///
/// One search at a time: a page left while it is read is dropped, and cancelled if it is running.
#[derive(Default)]
pub(crate) struct PagePictures {
    /// Made on the first search, on the host's jobs.
    queue: Option<JobQueue<PictureKey, Vec<pdfcraft_engine::PagePicture>>>,
    /// The pictures of the key's page.
    ready: Option<(PictureKey, Vec<pdfcraft_engine::PagePicture>)>,
}

impl PagePictures {
    /// A file smaller than this has its pictures read on the spot.
    const SMALL_FILE: usize = 8 * 1024 * 1024;

    /// The pictures on `page`, or `None` while they are being found (the queue's `wake` repaints on the answer).
    // PLAN: the decisions give `get(ctx, host, wake, doc, page)`; the queue needs the host's jobs and the wake only.
    pub(crate) fn get(
        &mut self,
        jobs: &Arc<dyn Jobs>,
        wake: &Wake,
        doc: &pdfcraft_engine::Document,
        page: usize,
    ) -> Option<&[pdfcraft_engine::PagePicture]> {
        let key = (page, doc.edit_generation());
        self.poll();
        let known = self.ready.as_ref().is_some_and(|(k, _)| *k == key);
        let searching = self.queue.as_ref().is_some_and(|q| q.contains(&key));
        if !known && !searching {
            match doc.page_pictures_job(page) {
                Some(work) if doc.bytes.len() < Self::SMALL_FILE => self.ready = Some((key, work())),
                Some(work) => self.start(jobs, wake, key, work),
                None => self.ready = Some((key, Vec::new())),
            }
        }
        self.poll();
        self.ready.as_ref().filter(|(k, _)| *k == key).map(|(_, pictures)| pictures.as_slice())
    }

    fn start(
        &mut self,
        jobs: &Arc<dyn Jobs>,
        wake: &Wake,
        key: PictureKey,
        work: impl FnOnce() -> Vec<pdfcraft_engine::PagePicture> + Send + 'static,
    ) {
        let queue = self.queue.get_or_insert_with(|| {
            // One search at a time: each is a full read of a page. The web has no lanes; `poll` runs the job.
            let options = QueueOptions { lanes: if cfg!(target_arch = "wasm32") { 0 } else { 1 }, name: "page-pictures", ..QueueOptions::default() };
            JobQueue::new(Arc::clone(jobs), wake.clone(), options)
        });
        queue.reprioritize(|k, priority| (*k == key).then_some(priority));
        queue.submit(key, Priority::VISIBLE, move |_| work());
    }

    /// Take the answers that have come; on the web, first run one waiting search.
    fn poll(&mut self) {
        let Some(queue) = &self.queue else { return };
        #[cfg(target_arch = "wasm32")]
        queue.run_inline(1);
        while let Some(done) = queue.try_recv() {
            self.ready = Some((done.key, done.result));
        }
    }
}

#[cfg(test)]
mod picture_tests {
    use super::*;
    use craft::host::{AppInfo, Host, ManualJobs};
    use egui_kittest::Harness;

    /// A page (200 × 300) with a square comment over 20..80 × 20..80 and a link over 120..180 ×
    /// 220..280, in a file past `SMALL_FILE`, so its pictures are found on the job queue.
    fn page_with_a_comment_and_a_link() -> Vec<u8> {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
            "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
            "<< /Type /Annot /Subtype /Square /Rect [20 20 80 80] /C [1 0 0] /Border [0 0 2] >>",
            "<< /Type /Annot /Subtype /Link /Rect [120 220 180 280] /Border [0 0 0] /A << /S /URI /URI (https://example.org/) >> >>",
        ];
        let mut out = b"%PDF-1.7\n%".to_vec();
        out.extend(std::iter::repeat_n(b'x', PagePictures::SMALL_FILE));
        out.push(b'\n');
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

    /// The page open with the Select tool on an app whose jobs wait in `jobs`, once it is on screen, and where a point
    /// on it is. The jobs are run only while the page settles.
    fn open_page(ctx: &egui::Context) -> (Harness<'static, PdfCraftApp>, Arc<ManualJobs>, impl Fn(f32, f32) -> egui::Pos2) {
        let jobs = Arc::new(ManualJobs::new());
        let host_jobs = jobs.clone();
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            {
                let host = Host::builder(AppInfo { id: "pdfcraft", name: "PdfCraft", version: "0" }).jobs(host_jobs).build();
                let services = crate::AppServices::new(
                    Wake::none(),
                    craft::platform::clipboard::Clipboard::new(craft::platform::clipboard::SystemClipboard::new()),
                    None,
                    None,
                    craft::platform::open::FileIntake::new(Wake::none()).0,
                    craft::platform::watch::FileWatch::new(Wake::none()).inline(),
                );
                let mut app = PdfCraftApp::with_host(Arc::new(host), services);
                app.set_option(&cc.egui_ctx, "language", "en").unwrap();
                app.open_bytes(ctx, "marks.pdf", None, page_with_a_comment_and_a_link()).unwrap();
                app
            }
        });
        let mut on_screen = None;
        for _ in 0..600 {
            jobs.run_all();
            h.run_steps(2);
            on_screen = h.state().views[0].page_screen_rect(0);
            if on_screen.is_some() {
                break;
            }
        }
        let r = on_screen.expect("the page is on screen");
        let k = r.width() / 200.0;
        (h, jobs, move |x, y| r.min + egui::vec2(x * k, (300.0 - y) * k))
    }

    fn click(h: &mut Harness<'static, PdfCraftApp>, p: egui::Pos2) {
        h.hover_at(p);
        h.run_steps(1);
        h.drag_at(p);
        h.run_steps(1);
        h.drop_at(p);
        h.run_steps(4);
    }

    /// A click on a blank part of the page whose pictures are still being found, as on a dense drawing: the search waits
    /// in the jobs, which the test does not run.
    fn click_still_being_looked_into(h: &mut Harness<'static, PdfCraftApp>, at: &impl Fn(f32, f32) -> egui::Pos2) {
        click(h, at(100.0, 150.0));
        h.run_steps(2);
        let _test = std::thread::current().name().and_then(|n| n.rsplit("::").next()).unwrap_or_default().to_owned();
        let _s = h.state();
        assert!(h.state().views[0].session.pending_pick.is_some(), "the click waits for its page");
    }

    /// A search that takes its time doesn't hold the window: the click returns before the search runs, and its answer is
    /// taken in the pass after the job has.
    #[test]
    fn a_slow_page_does_not_hold_up_the_window() {
        let ctx = &egui::Context::default();
        let (mut h, jobs, at) = open_page(ctx);
        click_still_being_looked_into(&mut h, &at);
        assert!(jobs.waiting() > 0, "the search is queued as a job");
        assert_eq!(h.state().views[0].session.picked_image, None);
        jobs.run_all();
        h.run_steps(3);
        assert_eq!(h.state().views[0].session.pending_pick, None, "the answer came: no picture there");
    }

    /// Escape takes back a click whose picture isn't known yet: it isn't selected when it is.
    #[test]
    fn escape_cancels_a_click_on_a_page_still_being_read() {
        let ctx = &egui::Context::default();
        let (mut h, _jobs, at) = open_page(ctx);
        click_still_being_looked_into(&mut h, &at);
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        assert_eq!(h.state().views[0].session.pending_pick, None);
    }

    /// Another tool lets go of the click, even while its page isn't drawn (Organize pages).
    #[test]
    fn another_tool_takes_back_a_click_on_a_page_not_drawn() {
        let ctx = &egui::Context::default();
        let (mut h, _jobs, at) = open_page(ctx);
        click_still_being_looked_into(&mut h, &at);
        let ctx = h.ctx.clone();
        assert!(h.state_mut().execute(&ctx, "page.organize"));
        h.run_steps(2);
        assert!(h.state().views[0].session.pending_pick.is_some(), "the click waits while the page isn't drawn");
        h.state_mut().quick_tool = crate::QuickTool::Hand;
        h.run_steps(1);
        h.state_mut().quick_tool = crate::QuickTool::Select;
        h.run_steps(1);
        assert_eq!(h.state().views[0].session.pending_pick, None);
    }

    /// A click a comment takes lets go of the earlier click: its picture isn't picked after the
    /// comment is selected.
    #[test]
    fn a_click_on_a_comment_takes_back_the_earlier_click() {
        let ctx = &egui::Context::default();
        let (mut h, _jobs, at) = open_page(ctx);
        click_still_being_looked_into(&mut h, &at);
        h.run_steps(30);
        click(&mut h, at(50.0, 50.0));
        assert!(h.state().views[0].session.comments.primary().is_some(), "the comment is selected");
        assert_eq!(h.state().views[0].session.pending_pick, None);
    }

    /// A click on a link follows it and lets go of the earlier click too.
    #[test]
    fn a_click_on_a_link_takes_back_the_earlier_click() {
        let ctx = &egui::Context::default();
        let (mut h, _jobs, at) = open_page(ctx);
        click_still_being_looked_into(&mut h, &at);
        h.run_steps(30);
        click(&mut h, at(150.0, 250.0));
        assert!(h.state().pending_link.is_some(), "the link is followed");
        assert_eq!(h.state().views[0].session.pending_pick, None);
    }
}

/// Forget the selected picture once the document has been edited since it was picked: Edit text &
/// images (or Undo) may have deleted or moved it, and its highlight would stay behind on an empty
/// spot of the page.
pub(crate) fn drop_stale(doc: &pdfcraft_engine::Document, session: &mut PageSession) {
    if session.picked_image.is_some_and(|p| p.generation != doc.edit_generation()) {
        session.picked_image = None;
    }
}

/// The selected picture's outline in the page body, over the page as shown with the view turned by `rotation`.
// PLAN: pdf-canvas-w8's body calls `selection::show_picked(ui, body, session, cx.info)`; it is here, and takes
// `cx.rotation` after `session`.
pub(crate) fn show_picked(ui: &mut egui::Ui, body: &PageBody<'_>, session: &PageSession, rotation: u16, info: &DocInfo) {
    let Some(picked) = session.picked_image else { return };
    let Some(info_page) = info.pages.get(picked.page) else { return };
    if !picked.rect.iter().all(|v| v.is_finite()) {
        return;
    }
    let Some(placed) = session.placed.iter().find(|p| p.page == picked.page) else { return };
    let xf = PageXform::of(placed, rotation, (info_page.width, info_page.height));
    let r = xf.user_rect(info, picked.page, picked.rect.map(|v| v as f32));
    let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()].map(|p| body.view.to_doc(p));
    let mut path = BezPath::new();
    path.move_to(corners[0]);
    for corner in &corners[1..] {
        path.line_to(*corner);
    }
    path.close_path();
    let _ = SelectionOutline::solid(tl!("Picked picture"), &[path]).show_at(ui, body.rect);
}

/// A file name without characters file systems refuse.
pub(crate) fn safe_name(s: &str) -> String {
    let cleaned: String = s.chars().map(|c| if c.is_control() || r#"/\:*?"<>|"#.contains(c) { '_' } else { c }).take(80).collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() { "PdfCraft".into() } else { cleaned }
}

/// The per-user folder that holds the file of the last Copy Image in Original Format: `Clipboard`
/// in the portable data folder in portable mode ([`crate::settings::portable_dir`]), otherwise the platform's
/// per-user cache folder: `~/Library/Caches/PdfCraft/Clipboard` (macOS),
/// `%LOCALAPPDATA%\PdfCraft\Clipboard` (Windows), or `$XDG_CACHE_HOME/pdfcraft/clipboard` /
/// `~/.cache/pdfcraft/clipboard` (others). Never the shared temporary folder, where another user
/// could plant a link or a folder of their own.
#[cfg(not(target_arch = "wasm32"))]
fn clipboard_dir() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    if let Some(dir) = crate::settings::portable_dir() {
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
    /// ⌘C / Ctrl+C with a picture picked and no text selected: copies the picture in its original format. The document
    /// may have changed since the click: the same picture must still be there, else it is let go. Returns whether
    /// there was a picked picture to copy.
    pub(crate) fn copy_picked_image(&mut self, i: usize) -> bool {
        let Some((id, picked)) = self.views.get(i).and_then(|v| Some((v.id, v.session.picked_image?))) else { return false };
        let current = self.session.get(id).and_then(|d| d.page_images(picked.page).get(picked.index).map(crate::canvas::image_selectable_bounds));
        if current == Some(picked.rect) {
            self.copy_page_image_file(picked.page, picked.index);
        } else {
            if let Some(view) = self.views.get_mut(i) {
                view.session.picked_image = None;
            }
            self.notify_tr("The page has changed; select the image again");
        }
        true
    }

    /// Copy Image in Original Format: image `index` on `page` as a file on the clipboard (JPEG
    /// as stored in the PDF, otherwise PNG).
    pub(crate) fn copy_page_image_file(&mut self, page: usize, index: usize) {
        // A newer copy wins over a whole-document copy still reading its pages.
        self.cancel_text_copies();
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
    fn put_file_on_clipboard(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let dir = clipboard_dir().ok_or("there is no folder for the copied file")?;
        let path = write_clipboard_file(&dir, name, bytes)?;
        // PLAN: assumed `PdfCraftApp.clipboard: craft::platform::clipboard::Clipboard` (pdf-hub-w1, the services given to `with_host`)
        self.clipboard.write(&[Flavor::files(&[path])]).map(|_| ()).map_err(|e| e.to_string())
    }

    #[cfg(target_arch = "wasm32")]
    fn put_file_on_clipboard(&mut self, _name: &str, _bytes: &[u8]) -> Result<(), String> {
        Err("the web version can't put files on the clipboard yet; use Save Image As".to_string())
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
