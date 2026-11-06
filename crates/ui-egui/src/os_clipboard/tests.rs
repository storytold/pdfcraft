//! A per-test-thread clipboard that records native access without touching the desktop.
use std::cell::RefCell;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use crate::PdfCraftApp;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};

thread_local! {
    static BACKEND: RefCell<Option<Arc<Backend>>> = const { RefCell::new(None) };
}

pub(super) struct Backend {
    accesses: AtomicUsize,
    first_on_ui: AtomicBool,
    ui: std::thread::ThreadId,
    lock: Mutex<()>,
    block: bool,
    released: Mutex<bool>,
    wake: Condvar,
}

pub(super) fn backend() -> Option<Arc<Backend>> {
    BACKEND.with(|backend| backend.borrow().clone())
}

pub(super) struct Installed(Option<Arc<Backend>>);
pub(super) fn install(value: Option<Arc<Backend>>) -> Installed {
    Installed(BACKEND.with(|backend| backend.replace(value)))
}
impl Drop for Installed {
    fn drop(&mut self) {
        BACKEND.with(|backend| backend.replace(self.0.take()));
    }
}

pub(super) fn access() -> Option<Result<(), arboard::Error>> {
    let backend = backend()?;
    let index = backend.accesses.fetch_add(1, Ordering::SeqCst);
    Some((|| {
        // The very same acquisition helper as production; a reverted blocking lock is
        // released by this backend's hard timeout, so even the red cannot hang CI.
        let _guard = super::acquire(&backend.lock)?;
        if index == 0 {
            backend.first_on_ui.store(std::thread::current().id() == backend.ui, Ordering::SeqCst);
            if backend.block {
                let released = backend.released.lock().unwrap();
                let _ = backend.wake.wait_timeout_while(released, expected_read_timeout() + Duration::from_secs(2), |released| !*released).unwrap();
            }
        }
        Ok(())
    })())
}

struct ClipboardProbe {
    backend: Arc<Backend>,
    _installed: Installed,
}
impl ClipboardProbe {
    fn new() -> Self {
        Self::with_block(false)
    }
    fn with_block(block: bool) -> Self {
        let backend = Arc::new(Backend {
            accesses: AtomicUsize::new(0),
            first_on_ui: AtomicBool::new(false),
            ui: std::thread::current().id(),
            lock: Mutex::new(()),
            block,
            released: Mutex::new(false),
            wake: Condvar::new(),
        });
        let installed = install(Some(backend.clone()));
        Self { backend, _installed: installed }
    }
    fn accesses(&self) -> usize {
        self.backend.accesses.load(Ordering::SeqCst)
    }
}
impl Drop for ClipboardProbe {
    fn drop(&mut self) {
        *self.backend.released.lock().unwrap() = true;
        self.backend.wake.notify_all();
    }
}

fn input(h: &mut Harness<'_, PdfCraftApp>, events: impl IntoIterator<Item = egui::Event>) {
    h.input_mut().events.extend(events);
    let ctx = h.ctx.clone();
    let mut raw = std::mem::take(h.input_mut());
    h.state_mut().prepare_raw_input(&ctx, &mut raw);
    *h.input_mut() = raw;
    h.step();
}

#[test]
fn copying_text_then_opening_the_page_menu_never_reads_the_native_clipboard() {
    let probe = ClipboardProbe::new();
    let bytes = pdfcraft_engine::Session::new().create_from_text("Clipboard", "Hello clipboard").unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("clipboard.pdf", None, bytes.as_ref().clone()).unwrap();
        app.system_clipboard = true;
        app
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while h.state().views[0].texts.get(&0).is_none_or(|text| text.glyphs.is_empty()) {
        assert!(std::time::Instant::now() < deadline, "the synthetic text layer arrives");
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    h.run_steps(3);
    let view = &h.state().views[0];
    let rect = view.page_screen_rect(0).unwrap();
    let page = &h.state().session.get(view.id).unwrap().info.pages[0];
    let text = view.texts.get(&0).unwrap();
    let first = text.glyphs.first().unwrap().rect;
    let last = text.glyphs.last().unwrap().rect;
    let screen = |x: f32, y: f32| egui::pos2(rect.left() + x * rect.width() / page.width, rect.top() + y * rect.height() / page.height);
    let start = screen(first[0] + 0.1, (first[1] + first[3]) * 0.5);
    let end = screen(last[2] - 0.1, (last[1] + last[3]) * 0.5);
    let button = |pos, pressed, button| egui::Event::PointerButton { pos, button, pressed, modifiers: egui::Modifiers::NONE };
    input(&mut h, [egui::Event::PointerMoved(start), button(start, true, egui::PointerButton::Primary)]);
    input(&mut h, [egui::Event::PointerMoved(end)]);
    input(&mut h, [button(end, false, egui::PointerButton::Primary)]);
    h.run_steps(3);
    assert!(h.state().views[0].selected_text().is_some_and(|text| text.contains("clipboard")));
    h.get_by_label("Copy").click();
    let mut copied = false;
    for _ in 0..5 {
        input(&mut h, []);
        copied |= h
            .output()
            .platform_output
            .commands
            .iter()
            .any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text.contains("clipboard")));
    }
    assert!(copied, "the floating Copy button exports the selected text");
    assert_eq!(probe.accesses(), 0, "Copy and idle frames do not query the native clipboard");
    let point = rect.center();
    input(&mut h, [egui::Event::PointerMoved(point), button(point, true, egui::PointerButton::Secondary)]);
    input(&mut h, [button(point, false, egui::PointerButton::Secondary)]);
    h.run_steps(3);
    assert_eq!(probe.accesses(), 0, "opening the menu must not enter a native clipboard call that can block on Wayland");
    assert!(!h.get_by_label_contains("Paste").accesskit_node().is_disabled(), "unknown native clipboard contents allow an explicit paste attempt");
    h.run_steps(5);
    assert_eq!(probe.accesses(), 0, "keeping the menu open must not poll the clipboard");
    if let Some(path) = std::env::var_os("PDFCRAFT_CLIPBOARD_MENU_SHOT") {
        h.render().unwrap().save(path).unwrap();
    }
    // An unavailable system clipboard must not fall back to a stale cached image.
    h.state_mut().last_copied_image = Some((1, 1, vec![255, 0, 0, 255]));
    h.get_by_label_contains("Paste").click();
    input(&mut h, []);
    h.run_steps(3);
    assert!(probe.accesses() > 0, "the explicit Paste command validates on a worker");
    assert!(!probe.backend.first_on_ui.load(Ordering::SeqCst), "native reads never run on the UI thread");
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert!(doc.page_images(0).is_empty(), "unavailable clipboard contents do not insert an image");
    assert!(!doc.dirty, "a rejected paste does not modify the document");
}

fn harness(image: bool) -> Harness<'static, PdfCraftApp> {
    let bytes = pdfcraft_engine::Session::new().create_from_text("Clipboard", "Hello clipboard").unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("clipboard.pdf", None, bytes.as_ref().clone()).unwrap();
        if image {
            let png = crate::create_ui::png_from_rgba(40, 40, &[255, 0, 0, 255].repeat(1600)).unwrap();
            app.add_image("square.png".into(), png);
            app.quick_tool = crate::QuickTool::Select;
        }
        app.system_clipboard = true;
        app
    });
    h.run_steps(4);
    h
}

fn menu(h: &mut Harness<'_, PdfCraftApp>, point: egui::Pos2) {
    for pressed in [true, false] {
        input(
            h,
            [
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton { pos: point, pressed, button: egui::PointerButton::Secondary, modifiers: egui::Modifiers::NONE },
            ],
        );
    }
    h.run_steps(2);
}

fn paste_from_menu(h: &mut Harness<'_, PdfCraftApp>) {
    let rect = h.state().views[0].page_screen_rect(0).unwrap();
    menu(h, rect.left_top() + egui::vec2(100.0, 100.0));
    h.get_by_label_contains("Paste").click();
    input(h, []);
}

// Keep this independent of READ_TIMEOUT so accidentally extending the selection-owner
// budget still fails the real-caller regressions, with a finite backend release deadline.
fn expected_read_timeout() -> Duration {
    if cfg!(any(target_os = "windows", target_os = "macos")) { Duration::from_secs(5) } else { Duration::from_secs(1) }
}

#[test]
fn native_clipboard_read_deadline_matches_the_platform() {
    assert_eq!(super::read_timeout_for(false), Duration::from_secs(1));
    assert_eq!(super::read_timeout_for(true), Duration::from_secs(5));
    assert_eq!(super::READ_TIMEOUT, expected_read_timeout());
}

fn timed_out(h: &Harness<'_, PdfCraftApp>, probe: &ClipboardProbe, elapsed: Duration) {
    assert!(elapsed < expected_read_timeout() + Duration::from_millis(600), "UI frame exceeded the bounded clipboard wait: {elapsed:?}");
    assert_eq!(probe.accesses(), 1, "one read, no duplicate text probe");
    assert!(!probe.backend.first_on_ui.load(Ordering::SeqCst), "read runs off the UI thread");
    assert_eq!(h.state().toast.as_ref().map(|toast| toast.0.as_str()), Some("The clipboard isn't responding"));
}

#[test]
fn blocked_context_paste_returns_the_frame_and_notifies() {
    let probe = ClipboardProbe::with_block(true);
    let mut h = harness(false);
    let start = Instant::now();
    paste_from_menu(&mut h);
    timed_out(&h, &probe, start.elapsed());
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
}

#[test]
fn blocked_ctrl_v_fallback_returns_the_frame_and_notifies() {
    let probe = ClipboardProbe::with_block(true);
    let mut h = harness(false);
    let start = Instant::now();
    input(
        &mut h,
        [
            egui::Event::ModifiersChanged(egui::Modifiers::COMMAND),
            egui::Event::Key { key: egui::Key::V, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND },
        ],
    );
    timed_out(&h, &probe, start.elapsed());
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
}

#[test]
fn blocked_create_from_clipboard_returns_and_notifies() {
    let probe = ClipboardProbe::with_block(true);
    let mut h = harness(false);
    let start = Instant::now();
    assert!(h.state_mut().execute("create.clipboard"));
    timed_out(&h, &probe, start.elapsed());
    assert_eq!(h.state().views.len(), 1);
}

#[test]
fn image_copy_after_a_stuck_read_never_waits_for_its_lock() {
    let probe = ClipboardProbe::with_block(true);
    let mut h = harness(true);
    paste_from_menu(&mut h);
    assert_eq!(h.state().toast.as_ref().map(|toast| toast.0.as_str()), Some("The clipboard isn't responding"));
    let image = h.state().session.get(h.state().views[0].id).unwrap().page_images(0)[0].clone();
    let page = &h.state().session.get(h.state().views[0].id).unwrap().info.pages[0];
    let center = page.user_to_view(((image.rect[0] + image.rect[2]) * 0.5) as f32, ((image.rect[1] + image.rect[3]) * 0.5) as f32);
    let point = h.state().views[0].page_xform(0).unwrap().norm_to_screen(center[0] / page.width, center[1] / page.height);
    menu(&mut h, point);
    h.get_by_label("Copy Image").click();
    let start = Instant::now();
    input(&mut h, []);
    assert!(start.elapsed() < Duration::from_millis(500), "Copy must not wait for a stuck reader's lock");
    assert!(h.state().toast.as_ref().unwrap().0.starts_with("Couldn't copy the image:"));
    assert_eq!(probe.accesses(), 2);
}

fn image_data(width: usize, height: usize, bytes: usize) -> Result<arboard::ImageData<'static>, arboard::Error> {
    Ok(arboard::ImageData { width, height, bytes: std::borrow::Cow::Owned(vec![128; bytes]) })
}

#[test]
fn clipboard_formats_keep_their_mode_priority() {
    use super::{ReadMode, select_content};
    use crate::create_ui::Clip;
    assert!(matches!(select_content(image_data(1, 1, 4), Ok("words".into()), ReadMode::Create),
        Some(Clip::Image { width: 1, height: 1, rgba }) if rgba == vec![128; 4]));
    assert!(matches!(select_content(image_data(1, 1, 4), Ok("words".into()), ReadMode::ImagePaste),
        Some(Clip::Text(text)) if text.is_empty()));
    for mode in [ReadMode::Create, ReadMode::ImagePaste] {
        assert!(matches!(select_content(image_data(1, 1, 4), Ok(String::new()), mode), Some(Clip::Image { .. })));
        assert!(matches!(select_content(image_data(1, 1, 4), Err(arboard::Error::ContentNotAvailable), mode), Some(Clip::Image { .. })));
        assert!(matches!(select_content(Err(arboard::Error::ContentNotAvailable), Ok("words".into()), mode), Some(Clip::Text(_))));
        assert!(select_content(Err(arboard::Error::ContentNotAvailable), Err(arboard::Error::ContentNotAvailable), mode).is_none());
    }
    // Preserve Paste's nonempty-text priority, including whitespace; Create ignores blank text.
    assert!(matches!(select_content(image_data(1, 1, 4), Ok(" ".into()), ReadMode::ImagePaste), Some(Clip::Text(_))));
    assert!(select_content(Err(arboard::Error::ContentNotAvailable), Ok(" ".into()), ReadMode::Create).is_none());
}

#[test]
fn invalid_create_image_falls_back_to_readable_text() {
    use super::{ReadMode, select_content};
    use crate::create_ui::{Clip, MAX_IMAGE_SIDE};
    // Zero dimensions, over the side/byte caps, malformed byte count and overflowing dimensions.
    let cases =
        [(0, 1, 0), (1, 0, 0), (MAX_IMAGE_SIDE + 1, 1, (MAX_IMAGE_SIDE + 1) * 4), (4096, 4097, 4096 * 4097 * 4), (2, 2, 4), (usize::MAX, 2, 0)];
    for (width, height, bytes) in cases {
        assert!(
            matches!(select_content(image_data(width, height, bytes), Ok("fallback".into()), ReadMode::Create),
            Some(Clip::Text(text)) if text == "fallback"),
            "Create must retain readable text beside invalid {width}x{height}/{bytes}-byte image"
        );
        // Paste already prefers text, and without text an invalid image remains unusable.
        assert!(
            matches!(select_content(image_data(width, height, bytes), Ok("fallback".into()), ReadMode::ImagePaste), Some(Clip::Text(text)) if text.is_empty())
        );
        for mode in [ReadMode::Create, ReadMode::ImagePaste] {
            assert!(select_content(image_data(width, height, bytes), Ok(String::new()), mode).is_none());
            assert!(select_content(image_data(width, height, bytes), Err(arboard::Error::ContentNotAvailable), mode).is_none());
        }
    }
}
