//! Editing bookmarks in the Bookmarks panel (M4.6), driven like a user would.

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

const PAGES: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
5 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("pages.pdf", None, PAGES.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "bookmarks").unwrap();
        app
    });
    h.run_steps(6);
    h
}

fn outline(h: &Harness<'static, PdfCraftApp>) -> Vec<(String, Option<usize>, usize)> {
    let id = h.state().views[0].id;
    fn flat(items: &[pdfcraft_render::OutlineItem], depth: usize, out: &mut Vec<(String, Option<usize>, usize)>) {
        for i in items {
            out.push((i.title.clone(), i.page, depth));
            flat(&i.children, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    flat(&h.state().session.get(id).unwrap().info.outline, 0, &mut out);
    out
}

/// The inline rename editor (the text field, not its inner text run).
fn field<'a>(h: &'a Harness<'static, PdfCraftApp>, value: &'a str) -> egui_kittest::Node<'a> {
    h.query_all_by_value(value).next().unwrap_or_else(|| panic!("no editor showing {value:?}"))
}

/// Add a bookmark with ⌘B at the current page and name it by typing.
fn add(h: &mut Harness<'static, PdfCraftApp>, title: &str) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::B);
    h.run_steps(3);
    field(h, "Untitled"); // the inline editor is open and focused
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    field(h, "Untitled").type_text(title);
    h.run_steps(1);
    h.key_press(Key::Enter);
    h.run_steps(3);
}

fn menu(h: &mut Harness<'static, PdfCraftApp>, bookmark: &str, item: &str) {
    h.get_by_label(bookmark).click_secondary();
    h.run_steps(2);
    h.get_by_label(item).click();
    h.run_steps(3);
}

#[test]
fn new_rename_reorder_indent_delete_and_undo() {
    let mut h = harness();
    h.get_by_label("This document has no bookmarks.");
    add(&mut h, "Intro");
    h.state_mut().set_option("page", "3").unwrap();
    h.run_steps(3);
    add(&mut h, "Results");
    assert_eq!(outline(&h), [("Intro".into(), Some(0), 0), ("Results".into(), Some(2), 0)]);

    menu(&mut h, "Results", "Indent");
    assert_eq!(outline(&h), [("Intro".into(), Some(0), 0), ("Results".into(), Some(2), 1)]);
    h.get_by_label("Results"); // its new parent opened, so it still shows
    menu(&mut h, "Results", "Outdent");
    menu(&mut h, "Results", "Move up");
    assert_eq!(outline(&h)[0].0, "Results");

    menu(&mut h, "Intro", "Rename");
    field(&h, "Intro");
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    field(&h, "Intro").type_text("Introduction");
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(outline(&h)[1].0, "Introduction");

    menu(&mut h, "Results", "Delete");
    assert_eq!(outline(&h), [("Introduction".into(), Some(0), 0)]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(outline(&h).len(), 2, "delete is undoable");
    assert!(h.state().session.get(h.state().views[0].id).unwrap().dirty);
}

/// The side panel's width as egui keeps it from one frame to the next.
fn panel_width(h: &Harness<'static, PdfCraftApp>) -> f32 {
    h.ctx.data(|data| data.get_temp::<egui::Rect>(egui::Id::unique("pdfcraft-inspector-body"))).expect("the inspector body is shown").width()
}

#[test]
fn the_panel_keeps_its_width_while_the_pointer_moves() {
    // The search row was 2 px wider than the panel, and a resizable panel keeps the width its
    // content used, so every repaint (any pointer move) widened it until it reached its maximum.
    let mut h = harness();
    add(&mut h, "Intro");
    h.get_by_label("Search"); // the search row shows once there are bookmarks
    let before = panel_width(&h);
    for i in 0..30 {
        h.hover_at(egui::pos2(300.0 + 10.0 * i as f32, 400.0));
        h.run_steps(1);
    }
    assert_eq!(panel_width(&h), before, "the Bookmarks panel drifted wider");
}

#[test]
fn escape_cancels_a_rename() {
    let mut h = harness();
    add(&mut h, "Keep");
    menu(&mut h, "Keep", "Rename");
    field(&h, "Keep").type_text("zzz");
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert_eq!(outline(&h)[0].0, "Keep");
}

#[test]
fn number_pages_dialog_labels_the_selected_pages() {
    let mut h = harness();
    h.state_mut().views[0].select_pages(&[0, 1]);
    assert!(h.state_mut().execute("page.number"));
    h.run_steps(3);
    h.get_by_label("Number pages");
    h.get_by_value("1, 2, 3").click(); // the style menu
    h.run_steps(2);
    h.get_by_label("i, ii, iii").click();
    h.run_steps(2);
    h.get_by_label_contains("Labels: i, ii");
    h.get_by_label("OK").click();
    h.run_steps(3);
    let id = h.state().views[0].id;
    let labels: Vec<String> = h.state().session.get(id).unwrap().info.pages.iter().map(|p| p.label.clone()).collect();
    assert_eq!(labels, ["i", "ii", "3"]);
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Number pages"));
}

#[test]
fn search_finds_collapsed_bookmarks_and_keeps_original_paths() {
    use pdfcraft_engine::Edit;
    let mut h = harness();
    for (parent, index, title, page) in
        [(vec![], 0, "Unrelated", 0), (vec![], 1, "Reports", 0), (vec![1], 0, "Other chapter", 1), (vec![1], 1, "Résumé results", 2)]
    {
        h.state_mut().apply_edit(Edit::AddBookmark { parent, index, title: title.into(), page, left: None, top: None });
    }
    h.run_steps(3);
    h.get_by_label("Bookmark options").click();
    h.run_steps(2);
    h.get_by_label("Collapse all bookmarks").click();
    h.run_steps(3);
    assert!(h.query_by_label("Résumé results").is_none());
    let id = h.state().views[0].id;
    // A full save stamps /ModDate from the clock: pin it, or two saves a second apart differ.
    let session = std::mem::take(&mut h.state_mut().session);
    h.state_mut().session = session.with_clock(|| 1_700_000_000);
    let before = h.state().session.save_full_bytes(id).unwrap();
    h.get_by_label("Search").click();
    h.run_steps(1);
    h.get_by_label("Search").type_text("RÉSUMÉ");
    h.run_steps(3);
    h.get_by_label("Reports");
    h.get_by_label("Résumé results").click();
    h.run_steps(3);
    assert_eq!(h.state().views[0].current, 2);
    assert!(h.query_by_label("Unrelated").is_none());
    assert!(h.query_by_label("Other chapter").is_none());
    assert_eq!(h.state().session.save_full_bytes(id).unwrap(), before, "search doesn't edit the PDF");
    if let Ok(dir) = std::env::var("PDFCRAFT_BOOKMARK_SHOTS") {
        let _gpu = crate::gpu();
        h.render().unwrap().save(std::path::Path::new(&dir).join("filtered-bookmarks.png")).unwrap();
    }
    menu(&mut h, "Résumé results", "Delete");
    assert_eq!(outline(&h).len(), 3);
    assert!(outline(&h).iter().any(|b| b.0 == "Other chapter"));
    h.get_by_label("No matches.");
    add(&mut h, "New section");
    assert!(outline(&h).iter().any(|b| b.0 == "New section"), "new bookmarks remain nameable while searching");
    h.get_by_label("No matches.");
    h.get_by_label("Clear").click();
    h.run_steps(3);
    h.get_by_label("Unrelated");
    assert!(h.query_by_label("Other chapter").is_none(), "search did not change saved expansion state");
}

#[test]
fn bookmark_search_is_per_document() {
    let mut h = harness();
    add(&mut h, "First document");
    h.get_by_label("Search").click();
    h.run_steps(1);
    h.get_by_label("Search").type_text("not found");
    h.run_steps(3);
    h.get_by_label("No matches.");
    h.state_mut().open_bytes("second.pdf", None, PAGES.to_vec()).unwrap();
    h.run_steps(3);
    add(&mut h, "Second document");
    h.get_by_label("Second document");
    h.state_mut().active = Some(0);
    h.run_steps(3);
    h.get_by_label("No matches.");
    h.get_by_label("Clear").click();
    h.run_steps(3);
    h.get_by_label("First document");
}

#[test]
fn new_bookmarks_from_structure_nest_the_tagged_headings() {
    const TAGGED: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /StructTreeRoot 5 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
5 0 obj << /Type /StructTreeRoot /K [6 0 R 7 0 R] >> endobj
6 0 obj << /S /H1 /Pg 3 0 R /ActualText (Overview) >> endobj
7 0 obj << /S /H2 /Pg 4 0 R /ActualText (Method) >> endobj
trailer << /Root 1 0 R >>
%%EOF";
    let mut h = harness();
    h.state_mut().open_bytes("tagged.pdf", None, TAGGED.to_vec()).unwrap();
    h.run_steps(3);
    h.get_by_label("Bookmark options").click();
    h.run_steps(2);
    h.get_by_label("New bookmarks from structure").click();
    h.run_steps(3);
    let id = h.state().active.and_then(|i| h.state().views.get(i)).unwrap().id;
    let titles: Vec<String> = h.state().session.get(id).unwrap().info.outline.iter().map(|o| o.title.clone()).collect();
    assert_eq!(titles, ["Untitled"]);
    h.get_by_label("Overview");
    h.get_by_label("Method");
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("New bookmarks from structure"));
    if let Ok(dir) = std::env::var("PDFCRAFT_BOOKMARK_SHOTS") {
        let _gpu = crate::gpu();
        h.render().unwrap().save(std::path::Path::new(&dir).join("bookmarks-from-structure.png")).unwrap();
    }
}

#[test]
fn expanding_and_collapsing_all_bookmarks() {
    use pdfcraft_engine::Edit;
    let mut h = harness();
    for (parent, title) in [(vec![], "Part"), (vec![0], "Chapter"), (vec![0, 0], "Section")] {
        h.state_mut().apply_edit(Edit::AddBookmark { parent, index: 0, title: title.into(), page: 0, left: None, top: None });
    }
    h.run_steps(3);
    let shown = |h: &Harness<'static, PdfCraftApp>, t: &str| h.query_by_label(t).is_some();
    h.get_by_label("Bookmark options").click();
    h.run_steps(2);
    h.get_by_label("Expand all bookmarks").click();
    h.run_steps(3);
    assert!(shown(&h, "Chapter") && shown(&h, "Section"));
    h.get_by_label("Bookmark options").click();
    h.run_steps(2);
    h.get_by_label("Collapse all bookmarks").click();
    h.run_steps(3);
    assert!(shown(&h, "Part") && !shown(&h, "Chapter"));
    h.get_by_label("Bookmark options").click();
    h.run_steps(2);
    h.get_by_label("Expand top-level bookmarks").click();
    h.run_steps(3);
    assert!(shown(&h, "Chapter") && !shown(&h, "Section"));
}

/// #3193: a bookmark's Style and Colour menus make its title bold, italic and coloured
/// (ISO 32000-2 §12.3.3, Tables 151 and 152), each an undoable edit.
#[test]
fn style_and_colour_from_the_bookmark_menu() {
    let mut h = harness();
    add(&mut h, "Intro");
    let look = |h: &Harness<'static, PdfCraftApp>| {
        let id = h.state().views[0].id;
        let b = &h.state().session.get(id).unwrap().info.outline[0];
        (b.bold, b.italic, b.color)
    };
    let submenu = |h: &mut Harness<'static, PdfCraftApp>, menu: &str, item: &str| {
        h.get_by_label("Intro").click_secondary();
        h.run_steps(2);
        // The submenu arrow is painted as an atom; the accessible name is the menu title.
        h.get_by_label(menu).click();
        h.run_steps(2);
        h.get_by_label(item).click();
        h.run_steps(3);
    };
    submenu(&mut h, "Style", "Bold");
    assert_eq!(look(&h), (true, false, None));
    submenu(&mut h, "Style", "Italic");
    assert_eq!(look(&h), (true, true, None));
    submenu(&mut h, "Colour", "Red");
    let (bold, italic, color) = look(&h);
    assert!(bold && italic, "the colour keeps the style");
    let [r, g, b] = color.expect("coloured");
    assert!((r - 0.89).abs() < 0.01 && (g - 0.13).abs() < 0.01 && (b - 0.13).abs() < 0.01, "{color:?}");
    submenu(&mut h, "Style", "Bold");
    assert!(!look(&h).0, "bold again turns it off");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert!(look(&h).0, "undoable");
}

/// Top-level bookmarks open as the file says, like every other level: a positive /Count shows
/// the children, a negative one hides them until the reader opens it (ISO 32000-2 §12.3.3,
/// Table 151). They used to open regardless.
#[test]
fn a_closed_top_level_bookmark_opens_closed() {
    const OUTLINED: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /Outlines 10 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
10 0 obj << /Type /Outlines /First 11 0 R /Last 13 0 R /Count 5 >> endobj
11 0 obj << /Title (Closed parent) /Parent 10 0 R /Next 12 0 R /First 21 0 R /Last 22 0 R /Count -2 /Dest [3 0 R /Fit] >> endobj
12 0 obj << /Title (Open parent) /Parent 10 0 R /Prev 11 0 R /Next 13 0 R /First 23 0 R /Last 24 0 R /Count 2 /Dest [3 0 R /Fit] >> endobj
13 0 obj << /Title (Uncounted parent) /Parent 10 0 R /Prev 12 0 R /First 25 0 R /Last 25 0 R /Dest [3 0 R /Fit] >> endobj
21 0 obj << /Title (Closed child A) /Parent 11 0 R /Next 22 0 R /Dest [3 0 R /Fit] >> endobj
22 0 obj << /Title (Closed child B) /Parent 11 0 R /Prev 21 0 R /Dest [3 0 R /Fit] >> endobj
23 0 obj << /Title (Open child A) /Parent 12 0 R /Next 24 0 R /Dest [3 0 R /Fit] >> endobj
24 0 obj << /Title (Open child B) /Parent 12 0 R /Prev 23 0 R /Dest [3 0 R /Fit] >> endobj
25 0 obj << /Title (Uncounted child) /Parent 13 0 R /Dest [3 0 R /Fit] >> endobj
trailer << /Root 1 0 R >>
%%EOF";
    let mut h = harness();
    h.state_mut().open_bytes("outlined.pdf", None, OUTLINED.to_vec()).unwrap();
    h.run_steps(4);
    let shown = |h: &Harness<'static, PdfCraftApp>, t: &str| h.query_by_label(t).is_some();
    assert!(shown(&h, "Closed parent") && shown(&h, "Open parent") && shown(&h, "Uncounted parent"));
    assert!(!shown(&h, "Closed child A") && !shown(&h, "Closed child B"), "a negative /Count opens closed");
    assert!(shown(&h, "Open child A") && shown(&h, "Open child B"), "a positive /Count opens open");
    assert!(!shown(&h, "Uncounted child"), "only a positive /Count opens a bookmark");

    // The disclosure triangle at the row's left still opens and closes it.
    let toggle = |h: &mut Harness<'static, PdfCraftApp>, title: &str| {
        let row = h.get_by_label(title).rect();
        let at = egui::pos2(row.left() + 8.0, row.top() + 14.0);
        h.hover_at(at);
        h.run_steps(1);
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.run_steps(3);
    };
    toggle(&mut h, "Closed parent");
    assert!(shown(&h, "Closed child A") && shown(&h, "Closed child B"), "opened by the reader");
    toggle(&mut h, "Closed parent");
    assert!(!shown(&h, "Closed child A"), "closed again");
    toggle(&mut h, "Open parent");
    assert!(!shown(&h, "Open child A"), "an open bookmark closes");

    // A search still shows the matches inside a closed bookmark, and clearing it restores it.
    h.get_by_label("Search").click();
    h.run_steps(1);
    h.get_by_label("Search").type_text("child B");
    h.run_steps(3);
    assert!(shown(&h, "Closed child B") && shown(&h, "Open child B"));
    h.get_by_label("Clear").click();
    h.run_steps(3);
    assert!(!shown(&h, "Closed child B") && !shown(&h, "Open child B"), "search leaves the open state alone");
}
