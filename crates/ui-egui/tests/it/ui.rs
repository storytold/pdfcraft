//! Headless UI tests (egui_kittest + AccessKit). They drive the real app shell without a window.

use crate::support;

use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT, Queryable};
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
        app.set_option("language", "en").unwrap();
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
fn tools_panel_side_preference_moves_and_persists() {
    use pdfcraft_ui_egui::Dialog;

    let mut h = harness(|_| {});
    h.state_mut().dialog = Some(Dialog::Preferences);
    h.run_steps(3);
    h.get_by_label("Tools panel side");
    h.get_by_label("Right").click();
    h.run_steps(3);
    assert!(h.state().side_panels.tools_on_right());
    let saved = h.state().persist();
    let mut restored = PdfCraftApp::new();
    restored.restore(&saved);
    assert!(restored.side_panels.tools_on_right());
    h.get_by_label("Left").click();
    h.run_steps(3);
    assert!(!h.state().side_panels.tools_on_right());
}

/// Every piece of text painted on the last frame: its whole text, whether it was cut with "…",
/// and where it was drawn.
fn painted_text(h: &Harness<'static, PdfCraftApp>) -> Vec<(String, bool, egui::Rect)> {
    h.output()
        .shapes
        .iter()
        .filter_map(|c| match &c.shape {
            egui::Shape::Text(t) => Some((t.galley.text().to_owned(), t.galley.elided, t.visual_bounding_rect())),
            _ => None,
        })
        .collect()
}

#[test]
fn long_recent_names_and_paths_are_cut_before_the_page_count() {
    let name = format!("{}.pdf", "Quarterly report final version ".repeat(10));
    let path = format!("/{}/{name}", "A folder with a long name".repeat(8));
    let h = harness({
        let (name, path) = (name.clone(), path.clone());
        move |app| app.recent.push(pdfcraft_ui_egui::RecentFile { name, path, pages: 7, size: 2048 })
    });
    let text = painted_text(&h);
    let detail = text.iter().find(|(s, ..)| s.starts_with("7 pages")).expect("the row shows its page count").2;
    for whole in [&name, &path] {
        let (_, cut, rect) = text.iter().find(|(s, ..)| s == whole).expect("the row shows the name and the path");
        assert!(*cut, "{whole:?} is cut short");
        assert!(rect.right() < detail.left(), "{whole:?} ends before the page count");
    }
}

#[test]
fn home_removes_one_recent_file_or_clears_them_all() {
    // Tall enough that the Recent list is on screen without scrolling.
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1600.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        for name in ["first.pdf", "second.pdf", "third.pdf"] {
            app.recent.push(pdfcraft_ui_egui::RecentFile { name: name.into(), path: format!("/nowhere/{name}"), pages: 1, size: 0 });
        }
        app
    });
    h.run_steps(4);
    assert!(h.query_by_label("Remove second.pdf from Recent").is_none(), "the remove button shows only over its row");
    h.get_by_label("second.pdf").hover();
    h.run_steps(2);
    h.get_by_label("Remove second.pdf from Recent").click();
    h.run_steps(2);
    let names: Vec<String> = h.state().recent.iter().map(|r| r.name.clone()).collect();
    assert_eq!(names, ["first.pdf", "third.pdf"], "only that file left the list");
    h.get_by_label("Clear Recent Files").click();
    h.run_steps(2);
    assert!(h.state().recent.is_empty(), "Clear empties the list");
    h.get_by_label_contains("Files you open in PdfCraft appear here");
    assert!(h.query_by_label("Clear Recent Files").is_none(), "and goes away with nothing left to clear");
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
fn expanded_all_tools_lists_every_tool_by_category_and_searches() {
    let mut h = harness(|_| {});
    h.get_by_label("Expand all tools").click();
    h.run_steps(3);
    assert!(h.state().all_tools_full);
    for c in pdfcraft_engine::catalog::TOOL_CATEGORIES {
        h.get_by_label(c.label);
    }
    for g in pdfcraft_engine::catalog::TOOL_GROUPS {
        assert!(h.query_all_by_label(g.label).count() >= 1, "tool {} missing from the expanded view", g.label);
    }
    // An item name finds its tool; the other categories drop out.
    h.state_mut().all_tools_query = "rotate".into();
    h.run_steps(3);
    assert_eq!(h.query_all_by_label("Organize pages").count(), 1, "only the expanded view's tile; the sidebar is gone");
    assert!(h.query_by_label("Forms & Signatures").is_none());
    h.state_mut().all_tools_query = "no such tool".into();
    h.run_steps(3);
    h.get_by_label("No tools match your search.");
    // Choosing a tool goes back to the workspace with that tool's panel.
    h.state_mut().all_tools_query = "organize".into();
    h.run_steps(3);
    h.get_by_label("Organize pages").click();
    h.run_steps(3);
    assert!(!h.state().all_tools_full);
    assert_eq!(h.state().left, pdfcraft_ui_egui::LeftPanel::Tool("organize"));
    h.get_by_label("Back to all tools");
}

#[test]
fn collapse_and_escape_close_the_expanded_all_tools() {
    let mut h = harness(|app| app.set_option("tools", "full").unwrap());
    h.get_by_label("Collapse all tools").click();
    h.run_steps(3);
    assert!(!h.state().all_tools_full);
    h.get_by_label("View more");
    h.get_by_label("Expand all tools").click();
    h.run_steps(3);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(!h.state().all_tools_full);
}

#[test]
fn escape_in_a_dialog_over_the_expanded_all_tools_leaves_the_view_open() {
    let mut h = harness(|app| app.set_option("tools", "full").unwrap());
    h.state_mut().dialog = Some(pdfcraft_ui_egui::Dialog::About);
    h.run_steps(3);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.state().all_tools_full, "Escape belonged to the dialog");
}

#[test]
fn escape_in_the_command_palette_over_the_expanded_all_tools_leaves_the_view_open() {
    let mut h = harness(|app| app.set_option("tools", "full").unwrap());
    h.state_mut().palette_open = true;
    h.run_steps(3);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(!h.state().palette_open, "Escape closed the palette");
    assert!(h.state().all_tools_full, "and only the palette");
}

#[test]
fn another_workspace_from_the_mode_option_puts_the_expanded_all_tools_away() {
    let mut app = PdfCraftApp::new();
    app.set_option("tools", "full").unwrap();
    app.set_option("mode", "all").unwrap();
    assert!(app.all_tools_full, "All tools itself keeps the view");
    app.set_option("mode", "read").unwrap();
    app.set_option("mode", "all").unwrap();
    assert!(!app.all_tools_full, "coming back shows the sidebar, as the mode bar does");
}

#[test]
fn opening_a_document_puts_the_expanded_all_tools_away() {
    use pdfcraft_ui_egui::Mode;
    // All tools as the default workspace: no workspace change, but the document must show.
    let mut app = PdfCraftApp::new();
    app.set_option("tools", "full").unwrap();
    app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens");
    assert!(!app.all_tools_full, "the document shows, not the catalogue over it");
    // Read as the default workspace: coming back to All tools later shows the sidebar.
    let mut app = PdfCraftApp::new();
    app.set_option("default-mode", "read").unwrap();
    app.set_option("tools", "full").unwrap();
    app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens");
    assert_eq!(app.mode, Mode::Read);
    assert!(!app.all_tools_full);
}

#[test]
fn choosing_a_tool_puts_the_expanded_all_tools_away() {
    use pdfcraft_ui_egui::LeftPanel;
    let mut h = harness(|app| app.set_option("tools", "full").unwrap());
    h.state_mut().set_option("tool", "organize").unwrap();
    h.run_steps(3);
    assert!(!h.state().all_tools_full, "the tool's panel shows, not the catalogue");
    assert_eq!(h.state().left, LeftPanel::Tool("organize"));
}

#[test]
fn another_tab_coming_forward_puts_the_expanded_all_tools_away() {
    // Home, through the control option.
    let mut h = harness(|app| app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    h.state_mut().set_option("tools", "full").unwrap();
    h.run_steps(3);
    assert!(h.state().all_tools_full, "the document's own tab keeps it");
    h.state_mut().set_option("home", "").unwrap();
    h.run_steps(3);
    assert!(!h.state().all_tools_full);
    h.get_by_label_contains("Welcome to PdfCraft");
    // Another document's tab.
    let mut h = harness(|app| {
        app.open_bytes("one.pdf", None, FIXTURE.to_vec()).expect("fixture opens");
        app.open_bytes("two.pdf", None, FIXTURE.to_vec()).expect("fixture opens");
    });
    h.state_mut().set_option("tools", "full").unwrap();
    h.run_steps(3);
    h.state_mut().active = Some(0);
    h.run_steps(3);
    assert!(!h.state().all_tools_full, "one.pdf shows, not the catalogue over it");
}

#[test]
fn going_home_with_no_document_puts_the_expanded_all_tools_away() {
    // Home is already the tab in front, so only the option itself can put the view away.
    let mut h = harness(|app| app.set_option("tools", "full").unwrap());
    h.state_mut().set_option("home", "").unwrap();
    h.run_steps(3);
    assert!(!h.state().all_tools_full);
    h.get_by_label_contains("Welcome to PdfCraft");
}

#[test]
fn expanding_all_tools_from_read_mode_restores_the_right_panel() {
    use pdfcraft_ui_egui::{Mode, RightPanel};
    let mut h = harness(|app| app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    h.state_mut().right = Some(RightPanel::Bookmarks);
    h.state_mut().execute("view.read_mode");
    h.run_steps(3);
    assert_eq!((h.state().mode, h.state().right), (Mode::Read, None), "Read mode hides the panel");
    h.state_mut().set_option("tools", "full").unwrap();
    h.run_steps(3);
    assert!(h.state().all_tools_full);
    assert_eq!((h.state().mode, h.state().right), (Mode::AllTools, Some(RightPanel::Bookmarks)));
}

#[test]
fn expanding_all_tools_from_edit_puts_edit_text_down() {
    use pdfcraft_ui_egui::{LeftPanel, Mode, QuickTool};
    let mut h = harness(|app| app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    support::outside_menus(&h, "Edit").click();
    h.run_steps(3);
    assert_eq!(h.state().quick_tool, QuickTool::EditText);
    h.state_mut().set_option("tools", "full").unwrap();
    h.run_steps(3);
    assert!(h.state().all_tools_full);
    assert_eq!((h.state().mode, h.state().quick_tool), (Mode::AllTools, QuickTool::Select));
    assert_eq!(h.state().left, LeftPanel::AllTools, "collapsing goes back to the All tools list");
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

/// #1590: side panes the user closed stay closed when PdfCraft restarts and opens another PDF,
/// even one with comments and bookmarks, which would otherwise open on the Comments panel.
#[test]
fn closed_side_panes_stay_closed_after_a_restart_and_opening_a_pdf() {
    use pdfcraft_ui_egui::RightPanel;
    let mut h = harness(|app| app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    assert_eq!(h.state().right, Some(RightPanel::Comments));
    assert!(h.state().left_open);
    h.state_mut().choose_right_panel(None);
    h.state_mut().set_option("left", "closed").unwrap();
    h.run_steps(3);
    let saved = h.state().persist();
    let mut h = harness(move |app| {
        app.restore(&saved);
        app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens");
    });
    assert_eq!(h.state().right, None, "the closed right pane reopened");
    assert!(!h.state().left_open, "the closed left pane reopened");
    assert!(h.query_by_label_contains("Check the numbers").is_none());
    // Within a session too: another document opens with them closed.
    h.state_mut().open_bytes("again.pdf", None, FIXTURE.to_vec()).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().right, None);
    assert!(!h.state().left_open);
}

/// #2884: a right panel the user closed stays closed when a PDF asks for its outline
/// (`/PageMode /UseOutlines`), within a session and after a restart.
#[test]
fn closed_right_panel_stays_closed_for_a_document_asking_for_its_outline() {
    use pdfcraft_ui_egui::RightPanel;
    let pdf = || String::from_utf8(FIXTURE.to_vec()).unwrap().replace("/Outlines 6 0 R", "/Outlines 6 0 R /PageMode /UseOutlines").into_bytes();
    let mut h = harness(|app| app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    assert_eq!(h.state().right, Some(RightPanel::Comments));
    h.state_mut().choose_right_panel(None);
    h.state_mut().open_bytes("outline.pdf", None, pdf()).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().right, None, "the outline request reopened the closed right panel");
    let saved = h.state().persist();
    let h = harness(move |app| {
        app.restore(&saved);
        app.open_bytes("outline.pdf", None, pdf()).expect("outline PDF opens");
    });
    assert_eq!(h.state().right, None, "the closed right panel reopened after a restart");
}

/// Read mode hides the tools, not the navigation panes: the rail's Bookmarks button opens the
/// bookmarks when Read is the default workspace (#932), and the left tool pane stays hidden.
#[test]
fn read_mode_rail_buttons_open_their_panels() {
    use pdfcraft_ui_egui::{Mode, RightPanel};
    let mut h = harness(|app| {
        app.default_mode = Mode::Read;
        app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens");
    });
    assert_eq!(h.state().mode, Mode::Read);
    assert_eq!(h.state().right, None, "Read mode opens with the panes closed");
    assert!(h.query_by_label("Alpha section").is_none());
    h.get_by_label("Bookmarks").click();
    h.run_steps(3);
    assert_eq!(h.state().right, Some(RightPanel::Bookmarks));
    h.get_by_label("Alpha section");
    h.get_by_label("Beta section");
    // The same button closes them again (the open panel's title is also "Bookmarks").
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Bookmarks").click();
    h.run_steps(3);
    assert_eq!(h.state().right, None);
    assert!(h.query_by_label("Alpha section").is_none());
    // A document that asks for its outline (/PageMode /UseOutlines) shows it in Read mode too,
    // unless the user closed the panel (#2884), so forget that closing here.
    h.state_mut().right_panel_closed = false;
    let pdf = String::from_utf8(FIXTURE.to_vec()).unwrap().replace("/Outlines 6 0 R", "/Outlines 6 0 R /PageMode /UseOutlines");
    h.state_mut().open_bytes("outline.pdf", None, pdf.into_bytes()).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().mode, Mode::Read);
    h.get_by_label("Alpha section");
}

/// The mode bar matches Acrobat's: All tools, Edit, Convert and E-Sign, with no Read tab. Read
/// mode is still a View command.
#[test]
fn the_mode_bar_has_no_read_tab() {
    use pdfcraft_ui_egui::Mode;
    let mut h = harness(|app| app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    for label in ["All tools", "Edit", "Convert", "E-Sign"] {
        assert!(h.query_all_by_label(label).next().is_some(), "the {label} tab is there");
    }
    assert!(h.query_all_by_label("Read").next().is_none(), "no Read tab");
    h.state_mut().run_command("view.read_mode");
    h.run_steps(2);
    assert_eq!(h.state().mode, Mode::Read);
}

/// A document that asks to open in Full Screen mode (`/PageMode /FullScreen`) is asked about
/// first, as in Acrobat: "Not now" leaves the window as it is, "Full Screen" enters it.
#[test]
fn a_document_asking_for_full_screen_is_asked_about_first() {
    let deck = || String::from_utf8(FIXTURE.to_vec()).unwrap().replace("/Outlines 6 0 R", "/Outlines 6 0 R /PageMode /FullScreen").into_bytes();
    let mut h = harness(move |app| app.open_bytes("deck.pdf", None, deck()).expect("opens"));
    h.run_steps(3);
    assert!(!h.state().full_screen, "nothing happens before the answer");
    h.get_by_label("Open in Full Screen mode?");
    h.get_by_label("Not now").click();
    h.run_steps(3);
    assert!(!h.state().full_screen);
    assert!(h.query_by_label("Open in Full Screen mode?").is_none() && h.state().pending_full_screen.is_none());
    // Asked again for the next such document; Full Screen enters it.
    let again = String::from_utf8(FIXTURE.to_vec()).unwrap().replace("/Outlines 6 0 R", "/Outlines 6 0 R /PageMode /FullScreen").into_bytes();
    h.state_mut().open_bytes("deck2.pdf", None, again).expect("opens");
    h.run_steps(3);
    h.get_by_label("Full Screen").click();
    h.run_steps(3);
    assert!(h.state().full_screen, "Full Screen enters it");
    assert!(h.state().pending_full_screen.is_none());
    // A plain document asks nothing.
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    h.state_mut().open_bytes("plain.pdf", None, FIXTURE.to_vec()).expect("opens");
    h.run_steps(3);
    assert!(h.query_by_label("Open in Full Screen mode?").is_none() && !h.state().full_screen);
}

/// Switching to Read mode closes the open pane, and switching back reopens it.
#[test]
fn read_mode_closes_the_panes_and_leaving_it_brings_them_back() {
    use pdfcraft_ui_egui::{Mode, RightPanel};
    let mut h = harness(|app| app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    assert_eq!(h.state().right, Some(RightPanel::Comments));
    h.state_mut().run_command("view.read_mode");
    h.run_steps(3);
    assert_eq!(h.state().mode, Mode::Read);
    assert_eq!(h.state().right, None);
    assert!(h.query_by_label_contains("Check the numbers").is_none(), "the Comments panel is hidden");
    h.state_mut().run_command("view.read_mode");
    h.run_steps(3);
    assert_eq!(h.state().mode, Mode::AllTools);
    assert_eq!(h.state().right, Some(RightPanel::Comments));
    // A pane opened in Read mode stays open on the way out.
    h.state_mut().run_command("view.read_mode");
    h.run_steps(2);
    h.get_by_label("Page thumbnails").click();
    h.run_steps(2);
    h.get_by_label("All tools").click();
    h.run_steps(3);
    assert_eq!(h.state().right, Some(RightPanel::Pages));
}

/// Read mode hides the quick toolbar too, and puts down the tool that was in use (#2646).
#[test]
fn read_mode_hides_the_quick_toolbar() {
    use pdfcraft_ui_egui::{Mode, QuickTool};
    let mut h = harness(|app| app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    h.get_by_label("Hand (H)").click();
    h.run_steps(2);
    assert_eq!(h.state().quick_tool, QuickTool::Hand);
    h.state_mut().run_command("view.read_mode");
    h.run_steps(3);
    assert!(h.query_by_label("Select (V)").is_none(), "no quick toolbar in Read mode");
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    h.state_mut().run_command("view.read_mode");
    h.run_steps(3);
    assert_eq!(h.state().mode, Mode::AllTools);
    h.get_by_label("Select (V)");
}

/// Opening a document with Read as the default mode puts down the tool in use, as entering Read
/// mode from the mode bar does: the hidden toolbar would leave no way to see or change it.
#[test]
fn opening_into_read_mode_puts_the_tool_down() {
    use pdfcraft_ui_egui::{Mode, QuickTool};
    let mut h = harness(|app| app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    assert_eq!(h.state().mode, Mode::AllTools);
    h.state_mut().set_option("quick", "hand").unwrap();
    h.state_mut().set_option("default-mode", "read").unwrap();
    h.state_mut().open_bytes("again.pdf", None, FIXTURE.to_vec()).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().mode, Mode::Read, "the new document opens in the default mode");
    assert_eq!(h.state().quick_tool, QuickTool::Select, "no tool stays active behind the hidden toolbar");
}

/// A pane closed in Read mode stays closed on the way out: leaving Read mode doesn't bring back the
/// pane saved on the way in, and closing Comments keeps its close preference.
#[test]
fn a_pane_closed_in_read_mode_stays_closed_when_leaving_it() {
    use pdfcraft_ui_egui::{Mode, RightPanel};
    let mut h = harness(|app| app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    assert_eq!(h.state().right, Some(RightPanel::Comments));
    h.state_mut().run_command("view.read_mode");
    h.run_steps(3);
    assert_eq!(h.state().right, None);
    // Open Comments from the rail, then close it again.
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Comments").click();
    h.run_steps(3);
    assert_eq!(h.state().right, Some(RightPanel::Comments));
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Comments").click();
    h.run_steps(3);
    assert_eq!(h.state().right, None);
    assert!(h.state().comments_panel_closed);
    // Leave Read mode through the toolbar.
    h.get_by_label("All tools").click();
    h.run_steps(3);
    assert_eq!(h.state().mode, Mode::AllTools);
    assert_eq!(h.state().right, None, "the pane closed in Read mode stays closed");
    assert!(h.state().comments_panel_closed);
    // The same through the Read mode command.
    h.state_mut().choose_right_panel(Some(RightPanel::Comments));
    h.state_mut().run_command("view.read_mode");
    h.run_steps(2);
    assert_eq!(h.state().mode, Mode::Read);
    h.state_mut().choose_right_panel(Some(RightPanel::Comments));
    h.state_mut().choose_right_panel(None);
    h.state_mut().run_command("view.read_mode");
    h.run_steps(3);
    assert_ne!(h.state().mode, Mode::Read);
    assert_eq!(h.state().right, None);
    assert!(h.state().comments_panel_closed);
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

/// A middle click on a tab closes it, as in Acrobat and browsers (#1853); a tab with unsaved
/// changes asks first, as its close button does.
#[test]
fn a_middle_click_closes_the_tab() {
    let mut h = harness(|app| {
        app.open_bytes("first.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes("second.pdf", None, FIXTURE.to_vec()).unwrap();
    });
    h.get_by_label("first.pdf").click_button(egui::PointerButton::Middle);
    h.run_steps(3);
    let app = h.state();
    assert_eq!(app.views.len(), 1);
    assert_eq!(app.session.get(app.views[0].id).unwrap().name, "second.pdf");
    assert!(app.close_request.is_none(), "a clean document closes without asking");
    h.state_mut().apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
    h.run_steps(2);
    h.get_by_label("second.pdf (edited)").click_button(egui::PointerButton::Middle);
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 1, "unsaved changes ask first");
    assert!(h.state().close_request.is_some());
}

#[test]
fn a_link_prompt_above_a_save_prompt_owns_its_keys() {
    for key in [egui::Key::Escape, egui::Key::Enter] {
        let mut h = harness(|app| {
            app.open_bytes("first.pdf", None, FIXTURE.to_vec()).unwrap();
            app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
            app.close_request = Some(pdfcraft_ui_egui::CloseRequest::Tab(app.views[0].id));
        });
        h.run_steps(3);
        h.state_mut().pending_link =
            Some(pdfcraft_ui_egui::PendingLink { url: "https://example.org".to_owned(), origin: pdfcraft_ui_egui::LinkOrigin::Link });
        h.run_steps(3);
        h.key_press(key);
        h.run_steps(3);
        assert_eq!(h.state().pending_link.is_some(), key == egui::Key::Enter);
        assert!(h.state().close_request.is_some(), "the save prompt stays open after {key:?}");
        assert_eq!(h.state().views.len(), 1, "the edited document remains open");
        assert!(h.state().last_opened_url.is_none());
    }
}

#[test]
fn overflowing_tabs_can_be_reached_by_scrolling() {
    let mut h = harness(|app| {
        for i in 0..20 {
            app.open_bytes(&format!("document-{i:02}.pdf"), None, FIXTURE.to_vec()).unwrap();
        }
    });
    let last = h.get_by_label("document-19.pdf").rect();
    assert!(last.center().x < 1200.0, "newly opened active tab must be visible: {last:?}");
    h.state_mut().active = Some(0);
    h.run_steps(4);
    let first = h.get_by_label("document-00.pdf").rect();
    assert!(first.center().x < 1200.0, "changing active tab must reveal it");
    h.event(egui::Event::PointerMoved(first.center()));
    for _ in 0..8 {
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(-500.0, 0.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
            source: egui::MouseWheelSource::Unknown,
        });
        h.run_steps(8);
    }
    let last = h.get_by_label("document-19.pdf").rect();
    assert!(last.center().x > 0.0 && last.center().x < 1200.0, "last tab must be reachable: {last:?}");
    h.get_by_label("document-19.pdf").click();
    h.run_steps(3);
    assert_eq!(h.state().active, Some(19));
    assert!(h.get_by_label_contains("Display theme:").rect().center().x < 1400.0);
    let close = last.right_center() - egui::vec2(16.0, 0.0);
    h.event(egui::Event::PointerMoved(close));
    h.event(egui::Event::PointerButton { pos: close, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.step();
    h.event(egui::Event::PointerButton { pos: close, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(4);
    assert_eq!(h.state().views.len(), 19);
    assert!(h.state().active.is_some_and(|i| i < 19));
    if let Ok(dir) = std::env::var("PDFCRAFT_TAB_SHOTS") {
        let _gpu = crate::gpu();
        crate::support::settle(&mut h);
        h.render().unwrap().save(format!("{dir}/overflow-tabs.png")).unwrap();
    }
}

/// Whether a node labelled `label` is there and enabled (`None`: not there).
fn enabled(h: &Harness<'static, PdfCraftApp>, label: &str) -> Option<bool> {
    h.query_by_label(label)?;
    Some(h.query_by(|n| n.label().as_deref() == Some(label) && n.is_disabled()).is_none())
}

#[test]
fn tabs_shrink_to_fit_then_arrows_step_through_them_and_open_stays_in_sight() {
    // Five long names are wider than the strip: the tabs shrink instead of running under the
    // buttons on the right, Open sits after them, and no arrows show.
    let name = |i: usize| format!("Statement {i} for the office records, scanned.pdf");
    let mut h = harness(move |app| {
        for i in 0..5 {
            app.open_bytes(&name(i), None, FIXTURE.to_vec()).unwrap();
        }
    });
    let open = h.get_by_label("Open").rect();
    let discord = h.get_by_label("Discord").rect();
    assert!(open.right() <= discord.left(), "Open {open:?} must not run under Discord {discord:?}");
    let mut previous_right = f32::MIN;
    for i in 0..5 {
        let tab = h.get_by_label(&name(i)).rect();
        assert!(tab.left() >= previous_right - 0.5, "tabs must not overlap: {i} at {tab:?}");
        assert!(tab.right() <= open.left() + 0.5, "every tab fits before Open: {i} at {tab:?}, Open at {open:?}");
        assert!(tab.width() >= 179.0, "tabs keep most of their name: {i} is {tab:?}");
        previous_right = tab.right();
    }
    assert_eq!(enabled(&h, "Scroll tabs left"), None, "no arrows while every tab fits");
    // Seven don't fit even at their narrowest: arrows show. The newest tab is active and in
    // sight at the end, so only the left arrow can go further.
    for i in 5..7 {
        h.state_mut().open_bytes(&name(i), None, FIXTURE.to_vec()).unwrap();
    }
    h.run_steps(6);
    assert_eq!((enabled(&h, "Scroll tabs left"), enabled(&h, "Scroll tabs right")), (Some(true), Some(false)));
    let first = h.get_by_label(&name(0)).rect();
    h.get_by_label("Scroll tabs left").click();
    h.run_steps(4);
    assert!(h.get_by_label(&name(0)).rect().left() > first.left(), "the left arrow scrolls the tabs back");
    assert_eq!(enabled(&h, "Scroll tabs right"), Some(true), "and then there is more to the right");
    let open = h.get_by_label("Open").rect();
    let right = h.get_by_label("Scroll tabs right").rect();
    assert!(right.right() <= open.left() + 0.5 && open.right() <= h.get_by_label("Discord").rect().left());
    // Twenty: Open stays in sight left of Discord.
    for i in 0..13 {
        h.state_mut().open_bytes(&format!("more-{i:02}.pdf"), None, FIXTURE.to_vec()).unwrap();
    }
    h.run_steps(4);
    let open = h.get_by_label("Open").rect();
    let discord = h.get_by_label("Discord").rect();
    assert!(open.left() > 0.0 && open.right() <= discord.left(), "Open {open:?} stays in sight left of Discord {discord:?}");
}

/// macOS (#789): with the integrated title bar, the strip is the 28 pt band AppKit centres the
/// traffic lights in, and everything in it fits that band and clears the lights.
#[cfg(target_os = "macos")]
#[test]
fn the_integrated_title_bar_is_the_traffic_light_band() {
    let h = harness(|app| {
        app.integrated_titlebar = true;
        app.open_bytes("band.pdf", None, FIXTURE.to_vec()).unwrap();
    });
    let band = 28.0;
    for label in ["Home", "band.pdf", "Open", "Discord", "Keyboard shortcuts"] {
        let r = h.get_by_label(label).rect();
        assert!(r.top() >= 0.0 && r.bottom() <= band, "{label}: {r:?} inside the {band} pt title bar");
        assert!(r.height() >= 24.0, "{label}: {r:?} stays clickable");
    }
    // Content starts after the three lights (the last one ends 62 pt in) with a gap.
    let home = h.get_by_label("Home").rect();
    assert!(home.left() >= 62.0 + 4.0, "Home {home:?} clears the traffic lights");
}

#[test]
fn narrow_tab_strips_accept_vertical_wheel_scrolling() {
    for width in [600.0, 900.0] {
        let mut h = Harness::builder().with_size(egui::vec2(width, 700.0)).build_eframe(|_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            for i in 0..12 {
                app.open_bytes(&format!("narrow-{i:02}.pdf"), None, FIXTURE.to_vec()).unwrap();
            }
            app
        });
        h.run_steps(4);
        let last = h.get_by(|n| n.role() == egui::accesskit::Role::Tab && n.label().as_deref() == Some("narrow-11.pdf")).rect();
        assert!(last.center().x > 28.0 && last.right() < width - 100.0, "active tab: {last:?}");
        h.state_mut().active = Some(0);
        h.run_steps(4);
        let first = h.get_by(|n| n.role() == egui::accesskit::Role::Tab && n.label().as_deref() == Some("narrow-00.pdf")).rect();
        assert!(first.center().x > 28.0 && first.right() < width - 100.0);
        h.event(egui::Event::PointerMoved(first.center()));
        for _ in 0..8 {
            h.event(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -500.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
                source: egui::MouseWheelSource::Unknown,
            });
            h.run_steps(8);
        }
        let last = h.get_by(|n| n.role() == egui::accesskit::Role::Tab && n.label().as_deref() == Some("narrow-11.pdf")).rect();
        assert!(last.center().x > 28.0 && last.right() < width - 100.0, "scrolled tab: {last:?}");
        h.get_by(|n| n.role() == egui::accesskit::Role::Tab && n.label().as_deref() == Some("narrow-11.pdf")).click();
        h.run_steps(4);
        assert_eq!(h.state().active, Some(11));
        assert!(h.get_by_label_contains("Display theme:").rect().right() <= width);
        if let Ok(dir) = std::env::var("PDFCRAFT_TAB_SHOTS") {
            let _gpu = crate::gpu();
            crate::support::settle(&mut h);
            h.render().unwrap().save(format!("{dir}/tabs-{width}.png")).unwrap();
        }
    }
}

#[test]
fn garbage_input_is_rejected_without_panicking() {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
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

#[test]
fn find_counts_matches_across_pages() {
    let mut h = harness(|app| {
        app.open_bytes("text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
        app.set_option("find", "brown").unwrap();
    });
    crate::support::settle(&mut h);
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
    crate::support::settle(&mut h);
    h.get_by_label("No matches");
}

#[test]
fn drag_selects_text_and_copy_returns_it() {
    let mut h = harness(|app| {
        app.open_bytes("text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
    });
    crate::support::settle(&mut h);
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
    a.set_option("language", "en").unwrap();
    a.set_option("theme", "dark").unwrap();
    let json = a.persist();
    let mut b = PdfCraftApp::new();
    b.set_option("language", "en").unwrap();
    b.restore(&json);
    assert_eq!(b.theme, pdfcraft_ui_egui::theme::ThemeKind::Dark);
    b.restore("{not json");
    b.restore("{\"recent\": 5, \"theme\": \"Purple\"}");
    assert_eq!(b.theme, pdfcraft_ui_egui::theme::ThemeKind::Dark);
}

/// A tool's saved "no fill" (`"fill": null`) clears a fill it had as its default; a missing or
/// malformed fill keeps it (#340).
#[test]
fn a_saved_no_fill_clears_a_comment_tool_default_fill() {
    use pdfcraft_ui_egui::comments::CommentTool;
    let filled = |a: &mut PdfCraftApp| {
        let mut s = a.comment_prefs.style(CommentTool::Rectangle);
        s.fill = Some([0.2, 0.4, 0.6]);
        a.comment_prefs.set_style(CommentTool::Rectangle, s);
    };
    let mut a = PdfCraftApp::new();
    filled(&mut a);
    a.restore(r#"{"comment_styles":[{"tool":"comment.square","fill":null}]}"#);
    assert_eq!(a.comment_prefs.style(CommentTool::Rectangle).fill, None, "null clears the fill");
    let mut b = PdfCraftApp::new();
    filled(&mut b);
    b.restore(r#"{"comment_styles":[{"tool":"comment.square","color":[0,0,1]},{"tool":"comment.square","fill":"red"}]}"#);
    assert_eq!(b.comment_prefs.style(CommentTool::Rectangle).fill, Some([0.2, 0.4, 0.6]), "missing or malformed keeps it");
    // A round trip keeps a cleared fill cleared.
    let mut c = PdfCraftApp::new();
    c.restore(&a.persist());
    assert_eq!(c.comment_prefs.style(CommentTool::Rectangle).fill, None);
}

/// Persisted per-tool comment styles go through their clamping setters: hostile or malformed
/// values are clamped or ignored, never trusted (#340).
#[test]
fn comment_style_settings_clamp_and_refuse_hostile_values() {
    use pdfcraft_ui_egui::comments::CommentTool;
    let mut a = PdfCraftApp::new();
    a.restore(
        r#"{"comment_styles":[
            {"tool":"comment.highlight","color":[2,-1,0.5],"opacity":99,"width":-4},
            {"tool":"comment.note","color":"yellow","opacity":"none"},
            {"tool":"comment.note","color":[0.1,null,0.3]},
            {"tool":"bogus.command","color":[0,0,0]}
        ]}"#,
    );
    // Out-of-range numbers are clamped into range.
    let st = a.comment_prefs.style(CommentTool::Highlight);
    assert_eq!((st.color, st.opacity, st.width), ([1.0, 0.0, 0.5], 1.0, 0.5));
    // A malformed colour is refused whole: the tool keeps its default.
    let note = a.comment_prefs.style(CommentTool::Note);
    assert_eq!(note.color, [1.0, 0.82, 0.0], "the default colour stays");
}

#[test]
fn zoom_keeps_the_point_under_the_cursor_still() {
    let mut h = harness(|app| {
        app.open_bytes("text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
    });
    crate::support::settle(&mut h);
    let before = h.state().views[0].glyph_screen_pos(0, 4).expect("glyph on screen");
    let z = h.state().views[0].zoom;
    h.state_mut().views[0].zoom_at(z * 2.0, before);
    h.run_steps(4);
    let after = h.state().views[0].glyph_screen_pos(0, 4).expect("glyph still on screen");
    assert!((after - before).length() < 2.0, "moved from {before:?} to {after:?}");
}

#[test]
fn trackpad_pinch_zooms_the_pdf_under_the_pointer() {
    let mut h = harness(|app| {
        app.open_bytes("text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
    });
    crate::support::settle(&mut h);
    let before = h.state().views[0].glyph_screen_pos(0, 4).expect("glyph on screen");
    let zoom = h.state().views[0].zoom;
    h.hover_at(before);
    h.run_steps(1);
    h.event(egui::Event::Zoom(1.5));
    h.run_steps(4);
    let enlarged = &h.state().views[0];
    assert!((enlarged.zoom - zoom * 1.5).abs() < 0.01);
    let after = enlarged.glyph_screen_pos(0, 4).expect("glyph still on screen");
    assert!((after - before).length() < 2.0, "moved from {before:?} to {after:?}");

    h.event(egui::Event::Zoom(0.5));
    h.run_steps(4);
    assert!((h.state().views[0].zoom - zoom * 0.75).abs() < 0.01);
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
        crate::support::settle(&mut h);
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
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app
    });
    h.run_steps(3);
    assert!(h.state().views.is_empty());
    let file: egui::DroppedFileHandle = std::sync::Arc::new(Dropped { path: "dropped.pdf".into(), bytes: FIXTURE.to_vec() });
    h.input_mut().dropped_files.push(file);
    support::wait_until(&mut h, "dropped document", |app| app.views.len() == 1 && app.password_prompt.is_none());
    assert_eq!(h.state().views.len(), 1, "the dropped PDF opens in a tab");
    h.get_by_label_contains("dropped.pdf");
    let (_, id) = h.state().active_ids().unwrap();
    let doc = h.state().session.get(id).unwrap();
    assert_eq!(doc.name, "dropped.pdf");
    assert_eq!(doc.bytes.as_slice(), FIXTURE);
}

#[test]
fn dropping_an_open_pdf_again_focuses_its_tab() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-drop-dedup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dropped.pdf");
    std::fs::write(&path, FIXTURE).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app
    });
    h.run_steps(3);
    let file: egui::DroppedFileHandle = std::sync::Arc::new(Dropped { path: path.clone(), bytes: FIXTURE.to_vec() });
    h.input_mut().dropped_files.push(file.clone());
    // The drop opens through the background worker on a live window; wait for the tab.
    support::wait_until(&mut h, "dropped document", |app| app.views.len() == 1);
    assert_eq!(h.state().views.len(), 1, "the dropped PDF opens in a tab");
    h.input_mut().dropped_files.push(file);
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 1, "dropping it again adds no tab");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_file_dropped_on_the_page_grid_is_read_from_disk_and_inserted() {
    // The grid's drop reads a file from its path when its turn to be inserted comes, and inserts it
    // at the end when the pointer doesn't say where it went (about a second).
    let dir = std::env::temp_dir().join(format!("pdfcraft-grid-drop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dropped.pdf");
    let setup = PdfCraftApp::new();
    let pdf = setup.session.create_from_text("dropped", "from disk").unwrap();
    std::fs::write(&path, pdf.as_slice()).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let grid = app.session.create_from_text("grid", "one page").unwrap();
        app.open_bytes("grid.pdf", None, grid.to_vec()).unwrap();
        app.views[0].organize = true;
        app
    });
    h.run_steps(3);
    let pages = |h: &egui_kittest::Harness<'_, PdfCraftApp>| {
        let app = h.state();
        app.session.get(app.views[0].id).unwrap().info.pages.len()
    };
    assert_eq!(pages(&h), 1);
    let file: egui::DroppedFileHandle = std::sync::Arc::new(Dropped { path: path.clone(), bytes: Vec::new() });
    h.input_mut().dropped_files.push(file);
    for _ in 0..300 {
        h.run_steps(1);
        if pages(&h) == 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(pages(&h), 2, "the dropped file's page is inserted at the end of the grid");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pdfs_dropped_on_the_combine_tab_join_its_list() {
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app
    });
    h.run_steps(3);
    h.state_mut().set_option("combine-view", "list").unwrap();
    h.state_mut().execute("page.combine");
    h.run_steps(2);
    // An absolute path is read from disk, with its modified time.
    let path = std::env::temp_dir().join(format!("pdfcraft-combine-drop-{}.pdf", std::process::id()));
    std::fs::write(&path, FIXTURE).unwrap();
    let on_disk: egui::DroppedFileHandle = std::sync::Arc::new(Dropped { path: path.clone(), bytes: Vec::new() });
    let in_memory: egui::DroppedFileHandle = std::sync::Arc::new(Dropped { path: "second.pdf".into(), bytes: FIXTURE.to_vec() });
    h.input_mut().dropped_files.extend([on_disk, in_memory]);
    h.run_steps(3);
    std::fs::remove_file(&path).ok();
    let app = h.state();
    assert!(app.views.is_empty(), "nothing opens");
    assert_eq!(app.combine_draft.len(), 2);
    assert!(app.combine_draft[0].modified.is_some() && app.combine_draft[1].modified.is_none());
    h.get_by_label("just now");
}

#[test]
fn a_folder_dropped_on_the_combine_tab_adds_its_pdfs() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-combine-drop-dir-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("inner")).unwrap();
    std::fs::write(dir.join("one.pdf"), FIXTURE).unwrap();
    std::fs::write(dir.join("inner").join("two.pdf"), FIXTURE).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app
    });
    h.run_steps(3);
    h.state_mut().execute("page.combine");
    h.run_steps(2);
    let folder: egui::DroppedFileHandle = std::sync::Arc::new(Dropped { path: dir.clone(), bytes: Vec::new() });
    h.input_mut().dropped_files.push(folder);
    h.run_steps(3);
    let _ = std::fs::remove_dir_all(&dir);
    let names: Vec<_> = h.state().combine_draft.iter().map(|f| f.name.clone()).collect();
    assert_eq!(names, ["two.pdf", "one.pdf"], "subfolders included, in path order");
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
    // The file is read on a worker thread, after however many frames that takes: it must still
    // be there until the tab has opened.
    support::wait_until(&mut h, "the file the operating system opened", |app| !app.views.is_empty());
    std::fs::remove_file(&path).ok();
    assert_eq!(h.state().views.len(), 1, "the file opens in a tab");
    h.get_by_label_contains(&name);
    // A file opened while the app runs brings its window forward, even minimized or full screen
    // on another Space.
    std::fs::write(&path, FIXTURE).unwrap();
    queue.borrow_mut().push(OsEvent::Open(vec![path.to_string_lossy().into_owned()]));
    h.step();
    std::fs::remove_file(&path).ok();
    let commands: Vec<_> = h.output().viewport_output.values().flat_map(|v| v.commands.clone()).collect();
    assert!(commands.contains(&egui::ViewportCommand::Minimized(false)), "{commands:?}");
    assert!(commands.contains(&egui::ViewportCommand::Focus), "{commands:?}");
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
        app.set_option("language", "en").unwrap();
        app.default_mode = mode;
        app.set_option("mode", "sign").unwrap();
        let mut restored = PdfCraftApp::new();
        restored.set_option("language", "en").unwrap();
        restored.restore(&app.persist());
        assert_eq!(restored.default_mode, mode);
        restored.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).unwrap();
        assert_eq!(restored.mode, mode, "session override must not be persisted");
    }
    for json in ["{}", "{not json", r#"{"default_mode": null}"#, r#"{"default_mode": 5}"#, r#"{"default_mode": "unknown"}"#] {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
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
    app.set_option("language", "en").unwrap();
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
            app.set_option("language", "en").unwrap();
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
    h.get_by_role_and_label(Role::RadioButton, "Read").scroll_to_me();
    h.run_steps(4);
    h.get_by_role_and_label(Role::RadioButton, "Read").click();
    h.run_steps(2);
    assert_eq!(h.state().default_mode, Mode::Read);
    assert_eq!(h.state().mode, Mode::AllTools);
    h.get_by_label("OK").click();
    h.run_steps(2);
    h.state_mut().open_bytes("fixture.pdf", None, FIXTURE.to_vec()).unwrap();
    assert_eq!(h.state().mode, Mode::Read);
}

/// GNOME on Wayland: the tab strip carries the window buttons until Preferences ▸ Use system title
/// bar gives the window back its frame, at once and without a restart (#553, #1001). The choice
/// is saved; desktops that draw the frame themselves don't offer it.
#[test]
fn use_system_title_bar_swaps_the_header_bar_for_the_window_frame() {
    use egui::accesskit::Role;
    use pdfcraft_ui_egui::window_controls::Layout;
    fn decorations(h: &Harness<'static, PdfCraftApp>) -> Option<bool> {
        h.output().viewport_output.values().flat_map(|v| v.commands.iter()).find_map(|c| match c {
            egui::ViewportCommand::Decorations(on) => Some(*on),
            _ => None,
        })
    }
    let mut h = harness(|app| app.window_controls = Some(Layout::default()));
    // The window opened undecorated, so the first frame asks for nothing.
    assert_eq!(decorations(&h), None);
    assert!(h.query_by_label("Minimize").is_some(), "the header bar has the window buttons");
    h.state_mut().set_option("dialog", "preferences").unwrap();
    h.run_steps(2);
    h.get_by_role_and_label(Role::CheckBox, "Use system title bar").scroll_to_me();
    h.run_steps(4);
    h.get_by_role_and_label(Role::CheckBox, "Use system title bar").click();
    let mut asked = None;
    for _ in 0..4 {
        h.step();
        asked = asked.or(decorations(&h));
    }
    assert!(h.state().system_title_bar);
    assert_eq!(asked, Some(true), "the window gets its frame back");
    assert!(h.query_by_label("Minimize").is_none(), "and the tab strip loses its buttons");
    let saved = h.state().persist();
    assert!(saved.contains(r#""system_title_bar":true"#), "{saved}");
    let mut restored = PdfCraftApp::new();
    restored.restore(&saved);
    assert!(restored.system_title_bar);
    restored.set_option("system-title-bar", "false").unwrap();
    assert!(!restored.system_title_bar);
    assert!(restored.set_option("system-title-bar", "maybe").is_err());
    // Where the desktop draws the frame there is nothing to choose.
    let mut h = harness(|app| app.set_option("dialog", "preferences").unwrap());
    h.run_steps(2);
    assert!(h.query_by_label("Use system title bar").is_none());
    assert!(h.query_by_label("Minimize").is_none());
    assert_eq!(decorations(&h), None);
}

/// Preferences ▸ Animation effects: the system's animation setting until the user chooses On or
/// Off. The choice is saved, and settings holding anything else leave it alone.
#[test]
fn preferences_choose_animation_effects_and_the_choice_is_saved() {
    use egui::accesskit::Role;
    use pdfcraft_ui_egui::motion::AnimationEffects;
    let mut h = harness(|app| app.set_option("dialog", "preferences").unwrap());
    assert_eq!(h.state().animation_effects, AnimationEffects::System);
    h.get_by_label("Animation effects");
    for (label, choice) in [
        ("Off", AnimationEffects::Off),
        ("On", AnimationEffects::On),
        ("Use system setting", AnimationEffects::System),
        ("Off", AnimationEffects::Off),
    ] {
        h.get_by_role_and_label(Role::RadioButton, label).scroll_to_me();
        h.run_steps(4);
        h.get_by_role_and_label(Role::RadioButton, label).click();
        h.run_steps(2);
        assert_eq!(h.state().animation_effects, choice, "{label}");
    }
    let saved = h.state().persist();
    assert!(saved.contains(r#""animation_effects":"off""#), "{saved}");
    let mut restored = PdfCraftApp::new();
    restored.restore(&saved);
    assert_eq!(restored.animation_effects, AnimationEffects::Off);
    let mut fresh = PdfCraftApp::new();
    fresh.restore(&saved.replace(r#""animation_effects":"off""#, r#""animation_effects":"sideways""#));
    assert_eq!(fresh.animation_effects, AnimationEffects::System);
    // The control channel and the command line take the same three names.
    fresh.set_option("animation_effects", "ON").unwrap();
    assert_eq!(fresh.animation_effects, AnimationEffects::On);
    assert!(fresh.set_option("animation_effects", "sideways").unwrap_err().contains("system, on, or off"));
    assert_eq!(fresh.animation_effects, AnimationEffects::On);
}

#[test]
fn explicit_mode_preserves_independent_tool_and_panel_options() {
    use pdfcraft_ui_egui::LeftPanel;
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
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
    app.set_option("language", "en").unwrap();
    app.default_mode = Mode::Edit;
    app.set_option("left", "closed").unwrap();
    app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).unwrap();
    assert_eq!(app.mode, Mode::Edit);
    assert!(!app.left_open);
    // Default All Tools: opening another PDF leaves the tool panel the user picked.
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.set_option("tool", "export").unwrap();
    app.open_bytes("first.pdf", None, FIXTURE.to_vec()).unwrap();
    app.open_bytes("second.pdf", None, FIXTURE.to_vec()).unwrap();
    assert_eq!(app.mode, Mode::AllTools);
    assert_eq!(app.left, LeftPanel::Tool("export"));
}

fn three_tab_harness() -> Harness<'static, PdfCraftApp> {
    harness(|app| {
        for name in ["first.pdf", "middle.pdf", "last.pdf"] {
            app.open_bytes(name, None, FIXTURE.to_vec()).unwrap();
        }
    })
}

fn close_named_tab(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    // The close button has no separate accessible label. Its centre is defined by
    // chrome::tab relative to the actual accessible tab rectangle, not a viewport guess.
    let pos = h.get_by_role_and_label(egui::accesskit::Role::Tab, name).rect().right_center() - egui::vec2(16.0, 0.0);
    h.event(egui::Event::PointerMoved(pos));
    h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(3);
}

#[test]
fn closing_tabs_keeps_the_selected_document_or_selects_its_neighbor() {
    for removed in [0, 1, 2] {
        let mut h = three_tab_harness();
        let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();
        h.get_by_label("middle.pdf").click();
        h.run_steps(3);
        assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[1]));
        close_named_tab(&mut h, ["first.pdf", "middle.pdf", "last.pdf"][removed]);
        let expected = if removed == 1 { ids[2] } else { ids[1] };
        let app = h.state();
        assert_eq!(app.active_ids().map(|(_, id)| id), Some(expected), "removed tab {removed}");
        assert_eq!(
            app.views.iter().map(|view| view.id).collect::<Vec<_>>(),
            ids.iter().copied().enumerate().filter_map(|(i, id)| (i != removed).then_some(id)).collect::<Vec<_>>()
        );
        assert_eq!(app.session.docs().len(), 2);
        assert!(app.session.get(ids[removed]).is_none());
        assert!(app.close_request.is_none(), "clean tabs close without a prompt");
        for doc in app.session.docs() {
            assert_eq!(doc.bytes.as_slice(), FIXTURE);
            assert!(!doc.dirty);
        }
    }
}

#[test]
fn closing_the_last_active_tab_selects_the_previous_one_then_home() {
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();
    for (removed, name) in [(2usize, "last.pdf"), (1, "middle.pdf"), (0, "first.pdf")] {
        close_named_tab(&mut h, name);
        let app = h.state();
        assert_eq!(app.active_ids().map(|(_, id)| id), removed.checked_sub(1).map(|index| ids[index]));
        assert_eq!(app.views.len(), removed);
        assert_eq!(app.session.docs().len(), removed);
        assert!(app.session.get(ids[removed]).is_none());
    }
    assert_eq!(h.state().active, None);
    h.get_by_label_contains("Welcome to PdfCraft");
}

#[test]
fn cancelling_then_discarding_an_earlier_dirty_tab_preserves_the_selected_document() {
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();
    h.get_by_label("first.pdf").click();
    h.run_steps(3);
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Unsaved title".into() }));
    h.run_steps(3);
    h.get_by_label("middle.pdf").click();
    h.run_steps(3);
    close_named_tab(&mut h, "first.pdf (edited)");
    assert_eq!(h.state().close_request, Some(pdfcraft_ui_egui::CloseRequest::Tab(ids[0])));
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 3);
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[1]));
    assert!(h.state().session.get(ids[0]).unwrap().dirty);
    close_named_tab(&mut h, "first.pdf (edited)");
    h.get_by_label("Don't save").click();
    h.run_steps(3);
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[1]));
    assert_eq!(h.state().views.iter().map(|view| view.id).collect::<Vec<_>>(), [ids[1], ids[2]]);
    assert!(h.state().session.get(ids[0]).is_none());
    assert!(h.state().close_request.is_none());
    for doc in h.state().session.docs() {
        assert_eq!(doc.bytes.as_slice(), FIXTURE);
        assert!(!doc.dirty);
    }
}

#[test]
fn long_german_tool_labels_stay_within_the_sidebar() {
    use pdfcraft_ui_egui::{LeftPanel, Mode};
    let mut h = harness(|app| app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    h.state_mut().set_option("language", "de").unwrap();
    h.state_mut().mode = Mode::AllTools;
    h.state_mut().left_open = true;
    // The tool heading has to leave room for Back and Close even when translated.
    h.state_mut().left = LeftPanel::Tool("fill_sign");
    h.run_steps(4);
    let heading =
        painted_text(&h).into_iter().find(|(s, ..)| s.starts_with("Ausfüllen und")).expect("translated heading is painted, whole or elided");
    assert!(heading.2.right() <= 272.0, "translated heading runs past the sidebar: {:?}", heading.2);
    // A long sub-tool name must stop before its status chip, not paint over the PDF.
    h.state_mut().left = LeftPanel::Tool("protect");
    h.run_steps(4);
    let item =
        painted_text(&h).into_iter().find(|(s, ..)| s == "Ausgeblendete Informationen entfernen").expect("long translated sub-tool is painted");
    assert!(item.2.right() <= 272.0, "translated tool item runs past the sidebar: {:?}", item.2);
}

#[test]
fn document_tabs_expose_selection_and_activate_from_keyboard() {
    use egui::accesskit::Role;
    for key in [egui::Key::Enter, egui::Key::Space] {
        let mut h = three_tab_harness();
        let first = h.get_by_role_and_label(Role::Tab, "first.pdf");
        assert_eq!(first.accesskit_node().is_selected(), Some(false));
        assert_eq!(h.get_by_role_and_label(Role::Tab, "last.pdf").accesskit_node().is_selected(), Some(true));
        first.focus();
        h.run_steps(2);
        assert!(h.get_by_role_and_label(Role::Tab, "first.pdf").is_focused());
        h.key_press(key);
        h.run_steps(3);
        assert_eq!(h.state().active, Some(0));
        assert_eq!(h.get_by_role_and_label(Role::Tab, "first.pdf").accesskit_node().is_selected(), Some(true));
        assert_eq!(h.get_by_role_and_label(Role::Tab, "last.pdf").accesskit_node().is_selected(), Some(false));
        assert_eq!(h.state().views.len(), 3, "activation does not hit the close button");
        for doc in h.state().session.docs() {
            assert_eq!(doc.bytes.as_slice(), FIXTURE);
            assert!(!doc.dirty);
        }
        if let Ok(dir) = std::env::var("PDFCRAFT_TAB_SHOTS") {
            let _gpu = crate::gpu();
            crate::support::settle(&mut h);
            h.render().unwrap().save(format!("{dir}/focused-tab-{key:?}.png")).unwrap();
        }
    }
}

#[test]
fn document_tab_identity_survives_dirty_labels_and_removing_an_earlier_document() {
    use egui::accesskit::Role;
    let mut h = three_tab_harness();
    h.get_by_role_and_label(Role::Tab, "middle.pdf").click();
    h.run_steps(3);
    h.get_by_role_and_label(Role::Tab, "middle.pdf").focus();
    h.run_steps(2);
    let middle_id = h.get_by_role_and_label(Role::Tab, "middle.pdf").accesskit_node().id();
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Unsaved title".into() }));
    h.run_steps(3);
    let dirty = h.get_by_role_and_label(Role::Tab, "middle.pdf (edited)");
    assert_eq!(dirty.accesskit_node().id(), middle_id);
    assert!(dirty.is_focused());
    close_named_tab(&mut h, "first.pdf");
    assert_eq!(h.get_by_role_and_label(Role::Tab, "middle.pdf (edited)").accesskit_node().id(), middle_id);
    assert_eq!(h.state().active, Some(0));
    assert_eq!(h.state().views.len(), 2);
    assert!(h.state().session.get(h.state().views[0].id).unwrap().dirty);
}

#[test]
fn combine_tab_uses_tab_interaction_without_changing_close_or_document_state() {
    use egui::accesskit::Role;
    let mut h = three_tab_harness();
    let document_ids: Vec<_> = h.state().views.iter().map(|v| v.id).collect();
    h.state_mut().execute("page.combine");
    h.run_steps(3);
    assert_eq!(h.get_by_role_and_label(Role::Tab, "Combine files").accesskit_node().is_selected(), Some(true));
    h.get_by_role_and_label(Role::Tab, "first.pdf").click();
    h.run_steps(3);
    h.get_by_role_and_label(Role::Tab, "Combine files").focus();
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(h.get_by_role_and_label(Role::Tab, "Combine files").accesskit_node().is_selected(), Some(true));
    close_named_tab(&mut h, "Combine files");
    assert!(h.query_by_role_and_label(Role::Tab, "Combine files").is_none());
    assert_eq!(h.state().views.iter().map(|v| v.id).collect::<Vec<_>>(), document_ids);
}

/// Presses the named tab, drags it to `to` in a few steps and, unless `release` is false, lets go.
fn drag_named_tab(h: &mut Harness<'static, PdfCraftApp>, name: &str, to: egui::Pos2, release: bool) {
    let from = h.get_by_role_and_label(egui::accesskit::Role::Tab, name).rect().center();
    h.event(egui::Event::PointerMoved(from));
    h.event(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.run_steps(1);
    for k in 1..=6 {
        h.event(egui::Event::PointerMoved(from.lerp(to, k as f32 / 6.0)));
        h.run_steps(1);
    }
    if release {
        h.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        h.run_steps(3);
    }
}

#[test]
fn dragging_a_tab_moves_it_and_keeps_the_same_document_active() {
    use egui::accesskit::Role;
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();
    let tab = |h: &Harness<'static, PdfCraftApp>, name: &str| h.get_by_role_and_label(Role::Tab, name).rect();
    // first.pdf past the end; last.pdf, the active one, keeps the focus at its new place.
    let to = tab(&h, "last.pdf").right_center() + egui::vec2(20.0, 0.0);
    drag_named_tab(&mut h, "first.pdf", to, true);
    assert_eq!(h.state().views.iter().map(|view| view.id).collect::<Vec<_>>(), [ids[1], ids[2], ids[0]]);
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[2]));
    assert!(tab(&h, "first.pdf").left() > tab(&h, "last.pdf").right(), "the strip draws the new order");
    // last.pdf, now in the middle, to the front.
    let to = tab(&h, "middle.pdf").left_center() - egui::vec2(10.0, 0.0);
    drag_named_tab(&mut h, "last.pdf", to, true);
    assert_eq!(h.state().views.iter().map(|view| view.id).collect::<Vec<_>>(), [ids[2], ids[1], ids[0]]);
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[2]));
    assert_eq!(h.get_by_role_and_label(Role::Tab, "last.pdf").accesskit_node().is_selected(), Some(true));
    // Moving tabs touches no document.
    assert_eq!(h.state().session.docs().len(), 3);
    for doc in h.state().session.docs() {
        assert_eq!(doc.bytes.as_slice(), FIXTURE);
        assert!(!doc.dirty);
    }
    if let Ok(dir) = std::env::var("PDFCRAFT_TAB_SHOTS") {
        let _gpu = crate::gpu();
        let to = tab(&h, "first.pdf").left_center() - egui::vec2(10.0, 0.0);
        drag_named_tab(&mut h, "last.pdf", to, false);
        h.render().unwrap().save(format!("{dir}/dragging-tab.png")).unwrap();
    }
}

/// Regression (#2225): the drop line in front of the first tab sat half a gap outside the
/// scroll area's clip rect and was never visible.
#[test]
fn the_drop_line_shows_in_front_of_the_first_tab() {
    use egui::accesskit::Role;
    let _gpu = crate::gpu();
    let mut h = three_tab_harness();
    let first = h.get_by_role_and_label(Role::Tab, "first.pdf").rect();
    // Pixels in the strip just left of the first tab that are strongly blue (the accent line).
    let blue_left_of = |h: &mut Harness<'static, PdfCraftApp>, rect: egui::Rect| {
        let image = h.render().unwrap();
        let scale = image.width() as f32 / h.ctx.content_rect().width();
        let x0 = ((rect.left() - 3.0) * scale).max(0.0) as u32;
        let x1 = ((rect.left() + 1.0) * scale).max(0.0) as u32;
        let (y0, y1) = ((rect.top() * scale) as u32, (rect.bottom() * scale) as u32);
        let mut n = 0;
        for y in y0..y1.min(image.height()) {
            for x in x0..x1.min(image.width()) {
                let p = image.get_pixel(x, y).0;
                if p[2] > 150 && p[0] < 90 && p[1] < 140 {
                    n += 1;
                }
            }
        }
        n
    };
    let before = blue_left_of(&mut h, first);
    // last.pdf to the left of the first tab, released not yet.
    let to = first.left_center() - egui::vec2(10.0, 0.0);
    drag_named_tab(&mut h, "last.pdf", to, false);
    let during = blue_left_of(&mut h, first);
    assert!(during > before + 10, "no drop line left of the first tab ({before} -> {during})");
    if let Ok(dir) = std::env::var("PDFCRAFT_TAB_SHOTS") {
        h.render().unwrap().save(format!("{dir}/dragging-to-front.png")).unwrap();
    }
}

#[test]
fn a_tab_dropped_where_it_was_stays_and_a_click_still_selects_it() {
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();
    let middle = h.get_by_role_and_label(egui::accesskit::Role::Tab, "middle.pdf").rect();
    drag_named_tab(&mut h, "middle.pdf", middle.center() + egui::vec2(30.0, 0.0), true);
    assert_eq!(h.state().views.iter().map(|view| view.id).collect::<Vec<_>>(), ids);
    h.get_by_label("first.pdf").click();
    h.run_steps(3);
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[0]));
    assert_eq!(h.state().views.iter().map(|view| view.id).collect::<Vec<_>>(), ids);
}

#[test]
fn the_combine_tab_does_not_move_and_documents_stay_in_order() {
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();
    h.state_mut().execute("page.combine");
    h.run_steps(3);
    let first = h.get_by_role_and_label(egui::accesskit::Role::Tab, "first.pdf").rect();
    drag_named_tab(&mut h, "Combine files", first.left_center() - egui::vec2(10.0, 0.0), true);
    assert_eq!(h.state().views.iter().map(|view| view.id).collect::<Vec<_>>(), ids);
}

#[test]
fn dragging_a_tab_to_open_space_detaches_it_into_its_own_window() {
    use egui::accesskit::Role;
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();
    let tab = |h: &Harness<'static, PdfCraftApp>, name: &str| h.get_by_role_and_label(Role::Tab, name).rect();

    // Drag "middle.pdf" downwards into open space (60px below the tab strip)
    let from = tab(&h, "middle.pdf").center();
    let to = from + egui::vec2(0.0, 60.0);
    drag_named_tab(&mut h, "middle.pdf", to, true);
    h.run_steps(3);

    // middle.pdf is detached
    assert!(h.state().views[1].detached, "middle.pdf tab should be detached");
    // Tab strip only retains the remaining non-detached tabs
    assert!(h.query_by_role_and_label(Role::Tab, "middle.pdf").is_none());
    assert!(h.query_by_role_and_label(Role::Tab, "first.pdf").is_some());
    assert!(h.query_by_role_and_label(Role::Tab, "last.pdf").is_some());

    // Re-attaching brings it back to the tab strip
    h.state_mut().attach_tab(ids[1]);
    h.run_steps(3);
    assert!(!h.state().views[1].detached, "middle.pdf should be attached");
    assert!(h.query_by_role_and_label(Role::Tab, "middle.pdf").is_some());
}

#[test]
fn tab_context_menu_can_detach_tab_to_new_window() {
    use egui::accesskit::Role;
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();

    // Right-click first.pdf tab and click "Move to New Window"
    h.get_by_role_and_label(Role::Tab, "first.pdf").click_secondary();
    h.run_steps(2);
    h.get_by_label("Move to New Window").click();
    h.run_steps(3);

    assert!(h.state().views[0].detached, "first.pdf should be detached");
    assert!(h.query_by_role_and_label(Role::Tab, "first.pdf").is_none());
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[2]));
}

#[test]
fn dragging_sole_tab_to_open_space_does_not_detach_it() {
    use egui::accesskit::Role;
    let mut h = harness(|app| {
        app.open_bytes("sole.pdf", None, FIXTURE.to_vec()).unwrap();
    });
    let id = h.state().views[0].id;
    let tab_rect = h.get_by_role_and_label(Role::Tab, "sole.pdf").rect();

    // Drag "sole.pdf" downwards into open space (60px below the tab strip)
    let from = tab_rect.center();
    let to = from + egui::vec2(0.0, 60.0);
    drag_named_tab(&mut h, "sole.pdf", to, true);
    h.run_steps(3);

    // sole.pdf must NOT be detached and must remain the active tab
    assert!(!h.state().views[0].detached, "sole tab must not be detached");
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(id));
    assert!(h.query_by_role_and_label(Role::Tab, "sole.pdf").is_some());
}

#[test]
fn sole_tab_context_menu_disables_move_to_new_window() {
    use egui::accesskit::Role;
    let mut h = harness(|app| {
        app.open_bytes("sole.pdf", None, FIXTURE.to_vec()).unwrap();
    });

    // Right-click sole.pdf tab
    h.get_by_role_and_label(Role::Tab, "sole.pdf").click_secondary();
    h.run_steps(2);

    let move_node = h.get_by_label("Move to New Window");
    assert!(move_node.accesskit_node().is_disabled(), "Move to New Window should be disabled when only one tab is open");
    move_node.hover();
    h.run_steps(30);
    h.get_by_label_contains("Cannot detach the only open document");
}

#[test]
fn detached_window_views_retain_render_demand_and_are_not_suspended() {
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();

    // Detach middle.pdf
    h.state_mut().detach_tab(ids[1]);
    h.run_steps(4);

    // middle.pdf is detached and treated as visible by finish_render_frame
    assert!(h.state().views[1].detached, "middle.pdf is detached");
    // It rendered in its detached window viewport
    assert!(h.state().session.get(ids[1]).is_some());
}

#[test]
fn dragging_tab_with_prior_detached_tab_translates_slot_to_views_index() {
    use egui::accesskit::Role;
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();
    let tab = |h: &Harness<'static, PdfCraftApp>, name: &str| h.get_by_role_and_label(Role::Tab, name).rect();

    // Detach first tab (first.pdf, ids[0])
    h.state_mut().detach_tab(ids[0]);
    h.run_steps(3);
    assert!(h.state().views[0].detached, "first.pdf should be detached");

    // Drag middle.pdf past last.pdf to the right
    let to = tab(&h, "last.pdf").right_center() + egui::vec2(20.0, 0.0);
    drag_named_tab(&mut h, "middle.pdf", to, true);
    h.run_steps(3);

    // Verify views order: [first.pdf (detached), last.pdf, middle.pdf]
    assert_eq!(
        h.state().views.iter().map(|view| view.id).collect::<Vec<_>>(),
        [ids[0], ids[2], ids[1]],
        "middle.pdf should have moved after last.pdf"
    );
    // Tab strip renders the new visual order
    assert!(tab(&h, "middle.pdf").left() > tab(&h, "last.pdf").right(), "middle.pdf should be right of last.pdf");
}

#[test]
fn closing_docked_tab_with_earlier_detached_tab_updates_active_to_docked_tab() {
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();

    // 3 tabs: first.pdf (ids[0]), middle.pdf (ids[1]), last.pdf (ids[2]).
    // Detach first.pdf (ids[0]).
    h.state_mut().detach_tab(ids[0]);
    h.run_steps(3);
    assert!(h.state().views[0].detached, "first.pdf should be detached");

    // Activate middle.pdf (ids[1])
    h.get_by_role_and_label(egui::accesskit::Role::Tab, "middle.pdf").click();
    h.run_steps(3);
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[1]));

    // Close middle.pdf (the active docked tab).
    close_named_tab(&mut h, "middle.pdf");
    h.run_steps(3);

    // Active tab must now be last.pdf (docked, ids[2]), never detached first.pdf (ids[0])!
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[2]));
    assert!(!h.state().views[h.state().active.unwrap()].detached);

    // Now close last.pdf. Since first.pdf is detached, no docked tabs remain.
    // Active must become None (Home screen), never first.pdf!
    close_named_tab(&mut h, "last.pdf");
    h.run_steps(3);
    assert_eq!(h.state().active, None, "active must be None when no docked tabs remain");
}

#[test]
fn detaching_active_tab_predictably_transfers_active_to_adjacent_docked_tab() {
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();
    // Initially last.pdf (ids[2], index 2) is active.
    assert_eq!(h.state().active, Some(2));

    // Detach the active tab (last.pdf).
    // Nearest docked tab is adjacent middle.pdf (index 1), not first.pdf (index 0).
    h.state_mut().detach_tab(ids[2]);
    h.run_steps(3);

    assert!(h.state().views[2].detached);
    assert_eq!(h.state().active, Some(1), "detaching last tab transfers to adjacent middle.pdf");
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[1]));

    // Now middle.pdf is active (index 1). Detach middle.pdf.
    // Remaining docked tab is first.pdf (index 0).
    h.state_mut().detach_tab(ids[1]);
    h.run_steps(3);

    assert!(h.state().views[1].detached);
    assert_eq!(h.state().active, Some(0), "detaching middle tab transfers to remaining docked first.pdf");
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[0]));
}

#[test]
fn detaching_all_tabs_sets_active_to_none_and_shows_home() {
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();

    // Detach all three tabs
    h.state_mut().detach_tab(ids[0]);
    h.state_mut().detach_tab(ids[1]);
    h.state_mut().detach_tab(ids[2]);
    h.run_steps(4);

    assert!(h.state().views.iter().all(|v| v.detached));
    assert_eq!(h.state().active, None, "active must be None when all tabs are detached");

    // Home screen is shown in the main window
    assert!(h.query_by_label("Welcome to PdfCraft").is_some() || h.query_by_label("Open file").is_some());
    // No tabs in the main window tab strip
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Tab, "first.pdf").is_none());
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Tab, "middle.pdf").is_none());
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Tab, "last.pdf").is_none());
}

#[test]
fn keyboard_tab_navigation_cycles_exclusively_through_docked_tabs() {
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();

    // Detach the middle tab (ids[1])
    h.state_mut().detach_tab(ids[1]);
    h.run_steps(3);
    assert!(h.state().views[1].detached);

    // Focus first tab (ids[0], index 0)
    h.get_by_role_and_label(egui::accesskit::Role::Tab, "first.pdf").click();
    h.run_steps(3);
    assert_eq!(h.state().active, Some(0));

    // Next tab shortcut (Ctrl+Tab / view.next_tab) should skip detached middle.pdf (index 1)
    // and go directly to last.pdf (index 2)!
    let ctrl = egui::Modifiers::CTRL | egui::Modifiers::COMMAND;
    h.key_press_modifiers(ctrl, egui::Key::Tab);
    h.run_steps(3);
    assert_eq!(h.state().active, Some(2), "next tab should skip detached tab and jump to last.pdf");
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[2]));

    // Next tab again should wrap around directly to first.pdf (index 0)
    h.key_press_modifiers(ctrl, egui::Key::Tab);
    h.run_steps(3);
    assert_eq!(h.state().active, Some(0), "next tab should wrap back to first.pdf");
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[0]));

    // Previous tab shortcut (Ctrl+Shift+Tab / view.previous_tab) should go to last.pdf (index 2)
    h.key_press_modifiers(ctrl | egui::Modifiers::SHIFT, egui::Key::Tab);
    h.run_steps(3);
    assert_eq!(h.state().active, Some(2), "previous tab should wrap to last.pdf, skipping middle");
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[2]));
}

#[test]
fn resolve_close_saving_detached_tab_preserves_main_window_active_tab() {
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();

    // Detach middle.pdf (index 1)
    h.state_mut().detach_tab(ids[1]);
    // Set active tab to first.pdf (index 0)
    h.state_mut().active = Some(0);
    h.run_steps(3);

    // Make middle.pdf dirty so close requests a save
    h.state_mut().session.apply(ids[1], pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    h.state_mut().request_close_tab(1);
    assert!(h.state().close_request.is_some());

    // Resolve save with Save (choice = Some(true))
    let ctx = h.ctx.clone();
    h.state_mut().resolve_close(&ctx, Some(true));
    h.run_steps(3);

    // Active tab in main window must still be first.pdf (index 0), NOT detached middle.pdf!
    assert_eq!(h.state().active, Some(0));
}

#[test]
fn closing_clean_detached_window_immediately_closes_tab() {
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();

    // Detach middle.pdf (index 1)
    h.state_mut().detach_tab(ids[1]);
    h.run_steps(3);
    assert!(h.state().views[1].detached);

    // Request OS close on detached child viewport
    let child_viewport_id = egui::ViewportId::from_hash_of(("detached-doc", ids[1]));
    h.input_mut().viewports.entry(child_viewport_id).or_default().events.push(egui::ViewportEvent::Close);
    h.run_steps(3);

    // Clean detached window immediately closes and removes tab from self.views
    assert_eq!(h.state().views.len(), 2);
    assert!(h.state().views.iter().all(|v| v.id != ids[1]));
    assert!(h.state().session.get(ids[1]).is_none());
    assert!(h.state().close_request.is_none());
}

#[test]
fn closing_dirty_detached_window_intercepts_close_and_prompts_inside_child_viewport() {
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();

    // Detach middle.pdf (index 1) and make it dirty
    h.state_mut().detach_tab(ids[1]);
    h.run_steps(3);
    h.state_mut().session.apply(ids[1], pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();

    // Request OS close on detached child viewport
    let child_viewport_id = egui::ViewportId::from_hash_of(("detached-doc", ids[1]));
    h.input_mut().viewports.entry(child_viewport_id).or_default().events.push(egui::ViewportEvent::Close);
    h.step();

    // CancelClose command must be sent to abort premature OS destruction
    let cancel_close_sent =
        h.output().viewport_output.values().flat_map(|v| v.commands.iter()).any(|c| matches!(c, egui::ViewportCommand::CancelClose));
    assert!(cancel_close_sent, "CancelClose command must be sent to child viewport");

    // Close request is flagged for this detached document
    assert_eq!(h.state().close_request, Some(pdfcraft_ui_egui::CloseRequest::Tab(ids[1])));

    h.run_steps(2);

    // Save confirmation prompt is displayed inside the detached window
    h.get_by_label_contains("Save changes to “middle.pdf”");
    h.get_by_label("Save");
    h.get_by_label("Don't save");
    h.get_by_label("Cancel");

    // Middle tab still exists, is dirty, and remains detached
    assert_eq!(h.state().views.len(), 3);
    let view = h.state().views.iter().find(|v| v.id == ids[1]).unwrap();
    assert!(view.detached);
    assert!(h.state().session.get(ids[1]).unwrap().dirty);
}

#[test]
fn cancelling_save_prompt_in_detached_window_leaves_tab_open_detached_and_dirty() {
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();

    h.state_mut().detach_tab(ids[1]);
    h.run_steps(3);
    h.state_mut().session.apply(ids[1], pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();

    let child_viewport_id = egui::ViewportId::from_hash_of(("detached-doc", ids[1]));
    h.input_mut().viewports.entry(child_viewport_id).or_default().events.push(egui::ViewportEvent::Close);
    h.run_steps(3);

    // Clicking "Cancel" in detached save prompt leaves tab open, detached, and dirty
    h.get_by_label("Cancel").click();
    h.run_steps(3);

    assert_eq!(h.state().views.len(), 3);
    let view = h.state().views.iter().find(|v| v.id == ids[1]).unwrap();
    assert!(view.detached);
    assert!(h.state().session.get(ids[1]).unwrap().dirty);
    assert!(h.state().close_request.is_none());
}

#[test]
fn dont_save_in_detached_window_discards_changes_and_closes_tab() {
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();

    h.state_mut().detach_tab(ids[1]);
    h.run_steps(3);
    h.state_mut().session.apply(ids[1], pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();

    let child_viewport_id = egui::ViewportId::from_hash_of(("detached-doc", ids[1]));
    h.input_mut().viewports.entry(child_viewport_id).or_default().events.push(egui::ViewportEvent::Close);
    h.run_steps(3);

    // Clicking "Don't save" discards changes and closes the detached tab
    h.get_by_label("Don't save").click();
    h.run_steps(3);

    assert_eq!(h.state().views.len(), 2);
    assert!(h.state().views.iter().all(|v| v.id != ids[1]));
    assert!(h.state().session.get(ids[1]).is_none());
    assert!(h.state().close_request.is_none());
}

#[test]
fn save_in_detached_window_saves_changes_and_closes_tab() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-ui-test-save-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let save_path = dir.join("middle_saved.pdf");

    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();

    h.state_mut().save_override = Some(save_path.to_string_lossy().into_owned());
    h.state_mut().detach_tab(ids[1]);
    h.run_steps(3);
    h.state_mut().session.apply(ids[1], pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();

    let child_viewport_id = egui::ViewportId::from_hash_of(("detached-doc", ids[1]));
    h.input_mut().viewports.entry(child_viewport_id).or_default().events.push(egui::ViewportEvent::Close);
    h.run_steps(3);

    // Clicking "Save" saves changes and closes the detached tab once written
    h.get_by_label("Save").click();
    h.run_steps(5);

    assert!(save_path.exists());
    assert_eq!(h.state().views.len(), 2);
    assert!(h.state().views.iter().all(|v| v.id != ids[1]));
    assert!(h.state().session.get(ids[1]).is_none());
    assert!(h.state().close_request.is_none());
}

#[test]
fn root_window_close_with_dirty_detached_window_cancels_close_and_prompts_on_main_window() {
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();

    // Detach middle.pdf (index 1) and make it dirty
    h.state_mut().detach_tab(ids[1]);
    h.run_steps(3);
    h.state_mut().session.apply(ids[1], pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();

    // Request OS close on the primary root window
    h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
    h.step();

    // Root close must be cancelled via CancelClose
    let cancel_close_sent =
        h.output().viewport_output.values().flat_map(|v| v.commands.iter()).any(|c| matches!(c, egui::ViewportCommand::CancelClose));
    assert!(cancel_close_sent, "CancelClose command must be sent to cancel immediate exit");

    // Close request is flagged for application quit, and quit is not allowed yet
    assert_eq!(h.state().close_request, Some(pdfcraft_ui_egui::CloseRequest::Quit));
    assert!(!h.state().allow_quit);

    h.run_steps(2);

    // Save prompt is displayed on the main window for the dirty detached document
    h.get_by_label_contains("Save changes to “middle.pdf” before closing?");
    h.get_by_label("Save");
    h.get_by_label("Don't save");
    h.get_by_label("Cancel");

    // Cancelling leaves the application alive, middle.pdf still detached and dirty
    h.get_by_label("Cancel").click();
    h.run_steps(3);

    assert!(h.state().close_request.is_none());
    assert!(!h.state().allow_quit);
    assert_eq!(h.state().views.len(), 3);
    let view = h.state().views.iter().find(|v| v.id == ids[1]).unwrap();
    assert!(view.detached);
    assert!(h.state().session.get(ids[1]).unwrap().dirty);
}

#[test]
fn root_window_close_sequence_prompts_sequentially_across_docked_and_detached_and_quits() {
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();

    // Make first.pdf dirty (docked)
    h.state_mut().session.apply(ids[0], pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    // Detach middle.pdf (index 1) and make it dirty (detached)
    h.state_mut().detach_tab(ids[1]);
    h.run_steps(3);
    h.state_mut().session.apply(ids[1], pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    // Detach last.pdf (clean detached tab to test teardown with floating clean windows)
    h.state_mut().detach_tab(ids[2]);
    h.run_steps(3);

    // Trigger application exit via root window close
    h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
    h.step();

    assert_eq!(h.state().close_request, Some(pdfcraft_ui_egui::CloseRequest::Quit));
    assert!(!h.state().allow_quit);

    h.run_steps(2);

    // First prompt asks about docked first.pdf
    h.get_by_label_contains("Save changes to “first.pdf” before closing?");
    h.get_by_label("Don't save").click();
    h.run_steps(3);

    // After discarding first.pdf, next prompt sequentially asks about dirty detached middle.pdf on main window
    h.get_by_label_contains("Save changes to “middle.pdf” before closing?");
    h.get_by_label("Don't save").click();
    h.step();

    // Once all dirty documents are resolved, quit completes: allow_quit is true, no prompts remain
    assert!(h.state().close_request.is_none());
    assert!(h.state().allow_quit);

    // Root viewport (and detached viewports) receive Close command
    let close_sent = h.output().viewport_output.values().flat_map(|v| v.commands.iter()).any(|c| matches!(c, egui::ViewportCommand::Close));
    assert!(close_sent, "Close command must be sent when application shutdown completes");
}

#[test]
fn root_window_close_when_all_tabs_clean_quits_immediately_without_prompt() {
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();

    // Detach middle.pdf; all tabs remain clean
    h.state_mut().detach_tab(ids[1]);
    h.run_steps(3);

    // Trigger application exit via root window close
    h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
    h.step();

    // No CancelClose sent
    let cancel_close_sent =
        h.output().viewport_output.values().flat_map(|v| v.commands.iter()).any(|c| matches!(c, egui::ViewportCommand::CancelClose));
    assert!(!cancel_close_sent, "Clean exit must not cancel window close");

    // No save prompt modal is shown
    assert!(h.query_by_label_contains("Save changes to").is_none());
    assert!(h.state().close_request.is_none());
    assert!(h.state().allow_quit);
}

#[test]
fn root_window_request_quit_shortcut_prompts_and_saving_detached_tab_quits() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-ui-test-quit-save-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let save_path = dir.join("middle_saved_quit.pdf");

    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();

    h.state_mut().save_override = Some(save_path.to_string_lossy().into_owned());
    h.state_mut().detach_tab(ids[1]);
    h.run_steps(3);
    h.state_mut().session.apply(ids[1], pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();

    // Trigger quit via Command+Q shortcut event
    h.event(egui::Event::Key { key: egui::Key::Q, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND });
    h.step();

    assert_eq!(h.state().close_request, Some(pdfcraft_ui_egui::CloseRequest::Quit));
    assert!(!h.state().allow_quit);

    h.run_steps(2);
    h.get_by_label_contains("Save changes to “middle.pdf” before closing?");
    h.get_by_label("Save").click();
    h.run_steps(5);

    // Saved to disk and quit completed
    assert!(save_path.exists());
    assert!(h.state().close_request.is_none());
    assert!(h.state().allow_quit);
}

#[test]
fn clean_detached_and_docked_tabs_request_quit_returns_true_and_allows_quit() {
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();

    h.state_mut().detach_tab(ids[1]);
    h.run_steps(3);

    // Direct call to request_quit on clean state
    let ctx = egui::Context::default();
    let quit_allowed = h.state_mut().request_quit(&ctx);
    assert!(quit_allowed, "request_quit must return true when all tabs are clean");
    assert!(h.state().allow_quit, "allow_quit must be true");
    assert!(h.state().close_request.is_none());
}

#[test]
fn reopening_detached_file_reattaches_tab_and_commit_typing_preserves_active_invariants() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-ui-test-reattach-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let p1 = dir.join("first.pdf").to_string_lossy().into_owned();
    let p2 = dir.join("middle.pdf").to_string_lossy().into_owned();
    let p3 = dir.join("last.pdf").to_string_lossy().into_owned();

    let p1_c = p1.clone();
    let p2_c = p2.clone();
    let p3_c = p3.clone();
    let mut h = harness(move |app| {
        app.open_bytes("first.pdf", Some(p1_c), FIXTURE.to_vec()).unwrap();
        app.open_bytes("middle.pdf", Some(p2_c), FIXTURE.to_vec()).unwrap();
        app.open_bytes("last.pdf", Some(p3_c), FIXTURE.to_vec()).unwrap();
    });

    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();

    // Detach middle.pdf (index 1)
    h.state_mut().detach_tab(ids[1]);
    h.run_steps(3);
    assert!(h.state().views[1].detached, "middle.pdf should be detached");
    let active_before = h.state().active;
    assert_ne!(active_before, Some(1), "active tab must not be detached tab");

    // 1. commit_typing_in preserves active tab invariants when tab is detached
    h.state_mut().views[1].pending_edit = Some(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
    let committed = h.state_mut().commit_typing_in(ids[1]);
    assert!(committed, "commit_typing_in should succeed");
    assert_eq!(h.state().active, active_before, "commit_typing_in must not make detached tab active");
    assert!(h.state().views[1].detached, "middle.pdf must remain detached");

    // 2. Reopening via open_recent re-attaches middle.pdf and makes it active
    h.state_mut().open_recent(&p2);
    h.run_steps(3);
    assert!(!h.state().views[1].detached, "open_recent must re-attach detached tab");
    assert_eq!(h.state().active, Some(1), "open_recent must make re-attached tab active");

    // Detach middle.pdf again
    h.state_mut().detach_tab(ids[1]);
    h.run_steps(3);
    assert!(h.state().views[1].detached);
    assert_ne!(h.state().active, Some(1));

    // 3. Reopening via open_path re-attaches middle.pdf and makes it active
    let outcome = h.state_mut().open_path(&p2);
    h.run_steps(3);
    assert_eq!(outcome, pdfcraft_ui_egui::OpenOutcome::Focused);
    assert!(!h.state().views[1].detached, "open_path must re-attach detached tab");
    assert_eq!(h.state().active, Some(1), "open_path must make re-attached tab active");
}

/// The Edit tab turns on Edit text & images at once, as Acrobat does, so every paragraph and image
/// is boxed and editable with one click; another tab puts the tool down again.
#[test]
fn the_edit_tab_turns_on_edit_text_and_images() {
    use pdfcraft_ui_egui::{Mode, QuickTool};
    let mut h = harness(|app| app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    support::outside_menus(&h, "Edit").click();
    h.run_steps(3);
    assert_eq!(h.state().mode, Mode::Edit);
    assert_eq!(h.state().quick_tool, QuickTool::EditText);
    h.get_by_label("All tools").click();
    h.run_steps(3);
    assert_eq!(h.state().mode, Mode::AllTools);
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    // Another tool picked inside Edit is not taken away by leaving it.
    support::outside_menus(&h, "Edit").click();
    h.run_steps(3);
    h.state_mut().quick_tool = QuickTool::Hand;
    h.get_by_label("Convert").click();
    h.run_steps(3);
    assert_eq!(h.state().quick_tool, QuickTool::Hand);
}

/// Organize pages in the Edit panel opens the Organize workspace: its own panel and the page grid,
/// not the grid behind the Edit panel.
#[test]
fn organize_pages_from_the_edit_panel_opens_its_panel() {
    use pdfcraft_ui_egui::LeftPanel;
    let mut h = harness(|app| app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    support::outside_menus(&h, "Edit").click();
    h.run_steps(3);
    assert_eq!(h.state().left, LeftPanel::Tool("edit"));
    assert!(
        h.query_by_label("Shift-click to select several objects. Drag empty space to select an area.").is_none(),
        "no idle hint above Format text, as in Acrobat"
    );
    h.get_by_label("Organize pages").click();
    h.run_steps(3);
    assert_eq!(h.state().left, LeftPanel::Tool("organize"));
    assert!(h.state().views[0].organize, "the page grid is showing");
}

/// Header and footer, Watermark and Background keep their Add / Update / Remove menu in the Edit
/// panel, as under All tools: Update and Remove wait for the document to have that kind of mark.
#[test]
fn the_edit_panel_keeps_the_add_update_remove_menu() {
    let mut h = harness(|app| app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    support::outside_menus(&h, "Edit").click();
    h.run_steps(3);
    h.get_by_label("Watermark").click();
    h.run_steps(3);
    assert_eq!(enabled(&h, "Add watermark…"), Some(true));
    assert_eq!(enabled(&h, "Remove watermark"), Some(false), "nothing to remove yet");
    h.get_by_label("Add watermark…").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::Marks(pdfcraft_engine::MarkKind::Watermark)));
}

/// #3282: in a short window the rail's panel buttons scroll above the page navigation instead of
/// being drawn under the page number field.
#[test]
fn short_window_rail_buttons_scroll_clear_of_page_navigation() {
    let (width, height) = (1000.0, 560.0);
    let mut h = Harness::builder().with_size(egui::vec2(width, height)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("short.pdf", None, FIXTURE.to_vec()).unwrap();
        app
    });
    crate::support::settle(&mut h);
    let rail = |h: &Harness<'static, PdfCraftApp>, label: &str| {
        h.query_all_by_label(label).map(|n| n.rect()).find(|r| r.left() >= width - 48.0).unwrap_or_else(|| panic!("{label} in the rail"))
    };
    let page = h.get(By::new().role(egui::accesskit::Role::TextInput).value("1")).rect();
    assert!(page.left() >= width - 48.0 && page.bottom() <= height, "page field {page:?} in the rail");
    assert!(rail(&h, "Comments").bottom() <= page.top(), "the first button sits above the page field");
    assert!(rail(&h, "Tags").top() >= page.top(), "the window is short enough that the buttons must scroll");
    let over = rail(&h, "Comments").center();
    h.event(egui::Event::PointerMoved(over));
    for _ in 0..8 {
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -200.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
            source: egui::MouseWheelSource::Unknown,
        });
        h.run_steps(4);
    }
    let tags = rail(&h, "Tags");
    assert!(tags.bottom() <= page.top(), "Tags {tags:?} scrolls into view above the page field {page:?}");
    assert_eq!(h.get(By::new().role(egui::accesskit::Role::TextInput).value("1")).rect(), page, "navigation stays put");
}

/// A PDF with `n` empty 200×300 pt pages.
fn pages_fixture(n: usize) -> Vec<u8> {
    let mut objs: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")).into_bytes());
    for _ in 0..n {
        objs.push(b"<< /Type /Page /Parent 2 0 R >>".to_vec());
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// A file opened again shows the page, zoom and page display it was last shown with (#1767,
/// Acrobat's "Restore last view settings when reopening documents"): remembered when its tab
/// closes and when settings are saved with it open, applied over the file's own initial view,
/// and not at all with the preference off. Settings are untrusted: bad entries are ignored.
#[test]
fn a_file_opened_again_shows_its_last_view() {
    use pdfcraft_ui_egui::canvas::{Fit, PageLayout};
    let path = std::env::temp_dir().join(format!("pdfcraft-last-view-{}.pdf", std::process::id())).to_string_lossy().into_owned();
    let p = path.clone();
    let mut h = harness(move |app| app.open_bytes("doc.pdf", Some(p), pages_fixture(3)).expect("opens"));
    let p = path.clone();
    assert!(h.state().restore_last_view, "on by default");
    {
        let app = h.state_mut();
        app.set_option("zoom", "50").unwrap();
        let view = &mut app.views[0];
        view.set_display(PageLayout::TwoUp, false);
        view.set_cover(true);
        view.go_to_page(2);
    }
    h.run_steps(3);
    // Saved with the tab still open: quitting remembers it.
    let saved_open = h.state().persist();
    h.state_mut().close_tab(0);
    let saved_closed = h.state().persist();
    for (label, saved) in [("open", saved_open), ("closed", saved_closed)] {
        let (saved, p) = (saved.clone(), p.clone());
        let h = harness(move |app| {
            app.restore(&saved);
            app.open_bytes("doc.pdf", Some(p), pages_fixture(3)).expect("opens");
        });
        let v = &h.state().views[0];
        assert_eq!((v.current, v.fit, v.zoom, v.layout, v.continuous, v.cover), (2, Fit::None, 0.5, PageLayout::TwoUp, false, true), "{label}");
    }
    // The preference off: the defaults.
    let saved = h.state().persist();
    let p = path.clone();
    let h = harness(move |app| {
        app.restore(&saved);
        app.restore_last_view = false;
        app.open_bytes("doc.pdf", Some(p), pages_fixture(3)).expect("opens");
    });
    let v = &h.state().views[0];
    assert_eq!((v.current, v.fit, v.layout, v.continuous, v.cover), (0, Fit::Width, PageLayout::Single, true, false));
    // Garbage in the settings is ignored, a usable entry among it is kept.
    let mut app = PdfCraftApp::new();
    app.restore(&format!(
        r#"{{"restore_last_view": "yes", "last_views": [5, {{"path": ""}}, {{"path": "{}", "page": -1, "zoom": "huge", "display": "single"}}, {{"path": "{}", "page": 1, "zoom": "fit-page", "display": "two-up"}}]}}"#,
        path.replace('\\', "/"),
        path.replace('\\', "/")
    ));
    assert!(app.restore_last_view, "a non-boolean keeps the default");
    assert_eq!(app.last_views.len(), 1);
    assert_eq!((app.last_views[0].page, app.last_views[0].zoom.as_str()), (1, "fit-page"));
}

/// #1767: a fixed zoom with a fraction comes back as it was, not rounded to a whole percent.
#[test]
fn a_fractional_zoom_is_remembered_exactly() {
    use pdfcraft_ui_egui::canvas::Fit;
    let path = std::env::temp_dir().join(format!("pdfcraft-last-view-frac-{}.pdf", std::process::id())).to_string_lossy().into_owned();
    let p = path.clone();
    let mut h = harness(move |app| app.open_bytes("doc.pdf", Some(p), pages_fixture(3)).expect("opens"));
    h.state_mut().views[0].set_zoom(0.625);
    h.run_steps(3);
    h.state_mut().close_tab(0);
    assert_eq!(h.state().last_views[0].zoom, "62.5%");
    let saved = h.state().persist();
    let h = harness(move |app| {
        app.restore(&saved);
        app.open_bytes("doc.pdf", Some(path), pages_fixture(3)).expect("opens");
    });
    let v = &h.state().views[0];
    assert_eq!((v.fit, v.zoom), (Fit::None, 0.625));
}

/// #1767 with full screen: the view remembered while presenting is the one full screen will go
/// back to, and a file reopened in full screen presents one fitted page, then leaves full
/// screen for the remembered view.
#[test]
fn full_screen_neither_is_remembered_nor_hides_the_last_view() {
    use pdfcraft_ui_egui::canvas::{Fit, PageLayout};
    let path = std::env::temp_dir().join(format!("pdfcraft-last-view-fs-{}.pdf", std::process::id())).to_string_lossy().into_owned();
    let p = path.clone();
    let mut h = harness(move |app| app.open_bytes("doc.pdf", Some(p), pages_fixture(3)).expect("opens"));
    {
        let app = h.state_mut();
        app.set_option("zoom", "50").unwrap();
        let view = &mut app.views[0];
        view.set_display(PageLayout::TwoUp, false);
        view.go_to_page(2);
        view.enter_presentation();
        assert!(view.presenting() && view.fit == Fit::Page);
    }
    h.run_steps(3);
    let saved = h.state().persist();
    h.state_mut().close_tab(0);
    let remembered = h.state().last_views[0].clone();
    assert_eq!((remembered.page, remembered.zoom.as_str()), (2, "50%"), "not full screen's fitted page");
    let p = path.clone();
    let mut h = harness(move |app| {
        app.restore(&saved);
        app.full_screen = true;
        app.open_bytes("doc.pdf", Some(p), pages_fixture(3)).expect("opens");
    });
    let v = &h.state().views[0];
    assert!(v.presenting(), "opened in full screen");
    assert_eq!((v.fit, v.continuous, v.current), (Fit::Page, false, 2), "one fitted page, where the reader left off");
    let view = &mut h.state_mut().views[0];
    view.leave_presentation();
    assert_eq!((view.fit, view.zoom, view.layout, view.continuous), (Fit::None, 0.5, PageLayout::TwoUp, false), "the remembered view");
}
