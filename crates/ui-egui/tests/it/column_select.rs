//! Issue #740: column select, as in Acrobat. Alt-drag (Option-drag on macOS) with the Select tool,
//! or a plain drag with Edit ▸ Column select, selects only the text inside the rectangle drawn.

use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

/// One 300×200 page with a three-row table, "Name Qty / Apple 12 / Pear 7". Its columns sit far
/// enough apart that reading order takes one column, then the other: glyphs 0–3 "Name", 4–8
/// "Apple", 9–12 "Pear", 13–15 "Qty", 16–17 "12", 18 "7".
fn table() -> Vec<u8> {
    let content =
        "BT /F1 14 Tf 20 150 Td (Name) Tj 130 0 Td (Qty) Tj -130 -24 Td (Apple) Tj 130 0 Td (12) Tj -130 -24 Td (Pear) Tj 130 0 Td (7) Tj ET";
    format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF",
        content.len()
    )
    .into_bytes()
}

fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("table.pdf", None, table()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("author", "Tester").unwrap();
        setup(&mut app);
        app
    });
    // Wait for the text layer itself (#1277): under load the first render takes longer than a
    // fixed second, and nothing may be pending yet before the text layer is asked for.
    crate::support::wait_until(&mut h, "the page's text layer", |app| app.views[0].glyph_screen_pos(0, 0).is_some() && !app.render_pending());
    h
}

fn glyph(h: &Harness<'static, PdfCraftApp>, g: usize) -> Pos2 {
    h.state().views[0].glyph_screen_pos(0, g).expect("glyph on screen")
}

fn text(h: &Harness<'static, PdfCraftApp>) -> Option<String> {
    h.state().views[0].selected_text()
}

/// Press at `from`, move to `to` and release there, with `modifiers` held throughout.
fn drag(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2, modifiers: Modifiers) {
    h.event(Event::ModifiersChanged(modifiers));
    h.hover_at(from);
    h.run_steps(1);
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers });
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(from + (to - from) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers });
    h.run_steps(1);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
}

/// Below-right of the top two rows: past the "y" of "Qty" (the wider cell), level with "12".
fn below_right_of_row_two(h: &Harness<'static, PdfCraftApp>) -> Pos2 {
    egui::pos2(glyph(h, 15).x.max(glyph(h, 17).x) + 12.0, glyph(h, 17).y + 8.0)
}

/// From above-left of "Name" to below-right of "12": the top two rows of both columns.
fn top_two_rows(h: &Harness<'static, PdfCraftApp>) -> (Pos2, Pos2) {
    (glyph(h, 0) - vec2(12.0, 12.0), below_right_of_row_two(h))
}

#[test]
fn alt_drag_selects_only_the_text_inside_the_rectangle() {
    let mut h = harness(|_| {});
    let (from, to) = top_two_rows(&h);
    // A plain drag selects the rows it covers. Reading order, column by column, would take "Pear"
    // from the row below on its way from "Name" to "12".
    drag(&mut h, from, to, Modifiers::NONE);
    assert_eq!(text(&h).as_deref(), Some("Name\tQty\nApple\t12"));
    h.run_steps(60);
    // Alt (Option on macOS) at the start of the drag: only the rectangle, row by row.
    drag(&mut h, from, to, Modifiers::ALT);
    assert_eq!(text(&h).as_deref(), Some("Name\tQty\nApple\t12"));
    // One column of numbers, which is what the issue asked for.
    h.run_steps(60);
    let (from, to) = (glyph(&h, 13) - vec2(8.0, 12.0), egui::pos2(glyph(&h, 15).x + 12.0, glyph(&h, 18).y + 12.0));
    drag(&mut h, from, to, Modifiers::ALT);
    assert_eq!(text(&h).as_deref(), Some("Qty\n12\n7"));
}

#[test]
fn the_column_select_tool_needs_no_modifier() {
    let mut h = harness(|app| assert!(app.execute("edit.column_select")));
    assert_eq!(h.state().quick_tool, pdfcraft_ui_egui::QuickTool::ColumnSelect);
    let (from, to) = top_two_rows(&h);
    drag(&mut h, from, to, Modifiers::NONE);
    assert_eq!(text(&h).as_deref(), Some("Name\tQty\nApple\t12"));
    // A rectangle around no text selects nothing.
    h.run_steps(60);
    let below = glyph(&h, 12) + vec2(0.0, 60.0);
    drag(&mut h, below, below + vec2(60.0, 30.0), Modifiers::NONE);
    assert_eq!(text(&h), None);
}

/// ⇧-click extends a reading-order selection; a column selection has no anchor glyph, so a
/// ⇧-click clears it like a plain click.
#[test]
fn shift_click_clears_a_column_selection() {
    let mut h = harness(|_| {});
    let (from, to) = top_two_rows(&h);
    drag(&mut h, from, to, Modifiers::ALT);
    assert!(text(&h).is_some());
    h.run_steps(60);
    let at = glyph(&h, 10);
    h.event(Event::ModifiersChanged(Modifiers::SHIFT));
    h.hover_at(at);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::SHIFT });
    }
    h.run_steps(1);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
    assert_eq!(text(&h), None);
}

/// Alt-drag with the highlighter marks only the cells inside the rectangle: one quad per piece
/// of a line. The drag starts on text, since off text the highlighter draws an area highlight.
#[test]
fn alt_drag_with_the_highlighter_marks_only_the_column() {
    let mut h = harness(|app| assert!(app.execute("comment.highlight")));
    // Just above-left of the centre of "N", still on the glyph.
    let (from, to) = (glyph(&h, 0) - vec2(2.0, 2.0), below_right_of_row_two(&h));
    drag(&mut h, from, to, Modifiers::ALT);
    h.run_steps(3);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    let marks: Vec<(&str, usize)> = doc.info.annotations.iter().map(|a| (a.subtype.as_str(), a.quads.len())).collect();
    assert_eq!(marks, [("Highlight", 4)], "Name, Qty, Apple and 12, each its own quad");
}

/// A plain drag along one row of a table that reads column by column selects that row, as
/// Preview does, not the rest of the first column and the top of the second between its ends.
/// The highlighter marks the same cells.
#[test]
fn a_plain_drag_along_one_table_row_selects_only_that_row() {
    let mut h = harness(|_| {});
    // From just left of "Apple" to past "12", level with both.
    let (from, to) = (glyph(&h, 4) - vec2(4.0, 0.0), glyph(&h, 17) + vec2(12.0, 0.0));
    for (a, b) in [(from, to), (to, from)] {
        h.key_press(egui::Key::Escape);
        h.run_steps(60);
        drag(&mut h, a, b, Modifiers::NONE);
        assert_eq!(text(&h).as_deref(), Some("Apple\t12"));
    }

    let mut h = harness(|app| assert!(app.execute("comment.highlight")));
    // The highlighter starts on text: the centre of "A".
    let (from, to) = (glyph(&h, 4), glyph(&h, 17) + vec2(12.0, 0.0));
    drag(&mut h, from, to, Modifiers::NONE);
    h.run_steps(3);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    let marks: Vec<(&str, usize)> = doc.info.annotations.iter().map(|a| (a.subtype.as_str(), a.quads.len())).collect();
    assert_eq!(marks, [("Highlight", 2)], "Apple and 12, each its own quad");
}

/// A drag down from "Name" that ends in the gap between the columns, level with "Pear", crosses
/// into the rows: the nearest glyph is still in the first column, but the pointer is not.
#[test]
fn a_drag_ending_between_the_columns_selects_the_rows() {
    let mut h = harness(|_| {});
    let (from, pear) = (glyph(&h, 0) - vec2(4.0, 0.0), glyph(&h, 12));
    // A little right of "Pear", well short of "7".
    let gap = pear + vec2((glyph(&h, 18).x - pear.x) / 4.0, 0.0);
    drag(&mut h, from, gap, Modifiers::NONE);
    assert_eq!(text(&h).as_deref(), Some("Name\tQty\nApple\t12\nPear"));
    // Over the first column, the drag stays in it.
    h.key_press(egui::Key::Escape);
    h.run_steps(60);
    drag(&mut h, from, pear, Modifiers::NONE);
    assert_eq!(text(&h).as_deref(), Some("Name\nApple\nPear"));
}

/// Two article columns, the right one a line shorter: its last glyph, the end of reading order,
/// sits above the left column's last line.
fn uneven_columns() -> Vec<u8> {
    let content = "BT /F1 14 Tf \
        1 0 0 1 20 250 Tm (Left first line.) Tj 1 0 0 1 220 250 Tm (Right first line.) Tj \
        1 0 0 1 20 225 Tm (Left other line.) Tj 1 0 0 1 220 225 Tm (Right final line.) Tj \
        1 0 0 1 20 200 Tm (Left final line.) Tj ET";
    format!(
        "%PDF-1.7\n\
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj\n\
4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n\
5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Courier >> endobj\n\
trailer << /Root 1 0 R >>\n%%EOF",
        content.len()
    )
    .into_bytes()
}

/// Select all takes the page's text in reading order, whole: it has no pointer, so the rows a
/// drag would take don't apply, though its last glyph is not the lowest.
#[test]
fn select_all_keeps_every_line_of_uneven_columns() {
    let mut h = uneven_harness();
    assert!(h.state_mut().views[0].select_all());
    h.run_steps(1);
    assert_eq!(text(&h).as_deref(), Some(UNEVEN_PAGE_TEXT));
}

/// Four clicks select the page, and a pointer that moves on a little after the fourth press
/// keeps it whole: the page unit has no rows to take.
#[test]
fn four_clicks_then_a_short_drag_keep_the_whole_page() {
    let mut h = uneven_harness();
    let at = glyph(&h, 2);
    for _ in 0..3 {
        for pressed in [true, false] {
            h.hover_at(at);
            h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
            h.run_steps(1);
        }
    }
    drag(&mut h, at, at + vec2(30.0, 8.0), Modifiers::NONE);
    assert_eq!(text(&h).as_deref(), Some(UNEVEN_PAGE_TEXT));
}

fn uneven_harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("uneven.pdf", None, uneven_columns()).expect("opens");
        app
    });
    crate::support::wait_until(&mut h, "the page's text layer", |app| app.views[0].glyph_screen_pos(0, 0).is_some() && !app.render_pending());
    h
}

const UNEVEN_PAGE_TEXT: &str = "Left first line.\nLeft other line.\nLeft final line.\nRight first line.\nRight final line.";

/// A drag after a double-click extends by whole words, and over a table it takes the rows as a
/// plain drag does: from "Name" down to "12", not reading order's detour through "Pear".
#[test]
fn a_double_click_drag_over_table_rows_selects_the_rows() {
    let mut h = harness(|_| {});
    let (name, two) = (glyph(&h, 1), glyph(&h, 17));
    for pressed in [true, false] {
        h.hover_at(name);
        h.event(Event::PointerButton { pos: name, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.run_steps(1);
    }
    drag(&mut h, name, two, Modifiers::NONE);
    assert_eq!(text(&h).as_deref(), Some("Name\tQty\nApple\t12"));
}
