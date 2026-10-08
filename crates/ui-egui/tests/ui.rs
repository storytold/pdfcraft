//! Headless UI tests (egui_kittest + AccessKit). They drive the real app shell without a window.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

/// A tiny PDF with two pages, two bookmarks and one sticky note.
const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /Outlines 6 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Annots [9 0 R] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >> endobj
6 0 obj << /Type /Outlines /First 7 0 R /Last 8 0 R /Count 2 >> endobj
7 0 obj << /Title (Alpha section) /Parent 6 0 R /Next 8 0 R /Dest [3 0 R /Fit] >> endobj
8 0 obj << /Title (Beta section) /Parent 6 0 R /Prev 7 0 R /Dest [4 0 R /Fit] >> endobj
9 0 obj << /Type /Annot /Subtype /Text /Rect [10 370 30 390] /T (Tester) /Contents (Check the numbers) >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        setup(&mut app);
        app
    });
    // Fonts install on frame 1 and apply on frame 2.
    h.run_steps(4);
    h
}

#[test]
fn home_shows_welcome_and_tools() {
    let h = harness(|_| {});
    h.get_by_label_contains("Welcome to PdfCraft");
    assert!(h.query_all_by_label("Organize pages").count() >= 2, "tool list + home card");
    h.get_by_label("Open file");
}

#[test]
fn every_catalog_tool_is_listed_after_view_more() {
    let mut h = harness(|_| {});
    h.get_by_label("View more").click();
    h.run_steps(3);
    for g in pdfcraft_engine::catalog::TOOL_GROUPS {
        assert!(h.query_all_by_label(g.label).count() >= 1, "tool {} missing from All tools", g.label);
    }
}

#[test]
fn opening_a_pdf_shows_comments_and_bookmarks() {
    let mut h = harness(|app| app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    // Documents with comments open on the Comments panel, like Acrobat.
    h.get_by_label_contains("Check the numbers");
    h.get_by_label("Bookmarks").click();
    h.run_steps(3);
    h.get_by_label("Alpha section");
    h.get_by_label("Beta section");
}

/// A right-to-left file name opens, lays out and paints; the tab's accessible name keeps the
/// logical text (only the painted label is put in visual order).
#[test]
fn a_tab_with_an_arabic_file_name_keeps_its_logical_accessible_name() {
    let name = "واحد اثنين.pdf";
    let mut h = harness(move |app| app.open_bytes(name, None, FIXTURE.to_vec()).expect("fixture opens"));
    let tab = h.get_by_label(name).rect();
    assert!(tab.width() > 64.0 && tab.height() > 0.0, "{tab:?}");
    // A name longer than the tab's 28-character limit is cut on a character boundary.
    let long = "واحد اثنين ثلاثة أربعة خمسة ستة سبعة ثمانية.pdf";
    h.state_mut().open_bytes(long, None, FIXTURE.to_vec()).expect("fixture opens");
    h.run_steps(3);
    h.get_by_label(long);
}

#[test]
fn garbage_input_is_rejected_without_panicking() {
    let mut app = PdfCraftApp::new();
    assert!(app.open_bytes("junk.pdf", None, b"this is not a pdf".to_vec()).is_err());
    assert!(app.open_bytes("empty.pdf", None, Vec::new()).is_err());
    let mut truncated = FIXTURE.to_vec();
    truncated.truncate(FIXTURE.len() / 3);
    // A truncated file either repairs or fails cleanly; it must not panic.
    let _ = app.open_bytes("truncated.pdf", None, truncated);
}

#[test]
fn theme_and_view_options_apply() {
    let mut h = harness(|app| {
        app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens");
        app.set_option("theme", "dark").unwrap();
        app.set_option("panel", "pages").unwrap();
        app.set_option("page", "2").unwrap();
    });
    h.run_steps(3);
    h.get_by_label("Page 2");
    assert!(h.state_mut().set_option("panel", "nope").is_err());
}

/// Two pages of real text.
const TEXT_FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 5 0 R /Resources << /Font << /F1 7 0 R >> >> >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 6 0 R /Resources << /Font << /F1 7 0 R >> >> >> endobj
5 0 obj << /Length 55 >> stream
BT /F1 14 Tf 20 150 Td (The quick brown fox) Tj ET
endstream endobj
6 0 obj << /Length 58 >> stream
BT /F1 14 Tf 20 150 Td (A second brown animal) Tj ET
endstream endobj
7 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    for _ in 0..200 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn find_counts_matches_across_pages() {
    let mut h = harness(|app| {
        app.open_bytes("text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
        app.set_option("find", "brown").unwrap();
    });
    settle(&mut h);
    h.get_by_label_contains("of 2");
    let v = &h.state().views[0];
    let f = v.find.as_ref().expect("find open");
    assert_eq!(f.matches.iter().map(|(p, _)| *p).collect::<Vec<_>>(), vec![0, 1]);
}

#[test]
fn find_reports_no_matches() {
    let mut h = harness(|app| {
        app.open_bytes("text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
        app.set_option("find", "zebra").unwrap();
    });
    settle(&mut h);
    h.get_by_label("No matches");
}

#[test]
fn drag_selects_text_and_copy_returns_it() {
    let mut h = harness(|app| {
        app.open_bytes("text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
    });
    settle(&mut h);
    // "The quick brown fox": glyph 4 = 'q', glyph 14 = 'n' of "brown".
    let (a, b) = {
        let v = &h.state().views[0];
        (v.glyph_screen_pos(0, 4).expect("text layer loaded"), v.glyph_screen_pos(0, 14).expect("glyph"))
    };
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    h.hover_at(a + (b - a) * 0.5);
    h.run_steps(1);
    h.hover_at(b);
    h.run_steps(2);
    h.drop_at(b);
    h.run_steps(2);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick brown"));
}

#[test]
fn persistence_round_trips_and_tolerates_garbage() {
    let mut a = PdfCraftApp::new();
    a.set_option("theme", "dark").unwrap();
    let json = a.persist();
    let mut b = PdfCraftApp::new();
    b.restore(&json);
    assert_eq!(b.theme, pdfcraft_ui_egui::theme::ThemeKind::Dark);
    b.restore("{not json");
    b.restore("{\"recent\": 5, \"theme\": \"Purple\"}");
    assert_eq!(b.theme, pdfcraft_ui_egui::theme::ThemeKind::Dark);
}

#[test]
fn zoom_keeps_the_point_under_the_cursor_still() {
    let mut h = harness(|app| {
        app.open_bytes("text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
    });
    settle(&mut h);
    let before = h.state().views[0].glyph_screen_pos(0, 4).expect("glyph on screen");
    let z = h.state().views[0].zoom;
    h.state_mut().views[0].zoom_at(z * 2.0, before);
    h.run_steps(4);
    let after = h.state().views[0].glyph_screen_pos(0, 4).expect("glyph still on screen");
    assert!((after - before).length() < 2.0, "moved from {before:?} to {after:?}");
}

#[test]
fn selection_works_in_every_view_rotation() {
    for deg in ["90", "180", "270"] {
        let mut h = harness(move |app| {
            app.open_bytes("text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
            app.set_option("left", "closed").unwrap();
            app.set_option("rotate", deg).unwrap();
            // Keep the whole rotated page on screen.
            app.set_option("zoom", "60").unwrap();
        });
        settle(&mut h);
        let (a, b) = {
            let v = &h.state().views[0];
            (v.glyph_screen_pos(0, 4).expect("glyph"), v.glyph_screen_pos(0, 14).expect("glyph"))
        };
        h.hover_at(a);
        h.run_steps(1);
        h.drag_at(a);
        h.run_steps(1);
        h.hover_at(a + (b - a) * 0.5);
        h.run_steps(1);
        h.hover_at(b);
        h.run_steps(2);
        h.drop_at(b);
        h.run_steps(2);
        assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick brown"), "rotation {deg}");
    }
}

/// A file dropped on the window, as the windowing layer hands it over.
#[derive(Debug)]
struct Dropped {
    path: std::path::PathBuf,
    bytes: Vec<u8>,
}

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.path
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        Ok(self.bytes.clone())
    }
}

#[test]
fn dropping_a_pdf_on_the_window_opens_it() {
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_cc| PdfCraftApp::new());
    h.run_steps(3);
    assert!(h.state().views.is_empty());
    let file: egui::DroppedFileHandle = std::sync::Arc::new(Dropped { path: "dropped.pdf".into(), bytes: FIXTURE.to_vec() });
    h.input_mut().dropped_files.push(file);
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 1, "the dropped PDF opens in a tab");
    h.get_by_label_contains("dropped.pdf");
}

#[test]
fn files_and_quit_from_the_operating_system() {
    // #73: macOS hands Finder double-clicks, Open With and Dock drops over as Apple events.
    use pdfcraft_ui_egui::OsEvent;
    let name = format!("pdfcraft-os-open-{}.pdf", std::process::id());
    let path = std::env::temp_dir().join(&name);
    std::fs::write(&path, FIXTURE).unwrap();
    let queue = std::rc::Rc::new(std::cell::RefCell::new(vec![OsEvent::Open(vec![path.to_string_lossy().into_owned()])]));
    let q = queue.clone();
    let mut h = harness(move |app| app.os_events = Some(Box::new(move || q.borrow_mut().drain(..).collect())));
    std::fs::remove_file(&path).ok();
    assert_eq!(h.state().views.len(), 1, "the file opens in a tab");
    h.get_by_label_contains(&name);
    // Quit closes the window like the close button, so unsaved changes are asked about.
    queue.borrow_mut().push(OsEvent::Quit);
    h.step();
    let closing = h.output().viewport_output.values().any(|v| v.commands.iter().any(|c| matches!(c, egui::ViewportCommand::Close)));
    assert!(closing);
}

#[test]
fn default_workspace_mode_persists_and_tolerates_invalid_settings() {
    use pdfcraft_ui_egui::Mode;
    for mode in [Mode::AllTools, Mode::Read, Mode::Edit, Mode::Convert, Mode::Sign] {
        let mut app = PdfCraftApp::new();
        app.default_mode = mode;
        app.set_option("mode", "sign").unwrap();
        let mut restored = PdfCraftApp::new();
        restored.restore(&app.persist());
        assert_eq!(restored.default_mode, mode);
        restored.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).unwrap();
        assert_eq!(restored.mode, mode, "session override must not be persisted");
    }
    for json in ["{}", "{not json", r#"{"default_mode": null}"#, r#"{"default_mode": 5}"#, r#"{"default_mode": "unknown"}"#] {
        let mut app = PdfCraftApp::new();
        app.restore(json);
        assert_eq!(app.default_mode, Mode::AllTools);
        app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).unwrap();
        assert_eq!(app.mode, Mode::AllTools);
    }
}

#[test]
fn newly_opened_pdfs_use_the_default_workspace() {
    use pdfcraft_ui_egui::{LeftPanel, Mode};
    let mut app = PdfCraftApp::new();
    app.restore(r#"{"default_mode": "edit"}"#);
    app.open_bytes("first.pdf", None, FIXTURE.to_vec()).unwrap();
    assert_eq!(app.mode, Mode::Edit);
    assert_eq!(app.left, LeftPanel::Tool("edit"));
    app.default_mode = Mode::Read;
    assert_eq!(app.mode, Mode::Edit, "changing the preference affects future opens");
    app.open_bytes("second.pdf", None, FIXTURE.to_vec()).unwrap();
    assert_eq!(app.mode, Mode::Read);
    // PDF Initial View is independent of the workspace preference.
    let pdf =
        String::from_utf8(FIXTURE.to_vec()).unwrap().replace("/Outlines 6 0 R", "/Outlines 6 0 R /PageLayout /TwoColumnLeft /PageMode /UseOutlines");
    app.open_bytes("initial-view.pdf", None, pdf.into_bytes()).unwrap();
    assert_eq!(app.mode, Mode::Read);
    assert_eq!(app.views.last().unwrap().layout, pdfcraft_ui_egui::canvas::PageLayout::TwoUp);
    assert_eq!(app.right, Some(pdfcraft_ui_egui::RightPanel::Bookmarks));
}

#[test]
fn explicit_mode_overrides_default_before_and_after_open() {
    use pdfcraft_ui_egui::Mode;
    for before in [false, true] {
        for (value, mode) in [("all", Mode::AllTools), ("read", Mode::Read), ("edit", Mode::Edit), ("convert", Mode::Convert), ("sign", Mode::Sign)] {
            let mut app = PdfCraftApp::new();
            app.default_mode = Mode::Read;
            if before {
                app.set_option("mode", value).unwrap();
            }
            app.open_bytes("first.pdf", None, FIXTURE.to_vec()).unwrap();
            if !before {
                // Desktop startup applies CLI options after the initial files open.
                app.set_option("mode", value).unwrap();
            }
            assert_eq!(app.mode, mode);
            app.set_option("default-mode", "edit").unwrap();
            app.open_bytes("second.pdf", None, FIXTURE.to_vec()).unwrap();
            assert_eq!(app.mode, mode);
            assert_eq!(app.default_mode, Mode::Edit);
        }
    }
}

#[test]
fn preferences_selects_default_workspace_for_next_open() {
    use egui::accesskit::Role;
    use pdfcraft_ui_egui::Mode;
    let mut h = harness(|app| app.set_option("dialog", "preferences").unwrap());
    h.get_by_label("Default workspace mode");
    h.get_by_role_and_label(Role::RadioButton, "Read").click();
    h.run_steps(2);
    assert_eq!(h.state().default_mode, Mode::Read);
    assert_eq!(h.state().mode, Mode::AllTools);
    h.get_by_label("OK").click();
    h.run_steps(2);
    h.state_mut().open_bytes("fixture.pdf", None, FIXTURE.to_vec()).unwrap();
    assert_eq!(h.state().mode, Mode::Read);
}

#[test]
fn explicit_mode_preserves_independent_tool_and_panel_options() {
    use pdfcraft_ui_egui::LeftPanel;
    let mut app = PdfCraftApp::new();
    app.set_option("tool", "export").unwrap();
    app.set_option("left", "closed").unwrap();
    app.set_option("mode", "edit").unwrap();
    assert_eq!(app.left, LeftPanel::Tool("export"));
    assert!(!app.left_open);
    app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).unwrap();
    assert_eq!(app.left, LeftPanel::Tool("export"));
    assert!(!app.left_open);
}

#[test]
fn opening_a_pdf_keeps_a_closed_left_panel_and_the_chosen_tool() {
    use pdfcraft_ui_egui::{LeftPanel, Mode};
    // `--left closed` without `--mode`, default workspace Edit: the panel stays closed.
    let mut app = PdfCraftApp::new();
    app.default_mode = Mode::Edit;
    app.set_option("left", "closed").unwrap();
    app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).unwrap();
    assert_eq!(app.mode, Mode::Edit);
    assert!(!app.left_open);
    // Default All Tools: opening another PDF leaves the tool panel the user picked.
    let mut app = PdfCraftApp::new();
    app.set_option("tool", "export").unwrap();
    app.open_bytes("first.pdf", None, FIXTURE.to_vec()).unwrap();
    app.open_bytes("second.pdf", None, FIXTURE.to_vec()).unwrap();
    assert_eq!(app.mode, Mode::AllTools);
    assert_eq!(app.left, LeftPanel::Tool("export"));
}
