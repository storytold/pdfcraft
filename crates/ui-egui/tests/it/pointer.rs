//! What the page's tools see of the pointer. A finger tap, a touch drag and a click whose press
//! and release share a frame act as a mouse click and drag do, and nothing drawn over the page (a
//! dialog, the palette, the quick bar, the chrome over a page scrolled under it) lets the page's
//! cursor, tips or presses through.
//!
//! The ignored tests pin live bugs that later steps of the page pointer router fix: one cursor
//! writer (step 2), one hit test (step 3), and press-time capture with cancel (step 4).

use egui_kittest::Harness;
use pdfcraft_engine::StampKind;
use pdfcraft_ui_egui::comments::CommentTool;
use pdfcraft_ui_egui::{PdfCraftApp, QuickTool, UnlockPrompt};

use crate::support::input::{coalesced_click, double_click, one_frame, touch_drag, touch_tap};

/// A PDF of `objects`, numbered from 1 with the catalog first.
fn pdf(objects: &[&str]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

fn stream(content: &str) -> String {
    format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len() + 1)
}

const CATALOG: &str = "<< /Type /Catalog /Pages 2 0 R >>";
const PAGES: &str = "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>";
const HELVETICA: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>";
/// One line of 12 pt Helvetica at x 40, y 300.
const LINE: &str = "BT /F1 12 Tf 40 300 Td (Signed and approved by the committee.) Tj ET";

/// One 300×400 page with a web link at x 50..250, y 300..350.
fn link_pdf() -> Vec<u8> {
    pdf(&[
        CATALOG,
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Link /Rect [50 300 250 350] /Border [0 0 0] /A << /S /URI /URI (https://example.org/) >> >>",
    ])
}

/// A 300×400 page with a paragraph at y≈300 and a link over its first words.
fn linked_text_pdf() -> Vec<u8> {
    let text = stream("BT /F1 12 Tf 40 300 Td (Read the guide at example.org before you start.) Tj ET");
    pdf(&[
        CATALOG,
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 6 0 R >> >> /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Link /Rect [40 295 160 312] /Border [0 0 0] /A << /S /URI /URI (https://example.org/) >> >>",
        text.as_str(),
        HELVETICA,
    ])
}

/// A 300×400 page with a line of text at y≈300 and a rectangle comment over it.
fn comment_on_text_pdf() -> Vec<u8> {
    let text = stream(LINE);
    pdf(&[
        CATALOG,
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 6 0 R >> >> /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Square /Rect [30 290 200 320] /C [1 0 0] /Border [0 0 2] /F 4 /T (Reviewer) >>",
        text.as_str(),
        HELVETICA,
    ])
}

/// A 300×400 page with one line of text at x 40, y 300.
fn text_pdf() -> Vec<u8> {
    let text = stream(LINE);
    pdf(&[CATALOG, PAGES, "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>", text.as_str(), HELVETICA])
}

/// A 300×400 page with an unchecked check box `agree` at x 100..120, y 200..220.
fn checkbox_pdf() -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /DA (/ZaDb 0 Tf 0 g) /DR << /Font << /ZaDb 5 0 R >> >> >> >>",
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /V /Off /AS /Off /Rect [100 200 120 220] /P 3 0 R /F 4 /MK << /CA (4) >> /DA (/ZaDb 11 Tf 0 g) >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /ZapfDingbats >>",
    ])
}

/// The app with `pdf` open at `zoom` percent and its first frames run. The left panel stays
/// open only where a test places the page relative to the window's chrome.
fn open(pdf: Vec<u8>, zoom: &'static str, left_open: bool) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("test.pdf", None, pdf).unwrap();
        if !left_open {
            app.set_option("left", "closed").unwrap();
        }
        app.set_option("zoom", zoom).unwrap();
        app
    });
    for _ in 0..30 {
        h.run_steps(2);
    }
    h
}

fn stamp_tool(h: &mut Harness<'_, PdfCraftApp>) {
    h.state_mut().quick_tool = QuickTool::Stamp(StampKind::DynApproved);
    h.run_steps(4);
}

fn page(h: &Harness<'_, PdfCraftApp>) -> egui::Rect {
    h.state().views[0].page_screen_rect(0).expect("page on screen")
}

/// The screen point at PDF (x, y) on the 300×400 page.
fn at(h: &Harness<'_, PdfCraftApp>, x: f32, y: f32) -> egui::Pos2 {
    let r = page(h);
    let k = r.width() / 300.0;
    r.min + egui::vec2(x * k, (400.0 - y) * k)
}

fn cursor_at(h: &mut Harness<'_, PdfCraftApp>, p: egui::Pos2) -> egui::CursorIcon {
    h.hover_at(p);
    h.run_steps(2);
    h.output().platform_output.cursor_icon
}

/// A mouse click: press and release in frames of their own, as `drag_at` and `drop_at` send them.
fn mouse_click(h: &mut Harness<'_, PdfCraftApp>, p: egui::Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(4);
}

/// A mouse drag from `from` by `by`, held over two frames on the way.
fn mouse_drag(h: &mut Harness<'_, PdfCraftApp>, from: egui::Pos2, by: egui::Vec2) {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    h.hover_at(from + by);
    h.run_steps(2);
    h.drop_at(from + by);
    for _ in 0..10 {
        h.run_steps(2);
    }
}

fn stamps(h: &Harness<'_, PdfCraftApp>) -> usize {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().info.annotations.iter().filter(|a| a.subtype == "Stamp").count()
}

fn square(h: &Harness<'_, PdfCraftApp>) -> Option<[f32; 4]> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().info.annotations.iter().find(|a| a.subtype == "Square").map(|a| a.rect)
}

fn agree(h: &Harness<'_, PdfCraftApp>) -> Vec<String> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "agree").unwrap().value.clone()
}

/// Hovers `p` until `want` accepts the cursor there (text layers and paragraph boxes arrive from
/// worker threads), and returns the last cursor seen.
fn wait_for_cursor(h: &mut Harness<'_, PdfCraftApp>, p: egui::Pos2, want: impl Fn(egui::CursorIcon) -> bool) -> egui::CursorIcon {
    let mut cursor = egui::CursorIcon::Default;
    for _ in 0..50 {
        cursor = cursor_at(h, p);
        if want(cursor) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    cursor
}

// ---------------------------------------------------------------------------------------------
// The cursor promises one thing and a press does another.

#[test]
#[ignore = "step 2 of the page pointer router: one cursor writer"]
fn stamp_tool_over_a_link_shows_its_crosshair() {
    let mut h = open(link_pdf(), "150", false);
    stamp_tool(&mut h);
    let on_link = at(&h, 150.0, 325.0);
    let off_link = at(&h, 150.0, 150.0);
    assert_eq!(cursor_at(&mut h, off_link), egui::CursorIcon::Crosshair, "the tool's cursor off the link");
    let cursor = cursor_at(&mut h, on_link);
    mouse_click(&mut h, on_link);
    assert_eq!((stamps(&h), h.state().pending_link.is_some()), (1, false), "a click places a stamp and does not follow the link");
    assert_eq!(cursor, egui::CursorIcon::Crosshair, "so the cursor is the stamp tool's, not the link hand");
}

#[test]
#[ignore = "step 2 of the page pointer router: one cursor writer"]
fn rectangle_tool_over_a_link_shows_its_crosshair() {
    let mut h = open(link_pdf(), "150", false);
    h.state_mut().quick_tool = QuickTool::Comment(CommentTool::Rectangle);
    h.run_steps(4);
    let off_link = at(&h, 150.0, 150.0);
    let off = cursor_at(&mut h, off_link);
    let on_link = at(&h, 150.0, 325.0);
    let on = cursor_at(&mut h, on_link);
    assert_eq!(on, off, "a drag over the link draws a rectangle, as it does off the link");
}

#[test]
#[ignore = "step 2 of the page pointer router: one cursor writer"]
fn edit_text_over_a_linked_word_offers_the_move_arrow() {
    let mut h = open(linked_text_pdf(), "150", false);
    h.state_mut().quick_tool = QuickTool::EditText;
    for _ in 0..30 {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // One paragraph, one line: a word outside the link, then a word inside it.
    let plain = at(&h, 230.0, 303.0);
    let linked = at(&h, 60.0, 303.0);
    let off = wait_for_cursor(&mut h, plain, |c| c == egui::CursorIcon::Move);
    let on = cursor_at(&mut h, linked);
    let id = h.state().views[0].id;
    let before = h.state().session.get(id).unwrap().text_blocks(0)[0].rect;
    let k = page(&h).width() / 300.0;
    mouse_drag(&mut h, linked, egui::vec2(20.0 * k, 0.0));
    let after = h.state().session.get(id).unwrap().text_blocks(0)[0].rect;
    assert_eq!(off, egui::CursorIcon::Move, "a closed paragraph offers the four-way arrow (#1717)");
    assert!((after[0] - before[0] - 20.0).abs() < 1.5, "a drag from the linked word moves the paragraph: {before:?} → {after:?}");
    assert_eq!(on, off, "the linked words belong to the same paragraph, and a drag there moves it too");
}

#[test]
#[ignore = "step 2 of the page pointer router: one cursor writer"]
fn select_tool_over_a_comment_on_text_offers_the_move_arrow() {
    let mut h = open(comment_on_text_pdf(), "150", false);
    // Inside the rectangle where there is no text, and inside it over a word.
    let blank = at(&h, 120.0, 315.0);
    let on_text = at(&h, 60.0, 303.0);
    let over_blank = cursor_at(&mut h, blank);
    let over_text = wait_for_cursor(&mut h, on_text, |c| c != egui::CursorIcon::Default);
    let before = square(&h).expect("the rectangle");
    let k = page(&h).width() / 300.0;
    mouse_drag(&mut h, on_text, egui::vec2(20.0 * k, 0.0));
    let after = square(&h).expect("the rectangle");
    assert_eq!(over_blank, egui::CursorIcon::Move, "a movable comment offers the four-way arrow");
    assert!((after[0] - before[0] - 20.0).abs() < 1.5, "a drag from the text moves the rectangle: {before:?} → {after:?}");
    assert_eq!(over_text, egui::CursorIcon::Move, "so it offers the arrow where text lies under it too");
}

/// Narrows the paragraph by dragging its left edge 100 pt inward, starting `dx` screen points
/// left of the edge. Returns the cursor shown at the press point and the line count after.
fn narrow_from_left(dx: f32) -> (egui::CursorIcon, usize) {
    let mut h = open(text_pdf(), "150", false);
    assert!(h.state_mut().execute("edit.edit_text"));
    for _ in 0..20 {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let id = h.state().views[0].id;
    let b = h.state().session.get(id).unwrap().text_blocks(0)[0].clone();
    let k = page(&h).width() / 300.0;
    let grip = at(&h, b.rect[0] as f32, (b.rect[1] + b.rect[3]) as f32 / 2.0) - egui::vec2(dx, 0.0);
    let cursor = cursor_at(&mut h, grip);
    h.drag_at(grip);
    h.run_steps(1);
    for i in 1..=4 {
        h.hover_at(grip + egui::vec2(25.0 * k * i as f32, 0.0));
        h.run_steps(1);
    }
    h.drop_at(grip + egui::vec2(100.0 * k, 0.0));
    for _ in 0..10 {
        h.run_steps(2);
    }
    (cursor, h.state().session.get(id).unwrap().text_lines(0).len())
}

#[test]
#[ignore = "step 3 of the page pointer router: one hit test"]
fn a_paragraph_edge_resizes_wherever_it_shows_the_resize_cursor() {
    let (inside_cursor, inside_lines) = narrow_from_left(1.0);
    assert_eq!(inside_cursor, egui::CursorIcon::ResizeHorizontal);
    assert!(inside_lines > 1, "control: a drag from the edge rewraps the paragraph");
    let (outside_cursor, outside_lines) = narrow_from_left(5.0);
    assert_eq!(outside_cursor, egui::CursorIcon::ResizeHorizontal, "the resize cursor shows 5 pt out");
    assert!(outside_lines > 1, "so a drag from there rewraps it too");
}

// ---------------------------------------------------------------------------------------------
// Things drawn over the page keep the page's cursor, tips and presses off.

#[test]
fn stamp_cursor_stays_off_the_command_palette() {
    let mut h = open(link_pdf(), "300", false);
    stamp_tool(&mut h);
    h.state_mut().palette_open = true;
    h.run_steps(4);
    let palette = h.ctx.memory(|m| m.area_rect(egui::Id::new("palette"))).expect("palette shown");
    // On the palette's frame margin, where none of its widgets sets a cursor, over the page.
    let p = egui::pos2(palette.left() + 4.0, palette.bottom() - 4.0);
    assert!(page(&h).contains(p), "the palette must lie over the page here");
    assert_eq!(cursor_at(&mut h, p), egui::CursorIcon::Default);
    // The page beside the palette still shows the tool.
    let beside = egui::pos2(palette.left() + 4.0, palette.bottom() + 40.0);
    assert!(page(&h).contains(beside));
    assert_eq!(cursor_at(&mut h, beside), egui::CursorIcon::Crosshair);
}

/// A link beside the open palette shows no tip: tips draw above the palette.
#[test]
fn link_tip_stays_off_while_the_palette_is_open() {
    use egui_kittest::kittest::Queryable;
    let mut h = open(link_pdf(), "300", false);
    h.state_mut().palette_open = true;
    h.run_steps(4);
    let palette = h.ctx.memory(|m| m.area_rect(egui::Id::new("palette"))).expect("palette shown");
    let beside = [60.0, 100.0, 150.0, 200.0, 240.0]
        .into_iter()
        .flat_map(|x| [305.0, 325.0, 345.0].map(|y| at(&h, x, y)))
        .find(|p| page(&h).contains(*p) && !palette.expand(40.0).contains(*p))
        .unwrap_or_else(|| panic!("no point of the link beside the palette {palette:?}, page {:?}", page(&h)));
    h.hover_at(beside);
    h.run_steps(3);
    assert!(h.query_by_label("https://example.org/").is_none(), "no tip beside the palette");
    h.state_mut().palette_open = false;
    h.hover_at(beside);
    h.run_steps(3);
    h.get_by_label("https://example.org/");
}

fn wheel(h: &mut Harness<'_, PdfCraftApp>, delta: egui::Vec2) {
    h.hover_at(egui::pos2(800.0, 500.0));
    h.event(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta, modifiers: egui::Modifiers::NONE, phase: egui::TouchPhase::Move });
    for _ in 0..10 {
        h.run_steps(2);
    }
}

#[test]
fn stamp_cursor_stays_off_the_quick_bar() {
    let mut h = open(link_pdf(), "400", true);
    stamp_tool(&mut h);
    // Bring the page under the quick bar's left end.
    wheel(&mut h, egui::vec2(-200.0, 0.0));
    let bar = h.ctx.memory(|m| m.area_rect(egui::Id::new("quick-bar"))).expect("quick bar shown");
    // Inside the bar's frame margin: no button there.
    let p = egui::pos2(bar.left() + 3.0, bar.bottom() - 3.0);
    assert!(page(&h).contains(p), "the quick bar must lie over the page here");
    assert_eq!(cursor_at(&mut h, p), egui::CursorIcon::Default);
    let below = egui::pos2(bar.left() + 3.0, bar.bottom() + 20.0);
    assert!(page(&h).contains(below));
    assert_eq!(cursor_at(&mut h, below), egui::CursorIcon::Crosshair);
}

#[test]
fn stamp_cursor_stays_off_the_chrome_above_a_page_scrolled_under_it() {
    let mut h = open(link_pdf(), "400", true);
    stamp_tool(&mut h);
    let top = page(&h).top();
    wheel(&mut h, egui::vec2(0.0, -300.0));
    let page = page(&h);
    // Above where the page started (the viewport's top edge, give or take its margin): the
    // toolbar, with the page scrolled up under it. Left of the link, which shows the hand.
    let x = page.left() + 20.0;
    let p = egui::pos2(x, top - 45.0);
    assert!(page.contains(p), "the page must extend under the chrome here");
    // The toolbar keeps its own cursor (a button's hand here); the page's is gone.
    assert_ne!(cursor_at(&mut h, p), egui::CursorIcon::Crosshair);
    assert_eq!(cursor_at(&mut h, egui::pos2(x, top + 40.0)), egui::CursorIcon::Crosshair);
}

/// Whether the page's hover tip was drawn in the last frame.
fn tip_shown(h: &Harness<'_, PdfCraftApp>) -> bool {
    let tip = egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("canvas-hover"));
    h.ctx.memory(|m| m.areas().is_visible(&tip))
}

/// Opens the Unlock prompt, a dialog with a backdrop over the whole window, from the next frame.
fn open_unlock_prompt(h: &mut Harness<'_, PdfCraftApp>) {
    let doc = h.state().views[0].id;
    h.state_mut().unlock_prompt = Some(UnlockPrompt { doc, name: "test.pdf".into(), input: String::new(), error: None, then: None });
}

#[test]
fn link_tip_stays_under_the_unlock_prompt() {
    let mut h = open(link_pdf(), "300", true);
    let page = page(&h);
    let k = page.width() / 300.0;
    // Near the link's top left corner, clear of the prompt's card in the middle of the window.
    let on_link = page.min + egui::vec2(55.0 * k, (400.0 - 345.0) * k);
    assert_eq!(cursor_at(&mut h, on_link), egui::CursorIcon::PointingHand);
    h.run_steps(2);
    assert!(tip_shown(&h), "the link's tip shows over an uncovered page");

    open_unlock_prompt(&mut h);
    h.hover_at(on_link + egui::vec2(1.0, 0.0));
    for _ in 0..4 {
        h.run_steps(2);
    }
    let card = h.ctx.memory(|m| m.area_rect(egui::Id::new("unlock")));
    assert!(card.is_none_or(|c| !c.contains(on_link)), "the point must be on the backdrop, not the card: {card:?}");
    assert!(!tip_shown(&h), "the link's tip must not float over the Unlock prompt");
    assert_ne!(h.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand);
}

#[test]
fn the_link_tip_lets_the_pointer_through_to_the_link() {
    let mut h = open(link_pdf(), "150", false);
    let p = at(&h, 100.0, 340.0);
    assert_eq!(cursor_at(&mut h, p), egui::CursorIcon::PointingHand);
    let tip = h.ctx.memory(|m| m.area_rect(egui::Id::new("canvas-hover"))).expect("the link's tip");
    // A quick move lands on the tip, drawn beside where the pointer was, and still on the link.
    let onto = tip.min + egui::vec2(6.0, 6.0);
    assert!(egui::Rect::from_two_pos(at(&h, 50.0, 350.0), at(&h, 250.0, 300.0)).contains(onto), "the point must be on the link");
    h.hover_at(onto);
    h.run_steps(1);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand, "the tip must not take the pointer from the link");
}

// Dialogs are drawn after the pages, so on the frame one opens egui does not know its layer yet
// and still gives the pages the pointer, and a press there.

#[test]
fn the_link_tip_stays_under_a_dialog_on_the_frame_it_opens() {
    let mut h = open(link_pdf(), "150", false);
    let p = at(&h, 100.0, 340.0);
    assert_eq!(cursor_at(&mut h, p), egui::CursorIcon::PointingHand);
    h.run_steps(2);
    assert!(tip_shown(&h), "the link's tip shows over an uncovered page");
    open_unlock_prompt(&mut h);
    h.run_steps(1);
    assert!(!tip_shown(&h), "the link's tip must not show over the Unlock prompt on the frame it opens");
}

#[test]
fn a_click_on_the_frame_a_dialog_opens_places_no_stamp() {
    let mut h = open(link_pdf(), "150", false);
    stamp_tool(&mut h);
    let p = at(&h, 150.0, 150.0);
    assert_eq!(cursor_at(&mut h, p), egui::CursorIcon::Crosshair);
    open_unlock_prompt(&mut h);
    let button = |pressed| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    one_frame(&mut h, [button(true), button(false)]);
    h.run_steps(4);
    assert_eq!(stamps(&h), 0, "a click in the frame the Unlock prompt opens places no stamp under it");
}

#[test]
fn a_double_click_on_a_dialog_backdrop_leaves_page_text_alone() {
    let mut h = open(text_pdf(), "150", false);
    let word = at(&h, 50.0, 303.0);
    // The I-beam shows the page's text layer has arrived.
    assert_eq!(wait_for_cursor(&mut h, word, |c| c == egui::CursorIcon::Text), egui::CursorIcon::Text, "the page text is live");
    // Control: without a dialog, a double-click there selects the word.
    double_click(&mut h, word);
    h.run_steps(4);
    assert!(h.state().views[0].selected_text().is_some_and(|t| !t.is_empty()), "control: a double-click selects the word");
    // A click on blank page clears it again.
    h.run_steps(30);
    let p = at(&h, 150.0, 150.0);
    mouse_click(&mut h, p);
    h.run_steps(30);
    assert!(h.state().views[0].selected_text().is_none_or(|t| t.is_empty()), "cleared before the dialog opens");
    h.state_mut().set_option("dialog", "shortcuts").unwrap();
    h.run_steps(6);
    assert!(h.state().dialog.is_some(), "a modal dialog is open");
    // On the dialog's backdrop, where the page's first word lies underneath.
    double_click(&mut h, word);
    h.run_steps(4);
    let selected = h.state().views[0].selected_text();
    assert!(selected.as_deref().is_none_or(str::is_empty), "a double-click on a dialog selects nothing on the page under it: {selected:?}");
}

// ---------------------------------------------------------------------------------------------
// Taps, touch drags and clicks within one frame. On the frame a touch (or a Windows pen) lifts,
// egui-winit has sent `PointerGone`, so there is no hover position; egui still reports the click.

#[test]
fn mouse_click_follows_a_link() {
    // Control for the tap below.
    let mut h = open(link_pdf(), "150", false);
    let p = at(&h, 150.0, 325.0);
    mouse_click(&mut h, p);
    assert!(h.state().pending_link.is_some(), "a click on a link asks to open it");
}

#[test]
fn touch_tap_follows_a_link() {
    let mut h = open(link_pdf(), "150", false);
    let p = at(&h, 150.0, 325.0);
    touch_tap(&mut h, p);
    assert!(h.state().pending_link.is_some(), "a tap on a link asks to open it, as a click does");
}

#[test]
fn coalesced_click_follows_a_link() {
    let mut h = open(link_pdf(), "150", false);
    let p = at(&h, 150.0, 325.0);
    coalesced_click(&mut h, p);
    assert!(h.state().pending_link.is_some(), "a click whose press and release share a frame follows the link");
}

#[test]
fn touch_tap_places_a_stamp() {
    let mut h = open(link_pdf(), "150", false);
    stamp_tool(&mut h);
    let p = at(&h, 150.0, 150.0);
    touch_tap(&mut h, p);
    assert_eq!(stamps(&h), 1, "a tap with the stamp tool places the stamp");
}

#[test]
fn coalesced_click_places_a_stamp() {
    let mut h = open(link_pdf(), "150", false);
    stamp_tool(&mut h);
    let p = at(&h, 150.0, 150.0);
    coalesced_click(&mut h, p);
    assert_eq!(stamps(&h), 1, "a click whose press and release share a frame places the stamp");
}

#[test]
fn mouse_click_ticks_a_check_box() {
    // Control for the tap below.
    let mut h = open(checkbox_pdf(), "150", false);
    let p = at(&h, 110.0, 210.0);
    mouse_click(&mut h, p);
    assert_eq!(agree(&h), ["Yes"], "a click ticks the box");
}

#[test]
fn touch_tap_ticks_a_check_box() {
    let mut h = open(checkbox_pdf(), "150", false);
    let p = at(&h, 110.0, 210.0);
    touch_tap(&mut h, p);
    assert_eq!(agree(&h), ["Yes"], "a tap ticks the box, as a click does");
}

#[test]
fn touch_drag_moves_a_comment_and_leaves_no_gesture_behind() {
    let mut h = open(comment_on_text_pdf(), "150", false);
    let k = page(&h).width() / 300.0;
    let before = square(&h);
    // A finger drags the rectangle 30 pt right, from a blank spot inside it.
    let from = at(&h, 120.0, 315.0);
    touch_drag(&mut h, from, from + egui::vec2(30.0 * k, 0.0));
    let after_touch = square(&h);
    assert_ne!(after_touch, before, "the touch drag moves the rectangle");
    // Later, a mouse drag on empty page far below it.
    let p = at(&h, 150.0, 100.0);
    mouse_drag(&mut h, p, egui::vec2(10.0 * k, 10.0 * k));
    assert_eq!(square(&h), after_touch, "an unrelated drag elsewhere leaves the rectangle where the touch put it");
}

// ---------------------------------------------------------------------------------------------
// Ending a gesture.

#[test]
#[ignore = "step 4 of the page pointer router: press-time capture and cancel"]
fn escape_cancels_a_signature_rectangle() {
    let mut h = open(link_pdf(), "150", false);
    h.state_mut().quick_tool = QuickTool::SignArea { certify: false };
    h.run_steps(4);
    let (a, b) = (at(&h, 60.0, 200.0), at(&h, 200.0, 120.0));
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    h.hover_at(b);
    h.run_steps(2);
    // Escape while the button is still held.
    h.key_press(egui::Key::Escape);
    h.run_steps(4);
    let s = h.state();
    assert_eq!(s.quick_tool, QuickTool::SignArea { certify: false }, "Escape abandons the rectangle; the tool stays");
    assert!(s.dialog.is_none(), "and no signing dialog opens: {:?}", s.dialog);
}
