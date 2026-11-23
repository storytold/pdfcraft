//! Headless UI tests (egui_kittest + AccessKit). They drive the real app shell without a window.

use crate::support;

use crate::support::AppHarness as _;
use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT, Queryable};
use pdfcraft_ui_egui::{PdfCraftApp, RightPanel};

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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            setup(&mut app);
            app
        }
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
    // Home has no Open button of its own (the title bar's stays), as production's.
    assert!(h.query_all_by_role_and_label(egui::accesskit::Role::Button, "Open…").count() >= 1, "Open stays reachable");
}

/// Starting the app makes its Home and navigation rail icons available to the renderer.
#[test]
fn app_start_registers_its_icons() {
    let _app = crate::control::new_app();
    for name in ["file-output", "code-xml", "message-square-text", "files", "text-cursor-input", "paperclip", "signature", "tag"] {
        assert!(craft::ui::primitives::has_icon(&craft::behavior::IconId::new(name)), "missing icon {name}");
    }
}

/// A side string of the control state's `side_panels` (`tools_side`, `rail_side`).
fn side(h: &mut Harness<'static, PdfCraftApp>, key: &str) -> String {
    pdfcraft_ui_egui::side_panels::state(h.state())[key].as_str().unwrap_or_default().to_owned()
}

/// The side panel the workspace has open, as `ui.state` reports it (`side_panel`).
pub(crate) fn right(app: &PdfCraftApp) -> Option<RightPanel> {
    let state = pdfcraft_ui_egui::side_panels::state(app);
    let open = state["side_panel"].as_str()?;
    [
        RightPanel::Comments,
        RightPanel::Bookmarks,
        RightPanel::Pages,
        RightPanel::Fields,
        RightPanel::Layers,
        RightPanel::Attachments,
        RightPanel::Signatures,
        RightPanel::Accessibility,
        RightPanel::Tags,
        RightPanel::Search,
        RightPanel::Compare,
    ]
    .into_iter()
    .find(|panel| format!("{panel:?}") == open)
}

/// Runs the command `id` as the app's own command entry does, with the harness's context.
fn run_command(h: &mut Harness<'static, PdfCraftApp>, id: &str) {
    let ctx = h.ctx.clone();
    h.state_mut().run_command(&ctx, id);
}

/// The text of every notice card the main window shows, oldest first.
fn posted_notices(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    use craft::ui::runtime::{env::WindowKey, toast};
    // A posted notice's text is the card's title; a message and lines may follow it.
    toast::cards(&h.ctx, &WindowKey::main())
        .into_iter()
        .map(|card| {
            let toast = &card.toast;
            let parts = toast.title.iter().chain(std::iter::once(&toast.message)).chain(&toast.lines);
            parts.map(|part| part.as_str()).filter(|text| !text.is_empty()).collect::<Vec<_>>().join("\n")
        })
        .collect()
}

/// Picks `option` in the open Preferences' drop-down named `row`, after opening `page` when given. A choice row is the
/// preferences pattern's drop-down, so its options show only while it is open: the last node of that name is the open
/// list's.
fn pick_preference(h: &mut Harness<'static, PdfCraftApp>, page: Option<&str>, row: &str, option: &str) {
    use egui::accesskit::Role;
    // Preferences is one scrolling page: `page` needs no opening.
    let _ = page;
    if h.query_by_role_and_label(Role::RadioGroup, row).is_some() {
        h.get_by_role_and_label(Role::RadioGroup, row).scroll_to_me();
        h.run_steps(4);
        h.get_by_role_and_label(Role::RadioGroup, row).get_by_label(option).click();
        h.run_steps(2);
        return;
    }
    h.get_by_role_and_label(Role::ComboBox, row).scroll_to_me();
    h.run_steps(4);
    h.get_by_role_and_label(Role::ComboBox, row).click();
    h.run_steps(3);
    h.get_all_by_label(option).last().unwrap_or_else(|| panic!("the {option:?} option of {row:?} is on screen")).click();
    h.run_steps(2);
}

#[test]
fn tools_panel_side_preference_moves_and_persists() {
    let mut h = harness(|_| {});
    h.set_option("dialog", "preferences").unwrap();
    h.run_steps(3);
    pick_preference(&mut h, None, "Tools panel side", "Right");
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(side(&mut h, "tools_side"), "right");
    let saved = h.state().settings_snapshot();
    let mut restored = harness(move |app| app.apply_settings(&saved));
    assert_eq!(side(&mut restored, "tools_side"), "right");
    h.set_option("dialog", "preferences").unwrap();
    h.run_steps(3);
    pick_preference(&mut h, None, "Tools panel side", "Left");
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(side(&mut h, "tools_side"), "left");
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
fn home_removes_one_recent_file_or_clears_them_all() {
    // Tall enough that the Recent list is on screen without scrolling.
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1600.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            for name in ["first.pdf", "second.pdf", "third.pdf"] {
                app.recent.push(pdfcraft_ui_egui::RecentFile { name: name.into(), path: format!("/nowhere/{name}"), pages: 1, size: 0 });
            }
            app
        }
    });
    h.run_steps(4);
    assert!(h.query_by_label("Remove second.pdf from Recent").is_none(), "the remove button shows only over its row");
    h.get_by(|n| n.role() == egui::accesskit::Role::ListItem && n.label().is_some_and(|s| s.starts_with("second.pdf,"))).hover();
    h.run_steps(2);
    h.get_by_label("Remove second.pdf from Recent").click();
    h.run_steps(2);
    let names: Vec<String> = h.state().recent.iter().map(|r| r.name.clone()).collect();
    assert_eq!(names, ["first.pdf", "third.pdf"], "only that file left the list");
    h.get_by(|n| n.role() == egui::accesskit::Role::ListItem && n.label().is_some_and(|s| s.starts_with("first.pdf,"))).click_secondary();
    h.run_steps(2);
    h.get_by_label("Clear Recent Files").click();
    // The menu delivers its action after closing; the next frame draws the empty list.
    h.run_steps(3);
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
        assert!(
            h.query_all_by(|n| n.role() == egui::accesskit::Role::ListItem
                && n.label().is_some_and(|label| label == g.label || label.starts_with(&format!("{},", g.label))))
                .count()
                >= 1,
            "tool {} missing from the expanded view",
            g.label
        );
    }
    // An item name finds its tool; the other categories drop out.
    h.state_mut().all_tools_query = "rotate".into();
    h.run_steps(3);
    assert_eq!(
        h.query_all_by(|n| n.role() == egui::accesskit::Role::ListItem
            && n.label().is_some_and(|label| label == "Organize pages" || label.starts_with("Organize pages,")))
            .count(),
        1,
        "only the expanded view's tile; the sidebar is gone"
    );
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
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "tools", "full").unwrap());
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
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "tools", "full").unwrap());
    h.set_option("dialog", "about").unwrap();
    h.run_steps(3);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.state().all_tools_full, "Escape belonged to the dialog");
}

#[test]
fn escape_in_the_command_palette_over_the_expanded_all_tools_leaves_the_view_open() {
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "tools", "full").unwrap());
    h.state_mut().palette_open = true;
    h.run_steps(3);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(!h.state().palette_open, "Escape closed the palette");
    assert!(h.state().all_tools_full, "and only the palette");
}

#[test]
fn another_workspace_from_the_mode_option_puts_the_expanded_all_tools_away() {
    let mut app = crate::control::new_app();
    app.set_option(&egui::Context::default(), "tools", "full").unwrap();
    app.set_option(&egui::Context::default(), "mode", "all").unwrap();
    assert!(app.all_tools_full, "All tools itself keeps the view");
    app.set_option(&egui::Context::default(), "mode", "read").unwrap();
    app.set_option(&egui::Context::default(), "mode", "all").unwrap();
    assert!(!app.all_tools_full, "coming back shows the sidebar, as the mode bar does");
}

#[test]
fn opening_a_document_puts_the_expanded_all_tools_away() {
    use pdfcraft_ui_egui::Mode;
    // All tools as the default workspace: no workspace change, but the document must show.
    let mut app = crate::control::new_app();
    app.set_option(&egui::Context::default(), "tools", "full").unwrap();
    app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens");
    assert!(!app.all_tools_full, "the document shows, not the catalogue over it");
    // Read as the default workspace: coming back to All tools later shows the sidebar.
    let mut app = crate::control::new_app();
    app.set_option(&egui::Context::default(), "default-mode", "read").unwrap();
    app.set_option(&egui::Context::default(), "tools", "full").unwrap();
    app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens");
    assert_eq!(app.mode, Mode::Read);
    assert!(!app.all_tools_full);
}

#[test]
fn choosing_a_tool_puts_the_expanded_all_tools_away() {
    use pdfcraft_ui_egui::LeftPanel;
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "tools", "full").unwrap());
    h.set_option("tool", "organize").unwrap();
    h.run_steps(3);
    assert!(!h.state().all_tools_full, "the tool's panel shows, not the catalogue");
    assert_eq!(h.state().left, LeftPanel::Tool("organize"));
}

#[test]
fn another_tab_coming_forward_puts_the_expanded_all_tools_away() {
    // Home, through the control option.
    let mut h = harness(|app| app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    h.set_option("tools", "full").unwrap();
    h.run_steps(3);
    assert!(h.state().all_tools_full, "the document's own tab keeps it");
    h.set_option("home", "").unwrap();
    h.run_steps(3);
    assert!(!h.state().all_tools_full);
    h.get_by_label_contains("Welcome to PdfCraft");
    // Another document's tab.
    let mut h = harness(|app| {
        app.open_bytes(&egui::Context::default(), "one.pdf", None, FIXTURE.to_vec()).expect("fixture opens");
        app.open_bytes(&egui::Context::default(), "two.pdf", None, FIXTURE.to_vec()).expect("fixture opens");
    });
    h.set_option("tools", "full").unwrap();
    h.run_steps(3);
    h.state_mut().active = Some(0);
    h.run_steps(3);
    assert!(!h.state().all_tools_full, "one.pdf shows, not the catalogue over it");
}

#[test]
fn going_home_with_no_document_puts_the_expanded_all_tools_away() {
    // Home is already the tab in front, so only the option itself can put the view away.
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "tools", "full").unwrap());
    h.set_option("home", "").unwrap();
    h.run_steps(3);
    assert!(!h.state().all_tools_full);
    h.get_by_label_contains("Welcome to PdfCraft");
}

#[test]
fn expanding_all_tools_from_read_mode_restores_the_right_panel() {
    use pdfcraft_ui_egui::{Mode, RightPanel};
    let mut h = harness(|app| app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    h.set_option("panel", "bookmarks").unwrap();
    h.execute("view.read_mode");
    h.run_steps(3);
    assert_eq!((h.state().mode, right(h.state())), (Mode::Read, None), "Read mode hides the panel");
    h.set_option("tools", "full").unwrap();
    h.run_steps(3);
    assert!(h.state().all_tools_full);
    assert_eq!((h.state().mode, right(h.state())), (Mode::AllTools, Some(RightPanel::Bookmarks)));
}

#[test]
fn expanding_all_tools_from_edit_puts_edit_text_down() {
    use pdfcraft_ui_egui::{LeftPanel, Mode, QuickTool};
    let mut h = harness(|app| app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    support::outside_menus(&h, "Edit").click();
    h.run_steps(3);
    assert_eq!(h.state().quick_tool, QuickTool::EditText);
    h.set_option("tools", "full").unwrap();
    h.run_steps(3);
    assert!(h.state().all_tools_full);
    assert_eq!((h.state().mode, h.state().quick_tool), (Mode::AllTools, QuickTool::Select));
    assert_eq!(h.state().left, LeftPanel::AllTools, "collapsing goes back to the All tools list");
}

#[test]
fn opening_a_pdf_shows_comments_and_bookmarks() {
    let mut h = harness(|app| app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    // Documents with comments open on the Comments panel, like Acrobat.
    h.get_by_label_contains("Check the numbers");
    // The rail's icon: the open column's tabs are named alike.
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Bookmarks").click();
    h.run_steps(3);
    h.get_by_label("Alpha section");
    h.get_by_label("Beta section");
}

/// #1590: side panes the user closed stay closed when PdfCraft restarts and opens another PDF,
/// even one with comments and bookmarks, which would otherwise open on the Comments panel.
#[test]
fn closed_side_panes_stay_closed_after_a_restart_and_opening_a_pdf() {
    use pdfcraft_ui_egui::RightPanel;
    let mut h = harness(|app| app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    assert_eq!(right(h.state()), Some(RightPanel::Comments));
    assert!(h.state().left_open());
    h.state_mut().choose_right_panel(None);
    h.set_option("left", "closed").unwrap();
    h.run_steps(3);
    let saved = h.state().settings_snapshot();
    let mut h = harness(move |app| {
        app.apply_settings(&saved);
        app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens");
    });
    assert_eq!(right(h.state()), None, "the closed right pane reopened");
    assert!(!h.state().left_open(), "the closed left pane reopened");
    assert!(h.query_by_label_contains("Check the numbers").is_none());
    // Within a session too: another document opens with them closed.
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "again.pdf", None, FIXTURE.to_vec()).unwrap();
    h.run_steps(3);
    assert_eq!(right(h.state()), None);
    assert!(!h.state().left_open());
}

/// #2884: a right panel the user closed stays closed when a PDF asks for its outline
/// (`/PageMode /UseOutlines`), within a session and after a restart.
#[test]
fn closed_right_panel_stays_closed_for_a_document_asking_for_its_outline() {
    use pdfcraft_ui_egui::RightPanel;
    let pdf = || String::from_utf8(FIXTURE.to_vec()).unwrap().replace("/Outlines 6 0 R", "/Outlines 6 0 R /PageMode /UseOutlines").into_bytes();
    let mut h = harness(|app| app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    assert_eq!(right(h.state()), Some(RightPanel::Comments));
    h.state_mut().choose_right_panel(None);
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "outline.pdf", None, pdf()).unwrap();
    h.run_steps(3);
    assert_eq!(right(h.state()), None, "the outline request reopened the closed right panel");
    let saved = h.state().settings_snapshot();
    let h = harness(move |app| {
        app.apply_settings(&saved);
        app.open_bytes(&egui::Context::default(), "outline.pdf", None, pdf()).expect("outline PDF opens");
    });
    assert_eq!(right(h.state()), None, "the closed right panel reopened after a restart");
}

#[test]
fn navigation_rail_toggles_between_right_and_left_and_persists() {
    let mut h = harness(|app| app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    assert_eq!(side(&mut h, "rail_side"), "right");
    h.get_by_label("Dock navigation rail on Left").click();
    h.run_steps(3);
    assert_eq!(side(&mut h, "rail_side"), "left");
    let saved = h.state().settings_snapshot();
    let mut h2 = harness(move |app| app.apply_settings(&saved));
    assert_eq!(side(&mut h2, "rail_side"), "left");
    h2.set_option("dialog", "preferences").unwrap();
    h2.run_steps(3);
    pick_preference(&mut h2, None, "Navigation rail side", "Right");
    h2.get_by_label("OK").click();
    h2.run_steps(3);
    assert_eq!(side(&mut h2, "rail_side"), "right");
    h2.set_option("dialog", "preferences").unwrap();
    h2.run_steps(3);
    pick_preference(&mut h2, None, "Navigation rail side", "Left");
    h2.get_by_label("OK").click();
    h2.run_steps(3);
    assert_eq!(side(&mut h2, "rail_side"), "left");
}

/// Read mode hides the tools, not the navigation panes: the rail's Bookmarks button opens the
/// bookmarks when Read is the default workspace (#932), and the left tool pane stays hidden.
#[test]
fn read_mode_rail_buttons_open_their_panels() {
    use pdfcraft_ui_egui::{Mode, RightPanel};
    let mut h = harness(|app| {
        app.default_mode = Mode::Read;
        app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens");
    });
    assert_eq!(h.state().mode, Mode::Read);
    assert_eq!(right(h.state()), None, "Read mode opens with the panes closed");
    assert!(h.query_by_label("Alpha section").is_none());
    h.get_by_label("Bookmarks").click();
    h.run_steps(3);
    assert_eq!(right(h.state()), Some(RightPanel::Bookmarks));
    h.get_by_label("Alpha section");
    h.get_by_label("Beta section");
    // The same button closes them again (the open panel's title is also "Bookmarks").
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Bookmarks").click();
    h.run_steps(3);
    assert_eq!(right(h.state()), None);
    assert!(h.query_by_label("Alpha section").is_none());
    // A document that asks for its outline (/PageMode /UseOutlines) shows it in Read mode too,
    // unless the user closed the panel (#2884), so forget that closing here.
    h.state_mut().right_panel_closed = false;
    let pdf = String::from_utf8(FIXTURE.to_vec()).unwrap().replace("/Outlines 6 0 R", "/Outlines 6 0 R /PageMode /UseOutlines");
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "outline.pdf", None, pdf.into_bytes()).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().mode, Mode::Read);
    h.get_by_label("Alpha section");
}

/// A document that asks to open in Full Screen mode (`/PageMode /FullScreen`) is asked about
/// first, as in Acrobat: "Not now" leaves the window as it is, "Full Screen" enters it.
#[test]
fn a_document_asking_for_full_screen_is_asked_about_first() {
    let deck = || String::from_utf8(FIXTURE.to_vec()).unwrap().replace("/Outlines 6 0 R", "/Outlines 6 0 R /PageMode /FullScreen").into_bytes();
    let mut h = harness(move |app| app.open_bytes(&egui::Context::default(), "deck.pdf", None, deck()).expect("opens"));
    h.run_steps(3);
    assert!(!h.state().full_screen(), "nothing happens before the answer");
    // The dialog and its title share the label: the dialog is the one node of its role.
    h.get_by_role_and_label(egui::accesskit::Role::Dialog, "Open in Full Screen mode?");
    h.get_by_label("Not now").click();
    h.run_steps(3);
    assert!(!h.state().full_screen());
    assert!(h.query_by_label("Open in Full Screen mode?").is_none(), "the prompt is gone");
    // Asked again for the next such document; Full Screen enters it.
    let again = String::from_utf8(FIXTURE.to_vec()).unwrap().replace("/Outlines 6 0 R", "/Outlines 6 0 R /PageMode /FullScreen").into_bytes();
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "deck2.pdf", None, again).expect("opens");
    h.run_steps(3);
    h.get_by_label("Full Screen").click();
    h.run_steps(3);
    assert!(h.state().full_screen(), "Full Screen enters it");
    assert!(h.query_by_label("Open in Full Screen mode?").is_none());
    // A plain document asks nothing.
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "plain.pdf", None, FIXTURE.to_vec()).expect("opens");
    h.run_steps(3);
    assert!(h.query_by_label("Open in Full Screen mode?").is_none() && !h.state().full_screen());
}

/// Switching to Read mode closes the open pane, and switching back reopens it.
#[test]
fn read_mode_closes_the_panes_and_leaving_it_brings_them_back() {
    use pdfcraft_ui_egui::{Mode, RightPanel};
    let mut h = harness(|app| app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    assert_eq!(right(h.state()), Some(RightPanel::Comments));
    run_command(&mut h, "view.read_mode");
    h.run_steps(3);
    assert_eq!(h.state().mode, Mode::Read);
    assert_eq!(right(h.state()), None);
    assert!(h.query_by_label_contains("Check the numbers").is_none(), "the Comments panel is hidden");
    run_command(&mut h, "view.read_mode");
    h.run_steps(3);
    assert_eq!(h.state().mode, Mode::AllTools);
    assert_eq!(right(h.state()), Some(RightPanel::Comments));
    // A pane opened in Read mode stays open on the way out.
    run_command(&mut h, "view.read_mode");
    h.run_steps(2);
    h.get_by_label("Page thumbnails").click();
    h.run_steps(2);
    h.get_by_label("All tools").click();
    h.run_steps(3);
    assert_eq!(right(h.state()), Some(RightPanel::Pages));
}

/// Read mode hides the quick toolbar too, and puts down the tool that was in use (#2646).
#[test]
fn read_mode_hides_the_quick_toolbar() {
    use pdfcraft_ui_egui::{Mode, QuickTool};
    let mut h = harness(|app| app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    // The toolbar's slots are named by their commands (L-233).
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Hand").click();
    h.run_steps(2);
    assert_eq!(h.state().quick_tool, QuickTool::Hand);
    run_command(&mut h, "view.read_mode");
    h.run_steps(3);
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Button, "Select").is_none(), "no quick toolbar in Read mode");
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    run_command(&mut h, "view.read_mode");
    h.run_steps(3);
    assert_eq!(h.state().mode, Mode::AllTools);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Select");
}

/// All tools ▸ Edit a PDF turns on Edit text & images at once, as the Edit tab does, so the page
/// is editable without clicking Edit again. Another tool from there puts editing down.
#[test]
fn edit_a_pdf_from_all_tools_turns_on_editing() {
    use pdfcraft_ui_egui::{Mode, QuickTool};
    let mut h = harness(|app| app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    assert_eq!(h.state().mode, Mode::AllTools);
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    h.get_by_label("Edit a PDF").click();
    h.run_steps(3);
    assert_eq!(h.state().mode, Mode::AllTools, "the workspace stays All tools, with its way back");
    assert_eq!(h.state().quick_tool, QuickTool::EditText, "Edit text & images is on at once");
    h.set_option("tool", "organize").unwrap();
    h.run_steps(3);
    assert_eq!(h.state().quick_tool, QuickTool::Select, "another tool puts editing down");
    // In the Edit workspace, another tool from its panel leaves editing on, as before.
    h.set_option("mode", "edit").unwrap();
    h.set_option("tool", "edit").unwrap();
    h.run_steps(2);
    assert_eq!(h.state().quick_tool, QuickTool::EditText);
    h.set_option("tool", "organize").unwrap();
    h.run_steps(3);
    assert_eq!(h.state().quick_tool, QuickTool::EditText);
    // Read mode hides the quick toolbar and keeps the tools down, so the control channel's
    // `tool edit` there leaves editing off (review on #4605).
    run_command(&mut h, "view.read_mode");
    h.run_steps(2);
    assert_eq!(h.state().mode, Mode::Read);
    h.set_option("tool", "edit").unwrap();
    h.run_steps(2);
    assert_eq!(h.state().quick_tool, QuickTool::Select, "no editing tool in Read mode");
}

/// Opening a document with Read as the default mode puts down the tool in use, as entering Read
/// mode from the mode bar does: the hidden toolbar would leave no way to see or change it.
#[test]
fn opening_into_read_mode_puts_the_tool_down() {
    use pdfcraft_ui_egui::{Mode, QuickTool};
    let mut h = harness(|app| app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    assert_eq!(h.state().mode, Mode::AllTools);
    h.set_option("quick", "hand").unwrap();
    h.set_option("default-mode", "read").unwrap();
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "again.pdf", None, FIXTURE.to_vec()).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().mode, Mode::Read, "the new document opens in the default mode");
    assert_eq!(h.state().quick_tool, QuickTool::Select, "no tool stays active behind the hidden toolbar");
}

/// A pane closed in Read mode stays closed on the way out: leaving Read mode doesn't bring back the
/// pane saved on the way in, and closing Comments keeps its close preference.
#[test]
fn a_pane_closed_in_read_mode_stays_closed_when_leaving_it() {
    use pdfcraft_ui_egui::{Mode, RightPanel};
    let mut h = harness(|app| app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    assert_eq!(right(h.state()), Some(RightPanel::Comments));
    run_command(&mut h, "view.read_mode");
    h.run_steps(3);
    assert_eq!(right(h.state()), None);
    // Open Comments from the rail, then close it again.
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Comments").click();
    h.run_steps(3);
    assert_eq!(right(h.state()), Some(RightPanel::Comments));
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Comments").click();
    h.run_steps(3);
    assert_eq!(right(h.state()), None);
    assert!(h.state().comments_panel_closed);
    // Leave Read mode through the toolbar.
    h.get_by_label("All tools").click();
    h.run_steps(3);
    assert_eq!(h.state().mode, Mode::AllTools);
    assert_eq!(right(h.state()), None, "the pane closed in Read mode stays closed");
    assert!(h.state().comments_panel_closed);
    // The same through the Read mode command.
    h.state_mut().choose_right_panel(Some(RightPanel::Comments));
    run_command(&mut h, "view.read_mode");
    h.run_steps(2);
    assert_eq!(h.state().mode, Mode::Read);
    h.state_mut().choose_right_panel(Some(RightPanel::Comments));
    h.state_mut().choose_right_panel(None);
    run_command(&mut h, "view.read_mode");
    h.run_steps(3);
    assert_ne!(h.state().mode, Mode::Read);
    assert_eq!(right(h.state()), None);
    assert!(h.state().comments_panel_closed);
}

/// A middle click on a tab closes it, as in Acrobat and browsers (#1853); a tab with unsaved
/// changes asks first, as its close button does.
#[test]
fn a_middle_click_closes_the_tab() {
    let mut h = harness(|app| {
        app.open_bytes(&egui::Context::default(), "first.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes(&egui::Context::default(), "second.pdf", None, FIXTURE.to_vec()).unwrap();
    });
    h.get_by_label("first.pdf").click_button(egui::PointerButton::Middle);
    h.run_steps(3);
    let app = h.state();
    assert_eq!(app.views.len(), 1);
    assert_eq!(app.session.get(app.views[0].id).unwrap().name, "second.pdf");
    assert!(h.query_by_label("Cancel").is_none(), "a clean document closes without asking");
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
    h.run_steps(2);
    h.get_by_label("second.pdf, modified").click_button(egui::PointerButton::Middle);
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 1, "unsaved changes ask first");
    h.get_by_label("Cancel");
}

#[test]
fn a_link_prompt_above_a_save_prompt_owns_its_keys() {
    for key in [egui::Key::Escape, egui::Key::Enter] {
        let mut h = harness(|app| {
            app.open_bytes(&egui::Context::default(), "first.pdf", None, FIXTURE.to_vec()).unwrap();
            app.apply_edit(&egui::Context::default(), pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
        });
        h.run_steps(3);
        // The save prompt is the close dialog closing the tab opens.
        assert!(h.execute("file.close"));
        h.run_steps(3);
        h.state_mut().pending_link =
            Some(pdfcraft_ui_egui::PendingLink { url: "https://example.org".to_owned(), origin: pdfcraft_ui_egui::LinkOrigin::Link });
        h.run_steps(3);
        h.key_press(key);
        h.run_steps(3);
        // Escape and Enter both answer Cancel on the link prompt.
        assert!(h.state().pending_link.is_none(), "{key:?} answered the link prompt");
        h.get_by_label("Cancel");
        assert_eq!(h.state().views.len(), 1, "the edited document remains open");
        let opened = h.output().platform_output.commands.iter().any(|c| matches!(c, egui::OutputCommand::OpenUrl(_)));
        assert!(!opened, "nothing opens");
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
            app.open_bytes(&egui::Context::default(), &name(i), None, FIXTURE.to_vec()).unwrap();
        }
    });
    let open = h.get_by_label("Open…").rect();
    let discord = h.get_by_label("Discord").rect();
    assert!(open.right() <= discord.left(), "Open {open:?} must not run under Discord {discord:?}");
    let mut previous_right = f32::MIN;
    for i in 0..5 {
        let tab = h.get_by_role_and_label(egui::accesskit::Role::Tab, &name(i)).rect();
        assert!(tab.left() >= previous_right - 0.5, "tabs must not overlap: {i} at {tab:?}");
        assert!(tab.right() <= open.left() + 0.5, "every tab fits before Open: {i} at {tab:?}, Open at {open:?}");
        previous_right = tab.right();
    }
    // Seven and twenty tabs: Open stays in sight left of Discord.
    for i in 5..7 {
        let ctx_h = h.ctx.clone();
        h.state_mut().open_bytes(&ctx_h, &name(i), None, FIXTURE.to_vec()).unwrap();
    }
    h.run_steps(6);
    let (open, discord) = (h.get_by_label("Open…").rect(), h.get_by_label("Discord").rect());
    assert!(open.right() <= discord.left(), "Open {open:?} stays left of Discord {discord:?} with seven tabs");
    for i in 0..13 {
        let ctx_h = h.ctx.clone();
        h.state_mut().open_bytes(&ctx_h, &format!("more-{i:02}.pdf"), None, FIXTURE.to_vec()).unwrap();
    }
    h.run_steps(4);
    let open = h.get_by_label("Open…").rect();
    let discord = h.get_by_label("Discord").rect();
    assert!(open.left() > 0.0 && open.right() <= discord.left(), "Open {open:?} stays in sight left of Discord {discord:?}");
}

<<<<<<< HEAD
=======
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

/// macOS with a right-to-left system language: AppKit mirrors the traffic lights to the right end
/// of the title bar, and the strip mirrors with them. Home sits beside the lights, the tabs read
/// from the right after it, then Open; Discord, shortcuts, theme and Preferences move to the far
/// left end.
#[cfg(target_os = "macos")]
fn mirrored_title_bar(names: &[&'static str]) -> Harness<'static, PdfCraftApp> {
    let names = names.to_vec();
    harness(move |app| {
        app.integrated_titlebar = true;
        app.titlebar_lights_right = Some(true);
        for name in names {
            app.open_bytes(name, None, FIXTURE.to_vec()).unwrap();
        }
    })
}

#[cfg(target_os = "macos")]
#[test]
fn the_integrated_title_bar_mirrors_with_traffic_lights_at_the_right() {
    let (width, band, lights) = (1400.0, 28.0, 62.0);
    let h = mirrored_title_bar(&["first.pdf", "last.pdf"]);
    let rect = |label: &str| h.get_by_label(label).rect();
    let theme = h.get_by_label_contains("Display theme:").rect();
    for (label, r) in [
        ("Home", rect("Home")),
        ("first.pdf", rect("first.pdf")),
        ("Open", rect("Open")),
        ("Discord", rect("Discord")),
        ("Display theme", theme),
        ("Preferences", rect("Preferences")),
    ] {
        assert!(r.top() >= 0.0 && r.bottom() <= band, "{label}: {r:?} inside the {band} pt title bar");
        assert!(r.right() <= width - lights - 4.0, "{label}: {r:?} clears the traffic lights");
    }
    // Home is the first thing after the lights, as it is on the left in a left-to-right system.
    let home = rect("Home");
    assert!(home.right() >= width - lights - 16.0, "Home {home:?} sits beside the lights");
    // Then the tabs, read from the right, then Open.
    assert!(rect("first.pdf").right() <= home.left(), "the first tab follows Home");
    assert!(rect("last.pdf").right() <= rect("first.pdf").left(), "the tabs read from the right");
    assert!(rect("first.pdf").right() >= home.left() - 12.0, "the tabs stay beside Home");
    assert!(rect("Open").right() <= rect("last.pdf").left(), "Open follows the tabs");
    // The other end: Preferences, theme, shortcuts and Discord, mirroring their order at the right.
    let (preferences, help, discord) = (rect("Preferences"), rect("Keyboard shortcuts"), rect("Discord"));
    assert!(preferences.left() < 20.0, "Preferences {preferences:?} at the far end");
    assert!(
        preferences.right() <= theme.left() && theme.right() <= help.left() && help.right() <= discord.left(),
        "Preferences, theme, shortcuts, Discord"
    );
    assert!(discord.right() < rect("Open").left(), "Discord stays clear of Open");
}

/// Mirrored, a tab dragged to the left moves later in the order, as it reads from the right.
#[cfg(target_os = "macos")]
#[test]
fn dragging_a_tab_in_the_mirrored_title_bar_follows_its_reading_order() {
    use egui::accesskit::Role;
    let mut h = mirrored_title_bar(&["first.pdf", "middle.pdf", "last.pdf"]);
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();
    let tab = |h: &Harness<'static, PdfCraftApp>, name: &str| h.get_by_role_and_label(Role::Tab, name).rect();
    // first.pdf past the end, which is the left.
    let to = tab(&h, "last.pdf").left_center() - egui::vec2(20.0, 0.0);
    drag_named_tab(&mut h, "first.pdf", to, true);
    assert_eq!(h.state().views.iter().map(|view| view.id).collect::<Vec<_>>(), [ids[1], ids[2], ids[0]]);
    assert!(tab(&h, "first.pdf").right() < tab(&h, "last.pdf").left(), "the strip draws the new order");
    // last.pdf, now in the middle, to the front, which is the right.
    let to = tab(&h, "middle.pdf").right_center() + egui::vec2(10.0, 0.0);
    drag_named_tab(&mut h, "last.pdf", to, true);
    assert_eq!(h.state().views.iter().map(|view| view.id).collect::<Vec<_>>(), [ids[2], ids[1], ids[0]]);
}

/// A mirrored strip that starts to scroll (here Home shows and the window narrows) shows its
/// reading start: the first tab beside Home, as the unmirrored strip does at its left end. It goes
/// there once, so a step the user scrolls stands.
#[cfg(target_os = "macos")]
#[test]
fn a_mirrored_strip_that_starts_to_scroll_shows_the_first_tab_beside_home() {
    let mut h = mirrored_title_bar(&["first.pdf", "second.pdf", "third.pdf", "last.pdf"]);
    h.state_mut().active = None;
    h.run_steps(4);
    assert!(h.query_by_label("Scroll tabs left").is_none(), "wide, the tabs don't scroll yet");
    h.set_size(egui::vec2(700.0, 600.0));
    h.run_steps(6);
    let home = h.get_by_label("Home").rect();
    let first = h.get_by_label("first.pdf").rect();
    assert!(first.right() <= home.left() && first.right() >= home.left() - 50.0, "first.pdf {first:?} beside Home {home:?}");
    h.get_by_label("Scroll tabs left").click();
    h.run_steps(6);
    let scrolled = h.get_by_label("first.pdf").rect();
    assert!(scrolled.left() > first.left() + 1.0, "the step stands: {first:?} -> {scrolled:?}");
}

/// Going to the reading start never hides the active tab: a document shown while the strip starts
/// to scroll stays in view.
#[cfg(target_os = "macos")]
#[test]
fn a_mirrored_strip_that_starts_to_scroll_keeps_the_active_tab_in_view() {
    let mut h = mirrored_title_bar(&["first.pdf", "second.pdf", "third.pdf", "last.pdf"]);
    h.run_steps(4);
    assert_eq!(h.state().active, Some(3));
    h.set_size(egui::vec2(700.0, 600.0));
    h.run_steps(6);
    let (home, open) = (h.get_by_label("Home").rect(), h.get_by_label("Open").rect());
    let last = h.get_by_label("last.pdf").rect();
    assert!(last.left() >= open.right() && last.right() <= home.left(), "last.pdf {last:?} between Open {open:?} and Home {home:?}");
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

>>>>>>> upstream/main
#[test]
fn garbage_input_is_rejected_without_panicking() {
    let mut app = crate::control::new_app();
    app.set_option(&egui::Context::default(), "language", "en").unwrap();
    assert!(app.open_bytes(&egui::Context::default(), "junk.pdf", None, b"this is not a pdf".to_vec()).is_err());
    assert!(app.open_bytes(&egui::Context::default(), "empty.pdf", None, Vec::new()).is_err());
    let mut truncated = FIXTURE.to_vec();
    truncated.truncate(FIXTURE.len() / 3);
    // A truncated file either repairs or fails cleanly; it must not panic.
    let _ = app.open_bytes(&egui::Context::default(), "truncated.pdf", None, truncated);
}

#[test]
fn theme_and_view_options_apply() {
    let mut h = harness(|app| {
        app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens");
        app.set_option(&egui::Context::default(), "theme", "dark").unwrap();
        app.set_option(&egui::Context::default(), "panel", "pages").unwrap();
        app.set_option(&egui::Context::default(), "page", "2").unwrap();
    });
    h.run_steps(3);
    h.get_by_label("Page 2");
    assert!(h.set_option("panel", "nope").is_err());
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
        app.open_bytes(&egui::Context::default(), "text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
        app.set_option(&egui::Context::default(), "find", "brown").unwrap();
    });
    crate::support::settle(&mut h);
    h.get_by_label_contains("of 2");
    let v = &h.state().views[0];
    let f = v.find.as_ref().expect("find open");
    assert_eq!(f.matches.matches().iter().map(|m| m.page).collect::<Vec<_>>(), vec![0, 1]);
}

#[test]
fn find_reports_no_matches() {
    let mut h = harness(|app| {
        app.open_bytes(&egui::Context::default(), "text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
        app.set_option(&egui::Context::default(), "find", "zebra").unwrap();
    });
    crate::support::settle(&mut h);
    h.get_by_label("No matches");
}

#[test]
fn drag_selects_text_and_copy_returns_it() {
    let mut h = harness(|app| {
        app.open_bytes(&egui::Context::default(), "text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
        app.set_option(&egui::Context::default(), "left", "closed").unwrap();
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
    let mut a = crate::control::new_app();
    a.set_option(&egui::Context::default(), "language", "en").unwrap();
    a.set_option(&egui::Context::default(), "theme", "dark").unwrap();
    let mut b = crate::control::new_app();
    b.set_option(&egui::Context::default(), "language", "en").unwrap();
    b.apply_settings(&a.settings_snapshot());
    assert_eq!(b.theme_preference, craft::theme::ModeChoice::Dark);
    b.apply_settings(&pdfcraft_ui_egui::settings::import_json("{not json").0);
    b.apply_settings(&pdfcraft_ui_egui::settings::import_json("{\"recent\": 5, \"theme\": \"Purple\"}").0);
    assert_eq!(b.theme_preference, craft::theme::ModeChoice::Dark);
}

/// A tool's saved "no fill" (`"fill": null`) clears a fill it had as its default; a missing or
/// malformed fill keeps it (#340).
#[test]
fn a_saved_no_fill_clears_a_comment_tool_default_fill() {
    use pdfcraft_ui_egui::comments::CommentTool;
    let restore = |app: &mut PdfCraftApp, json: &str| app.apply_settings(&pdfcraft_ui_egui::settings::import_json(json).0);
    let filled = |a: &mut PdfCraftApp| {
        let mut s = a.comment_prefs.style(CommentTool::Rectangle);
        s.fill = Some([0.2, 0.4, 0.6]);
        a.comment_prefs.set_style(CommentTool::Rectangle, s);
    };
    let mut a = crate::control::new_app();
    filled(&mut a);
    restore(&mut a, r#"{"comment_styles":[{"tool":"comment.square","fill":null}]}"#);
    assert_eq!(a.comment_prefs.style(CommentTool::Rectangle).fill, None, "null clears the fill");
    let mut b = crate::control::new_app();
    filled(&mut b);
    restore(&mut b, r#"{"comment_styles":[{"tool":"comment.square","color":[0,0,1]},{"tool":"comment.square","fill":"red"}]}"#);
    assert_eq!(b.comment_prefs.style(CommentTool::Rectangle).fill, Some([0.2, 0.4, 0.6]), "missing or malformed keeps it");
    // A round trip keeps a cleared fill cleared.
    let mut c = crate::control::new_app();
    c.apply_settings(&a.settings_snapshot());
    assert_eq!(c.comment_prefs.style(CommentTool::Rectangle).fill, None);
}

/// A tool's saved "Same as text" border colour (`"border_color": null`) clears an own border
/// colour it had; a missing or malformed one keeps it.
#[test]
fn a_saved_same_as_text_clears_a_comment_tool_border_colour() {
    use pdfcraft_ui_egui::comments::CommentTool;
    let red = Some([0.89, 0.13, 0.13]);
    let mut a = crate::control::new_app();
    let restore = |app: &mut PdfCraftApp, json: &str| app.apply_settings(&pdfcraft_ui_egui::settings::import_json(json).0);
    a.comment_prefs.set_border_color(CommentTool::TextBox, red);
    restore(&mut a, r#"{"comment_styles":[{"tool":"comment.freetext","border_color":"red"}]}"#);
    assert_eq!(a.comment_prefs.style(CommentTool::TextBox).border_color, red, "malformed keeps it");
    restore(&mut a, r#"{"comment_styles":[{"tool":"comment.freetext","border_color":null}]}"#);
    assert_eq!(a.comment_prefs.style(CommentTool::TextBox).border_color, None, "null clears it");
    let mut b = crate::control::new_app();
    b.comment_prefs.set_border_color(CommentTool::TextBox, red);
    b.apply_settings(&a.settings_snapshot());
    assert_eq!(b.comment_prefs.style(CommentTool::TextBox).border_color, None, "a round trip keeps it cleared");
}

/// Persisted per-tool comment styles go through their clamping setters: hostile or malformed
/// values are clamped or ignored, never trusted (#340).
#[test]
fn comment_style_settings_clamp_and_refuse_hostile_values() {
    use pdfcraft_ui_egui::comments::CommentTool;
    let mut a = crate::control::new_app();
    a.apply_settings(
        &pdfcraft_ui_egui::settings::import_json(
            r#"{"comment_styles":[
            {"tool":"comment.highlight","color":[2,-1,0.5],"opacity":99,"width":-4},
            {"tool":"comment.note","color":"yellow","opacity":"none"},
            {"tool":"comment.note","color":[0.1,null,0.3]},
            {"tool":"bogus.command","color":[0,0,0]}
        ]}"#,
        )
        .0,
    );
    // Out-of-range numbers are clamped into range.
    let st = a.comment_prefs.style(CommentTool::Highlight);
    assert_eq!((st.color, st.opacity, st.width), ([1.0, 0.0, 0.5], 1.0, 0.5));
    // A malformed colour is refused whole: the tool keeps its default.
    let note = a.comment_prefs.style(CommentTool::Note);
    assert_eq!(note.color, [1.0, 0.82, 0.0], "the default colour stays");
}

/// A tool's fill's and text's own opacities survive a restart, clamped; a tool without a fill or
/// text keeps none, and the quick bar's Opacity (the whole comment's) clears them.
#[test]
fn part_opacity_defaults_persist_and_clamp() {
    use pdfcraft_ui_egui::comments::CommentTool;
    let mut a = crate::control::new_app();
    a.comment_prefs.set_part_opacity(CommentTool::Rectangle, Some(0.3), None);
    a.comment_prefs.set_part_opacity(CommentTool::TextBox, Some(0.2), Some(1.0));
    a.comment_prefs.set_part_opacity(CommentTool::Line, Some(0.5), Some(0.5));
    let mut b = crate::control::new_app();
    b.apply_settings(&a.settings_snapshot());
    let parts = |app: &PdfCraftApp, tool| {
        let s = app.comment_prefs.style(tool);
        (s.fill_opacity, s.text_opacity)
    };
    assert_eq!(parts(&b, CommentTool::Rectangle), (Some(0.3), None));
    assert_eq!(parts(&b, CommentTool::TextBox), (Some(0.2), Some(1.0)));
    assert_eq!(parts(&b, CommentTool::Line), (None, None), "a line has no fill or text of its own");
    b.apply_settings(
        &pdfcraft_ui_egui::settings::import_json(r#"{"comment_styles":[{"tool":"comment.freetext","fill_opacity":7,"text_opacity":-1}]}"#).0,
    );
    assert_eq!(parts(&b, CommentTool::TextBox), (Some(1.0), Some(0.1)), "clamped; the text stays visible");
}

#[test]
fn trackpad_pinch_zooms_the_pdf_under_the_pointer() {
    let mut h = harness(|app| {
        app.open_bytes(&egui::Context::default(), "text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
        app.set_option(&egui::Context::default(), "left", "closed").unwrap();
        app.set_option(&egui::Context::default(), "panel", "none").unwrap();
    });
    crate::support::settle(&mut h);
    let before = h.state().views[0].glyph_screen_pos(0, 4).expect("glyph on screen");
    let zoom = h.state().views[0].zoom();
    h.hover_at(before);
    h.run_steps(1);
    h.event(egui::Event::Zoom(1.5));
    h.run_steps(4);
    let enlarged = &h.state().views[0];
    assert!((enlarged.zoom() - zoom * 1.5).abs() < 0.01);
    let after = enlarged.glyph_screen_pos(0, 4).expect("glyph still on screen");
    assert!((after - before).length() < 2.0, "moved from {before:?} to {after:?}");

    h.event(egui::Event::Zoom(0.5));
    h.run_steps(4);
    assert!((h.state().views[0].zoom() - zoom * 0.75).abs() < 0.01);
}

#[test]
fn selection_works_in_every_view_rotation() {
    for deg in ["90", "180", "270"] {
        let mut h = harness(move |app| {
            app.open_bytes(&egui::Context::default(), "text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "rotate", deg).unwrap();
            // Keep the whole rotated page on screen.
            app.set_option(&egui::Context::default(), "zoom", "60").unwrap();
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
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app
        }
    });
    h.run_steps(3);
    assert!(h.state().views.is_empty());
    let file: egui::DroppedFileHandle = std::sync::Arc::new(Dropped { path: "dropped.pdf".into(), bytes: FIXTURE.to_vec() });
    support::input::drop_files(&mut h, [file]);
    support::wait_until(&mut h, "dropped document", |app| app.views.len() == 1 && app.password_prompt.is_none());
    assert_eq!(h.state().views.len(), 1, "the dropped PDF opens in a tab");
    h.get_by_role_and_label(egui::accesskit::Role::Tab, "dropped.pdf");
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
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app
        }
    });
    h.run_steps(3);
    let file: egui::DroppedFileHandle = std::sync::Arc::new(Dropped { path: path.clone(), bytes: FIXTURE.to_vec() });
    support::input::drop_files(&mut h, [file.clone()]);
    // The drop opens through the background worker on a live window; wait for the tab.
    support::wait_until(&mut h, "dropped document", |app| app.views.len() == 1);
    assert_eq!(h.state().views.len(), 1, "the dropped PDF opens in a tab");
    support::input::drop_files(&mut h, [file]);
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 1, "dropping it again adds no tab");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_file_dropped_on_the_page_grid_is_read_from_disk_and_inserted() {
    // The positional drop waits for a pointer; a drop outside the grid appends the file from disk.
    let dir = std::env::temp_dir().join(format!("pdfcraft-grid-drop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dropped.pdf");
    let setup = crate::control::new_app();
    let pdf = setup.session.create_from_text("dropped", "from disk").unwrap();
    std::fs::write(&path, pdf.as_slice()).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            let grid = app.session.create_from_text("grid", "one page").unwrap();
            app.open_bytes(&egui::Context::default(), "grid.pdf", None, grid.to_vec()).unwrap();
            app.views[0].organize = true;
            app
        }
    });
    h.run_steps(3);
    let pages = |h: &egui_kittest::Harness<'_, PdfCraftApp>| {
        let app = h.state();
        app.session.get(app.views[0].id).unwrap().info.pages.len()
    };
    assert_eq!(pages(&h), 1);
    let file: egui::DroppedFileHandle = std::sync::Arc::new(Dropped { path: path.clone(), bytes: Vec::new() });
    support::input::drop_files(&mut h, [file]);
    assert_eq!(pages(&h), 1, "the drop waits for the pointer");
    h.hover_at(egui::pos2(10.0, 400.0));
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
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app
        }
    });
    h.run_steps(3);
    h.set_option("combine-view", "list").unwrap();
    h.execute("page.combine");
    h.run_steps(2);
    // An absolute path is read from disk, with its modified time.
    let path = std::env::temp_dir().join(format!("pdfcraft-combine-drop-{}.pdf", std::process::id()));
    std::fs::write(&path, FIXTURE).unwrap();
    let on_disk: egui::DroppedFileHandle = std::sync::Arc::new(Dropped { path: path.clone(), bytes: Vec::new() });
    let in_memory: egui::DroppedFileHandle = std::sync::Arc::new(Dropped { path: "second.pdf".into(), bytes: FIXTURE.to_vec() });
    support::input::drop_files(&mut h, [on_disk, in_memory]);
    h.run_steps(3);
    std::fs::remove_file(&path).ok();
    let app = h.state();
    assert!(app.views.is_empty(), "nothing opens");
    assert_eq!(app.combine_draft.len(), 2);
    assert!(app.combine_draft[0].modified.is_some() && app.combine_draft[1].modified.is_none());
}

#[test]
fn a_folder_dropped_on_the_combine_tab_adds_its_pdfs() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-combine-drop-dir-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("inner")).unwrap();
    std::fs::write(dir.join("one.pdf"), FIXTURE).unwrap();
    std::fs::write(dir.join("inner").join("two.pdf"), FIXTURE).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app
        }
    });
    h.run_steps(3);
    h.execute("page.combine");
    h.run_steps(2);
    let folder: egui::DroppedFileHandle = std::sync::Arc::new(Dropped { path: dir.clone(), bytes: Vec::new() });
    support::input::drop_files(&mut h, [folder]);
    h.run_steps(3);
    let _ = std::fs::remove_dir_all(&dir);
    let names: Vec<_> = h.state().combine_draft.iter().map(|f| f.name.clone()).collect();
    assert_eq!(names, ["two.pdf", "one.pdf"], "subfolders included, in path order");
}

/// The Combine files tab, showing, with one file in its list already.
fn combine_tab_with_one_file() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app
        }
    });
    h.run_steps(3);
    h.set_option("combine-view", "list").unwrap();
    h.execute("page.combine");
    let ctx_h = h.ctx.clone();
    h.state_mut().use_files(&ctx_h, pdfcraft_ui_egui::FilePurpose::Combine, vec![("first.pdf".into(), FIXTURE.to_vec())]);
    h.run_steps(2);
    h
}

fn combine_names(h: &Harness<'_, PdfCraftApp>) -> Vec<String> {
    h.state().combine_draft.iter().map(|f| f.name.clone()).collect()
}

#[test]
fn files_dropped_on_the_combine_tab_join_the_end_of_its_list_in_the_order_dropped() {
    let mut h = combine_tab_with_one_file();
    let dir = std::env::temp_dir().join(format!("pdfcraft-combine-drop-order-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Files on disk come with their path (and no bytes); browsers hand over only a name and bytes.
    let on_disk = |name: &str| -> egui::DroppedFileHandle {
        std::fs::write(dir.join(name), FIXTURE).unwrap();
        std::sync::Arc::new(Dropped { path: dir.join(name), bytes: Vec::new() })
    };
    let in_memory =
        |name: &str, bytes: &[u8]| -> egui::DroppedFileHandle { std::sync::Arc::new(Dropped { path: name.into(), bytes: bytes.to_vec() }) };
    let dropped = [on_disk("c.pdf"), in_memory("a.pdf", FIXTURE), in_memory("notes.txt", b"Some notes"), on_disk("b.pdf")];
    support::input::drop_files(&mut h, dropped);
    h.run_steps(3);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(h.state().views.is_empty(), "nothing opens");
    assert_eq!(combine_names(&h), ["first.pdf", "c.pdf", "a.pdf", "notes.txt", "b.pdf"]);
    let draft = &h.state().combine_draft;
    assert!(draft.iter().all(|f| f.problem.is_none()), "every file can be combined");
    assert!(draft[3].pages == 1, "the text file became a page");
    assert!(draft[1].modified.is_some() && draft[2].modified.is_none());
    // One drop, one step to undo.
    h.execute("edit.undo");
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["first.pdf"]);
}

#[test]
fn a_file_combine_does_not_take_is_left_out_of_a_drop_and_named() {
    let mut h = combine_tab_with_one_file();
    let path = std::env::temp_dir().join(format!("pdfcraft-combine-drop-{}.zip", std::process::id()));
    std::fs::write(&path, b"PK\x03\x04").unwrap();
    let dropped: [egui::DroppedFileHandle; 4] = [
        std::sync::Arc::new(Dropped { path: "report.docx".into(), bytes: b"PK\x03\x04".to_vec() }),
        std::sync::Arc::new(Dropped { path: "one.pdf".into(), bytes: FIXTURE.to_vec() }),
        std::sync::Arc::new(Dropped { path: path.clone(), bytes: Vec::new() }),
        // A PDF without the name of one is still a PDF.
        std::sync::Arc::new(Dropped { path: "scan".into(), bytes: FIXTURE.to_vec() }),
    ];
    support::input::drop_files(&mut h, dropped);
    h.run_steps(3);
    std::fs::remove_file(&path).ok();
    assert_eq!(combine_names(&h), ["first.pdf", "one.pdf", "scan"]);
    let zip = path.file_name().unwrap().to_string_lossy().into_owned();
    let notice = posted_notices(&h).join("; ");
    assert!(notice.contains("report.docx") && notice.contains(&zip), "the refused names are told: {notice:?}");
}

#[test]
fn a_pdf_dropped_on_a_document_opens_while_the_combine_tab_waits_behind_it() {
    let mut h = combine_tab_with_one_file();
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "open.pdf", None, FIXTURE.to_vec()).unwrap();
    h.run_steps(2);
    assert!(!h.state().combine_showing() && h.state().combine_tab.open);
    let file: egui::DroppedFileHandle = std::sync::Arc::new(Dropped { path: "dropped.pdf".into(), bytes: FIXTURE.to_vec() });
    support::input::drop_files(&mut h, [file]);
    support::wait_until(&mut h, "dropped document", |app| app.views.len() == 2);
    assert_eq!(combine_names(&h), ["first.pdf"], "the Combine files list is left as it was");
}

/// An app on a host whose jobs run on `jobs`, taking its file requests from `intake`, with an
/// inline file watcher.
fn host_app(jobs: std::sync::Arc<dyn craft::host::Jobs>, intake: craft::platform::open::FileIntake) -> PdfCraftApp {
    use craft::host::{AppInfo, Host, ScriptedFiles, Wake};
    use craft::platform::clipboard::{Clipboard, SystemClipboard};
    use craft::platform::watch::FileWatch;
    let info = AppInfo { id: "pdfcraft", name: "PdfCraft", version: "0" };
    let host = Host::builder(info).jobs(jobs).files(std::sync::Arc::new(ScriptedFiles::new())).build();
    let services = pdfcraft_ui_egui::AppServices::new(
        Wake::none(),
        Clipboard::new(SystemClipboard::new()),
        None,
        None,
        intake,
        FileWatch::new(Wake::none()).inline(),
    );
    PdfCraftApp::with_host(std::sync::Arc::new(host), services)
}

/// A window whose app takes the OS's file requests from the returned sender (`FileIntake`). Its
/// jobs run on threads, as the running app's do, so a requested file opens while the test steps.
fn intake_harness() -> (Harness<'static, PdfCraftApp>, craft::platform::open::IntakeSender) {
    use craft::host::Wake;
    use craft::platform::open::FileIntake;
    let (intake, sender) = FileIntake::new(Wake::none());
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = host_app(std::sync::Arc::new(craft::host::ThreadJobs), intake);
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app
        }
    });
    h.run_steps(4);
    (h, sender)
}

#[test]
fn files_and_quit_from_the_operating_system() {
    // #73: macOS hands Finder double-clicks, Open With and Dock drops over as Apple events.
    let name = format!("pdfcraft-os-open-{}.pdf", std::process::id());
    let path = std::env::temp_dir().join(&name);
    std::fs::write(&path, FIXTURE).unwrap();
    let (mut h, sender) = intake_harness();
    sender.open(vec![path.clone()]);
    // The file is read on a worker thread, after however many frames that takes: it must still
    // be there until the tab has opened.
    support::wait_until(&mut h, "the file the operating system opened", |app| !app.views.is_empty());
    std::fs::remove_file(&path).ok();
    assert_eq!(h.state().views.len(), 1, "the file opens in a tab");
    h.get_by_role_and_label(egui::accesskit::Role::Tab, &name);
    // A file opened while the app runs brings its window forward, even minimized or full screen
    // on another Space.
    std::fs::write(&path, FIXTURE).unwrap();
    sender.open(vec![path.clone()]);
    h.step();
    std::fs::remove_file(&path).ok();
    let commands: Vec<_> = h.output().viewport_output.values().flat_map(|v| v.commands.clone()).collect();
    assert!(commands.contains(&egui::ViewportCommand::Minimized(false)), "{commands:?}");
    assert!(commands.contains(&egui::ViewportCommand::Focus), "{commands:?}");
    // Quit closes the window like the close button, so unsaved changes are asked about.
    craft::quit::on_menu_event(&h.ctx, craft::quit::QUIT_MENU_ID);
    h.step();
    let closing = h.output().viewport_output.values().any(|v| v.commands.iter().any(|c| matches!(c, egui::ViewportCommand::Close)));
    assert!(closing);
}

/// #585: File Explorer's "Combine files in PdfCraft" starts one launch per selected file, and the
/// running PdfCraft collects them (an `open_for(.., "combine")` each) into its Combine files tab.
#[test]
fn files_to_combine_from_the_operating_system() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-os-combine-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let paths: Vec<std::path::PathBuf> = ["one.pdf", "two.pdf"]
        .iter()
        .map(|n| {
            let p = dir.join(n);
            std::fs::write(&p, FIXTURE).unwrap();
            p
        })
        .collect();
    let (mut h, sender) = intake_harness();
    sender.open_for(vec![paths[0].clone()], "combine");
    h.run_steps(3);
    assert!(h.state().combine_showing(), "the Combine files tab shows");
    assert!(h.state().views.is_empty(), "nothing opens as a document");
    sender.open_for(vec![paths[1].clone()], "combine");
    h.run_steps(3);
    let _ = std::fs::remove_dir_all(&dir);
    let names: Vec<_> = h.state().combine_draft.iter().map(|f| f.name.clone()).collect();
    assert_eq!(names, ["one.pdf", "two.pdf"], "each launch's file joins the list, in order");
    assert!(h.state().combine_showing() && h.state().views.is_empty());
}

/// #2503 review: a PDF still opening when Combine arrives opens behind the Combine files tab
/// instead of hiding it; an Open that comes after Combine still shows its document.
#[test]
fn an_open_finishing_after_combine_keeps_the_combine_tab() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-os-combine-pending-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let paths: Vec<std::path::PathBuf> = ["opening.pdf", "combined.pdf", "later.pdf"]
        .iter()
        .map(|n| {
            let p = dir.join(n);
            std::fs::write(&p, FIXTURE).unwrap();
            p
        })
        .collect();
    let (mut h, sender) = intake_harness();
    h.run_steps(2);
    // Both in one frame: the open goes to the worker and is still pending when Combine shows.
    sender.open(vec![paths[0].clone()]);
    sender.open_for(vec![paths[1].clone()], "combine");
    for _ in 0..200 {
        h.run_steps(1);
        if !h.state().views.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(h.state().views.len(), 1, "the earlier open finished");
    assert!(h.state().combine_showing(), "and the Combine files tab still shows");
    sender.open(vec![paths[2].clone()]);
    for _ in 0..200 {
        h.run_steps(1);
        if h.state().views.len() == 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(h.state().views.len(), 2);
    assert!(!h.state().combine_showing() && h.state().active == Some(1), "a later Open shows its document");
}

#[test]
fn default_workspace_mode_persists_and_tolerates_invalid_settings() {
    use pdfcraft_ui_egui::Mode;
    for mode in [Mode::AllTools, Mode::Read, Mode::Edit, Mode::Convert, Mode::Sign] {
        let mut app = crate::control::new_app();
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        app.default_mode = mode;
        app.set_option(&egui::Context::default(), "mode", "sign").unwrap();
        let mut restored = crate::control::new_app();
        restored.set_option(&egui::Context::default(), "language", "en").unwrap();
        restored.apply_settings(&app.settings_snapshot());
        assert_eq!(restored.default_mode, mode);
        restored.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).unwrap();
        assert_eq!(restored.mode, mode, "session override must not be persisted");
    }
    for json in ["{}", "{not json", r#"{"default_mode": null}"#, r#"{"default_mode": 5}"#, r#"{"default_mode": "unknown"}"#] {
        let mut app = crate::control::new_app();
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        app.apply_settings(&pdfcraft_ui_egui::settings::import_json(json).0);
        assert_eq!(app.default_mode, Mode::AllTools);
        app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).unwrap();
        assert_eq!(app.mode, Mode::AllTools);
    }
    // Chosen in Preferences, the default applies to the next open.
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "dialog", "preferences").unwrap());
    pick_preference(&mut h, Some("Documents and view"), "Default workspace mode", "Read");
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert_eq!(h.state().default_mode, Mode::Read);
    assert_eq!(h.state().mode, Mode::AllTools);
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "fixture.pdf", None, FIXTURE.to_vec()).unwrap();
    assert_eq!(h.state().mode, Mode::Read);
}

#[test]
fn newly_opened_pdfs_use_the_default_workspace() {
    use pdfcraft_ui_egui::{LeftPanel, Mode};
    let mut app = crate::control::new_app();
    app.set_option(&egui::Context::default(), "language", "en").unwrap();
    app.apply_settings(&pdfcraft_ui_egui::settings::import_json(r#"{"default_mode": "edit"}"#).0);
    app.open_bytes(&egui::Context::default(), "first.pdf", None, FIXTURE.to_vec()).unwrap();
    assert_eq!(app.mode, Mode::Edit);
    assert_eq!(app.left, LeftPanel::Tool("edit"));
    app.default_mode = Mode::Read;
    assert_eq!(app.mode, Mode::Edit, "changing the preference affects future opens");
    app.open_bytes(&egui::Context::default(), "second.pdf", None, FIXTURE.to_vec()).unwrap();
    assert_eq!(app.mode, Mode::Read);
    // PDF Initial View is independent of the workspace preference.
    let pdf =
        String::from_utf8(FIXTURE.to_vec()).unwrap().replace("/Outlines 6 0 R", "/Outlines 6 0 R /PageLayout /TwoColumnLeft /PageMode /UseOutlines");
    app.open_bytes(&egui::Context::default(), "initial-view.pdf", None, pdf.into_bytes()).unwrap();
    assert_eq!(app.mode, Mode::Read);
    assert_eq!(app.views.last().unwrap().view.display, craft::pages::PageDisplay::TwoUp);
    assert_eq!(right(&app), Some(pdfcraft_ui_egui::RightPanel::Bookmarks));
}

#[test]
fn explicit_mode_overrides_default_before_and_after_open() {
    use pdfcraft_ui_egui::Mode;
    for before in [false, true] {
        for (value, mode) in [("all", Mode::AllTools), ("read", Mode::Read), ("edit", Mode::Edit), ("convert", Mode::Convert), ("sign", Mode::Sign)] {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.default_mode = Mode::Read;
            if before {
                app.set_option(&egui::Context::default(), "mode", value).unwrap();
            }
            app.open_bytes(&egui::Context::default(), "first.pdf", None, FIXTURE.to_vec()).unwrap();
            if !before {
                // Desktop startup applies CLI options after the initial files open.
                app.set_option(&egui::Context::default(), "mode", value).unwrap();
            }
            assert_eq!(app.mode, mode);
            app.set_option(&egui::Context::default(), "default-mode", "edit").unwrap();
            app.open_bytes(&egui::Context::default(), "second.pdf", None, FIXTURE.to_vec()).unwrap();
            assert_eq!(app.mode, mode);
            assert_eq!(app.default_mode, Mode::Edit);
        }
    }
}

/// Preferences ▸ Animation effects: the system's animation setting until the user chooses On or
/// Off. The choice is saved, and settings holding anything else leave it alone.
#[test]
fn preferences_choose_animation_effects_and_the_choice_is_saved() {
    use craft::theme::MotionChoice;
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "dialog", "preferences").unwrap());
    // PLAN: assumed the app keeps the choice in `animation_effects: MotionChoice` and the settings JSON key `animation_effects` (pdf-shell-w9)
    assert_eq!(h.state().animation_effects, MotionChoice::System);
    for (label, choice) in
        [("Off", MotionChoice::Off), ("On", MotionChoice::Full), ("Use system setting", MotionChoice::System), ("Off", MotionChoice::Off)]
    {
        pick_preference(&mut h, None, "Animation effects", label);
        h.get_by_label("OK").click();
        h.run_steps(2);
        assert_eq!(h.state().animation_effects, choice, "{label}");
        h.set_option("dialog", "preferences").unwrap();
        h.run_steps(2);
    }
    let saved = serde_json::to_string(&h.state().settings_snapshot()).unwrap();
    assert!(saved.contains(r#""animation_effects":"off""#), "{saved}");
    let mut restored = crate::control::new_app();
    restored.apply_settings(&serde_json::from_str::<pdfcraft_ui_egui::settings::PdfCraftSettings>(&saved).unwrap());
    assert_eq!(restored.animation_effects, MotionChoice::Off);
    // Settings from an earlier release (the flat JSON object) are untrusted: an unknown name leaves the system choice.
    let mut legacy = crate::control::new_app();
    legacy.apply_settings(&pdfcraft_ui_egui::settings::import_json(r#"{"animation_effects":"off"}"#).0);
    assert_eq!(legacy.animation_effects, MotionChoice::Off);
    let mut fresh = crate::control::new_app();
    fresh.apply_settings(&pdfcraft_ui_egui::settings::import_json(r#"{"animation_effects":"sideways"}"#).0);
    assert_eq!(fresh.animation_effects, MotionChoice::System);
    // The control channel and the command line take the same three names.
    fresh.set_option(&egui::Context::default(), "animation_effects", "ON").unwrap();
    assert_eq!(fresh.animation_effects, MotionChoice::Full);
    assert!(fresh.set_option(&egui::Context::default(), "animation_effects", "sideways").unwrap_err().contains("system, on, or off"));
    assert_eq!(fresh.animation_effects, MotionChoice::Full);
}

#[test]
fn explicit_mode_preserves_independent_tool_and_panel_options() {
    use pdfcraft_ui_egui::LeftPanel;
    let mut app = crate::control::new_app();
    app.set_option(&egui::Context::default(), "language", "en").unwrap();
    app.set_option(&egui::Context::default(), "tool", "export").unwrap();
    app.set_option(&egui::Context::default(), "left", "closed").unwrap();
    app.set_option(&egui::Context::default(), "mode", "edit").unwrap();
    assert_eq!(app.left, LeftPanel::Tool("export"));
    assert!(!app.left_open());
    app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).unwrap();
    assert_eq!(app.left, LeftPanel::Tool("export"));
    assert!(!app.left_open());
}

#[test]
fn opening_a_pdf_keeps_a_closed_left_panel_and_the_chosen_tool() {
    use pdfcraft_ui_egui::{LeftPanel, Mode};
    // `--left closed` without `--mode`, default workspace Edit: the panel stays closed.
    let mut app = crate::control::new_app();
    app.set_option(&egui::Context::default(), "language", "en").unwrap();
    app.default_mode = Mode::Edit;
    app.set_option(&egui::Context::default(), "left", "closed").unwrap();
    app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).unwrap();
    assert_eq!(app.mode, Mode::Edit);
    assert!(!app.left_open());
    // Default All Tools: opening another PDF leaves the tool panel the user picked.
    let mut app = crate::control::new_app();
    app.set_option(&egui::Context::default(), "language", "en").unwrap();
    app.set_option(&egui::Context::default(), "tool", "export").unwrap();
    app.open_bytes(&egui::Context::default(), "first.pdf", None, FIXTURE.to_vec()).unwrap();
    app.open_bytes(&egui::Context::default(), "second.pdf", None, FIXTURE.to_vec()).unwrap();
    assert_eq!(app.mode, Mode::AllTools);
    assert_eq!(app.left, LeftPanel::Tool("export"));
}

fn three_tab_harness() -> Harness<'static, PdfCraftApp> {
    harness(|app| {
        for name in ["first.pdf", "middle.pdf", "last.pdf"] {
            app.open_bytes(&egui::Context::default(), name, None, FIXTURE.to_vec()).unwrap();
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
        assert!(h.query_by_label("Don't save").is_none(), "clean tabs close without a prompt");
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
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Unsaved title".into() }));
    h.run_steps(3);
    h.get_by_label("middle.pdf").click();
    h.run_steps(3);
    let discard = "Don't save";
    close_named_tab(&mut h, "first.pdf, modified");
    h.get_by_label(discard);
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 3);
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[1]));
    assert!(h.state().session.get(ids[0]).unwrap().dirty);
    close_named_tab(&mut h, "first.pdf, modified");
    h.get_by_label(discard).click();
    h.run_steps(3);
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[1]));
    assert_eq!(h.state().views.iter().map(|view| view.id).collect::<Vec<_>>(), [ids[1], ids[2]]);
    assert!(h.state().session.get(ids[0]).is_none());
    assert!(h.query_by_label_contains("before closing?").is_none(), "no prompt is left");
    for doc in h.state().session.docs() {
        assert_eq!(doc.bytes.as_slice(), FIXTURE);
        assert!(!doc.dirty);
    }
}

#[test]
fn long_german_tool_labels_stay_within_the_sidebar() {
    if crate::support::skip_without_catalogs("long_german_tool_labels_stay_within_the_sidebar") {
        return;
    }
    let mut h = harness(|app| app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    h.set_option("language", "de").unwrap();
    h.set_option("mode", "all").unwrap();
    h.set_option("left", "open").unwrap();
    // The tool heading has to leave room for Back and Close even when translated.
    h.set_option("tool", "fill_sign").unwrap();
    h.run_steps(4);
    let heading =
        painted_text(&h).into_iter().find(|(s, ..)| s.starts_with("Ausfüllen und")).expect("translated heading is painted, whole or elided");
    assert!(heading.2.right() <= 330.0, "translated heading runs past the sidebar: {:?}", heading.2);
    // A long sub-tool name must stop before its status chip, not paint over the PDF.
    h.set_option("tool", "protect").unwrap();
    h.run_steps(4);
    let item =
        painted_text(&h).into_iter().find(|(s, ..)| s == "Ausgeblendete Informationen entfernen").expect("long translated sub-tool is painted");
    assert!(item.2.right() <= 330.0, "translated tool item runs past the sidebar: {:?}", item.2);
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
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Unsaved title".into() }));
    h.run_steps(3);
    let dirty = h.get_by_role_and_label(Role::Tab, "middle.pdf, modified");
    assert_eq!(dirty.accesskit_node().id(), middle_id);
    assert!(dirty.is_focused());
    close_named_tab(&mut h, "first.pdf");
    assert_eq!(h.get_by_role_and_label(Role::Tab, "middle.pdf, modified").accesskit_node().id(), middle_id);
    assert_eq!(h.state().active, Some(0));
    assert_eq!(h.state().views.len(), 2);
    assert!(h.state().session.get(h.state().views[0].id).unwrap().dirty);
}

#[test]
fn combine_tab_uses_tab_interaction_without_changing_close_or_document_state() {
    use egui::accesskit::Role;
    let mut h = three_tab_harness();
    let document_ids: Vec<_> = h.state().views.iter().map(|v| v.id).collect();
    h.execute("page.combine");
    h.run_steps(3);
    assert_eq!(h.get_by_role_and_label(Role::Tab, "Combine files").accesskit_node().is_selected(), Some(true));
    h.get_by_role_and_label(Role::Tab, "first.pdf").click();
    h.run_steps(3);
    // The strip has one keyboard stop; move its lead to Combine without selecting it.
    h.get_by_role_and_label(Role::Tab, "first.pdf").focus();
    h.run_steps(2);
    for _ in 0..3 {
        h.key_press(egui::Key::ArrowRight);
        h.run_steps(1);
    }
    assert!(h.get_by_role_and_label(Role::Tab, "Combine files").is_focused());
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
    // Pressing first.pdf selects it; dragging moves it without changing that selection.
    // The strip's between-tab drop zones are inside the tabs, not over the trailing Open button.
    let to = tab(&h, "last.pdf").right_center() - egui::vec2(10.0, 0.0);
    drag_named_tab(&mut h, "first.pdf", to, true);
    assert_eq!(h.state().views.iter().map(|view| view.id).collect::<Vec<_>>(), [ids[1], ids[2], ids[0]]);
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[0]));
    assert!(tab(&h, "first.pdf").left() > tab(&h, "last.pdf").right(), "the strip draws the new order");
    // last.pdf, now in the middle, to the front.
    let to = tab(&h, "middle.pdf").left_center() + egui::vec2(10.0, 0.0);
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
    h.execute("page.combine");
    h.run_steps(3);
    let first = h.get_by_role_and_label(egui::accesskit::Role::Tab, "first.pdf").rect();
    drag_named_tab(&mut h, "Combine files", first.left_center() - egui::vec2(10.0, 0.0), true);
    assert_eq!(h.state().views.iter().map(|view| view.id).collect::<Vec<_>>(), ids);
}

/// The Edit tab turns on Edit text & images at once, as Acrobat does, so every paragraph and image
/// is boxed and editable with one click; another tab puts the tool down again.
#[test]
fn the_edit_tab_turns_on_edit_text_and_images() {
    use pdfcraft_ui_egui::{Mode, QuickTool};
    let mut h = harness(|app| app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
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
    let mut h = harness(|app| app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
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
    let mut h = harness(|app| app.open_bytes(&egui::Context::default(), "fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    support::outside_menus(&h, "Edit").click();
    h.run_steps(3);
    h.get_by_label("Watermark").click();
    h.run_steps(3);
    assert_eq!(enabled(&h, "Add watermark…"), Some(true));
    assert_eq!(enabled(&h, "Remove watermark"), Some(false), "nothing to remove yet");
    h.get_by_label("Add watermark…").click();
    h.run_steps(3);
    // The watermark question is open on the dialog host: its Cancel is on screen.
    h.get_by_label("Cancel");
}

/// #3282: in a short window the rail's panel buttons scroll above the page navigation instead of
/// being drawn under the page number field.
#[test]
fn short_window_rail_buttons_scroll_clear_of_page_navigation() {
    // The rail starts on the right; the dock button moves it to the left.
    for on_left in [false, true] {
        let (width, height) = (1000.0, 560.0);
        let mut h = Harness::builder().with_size(egui::vec2(width, height)).build_eframe(move |cc| {
            crate::support::install_runtime(&cc.egui_ctx);
            {
                let mut app = crate::control::new_app();
                app.set_option(&egui::Context::default(), "language", "en").unwrap();
                app.open_bytes(&egui::Context::default(), "short.pdf", None, FIXTURE.to_vec()).unwrap();
                app
            }
        });
        crate::support::settle(&mut h);
        if on_left {
            h.get_by_label("Dock navigation rail on Left").click();
            crate::support::settle(&mut h);
        }
        // The comfortable-density rail is 50 points wide.
        let rail_width = 50.0;
        let in_rail = |r: egui::Rect| if on_left { r.right() <= rail_width } else { r.left() >= width - rail_width };
        let rail = |h: &Harness<'static, PdfCraftApp>, label: &str| {
            h.query_all_by_label(label).map(|n| n.rect()).find(|r| in_rail(*r)).unwrap_or_else(|| panic!("{label} in the rail"))
        };
        let page = h.get(By::new().role(egui::accesskit::Role::TextInput).label("Current page")).rect();
        assert!(in_rail(page) && page.bottom() <= height, "page field {page:?} in the rail");
        let dock_label = if on_left { "Dock navigation rail on Right" } else { "Dock navigation rail on Left" };
        let dock = rail(&h, dock_label);
        assert!(dock.top() >= page.bottom() && dock.bottom() <= height, "dock button {dock:?} stays below navigation and inside the window");
        assert!(rail(&h, "Comments").bottom() <= page.top(), "the first button sits above the page field");
        assert!(rail(&h, "Tags").top() >= page.top(), "the window is short enough that the buttons must scroll");
        let first = rail(&h, "Comments");
        let over = first.center();
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
        assert!(tags.top() >= first.top(), "Tags {tags:?} is fully visible in the scrolling area");
        assert!(tags.bottom() <= page.top(), "Tags {tags:?} scrolls into view above the page field {page:?}");
        assert_eq!(h.get(By::new().role(egui::accesskit::Role::TextInput).label("Current page")).rect(), page, "navigation stays put");
    }
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
    use craft::pages::PageDisplay;
    use pdfcraft_ui_egui::canvas::Fit;
    let path = std::env::temp_dir().join(format!("pdfcraft-last-view-{}.pdf", std::process::id())).to_string_lossy().into_owned();
    let p = path.clone();
    let mut h = harness(move |app| app.open_bytes(&egui::Context::default(), "doc.pdf", Some(p), pages_fixture(3)).expect("opens"));
    let p = path.clone();
    assert!(h.state().restore_last_view, "on by default");
    {
        let app = h.state_mut();
        app.set_option(&egui::Context::default(), "zoom", "50").unwrap();
        let view = &mut app.views[0];
        view.view.display = PageDisplay::TwoUp;
        view.view.continuous = false;
        view.view.cover = true;
        let now = view.current();
        view.view.go_to(now);
        view.go_to_page(2);
    }
    h.run_steps(3);
    // Saved with the tab still open: quitting remembers it.
    let saved_open = h.state().settings_snapshot();
    h.state_mut().close_tab(0);
    let saved_closed = h.state().settings_snapshot();
    for (label, saved) in [("open", saved_open), ("closed", saved_closed)] {
        let p = p.clone();
        let h = harness(move |app| {
            app.apply_settings(&saved);
            app.open_bytes(&egui::Context::default(), "doc.pdf", Some(p), pages_fixture(3)).expect("opens");
        });
        let v = &h.state().views[0];
        assert_eq!(
            (v.current(), v.fit(), v.zoom(), v.view.display, v.view.continuous, v.view.cover),
            (2, Fit::None, 0.5, PageDisplay::TwoUp, false, true),
            "{label}"
        );
    }
    // The preference off: the defaults.
    let saved = h.state().settings_snapshot();
    let p = path.clone();
    let h = harness(move |app| {
        app.apply_settings(&saved);
        app.restore_last_view = false;
        app.open_bytes(&egui::Context::default(), "doc.pdf", Some(p), pages_fixture(3)).expect("opens");
    });
    let v = &h.state().views[0];
    assert_eq!((v.current(), v.fit(), v.view.display, v.view.continuous, v.view.cover), (0, Fit::Width, PageDisplay::Single, true, false));
    // Garbage in the settings is ignored, a usable entry among it is kept.
    let mut app = crate::control::new_app();
    app.apply_settings(&pdfcraft_ui_egui::settings::import_json(&format!(
        r#"{{"restore_last_view": "yes", "last_views": [5, {{"path": ""}}, {{"path": "{}", "page": -1, "zoom": "huge", "display": "single"}}, {{"path": "{}", "page": 1, "zoom": "fit-page", "display": "two-up"}}]}}"#,
        path.replace('\\', "/"),
        path.replace('\\', "/")
    )).0);
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
    let mut h = harness(move |app| app.open_bytes(&egui::Context::default(), "doc.pdf", Some(p), pages_fixture(3)).expect("opens"));
    h.state_mut().views[0].view.zoom = 0.625;
    h.state_mut().views[0].view.fit = Fit::None;
    h.run_steps(3);
    h.state_mut().close_tab(0);
    assert_eq!(h.state().last_views[0].zoom, "62.5%");
    let saved = h.state().settings_snapshot();
    let h = harness(move |app| {
        app.apply_settings(&saved);
        app.open_bytes(&egui::Context::default(), "doc.pdf", Some(path), pages_fixture(3)).expect("opens");
    });
    let v = &h.state().views[0];
    assert_eq!((v.fit(), v.zoom()), (Fit::None, 0.625));
}

/// #1767 with full screen: the view remembered while presenting is the one full screen will go
/// back to, and a file reopened in full screen presents one fitted page, then leaves full
/// screen for the remembered view.
#[test]
fn full_screen_neither_is_remembered_nor_hides_the_last_view() {
    use craft::pages::PageDisplay;
    use pdfcraft_ui_egui::canvas::Fit;
    let path = std::env::temp_dir().join(format!("pdfcraft-last-view-fs-{}.pdf", std::process::id())).to_string_lossy().into_owned();
    let p = path.clone();
    let mut h = harness(move |app| app.open_bytes(&egui::Context::default(), "doc.pdf", Some(p), pages_fixture(3)).expect("opens"));
    {
        let app = h.state_mut();
        app.set_option(&egui::Context::default(), "zoom", "50").unwrap();
        let view = &mut app.views[0];
        view.view.display = PageDisplay::TwoUp;
        view.view.continuous = false;
        let now = view.current();
        view.view.go_to(now);
        view.go_to_page(2);
        view.enter_presentation();
        assert!(view.presenting() && view.fit() == Fit::Page);
    }
    h.run_steps(3);
    let saved = h.state().settings_snapshot();
    h.state_mut().close_tab(0);
    let remembered = h.state().last_views[0].clone();
    assert_eq!((remembered.page, remembered.zoom.as_str()), (2, "50%"), "not full screen's fitted page");
    let p = path.clone();
    let mut h = harness(move |app| {
        app.apply_settings(&saved);
        app.set_full_screen(true);
        app.open_bytes(&egui::Context::default(), "doc.pdf", Some(p), pages_fixture(3)).expect("opens");
    });
    let v = &h.state().views[0];
    assert!(v.presenting(), "opened in full screen");
    assert_eq!((v.fit(), v.view.continuous, v.current()), (Fit::Page, false, 2), "one fitted page, where the reader left off");
    let view = &mut h.state_mut().views[0];
    view.leave_presentation();
    assert_eq!(
        (view.fit(), view.zoom(), view.view.display, view.view.continuous),
        (Fit::None, 0.5, PageDisplay::TwoUp, false),
        "the remembered view"
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn home_recent_context_menu_reveals_the_target_without_opening_or_removing_it() {
    let missing = std::env::temp_dir().join(format!("pdfcraft-recent-missing-{}", std::process::id())).join("jährlicher report.pdf");
    assert!(!missing.exists());
    let expected = missing.to_string_lossy().into_owned();
    let target = expected.clone();
    // The existence check runs as a job the test runs itself, so no worker races the frames.
    let jobs = std::sync::Arc::new(craft::host::ManualJobs::new());
    let queue = jobs.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1600.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let (intake, _sender) = craft::platform::open::FileIntake::new(craft::host::Wake::none());
            let mut app = host_app(jobs, intake);
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            for (name, path) in [("first.pdf", ".".to_owned()), ("target.pdf", target)] {
                app.recent.push(pdfcraft_ui_egui::RecentFile { name: name.into(), path, pages: 1, size: 0 });
            }
            app
        }
    });
    h.run_steps(4);
    queue.run_all();
    h.run_steps(2);
    let label = "Show in Folder";
    h.get_by(|n| n.role() == egui::accesskit::Role::ListItem && n.label().is_some_and(|s| s.starts_with("target.pdf,"))).click_secondary();
    h.run_steps(3);
    assert!(!h.get_by_label(label).accesskit_node().is_disabled());
    assert!(h.state().views.is_empty(), "right-click must not open the PDF");
    assert_eq!(h.state().recent.len(), 2);
    h.get_by_label(label).click();
    h.run_steps(2);
    queue.run_all();
    h.run_steps(3);
    assert!(h.query_by_label(label).is_none(), "choosing the action closes its menu");
    assert!(h.state().views.is_empty(), "reveal must not open the PDF");
    assert_eq!(h.state().active, None);
    assert_eq!(h.state().recent.len(), 2, "reveal must not remove missing recent files");
    assert_eq!(h.state().recent[1].path, expected);
    assert!(!posted_notices(&h).is_empty(), "the selected path is checked and the reveal error is shown");
}
