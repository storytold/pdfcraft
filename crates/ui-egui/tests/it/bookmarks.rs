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

/// #5529: the title is selected when the rename editor opens, so typing replaces it at once,
/// with no Ctrl+A. New bookmarks and double-click rename both get this.
#[test]
fn typing_replaces_the_title_when_the_editor_opens() {
    let mut h = harness();
    h.key_press_modifiers(Modifiers::COMMAND, Key::B);
    h.run_steps(3);
    field(&h, "Untitled").type_text("Intro"); // the whole default is replaced
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(outline(&h), [("Intro".into(), Some(0), 0)]);

    menu(&mut h, "Intro", "Rename");
    field(&h, "Intro").type_text("Introduction");
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(outline(&h)[0].0, "Introduction");
}

/// The selection is one-shot: after the user collapses it to a caret, later frames must not
/// reselect, so typing at that caret inserts.
#[test]
fn a_rename_editor_keeps_the_users_caret_across_frames() {
    let mut h = harness();
    h.key_press_modifiers(Modifiers::COMMAND, Key::B);
    h.run_steps(3);
    h.key_press(Key::ArrowLeft); // collapses the opening selection to the caret at 0
    h.run_steps(2);
    field(&h, "Untitled").type_text("I"); // inserts at the caret, does not replace
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(outline(&h)[0].0, "IUntitled");
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

/// Two pages whose chapter and section headings are set apart only by style (#1693).
fn styled_pdf() -> Vec<u8> {
    let page1 = "BT /FB 18 Tf 72 700 Td (Introduction) Tj ET BT /FR 11 Tf 72 670 Td (Some text) Tj ET BT /FB 14 Tf 72 640 Td (Background) Tj ET";
    let page2 = "BT /FB 18 Tf 72 700 Td (Results) Tj ET BT /FR 11 Tf 72 670 Td (More text) Tj ET";
    format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 612 792] /Resources << /Font << /FB 7 0 R /FR 8 0 R >> >> >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Contents 5 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /Contents 6 0 R >> endobj
5 0 obj << /Length {} >>
stream
{page1}
endstream
endobj
6 0 obj << /Length {} >>
stream
{page2}
endstream
endobj
7 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >> endobj
8 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >> endobj
trailer << /Root 1 0 R >>
%%EOF",
        page1.len(),
        page2.len()
    )
    .into_bytes()
}

/// #1693: Bookmark options ▸ New bookmarks from text styles… lists the document's styles with a
/// sample, offers the largest for level 1, and Create makes the bookmarks (sections under
/// their chapter) as one undo step. Cancel changes nothing; a document without text says so.
#[test]
fn new_bookmarks_from_text_styles_from_the_bookmark_options() {
    let mut h = harness();
    h.state_mut().open_bytes("styled.pdf", None, styled_pdf()).unwrap();
    h.run_steps(3);
    let id = h.state().active.and_then(|i| h.state().views.get(i)).unwrap().id;
    let open = |h: &mut Harness<'static, PdfCraftApp>| {
        h.get_by_label("Bookmark options").click();
        h.run_steps(2);
        h.get_by_label("New bookmarks from text styles…").click();
        h.run_steps(3);
    };
    open(&mut h);
    assert_eq!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::BookmarksFromTextStyles));
    assert_eq!(h.state().text_style_draft.levels, [Some(0), None, None, None], "the largest style is offered for level 1");
    // Cancel: nothing made.
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog, None);
    assert!(h.state().session.get(id).unwrap().info.outline.is_empty());
    // Level 2 from its list: the sections' style.
    open(&mut h);
    // Each level's list is named after it, for screen readers too.
    h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Level 2").click();
    h.run_steps(2);
    // It lists every style, by font, size, weight and a sample.
    h.get_by_label("Helvetica-Bold, 18 pt, bold · “Introduction” (2 lines)");
    h.get_by_label("Helvetica, 11 pt · “Some text” (2 lines)");
    h.run_steps(2);
    h.get_by_label("Helvetica-Bold, 14 pt, bold · “Background” (1 line)").click();
    h.run_steps(2);
    assert_eq!(h.state().text_style_draft.levels, [Some(0), Some(1), None, None]);
    h.get_by_label("Create").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, None);
    let outline = h.state().session.get(id).unwrap().info.outline.clone();
    assert_eq!(outline.len(), 1);
    assert_eq!(outline[0].title, "Untitled");
    let titles: Vec<(&str, Option<usize>)> = outline[0].children.iter().map(|o| (o.title.as_str(), o.page)).collect();
    assert_eq!(titles, [("Introduction", Some(0)), ("Results", Some(1))]);
    assert_eq!(outline[0].children[0].children[0].title, "Background");
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("New bookmarks from text styles"));
    assert_eq!(h.state().right, Some(pdfcraft_ui_egui::RightPanel::Bookmarks));
    // A document with no text: nothing to choose from.
    h.state_mut().open_bytes("pages.pdf", None, PAGES.to_vec()).unwrap();
    h.run_steps(3);
    open(&mut h);
    let message = h.get_by_label("This document has no text to make bookmarks from.").rect();
    // The dialog's Close, below the message (the panel has a Close of its own).
    h.get_all_by_label("Close").find(|n| n.rect().top() > message.bottom()).expect("the dialog's Close").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog, None);
    // The command (Pages menu, command palette) opens it too.
    assert!(h.state_mut().execute("bookmark.from_text_styles"));
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::BookmarksFromTextStyles));
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

/// The active document's outline, flattened like [`outline`].
fn active_outline(h: &Harness<'static, PdfCraftApp>) -> Vec<(String, Option<usize>, usize)> {
    let i = h.state().active.unwrap();
    let id = h.state().views[i].id;
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

const ONE_PAGE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

#[test]
fn cut_copy_and_paste_a_bookmark_subtree_within_and_between_documents() {
    let mut h = harness();
    add(&mut h, "Intro");
    h.state_mut().set_option("page", "3").unwrap();
    h.run_steps(3);
    add(&mut h, "Results");
    menu(&mut h, "Results", "Indent");
    let tree = [("Intro".to_string(), Some(0), 0), ("Results".to_string(), Some(2), 1)];
    assert_eq!(outline(&h), tree);

    // Copy and paste after itself: the whole subtree, as one undo step.
    menu(&mut h, "Intro", "Copy");
    menu(&mut h, "Intro", "Paste");
    assert_eq!(outline(&h), [tree.clone(), tree.clone()].concat());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(outline(&h), tree, "a paste is one undo step");
    // Pasted on a child, it goes in after that child, at the child's level.
    menu(&mut h, "Results", "Paste");
    let nested = [("Intro".to_string(), Some(0), 1), ("Results".to_string(), Some(2), 2)];
    assert_eq!(outline(&h), [tree.clone(), nested].concat());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(outline(&h), tree);

    // Cut takes the subtree away in one step and keeps it to paste.
    menu(&mut h, "Intro", "Cut");
    assert!(outline(&h).is_empty());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(outline(&h), tree, "a cut is one undo step");

    // Into a one-page document: Results' page 3 is not there, so it keeps its place without a
    // destination rather than pointing at the wrong page, and the toast says so.
    h.state_mut().open_bytes("one.pdf", None, ONE_PAGE.to_vec()).unwrap();
    h.run_steps(3);
    h.get_by_label("Bookmark options").click();
    h.run_steps(2);
    h.get_by_label("Paste bookmarks").click();
    h.run_steps(3);
    assert_eq!(active_outline(&h), [("Intro".to_string(), Some(0), 0), ("Results".to_string(), None, 1)]);
    let toast = h.state().toast.as_ref().map(|t| t.0.clone()).unwrap_or_default();
    assert!(toast.contains("1 pasted bookmark points past the last page"), "{toast}");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert!(active_outline(&h).is_empty());
    assert_eq!(outline(&h), tree, "the source is untouched");
}

/// A one-page document whose single bookmark "Top" heads a chain `depth` bookmarks deep.
fn deep_outline_pdf(depth: usize) -> Vec<u8> {
    let mut pdf = String::from(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /Outlines 4 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Outlines /First 10 0 R /Last 10 0 R /Count 1 >> endobj
",
    );
    for k in 0..depth {
        let id = 10 + k;
        let parent = if k == 0 { 4 } else { id - 1 };
        let title = if k == 0 { "Top".to_string() } else { format!("L{k}") };
        let kids = if k + 1 < depth { format!("/First {0} 0 R /Last {0} 0 R /Count 1", id + 1) } else { String::new() };
        pdf.push_str(&format!("{id} 0 obj << /Title ({title}) /Parent {parent} 0 R {kids} /Dest [3 0 R /Fit] >> endobj\n"));
    }
    pdf.push_str("trailer << /Root 1 0 R >>\n%%EOF");
    pdf.into_bytes()
}

/// Cutting a subtree nested deeper than a copy can hold would lose what the copy left out, so
/// the cut copies and says why it deleted nothing.
#[test]
fn cutting_a_subtree_too_deep_to_copy_keeps_it() {
    let mut h = harness();
    h.state_mut().open_bytes("deep.pdf", None, deep_outline_pdf(40)).unwrap();
    h.run_steps(3);
    let before = active_outline(&h);
    assert_eq!(before.first().map(|b| b.0.as_str()), Some("Top"));
    menu(&mut h, "Top", "Cut");
    assert_eq!(active_outline(&h), before, "nothing was cut");
    let toast = h.state().toast.as_ref().map(|t| t.0.clone()).unwrap_or_default();
    assert!(toast.contains("so nothing was cut"), "{toast}");
    assert!(h.state().bookmark_clipboard.is_some(), "the copy is still on the bookmark clipboard");
}

fn pointer_drag(h: &mut Harness<'static, PdfCraftApp>, from: egui::Pos2, to: egui::Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    for step in 1..=6 {
        h.hover_at(from + (to - from) * (step as f32 / 6.0));
        h.run_steps(1);
    }
    h.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(4);
}

#[test]
fn drag_top_center_bottom_outdent_and_undo() {
    let mut h = harness();
    for (i, title) in ["Ch1", "Ch2", "Ch3"].iter().enumerate() {
        h.state_mut().apply_edit(pdfcraft_engine::Edit::AddBookmark {
            parent: vec![],
            index: i,
            title: (*title).into(),
            page: i,
            left: None,
            top: None,
        });
    }
    h.run_steps(3);

    // Center 50% of Ch2 -> nests Ch3 inside Ch2.
    let from = h.get_by_label("Ch3").rect().center();
    let into_ch2 = h.get_by_label("Ch2").rect().center();
    pointer_drag(&mut h, from, into_ch2);
    assert_eq!(outline(&h), [("Ch1".into(), Some(0), 0), ("Ch2".into(), Some(1), 0), ("Ch3".into(), Some(2), 1)]);

    // Top 25% of Ch1 -> moves Ch3 before Ch1 at depth 1.
    let ch3_rect = h.get_by_label("Ch3").rect();
    let ch1_top = h.get_by_label("Ch1").rect();
    pointer_drag(&mut h, ch3_rect.center(), egui::pos2(ch1_top.left() + 4.0, ch1_top.top() + 2.0));
    assert_eq!(outline(&h), [("Ch3".into(), Some(2), 0), ("Ch1".into(), Some(0), 0), ("Ch2".into(), Some(1), 0)]);

    // Undo restores previous nested state in one step.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(outline(&h), [("Ch1".into(), Some(0), 0), ("Ch2".into(), Some(1), 0), ("Ch3".into(), Some(2), 1)]);

    // Bottom blank space at far left X outdents Ch3 back to top level after Ch2.
    let ch3_rect = h.get_by_label("Ch3").rect();
    let below_ch3_left = egui::pos2(ch3_rect.left() + 2.0, ch3_rect.bottom() + 24.0);
    pointer_drag(&mut h, ch3_rect.center(), below_ch3_left);
    assert_eq!(outline(&h), [("Ch1".into(), Some(0), 0), ("Ch2".into(), Some(1), 0), ("Ch3".into(), Some(2), 0)]);
}

#[test]
fn multi_select_caravan_drag_and_keyboard_shortcuts_and_read_only() {
    let mut h = harness();
    for (i, title) in ["Alpha", "Beta", "Gamma"].iter().enumerate() {
        h.state_mut().apply_edit(pdfcraft_engine::Edit::AddBookmark {
            parent: vec![],
            index: i,
            title: (*title).into(),
            page: i,
            left: None,
            top: None,
        });
    }
    h.run_steps(3);

    // Click Alpha, Ctrl-click Beta, drag caravan onto Gamma.
    h.get_by_label("Alpha").click();
    h.run_steps(2);
    let beta_pos = h.get_by_label("Beta").rect().center();
    h.event(egui::Event::ModifiersChanged(Modifiers::COMMAND));
    h.hover_at(beta_pos);
    h.event(egui::Event::PointerButton { pos: beta_pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::COMMAND });
    h.event(egui::Event::PointerButton { pos: beta_pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::COMMAND });
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);

    let gamma_pos = h.get_by_label("Gamma").rect().center();
    pointer_drag(&mut h, beta_pos, gamma_pos);
    assert_eq!(outline(&h), [("Gamma".into(), Some(2), 0), ("Alpha".into(), Some(0), 1), ("Beta".into(), Some(1), 1)]);

    // Ctrl+A, Ctrl+C, Ctrl+V while panel has focus duplicates visible bookmarks; Delete deletes selection.
    h.get_by_label("Gamma").click();
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::C);
    h.run_steps(1);
    h.get_by_label("Gamma").click();
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::V);
    h.run_steps(3);
    assert!(outline(&h).len() > 3, "pasting selected tree adds copies cleanly");

    let count_before_delete = outline(&h).len();
    h.get_all_by_label("Alpha").next().unwrap().click();
    h.run_steps(1);
    h.key_press(Key::Delete);
    h.run_steps(3);
    assert_eq!(outline(&h).len(), count_before_delete - 1);
    assert!(outline(&h).iter().any(|(t, _, _)| t == "Gamma"));

    // Read-only PDF (assemble_document = false) disables mutation while preserving collapse & nav.
    let id = h.state().views[0].id;
    let bytes = h.state().session.save_bytes(id).unwrap();
    let mut cos_doc = pdfcraft_cos::Document::open(bytes).unwrap();
    cos_doc
        .set_encryption(&pdfcraft_cos::NewEncryption {
            algorithm: pdfcraft_cos::Algorithm::Aes256,
            user_password: "",
            owner_password: "owner",
            permissions: 0,
            encrypt_metadata: true,
            seed: [9u8; 32],
        })
        .unwrap();
    let locked = pdfcraft_cos::write_full(&cos_doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
    h.state_mut().open_bytes("locked.pdf", None, locked.clone()).unwrap();
    h.run_steps(3);
    assert!(h.query_by_label("New bookmark (Ctrl+B)").is_none());
    // Collapse toggle on "Gamma" works cleanly without dirtying COS.
    let g_row = h.get_all_by_label("Gamma").next().unwrap().rect();
    let tri = egui::pos2(g_row.left() + 8.0, g_row.top() + 14.0);
    h.hover_at(tri);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: tri, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.event(egui::Event::PointerButton { pos: tri, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    let locked_id = h.state().views[1].id;
    assert!(h.state().session.get(locked_id).unwrap().can_undo().is_none());
}

#[test]
fn insert_in_page_order_set_destination_current_view_and_single_line_truncation() {
    let mut h = harness();
    let long_title = format!("Very Long Single Line Heading {}", "W".repeat(80));
    h.state_mut().apply_edit(pdfcraft_engine::Edit::AddBookmark {
        parent: vec![],
        index: 0,
        title: long_title.clone(),
        page: 0,
        left: None,
        top: None,
    });
    h.state_mut().apply_edit(pdfcraft_engine::Edit::AddBookmark {
        parent: vec![],
        index: 1,
        title: "Third Page".into(),
        page: 2,
        left: None,
        top: None,
    });
    h.run_steps(3);

    // Single-line truncation preserves fixed 28.0 px row height.
    let long_rect = h.get_by_label(&long_title).rect();
    assert!((long_rect.height() - 28.0).abs() <= 0.5, "expected single-line 28px row, got {}", long_rect.height());

    // Toggle "Insert in page order" and create a bookmark on page 2 (0-based 1) via Ctrl+B.
    h.get_by_label("Bookmark options").click();
    h.run_steps(2);
    h.get_by_label("Insert in page order").click();
    h.run_steps(2);
    h.state_mut().views[0].go_to_page(1);
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::COMMAND, Key::B);
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(outline(&h).iter().map(|b| b.1).collect::<Vec<_>>(), [Some(0), Some(1), Some(2)]);

    // Set destination to current view captures active zoom (150% -> zoom 1.5).
    h.state_mut().set_option("zoom", "150").unwrap();
    h.state_mut().views[0].go_to_page(2);
    h.run_steps(3);
    menu(&mut h, "Third Page", "Set destination to current view");
    let id = h.state().views[0].id;
    let third_view = h.state().session.get(id).unwrap().info.outline[2].view;
    assert!(matches!(third_view, pdfcraft_render::DestView::Xyz { zoom: Some(z), .. } if (z - 1.5).abs() < 0.05), "{third_view:?}");
}
