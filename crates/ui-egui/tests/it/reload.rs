//! A document reloads when its file changes on disk (#431): a LaTeX or Typst build rewriting the
//! open PDF. Here the watcher looks once per frame on the test's thread (`run_inline`); in the
//! app a worker thread looks every quarter second.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT, Queryable};
use pdfcraft_ui_egui::{PdfCraftApp, RecoveryStore, SaveTarget};

/// An `n`-page document with a proper xref table; page `i` shows "Page i+1", and with `notes`
/// each page has a sticky note.
fn fixture_with(n: usize, notes: bool) -> Vec<u8> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    let annots = if notes { " /Annots [<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /Contents (note) >>]" } else { "" };
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >>{annots} >>", 5 + 2 * i));
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
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

fn fixture(n: usize) -> Vec<u8> {
    fixture_with(n, false)
}

/// A PDF in a fresh temp folder, removed when the test ends.
struct TempPdf {
    dir: PathBuf,
    path: String,
    builds: std::cell::Cell<u64>,
}

impl TempPdf {
    fn new(test: &str, bytes: &[u8]) -> Self {
        let dir = std::env::temp_dir().join(format!("pdfcraft-reload-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("thesis.pdf").to_string_lossy().into_owned();
        std::fs::write(&path, bytes).unwrap();
        Self { dir, path, builds: std::cell::Cell::new(0) }
    }

    /// Rewrite it as a build does: new contents, and a later modification time every time.
    fn rebuild(&self, bytes: &[u8]) {
        std::fs::write(&self.path, bytes).unwrap();
        self.builds.set(self.builds.get() + 1);
        let file = std::fs::File::options().write(true).open(&self.path).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(60 * self.builds.get())).unwrap();
    }
}

impl Drop for TempPdf {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn harness(path: &str) -> Harness<'static, PdfCraftApp> {
    harness_at(path, egui::vec2(1400.0, 900.0))
}

fn harness_at(path: &str, size: egui::Vec2) -> Harness<'static, PdfCraftApp> {
    let path = path.to_owned();
    let mut h = Harness::builder().with_size(size).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        // The labels looked for are English, whatever the machine's language.
        app.set_option("language", "en").unwrap();
        // The labels below are English whatever the developer's system language is.
        app.set_option("language", "en").unwrap();
        app.run_inline = true;
        app.open_path(&path);
        app
    });
    h.run_steps(4);
    h
}

fn pages(h: &Harness<'static, PdfCraftApp>) -> usize {
    let app = h.state();
    app.session.get(app.views[0].id).unwrap().info.pages.len()
}

fn dirty(h: &Harness<'static, PdfCraftApp>) -> bool {
    let app = h.state();
    app.session.get(app.views[0].id).unwrap().dirty
}

fn rotate_first_page(h: &mut Harness<'static, PdfCraftApp>) {
    h.state_mut().views[0].select_pages(&[0]);
    h.state_mut().apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
}

#[test]
fn a_rebuilt_file_reloads_in_place_keeping_the_page_and_zoom() {
    let file = TempPdf::new("rebuild", &fixture(3));
    let mut h = harness(&file.path);
    let id = h.state().views[0].id;
    h.state_mut().views[0].fit = pdfcraft_ui_egui::canvas::Fit::None;
    h.state_mut().views[0].zoom = 1.5;
    h.state_mut().views[0].go_to_page(2);
    h.run_steps(4);
    file.rebuild(&fixture(4));
    // One look sees the change, the next sees it settled and reloads.
    h.run_steps(3);
    let app = h.state();
    assert_eq!(pages(&h), 4, "the new version shows");
    assert_eq!((app.views.len(), app.views[0].id), (1, id), "in the same tab");
    assert_eq!((app.views[0].current, app.views[0].zoom), (2, 1.5), "at the same page and zoom");
}

#[test]
fn a_watched_label_replacement_keeps_the_display_mode_and_focused_page() {
    use pdfcraft_engine::{Edit, LabelStyle, Session};
    use pdfcraft_ui_egui::canvas::display_token;
    use std::sync::Arc;

    let labelled = |ranges: &[(usize, usize, LabelStyle)]| {
        let mut source = Session::new().with_clock(|| 1_700_000_000);
        let id = source.open("labels.pdf", None, Arc::new(fixture(5)), None).unwrap();
        for &(from, to, style) in ranges {
            source.apply(id, Edit::NumberPages { from, to, style, prefix: String::new(), first: 1 }).unwrap();
        }
        source.save_bytes(id).unwrap()
    };
    let original = labelled(&[(0, 1, LabelStyle::LowerRoman), (2, 4, LabelStyle::Decimal)]);
    let replacement = labelled(&[(0, 4, LabelStyle::LowerRoman)]);
    for display in ["single", "continuous"] {
        let file = TempPdf::new(&format!("focused-label-{display}"), &original);
        let mut h = harness(&file.path);
        let id = h.state().views[0].id;
        h.state_mut().set_option("layout", display).unwrap();
        h.state_mut().set_option("zoom", "100").unwrap();
        h.state_mut().set_option("page", "4").unwrap();
        h.run_steps(3);
        let view = &h.state().views[0];
        assert_eq!((view.current, display_token(view.layout, view.continuous())), (3, display));
        let doc = h.state().session.get(id).unwrap();
        assert_eq!(doc.bytes, original);
        assert!(!doc.dirty);
        assert_eq!((doc.can_undo(), doc.can_redo()), (None, None));
        assert!(h.state_mut().execute("view.focus_page_input"));
        h.run_steps(3);
        h.get(By::new().role(egui::accesskit::Role::TextInput).value("2"));
        let editor = h.ctx.memory(|m| m.focused()).expect("the page editor is focused");

        // A build atomically publishes a whole, clean PDF. The real watcher detects it.
        let next = file.dir.join("next.pdf");
        std::fs::write(&next, replacement.as_slice()).unwrap();
        let published = std::fs::File::options().write(true).open(&next).unwrap();
        published.set_modified(SystemTime::now() + Duration::from_secs(60)).unwrap();
        drop(published);
        std::fs::rename(next, &file.path).unwrap();
        for _ in 0..8 {
            h.run_steps(1);
            if h.state().session.get(id).unwrap().bytes == replacement {
                break;
            }
        }
        h.run_steps(2);
        let app = h.state();
        let view = &app.views[0];
        let doc = app.session.get(id).unwrap();
        assert_eq!((app.views.len(), app.active, view.id), (1, Some(0), id));
        assert_eq!(doc.bytes, replacement, "the watcher loaded the replacement before checking navigation");
        assert_eq!((doc.info.pages.len(), doc.info.pages[3].label.as_str()), (5, "iv"));
        // First causal negative: a single-page reader must not become a continuous reader.
        assert_eq!((view.current, display_token(view.layout, view.continuous())), (3, display), "reload retains the physical page and display mode");
        assert_eq!(view.zoom, 1.0);
        assert_eq!(view.page_input, "iv");
        assert_eq!(h.ctx.memory(|m| m.focused()), Some(editor));
        h.get(By::new().role(egui::accesskit::Role::TextInput).value("iv"));

        // Keep the replacement editor's selection and then its caret, without re-focusing it.
        h.event(egui::Event::Text("2".into()));
        h.run_steps(2);
        assert_eq!((h.state().views[0].current, h.state().views[0].page_input.as_str()), (3, "2"));
        h.event(egui::Event::Text("x".into()));
        h.run_steps(1);
        assert_eq!(h.state().views[0].page_input, "2x");
        h.key_press(egui::Key::Backspace);
        h.run_steps(1);
        assert_eq!(h.state().views[0].page_input, "2");
        h.key_press(egui::Key::Enter);
        h.run_steps(3);
        let app = h.state();
        assert_eq!((app.views[0].id, app.views[0].current, app.views[0].page_input.as_str()), (id, 1, "ii"));
        let doc = app.session.get(id).unwrap();
        assert_eq!(doc.path.as_deref(), Some(file.path.as_str()));
        assert!(!doc.dirty);
        assert_eq!((doc.can_undo(), doc.can_redo()), (None, None));
        assert_eq!(doc.bytes, replacement);
        assert_eq!(app.session.save_bytes(id).unwrap(), replacement);
        assert_eq!(std::fs::read(&file.path).unwrap(), replacement.as_ref().clone());
    }
}

#[test]
fn a_half_written_file_waits_until_the_pdf_is_whole() {
    let file = TempPdf::new("partial", &fixture(2));
    let mut h = harness(&file.path);
    let whole = fixture(5);
    // A build part-way through: whole objects up to the fourth page, no cross-reference table
    // or end yet. The lenient parser would open this as a document with pages missing.
    let cut = whole.windows(9).position(|w| w == b"\n10 0 obj").unwrap();
    file.rebuild(&whole[..cut]);
    h.run_steps(4);
    assert_eq!(pages(&h), 2, "half a file is not loaded");
    file.rebuild(&whole);
    h.run_steps(3);
    assert_eq!(pages(&h), 5, "the whole one is");
}

#[test]
fn saving_does_not_reload_or_lose_undo() {
    let file = TempPdf::new("save", &fixture(2));
    let mut h = harness(&file.path);
    rotate_first_page(&mut h);
    h.state_mut().save_active(SaveTarget::InPlace);
    h.run_steps(4);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    assert!(!doc.dirty, "saved");
    assert!(doc.can_undo().is_some(), "a reload would have cleared the undo history");
}

#[test]
fn unsaved_changes_are_kept_until_reload_is_chosen() {
    let file = TempPdf::new("dirty", &fixture(2));
    let mut h = harness(&file.path);
    rotate_first_page(&mut h);
    file.rebuild(&fixture(4));
    h.run_steps(3);
    assert_eq!((pages(&h), dirty(&h)), (2, true), "not replaced behind the user's back");
    h.get_by_label_contains("This file changed on disk");
    h.get_by_label("Reload").click();
    h.run_steps(3);
    assert_eq!((pages(&h), dirty(&h)), (4, false), "replaced when asked");
    assert!(h.query_by_label_contains("This file changed on disk").is_none());
}

#[test]
fn a_deleted_file_leaves_the_document_open() {
    let file = TempPdf::new("deleted", &fixture(2));
    let mut h = harness(&file.path);
    std::fs::remove_file(&file.path).unwrap();
    h.run_steps(4);
    assert_eq!((h.state().views.len(), pages(&h)), (1, 2), "still open, as it was");
    file.rebuild(&fixture(3));
    h.run_steps(3);
    assert_eq!(pages(&h), 3, "and the next build shows");
}

#[test]
fn nothing_reloads_with_the_preference_off() {
    assert!(PdfCraftApp::new().reload_changed_files, "on by default");
    let file = TempPdf::new("off", &fixture(2));
    let mut h = harness(&file.path);
    h.state_mut().reload_changed_files = false;
    h.run_steps(1);
    file.rebuild(&fixture(3));
    h.run_steps(4);
    assert_eq!(pages(&h), 2);
    let mut restarted = PdfCraftApp::new();
    restarted.restore(&h.state().persist());
    assert!(!restarted.reload_changed_files, "the choice is kept");
}

#[test]
fn a_shorter_new_version_drops_what_pointed_into_the_old_one() {
    let file = TempPdf::new("shorter", &fixture_with(5, true));
    let mut h = harness(&file.path);
    h.state_mut().views[0].go_to_page(4);
    h.state_mut().views[0].select_pages(&[3, 4]);
    h.state_mut().views[0].comments.selected = Some((4, 0));
    h.run_steps(2);
    file.rebuild(&fixture(1));
    h.run_steps(4);
    let view = &h.state().views[0];
    assert_eq!(pages(&h), 1);
    assert_eq!(view.current, 0, "the page is clamped");
    assert!(view.selected.is_empty() && view.comments.selected.is_none(), "no selection points past the new version");
}

/// Save asks before writing over a version of the file the document did not come from (#2166):
/// something else wrote it since the open or the last save. Cancel keeps the file on disk;
/// Save anyway replaces it, and the next Save does not ask.
#[test]
fn saving_over_a_file_changed_by_something_else_asks_first() {
    let file = TempPdf::new("overwrite", &fixture(2));
    let mut h = harness(&file.path);
    rotate_first_page(&mut h);
    file.rebuild(&fixture(4));
    h.run_steps(3);
    h.get_by_label_contains("This file changed on disk");
    assert!(!h.state_mut().save_active(SaveTarget::InPlace), "not written yet");
    h.run_steps(2);
    h.get_by_label("Save over the changed file?");
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(std::fs::read(&file.path).unwrap(), fixture(4), "Cancel keeps the newer file");
    assert!(dirty(&h) && h.state().pending_overwrite.is_none());
    assert!(!h.state_mut().save_active(SaveTarget::InPlace));
    h.run_steps(2);
    h.get_by_label("Save anyway").click();
    h.run_steps(3);
    assert!(!dirty(&h), "written");
    assert_ne!(std::fs::read(&file.path).unwrap(), fixture(4));
    assert_eq!(pages(&h), 2, "the document, not the file, is what is shown");
    // Our own write is the version the document came from now.
    rotate_first_page(&mut h);
    assert!(h.state_mut().save_active(SaveTarget::InPlace), "written at once");
}

const DISK_CHANGE_NOTICE: &str =
    "This file changed on disk. Reload shows the new version and drops your unsaved changes; Save replaces it with this version.";

fn assert_disk_change_notice_geometry(h: &Harness<'static, PdfCraftApp>, size: egui::Vec2, narrow: bool) {
    let notice = h.get_by_label(DISK_CHANGE_NOTICE).rect();
    let reload = h.get_by_label("Reload").rect();
    let dismiss = h.get_by_label("Dismiss").rect();
    let inside = |rect: egui::Rect| rect.left() >= 0.0 && rect.top() >= 0.0 && rect.right() <= size.x && rect.bottom() <= size.y;
    assert!(inside(notice), "notice text is clipped by the viewport: {notice:?}");
    assert!(inside(reload), "Reload is outside the viewport: {reload:?}");
    assert!(inside(dismiss), "Dismiss is outside the viewport: {dismiss:?}");
    assert!(!notice.intersects(reload), "notice text overlaps Reload: {notice:?}, {reload:?}");
    assert!(!notice.intersects(dismiss), "notice text overlaps Dismiss: {notice:?}, {dismiss:?}");
    assert!(!reload.intersects(dismiss), "notice buttons overlap: {reload:?}, {dismiss:?}");
    if narrow {
        assert!(notice.height() > 24.0, "the message should wrap at the narrow size: {notice:?}");
    }
}

/// The disk-change notice wraps before its actions at narrow widths, and both choices keep their
/// distinct file/document outcomes.
#[test]
fn changed_file_notice_wraps_and_keeps_both_actions_reachable() {
    for (name, size, narrow) in [("wide", egui::vec2(1400.0, 900.0), false), ("narrow", egui::vec2(800.0, 600.0), true)] {
        let external = fixture(4);
        let dismissed_file = TempPdf::new(&format!("notice-dismiss-{name}"), &fixture(2));
        let mut dismissed = harness_at(&dismissed_file.path, size);
        rotate_first_page(&mut dismissed);
        dismissed_file.rebuild(&external);
        dismissed.run_steps(3);
        assert_disk_change_notice_geometry(&dismissed, size, narrow);
        dismissed.get_by_label("Dismiss").click();
        dismissed.run_steps(2);
        assert!(dismissed.query_by_label(DISK_CHANGE_NOTICE).is_none(), "Dismiss hides the notice");
        assert_eq!(pages(&dismissed), 2, "Dismiss keeps the open document");
        assert!(dirty(&dismissed), "Dismiss keeps the unsaved rotation");
        assert_eq!(std::fs::read(&dismissed_file.path).unwrap(), external, "Dismiss preserves the external file");

        let reloaded_file = TempPdf::new(&format!("notice-reload-{name}"), &fixture(2));
        let mut reloaded = harness_at(&reloaded_file.path, size);
        rotate_first_page(&mut reloaded);
        reloaded_file.rebuild(&external);
        reloaded.run_steps(3);
        assert_disk_change_notice_geometry(&reloaded, size, narrow);
        reloaded.get_by_label("Reload").click();
        reloaded.run_steps(3);
        assert!(reloaded.query_by_label(DISK_CHANGE_NOTICE).is_none(), "Reload consumes the change notice");
        assert_eq!(pages(&reloaded), 4, "Reload shows the external version");
        assert!(!dirty(&reloaded), "Reload drops the unsaved rotation");
        let doc = reloaded.state().session.get(reloaded.state().views[0].id).unwrap();
        assert_eq!(doc.bytes.as_ref(), &external, "Reload adopts the external bytes");
        assert_eq!(std::fs::read(&reloaded_file.path).unwrap(), external, "Reload does not overwrite the external file");
    }
}

/// A reload click in the narrow both-pane layout asks for explicit confirmation. Cancel, Escape
/// and backdrop dismissal all retain the working document and its recovery snapshot.
#[test]
fn concealed_changed_file_notice_requires_confirmation_before_discard() {
    use egui::accesskit::Role;

    struct TempRecovery {
        dir: PathBuf,
        store: RecoveryStore,
    }
    impl TempRecovery {
        fn new() -> Self {
            let nonce = SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let dir = std::env::temp_dir().join(format!("pdfcraft-reload-confirm-{}-{nonce}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Self { store: RecoveryStore::new(&dir), dir }
        }
    }
    impl Drop for TempRecovery {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn click_at(h: &mut Harness<'static, PdfCraftApp>, at: egui::Pos2) {
        h.hover_at(at);
        h.run_steps(1);
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
            h.run_steps(1);
        }
        h.run_steps(1);
    }

    fn wait_for_prompt(h: &mut Harness<'static, PdfCraftApp>) {
        for _ in 0..8 {
            if h.query_all_by_role_and_label(Role::Label, "Reload").any(|node| node.rect().center().y > 200.0) {
                return;
            }
            h.run_steps(1);
            assert_eq!((pages(h), dirty(h)), (2, true), "waiting for a visible warning must not discard edits");
        }
        panic!("confirmation card did not become visible");
    }

    fn assert_cancelled(h: &Harness<'static, PdfCraftApp>, recovery: &RecoveryStore, external: &[u8], answer: &str) {
        assert_eq!((pages(h), dirty(h)), (2, true), "{answer} keeps the edited working document");
        assert!(h.state().views[0].disk_changed, "{answer} keeps the change notice active");
        assert_eq!(recovery.list().len(), 1, "{answer} keeps the unsaved recovery snapshot");
        let path = h.state().session.get(h.state().views[0].id).unwrap().path.clone().unwrap();
        assert_eq!(std::fs::read(path).unwrap(), external, "{answer} leaves external bytes intact");
        assert!(
            h.get_all_by_role_and_label(Role::Button, "Reload").any(|node| node.rect().center().y < 180.0),
            "{answer} leaves the banner available"
        );
    }

    let original = fixture(2);
    let external = fixture(4);
    let file = TempPdf::new("notice-both-panes-confirm", &original);
    let recovery = TempRecovery::new();
    let size = egui::vec2(800.0, 600.0);
    let path = file.path.clone();
    let store = recovery.store.clone();
    let mut h = Harness::builder().with_size(size).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.set_option("panel", "comments").unwrap();
        app.left_open = true;
        app.run_inline = true;
        app.enable_recovery(store);
        app.open_path(&path);
        app
    });
    h.run_steps(4);
    rotate_first_page(&mut h);
    h.state_mut().autosave_now();
    assert_eq!(recovery.store.list().len(), 1, "the edit has a recovery snapshot before the disk change");
    file.rebuild(&external);
    h.run_steps(3);

    let click_banner_reload = |h: &Harness<'static, PdfCraftApp>| {
        h.get_all_by_role_and_label(Role::Button, "Reload").find(|node| node.rect().center().y < 180.0).expect("the banner Reload action").click();
    };
    let modal_label = |h: &Harness<'static, PdfCraftApp>, label: &str| {
        h.get_all_by_role_and_label(Role::Label, label)
            .find(|node| node.rect().center().y > 200.0)
            .expect("the centered confirmation card label")
            .rect()
    };
    let modal_button_disabled = |h: &Harness<'static, PdfCraftApp>, label: &str| {
        h.get_all_by_role_and_label(Role::Button, label)
            .find(|node| node.rect().center().y > 200.0)
            .expect("the centered confirmation card button")
            .accesskit_node()
            .is_disabled()
    };
    let click_modal_button = |h: &Harness<'static, PdfCraftApp>, label: &str| {
        h.get_all_by_role_and_label(Role::Button, label)
            .find(|node| node.rect().center().y > 200.0)
            .expect("the centered confirmation card button")
            .click();
    };
    let assert_prompt_open = |h: &Harness<'static, PdfCraftApp>| {
        let heading = modal_label(h, "Reload");
        assert!(heading.width() > 0.0, "the modal has its heading");
        let warning = modal_label(h, DISK_CHANGE_NOTICE);
        assert!(warning.height() > 24.0, "the full warning wraps in the modal: {warning:?}");
        let cancel = h
            .get_all_by_role_and_label(Role::Button, "Cancel")
            .find(|node| node.rect().center().y > 200.0)
            .expect("the confirmation card Cancel action");
        let reload = h
            .get_all_by_role_and_label(Role::Button, "Reload")
            .find(|node| node.rect().center().y > 200.0)
            .expect("the confirmation card Reload action");
        assert!(!cancel.accesskit_node().is_disabled(), "Cancel is available immediately");
        assert!(cancel.is_focused(), "Cancel has default focus on the first displayed frame");
        assert!(reload.accesskit_node().is_disabled(), "confirming Reload is disabled on the first displayed frame");
    };

    let before_pending_prompt_zoom = (h.state().views[0].zoom, h.state().views[0].fit);
    click_banner_reload(&h);
    h.run_steps(1);

    // Before the confirmation area has been presented, page navigation must already be blocked.
    let gesture_pos = egui::pos2(300.0, 500.0);
    let viewport = h.state().views[0].viewport_rect();
    assert!(viewport.contains(gesture_pos), "the gesture point is on the page canvas: {viewport:?}");
    h.hover_at(gesture_pos);
    h.event(egui::Event::Zoom(1.5));
    h.run_steps(1);

    // This is deliberately the first document-state assertion after clicking the warning banner.
    // Current direct-reload behavior fails here by replacing the edited two-page document.
    assert_eq!((pages(&h), dirty(&h)), (2, true), "concealed warning must not discard unsaved work before confirmation");
    assert_eq!(
        (h.state().views[0].zoom, h.state().views[0].fit),
        before_pending_prompt_zoom,
        "pinch input is blocked as soon as reload confirmation is pending"
    );

    assert_eq!(std::fs::read(&file.path).unwrap(), external, "the prompt does not write over the external version");
    assert_eq!(recovery.store.list().len(), 1, "the prompt keeps the recovery snapshot");
    wait_for_prompt(&mut h);
    assert_prompt_open(&h);
    h.run_steps(2);
    assert!(!modal_button_disabled(&h, "Reload"), "the explicit Reload action becomes available after the first frame");
    click_modal_button(&h, "Cancel");
    h.run_steps(2);
    assert_cancelled(&h, &recovery.store, &external, "Cancel");
    // The same delivered gesture works again after cancellation, so the blocked input was real.
    h.hover_at(gesture_pos);
    h.run_steps(1);
    let cancelled_zoom = h.state().views[0].zoom;
    h.event(egui::Event::Zoom(1.5));
    h.run_steps(2);
    assert!(h.state().views[0].zoom > cancelled_zoom * 1.4, "pinch input works again after cancellation");

    click_banner_reload(&h);
    h.run_steps(1);
    assert_eq!((pages(&h), dirty(&h)), (2, true), "reopening the prompt does not discard edits");
    wait_for_prompt(&mut h);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_cancelled(&h, &recovery.store, &external, "Escape");

    click_banner_reload(&h);
    h.run_steps(1);
    assert_eq!((pages(&h), dirty(&h)), (2, true), "reopening after Escape keeps edits");
    wait_for_prompt(&mut h);
    click_at(&mut h, egui::pos2(5.0, 5.0));
    assert_cancelled(&h, &recovery.store, &external, "backdrop click");

    click_banner_reload(&h);
    h.run_steps(2);
    assert_eq!((pages(&h), dirty(&h)), (2, true), "confirmation still precedes replacement");
    wait_for_prompt(&mut h);
    h.run_steps(1);
    click_modal_button(&h, "Reload");
    h.run_steps(3);
    assert_eq!((pages(&h), dirty(&h)), (4, false), "explicit confirmation adopts the external version");
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert_eq!(doc.bytes.as_ref(), &external, "the opened document adopts the external bytes");
    assert_eq!(std::fs::read(&file.path).unwrap(), external, "Reload never overwrites the external file");
    assert!(recovery.store.list().is_empty(), "successful reload clears the recovery snapshot");
}

/// With the watcher off nothing notices the change until Save looks at the file.
#[test]
fn saving_checks_the_file_with_the_watcher_off_too() {
    let file = TempPdf::new("overwrite-unwatched", &fixture(2));
    let path = file.path.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.run_inline = true;
        app.reload_changed_files = false;
        app.open_path(&path);
        app
    });
    h.run_steps(4);
    rotate_first_page(&mut h);
    file.rebuild(&fixture(4));
    h.run_steps(3);
    assert!(h.query_by_label_contains("This file changed on disk").is_none(), "nothing watched");
    assert!(!h.state_mut().save_active(SaveTarget::InPlace));
    h.run_steps(2);
    h.get_by_label("Save over the changed file?");
}

/// One file, one tab (#2166): opening a file that is open already brings its tab forward,
/// however its path is spelled.
#[test]
fn opening_a_file_that_is_open_focuses_its_tab() {
    let file = TempPdf::new("twice", &fixture(2));
    let mut h = harness(&file.path);
    let id = h.state().views[0].id;
    h.state_mut().open_bytes("other.pdf", None, fixture(1)).unwrap();
    h.run_steps(2);
    assert_eq!((h.state().views.len(), h.state().active), (2, Some(1)));
    let spelled = file.dir.join(".").join("thesis.pdf").to_string_lossy().into_owned();
    assert_ne!(spelled, file.path);
    h.state_mut().open_path(&spelled);
    h.run_steps(2);
    let app = h.state();
    assert_eq!((app.views.len(), app.active, app.views[0].id), (2, Some(0), id), "focused, not opened again");
}
