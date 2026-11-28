//! The command registry drives menus, shortcuts and the palette: every registered command must
//! be implemented, disabled commands must say why, and the app's platform split of chords holds.
//! The palette's own listing and Enter-runs behaviour, the shortcut catalog and the chord text are
//! the framework's to test.

use std::sync::Arc;

use crate::control::{ControlClient, app_with_files, new_app, ok};
use crate::support;

use crate::support::AppHarness as _;
use craft::host::{Picked, ScriptedFiles};
use egui::os::OperatingSystem;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::commands::COMMANDS;
use pdfcraft_ui_egui::PdfCraftApp;
use serde_json::json;

fn fixture(n: usize) -> Vec<u8> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i));
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

/// An app with three pages open, and the control channel attached for what only `ui.state` reports.
fn harness_controlled() -> (Harness<'static, PdfCraftApp>, ControlClient) {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.attach_control(craft::automation::ControlChannel::new(rx), None);
            app.open_bytes(&egui::Context::default(), "doc.pdf", None, fixture(3)).unwrap();
            app
        }
    });
    h.run_steps(4);
    (h, tx)
}

fn harness() -> Harness<'static, PdfCraftApp> {
    harness_controlled().0
}

/// Whether the palette is on screen: its command rows are list-box options in the access tree.
fn palette_open(h: &Harness<'static, PdfCraftApp>) -> bool {
    h.query_all_by_role(egui::accesskit::Role::ListBoxOption).next().is_some()
}

/// Whether the comments panel is open, as the dock layout `ui.state` reports it says.
// PLAN: assumed `ui.state["side_panels"]` (pdf-hub-w3, from `side_panels::state`) names the open
// panel; the panel is found by its name, not by the report's shape.
fn comments_panel_open(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient) -> bool {
    ok(h, c, "ui.state", json!({}))["side_panels"].to_string().to_lowercase().contains("comment")
}

/// Whether the last frame asked the window to close.
fn close_requested(h: &Harness<'static, PdfCraftApp>) -> bool {
    h.output().viewport_output.values().any(|v| v.commands.iter().any(|c| matches!(c, egui::ViewportCommand::Close)))
}

/// Whether the export dialog a tool opens is on screen: the only dialog these tests open.
fn export_open(h: &Harness<'static, PdfCraftApp>) -> bool {
    support::dialog_open(h)
}

/// Runs the registry command `id` as the menu does, with the harness's own context.
fn run_command(h: &mut Harness<'static, PdfCraftApp>, id: &str) {
    let ctx = h.ctx.clone();
    h.state_mut().run_command(&ctx, id);
}

/// Commands that open a native file picker can't run headless.
/// (Create ▸ Clipboard reads the system clipboard: tests use `create_from_clip`.)
const PICKERS: &[&str] = &[
    "file.open",
    "page.combine",
    "page.insert",
    "file.save_as",
    "create.file",
    "create.images",
    "create.multiple",
    "page.replace",
    "create.clipboard",
    "a11y.report",
    "ocr.recognize_batch",
    "form.merge_data",
    "export.docx",
    "export.pptx",
    "export.html",
    "export.rtf",
    "measure.export",
];

#[test]
fn every_registered_command_is_implemented() {
    for spec in COMMANDS {
        // Page-text Copy needs a loaded view and egui context, covered by text_select.
        if PICKERS.contains(&spec.id) || spec.id == "edit.copy" {
            continue;
        }
        // A picker the list above misses (file.pin_folder, create.multiple_separate did) must not
        // open a real file dialog on the desktop running the tests: the scripted files answer it
        // as a cancel, which is what a service with nothing queued does.
        let mut app = app_with_files(Arc::new(ScriptedFiles::new()));
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        if spec.id.starts_with("form.") || spec.id == "comment.flatten" {
            app.open_bytes(&egui::Context::default(), "form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
        } else if spec.id == "protect.unlock" {
            // Only a permissions password: opens restricted, and can be unlocked.
            let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(fixture(3))).unwrap();
            doc.set_encryption(&pdfcraft_cos::NewEncryption {
                algorithm: pdfcraft_cos::Algorithm::Aes256,
                user_password: "",
                owner_password: "owner",
                permissions: 0b0100,
                encrypt_metadata: true,
                seed: [5; 32],
            })
            .unwrap();
            app.open_bytes(&egui::Context::default(), "restricted.pdf", None, pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()).unwrap();
        } else {
            app.open_bytes(&egui::Context::default(), "doc.pdf", None, fixture(3)).unwrap();
        }
        // Give undo/redo something to do.
        app.apply_edit(&egui::Context::default(), pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
        if spec.id == "edit.redo" {
            app.undo(&egui::Context::default());
        }
        if spec.id == "prepress.marks.remove" {
            use pdfcraft_engine::{Edit, PrinterMarks};
            app.apply_edit(&egui::Context::default(), Edit::AddPrinterMarks { pages: vec![0], settings: PrinterMarks::default(), replace: false });
        }
        // Copying comments needs a selected comment; pasting needs copied ones.
        if matches!(spec.id, "comment.copy" | "comment.paste") {
            let shape = pdfcraft_engine::Shape::Rectangle { rect: [10.0, 10.0, 50.0, 50.0] };
            let style = pdfcraft_engine::Style::default_for(&shape);
            assert!(app.apply_edit(
                &egui::Context::default(),
                pdfcraft_engine::Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
                    page: 0,
                    shape,
                    style,
                    contents: String::new(),
                    author: String::new(),
                })
            ));
            app.views[0].session.comments.select_only((0, 0));
            if spec.id == "comment.paste" {
                assert!(app.copy_selected_comments(0));
            }
        }
        if spec.id.starts_with("edit.") && (spec.id.ends_with(".update") || spec.id.ends_with(".remove")) {
            use pdfcraft_engine::{Background, Edit, HeaderFooter, Watermark};
            let mut hf = HeaderFooter::default();
            hf.text[1] = "x".into();
            app.apply_edit(&egui::Context::default(), Edit::AddHeaderFooter { pages: vec![0], settings: hf, replace: false });
            app.apply_edit(
                &egui::Context::default(),
                Edit::AddWatermark { pages: vec![0], settings: Watermark { text: "x".into(), ..Watermark::default() }, replace: false, file: None },
            );
            app.apply_edit(
                &egui::Context::default(),
                Edit::AddBackground {
                    pages: vec![0],
                    settings: Background { color: [1.0; 3], opacity: 1.0, ..Background::default() },
                    replace: false,
                    file: None,
                },
            );
        }
        if matches!(spec.id, "comment.flatten" | "comment.export" | "comment.summarize") {
            use pdfcraft_engine::{Edit, NewAnnotation, Shape, Style};
            let shape = Shape::Rectangle { rect: [10.0, 10.0, 50.0, 50.0] };
            app.apply_edit(
                &egui::Context::default(),
                Edit::AddAnnotation(NewAnnotation {
                    page: 0,
                    style: Style::default_for(&shape),
                    shape,
                    contents: String::new(),
                    author: String::new(),
                }),
            );
        }
        if spec.id.starts_with("redact.") {
            use pdfcraft_engine::{Edit, NewAnnotation, Shape, Style};
            let shape =
                Shape::Redact { quads: vec![pdfcraft_engine::rect_quad([10.0, 10.0, 50.0, 50.0])], overlay: String::new(), look: Default::default() };
            app.apply_edit(
                &egui::Context::default(),
                Edit::AddAnnotation(NewAnnotation {
                    page: 0,
                    style: Style::default_for(&shape),
                    shape,
                    contents: String::new(),
                    author: String::new(),
                }),
            );
        }
        if spec.id == "protect.remove" {
            let p = pdfcraft_engine::Protection { open_password: Some("pw".into()), ..Default::default() };
            app.apply_edit(&egui::Context::default(), pdfcraft_engine::Edit::Protect(p));
        }
        // The cover toggle needs two-page view first (it is disabled elsewhere).
        if spec.id == "view.layout.cover" {
            app.set_option(&egui::Context::default(), "layout", "two-up").unwrap();
        }
        // History commands need a real jump (and Back first to make Forward available).
        if matches!(spec.id, "view.previous_view" | "view.next_view") {
            app.views[0].go_to_page(1);
            if spec.id == "view.next_view" {
                assert!(app.views[0].view_history(false));
            }
        }
        assert!(app.execute(&egui::Context::default(), spec.id), "{} is registered but not implemented (or wrongly disabled)", spec.id);
    }
}

/// A disabled command says why. The first two have a chord, which is pressed as a user would; the
/// cover page has none, so the command table fires it.
#[test]
fn disabled_commands_explain_themselves() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app
        }
    });
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run_steps(3);
    assert_eq!(support::shown_notice(&h).as_deref(), Some("Open a document first"));
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "doc.pdf", None, fixture(2)).unwrap();
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(support::shown_notice(&h).as_deref(), Some("Nothing to undo"));
    // View state counts too: the cover page exists only in two-page view.
    assert!(!h.execute("view.layout.cover"));
    h.run_steps(3);
    assert_eq!(support::shown_notice(&h).as_deref(), Some("Switch to two-page view first to show the cover page"));
    assert!(!h.execute("no.such.command"));
}

/// An app on a file service that answers the first pick with `picked`, none queued when `None`.
fn harness_picking(picked: Option<&std::path::Path>, setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let files = match picked {
        Some(path) => ScriptedFiles::answering([Picked::Paths(vec![path.to_path_buf()])]).with_file(path, fixture(2)),
        None => ScriptedFiles::new(),
    };
    let files = Arc::new(files);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = app_with_files(files);
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            setup(&mut app);
            app
        }
    });
    h.run_steps(2);
    h
}

#[test]
fn a_tool_chosen_with_no_document_asks_for_one_and_runs_on_it() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-tool-open-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("picked.pdf");
    std::fs::write(&path, fixture(2)).unwrap();
    let mut h = harness_picking(Some(&path), |_| {});
    run_command(&mut h, "export.text");
    assert!(
        support::step_until(&mut h, |h| h.state().active.is_some() && export_open(h)),
        "timed out waiting for the picked document and chosen tool"
    );
    assert!(h.state().active.is_some(), "the picked file is open");
    assert!(export_open(&h), "and the tool runs on it");
    let (_, id) = h.state().active_ids().unwrap();
    let doc = h.state().session.get(id).unwrap();
    assert_eq!(doc.name, "picked.pdf");
    assert_eq!(doc.bytes.as_slice(), fixture(2).as_slice());
    // Cancelled: nothing opens and nothing runs.
    let mut h = harness_picking(None, |_| {});
    run_command(&mut h, "export.text");
    h.run_steps(3);
    assert!(h.state().active.is_none() && !export_open(&h));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_tool_chosen_with_an_already_open_file_focuses_its_tab_and_still_runs() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-tool-focus-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("picked.pdf");
    std::fs::write(&path, fixture(2)).unwrap();
    let opened = path.to_string_lossy().into_owned();
    // The file is already open, but the Home tab is showing (no active document).
    let mut h = harness_picking(Some(&path), move |app| {
        app.open_path(&opened);
        app.active = None;
    });
    run_command(&mut h, "export.text");
    h.run_steps(3);
    let app = h.state();
    assert_eq!(app.views.len(), 1, "the existing tab is reused, not duplicated");
    assert_eq!(app.active, Some(0), "the pick focused the tab it already opened");
    assert!(export_open(&h), "the tool still runs on it");
    let _ = std::fs::remove_dir_all(&dir);
}

/// One row of the chord table: what the user does and what the app must do, on a harness with
/// three pages open.
type ChordRow = (&'static str, fn(&mut Harness<'static, PdfCraftApp>, &ControlClient));

/// The app's platform split of chords (the runtime fires catalog chords; these are the ones whose
/// meaning differs by platform or that leave a state the next frame could undo).
const CHORDS: &[ChordRow] = &[
    ("Ctrl+M opens the comments panel and keeps it open (Windows)", ctrl_m_opens_comments_on_windows),
    ("Ctrl+M opens the comments panel and keeps it open (Linux)", ctrl_m_opens_comments_on_linux),
    ("Cmd+M is left to the OS for minimize (macOS)", mac_cmd_m_is_left_to_the_os),
    ("Cmd+W closes the clean document tab (macOS)", mac_cmd_w_closes_the_tab),
    ("Ctrl+Tab steps the tabs in order and wraps", ctrl_tab_steps_and_wraps),
    ("Escape leaves full screen", escape_leaves_full_screen),
];

#[test]
fn platform_split_chords_fire_their_commands() {
    for (name, row) in CHORDS {
        let (mut h, c) = harness_controlled();
        eprintln!("chord row: {name}");
        row(&mut h, &c);
    }
}

fn ctrl_m_opens_comments_on_windows(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient) {
    ctrl_m_opens_comments(h, c, OperatingSystem::Windows);
}

fn ctrl_m_opens_comments_on_linux(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient) {
    ctrl_m_opens_comments(h, c, OperatingSystem::Nix);
}

fn ctrl_m_opens_comments(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, os: OperatingSystem) {
    h.ctx.set_os(os);
    h.run_steps(2);
    assert!(!comments_panel_open(h, c), "{os:?}: starts without the comments panel");
    // Real Windows/Linux Ctrl events set both ctrl and command; the second press must not toggle.
    for _ in 0..2 {
        h.key_press_modifiers(Modifiers::CTRL | Modifiers::COMMAND, Key::M);
        h.run_steps(2);
        assert!(comments_panel_open(h, c), "{os:?}: opens rather than toggles");
    }
}

fn mac_cmd_m_is_left_to_the_os(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient) {
    h.ctx.set_os(OperatingSystem::Mac);
    h.run_steps(2);
    let command_m = Modifiers { mac_cmd: true, command: true, ..Modifiers::NONE };
    h.input_mut().events.push(egui::Event::Key { key: Key::M, physical_key: None, pressed: true, repeat: false, modifiers: command_m });
    h.step();
    let chord = craft::menu::Chord::new(craft::keymap::Mods { cmd: true, ..Default::default() }, craft::keymap::Key::M);
    assert!(craft::ui::runtime::commands::holder(&h.ctx, &chord).is_none(), "Command+M must be available for Minimize");
    h.run_steps(2);
    assert!(!comments_panel_open(h, c), "no app command ran");
}

fn mac_cmd_w_closes_the_tab(h: &mut Harness<'static, PdfCraftApp>, _c: &ControlClient) {
    h.ctx.set_os(OperatingSystem::Mac);
    h.run_steps(2);
    h.key_press_modifiers(Modifiers { mac_cmd: true, command: true, ..Modifiers::NONE }, Key::W);
    h.run_steps(2);
    assert!(h.state().views.is_empty(), "⌘W closes the clean document tab");
}

/// Ctrl+Tab and Ctrl+Shift+Tab step through the tabs in the strip's order, wrap around at either
/// end and include the Combine files tab after the documents (#1542). A Windows or Linux Ctrl
/// press is both `ctrl` and `command`.
fn ctrl_tab_steps_and_wraps(h: &mut Harness<'static, PdfCraftApp>, _c: &ControlClient) {
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "second.pdf", None, fixture(1)).unwrap();
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "third.pdf", None, fixture(1)).unwrap();
    h.run_steps(3);
    let ctrl = Modifiers::CTRL | Modifiers::COMMAND;
    let press = |h: &mut Harness<'static, PdfCraftApp>, modifiers: Modifiers| {
        h.key_press_modifiers(modifiers, Key::Tab);
        h.run_steps(2);
        h.state().active
    };
    assert_eq!(h.state().active, Some(2));
    assert_eq!(press(h, ctrl), Some(0), "the last tab wraps to the first");
    assert_eq!(press(h, ctrl), Some(1));
    assert_eq!(press(h, ctrl | Modifiers::SHIFT), Some(0));
    assert_eq!(press(h, ctrl | Modifiers::SHIFT), Some(2), "the first tab wraps to the last");
    // The order is the one on screen, after the tabs are moved.
    let ids: Vec<_> = h.state().views.iter().map(|v| v.id).collect();
    h.state_mut().move_tab(2, 0);
    h.run_steps(2);
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[2]));
    press(h, ctrl);
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[0]));
    // Combine files comes after the documents.
    h.execute("page.combine");
    h.run_steps(2);
    assert!(h.state().combine_showing());
    assert_eq!(press(h, ctrl), Some(0), "Combine files wraps to the first document");
    assert_eq!(press(h, ctrl | Modifiers::SHIFT), None);
    assert!(h.state().combine_showing(), "back from the first document is Combine files");
    // Plain Tab still belongs to keyboard focus, not to the tabs.
    press(h, Modifiers::NONE);
    assert!(h.state().combine_showing());
    for doc in h.state().session.docs() {
        assert!(!doc.dirty, "switching tabs changes no document");
    }
}

fn escape_leaves_full_screen(h: &mut Harness<'static, PdfCraftApp>, _c: &ControlClient) {
    assert!(!h.state().full_screen());
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(2);
    assert!(h.state().full_screen(), "⌘L / Ctrl+L enters full screen");
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(!h.state().full_screen(), "Escape leaves full screen");
}

/// Check that choosing Edit ▸ Pages ▸ Delete pages in the menu bar reduces the page count once.
#[test]
fn the_pages_menu_comes_from_the_registry() {
    let mut h = harness();
    support::open_menu(&mut h, "Edit");
    support::menu_row(&h, "Pages").hover();
    h.run_steps(3);
    support::menu_row(&h, "Delete pages").click();
    h.run_steps(3);
    let app = h.state();
    assert_eq!(app.session.get(app.views[0].id).unwrap().info.pages.len(), 2);
}

/// Check that Open Recent lists a recorded file and opens it in a new active tab without replacing the
/// original.
#[test]
fn the_open_recent_menu_lists_files_and_opens_one() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-recent-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("recent.pdf");
    std::fs::write(&path, fixture(2)).unwrap();
    let path = path.to_string_lossy().into_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe({
        let path = path.clone();
        move |cc| {
            crate::support::install_runtime(&cc.egui_ctx);
            let mut app = new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "doc.pdf", None, fixture(3)).unwrap();
            app.recent.push(pdfcraft_ui_egui::RecentFile { name: "recent.pdf".into(), path, pages: 2, size: 0 });
            app
        }
    });
    h.run_steps(4);
    support::open_menu(&mut h, "File");
    support::menu_row(&h, "Open Recent").hover();
    h.run_steps(3);
    let original = h.state().views[0].id;
    support::menu_row(&h, "recent.pdf").click();
    support::wait_until(&mut h, "recent document", |app| app.views.len() == 2 && app.password_prompt.is_none());
    let app = h.state();
    let active = app.active.unwrap();
    assert_eq!(app.session.get(app.views[active].id).and_then(|d| d.path.as_deref()), Some(path.as_str()), "the active tab is the recent file");
    assert_eq!(app.session.get(app.views[active].id).unwrap().bytes.as_slice(), fixture(2).as_slice());
    assert!(app.views.iter().any(|view| view.id == original));
    assert_eq!(app.session.get(original).unwrap().bytes.as_slice(), fixture(3).as_slice());
}

fn recent_file(name: &str) -> pdfcraft_ui_egui::RecentFile {
    pdfcraft_ui_egui::RecentFile { name: name.into(), path: format!("/nowhere/{name}"), pages: 1, size: 0 }
}

/// Firing Clear Recent Files empties the list, saves the empty list, and says so when there was
/// nothing to clear.
#[test]
fn clear_recent_files_runs_from_the_palette_and_is_saved() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.recent.extend([recent_file("first.pdf"), recent_file("second.pdf")]);
            app
        }
    });
    h.run_steps(3);
    assert!(h.execute("file.clear_recent"));
    assert!(h.state().recent.is_empty());
    let mut restarted = new_app();
    restarted.recent.push(recent_file("stale.pdf"));
    restarted.apply_settings(&h.state().settings_snapshot());
    assert!(restarted.recent.is_empty(), "the empty list is what's saved");
    h.execute("file.clear_recent");
    h.run_steps(3);
    assert!(support::shown_notice(&h).is_some_and(|m| m.contains("No recent files")), "nothing to clear is said, not silent");
}

#[test]
fn localized_shortcuts_update_immediately_and_keep_command_ids_and_key_bindings() {
    if crate::support::skip_without_catalogs("localized_shortcuts_update_immediately_and_keep_command_ids_and_key_bindings") {
        return;
    }
    let mut h = harness();
    h.ctx.set_os(OperatingSystem::Windows);
    h.set_option("language", "de").unwrap();
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::CTRL | Modifiers::COMMAND, Key::K);
    h.run_steps(3);
    assert!(palette_open(&h), "the localized Strg+K display still binds Ctrl+K");
}

/// Searching by the stable command id works in a translated interface: the id is typed into the open palette
/// and Enter runs the match. The port's palette does not take a typed query yet (the `palette` option only opens it).
#[ignore = "known gap of the egui-to-craft port: see docs/port-known-gaps.md"]
#[test]
fn palette_search_by_command_id_runs_the_command_in_a_translated_interface() {
    if crate::support::skip_without_catalogs("palette_search_by_command_id_runs_the_command_in_a_translated_interface") {
        return;
    }
    let mut h = harness();
    h.ctx.set_os(OperatingSystem::Windows);
    h.set_option("language", "de").unwrap();
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::CTRL | Modifiers::COMMAND, Key::K);
    h.run_steps(3);
    assert!(palette_open(&h));
    h.event(egui::Event::Text("view.read_mode".into()));
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(h.state().mode, pdfcraft_ui_egui::Mode::Read);
    assert!(!palette_open(&h));
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert!(!doc.dirty);
}

/// Check that the quit shortcut needs exact platform modifiers and that unsaved work opens the save prompt
/// instead of closing: the close is the window's close intent, and the prompt is answered by its button.
#[test]
fn quit_shortcut_requires_exact_modifiers_and_preserves_unsaved_work() {
    for (os, modifiers) in [
        (OperatingSystem::Windows, Modifiers { ctrl: true, command: true, ..Default::default() }),
        (OperatingSystem::Mac, Modifiers { mac_cmd: true, command: true, ..Default::default() }),
    ] {
        for (extra, dirty, should_close) in
            [(Modifiers::ALT, false, false), (Modifiers::SHIFT, false, false), (Modifiers::NONE, false, true), (Modifiers::NONE, true, false)]
        {
            let mut h = harness();
            h.ctx.set_os(os);
            if dirty {
                assert!(h.execute("page.insert_blank"));
                h.run_steps(2);
            }
            h.event(egui::Event::Key { key: Key::Q, physical_key: None, pressed: true, repeat: false, modifiers: modifiers | extra });
            h.step();
            // Catalog commands run in the next frame's input hook.
            h.step();
            assert_eq!(close_requested(&h), should_close, "{os:?}, extra={extra:?}, dirty={dirty}");
            if dirty {
                // The save prompt is a modal laid out after the key pass; let that pass settle
                // before looking it up.
                h.run_steps(2);
                h.get_by_label("Cancel").click();
                h.run_steps(2);
                assert_eq!(h.state().views.len(), 1);
                assert!(h.state().session.get(h.state().views[0].id).unwrap().dirty);
                assert!(!close_requested(&h));
            }
        }
    }
}

#[test]
fn command_w_closes_empty_macos_window_but_not_an_empty_windows_window() {
    for (os, should_close) in [(OperatingSystem::Mac, true), (OperatingSystem::Windows, false)] {
        let mut h = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(|cc| {
            crate::support::install_runtime(&cc.egui_ctx);
            new_app()
        });
        h.ctx.set_os(os);
        h.run_steps(4);
        assert!(h.state().views.is_empty());
        let modifiers = if os == OperatingSystem::Mac {
            Modifiers { mac_cmd: true, command: true, ..Modifiers::NONE }
        } else {
            Modifiers { ctrl: true, command: true, ..Modifiers::NONE }
        };
        h.event(egui::Event::Key { key: Key::W, physical_key: None, pressed: true, repeat: false, modifiers });
        h.step();
        h.step();
        assert_eq!(close_requested(&h), should_close, "{os:?}: only an empty macOS window should close");
    }
}
