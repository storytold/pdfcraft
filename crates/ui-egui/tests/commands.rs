//! The command registry drives menus, shortcuts and the palette: every registered command must
//! be implemented, disabled commands must say why, and every surface must reach the same action.

mod support;

use egui::os::OperatingSystem;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::commands::COMMANDS;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

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

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, fixture(3)).unwrap();
        app
    });
    h.run_steps(4);
    h
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
        // Page-text Copy needs a loaded view and egui context, covered by ui_group::text_select.
        if PICKERS.contains(&spec.id) || spec.id == "edit.copy" {
            continue;
        }
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        if spec.id.starts_with("form.") || spec.id == "comment.flatten" {
            app.open_bytes("form.pdf", None, include_bytes!("data/form.pdf").to_vec()).unwrap();
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
            app.open_bytes("restricted.pdf", None, pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()).unwrap();
        } else {
            app.open_bytes("doc.pdf", None, fixture(3)).unwrap();
        }
        // Give undo/redo something to do.
        app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
        if spec.id == "edit.redo" {
            app.undo();
        }
        if spec.id.starts_with("edit.") && (spec.id.ends_with(".update") || spec.id.ends_with(".remove")) {
            use pdfcraft_engine::{Background, Edit, HeaderFooter, Watermark};
            let mut hf = HeaderFooter::default();
            hf.text[1] = "x".into();
            app.apply_edit(Edit::AddHeaderFooter { pages: vec![0], settings: hf, replace: false });
            app.apply_edit(Edit::AddWatermark {
                pages: vec![0],
                settings: Watermark { text: "x".into(), ..Watermark::default() },
                replace: false,
                file: None,
            });
            app.apply_edit(Edit::AddBackground {
                pages: vec![0],
                settings: Background { color: [1.0; 3], opacity: 1.0, ..Background::default() },
                replace: false,
                file: None,
            });
        }
        if matches!(spec.id, "comment.flatten" | "comment.export" | "comment.summarize") {
            use pdfcraft_engine::{Edit, NewAnnotation, Shape, Style};
            let shape = Shape::Rectangle { rect: [10.0, 10.0, 50.0, 50.0] };
            app.apply_edit(Edit::AddAnnotation(NewAnnotation {
                page: 0,
                style: Style::default_for(&shape),
                shape,
                contents: String::new(),
                author: String::new(),
            }));
        }
        if spec.id.starts_with("redact.") {
            use pdfcraft_engine::{Edit, NewAnnotation, Shape, Style};
            let shape =
                Shape::Redact { quads: vec![pdfcraft_engine::rect_quad([10.0, 10.0, 50.0, 50.0])], overlay: String::new(), look: Default::default() };
            app.apply_edit(Edit::AddAnnotation(NewAnnotation {
                page: 0,
                style: Style::default_for(&shape),
                shape,
                contents: String::new(),
                author: String::new(),
            }));
        }
        if spec.id == "protect.remove" {
            let p = pdfcraft_engine::Protection { open_password: Some("pw".into()), ..Default::default() };
            app.apply_edit(pdfcraft_engine::Edit::Protect(p));
        }
        // The cover toggle needs two-page view first (it is disabled elsewhere).
        if spec.id == "view.layout.cover" {
            app.set_option("layout", "two-up").unwrap();
        }
        // History commands need a real jump (and Back first to make Forward available).
        if matches!(spec.id, "view.previous_view" | "view.next_view") {
            app.views[0].go_to_page(1);
            if spec.id == "view.next_view" {
                assert!(app.views[0].view_history(false));
            }
        }
        let dir = std::env::temp_dir().join(format!("pdfcraft-cmd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        app.save_override = Some(dir.join("out.pdf").to_string_lossy().into_owned());
        // A picker the list above misses (file.pin_folder, create.multiple_separate did) must not
        // open a real file dialog on the desktop running the tests: answer it as a cancel.
        app.pick_override = Some(Vec::new());
        assert!(app.execute(spec.id), "{} is registered but not implemented (or wrongly disabled)", spec.id);
    }
}

#[test]
fn disabled_commands_explain_themselves() {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    assert!(!app.execute("file.save"));
    assert_eq!(app.toast.as_ref().map(|t| t.0.as_str()), Some("Open a document first"));
    app.open_bytes("doc.pdf", None, fixture(2)).unwrap();
    assert!(!app.execute("edit.undo"));
    assert_eq!(app.toast.as_ref().map(|t| t.0.as_str()), Some("Nothing to undo"));
    // View state counts too: the cover page exists only in two-page view.
    assert!(!app.execute("view.layout.cover"));
    assert_eq!(app.toast.as_ref().map(|t| t.0.as_str()), Some("Switch to two-page view first to show the cover page"));
    assert!(!app.execute("no.such.command"));
}

#[test]
fn a_tool_chosen_with_no_document_asks_for_one_and_runs_on_it() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-tool-open-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("picked.pdf");
    std::fs::write(&path, fixture(2)).unwrap();
    let pick = path.to_string_lossy().into_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.pick_override = Some(vec![pick]);
        app
    });
    h.run_steps(2);
    h.state_mut().run_command("export.text");
    assert!(h.state().toast.as_ref().is_none_or(|t| t.0 != "Open a document first"));
    support::wait_until(&mut h, "picked document and chosen tool", |app| app.active.is_some() && matches!(app.dialog, Some(Dialog::Export(_))));
    assert!(h.state().active.is_some(), "the picked file is open");
    assert!(matches!(h.state().dialog, Some(Dialog::Export(_))), "and the tool runs on it: {:?}", h.state().dialog);
    let (_, id) = h.state().active_ids().unwrap();
    let doc = h.state().session.get(id).unwrap();
    assert_eq!(doc.name, "picked.pdf");
    assert_eq!(doc.bytes.as_slice(), fixture(2).as_slice());
    // Cancelled: nothing opens and nothing runs.
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.pick_override = Some(Vec::new());
        app
    });
    h.run_steps(2);
    h.state_mut().run_command("export.text");
    h.run_steps(3);
    assert!(h.state().active.is_none() && h.state().dialog.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_tool_chosen_with_an_already_open_file_focuses_its_tab_and_still_runs() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-tool-focus-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("picked.pdf");
    std::fs::write(&path, fixture(2)).unwrap();
    let pick = path.to_string_lossy().into_owned();
    let picked = pick.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        // The file is already open, but the Home tab is showing (no active document).
        app.open_path(&picked);
        app.active = None;
        app.pick_override = Some(vec![pick]);
        app
    });
    h.run_steps(2);
    h.state_mut().run_command("export.text");
    h.run_steps(3);
    let app = h.state();
    assert_eq!(app.views.len(), 1, "the existing tab is reused, not duplicated");
    assert_eq!(app.active, Some(0), "the pick focused the tab it already opened");
    assert!(matches!(app.dialog, Some(Dialog::Export(_))), "the tool still runs on it: {:?}", app.dialog);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn shortcuts_run_registered_commands() {
    let mut h = harness();
    h.key_press_modifiers(Modifiers::COMMAND, Key::D);
    h.run_steps(2);
    assert!(matches!(h.state().dialog, Some(Dialog::Properties(_))));
    h.state_mut().dialog = None;
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run_steps(2);
    assert!(h.state().palette_open);
    h.key_press(Key::Escape);
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::CTRL, Key::H);
    h.run_steps(2);
    assert_eq!(h.state().mode, pdfcraft_ui_egui::Mode::Read);
}

#[test]
fn the_palette_lists_commands_with_shortcuts_and_runs_them() {
    let mut h = harness();
    h.state_mut().set_option("palette", "rotate pages clockwise").unwrap();
    h.run_steps(3);
    h.get_by_label("Rotate pages clockwise");
    h.key_press(Key::Enter);
    h.run_steps(3);
    let app = h.state();
    assert!(!app.palette_open, "Enter runs the top hit and closes the palette");
    let doc = app.session.get(app.views[0].id).unwrap();
    assert_eq!(doc.info.pages[0].rotation, 90);
    assert_eq!(doc.can_undo(), Some("Rotate page"));
}

#[test]
fn palette_arrow_keys_select_results_and_enter_runs_the_selection() {
    let mut h = harness();
    h.state_mut().set_option("palette", "rotate pages").unwrap();
    h.run_steps(2);
    h.get_by_label("Rotate pages clockwise");
    h.get_by_label("Rotate pages counterclockwise");
    h.key_press(Key::ArrowDown);
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(2);
    let app = h.state();
    assert!(!app.palette_open);
    assert_eq!(app.session.get(app.views[0].id).unwrap().info.pages[0].rotation, 270);
}

#[test]
fn the_pages_menu_comes_from_the_registry() {
    let mut h = harness();
    h.get_by_label("Menu").click();
    h.run_steps(2);
    h.get_by_label("Pages ⏵").hover();
    h.run_steps(3);
    h.get_by_label_contains("Delete pages").click();
    h.run_steps(3);
    let app = h.state();
    assert_eq!(app.session.get(app.views[0].id).unwrap().info.pages.len(), 2);
}

#[test]
fn the_open_recent_menu_lists_files_and_opens_one() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-recent-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("recent.pdf");
    std::fs::write(&path, fixture(2)).unwrap();
    let path = path.to_string_lossy().into_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe({
        let path = path.clone();
        move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("doc.pdf", None, fixture(3)).unwrap();
            app.recent.push(pdfcraft_ui_egui::RecentFile { name: "recent.pdf".into(), path, pages: 2, size: 0 });
            app
        }
    });
    h.run_steps(4);
    h.get_by_label("Menu").click();
    h.run_steps(2);
    h.get_by_label("File ⏵").hover();
    h.run_steps(3);
    h.get_by_label("Open Recent ⏵").hover();
    h.run_steps(3);
    let original = h.state().views[0].id;
    h.get_by_label_contains("recent.pdf").click();
    support::wait_until(&mut h, "recent document", |app| app.views.len() == 2 && app.password_prompt.is_none());
    let app = h.state();
    let active = app.active.unwrap();
    assert_eq!(app.session.get(app.views[active].id).and_then(|d| d.path.as_deref()), Some(path.as_str()), "the active tab is the recent file");
    assert_eq!(app.session.get(app.views[active].id).unwrap().bytes.as_slice(), fixture(2).as_slice());
    assert!(app.views.iter().any(|view| view.id == original));
    assert_eq!(app.session.get(original).unwrap().bytes.as_slice(), fixture(3).as_slice());
}

#[test]
fn the_open_recent_menu_is_disabled_while_the_list_is_empty() {
    let mut h = harness();
    h.get_by_label("Menu").click();
    h.run_steps(2);
    h.get_by_label("File ⏵").hover();
    h.run_steps(3);
    assert!(
        h.query_by(|n| n.label().as_deref() == Some("Open Recent") && n.is_disabled()).is_some(),
        "Open Recent is disabled while no file has been opened"
    );
}

fn recent_file(name: &str) -> pdfcraft_ui_egui::RecentFile {
    pdfcraft_ui_egui::RecentFile { name: name.into(), path: format!("/nowhere/{name}"), pages: 1, size: 0 }
}

#[test]
fn clear_recent_files_at_the_foot_of_open_recent_empties_the_list() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, fixture(3)).unwrap();
        app.recent.extend([recent_file("first.pdf"), recent_file("second.pdf")]);
        app
    });
    h.run_steps(4);
    h.get_by_label("Menu").click();
    h.run_steps(2);
    h.get_by_label("File ⏵").hover();
    h.run_steps(3);
    h.get_by_label("Open Recent ⏵").hover();
    h.run_steps(3);
    h.get_by_label("Clear Recent Files").click();
    h.run_steps(3);
    assert!(h.state().recent.is_empty(), "the list is empty");
    h.get_by_label("Menu").click();
    h.run_steps(2);
    h.get_by_label("File ⏵").hover();
    h.run_steps(3);
    assert!(h.query_by(|n| n.label().as_deref() == Some("Open Recent") && n.is_disabled()).is_some(), "and Open Recent is disabled");
}

#[test]
fn clear_recent_files_runs_from_the_palette_and_is_saved() {
    let mut app = PdfCraftApp::new();
    app.recent.extend([recent_file("first.pdf"), recent_file("second.pdf")]);
    assert!(app.execute("file.clear_recent"));
    assert!(app.recent.is_empty());
    let mut restarted = PdfCraftApp::new();
    restarted.recent.push(recent_file("stale.pdf"));
    restarted.restore(&app.persist());
    assert!(restarted.recent.is_empty(), "the empty list is what's saved");
    app.execute("file.clear_recent");
    assert!(app.toast.as_ref().is_some_and(|(m, _)| m.contains("No recent files")), "nothing to clear is said, not silent");
}

#[test]
fn the_shortcuts_dialog_lists_the_real_bindings() {
    let mut h = harness();
    h.ctx.set_os(OperatingSystem::Nix);
    h.state_mut().execute("help.shortcuts");
    h.run_steps(3);
    h.get_by_label("Ctrl+Shift+S");
    h.get_by_label("Save as");
    // The keys the document view handles itself are written for the platform too, and Fit
    // visible (a registered command) is listed once.
    h.get_by_label("Ctrl+1");
    h.get_by_label("Ctrl+3");
    h.get_by_label("Ctrl+G / Ctrl+Shift+G");
    h.get_by_label("← / →, Ctrl+← / Ctrl+→");
    h.get_by_label("Zoom in / out (also pinch or Ctrl-scroll)");
    assert_eq!(h.query_all_by_label_contains("⌘").count(), 0, "no ⌘ outside macOS");
    h.ctx.set_os(OperatingSystem::Mac);
    h.run_steps(2);
    h.get_by_label("⇧⌘S");
    h.get_by_label("⌘1");
    h.get_by_label("Zoom in / out (also pinch or ⌘-scroll)");
}

#[test]
fn localized_shortcuts_update_immediately_and_keep_command_ids_and_key_bindings() {
    let mut h = harness();
    h.ctx.set_os(OperatingSystem::Windows);
    h.state_mut().set_option("language", "de").unwrap();
    h.state_mut().execute("help.shortcuts");
    h.run_steps(3);
    h.get_by_label("Strg+Umschalt+S");
    h.state_mut().set_option("language", "en").unwrap();
    h.run_steps(2);
    h.get_by_label("Ctrl+Shift+S");
    h.state_mut().set_option("language", "de").unwrap();
    h.ctx.set_os(OperatingSystem::Mac);
    h.run_steps(2);
    h.get_by_label("⇧⌘S");
    h.key_press(Key::Escape);
    h.run_steps(2);
    h.ctx.set_os(OperatingSystem::Windows);
    h.key_press_modifiers(Modifiers::CTRL | Modifiers::COMMAND, Key::K);
    h.run_steps(3);
    assert!(h.state().palette_open, "the localized Strg+K display still binds Ctrl+K");
    // Searching by the stable command id still works in a translated interface.
    h.state_mut().set_option("palette", "view.read_mode").unwrap();
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(h.state().mode, pdfcraft_ui_egui::Mode::Read);
    assert!(!h.state().palette_open);
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert!(!doc.dirty);
}

/// Menu ▸ View open, with egui told the app runs on `os`.
fn view_menu(os: OperatingSystem) -> Harness<'static, PdfCraftApp> {
    let mut h = harness();
    h.ctx.set_os(os);
    h.get_by_label("Menu").click();
    h.run_steps(2);
    h.get_by_label("View ⏵").hover();
    h.run_steps(3);
    h
}

#[test]
fn view_menu_and_tooltips_write_shortcuts_for_the_platform() {
    // On Linux, View ▸ Zoom and Page navigation showed ⌘1, ⌘[ … (hard-coded) while the
    // registered items under them showed Ctrl+K; tooltips showed ⌘ too.
    let h = view_menu(OperatingSystem::Nix);
    h.get_by_label("Actual size Ctrl+1");
    h.get_by_label("Zoom to page level Ctrl+0");
    h.get_by_label("Rotate view counterclockwise Ctrl+Shift+−");
    h.get_by_label("Previous view Ctrl+[");
    h.get_by_label("Find tools and commands… Ctrl+K");
    h.get_by_label("Zoom in (Ctrl++)");
    h.get_by_label("Print (Ctrl+P)");
    assert_eq!(h.query_all_by_label_contains("⌘").count(), 0, "no ⌘ outside macOS");
    assert_eq!(h.query_all_by_label_contains("Fit visible").count(), 1, "View lists Fit visible once");
    h.get_by_label("Fit visible Ctrl+3");

    let h = view_menu(OperatingSystem::Mac);
    h.get_by_label("Actual size ⌘1");
    h.get_by_label("Fit visible ⌘3");
    h.get_by_label("Rotate view clockwise (⇧⌘+)");
    assert_eq!(h.query_all_by_label_contains("Ctrl+").count(), 0, "no Ctrl+ on macOS");
}

#[test]
fn the_top_bar_save_button_names_its_shortcut_not_a_milestone() {
    // #962: on Windows the Save button's tooltip and accessibility name read "Save (M4)",
    // an internal milestone tag, while Print and Document properties named their shortcuts.
    let mut h = harness();
    h.ctx.set_os(OperatingSystem::Windows);
    h.run_steps(2);
    h.get_by_label("Save (Ctrl+S)");
    h.get_by_label("Print (Ctrl+P)");
    assert_eq!(h.query_all_by_label_contains("(M4)").count(), 0, "no milestone tag in the top bar");
    assert_eq!(h.query_all_by_label_contains("⌘").count(), 0, "no ⌘ outside macOS");

    let mut h = harness();
    h.ctx.set_os(OperatingSystem::Mac);
    h.run_steps(2);
    h.get_by_label("Save (⌘S)");
}

#[test]
fn full_screen_toggles_by_shortcut_and_command_and_escape_leaves() {
    let mut h = harness();
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(2);
    assert!(h.state().full_screen, "⌘L / Ctrl+L enters full screen");
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(!h.state().full_screen, "Escape leaves full screen");
    h.state_mut().execute("view.full_screen");
    h.run_steps(2);
    assert!(h.state().full_screen);
    h.state_mut().execute("view.full_screen");
    h.run_steps(2);
    assert!(!h.state().full_screen, "the command toggles back");
}

#[test]
fn the_about_dialog_shows_the_running_version() {
    let mut h = harness();
    h.state_mut().execute("help.about");
    h.run_steps(3);
    h.get_by_label_contains(&format!("Version {}", env!("CARGO_PKG_VERSION")));
}

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
                assert!(h.state_mut().execute("page.insert_blank"));
                h.run_steps(2);
            }
            h.event(egui::Event::Key { key: Key::Q, physical_key: None, pressed: true, repeat: false, modifiers: modifiers | extra });
            h.step();
            let closes = h.output().viewport_output.values().any(|v| v.commands.contains(&egui::ViewportCommand::Close));
            assert_eq!(closes, should_close, "{os:?}, extra={extra:?}, dirty={dirty}");
            if dirty {
                h.get_by_label("Cancel").click();
                h.run_steps(2);
                assert_eq!(h.state().views.len(), 1);
                assert!(h.state().session.get(h.state().views[0].id).unwrap().dirty);
                assert!(!h.output().viewport_output.values().any(|v| v.commands.contains(&egui::ViewportCommand::Close)));
            }
        }
    }
}

#[test]
fn command_w_closes_empty_macos_window_but_not_an_empty_windows_window() {
    for (os, should_close) in [(OperatingSystem::Mac, true), (OperatingSystem::Windows, false)] {
        let mut h = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(|_cc| PdfCraftApp::new());
        h.ctx.set_os(os);
        h.run_steps(4);
        assert!(h.state().views.is_empty());
        h.event(egui::Event::Key { key: Key::W, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND });
        h.step();
        let closes = h.output().viewport_output.values().any(|v| v.commands.iter().any(|c| matches!(c, egui::ViewportCommand::Close)));
        assert_eq!(closes, should_close, "{os:?}: only an empty macOS window should close");
    }
}

#[test]
fn command_w_still_closes_the_document_tab_on_macos() {
    let mut h = harness();
    h.ctx.set_os(OperatingSystem::Mac);
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::COMMAND, Key::W);
    h.run_steps(2);
    assert!(h.state().views.is_empty(), "⌘W closes the clean document tab");
}

fn palette_ime_output_requests_interrupt(h: &Harness<'_, PdfCraftApp>, phase: &str) -> bool {
    assert!(h.state().palette_open, "palette closed during {phase}");
    let ime = h.output().platform_output.ime.as_ref().expect("focused palette advertises IME");
    eprintln!("{phase}: query={:?}; should_interrupt_composition={}", h.state().palette_query, ime.should_interrupt_composition);
    ime.should_interrupt_composition
}

fn palette_ime_output_is_uninterrupted(h: &Harness<'_, PdfCraftApp>, phase: &str) {
    assert!(!palette_ime_output_requests_interrupt(h, phase), "palette interrupted composition during {phase}");
}

#[test]
fn palette_ime_idle_frames_and_reopen_do_not_interrupt_composition() {
    let mut h = harness();
    for opening in 0..2 {
        h.state_mut().run_command("view.palette");
        h.run_steps(3);
        for frame in 0..3 {
            h.step();
            palette_ime_output_is_uninterrupted(&h, &format!("opening {opening}, idle {frame}"));
        }
        h.key_press(Key::Escape);
        h.run_steps(2);
        assert!(!h.state().palette_open);
    }
}

#[test]
fn palette_ime_chinese_preedit_commit_and_cancel_remain_uninterrupted() {
    let mut h = harness();
    let mut interruptions = Vec::new();
    h.state_mut().run_command("view.palette");
    h.run_steps(3);
    for preedit in ["ni", "nih"] {
        h.event(egui::Event::Ime(egui::ImeEvent::Preedit { text: preedit.into(), active_range_chars: Some(0..preedit.chars().count()) }));
        h.step();
        interruptions.push(palette_ime_output_requests_interrupt(&h, preedit));
        h.step();
        interruptions.push(palette_ime_output_requests_interrupt(&h, "preedit idle"));
    }
    h.event(egui::Event::Ime(egui::ImeEvent::Commit("你".into())));
    h.step();
    assert_eq!(h.state().palette_query, "你");
    interruptions.push(palette_ime_output_requests_interrupt(&h, "Chinese commit"));
    h.run_steps(2);
    assert_eq!(h.state().palette_query, "你");
    interruptions.push(palette_ime_output_requests_interrupt(&h, "committed idle"));
    h.event(egui::Event::Ime(egui::ImeEvent::Preedit { text: "hao".into(), active_range_chars: Some(0..3) }));
    h.step();
    interruptions.push(palette_ime_output_requests_interrupt(&h, "second preedit"));
    h.event(egui::Event::Ime(egui::ImeEvent::Preedit { text: String::new(), active_range_chars: None }));
    h.step();
    assert_eq!(h.state().palette_query, "你");
    interruptions.push(palette_ime_output_requests_interrupt(&h, "cancel preedit"));
    assert!(interruptions.iter().all(|interrupt| !interrupt), "composition interrupted: {interruptions:?}");
}

#[test]
fn palette_ime_outside_click_closes_without_running_a_command() {
    let mut h = harness();
    h.state_mut().set_option("palette", "rotate pages clockwise").unwrap();
    h.run_steps(3);
    h.event(egui::Event::PointerButton {
        pos: egui::pos2(20.0, 850.0),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    h.step();
    assert!(!h.state().palette_open);
    assert!(h.state().palette_query.is_empty());
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().info.pages[0].rotation, 0);
}

/// Ctrl+Tab and Ctrl+Shift+Tab step through the tabs in the strip's order, wrap around at either
/// end and include the Combine files tab after the documents (#1542). A Windows or Linux Ctrl
/// press is both `ctrl` and `command`.
#[test]
fn ctrl_tab_steps_through_the_tabs_in_order_and_wraps_around() {
    let mut h = harness();
    h.state_mut().open_bytes("second.pdf", None, fixture(1)).unwrap();
    h.state_mut().open_bytes("third.pdf", None, fixture(1)).unwrap();
    h.run_steps(3);
    let ctrl = Modifiers::CTRL | Modifiers::COMMAND;
    let press = |h: &mut Harness<'static, PdfCraftApp>, modifiers: Modifiers| {
        h.key_press_modifiers(modifiers, Key::Tab);
        h.run_steps(2);
        h.state().active
    };
    assert_eq!(h.state().active, Some(2));
    assert_eq!(press(&mut h, ctrl), Some(0), "the last tab wraps to the first");
    assert_eq!(press(&mut h, ctrl), Some(1));
    assert_eq!(press(&mut h, ctrl | Modifiers::SHIFT), Some(0));
    assert_eq!(press(&mut h, ctrl | Modifiers::SHIFT), Some(2), "the first tab wraps to the last");
    // The order is the one on screen, after the tabs are moved.
    let ids: Vec<_> = h.state().views.iter().map(|v| v.id).collect();
    h.state_mut().move_tab(2, 0);
    h.run_steps(2);
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[2]));
    press(&mut h, ctrl);
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[0]));
    // Combine files comes after the documents.
    h.state_mut().execute("page.combine");
    h.run_steps(2);
    assert!(h.state().combine_showing());
    assert_eq!(press(&mut h, ctrl), Some(0), "Combine files wraps to the first document");
    assert_eq!(press(&mut h, ctrl | Modifiers::SHIFT), None);
    assert!(h.state().combine_showing(), "back from the first document is Combine files");
    // Plain Tab still belongs to keyboard focus, not to the tabs.
    press(&mut h, Modifiers::NONE);
    assert!(h.state().combine_showing());
    for doc in h.state().session.docs() {
        assert!(!doc.dirty, "switching tabs changes no document");
    }
}
