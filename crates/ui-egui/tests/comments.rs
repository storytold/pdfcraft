//! Commenting in the real shell (egui_kittest): tools, gestures, selection, the composer and
//! the Comments panel.

use egui::{Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_render::Annotation;
use pdfcraft_ui_egui::{PdfCraftApp, QuickTool};

mod support;

/// Two 300×200 pages of Helvetica text.
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

fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("author", "Tester").unwrap();
        setup(&mut app);
        app
    });
    h.run_steps(4);
    settle(&mut h);
    h
}

fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    for _ in 0..200 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// A point on page 1 in PDF user space → screen (no rotation; MediaBox 300×200).
fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).expect("page 1 on screen");
    pos2(r.left() + x / 300.0 * r.width(), r.top() + (200.0 - y) / 200.0 * r.height())
}

fn drag(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(from + (to - from) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(3);
}

fn click(h: &mut Harness<'static, PdfCraftApp>, p: Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
}

/// Drag between two page points (user space).
fn drag_pt(h: &mut Harness<'static, PdfCraftApp>, a: (f32, f32), b: (f32, f32)) {
    let (a, b) = (at(h, a.0, a.1), at(h, b.0, b.1));
    drag(h, a, b);
}

/// Click a page point (user space).
fn click_pt(h: &mut Harness<'static, PdfCraftApp>, p: (f32, f32)) {
    let p = at(h, p.0, p.1);
    click(h, p);
}

/// A text field by its placeholder ("Add a comment", "Add a reply").
fn field<'a>(h: &'a Harness<'static, PdfCraftApp>, placeholder: &'a str) -> egui_kittest::Node<'a> {
    h.query_all_by(|n| n.placeholder() == Some(placeholder)).next().unwrap_or_else(|| panic!("no field {placeholder:?}"))
}

fn comments(h: &Harness<'static, PdfCraftApp>) -> Vec<Annotation> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().info.annotations.clone()
}

#[test]
fn dragging_over_text_with_the_highlighter_highlights_it() {
    let mut h = harness(|app| app.set_option("quick", "highlight").unwrap());
    let (a, b) = {
        let v = &h.state().views[0];
        (v.glyph_screen_pos(0, 4).expect("text layer"), v.glyph_screen_pos(0, 14).expect("glyph"))
    };
    drag(&mut h, a, b);
    let c = comments(&h);
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!((c[0].subtype.as_str(), c[0].author.as_deref(), c[0].quads.len()), ("Highlight", Some("Tester"), 1));
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)), "the new comment is selected");
    assert_eq!(h.state().quick_tool, QuickTool::Comment(pdfcraft_ui_egui::comments::CommentTool::Highlight), "the highlighter stays on");
    // The Comments panel opened with the tool and lists it.
    h.get_by_label_contains("Comments");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Add highlight"));
}

#[test]
fn the_quick_bar_colour_changes_the_selected_comment() {
    use pdfcraft_ui_egui::comments::CommentTool;

    let mut h = harness(|app| app.set_option("quick", "highlight").unwrap());
    let (a, b) = {
        let v = &h.state().views[0];
        (v.glyph_screen_pos(0, 4).expect("text layer"), v.glyph_screen_pos(0, 14).expect("glyph"))
    };
    drag(&mut h, a, b);
    assert_eq!(comments(&h)[0].color, Some([1.0, 0.94, 0.0]));

    h.get_by_label("Colour").click();
    h.run_steps(2);
    h.get_by_label("Blue").click();
    h.run_steps(3);

    let state = h.state();
    let doc = state.session.get(state.views[0].id).expect("document");
    assert_eq!(doc.comment_props(0, 0).and_then(|props| props.color), Some([0.0, 0.47, 0.84]));
    assert_eq!(doc.can_undo(), Some("Change comment properties"), "recoloring is one undo step");
    assert_eq!(state.comment_prefs.style(CommentTool::Highlight).color, [0.0, 0.47, 0.84]);
}

#[test]
fn the_quick_bar_colour_changes_the_selected_arrow() {
    use pdfcraft_ui_egui::comments::CommentTool;

    // Arrows are saved as Line annotations; the Arrow tool's colour must still reach them.
    let mut h = harness(|app| app.set_option("quick", "arrow").unwrap());
    drag_pt(&mut h, (20.0, 20.0), (120.0, 30.0));
    assert_eq!(comments(&h)[0].subtype, "Line");
    assert_eq!(h.state().quick_tool, QuickTool::Comment(CommentTool::Arrow), "the arrow tool is still active");
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)), "the new arrow is selected");

    h.get_by_label("Colour").click();
    h.run_steps(2);
    h.get_by_label("Blue").click();
    h.run_steps(3);

    let state = h.state();
    let doc = state.session.get(state.views[0].id).expect("document");
    assert_eq!(doc.comment_props(0, 0).and_then(|props| props.color), Some([0.0, 0.47, 0.84]));
    assert_eq!(doc.can_undo(), Some("Change comment properties"));
}

#[test]
fn the_quick_bar_colour_skips_the_selected_comment_when_commenting_is_forbidden() {
    use pdfcraft_ui_egui::comments::CommentTool;

    // One page with a yellow highlight, encrypted to allow printing only (no commenting).
    let plain = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [4 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Highlight /Rect [20 140 120 165] /QuadPoints [20 165 120 165 20 140 120 140] /C [1 0.94 0] >> endobj
trailer << /Root 1 0 R >>
%%EOF";
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(plain.to_vec())).expect("fixture parses");
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: 0b0100,
        encrypt_metadata: true,
        seed: [5; 32],
    })
    .expect("encrypts");
    let bytes = pdfcraft_cos::write_full(&doc, &Default::default()).expect("writes");
    let mut h = harness(move |app| {
        app.open_bytes("restricted.pdf", None, bytes).expect("opens restricted");
        app.set_option("quick", "highlight").unwrap();
    });
    let id = h.state().active_ids().expect("active document").1;
    assert!(!h.state().session.get(id).expect("document").allows_annotation(), "commenting is forbidden");
    let index = h.state().views.iter().position(|v| v.id == id).expect("view");
    h.state_mut().views[index].comments.selected = Some((0, 0));
    h.run_steps(2);

    h.get_by_label("Colour").click();
    h.run_steps(2);
    h.get_by_label("Blue").click();
    h.run_steps(3);

    let state = h.state();
    let doc = state.session.get(id).expect("document");
    assert_eq!(doc.comment_props(0, 0).and_then(|props| props.color), Some([1.0, 0.94, 0.0]), "the comment keeps its colour");
    assert_eq!(doc.can_undo(), None, "no edit was made");
    assert_eq!(state.toast, None, "picking a colour is not reported as a failed edit");
    assert_eq!(state.comment_prefs.style(CommentTool::Highlight).color, [0.0, 0.47, 0.84], "the tool default still changes");
}

#[test]
fn a_new_highlight_opens_its_note_for_typing() {
    let mut h = harness(|app| app.set_option("quick", "highlight").unwrap());
    assert_eq!(h.state().views[0].comments.editing, None);
    let (a, b) = {
        let v = &h.state().views[0];
        (v.glyph_screen_pos(0, 4).expect("text layer"), v.glyph_screen_pos(0, 14).expect("glyph"))
    };
    drag(&mut h, a, b);
    assert_eq!(comments(&h).len(), 1);
    assert_eq!(h.state().views[0].comments.editing, Some((0, 0, String::new())), "the Comments panel edits the new highlight");
    // An area highlight opens its note too, replacing the empty one.
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    assert_eq!(comments(&h).len(), 2);
    assert_eq!(h.state().views[0].comments.editing, Some((0, 1, String::new())));
    // A note being typed is kept: the next highlight is only selected.
    h.state_mut().views[0].comments.editing = Some((0, 1, "typed".into()));
    drag_pt(&mut h, (160.0, 100.0), (260.0, 40.0));
    assert_eq!(comments(&h).len(), 3);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 2)));
    assert_eq!(h.state().views[0].comments.editing, Some((0, 1, "typed".into())));
}

#[test]
fn ctrl_enter_posts_a_highlight_comment_and_shift_enter_adds_a_line() {
    let mut h = harness(|app| {
        app.set_option("quick", "highlight").unwrap();
        app.set_option("panel", "comments").unwrap();
    });
    let (a, b) = {
        let v = &h.state().views[0];
        (v.glyph_screen_pos(0, 4).expect("text layer"), v.glyph_screen_pos(0, 14).expect("glyph"))
    };
    drag(&mut h, a, b);
    h.query_all_by(|n| n.role() == egui::accesskit::Role::MultilineTextInput && n.placeholder() != Some("Add a reply"))
        .next()
        .expect("highlight editor")
        .click();
    h.run_steps(2);
    h.event(egui::Event::Text("First line".into()));
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().views[0].comments.editing.as_ref().unwrap().2, "First line\n");
    assert_eq!(comments(&h)[0].contents.as_deref().unwrap_or(""), "", "a newline does not post");
    h.event(egui::Event::Text("Second line".into()));
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(h.state().views[0].comments.editing, None);
    assert_eq!(comments(&h)[0].contents.as_deref(), Some("First line\nSecond line"));
    h.state_mut().undo();
    h.run_steps(2);
    assert_eq!(comments(&h)[0].contents.as_deref().unwrap_or(""), "", "posting is one undo step");
}

#[test]
fn ctrl_enter_posts_a_reply_and_shift_enter_adds_a_line() {
    let mut h = harness(|app| app.set_option("panel", "comments").unwrap());
    field(&h, "Add a comment").click();
    h.run_steps(2);
    field(&h, "Add a comment").type_text("Parent");
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    field(&h, "Add a reply").click();
    h.run_steps(2);
    field(&h, "Add a reply").type_text("First line");
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(comments(&h).len(), 1, "Shift+Enter does not post a reply");
    assert_eq!(h.state().views[0].comments.reply, "First line\n");
    h.event(egui::Event::Text("Second line".into()));
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Enter);
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!(c.len(), 2);
    assert_eq!(c[1].contents.as_deref(), Some("First line\nSecond line"));
    assert_eq!(c[1].in_reply_to, c[0].name);
    assert_eq!(c[1].author.as_deref(), Some("Tester"));
    assert!(h.state().views[0].comments.reply.is_empty());
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(comments(&h).len(), 2, "an empty reply is not posted");
    h.state_mut().undo();
    h.run_steps(2);
    assert_eq!(comments(&h).len(), 1, "posting is one undo step");
    h.state_mut().set_option("comment", "1:1").unwrap();
    h.run_steps(2);
    field(&h, "Add a reply").click();
    h.run_steps(2);
    field(&h, "Add a reply").type_text("Enter still posts");
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(comments(&h)[1].contents.as_deref(), Some("Enter still posts"));
}

#[test]
fn the_comment_composer_shortcut_only_posts_the_focused_editor() {
    let mut h = harness(|app| app.set_option("quick", "note").unwrap());
    click_pt(&mut h, (250.0, 180.0));
    field(&h, "Add a comment").click();
    h.run_steps(2);
    h.event(egui::Event::Text("First line".into()));
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().views[0].comments.composer.as_ref().unwrap().text, "First line\n");
    assert!(comments(&h).is_empty());
    h.event(egui::Event::Text("Second line".into()));
    h.run_steps(2);
    h.query_all_by_value("1").next().expect("current page input").focus();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Enter);
    h.run_steps(2);
    assert!(h.state().views[0].comments.composer.is_some(), "a shortcut outside the editor keeps the draft");
    assert!(comments(&h).is_empty());
    h.query_all_by(|n| n.role() == egui::accesskit::Role::MultilineTextInput).next().expect("composer editor").focus();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Enter);
    h.run_steps(3);
    assert!(h.state().views[0].comments.composer.is_none());
    assert_eq!(comments(&h)[0].contents.as_deref(), Some("First line\nSecond line"));
}

#[test]
fn a_selection_made_first_is_marked_when_a_tool_is_picked() {
    let mut h = harness(|_| {});
    h.state_mut().views[0].select_text(0, 4, 8);
    assert!(h.state_mut().execute("comment.strikeout"));
    h.run_steps(2);
    let c = comments(&h);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].subtype, "StrikeOut");
    assert!(h.state().views[0].selected_text().is_none(), "the selection is consumed");
}

#[test]
fn selected_text_shows_actions_and_underlines_without_changing_tools() {
    let mut h = harness(|_| {});
    h.state_mut().views[0].select_text(0, 4, 8);
    h.run_steps(3);
    h.get_by_label("Add comment");
    h.get_by_label("Replace text");
    h.get_by_label("Underline").click();
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].subtype, "Underline");
    assert!(h.state().views[0].selected_text().is_none());
    assert_eq!(h.state().quick_tool, QuickTool::Select);
}

#[test]
fn selected_text_replace_action_opens_replacement_composer() {
    let mut h = harness(|_| {});
    h.state_mut().views[0].select_text(0, 4, 8);
    h.run_steps(3);
    h.get_by_label("Replace text").click();
    h.run_steps(2);
    h.get_by_label("Replacement text");
    assert!(h.state().views[0].selected_text().is_none());
}

#[test]
fn selected_text_comment_action_opens_note_composer() {
    let mut h = harness(|_| {});
    h.state_mut().views[0].select_text(0, 4, 8);
    h.run_steps(3);
    h.get_by_label("Add comment").click();
    h.run_steps(2);
    assert!(matches!(h.state().views[0].comments.composer.as_ref().map(|c| &c.kind), Some(pdfcraft_ui_egui::comments::ComposerKind::Note)));
    assert!(h.state().views[0].selected_text().is_none());
}

#[test]
fn shapes_and_ink_are_drawn_by_dragging() {
    let mut h = harness(|app| app.set_option("quick", "square").unwrap());
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    h.state_mut().set_option("quick", "ink").unwrap();
    drag_pt(&mut h, (160.0, 100.0), (260.0, 40.0));
    h.state_mut().set_option("quick", "arrow").unwrap();
    drag_pt(&mut h, (20.0, 20.0), (120.0, 30.0));
    // A click with a drawing tool draws nothing.
    h.state_mut().set_option("quick", "circle").unwrap();
    click_pt(&mut h, (200.0, 180.0));
    let c = comments(&h);
    let mut c = c;
    c.sort_by_key(|a| a.index);
    let kinds: Vec<&str> = c.iter().map(|a| a.subtype.as_str()).collect();
    assert_eq!(kinds, ["Square", "Ink", "Line"], "{c:?}");
    let r = c[0].rect;
    assert!((r[0] - 40.0).abs() < 3.0 && (r[1] - 40.0).abs() < 3.0 && (r[2] - 140.0).abs() < 3.0 && (r[3] - 100.0).abs() < 3.0, "{r:?}");
}

/// Press, drag and release a stroke, running only one frame after the release. Finished
/// renders are held back from the view (`hold_rasters`), so the raster the commit asks for can't
/// arrive before the test looks, however fast it renders; a test lets it through by clearing it.
fn stroke_without_settling(h: &mut Harness<'static, PdfCraftApp>, a: (f32, f32), b: (f32, f32)) {
    h.state_mut().views[0].hold_rasters = true;
    let (from, to) = (at(h, a.0, a.1), at(h, b.0, b.1));
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(from + (to - from) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(1);
}

/// #1858: on release the live stroke is replaced by the committed annotation, but the page
/// raster that draws it arrives later. The stroke stays painted from its points meanwhile.
#[test]
fn a_released_pen_stroke_stays_drawn_until_the_page_raster_has_it() {
    let mut h = harness(|app| app.set_option("quick", "ink").unwrap());
    stroke_without_settling(&mut h, (40.0, 100.0), (140.0, 60.0));
    assert_eq!(comments(&h).len(), 1, "the stroke is committed");
    assert_eq!(h.state().views[0].comments.settling_len(), 1, "still drawn from its points");
    h.run_steps(5);
    assert_eq!(h.state().views[0].comments.settling_len(), 1, "drawn from its points for as long as its raster takes");
    h.state_mut().views[0].hold_rasters = false;
    support::wait_until(&mut h, "the page raster with the stroke", |app| app.views[0].comments.settling_len() == 0);
    assert_eq!(h.state().views[0].comments.settling_len(), 0, "the raster draws it now");
}

#[test]
fn undoing_a_pen_stroke_before_its_raster_leaves_no_ghost() {
    let mut h = harness(|app| app.set_option("quick", "ink").unwrap());
    stroke_without_settling(&mut h, (40.0, 100.0), (140.0, 60.0));
    assert_eq!(h.state().views[0].comments.settling_len(), 1);
    h.state_mut().undo();
    assert_eq!(h.state().views[0].comments.settling_len(), 0);
}

#[test]
fn pen_strokes_stay_unselected_so_writing_continues() {
    let mut h = harness(|app| app.set_option("quick", "ink").unwrap());
    // No Comments panel: the author's name should only show in a hover popup.
    h.state_mut().set_option("panel", "none").unwrap();
    h.run_steps(2);
    drag_pt(&mut h, (40.0, 100.0), (140.0, 60.0));
    assert_eq!(h.state().views[0].comments.selected, None, "the first stroke isn't selected");
    // The pointer still rests on the stroke: no author popup covers it.
    h.hover_at(at(&h, 140.0, 60.0));
    h.run_steps(3);
    assert!(h.query_by_label_contains("Tester").is_none(), "no author popup while drawing");
    drag_pt(&mut h, (60.0, 100.0), (160.0, 60.0));
    let c = comments(&h);
    let kinds: Vec<&str> = c.iter().map(|a| a.subtype.as_str()).collect();
    assert_eq!(kinds, ["Ink", "Ink"], "{c:?}");
    assert_eq!(h.state().views[0].comments.selected, None, "nothing is selected after drawing");
    assert_eq!(h.state().quick_tool, QuickTool::Comment(pdfcraft_ui_egui::comments::CommentTool::Ink), "the pen stays on");
    // Other drawn shapes are still selected when made.
    h.state_mut().set_option("quick", "square").unwrap();
    drag_pt(&mut h, (180.0, 100.0), (260.0, 40.0));
    assert_eq!(h.state().views[0].comments.selected, Some((0, 2)));
}

#[test]
fn sticky_note_composer_posts_and_returns_to_select() {
    let mut h = harness(|app| app.set_option("quick", "note").unwrap());
    click_pt(&mut h, (250.0, 180.0));
    assert!(h.state().views[0].comments.composer.is_some(), "the composer opened");
    field(&h, "Add a comment").type_text("Please check");
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!(c.len(), 1);
    assert_eq!((c[0].subtype.as_str(), c[0].contents.as_deref()), ("Text", Some("Please check")));
    assert!(h.state().views[0].comments.composer.is_none());
    assert_eq!(h.state().quick_tool, QuickTool::Select, "one-shot tools return to Select");
    // Cancelling another one creates nothing.
    h.state_mut().set_option("quick", "freetext").unwrap();
    click_pt(&mut h, (50.0, 60.0));
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(comments(&h).len(), 1);
}

#[test]
fn comments_are_selected_moved_and_deleted_with_the_select_tool() {
    let mut h = harness(|app| app.set_option("quick", "square").unwrap());
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    h.state_mut().set_option("quick", "select").unwrap();
    // Click away, then on the rectangle's border area.
    click_pt(&mut h, (250.0, 20.0));
    assert_eq!(h.state().views[0].comments.selected, None);
    click_pt(&mut h, (90.0, 70.0));
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)));
    drag_pt(&mut h, (90.0, 70.0), (140.0, 70.0));
    let r = comments(&h)[0].rect;
    assert!((r[0] - 90.0).abs() < 3.0 && (r[2] - 190.0).abs() < 3.0, "moved right by 50 pt: {r:?}");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Move comment"));
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert!(comments(&h).is_empty());
    h.state_mut().undo();
    h.run_steps(2);
    assert_eq!(comments(&h).len(), 1);
}

#[test]
fn a_shape_just_drawn_resizes_from_its_handles_without_the_select_tool() {
    // #2338: the new shape showed its handles, but only the Select tool answered them.
    let square = QuickTool::Comment(pdfcraft_ui_egui::comments::CommentTool::Rectangle);
    let mut h = harness(|app| app.set_option("quick", "square").unwrap());
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)), "the new rectangle is selected");
    let before = comments(&h)[0].rect;
    // Drag its bottom-right handle 40 pt right and 20 pt down.
    drag_pt(&mut h, (before[2], before[1]), (before[2] + 40.0, before[1] - 20.0));
    let c = comments(&h);
    assert_eq!(c.len(), 1, "the handle resized the rectangle instead of drawing another: {c:?}");
    let r = c[0].rect;
    assert!((r[2] - (before[2] + 40.0)).abs() < 3.0 && (r[1] - (before[1] - 20.0)).abs() < 3.0, "resized: {before:?} → {r:?}");
    assert!((r[0] - before[0]).abs() < 1.0 && (r[3] - before[3]).abs() < 1.0, "the opposite corner stays: {before:?} → {r:?}");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Resize comment"));
    assert_eq!(h.state().quick_tool, square, "the rectangle tool stays on");
    // Away from the handles the tool still draws, over the shape too.
    drag_pt(&mut h, (60.0, 90.0), (120.0, 60.0));
    assert_eq!(comments(&h).len(), 2, "a drag inside the rectangle draws a second one");
}

#[test]
fn the_panel_posts_comments_and_replies() {
    let mut h = harness(|app| app.set_option("panel", "comments").unwrap());
    field(&h, "Add a comment").click();
    h.run_steps(2);
    field(&h, "Add a comment").type_text("General remark");
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!((c.len(), c[0].subtype.as_str(), c[0].contents.as_deref()), (1, "Text", Some("General remark")));
    // The new comment is selected: reply to it.
    field(&h, "Add a reply").click();
    h.run_steps(2);
    field(&h, "Add a reply").type_text("Agreed");
    h.run_steps(1);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!(c.len(), 2);
    let reply = c.iter().find(|a| a.in_reply_to.is_some()).expect("a reply");
    assert_eq!((reply.contents.as_deref(), reply.author.as_deref()), (Some("Agreed"), Some("Tester")));
    h.get_by_label_contains("Agreed");
}

/// The side panel's width as egui keeps it from one frame to the next.
fn panel_width(h: &Harness<'static, PdfCraftApp>) -> f32 {
    h.ctx.data(|data| data.get_temp::<egui::Rect>(egui::Id::new("pdfcraft-inspector-body"))).expect("the inspector body is shown").width()
}

#[test]
fn the_panel_keeps_its_width_with_a_reply_box_open() {
    // The reply row reserved 48 px for "Post"; Russian's wider button overflowed it, and a
    // resizable panel keeps the width its content used, so every repaint widened the panel.
    let mut h = harness(|app| app.set_option("panel", "comments").unwrap());
    field(&h, "Add a comment").click();
    h.run_steps(2);
    field(&h, "Add a comment").type_text("General remark");
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    field(&h, "Add a reply").click(); // the new comment is selected
    h.run_steps(2);
    field(&h, "Add a reply").type_text("Agreed");
    h.run_steps(1);
    h.state_mut().set_option("language", "ru").unwrap();
    h.run_steps(3);
    let before = panel_width(&h);
    h.get_by_label("Опубликовать").hover();
    for _ in 0..30 {
        h.run_steps(1);
    }
    assert_eq!(panel_width(&h), before, "the Comments panel drifted wider");
}

#[test]
fn line_ending_properties_change_and_undo() {
    let mut h = harness(|app| app.set_option("quick", "line").unwrap());
    drag_pt(&mut h, (40.0, 100.0), (160.0, 40.0));
    h.state_mut().open_comment_props(0, 0);
    h.run_steps(2);
    h.get_by_label("Line ending");
    h.query_all_by_value("None").next().expect("start ending").click();
    h.run_steps(2);
    for label in ["Open arrow", "Closed arrow", "Butt", "Reverse open arrow", "Reverse closed arrow", "Slash", "Square", "Circle", "Diamond"] {
        h.get_by_label(label);
    }
    assert_eq!(h.query_all_by_label("ROpenArrow").count(), 0);
    assert_eq!(h.query_all_by_label("OpenArrow").count(), 0);
    h.get_by_label("None").click();
    h.run_steps(1);
    {
        let d = h.state_mut().comment_props.as_mut().expect("open");
        assert_eq!(d.edited.endings.as_deref(), Some(&[pdfcraft_engine::LineEnding::None, pdfcraft_engine::LineEnding::None][..]));
        d.edited.endings = Some(vec![pdfcraft_engine::LineEnding::None, pdfcraft_engine::LineEnding::Diamond]);
    }
    h.get_by_label("OK").click();
    h.run_steps(3);
    let endings = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        assert_eq!(doc.can_undo(), Some("Change comment properties"));
        doc.comment_props(0, 0).unwrap().endings
    };
    assert_eq!(endings.as_deref(), Some(&[pdfcraft_engine::LineEnding::None, pdfcraft_engine::LineEnding::Diamond][..]));
    h.state_mut().execute("edit.undo");
    h.run_steps(2);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(
        doc.comment_props(0, 0).unwrap().endings.as_deref(),
        Some(&[pdfcraft_engine::LineEnding::None, pdfcraft_engine::LineEnding::None][..])
    );
}

#[test]
fn comment_properties_change_appearance_and_author() {
    let mut h = harness(|app| app.set_option("quick", "square").unwrap());
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    h.state_mut().open_comment_props(0, 0);
    h.run_steps(2);
    h.get_by_label("Rectangle Properties");
    {
        let d = h.state_mut().comment_props.as_mut().expect("open");
        assert_eq!(d.original.width, Some(2.0));
        d.edited.color = Some([0.0, 0.0, 1.0]);
        d.edited.width = Some(5.0);
        d.edited.author = "Grace".into();
    }
    h.get_by_label("General").click();
    h.run_steps(2);
    h.get_by_label("Author");
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    let p = doc.comment_props(0, 0).unwrap();
    assert_eq!((p.color, p.width, p.author.as_str()), (Some([0.0, 0.0, 1.0]), Some(5.0), "Grace"));
    assert_eq!(doc.can_undo(), Some("Change comment properties"), "one undo step");
}

/// #686: rectangles, ovals, polygons and clouds can be filled, from the quick bar for new ones
/// and from Properties for existing ones; "No fill" takes it away again.
#[test]
fn shapes_take_a_fill_colour_from_the_quick_bar_and_properties() {
    use pdfcraft_ui_egui::comments::CommentTool;
    let mut h = harness(|app| app.set_option("quick", "square").unwrap());
    h.run_steps(2);
    // No fill by default; the quick bar says so.
    h.get_by_label("Fill colour: none");
    h.state_mut().comment_prefs.set_fill(CommentTool::Rectangle, Some([1.0, 0.5, 0.0]));
    // Tools without an interior keep no fill.
    h.state_mut().comment_prefs.set_fill(CommentTool::Line, Some([1.0, 0.5, 0.0]));
    assert_eq!(h.state().comment_prefs.style(CommentTool::Line).fill, None);
    h.run_steps(2);
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    let fill_of = |h: &Harness<'static, PdfCraftApp>, index: usize| {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let p = doc.comment_props(0, index).unwrap();
        assert!(p.fillable);
        p.fill
    };
    assert_eq!(fill_of(&h, 0), Some([1.0, 0.5, 0.0]), "the new rectangle is filled");
    // Quick bar ▸ Fill colour ▸ No fill: the next rectangle has none.
    h.state_mut().set_option("quick", "square").unwrap();
    h.run_steps(2);
    h.get_by_label("Fill colour").click();
    h.run_steps(2);
    h.get_by_label("No fill").click();
    h.run_steps(2);
    assert_eq!(h.state().comment_prefs.style(CommentTool::Rectangle).fill, None);
    drag_pt(&mut h, (160.0, 100.0), (260.0, 40.0));
    assert_eq!(fill_of(&h, 1), None);

    // Properties ▸ Fill colour changes an existing one, as one undo step.
    h.state_mut().open_comment_props(0, 0);
    h.run_steps(2);
    h.get_by_label("Rectangle Properties");
    h.get_by_label("Fill colour");
    assert_eq!(h.state().comment_props.as_ref().unwrap().original.fill, Some([1.0, 0.5, 0.0]));
    h.get_by_label("No fill").click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(fill_of(&h, 0), None);
    let undo = h.state().session.get(h.state().views[0].id).unwrap().can_undo().map(str::to_owned);
    assert_eq!(undo.as_deref(), Some("Change comment properties"));
    h.state_mut().open_comment_props(0, 1);
    h.run_steps(2);
    h.state_mut().comment_props.as_mut().expect("open").edited.fill = Some([0.0, 0.0, 1.0]);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(fill_of(&h, 1), Some([0.0, 0.0, 1.0]));
    // Make Current Properties Default carries the fill into the tool.
    h.state_mut().make_comment_default(0, 1);
    assert_eq!(h.state().comment_prefs.style(CommentTool::Rectangle).fill, Some([0.0, 0.0, 1.0]));
}

#[test]
fn the_panel_filters_and_sorts() {
    let mut h = harness(|app| {
        app.set_option("panel", "comments").unwrap();
        app.set_option("quick", "square").unwrap();
    });
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    h.state_mut().set_option("quick", "circle").unwrap();
    drag_pt(&mut h, (160.0, 100.0), (260.0, 40.0));
    h.state_mut().set_option("quick", "select").unwrap();
    h.run_steps(2);
    let ovals = h.query_all_by_label("Oval").count();
    assert!(ovals >= 1);
    h.state_mut().views[0].comments.hidden_types.push("Oval".into());
    h.run_steps(2);
    assert_eq!(h.query_all_by_label("Oval").count(), ovals - 1, "the oval's card is filtered out");
    assert!(h.query_all_by_label("Rectangle").count() >= 1);
    h.state_mut().views[0].comments.hidden_types.clear();
    h.state_mut().views[0].comments.sort = pdfcraft_ui_egui::comments::SortBy::Type;
    h.run_steps(2);
    // Grouped by type: a group header names each type.
    assert_eq!(h.query_all_by_label("Oval").count(), ovals + 1, "a group header was added");
}

/// #1554: a page group's chevron folds that page's cards away and back, and the choice holds.
#[test]
fn page_groups_collapse_and_expand() {
    let mut h = harness(|app| app.set_option("panel", "comments").unwrap());
    for (page, text) in [(0, "First page note"), (1, "Second page note")] {
        let shape = pdfcraft_engine::Shape::Note { at: [100.0, 100.0], icon: pdfcraft_engine::NoteIcon::Comment };
        h.state_mut().apply_edit(pdfcraft_engine::Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
            page,
            style: pdfcraft_engine::Style::default_for(&shape),
            shape,
            contents: text.into(),
            author: "Tester".into(),
        }));
    }
    h.state_mut().views[0].comments.selected = None;
    h.run_steps(3);
    let shown = |h: &Harness<'static, PdfCraftApp>, text: &str| h.query_all_by_label_contains(text).count() > 0;
    assert!(shown(&h, "First page note") && shown(&h, "Second page note"));
    assert_eq!(h.query_all_by_label("Collapse").count(), 2, "one chevron per page group");

    h.query_all_by_label("Collapse").next().unwrap().click();
    h.run_steps(3);
    assert!(!shown(&h, "First page note"), "page 1's card is folded away");
    assert!(shown(&h, "Second page note"), "page 2 stays open");
    h.run_steps(5);
    assert!(!shown(&h, "First page note"), "the group stays folded on later frames");

    h.get_by_label("Expand").click();
    h.run_steps(3);
    assert!(shown(&h, "First page note"), "page 1's card is back");

    // Selecting a comment on a folded page opens its group so the card can be shown.
    h.query_all_by_label("Collapse").next().unwrap().click();
    h.run_steps(3);
    assert!(!shown(&h, "First page note"));
    h.state_mut().views[0].comments.selected = Some((0, 0));
    h.state_mut().views[0].comments.reveal = true;
    h.run_steps(3);
    assert!(shown(&h, "First page note"), "the selected comment's group opened");

    // "…" ▸ Collapse all pages, then Expand all pages.
    h.state_mut().views[0].comments.selected = None;
    h.get_by_label("More options").click();
    h.run_steps(2);
    h.get_by_label("Collapse all pages").click();
    h.run_steps(3);
    assert!(!shown(&h, "First page note") && !shown(&h, "Second page note"), "every page is folded");
    assert_eq!(h.query_all_by_label("Expand").count(), 2);
    h.get_by_label("More options").click();
    h.run_steps(2);
    h.get_by_label("Expand all pages").click();
    h.run_steps(3);
    assert!(shown(&h, "First page note") && shown(&h, "Second page note"), "every page is open again");
}

#[test]
fn comments_take_checkmarks_lock_hide_and_summarize() {
    let mut h = harness(|app| {
        app.set_option("panel", "comments").unwrap();
        app.set_option("quick", "square").unwrap();
    });
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    h.state_mut().set_option("quick", "select").unwrap();
    h.run_steps(2);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)), "the new comment is selected");
    // The card's checkmark.
    h.get_by_label("Mark with checkmark").click();
    h.run_steps(3);
    assert!(comments(&h).iter().any(|a| a.state.as_deref() == Some("Marked")));
    h.get_by_label("Remove checkmark");

    // "…" ▸ Copy text puts the comment's text on the clipboard.
    h.state_mut().apply_edit(pdfcraft_engine::Edit::SetAnnotationContents { page: 0, index: 0, text: "Copy me".into() });
    h.run_steps(2);
    h.get_by_label("More").click();
    h.run_steps(2);
    h.get_by_label("Copy text").click();
    let mut copied = false;
    for _ in 0..4 {
        h.step();
        copied |= h.output().platform_output.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(t) if t == "Copy me"));
    }
    assert!(copied, "the text is copied");
    h.run_steps(2);

    // Properties ▸ Locked.
    h.state_mut().open_comment_props(0, 0);
    h.run_steps(2);
    h.get_by_label("Locked").click();
    h.run_steps(1);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(comments(&h)[0].locked);
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert!(comments(&h).iter().any(|a| a.in_reply_to.is_none()), "a locked comment isn't deleted");

    // "…" ▸ Hide all comments, then Show all comments.
    h.get_by_label("More options").click();
    h.run_steps(2);
    h.get_by_label("Hide all comments").click();
    h.run_steps(2);
    let s = h.state();
    assert!(s.session.get(s.views[0].id).unwrap().comments_hidden());
    h.get_by_label("More options").click();
    h.run_steps(2);
    h.get_by_label("Show all comments").click();
    h.run_steps(2);
    let s = h.state();
    assert!(!s.session.get(s.views[0].id).unwrap().comments_hidden());

    // "…" ▸ Create PDF comment summary.
    h.get_by_label("More options").click();
    h.run_steps(2);
    h.get_by_label("Create PDF comment summary…").click();
    h.run_steps(2);
    h.get_by_label("Summarize Options");
    h.get_by_label("Create PDF Comment Summary").click();
    h.run_steps(3);
    let s = h.state();
    assert_eq!(s.views.len(), 2);
    assert!(s.session.get(s.views[1].id).unwrap().name.starts_with("Summary of comments on text"));
}

#[test]
fn polygons_clouds_connected_lines_callouts_and_inserted_text() {
    let mut h = harness(|app| app.set_option("quick", "polygon").unwrap());
    // Polygon: click the corners, Enter finishes.
    for p in [(40.0, 40.0), (120.0, 40.0), (80.0, 110.0)] {
        click_pt(&mut h, p);
    }
    assert_eq!(comments(&h).len(), 0, "still drawing");
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].subtype, "Polygon");

    // Cloud: clicking the first point again closes it.
    h.state_mut().set_option("quick", "cloud").unwrap();
    for p in [(160.0, 40.0), (260.0, 40.0), (260.0, 100.0), (160.0, 100.0), (160.0, 40.0)] {
        click_pt(&mut h, p);
    }
    assert_eq!(comments(&h).len(), 2);

    // Connected lines.
    h.state_mut().set_option("quick", "polyline").unwrap();
    for p in [(20.0, 150.0), (60.0, 130.0), (100.0, 150.0)] {
        click_pt(&mut h, p);
    }
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert!(comments(&h).iter().any(|a| a.subtype == "PolyLine"));

    // Callout: drag from the target to where the box goes, then type.
    h.state_mut().set_option("quick", "callout").unwrap();
    drag_pt(&mut h, (60.0, 160.0), (180.0, 190.0));
    assert!(h.state().views[0].comments.composer.is_some(), "the composer opened");
    field(&h, "Type text").type_text("Why?");
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let callout = comments(&h).into_iter().find(|a| a.subtype == "FreeText").expect("a callout");
    assert_eq!(callout.contents.as_deref(), Some("Why?"));
    assert!(callout.rect[0] <= 60.0 && callout.rect[1] <= 160.0, "holds the leader: {:?}", callout.rect);

    // Insert text at a click.
    h.state_mut().set_option("quick", "caret").unwrap();
    click_pt(&mut h, (95.0, 150.0));
    field(&h, "Text to insert").type_text("very ");
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let caret = comments(&h).into_iter().find(|a| a.subtype == "Caret").expect("a caret");
    assert_eq!(caret.contents.as_deref(), Some("very"));
    assert_eq!(h.state().quick_tool, QuickTool::Select);
}

fn drawn_points(h: &Harness<'static, PdfCraftApp>) -> usize {
    match &h.state().views[0].comments.gesture {
        Some(pdfcraft_ui_egui::comments::Gesture::Draw { points, .. }) => points.len(),
        _ => 0,
    }
}

#[test]
fn a_wrong_polygon_point_is_taken_back_with_backspace_or_undo() {
    let mut h = harness(|app| app.set_option("quick", "polygon").unwrap());
    for p in [(40.0, 40.0), (120.0, 40.0), (200.0, 150.0)] {
        click_pt(&mut h, p);
    }
    assert_eq!(drawn_points(&h), 3);
    // Backspace takes back the wrong click, then Undo takes back the one before it.
    h.key_press(egui::Key::Backspace);
    h.run_steps(2);
    assert_eq!(drawn_points(&h), 2);
    h.state_mut().undo();
    h.run_steps(2);
    assert_eq!(drawn_points(&h), 1);
    // The right corner goes where the wrong one was, and the polygon finishes with all three.
    click_pt(&mut h, (80.0, 110.0));
    click_pt(&mut h, (120.0, 40.0));
    assert_eq!(drawn_points(&h), 3);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].subtype, "Polygon");
    // Taking back the only point cancels the drawing without making a comment.
    h.state_mut().set_option("quick", "polygon").unwrap();
    click_pt(&mut h, (200.0, 200.0));
    h.key_press(egui::Key::Backspace);
    h.run_steps(2);
    assert_eq!(drawn_points(&h), 0);
    assert!(h.state().views[0].comments.gesture.is_none());
    assert_eq!(comments(&h).len(), 1);
}

#[test]
fn inserted_text_snaps_to_the_gap_between_characters_on_the_baseline() {
    let mut h = harness(|app| app.set_option("quick", "caret").unwrap());
    // "The quick brown fox", Helvetica 14 pt from x = 20 on the baseline y = 150. With the
    // standard-14 widths "The quick b" ends at 92.366 and "r" (333/1000 em) at 97.028, so a click
    // in the right half of "r", above the baseline, puts the tip between "r" and "o".
    click_pt(&mut h, (95.5, 155.0));
    field(&h, "Text to insert").type_text("x");
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let caret = comments(&h).into_iter().find(|a| a.subtype == "Caret").expect("a caret");
    let [x0, y0, x1, y1] = caret.rect;
    assert!(((x0 + x1) / 2.0 - 97.03).abs() < 0.3, "tip at the r|o gap: {:?}", caret.rect);
    assert!((y1 - 150.0).abs() < 0.3, "tip on the baseline: {:?}", caret.rect);
    assert!((y1 - y0 - 7.0).abs() < 0.3, "half the 14 pt em: {:?}", caret.rect);

    // Away from the text the caret stays where the click was, at its default size.
    h.state_mut().set_option("quick", "caret").unwrap();
    click_pt(&mut h, (200.0, 60.0));
    field(&h, "Text to insert").type_text("y");
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let away = comments(&h).into_iter().find(|a| a.subtype == "Caret" && a.contents.as_deref() == Some("y")).expect("a second caret");
    let [x0, y0, x1, y1] = away.rect;
    assert!(((x0 + x1) / 2.0 - 200.0).abs() < 1.0 && (y1 - 60.0).abs() < 1.0 && (y1 - y0 - 8.0).abs() < 0.3, "{:?}", away.rect);
}

#[test]
fn the_panel_filters_by_colour_and_checkmark() {
    let mut h = harness(|app| {
        app.set_option("panel", "comments").unwrap();
        app.set_option("quick", "square").unwrap();
    });
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    h.state_mut().set_option("quick", "circle").unwrap();
    drag_pt(&mut h, (160.0, 100.0), (260.0, 40.0));
    h.state_mut().set_option("quick", "select").unwrap();
    h.state_mut().apply_edit(pdfcraft_engine::Edit::StyleAnnotation {
        page: 0,
        index: 1,
        color: Some([0.0, 0.47, 0.84]),
        opacity: None,
        width: None,
        endings: None,
    });
    h.state_mut().apply_edit(pdfcraft_engine::Edit::MarkAnnotation { page: 0, index: 0, marked: true, author: "Tester".into() });
    h.run_steps(2);
    let cards = |h: &Harness<'static, PdfCraftApp>| (h.query_all_by_label("Rectangle").count(), h.query_all_by_label("Oval").count());
    // The filter menu lists the colours by name (counted with it open: it names the types too).
    h.get_by_label("Filter comments").click();
    h.run_steps(2);
    let before = cards(&h);
    h.get_by_label("Blue").click();
    h.run_steps(2);
    assert_eq!(h.state().views[0].comments.hidden_colors, vec!["0077D6".to_string()]);
    assert_eq!(cards(&h).1, before.1 - 1, "the blue oval is filtered out");
    h.get_by_label("Checked").click();
    h.run_steps(2);
    assert_eq!(cards(&h).0, before.0 - 1, "the checked rectangle is filtered out");
    h.get_by_label("Show all").click();
    h.run_steps(2);
    assert_eq!(cards(&h), before);
}

#[test]
fn make_current_properties_default() {
    let mut h = harness(|app| {
        app.set_option("panel", "comments").unwrap();
        app.set_option("quick", "square").unwrap();
    });
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    h.state_mut().set_option("quick", "select").unwrap();
    h.state_mut().apply_edit(pdfcraft_engine::Edit::StyleAnnotation {
        page: 0,
        index: 0,
        color: Some([0.0, 0.47, 0.84]),
        opacity: Some(0.5),
        width: Some(5.0),
        endings: None,
    });
    h.run_steps(2);
    h.get_by_label("More").click();
    h.run_steps(2);
    h.get_by_label("Make Current Properties Default").click();
    h.run_steps(3);
    let st = h.state().comment_prefs.style(pdfcraft_ui_egui::comments::CommentTool::Rectangle);
    assert_eq!((st.color, st.opacity, st.width), ([0.0, 0.47, 0.84], 0.5, 5.0));
    // The default survives a restart (#340): a fresh app reads the persisted settings and its
    // next rectangle still takes the style.
    let json = h.state().persist();
    let mut fresh = PdfCraftApp::new();
    fresh.restore(&json);
    let st = fresh.comment_prefs.style(pdfcraft_ui_egui::comments::CommentTool::Rectangle);
    assert_eq!((st.color, st.opacity, st.width), ([0.0, 0.47, 0.84], 0.5, 5.0), "the tool default persisted");
    // The next rectangle takes it.
    h.state_mut().set_option("quick", "square").unwrap();
    drag_pt(&mut h, (160.0, 100.0), (260.0, 40.0));
    let c = comments(&h);
    let last = c.iter().rfind(|a| a.subtype == "Square").unwrap();
    assert_eq!(last.color, Some([0.0, 0.47, 0.84]));
}

/// Acrobat's Properties ▸ Make Properties Default: OK with the box ticked makes the edited look
/// the tool's default (here a strikethrough's colour); without it, only the comment changes.
#[test]
fn properties_dialog_makes_the_strikethrough_colour_default() {
    use pdfcraft_ui_egui::comments::CommentTool;
    let mut h = harness(|app| app.set_option("quick", "strikeout").unwrap());
    let green = [0.18, 0.62, 0.36];
    let blue = [0.0, 0.47, 0.84];
    assert_ne!(h.state().comment_prefs.style(CommentTool::StrikeOut).color, green);
    drag_pt(&mut h, (18.0, 155.0), (150.0, 155.0));
    assert_eq!(comments(&h).iter().filter(|a| a.subtype == "StrikeOut").count(), 1);

    // Without the box: the comment turns blue, the tool keeps its colour.
    let before = h.state().comment_prefs.style(CommentTool::StrikeOut);
    h.state_mut().open_comment_props(0, 0);
    h.run_steps(2);
    h.state_mut().comment_props.as_mut().expect("open").edited.color = Some(blue);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(h.state().comment_prefs.style(CommentTool::StrikeOut).color, before.color);

    // With it: green, and a 0% opacity is clamped so the next strikethrough stays visible.
    h.state_mut().open_comment_props(0, 0);
    h.run_steps(2);
    {
        let d = h.state_mut().comment_props.as_mut().expect("open");
        d.edited.color = Some(green);
        d.edited.opacity = 0.0;
    }
    h.get_by_label("Make Properties Default").click();
    h.run_steps(1);
    assert!(h.state().comment_props.as_ref().is_some_and(|d| d.make_default));
    h.get_by_label("OK").click();
    h.run_steps(3);
    let st = h.state().comment_prefs.style(CommentTool::StrikeOut);
    assert_eq!((st.color, st.opacity), (green, 0.1));
    let fresh = {
        let mut fresh = PdfCraftApp::new();
        fresh.restore(&h.state().persist());
        fresh.comment_prefs.style(CommentTool::StrikeOut).color
    };
    assert_eq!(fresh, green, "the default persisted");
    drag_pt(&mut h, (18.0, 155.0), (150.0, 155.0));
    let c = comments(&h);
    let made = c.iter().rfind(|a| a.subtype == "StrikeOut").and_then(|a| a.color).expect("a colougreen strikethrough");
    assert!(made.iter().zip(green).all(|(m, r)| (f64::from(*m) - r).abs() < 0.01), "the next strikethrough is green: {made:?}");
}

/// #1302: making an arrow's properties the default updates the Arrow tool, not later plain lines.
#[test]
fn an_arrows_default_does_not_restyle_lines() {
    use pdfcraft_ui_egui::comments::CommentTool;

    let mut h = harness(|app| {
        app.set_option("panel", "comments").unwrap();
        app.set_option("quick", "arrow").unwrap();
    });
    drag_pt(&mut h, (20.0, 40.0), (140.0, 80.0));
    h.state_mut().set_option("quick", "select").unwrap();
    h.state_mut().apply_edit(pdfcraft_engine::Edit::StyleAnnotation {
        page: 0,
        index: 0,
        color: Some([0.0, 0.47, 0.84]),
        opacity: Some(0.5),
        width: Some(6.0),
        endings: None,
    });
    h.run_steps(2);
    h.get_by_label("More").click();
    h.run_steps(2);
    h.get_by_label("Make Current Properties Default").click();
    h.run_steps(3);
    let arrow = h.state().comment_prefs.style(CommentTool::Arrow);
    let line = h.state().comment_prefs.style(CommentTool::Line);
    assert_eq!((arrow.color, arrow.opacity, arrow.width), ([0.0, 0.47, 0.84], 0.5, 6.0));
    assert_eq!((line.color, line.opacity, line.width), ([0.89, 0.13, 0.13], 1.0, 2.0), "plain lines keep their own default");

    h.state_mut().set_option("quick", "arrow").unwrap();
    drag_pt(&mut h, (20.0, 100.0), (140.0, 140.0));
    h.state_mut().set_option("quick", "line").unwrap();
    drag_pt(&mut h, (160.0, 40.0), (260.0, 80.0));
    let c = comments(&h);
    let arrows = c.iter().filter(|a| a.subtype == "Line").collect::<Vec<_>>();
    assert!(arrows.len() >= 3, "{c:?}");
    let next_arrow = &arrows[1];
    let plain = arrows.last().unwrap();
    assert_eq!(next_arrow.color, Some([0.0, 0.47, 0.84]));
    assert_eq!(plain.color, Some([0.89, 0.13, 0.13]));
}

#[test]
fn highlighting_an_area_off_the_text() {
    let mut h = harness(|app| app.set_option("quick", "highlight").unwrap());
    // Blank space below the text line (text is at y 150).
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    let c = comments(&h);
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!((c[0].subtype.as_str(), c[0].quads.len()), ("Highlight", 1));
    let q = c[0].quads[0];
    assert!((q[0] - 40.0).abs() < 3.0 && (q[1] - 100.0).abs() < 3.0, "{q:?}");
    // Dragging over text still highlights the text (once the text layer is in).
    let mut glyphs = None;
    for _ in 0..200 {
        h.run_steps(1);
        let v = &h.state().views[0];
        if let (Some(a), Some(b)) = (v.glyph_screen_pos(0, 4), v.glyph_screen_pos(0, 14)) {
            glyphs = Some((a, b));
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let (a, b) = glyphs.expect("text layer");
    drag(&mut h, a, b);
    assert_eq!(comments(&h).len(), 2);
}

#[test]
fn replacing_text_strikes_it_and_adds_a_caret() {
    let mut h = harness(|app| app.set_option("quick", "replace").unwrap());
    drag_pt(&mut h, (20.0, 155.0), (120.0, 155.0));
    assert!(h.state().views[0].comments.composer.is_some(), "the composer asks for the replacement");
    field(&h, "Replacement text").type_text("slow red");
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let c = comments(&h);
    let strike = c.iter().find(|a| a.subtype == "StrikeOut").expect("a strikeout");
    let caret = c.iter().find(|a| a.subtype == "Caret").expect("a caret");
    assert_eq!(caret.contents.as_deref(), Some("slow red"));
    assert_eq!(caret.in_reply_to, strike.name, "grouped with the strikeout");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Replace text"));
    assert_eq!(h.state().quick_tool, QuickTool::Select);
}

#[test]
fn attaching_a_file_as_a_comment() {
    let mut h = harness(|app| app.set_option("quick", "attach").unwrap());
    h.state_mut().attach_override = Some(("notes.txt".into(), b"remember the milk".to_vec()));
    click_pt(&mut h, (250.0, 180.0));
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!((c.len(), c[0].subtype.as_str(), c[0].contents.as_deref()), (1, "FileAttachment", Some("notes.txt")));
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert!(doc.info.attachments.iter().any(|a| a.name == "notes.txt"), "listed in the Attachments panel");
    assert_eq!(s.quick_tool, QuickTool::Select);
}

/// Drag with the middle button (the mouse wheel) in small steps.
fn middle_drag(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2) {
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Middle, pressed, modifiers: egui::Modifiers::NONE };
    h.event(egui::Event::PointerMoved(from));
    h.run_steps(1);
    h.event(button(from, true));
    h.run_steps(1);
    for k in 1..=4 {
        h.event(egui::Event::PointerMoved(from + (to - from) * (k as f32 / 4.0)));
        h.run_steps(1);
    }
    h.event(button(to, false));
    h.run_steps(3);
}

#[test]
fn a_middle_drag_with_a_drawing_tool_scrolls_instead_of_drawing() {
    // egui's drag responses accept any button, so the drawing tools used to draw with the wheel.
    let mut h = harness(|app| {
        app.set_option("zoom", "400").unwrap();
        app.set_option("quick", "ink").unwrap();
    });
    let from = at(&h, 150.0, 100.0);
    let top = h.state().views[0].page_screen_rect(0).expect("page 1 on screen").top();
    middle_drag(&mut h, from, from - egui::vec2(0.0, 120.0));
    assert!(comments(&h).is_empty(), "{:?}", comments(&h));
    let s = h.state();
    assert!(!s.session.get(s.views[0].id).unwrap().dirty, "scrolling never edits the PDF");
    assert!(!s.views[0].middle_panning(), "releasing the wheel ends the pan");
    let moved = s.views[0].page_screen_rect(0).expect("page 1 on screen").top();
    if cfg!(target_os = "linux") {
        // Linux latches auto-scroll instead of panning with the drag.
        assert!(s.views[0].auto_scrolling());
    } else {
        assert!((moved - (top - 120.0)).abs() < 1.0, "the page follows the pointer: {top} -> {moved}");
    }
    // The tool still draws with the primary button afterwards.
    if cfg!(target_os = "linux") {
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
    }
    drag_pt(&mut h, (60.0, 120.0), (200.0, 80.0));
    let kinds: Vec<String> = comments(&h).into_iter().map(|a| a.subtype).collect();
    assert_eq!(kinds, ["Ink"]);
}

#[test]
fn erasing_part_of_a_drawing() {
    let mut h = harness(|app| app.set_option("quick", "ink").unwrap());
    drag_pt(&mut h, (40.0, 60.0), (260.0, 60.0));
    assert_eq!(comments(&h).len(), 1);
    h.state_mut().set_option("quick", "eraser").unwrap();
    drag_pt(&mut h, (150.0, 90.0), (150.0, 30.0));
    let c = comments(&h);
    assert_eq!(c.len(), 1);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.can_undo(), Some("Erase"));
    // Rubbing along the rest removes the drawing.
    drag_pt(&mut h, (40.0, 60.0), (260.0, 60.0));
    assert!(comments(&h).is_empty(), "{:?}", comments(&h));
}

#[test]
fn hovering_a_comment_shows_its_author_and_text() {
    // A square comment by Ada over the first page's text.
    let pdf = String::from_utf8_lossy(TEXT_FIXTURE).replace("/Contents 5 0 R /Resources", "/Contents 5 0 R /Annots [8 0 R] /Resources").replace(
        "trailer",
        "8 0 obj << /Type /Annot /Subtype /Square /Rect [50 50 150 120] /C [1 0 0] /T (Ada) /Contents (Check this figure) >> endobj\ntrailer",
    );
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("hover.pdf", None, pdf.into_bytes()).expect("opens");
        app.set_option("left", "closed").unwrap();
        // No Comments panel: the comment's text should only appear in the hover popup.
        app.set_option("panel", "none").unwrap();
        app
    });
    h.run_steps(4);
    settle(&mut h);
    assert!(h.query_by_label_contains("Check this figure").is_none(), "no popup before hovering");
    let p = at(&h, 100.0, 85.0);
    h.hover_at(p);
    h.run_steps(3);
    h.get_by_label_contains("Check this figure");
    h.get_by_label_contains("Ada");
    // The tip is drawn above every window. A dialog must hide it, or the author floats over the dialog.
    h.state_mut().set_option("dialog", "shortcuts").unwrap();
    h.hover_at(p);
    h.run_steps(3);
    assert!(h.query_by_label_contains("Check this figure").is_none(), "a dialog covers the page hover");
}

#[test]
fn preference_hides_author_only_highlight_hover_previews() {
    // A highlight annotation with author Ada and empty contents over the first page.
    let pdf = String::from_utf8_lossy(TEXT_FIXTURE).replace("/Contents 5 0 R /Resources", "/Contents 5 0 R /Annots [8 0 R] /Resources").replace(
        "trailer",
        "8 0 obj << /Type /Annot /Subtype /Highlight /Rect [50 50 150 120] /C [1 1 0] /T (Ada) /QuadPoints [50 120 150 120 50 50 150 50] >> endobj\ntrailer",
    );
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("highlight_hover.pdf", None, pdf.into_bytes()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app
    });
    h.run_steps(4);
    settle(&mut h);

    let p = at(&h, 100.0, 85.0);
    // By default, author-only hover is shown.
    h.hover_at(p);
    h.run_steps(3);
    h.get_by_label("Ada");

    // Enabling preference hides author-only highlight hover.
    h.state_mut().set_option("hide-empty-highlight-hover", "true").unwrap();
    h.hover_at(p);
    h.run_steps(3);
    assert!(h.query_by_label("Ada").is_none(), "author-only hover must be suppressed");
}

#[test]
fn the_squiggly_tool_marks_selected_text() {
    let mut h = harness(|_| {});
    h.state_mut().views[0].select_text(0, 4, 8);
    assert!(h.state_mut().execute("comment.squiggly"));
    h.run_steps(2);
    let c = comments(&h);
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!((c[0].subtype.as_str(), c[0].quads.len()), ("Squiggly", 1));
    // It sits with the other text markup in the Highlight ▸ flyout, and edits as itself.
    assert!(pdfcraft_ui_egui::comments::GROUPS[1].contains(&pdfcraft_ui_egui::comments::CommentTool::Squiggly));
    assert_eq!(pdfcraft_ui_egui::comments::tool_for(&c[0]), Some(pdfcraft_ui_egui::comments::CommentTool::Squiggly));
}

#[test]
fn the_opacity_set_for_a_tool_goes_into_its_new_comments() {
    use pdfcraft_ui_egui::comments::CommentTool;
    let mut h = harness(|app| app.comment_prefs.set_opacity(CommentTool::StrikeOut, 0.5));
    // Clamped: an invisible comment can't be found again.
    h.state_mut().comment_prefs.set_opacity(CommentTool::Underline, 0.0);
    assert!((h.state().comment_prefs.style(CommentTool::Underline).opacity - 0.1).abs() < 1e-9);
    h.state_mut().views[0].select_text(0, 4, 8);
    assert!(h.state_mut().execute("comment.strikeout"));
    h.run_steps(2);
    let c = comments(&h);
    assert_eq!(c.len(), 1, "{c:?}");
    // The new comment carries /CA 0.5 (read back from the document's bytes).
    let bytes = h.state().session.get(h.state().views[0].id).unwrap().bytes.as_ref().clone();
    let doc = pdfcraft_cos::Document::open(std::sync::Arc::new(bytes)).unwrap();
    let opacities: Vec<f64> = doc
        .object_numbers()
        .into_iter()
        .filter_map(|n| doc.try_get(n).ok())
        .filter_map(|o| o.as_dict().cloned())
        .filter(|d| d.name(b"Subtype") == Some(b"StrikeOut"))
        .filter_map(|d| d.get(b"CA").and_then(|v| v.as_f64()))
        .collect();
    assert_eq!(opacities, [0.5]);
    // The quick bar offers the control.
    h.state_mut().set_option("quick", "strikeout").unwrap();
    h.run_steps(2);
    h.get_by_label("Opacity");
}

/// #225: comment tools open the Comments panel until the user closes it; then it stays closed,
/// across restarts, until they open it again.
#[test]
fn a_closed_comments_panel_stays_closed_when_picking_comment_tools() {
    use pdfcraft_ui_egui::RightPanel;
    let mut h = harness(|_| {});
    assert_eq!(h.state().right, None);
    assert!(h.state_mut().execute("comment.highlight"));
    h.run_steps(2);
    assert_eq!(h.state().right, Some(RightPanel::Comments), "Acrobat opens Comments with the tools");
    h.get_by_label("Close").click();
    h.run_steps(2);
    assert_eq!(h.state().right, None);
    for tool in ["select", "comment.underline", "comment.note", "comment.highlight"] {
        if tool == "select" {
            h.state_mut().quick_tool = QuickTool::Select;
        } else {
            assert!(h.state_mut().execute(tool));
        }
        h.run_steps(2);
        assert_eq!(h.state().right, None, "{tool} reopened the closed Comments panel");
    }
    let mut again = PdfCraftApp::new();
    again.restore(&h.state().persist());
    assert!(again.comments_panel_closed, "the choice survives a restart");
    // Opening it again from the rail lets the tools open it once more.
    h.get_by_label("Comments").click();
    h.run_steps(2);
    assert_eq!(h.state().right, Some(RightPanel::Comments));
    assert!(!h.state().comments_panel_closed);
    // What the rail and View menu call; "Bookmarks" labels two controls on screen.
    h.state_mut().choose_right_panel(Some(RightPanel::Bookmarks));
    h.state_mut().choose_right_panel(None);
    h.run_steps(2);
    assert!(!h.state().comments_panel_closed, "switching to and closing another panel isn't closing Comments");
    assert!(h.state_mut().execute("comment.strikeout"));
    h.run_steps(2);
    assert_eq!(h.state().right, Some(RightPanel::Comments));
}

/// #1908: Comment properties of a text box has a Font size control; changing it redraws the box
/// with the new /DA size as one undo step, and shapes without text have no such control.
#[test]
fn text_box_properties_change_the_font_size() {
    use pdfcraft_engine::{Edit, NewAnnotation, Shape};
    let mut h = harness(|_| {});
    let tb = NewAnnotation {
        page: 0,
        shape: Shape::TextBox { rect: [40.0, 600.0, 240.0, 640.0], font_size: 12.0 },
        style: Default::default(),
        contents: "Hello".into(),
        author: "Ada".into(),
    };
    h.state_mut().apply_edit(Edit::AddAnnotation(tb));
    h.run_steps(2);
    h.state_mut().open_comment_props(0, 0);
    h.run_steps(2);
    h.get_by_label("Font size");
    {
        let d = h.state_mut().comment_props.as_mut().expect("open");
        assert_eq!(d.original.font_size, Some(12.0));
        d.edited.font_size = Some(30.0);
    }
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.comment_props(0, 0).unwrap().font_size, Some(30.0));
    assert_eq!(doc.can_undo(), Some("Change comment properties"));
}
