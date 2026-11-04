//! What the page's tools see of the pointer. A finger tap, a touch drag and a click whose press
//! and release share a frame act as a mouse click and drag do, and nothing drawn over the page (a
//! dialog, the palette, the quick bar, the chrome over a page scrolled under it) lets the page's
//! cursor, tips or presses through.
//!
//! The ignored tests pin live bugs that later steps of the page pointer router fix: one hit test
//! (step 3), and press-time capture with cancel (step 4).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::StampKind;
use pdfcraft_ui_egui::comments::CommentTool;
use pdfcraft_ui_egui::fill_sign::FillTool;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp, QuickTool, UnlockPrompt};

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

/// A 300×400 page with a picture at x 50..250, y 250..350 and a web link over its middle, at
/// x 100..200, y 280..320.
fn linked_picture_pdf() -> Vec<u8> {
    let picture =
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\nA\nendstream";
    let content = stream("q 200 0 0 100 50 250 cm /Im0 Do Q");
    pdf(&[
        CATALOG,
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /XObject << /Im0 6 0 R >> >> /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Link /Rect [100 280 200 320] /Border [0 0 0] /A << /S /URI /URI (https://example.org/) >> >>",
        content.as_str(),
        picture,
    ])
}

/// Two 300×400 pages, the first with a line of text at its bottom edge, at y 4, the second with one
/// near its top, at y 380.
fn bottom_line_pages_pdf() -> Vec<u8> {
    let bottom = stream("BT /F1 12 Tf 40 4 Td (Bottom line) Tj ET");
    let top = stream("BT /F1 12 Tf 40 380 Td (Top line) Tj ET");
    pdf(&[
        CATALOG,
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 7 0 R >> >> >>",
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font << /F1 7 0 R >> >> >>",
        bottom.as_str(),
        top.as_str(),
        HELVETICA,
    ])
}

/// A 300×400 page with a picture at x 50..250, y 250..350, and graphics states nested deeper than
/// the objects selected together are read through: they can't be selected together on this page.
fn ungroupable_picture_pdf() -> Vec<u8> {
    let picture =
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\nA\nendstream";
    let content = stream(&format!("q 200 0 0 100 50 250 cm /Im0 Do Q {}{}", "q ".repeat(1030), "Q ".repeat(1030)));
    pdf(&[CATALOG, PAGES, "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Im0 5 0 R >> >> >>", content.as_str(), picture])
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

/// A 300×400 page with two rectangle comments overlapping at x 100..150, y 200..250: "Bottom
/// note" first in `/Annots`, then "Top note" over it.
fn stacked_comments_pdf() -> Vec<u8> {
    pdf(&[
        CATALOG,
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
        "<< /Type /Annot /Subtype /Square /Rect [50 150 150 250] /C [1 0 0] /F 4 /T (Ana) /Contents (Bottom note) >>",
        "<< /Type /Annot /Subtype /Square /Rect [100 200 200 300] /C [0 0 1] /F 4 /T (Ben) /Contents (Top note) >>",
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

/// A 300×400 page with two unchecked check boxes side by side: `agree` at x 100..120 and
/// `other` at x 120..140, both at y 200..220.
fn two_checkboxes_pdf() -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] /DA (/ZaDb 0 Tf 0 g) /DR << /Font << /ZaDb 6 0 R >> >> >> >>",
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /V /Off /AS /Off /Rect [100 200 120 220] /P 3 0 R /F 4 /MK << /CA (4) >> /DA (/ZaDb 11 Tf 0 g) >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (other) /V /Off /AS /Off /Rect [120 200 140 220] /P 3 0 R /F 4 /MK << /CA (4) >> /DA (/ZaDb 11 Tf 0 g) >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /ZapfDingbats >>",
    ])
}

/// The check box `agree` of [`checkbox_pdf`], with a highlight drawn over it.
fn highlighted_checkbox_pdf() -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /DA (/ZaDb 0 Tf 0 g) /DR << /Font << /ZaDb 5 0 R >> >> >> >>",
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 6 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /V /Off /AS /Off /Rect [100 200 120 220] /P 3 0 R /F 4 /MK << /CA (4) >> /DA (/ZaDb 11 Tf 0 g) >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /ZapfDingbats >>",
        "<< /Type /Annot /Subtype /Highlight /Rect [95 195 125 225] /QuadPoints [95 225 125 225 95 195 125 195] /C [1 1 0] /F 4 /T (Ana) >>",
    ])
}

/// A 300×400 page with a push button at x 100..200, y 200..220 whose action opens a web address.
fn button_pdf() -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) >> >>",
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (web) /Rect [100 200 200 220] /P 3 0 R /A << /S /URI /URI (https://example.org/) >> >>",
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

/// Whether the page's hover tip shows and says `text`.
fn tip_says(h: &Harness<'_, PdfCraftApp>, text: &str) -> bool {
    let Some(tip) = h.ctx.memory(|m| m.area_rect(egui::Id::unique("canvas-hover"))).filter(|_| tip_shown(h)) else { return false };
    h.query_all_by_label_contains(text).any(|n| tip.contains(n.rect().center()))
}

/// Hovers `p` until `want` accepts the cursor there (text layers and paragraph boxes arrive from
/// worker threads, `support::step_until`), and returns the last cursor seen.
fn wait_for_cursor(h: &mut Harness<'static, PdfCraftApp>, p: egui::Pos2, want: impl Fn(egui::CursorIcon) -> bool) -> egui::CursorIcon {
    let mut cursor = egui::CursorIcon::Default;
    let _ = crate::support::step_until(h, |h| {
        cursor = cursor_at(h, p);
        want(cursor)
    });
    cursor
}

// ---------------------------------------------------------------------------------------------
// The cursor promises one thing and a press does another.

#[test]
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

#[test]
fn a_links_tip_shows_only_with_its_hand() {
    let mut h = open(link_pdf(), "150", false);
    let on_link = at(&h, 150.0, 325.0);
    let hover = |h: &mut Harness<'_, PdfCraftApp>, tool: QuickTool, nudge: f32| {
        h.state_mut().quick_tool = tool;
        h.run_steps(4);
        // A new position each time, so the hover is fresh.
        let cursor = cursor_at(h, on_link + egui::vec2(nudge, 0.0));
        h.run_steps(2);
        (cursor, tip_says(h, "https://example.org/"))
    };
    use egui::CursorIcon::{Crosshair, PointingHand};
    assert_eq!(hover(&mut h, QuickTool::Select, 0.0), (PointingHand, true), "the Select tool follows the link, and the tip says where to");
    assert_eq!(hover(&mut h, QuickTool::Link, 1.0), (PointingHand, true), "the Link tool edits it");
    let stamp = QuickTool::Stamp(StampKind::DynApproved);
    assert_eq!(hover(&mut h, stamp, 2.0), (Crosshair, false), "a click places a stamp, so neither hand nor tip offers the link");
    let rectangle = QuickTool::Comment(CommentTool::Rectangle);
    assert_eq!(hover(&mut h, rectangle, 3.0), (Crosshair, false), "a drag draws a rectangle");
}

/// A drag from plain text onto a link still selects the text: the press decides, not where the
/// pointer has got to once egui calls it a drag.
#[test]
fn a_drag_from_text_onto_a_link_selects_the_text() {
    let mut h = open(linked_text_pdf(), "150", false);
    // "before you start." lies right of the link, which covers x 40..160.
    let (plain, linked) = (at(&h, 200.0, 303.0), at(&h, 100.0, 303.0));
    assert_eq!(wait_for_cursor(&mut h, plain, |c| c == egui::CursorIcon::Text), egui::CursorIcon::Text, "the page's text has arrived");
    mouse_drag(&mut h, plain, linked - plain);
    let selected = h.state().views[0].selected_text().unwrap_or_default();
    assert!(selected.contains("example.org"), "the drag selected across the link: {selected:?}");
}

/// With the Select tool a click on a link over a picture follows the link and picks no picture; one
/// on the picture beside the link picks it. The link's click used to pick the picture under it too.
#[test]
fn a_click_on_a_link_over_a_picture_follows_it_and_picks_no_picture() {
    let mut h = open(linked_picture_pdf(), "150", false);
    let beside = at(&h, 70.0, 300.0);
    mouse_click(&mut h, beside);
    assert!(h.state().views[0].picked_image.is_some(), "the picture beside the link is picked");
    h.state_mut().views[0].picked_image = None;
    let on_link = at(&h, 150.0, 300.0);
    mouse_click(&mut h, on_link);
    assert_eq!(pending_url(&h).as_deref(), Some("https://example.org/"), "the link is followed");
    assert!(h.state().views[0].picked_image.is_none(), "and no picture is picked");
}

/// The I-beam drops over a link only where a press would go to it: with the Select tool the link's
/// hand shows there, and with the highlighter, whose drag selects the link's text, the I-beam. The
/// highlighter's I-beam used to drop over every link.
#[test]
fn the_i_beam_drops_over_a_link_only_where_the_press_goes_to_it() {
    let mut h = open(linked_text_pdf(), "150", false);
    let (plain, linked) = (at(&h, 200.0, 303.0), at(&h, 100.0, 303.0));
    assert_eq!(wait_for_cursor(&mut h, plain, |c| c == egui::CursorIcon::Text), egui::CursorIcon::Text, "the page's text has arrived");
    assert_eq!(cursor_at(&mut h, linked), egui::CursorIcon::PointingHand, "the Select tool follows the link");
    h.state_mut().quick_tool = QuickTool::Comment(CommentTool::Highlight);
    h.run_steps(2);
    assert_eq!(cursor_at(&mut h, linked), egui::CursorIcon::Text, "the highlighter selects its text");
}

/// Over two comments the tip names the one drawn on top (#3620), and a click should select that
/// one too. The renderer paints in `/Annots` order, but the hit test takes the last comment in
/// inspection's order, which is by top edge, and selects the one underneath.
#[test]
fn a_click_on_stacked_comments_selects_the_one_drawn_on_top() {
    let mut h = open(stacked_comments_pdf(), "150", false);
    let overlap = at(&h, 125.0, 225.0);
    assert_eq!(cursor_at(&mut h, overlap), egui::CursorIcon::Move);
    h.run_steps(2);
    assert!(tip_says(&h, "Top note") && !tip_says(&h, "Bottom note"), "the tip names the comment drawn on top");
    mouse_click(&mut h, overlap);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 1)), "a click selects it too");
}

#[test]
fn a_comments_tip_shows_with_every_tool_beside_its_cursor() {
    let mut h = open(comment_on_text_pdf(), "150", false);
    // Inside the rectangle comment by "Reviewer": where there is no text, and over a word.
    let (blank, on_text) = (at(&h, 120.0, 315.0), at(&h, 60.0, 303.0));
    let hover = |h: &mut Harness<'static, PdfCraftApp>, tool: QuickTool, p: egui::Pos2, want: egui::CursorIcon| {
        h.state_mut().quick_tool = tool;
        h.run_steps(4);
        let cursor = wait_for_cursor(h, p, |c| c == want);
        h.run_steps(2);
        (cursor, tip_says(h, "Reviewer"))
    };
    use egui::CursorIcon::{Crosshair, Move, Text};
    // A new position each time, so the hover is fresh.
    let nudge = |n: f32| blank + egui::vec2(n, 0.0);
    assert_eq!(hover(&mut h, QuickTool::Select, blank, Move), (Move, true), "the Select tool picks the comment up");
    let stamp = QuickTool::Stamp(StampKind::DynApproved);
    assert_eq!(hover(&mut h, stamp, nudge(1.0), Crosshair), (Crosshair, true), "a stamp keeps its crosshair, and the comment still says whose it is");
    let rectangle = QuickTool::Comment(CommentTool::Rectangle);
    assert_eq!(hover(&mut h, rectangle, nudge(2.0), Crosshair), (Crosshair, true), "so does a drawing tool");
    let highlight = QuickTool::Comment(CommentTool::Highlight);
    assert_eq!(hover(&mut h, highlight, on_text, Text), (Text, true), "and the highlighter its I-beam");
    // Not while drawing freehand, where it would cover the next stroke (#429), nor with Fill & Sign.
    let ink = QuickTool::Comment(CommentTool::Ink);
    assert_eq!(hover(&mut h, ink, nudge(3.0), Crosshair), (Crosshair, false), "freehand");
    let fill = QuickTool::Fill(FillTool::Check);
    assert_eq!(hover(&mut h, fill, nudge(4.0), Crosshair), (Crosshair, false), "Fill & Sign");
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

/// Check that the command palette's frame keeps the default cursor over the page while the page beside it
/// still shows the stamp tool.
#[test]
fn stamp_cursor_stays_off_the_command_palette() {
    let mut h = open(link_pdf(), "300", false);
    stamp_tool(&mut h);
    h.state_mut().palette_open = true;
    h.run_steps(4);
    let palette = h.ctx.memory(|m| m.area_rect(egui::Id::unique("palette"))).expect("palette shown");
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
    let palette = h.ctx.memory(|m| m.area_rect(egui::Id::unique("palette"))).expect("palette shown");
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

/// A panel's resize edge dragged over a link shows neither the link's tip nor its cursor, and the
/// link answers again once the edge is let go.
#[test]
fn a_dragged_panel_edge_passes_over_a_link_without_its_tip() {
    use egui::accesskit::Role;
    use egui_kittest::kittest::Queryable;
    let mut h = open(link_pdf(), "150", false);
    h.state_mut().set_option("panel", "comments").unwrap();
    for _ in 0..4 {
        h.run_steps(2);
    }
    let on_link = at(&h, 100.0, 340.0);
    assert_eq!(cursor_at(&mut h, on_link), egui::CursorIcon::PointingHand);
    h.run_steps(2);
    assert!(tip_shown(&h), "the link's tip shows over an uncovered page");
    // Away from the link, then the side panel's resize edge (the rightmost one) is picked up and
    // carried over it.
    h.hover_at(at(&h, 150.0, 100.0));
    h.run_steps(4);
    let tab = h
        .get_all_by(|n| n.role() == Role::Splitter && n.label().as_deref() == Some("Resize panels"))
        .map(|n| n.rect().center())
        .max_by(|a, b| a.x.total_cmp(&b.x))
        .expect("the side panel has a resize edge")
        // The pane's half of the edge: the page, drawn after the panes, takes the other half.
        + egui::vec2(2.0, 0.0);
    h.hover_at(tab);
    h.run_steps(1);
    h.drag_at(tab);
    h.run_steps(1);
    for i in 1..=8 {
        h.hover_at(tab + (on_link - tab) * (i as f32 / 8.0));
        h.run_steps(1);
    }
    for _ in 0..4 {
        h.run_steps(2);
        assert!(h.ctx.dragged_id().is_some(), "the edge is still being dragged");
        assert!(!tip_shown(&h), "no link tip under a dragged panel edge");
        assert_ne!(h.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand);
    }
    // Back to the start and let go there, which leaves the width as it was.
    for i in 1..=8 {
        h.hover_at(on_link + (tab - on_link) * (i as f32 / 8.0));
        h.run_steps(1);
    }
    h.drop_at(tab);
    h.run_steps(2);
    assert!(h.ctx.dragged_id().is_none());
    assert_eq!(cursor_at(&mut h, on_link), egui::CursorIcon::PointingHand, "the link answers again");
    h.run_steps(2);
    assert!(tip_shown(&h), "and shows its tip");
}

/// Send one wheel event over the page and let its scrolling settle.
fn wheel(h: &mut Harness<'_, PdfCraftApp>, delta: egui::Vec2) {
    h.hover_at(egui::pos2(800.0, 500.0));
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta,
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
    });
    for _ in 0..10 {
        h.run_steps(2);
    }
}

/// Check that the quick bar's frame keeps the default cursor over the page while the page below it still
/// shows the stamp tool.
#[test]
fn stamp_cursor_stays_off_the_quick_bar() {
    let mut h = open(link_pdf(), "400", true);
    stamp_tool(&mut h);
    // Bring the page under the quick bar's left end.
    wheel(&mut h, egui::vec2(-200.0, 0.0));
    let bar = h.ctx.memory(|m| m.area_rect(egui::Id::unique("quick-bar"))).expect("quick bar shown");
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
    let tip = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("canvas-hover"));
    h.ctx.memory(|m| m.areas().is_visible(&tip))
}

/// Opens the Unlock prompt, a dialog with a backdrop over the whole window, from the next frame.
fn open_unlock_prompt(h: &mut Harness<'_, PdfCraftApp>) {
    let doc = h.state().views[0].id;
    h.state_mut().unlock_prompt = Some(UnlockPrompt { doc, name: "test.pdf".into(), input: String::new(), error: None, then: None });
}

/// Check that the link tip and pointing cursor disappear under the Unlock prompt.
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
    let card = h.ctx.memory(|m| m.area_rect(egui::Id::unique("unlock")));
    assert!(card.is_none_or(|c| !c.contains(on_link)), "the point must be on the backdrop, not the card: {card:?}");
    assert!(!tip_shown(&h), "the link's tip must not float over the Unlock prompt");
    assert_ne!(h.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand);
}

/// Check that moving onto a link's floating tip still reports the link's pointing-hand cursor.
#[test]
fn the_link_tip_lets_the_pointer_through_to_the_link() {
    let mut h = open(link_pdf(), "150", false);
    let p = at(&h, 100.0, 340.0);
    assert_eq!(cursor_at(&mut h, p), egui::CursorIcon::PointingHand);
    let tip = h.ctx.memory(|m| m.area_rect(egui::Id::unique("canvas-hover"))).expect("the link's tip");
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

/// The value of the field named `name` on the first view's document.
fn field_value(h: &Harness<'_, PdfCraftApp>, name: &str) -> Vec<String> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == name).unwrap().value.clone()
}

/// A press belongs to the field it lands on: pressed at the right edge of `agree` and released just
/// over the edge of `other` beside it (a click still, to egui), it ticks neither, as a button
/// pressed and released off itself does nothing. The release used to tick `other`.
#[test]
fn a_press_on_one_check_box_released_over_another_ticks_neither() {
    let mut h = open(two_checkboxes_pdf(), "150", false);
    let (press, release) = (at(&h, 119.0, 210.0), at(&h, 121.0, 210.0));
    slipping_click(&mut h, press, release);
    assert_eq!((field_value(&h, "agree"), field_value(&h, "other")), (Vec::<String>::new(), Vec::new()), "neither box is ticked");
}

/// A click whose pointer slips from `press` to `release` before the button comes up (a click
/// still, to egui, while they are close), in frames of their own as [`mouse_click`] sends them.
fn slipping_click(h: &mut Harness<'_, PdfCraftApp>, press: egui::Pos2, release: egui::Pos2) {
    h.hover_at(press);
    h.run_steps(1);
    h.drag_at(press);
    h.run_steps(1);
    h.drop_at(release);
    h.run_steps(4);
}

/// A click follows the link its press was on only when released over it: pressed just inside the
/// link's edge and released just past it, it follows nothing, as a button pressed and released off
/// itself does nothing. The press used to be enough.
#[test]
fn a_press_on_a_link_released_off_it_follows_nothing() {
    let mut h = open(link_pdf(), "150", false);
    // The link ends at x 250.
    let press = at(&h, 249.0, 325.0);
    slipping_click(&mut h, press, press + egui::vec2(5.0, 0.0));
    assert_eq!(h.state().pending_link, None, "nothing asks to open the link");
    mouse_click(&mut h, press);
    assert!(h.state().pending_link.is_some(), "a click there asks to open it");
}

/// The same for a push button: pressed just inside its edge and released just past it, it runs no
/// action. The press used to be enough.
#[test]
fn a_press_on_a_button_released_off_it_runs_nothing() {
    let mut h = open(button_pdf(), "150", false);
    // The button ends at x 200.
    let press = at(&h, 199.0, 210.0);
    slipping_click(&mut h, press, press + egui::vec2(5.0, 0.0));
    assert_eq!(h.state().pending_link, None, "the button's action does not run");
    mouse_click(&mut h, press);
    assert!(h.state().pending_link.is_some(), "a click there runs it");
}

/// Text markup lies on the text, under a form field: a click on a check box under a highlight
/// ticks the box, and the field's hand shows there.
#[test]
fn a_check_box_under_a_highlight_takes_the_click() {
    let mut h = open(highlighted_checkbox_pdf(), "150", false);
    let p = at(&h, 110.0, 210.0);
    assert_eq!(cursor_at(&mut h, p), egui::CursorIcon::PointingHand);
    mouse_click(&mut h, p);
    assert_eq!(field_value(&h, "agree"), ["Yes"], "the box is ticked");
    assert_eq!(h.state().views[0].comments.selected, None, "and the highlight is not selected");
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

/// A highlight dragged over the line of [`text_pdf`] at 150 %, from x 45 to x 150, with Escape
/// before the release when `escape`; the comments then on the page.
fn drag_a_highlight(escape: bool) -> usize {
    let mut h = open(text_pdf(), "150", false);
    h.state_mut().quick_tool = QuickTool::Comment(CommentTool::Highlight);
    h.run_steps(4);
    let (a, b) = (at(&h, 45.0, 303.0), at(&h, 150.0, 303.0));
    assert_eq!(wait_for_cursor(&mut h, a, |c| c == egui::CursorIcon::Text), egui::CursorIcon::Text, "the page's text has arrived");
    h.drag_at(a);
    h.run_steps(1);
    h.hover_at(b);
    h.run_steps(2);
    if escape {
        h.key_press(egui::Key::Escape);
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(4);
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().info.annotations.len()
}

/// Escape mid-drag abandons a highlight being drawn over text: the release makes no highlight.
#[test]
fn escape_cancels_a_highlight_being_dragged_over_text() {
    assert_eq!(drag_a_highlight(false), 1, "control: the drag highlights the text");
    assert_eq!(drag_a_highlight(true), 0, "with Escape before the release, no highlight is made");
}

/// A click on a comment, and in the frame of its release a press beside the page, then a drag
/// from there: the drag is not the comment's, so it does not move it.
#[test]
fn a_drag_from_beside_the_page_right_after_a_click_leaves_the_comment_alone() {
    let mut h = open(comment_on_text_pdf(), "150", false);
    let on_comment = at(&h, 120.0, 315.0);
    let beside = page(&h).left_center() - egui::vec2(20.0, 0.0);
    let before = square(&h).expect("the rectangle");
    h.hover_at(on_comment);
    h.run_steps(1);
    h.drag_at(on_comment);
    h.run_steps(1);
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    one_frame(&mut h, [button(on_comment, false), egui::Event::PointerMoved(beside), button(beside, true)]);
    h.run_steps(1);
    h.hover_at(beside + egui::vec2(0.0, 60.0));
    h.run_steps(2);
    h.drop_at(beside + egui::vec2(0.0, 60.0));
    h.run_steps(4);
    assert_eq!(square(&h), Some(before), "the comment stays where it was");
}

/// Only the primary button draws a signature rectangle: a right-drag opens no signing dialog.
#[test]
fn a_right_drag_draws_no_signature_rectangle() {
    let mut h = open(link_pdf(), "150", false);
    h.state_mut().quick_tool = QuickTool::SignArea { certify: false };
    h.run_steps(4);
    let (a, b) = (at(&h, 60.0, 200.0), at(&h, 200.0, 120.0));
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE };
    h.hover_at(a);
    h.run_steps(1);
    h.event(button(a, true));
    h.run_steps(1);
    h.hover_at(b);
    h.run_steps(2);
    h.event(button(b, false));
    h.run_steps(4);
    let s = h.state();
    assert_eq!(s.quick_tool, QuickTool::SignArea { certify: false }, "the tool stays: no rectangle was drawn");
    assert!(s.dialog.is_none(), "and no signing dialog opens: {:?}", s.dialog);
}

/// Escape with the button down drops the press: its release places no stamp, and the next click
/// places one.
#[test]
fn escape_mid_press_keeps_its_release_from_placing_a_stamp() {
    let mut h = open(link_pdf(), "150", false);
    stamp_tool(&mut h);
    let p = at(&h, 150.0, 150.0);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.key_press(egui::Key::Escape);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(4);
    assert_eq!(stamps(&h), 0, "the cancelled press places nothing on release");
    mouse_click(&mut h, p);
    assert_eq!(stamps(&h), 1, "the next click places a stamp");
}

/// Escape mid-drag abandons a crop rectangle: the page keeps its boxes.
#[test]
fn escape_cancels_a_crop_rectangle() {
    let mut h = open(text_pdf(), "150", false);
    h.state_mut().quick_tool = QuickTool::Crop;
    h.run_steps(4);
    let (a, b) = (at(&h, 40.0, 360.0), at(&h, 260.0, 60.0));
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    h.hover_at(b);
    h.run_steps(2);
    h.key_press(egui::Key::Escape);
    h.run_steps(1);
    h.drop_at(b);
    h.run_steps(4);
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), None, "no page box was set");
}

// ---------------------------------------------------------------------------------------------
// One link test: what the Select tool follows, the Link tool edits and a right-click opens is
// the same link.

/// A 300×400 page with two web links overlapping at x 150..200, y 300..350: `first` first in
/// `/Annots`, then `second` drawn over it.
fn stacked_links_pdf() -> Vec<u8> {
    pdf(&[
        CATALOG,
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
        "<< /Type /Annot /Subtype /Link /Rect [50 300 200 350] /Border [0 0 0] /A << /S /URI /URI (https://example.org/first) >> >>",
        "<< /Type /Annot /Subtype /Link /Rect [150 300 250 350] /Border [0 0 0] /A << /S /URI /URI (https://example.org/second) >> >>",
    ])
}

/// A 300×400 page with a link that does nothing (no action) at x 10..40, y 300..350, then a web
/// link at x 50..250 beside it: second in `/Annots`, and the first the renderer can follow.
fn dead_link_pdf() -> Vec<u8> {
    pdf(&[
        CATALOG,
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
        "<< /Type /Annot /Subtype /Link /Rect [10 300 40 350] /Border [0 0 0] >>",
        "<< /Type /Annot /Subtype /Link /Rect [50 300 250 350] /Border [0 0 0] /A << /S /URI /URI (https://example.org/) >> >>",
    ])
}

/// [`link_pdf`], with a rectangle comment drawn over the middle of its link.
fn note_over_link_pdf() -> Vec<u8> {
    pdf(&[
        CATALOG,
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
        "<< /Type /Annot /Subtype /Link /Rect [50 300 250 350] /Border [0 0 0] /A << /S /URI /URI (https://example.org/) >> >>",
        "<< /Type /Annot /Subtype /Square /Rect [100 310 200 340] /C [1 0 0] /F 4 /T (Ana) /Contents (A note) >>",
    ])
}

fn link_tool(h: &mut Harness<'_, PdfCraftApp>) {
    h.state_mut().quick_tool = QuickTool::Link;
    h.run_steps(4);
}

/// A right-click at `p`: press and release in frames of their own.
fn right_click(h: &mut Harness<'_, PdfCraftApp>, p: egui::Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE });
    }
    h.run_steps(3);
}

fn pending_url(h: &Harness<'_, PdfCraftApp>) -> Option<String> {
    h.state().pending_link.as_ref().map(|l| l.url.clone())
}

/// Of two stacked links the Link tool selects the one drawn on top, the one the Select tool
/// follows. It used to select the one underneath.
#[test]
fn the_link_tool_selects_the_topmost_of_stacked_links() {
    let mut h = open(stacked_links_pdf(), "150", false);
    let both = at(&h, 175.0, 325.0);
    mouse_click(&mut h, both);
    assert_eq!(pending_url(&h).as_deref(), Some("https://example.org/second"), "the Select tool follows the link on top");
    h.state_mut().pending_link = None;
    link_tool(&mut h);
    mouse_click(&mut h, both);
    assert_eq!(h.state().views[0].links.selected, Some((0, 1)), "the Link tool selects it too");
}

/// The Link tool edits every link, one that does nothing included. The Select tool, which
/// follows links, shows no hand over it, and follows the link beside it by its own `/Annots`
/// place, not by its place among the links the renderer can follow.
#[test]
fn the_link_tool_selects_a_link_with_no_action() {
    let mut h = open(dead_link_pdf(), "150", false);
    let dead = at(&h, 25.0, 325.0);
    assert_ne!(cursor_at(&mut h, dead), egui::CursorIcon::PointingHand, "the Select tool has nothing to follow there");
    let web = at(&h, 150.0, 325.0);
    mouse_click(&mut h, web);
    assert_eq!(pending_url(&h).as_deref(), Some("https://example.org/"), "the web link is followed");
    h.state_mut().pending_link = None;
    link_tool(&mut h);
    assert_eq!(cursor_at(&mut h, dead), egui::CursorIcon::PointingHand);
    mouse_click(&mut h, dead);
    assert_eq!(h.state().views[0].links.selected, Some((0, 0)), "the Link tool selects the link that does nothing");
}

/// With the Link tool a drag that starts on a link draws a new link: the press is the link's, its
/// drag is not.
#[test]
fn a_drag_with_the_link_tool_from_a_link_draws_a_new_link() {
    let mut h = open(link_pdf(), "150", false);
    link_tool(&mut h);
    let on_link = at(&h, 150.0, 325.0);
    mouse_drag(&mut h, on_link, egui::vec2(80.0, 120.0));
    let s = h.state();
    assert_eq!(s.dialog, Some(Dialog::LinkProps));
    assert_eq!(s.link_draft.as_ref().map(|d| d.index), Some(None), "Link Properties opens for a new link");
}

/// With the Select tool a right-click finds a link only where a click follows it: just beside the
/// link it opens the page's menu. The Link tool's margin around links used to reach there.
#[test]
fn a_right_click_beside_a_link_with_the_select_tool_opens_no_link_menu() {
    let mut h = open(link_pdf(), "150", false);
    // The link ends at x 250.
    let beside = at(&h, 250.0, 325.0) + egui::vec2(1.5, 0.0);
    right_click(&mut h, beside);
    assert_eq!(h.state().views[0].links.context, None, "no link under the right-click");
    assert!(h.query_by_label("Remove all links").is_none(), "the page's menu has no link items");
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    let on_link = at(&h, 245.0, 325.0);
    right_click(&mut h, on_link);
    assert_eq!(h.state().views[0].links.context, Some((0, 0)), "on the link, its menu");
}

/// A right-click goes to what a press there would go to: on a note over a link, the note. It used
/// to open the link's menu.
#[test]
fn a_right_click_on_a_note_over_a_link_opens_the_notes_menu() {
    let mut h = open(note_over_link_pdf(), "150", false);
    let on_note = at(&h, 150.0, 325.0);
    right_click(&mut h, on_note);
    let v = &h.state().views[0];
    assert_eq!((v.links.context, v.comments.selected), (None, Some((0, 1))), "the note is selected, not the link");
    assert!(h.query_by_label("Remove all links").is_none(), "the menu is the note's");
}

/// [`link_pdf`], with a highlight drawn over the middle of its link.
fn highlight_over_link_pdf() -> Vec<u8> {
    pdf(&[
        CATALOG,
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
        "<< /Type /Annot /Subtype /Link /Rect [50 300 250 350] /Border [0 0 0] /A << /S /URI /URI (https://example.org/) >> >>",
        "<< /Type /Annot /Subtype /Highlight /Rect [100 310 200 340] /QuadPoints [100 340 200 340 100 310 200 310] /C [1 1 0] /F 4 /T (Ana) >>",
    ])
}

/// A click on a highlight over a link follows the link, but a right-click there is the
/// highlight's: it is selected and its menu opens. The link's menu used to open, with the
/// highlight selected underneath.
#[test]
fn a_right_click_on_a_highlight_over_a_link_opens_the_highlights_menu() {
    let mut h = open(highlight_over_link_pdf(), "150", false);
    let on_both = at(&h, 150.0, 325.0);
    right_click(&mut h, on_both);
    let v = &h.state().views[0];
    assert_eq!((v.links.context, v.comments.selected), (None, Some((0, 1))), "the highlight is selected, not the link");
    assert!(h.query_by_label("Remove all links").is_none(), "the menu is the highlight's");
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    mouse_click(&mut h, on_both);
    assert!(h.state().pending_link.is_some(), "a click there still follows the link");
}

/// A comment's menu opens on a right-click whatever the tool, though a press there is the tool's:
/// with the Rectangle tool, a right-click on a comment selects it.
#[test]
fn a_right_click_on_a_comment_with_a_drawing_tool_opens_its_menu() {
    let mut h = open(comment_on_text_pdf(), "150", false);
    h.state_mut().quick_tool = QuickTool::Comment(CommentTool::Rectangle);
    h.run_steps(4);
    let on_comment = at(&h, 100.0, 305.0);
    right_click(&mut h, on_comment);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)), "the comment is selected for its menu");
    assert!(h.query_all_by_label("Reply").next().is_some(), "and its menu is open");
}

/// The check box `agree` of [`checkbox_pdf`], with a rectangle comment at x 150..250, y 100..150.
fn comment_and_checkbox_pdf() -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /DA (/ZaDb 0 Tf 0 g) /DR << /Font << /ZaDb 5 0 R >> >> >> >>",
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 6 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /V /Off /AS /Off /Rect [100 200 120 220] /P 3 0 R /F 4 /MK << /CA (4) >> /DA (/ZaDb 11 Tf 0 g) >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /ZapfDingbats >>",
        "<< /Type /Annot /Subtype /Square /Rect [150 100 250 150] /C [1 0 0] /F 4 /T (Ana) >>",
    ])
}

/// A click on anything but the selected comment leaves it unselected, a form field included, as
/// a click on a link or on the page does. A field's click used to keep it selected.
#[test]
fn a_click_on_a_form_field_deselects_the_selected_comment() {
    let mut h = open(comment_and_checkbox_pdf(), "150", false);
    let comment = at(&h, 200.0, 125.0);
    mouse_click(&mut h, comment);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 1)), "the comment is selected");
    let check_box = at(&h, 110.0, 210.0);
    mouse_click(&mut h, check_box);
    assert_eq!(h.state().views[0].comments.selected, None, "a click on the check box leaves it unselected");
    assert_eq!(agree(&h).len(), 1, "and ticks the box");
}

/// The camera tool's quadrilateral, corner by corner, in page view points.
fn camera_corners(h: &Harness<'_, PdfCraftApp>) -> Option<[[f32; 2]; 4]> {
    h.state().views[0].camera.map(|q| q.corners)
}

/// A corner of the camera tool's quadrilateral is a press of its own: Escape in the middle of its
/// drag cancels it, and once the button is up no corner follows the pointer. Today Escape also
/// leaves the camera tool before the pages see it; this holds whatever the order of the keys.
#[test]
fn escape_mid_corner_drag_leaves_no_corner_following_the_pointer() {
    let mut h = open(text_pdf(), "150", false);
    h.state_mut().quick_tool = QuickTool::EnhanceCamera;
    h.run_steps(4);
    // The left top corner starts 5% in from the page's edges: view (15, 20) on the 300×400 page.
    let corner = at(&h, 15.0, 380.0);
    let start = camera_corners(&h);
    h.hover_at(corner);
    h.run_steps(1);
    h.drag_at(corner);
    h.run_steps(1);
    h.hover_at(corner + egui::vec2(30.0, 30.0));
    h.run_steps(2);
    assert_ne!(camera_corners(&h), start, "the corner follows its drag");
    h.key_press(egui::Key::Escape);
    h.run_steps(1);
    let end = corner + egui::vec2(60.0, 60.0);
    h.hover_at(end);
    h.run_steps(1);
    h.drop_at(end);
    h.run_steps(2);
    let after = camera_corners(&h);
    h.hover_at(corner + egui::vec2(120.0, 90.0));
    h.run_steps(3);
    assert_eq!(camera_corners(&h), after, "no corner follows the pointer once the press is cancelled");
    assert!(h.state().views[0].camera.is_none_or(|q| q.grab.is_none()), "and none is held");
}

/// A 300×400 page with a text field `name` (tooltip "Your name") at x 50..250, y 200..220, and a
/// rectangle comment drawn over its right end, x 150..260, y 190..230.
fn commented_text_field_pdf() -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) >> >>",
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /TU (Your name) /Rect [50 200 250 220] /P 3 0 R /F 4 /DA (/Helv 12 Tf 0 g) >>",
        "<< /Type /Annot /Subtype /Square /Rect [150 190 260 230] /C [1 0 0] /F 4 /T (Ana) >>",
    ])
}

/// Rests the pointer at `p` long enough for a tooltip.
fn rest_at(h: &mut Harness<'_, PdfCraftApp>, p: egui::Pos2) {
    h.hover_at(p);
    for _ in 0..40 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
}

/// A field's tooltip (`/TU`) shows only where a press would take the field: not under a comment
/// drawn over it. It used to show over the whole field.
#[test]
fn a_fields_tooltip_shows_only_where_the_field_takes_the_press() {
    let mut h = open(commented_text_field_pdf(), "150", false);
    let under_comment = at(&h, 200.0, 210.0);
    rest_at(&mut h, under_comment);
    assert!(h.query_by_label("Your name").is_none(), "no field tooltip where the comment takes the press");
    let on_field = at(&h, 100.0, 210.0);
    rest_at(&mut h, on_field);
    assert!(h.query_all_by_label("Your name").next().is_some(), "the field's tooltip where it takes the press");
}

/// With the Link tool a right-click on a link is the link's, as its press there is, though a note
/// lies over it: the Link tool edits links and reaches no comment.
#[test]
fn a_right_click_with_the_link_tool_on_a_note_over_a_link_selects_the_link() {
    let mut h = open(note_over_link_pdf(), "150", false);
    link_tool(&mut h);
    let on_both = at(&h, 150.0, 325.0);
    right_click(&mut h, on_both);
    let v = &h.state().views[0];
    assert_eq!((v.links.selected, v.links.context, v.comments.selected), (Some((0, 0)), Some((0, 0)), None), "the link, not the note");
}

/// With Add text a right-click leaves comments alone, as before: Add text takes the page's
/// right-clicks.
#[test]
fn a_right_click_with_add_text_selects_no_comment() {
    let mut h = open(comment_on_text_pdf(), "150", false);
    h.state_mut().quick_tool = QuickTool::AddText;
    h.run_steps(4);
    // Above the line of text the comment covers, where no paragraph is.
    let on_comment = at(&h, 100.0, 317.0);
    right_click(&mut h, on_comment);
    assert_eq!(h.state().views[0].comments.selected, None, "the comment under the pointer is not selected");
}

/// With the Select tool a right-click finds a link only where a click follows it: a link that
/// does nothing has no menu. The Link tool's own test used to give it one.
#[test]
fn a_right_click_with_the_select_tool_on_a_link_with_no_action_opens_no_link_menu() {
    let mut h = open(dead_link_pdf(), "150", false);
    let dead = at(&h, 25.0, 325.0);
    right_click(&mut h, dead);
    assert_eq!(h.state().views[0].links.context, None);
    assert!(h.query_by_label("Remove all links").is_none(), "the page's menu has no link items");
}

/// [`link_pdf`], with a check box over the middle of its link.
fn field_over_link_pdf() -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] /DA (/ZaDb 0 Tf 0 g) /DR << /Font << /ZaDb 6 0 R >> >> >> >>",
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
        "<< /Type /Annot /Subtype /Link /Rect [50 300 250 350] /Border [0 0 0] /A << /S /URI /URI (https://example.org/) >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /V /Off /AS /Off /Rect [140 315 160 335] /P 3 0 R /F 4 /MK << /CA (4) >> /DA (/ZaDb 11 Tf 0 g) >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /ZapfDingbats >>",
    ])
}

/// A right-click on a form field over a link is the field's, as a press there is: no link menu.
#[test]
fn a_right_click_on_a_field_over_a_link_opens_no_link_menu() {
    let mut h = open(field_over_link_pdf(), "150", false);
    let on_field = at(&h, 150.0, 325.0);
    right_click(&mut h, on_field);
    assert_eq!(h.state().views[0].links.context, None, "the field is on top");
    assert!(h.query_by_label("Remove all links").is_none());
}

/// While the palette is open, a field beside it still shows its tooltip, as before: the palette
/// keeps the page's own tips away, not egui's tooltip of a field.
#[test]
fn a_fields_tooltip_shows_beside_the_open_palette() {
    let mut h = open(commented_text_field_pdf(), "150", false);
    h.state_mut().palette_open = true;
    h.run_steps(4);
    let palette = h.ctx.memory(|m| m.area_rect(egui::Id::unique("palette"))).expect("palette shown");
    let on_field = at(&h, 60.0, 210.0);
    assert!(!palette.expand(20.0).contains(on_field), "the field is beside the palette {palette:?}");
    rest_at(&mut h, on_field);
    assert!(h.query_all_by_label("Your name").next().is_some(), "the field's tooltip shows");
}

/// Two blank 300×400 pages.
fn two_pages_pdf() -> Vec<u8> {
    pdf(&[
        CATALOG,
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R >>",
        "<< /Type /Page /Parent 2 0 R >>",
    ])
}

/// Sets a tool up (`tool`), presses above page 1's bottom edge and drags onto page 2 in one frame,
/// so egui sees the drag only once the pointer is on page 2, and returns the app after the release.
fn drag_seen_only_past_page_ones_edge(tool: impl FnOnce(&mut PdfCraftApp)) -> Harness<'static, PdfCraftApp> {
    let mut h = open(two_pages_pdf(), "100", false);
    tool(h.state_mut());
    h.run_steps(4);
    let (one, two) = {
        let v = &h.state().views[0];
        (v.page_screen_rect(0).expect("page 1 on screen"), v.page_screen_rect(1).expect("page 2 on screen"))
    };
    let press = egui::pos2(one.min.x + 50.0, one.max.y - 30.0);
    let release = egui::pos2(one.min.x + 150.0, two.min.y + 20.0);
    h.hover_at(press);
    h.run_steps(1);
    h.drag_at(press);
    h.run_steps(1);
    h.hover_at(release);
    h.run_steps(1);
    h.drop_at(release);
    h.run_steps(4);
    h
}

// A drag seen only past its page's edge still draws on the page its press began on, as zoom, crop
// and Sign area do. With these three it used to draw nothing anywhere.

#[test]
fn a_redaction_box_dragged_off_its_page_is_drawn_on_it() {
    let h = drag_seen_only_past_page_ones_edge(|app| app.quick_tool = QuickTool::Redact);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert!(doc.info.annotations.iter().any(|a| a.subtype == "Redact" && a.page == 0), "a redaction mark on page 1");
}

#[test]
fn a_link_rectangle_dragged_off_its_page_is_drawn_on_it() {
    let h = drag_seen_only_past_page_ones_edge(|app| app.quick_tool = QuickTool::Link);
    assert_eq!(h.state().link_draft.as_ref().map(|d| (d.page, d.index)), Some((0, None)), "a new link on page 1");
}

#[test]
fn a_field_dragged_off_its_page_is_drawn_on_it() {
    let h = drag_seen_only_past_page_ones_edge(|app| assert!(app.execute("form.add.text"), "Prepare a form, with the text field tool"));
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert!(doc.form.iter().any(|f| f.widgets.iter().any(|w| w.page == Some(0))), "a field on page 1");
}

/// With the Link tool a right-click on a link is the link's and leaves comments as they are, as
/// before: the selected comment stays selected.
#[test]
fn a_right_click_with_the_link_tool_on_a_link_keeps_the_selected_comment() {
    let mut h = open(note_over_link_pdf(), "150", false);
    let on_note = at(&h, 150.0, 325.0);
    mouse_click(&mut h, on_note);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 1)), "the note is selected");
    link_tool(&mut h);
    // On the link, away from the note.
    let on_link = at(&h, 70.0, 325.0);
    right_click(&mut h, on_link);
    let v = &h.state().views[0];
    assert_eq!((v.links.context, v.comments.selected), (Some((0, 0)), Some((0, 1))), "the link's menu; the note stays selected");
}

/// A 300×400 page with a rectangle comment round the camera tool's first corner (view 15, 20).
fn comment_at_camera_corner_pdf() -> Vec<u8> {
    pdf(&[
        CATALOG,
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Square /Rect [5 365 45 395] /C [1 0 0] /F 4 /T (Ana) >>",
    ])
}

/// A camera corner has no menu: a right-click on a comment under it opens the comment's, as
/// before.
#[test]
fn a_right_click_on_a_comment_under_a_camera_corner_opens_the_comments_menu() {
    let mut h = open(comment_at_camera_corner_pdf(), "150", false);
    h.state_mut().quick_tool = QuickTool::EnhanceCamera;
    h.run_steps(4);
    let corner = at(&h, 15.0, 380.0);
    right_click(&mut h, corner);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)), "the comment is selected for its menu");
}

/// A right-button drag from `a` to `b`, in frames of their own.
fn right_drag(h: &mut Harness<'_, PdfCraftApp>, a: egui::Pos2, b: egui::Pos2) {
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE };
    h.hover_at(a);
    h.run_steps(1);
    h.event(button(a, true));
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(a + (b - a) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.event(button(b, false));
    h.run_steps(3);
}

// A right-button drag starts nothing a left-button drag does. egui's drag start counts every
// button, and each of these started on it.

/// With the Rectangle tool, a right-button drag draws no rectangle.
#[test]
fn a_right_button_drag_draws_no_rectangle() {
    let mut h = open(text_pdf(), "150", false);
    h.state_mut().quick_tool = QuickTool::Comment(CommentTool::Rectangle);
    h.run_steps(4);
    let (a, b) = (at(&h, 60.0, 150.0), at(&h, 200.0, 80.0));
    right_drag(&mut h, a, b);
    let s = h.state();
    assert!(s.session.get(s.views[0].id).unwrap().info.annotations.is_empty(), "no rectangle is drawn");
}

/// With Prepare open, a right-button drag around a field selects it by no selection rectangle.
#[test]
fn a_right_button_drag_selects_no_field() {
    let mut h = open(comment_and_checkbox_pdf(), "150", false);
    assert!(h.state_mut().execute("form.prepare"));
    h.run_steps(4);
    let (a, b) = (at(&h, 60.0, 250.0), at(&h, 140.0, 180.0));
    right_drag(&mut h, a, b);
    assert!(h.state().views[0].prepare.selected.is_none(), "no field is selected");
}

/// With the Link tool, a right-button drag draws no link.
#[test]
fn a_right_button_drag_draws_no_link() {
    let mut h = open(text_pdf(), "150", false);
    link_tool(&mut h);
    let (a, b) = (at(&h, 60.0, 150.0), at(&h, 200.0, 80.0));
    right_drag(&mut h, a, b);
    assert!(h.state().link_draft.is_none() && h.state().dialog != Some(Dialog::LinkProps), "no link is drawn");
}

/// With the camera, a right-button drag on another page leaves the quadrilateral where it is.
#[test]
fn a_right_button_drag_leaves_the_camera_quadrilateral() {
    let mut h = open(two_pages_pdf(), "100", false);
    h.state_mut().quick_tool = QuickTool::EnhanceCamera;
    h.run_steps(4);
    assert_eq!(h.state().views[0].camera.as_ref().map(|q| q.page), Some(0));
    let two = h.state().views[0].page_screen_rect(1).expect("page 2 on screen");
    right_drag(&mut h, two.min + egui::vec2(40.0, 40.0), two.min + egui::vec2(120.0, 100.0));
    assert_eq!(h.state().views[0].camera.as_ref().map(|q| q.page), Some(0), "the quadrilateral stays on page 1");
}

/// With the Select tool, a right-button drag across text selects none.
#[test]
fn a_right_button_drag_selects_no_text() {
    let mut h = open(text_pdf(), "150", false);
    let (a, b) = (at(&h, 45.0, 303.0), at(&h, 200.0, 303.0));
    assert_eq!(wait_for_cursor(&mut h, a, |c| c == egui::CursorIcon::Text), egui::CursorIcon::Text, "the page's text has arrived");
    right_drag(&mut h, a, b);
    assert!(h.state().views[0].selected_text().is_none(), "no text is selected");
}

/// A 300×400 page with a web link along its left edge, x 0..100, y 300..350.
fn edge_link_pdf() -> Vec<u8> {
    pdf(&[
        CATALOG,
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Link /Rect [0 300 100 350] /Border [0 0 0] /A << /S /URI /URI (https://example.org/) >> >>",
    ])
}

/// The Link tool's margin round a link stops at the page's edge: beside the page, where a press
/// does nothing, no hand promises one (as before the margin moved into the one link test).
#[test]
fn the_link_tools_hand_stops_at_the_pages_edge() {
    let mut h = open(edge_link_pdf(), "150", false);
    link_tool(&mut h);
    let on_link = at(&h, 10.0, 325.0);
    assert_eq!(cursor_at(&mut h, on_link), egui::CursorIcon::PointingHand, "on the link");
    let beside = at(&h, 0.0, 325.0) - egui::vec2(1.0, 0.0);
    assert_ne!(cursor_at(&mut h, beside), egui::CursorIcon::PointingHand, "just off the page");
}

/// With Edit text a right-click goes to what lies on top under it, as with every tool: on a
/// paragraph under a comment, the comment, selected for its menu; on the paragraph beside it, the
/// paragraph, and the comment is unselected. The page's menu is anchored there either way (Paste on
/// the picture menu, Delete page). Edit text used to keep the right-clicks on its paragraphs, and
/// left the comment over them unselected.
#[test]
fn a_right_click_with_edit_text_on_a_paragraph_under_a_comment_selects_the_comment() {
    let mut h = open(comment_on_text_pdf(), "150", false);
    h.state_mut().quick_tool = QuickTool::EditText;
    h.run_steps(4);
    let on_text = at(&h, 100.0, 303.0);
    wait_for_cursor(&mut h, on_text, |c| c != egui::CursorIcon::Default);
    right_click(&mut h, on_text);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)), "the comment over the paragraph is selected");
    assert_eq!(h.state().views[0].comments.context_at.map(|(page, _)| page), Some(0), "the menu is anchored on the page");
    // The rectangle ends at x 200; the line goes on.
    let beside = at(&h, 230.0, 303.0);
    right_click(&mut h, beside);
    assert_eq!(h.state().views[0].comments.selected, None, "the paragraph beside the comment leaves it unselected");
}

/// With Edit text, a right-click on added text over a comment selects no comment: what has no menu
/// gives way to a picture under it and to nothing else, as before.
#[test]
fn a_right_click_with_edit_text_on_added_text_over_a_comment_selects_no_comment() {
    let mut h = open(comment_on_text_pdf(), "150", false);
    assert!(h.state_mut().execute("edit.edit_text"));
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::AddText {
        page: 0,
        text: pdfcraft_engine::AddedText { rect: [110.0, 296.0, 190.0, 311.0], text: "Added".into(), ..Default::default() },
    }));
    h.run_steps(4);
    let on_added = at(&h, 150.0, 303.0);
    right_click(&mut h, on_added);
    assert_eq!(h.state().views[0].comments.selected, None, "the comment under the added text is not selected");
}

/// With a Prepare field tool a right-click is the tool's, as before: a comment under it is not
/// selected.
#[test]
fn a_right_click_with_a_field_tool_selects_no_comment() {
    let mut h = open(comment_and_checkbox_pdf(), "150", false);
    assert!(h.state_mut().execute("form.add.text"));
    h.run_steps(4);
    let on_comment = at(&h, 200.0, 125.0);
    right_click(&mut h, on_comment);
    assert_eq!(h.state().views[0].comments.selected, None);
}

/// With the Link tool a click that began off the page (in the gutter beside it) is no page's: the
/// selected link stays selected. It used to unselect it.
#[test]
fn a_click_that_began_off_the_page_keeps_the_selected_link() {
    let mut h = open(link_pdf(), "150", false);
    link_tool(&mut h);
    let on_link = at(&h, 150.0, 325.0);
    mouse_click(&mut h, on_link);
    assert_eq!(h.state().views[0].links.selected, Some((0, 0)), "the link is selected");
    let left = page(&h).min.x;
    slipping_click(&mut h, egui::pos2(left - 2.0, on_link.y), egui::pos2(left + 3.0, on_link.y));
    assert_eq!(h.state().views[0].links.selected, Some((0, 0)), "a click that began off the page leaves it selected");
}

/// A right-click while a comment is moved leaves the moved comment selected: the move ends on
/// it. The right-click used to select the comment under it mid-drag.
#[test]
fn a_right_click_during_a_comment_move_keeps_the_moved_comment_selected() {
    let mut h = open(stacked_comments_pdf(), "150", false);
    // "Bottom note" alone, then "Top note" alone.
    let (bottom, top) = (at(&h, 60.0, 160.0), at(&h, 190.0, 290.0));
    h.hover_at(bottom);
    h.run_steps(1);
    h.drag_at(bottom);
    h.run_steps(1);
    h.hover_at(bottom + egui::vec2(0.0, 20.0));
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: top, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE });
    }
    h.run_steps(1);
    let end = bottom + egui::vec2(0.0, 40.0);
    h.hover_at(end);
    h.run_steps(1);
    h.drop_at(end);
    h.run_steps(3);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)), "the moved comment stays selected");
}

/// A click on added text leaves the selected comment unselected, as a click on a form field does
/// (ruling 1). It used to keep it selected.
#[test]
fn a_click_on_added_text_deselects_the_selected_comment() {
    let mut h = open(comment_on_text_pdf(), "150", false);
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::AddText {
        page: 0,
        text: pdfcraft_engine::AddedText { rect: [40.0, 195.0, 140.0, 210.0], text: "Added".into(), ..Default::default() },
    }));
    // The Edit panel open, with the Select tool: added text is picked up.
    assert!(h.state_mut().execute("edit.text"));
    h.run_steps(3);
    h.state_mut().quick_tool = QuickTool::Select;
    h.run_steps(3);
    let on_comment = at(&h, 150.0, 315.0);
    mouse_click(&mut h, on_comment);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)), "the comment is selected");
    let on_added = at(&h, 90.0, 202.0);
    mouse_click(&mut h, on_added);
    assert_eq!(h.state().views[0].comments.selected, None, "a click on the added text leaves it unselected");
}

/// A comment gesture left over with no button down (a release the pages never saw) does not keep
/// a right-click from selecting the comment under it.
#[test]
fn a_right_click_selects_its_comment_though_a_gesture_was_left_over() {
    let mut h = open(stacked_comments_pdf(), "150", false);
    let bottom = at(&h, 60.0, 160.0);
    h.state_mut().views[0].comments.gesture = Some(pdfcraft_ui_egui::comments::Gesture::Move { page: 0, index: 0, from: bottom });
    let top = at(&h, 190.0, 290.0);
    right_click(&mut h, top);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 1)), "the note under the right-click is selected");
}

/// A comment's move ends once its button is up, though the pages missed its release, and applies
/// nothing: where it came up is not known, and the pointer has moved on. A drag elsewhere later moves
/// nothing either. The move waited for the next drag's end on the page, and took the comment there.
#[test]
fn a_comment_move_whose_release_the_pages_missed_ends_once_the_button_is_up() {
    let mut h = open(stacked_comments_pdf(), "150", false);
    let rect = |h: &Harness<'_, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().info.annotations.iter().find(|a| a.index == 0).map(|a| a.rect)
    };
    let before = rect(&h);
    let bottom = at(&h, 60.0, 160.0);
    h.state_mut().views[0].comments.gesture = Some(pdfcraft_ui_egui::comments::Gesture::Move { page: 0, index: 0, from: bottom });
    let elsewhere = at(&h, 250.0, 100.0);
    h.hover_at(elsewhere);
    h.run_steps(2);
    assert!(h.state().views[0].comments.gesture.is_none(), "the move is over");
    assert_eq!(rect(&h), before, "and moved nothing to where the pointer turned up");
    let blank = at(&h, 250.0, 60.0);
    mouse_drag(&mut h, blank, egui::vec2(30.0, 0.0));
    assert_eq!(rect(&h), before, "nothing moved the comment");
}

/// A click that began off the page (in the gutter beside it) is no page's: the selected comment
/// stays selected, as the Link tool's and Prepare's selections do. It used to unselect it.
#[test]
fn a_click_that_began_off_the_page_keeps_the_selected_comment() {
    let mut h = open(stacked_comments_pdf(), "150", false);
    let bottom = at(&h, 60.0, 160.0);
    mouse_click(&mut h, bottom);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)), "the note is selected");
    let left = page(&h).min.x;
    slipping_click(&mut h, egui::pos2(left - 2.0, bottom.y), egui::pos2(left + 3.0, bottom.y));
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)), "a click that began off the page leaves it selected");
}

/// A touch's long press on a comment selects it for its menu, as a right-click does: egui reports
/// it as a right-click with the finger still down, which holds no drag.
#[test]
fn a_long_press_on_a_comment_selects_it_for_its_menu() {
    let mut h = open(stacked_comments_pdf(), "150", false);
    let top = at(&h, 190.0, 290.0);
    let touch = |phase, pos| egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(1), phase, pos, force: None };
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    one_frame(&mut h, [touch(egui::TouchPhase::Start, top), egui::Event::PointerMoved(top), button(top, true)]);
    // Held still, past egui's long-press time.
    h.run_steps(6);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 1)), "the note under the finger is selected");
    one_frame(&mut h, [touch(egui::TouchPhase::End, top), button(top, false), egui::Event::PointerGone]);
    h.run_steps(2);
}

/// With the page menu anchored on page 1, presses on page 2 and holds the button, presses Escape
/// first with `escape`, right-clicks on page 2, and gives the page the menu is anchored on.
fn menu_page_after_a_right_click_with_the_button_held(escape: bool) -> Option<usize> {
    let mut h = open(two_pages_pdf(), "100", false);
    h.run_steps(2);
    h.state_mut().views[0].comments.context_at = Some((0, [10.0, 10.0]));
    let two = h.state().views[0].page_screen_rect(1).expect("page 2 on screen");
    let press = two.min + egui::vec2(50.0, 30.0);
    h.hover_at(press);
    h.run_steps(1);
    h.drag_at(press);
    h.run_steps(1);
    h.hover_at(press + egui::vec2(40.0, 0.0));
    h.run_steps(1);
    if escape {
        h.key_press(egui::Key::Escape);
        h.run_steps(1);
    }
    let menu = press + egui::vec2(100.0, 60.0);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: menu, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE });
    }
    h.run_steps(1);
    let page = h.state().views[0].comments.context_at.map(|(page, _)| page);
    h.drop_at(press + egui::vec2(40.0, 0.0));
    h.run_steps(2);
    page
}

/// A right-click while the primary button is held anchors the page's menu where it is (Delete page,
/// Paste): the held press leaves only the selections alone. It used to keep the menu on the page
/// right-clicked before.
#[test]
fn a_right_click_with_the_button_held_anchors_the_menu_on_its_page() {
    assert_eq!(menu_page_after_a_right_click_with_the_button_held(false), Some(1), "the menu is page 2's");
}

/// So does a right-click while a press Escape cancelled is held, though the cancelled press takes
/// nothing else.
#[test]
fn a_right_click_after_escape_cancelled_the_press_anchors_the_menu_on_its_page() {
    assert_eq!(menu_page_after_a_right_click_with_the_button_held(true), Some(1), "the menu is page 2's");
}

/// With Edit text a right-click on added text reaches comments, as before: the selected comment is
/// unselected.
#[test]
fn a_right_click_with_edit_text_on_added_text_unselects_the_comment() {
    let mut h = open(comment_on_text_pdf(), "150", false);
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::AddText {
        page: 0,
        text: pdfcraft_engine::AddedText { rect: [40.0, 195.0, 140.0, 210.0], text: "Added".into(), ..Default::default() },
    }));
    h.state_mut().quick_tool = QuickTool::EditText;
    h.run_steps(4);
    h.state_mut().views[0].comments.selected = Some((0, 0));
    let on_added = at(&h, 90.0, 202.0);
    right_click(&mut h, on_added);
    assert_eq!(h.state().views[0].comments.selected, None);
}

/// Two pages with their gaps hidden, after `tool` set the tool up, and a point on the row of pixels
/// where they touch, which lies on both.
fn touching_pages(tool: impl FnOnce(&mut PdfCraftApp)) -> (Harness<'static, PdfCraftApp>, egui::Pos2) {
    let mut h = open(two_pages_pdf(), "100", false);
    h.state_mut().views[0].show_page_gaps = false;
    tool(h.state_mut());
    h.run_steps(4);
    let (one, two) = {
        let v = &h.state().views[0];
        (v.page_screen_rect(0).expect("page 1 on screen"), v.page_screen_rect(1).expect("page 2 on screen"))
    };
    let seam = egui::pos2(one.min.x + 100.0, one.max.y);
    assert!(one.contains(seam) && two.contains(seam), "the pages touch: {one:?} {two:?}");
    (h, seam)
}

/// The pages the document's stamps are on.
fn stamp_pages(h: &Harness<'_, PdfCraftApp>) -> Vec<usize> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().info.annotations.iter().filter(|a| a.subtype == "Stamp").map(|a| a.page).collect()
}

/// Where pages touch (the gaps hidden), a press on the row they share is the first page's: a stamp
/// clicked there lands on page 1. The second page used to take the press over.
#[test]
fn a_stamp_clicked_where_two_pages_touch_lands_on_the_first() {
    let (mut h, seam) = touching_pages(|app| app.quick_tool = QuickTool::Stamp(StampKind::DynApproved));
    mouse_click(&mut h, seam);
    assert_eq!(stamp_pages(&h), vec![0]);
}

/// A click pressed on one page and released on the next (the gaps hidden, within egui's click
/// distance) places nothing: the press is the first page's, and the pointer is off it. `main` placed
/// the stamp on the second page.
#[test]
fn a_stamp_click_pressed_on_one_page_and_released_on_the_next_places_nothing() {
    let (mut h, seam) = touching_pages(|app| app.quick_tool = QuickTool::Stamp(StampKind::DynApproved));
    slipping_click(&mut h, seam - egui::vec2(0.0, 2.0), seam + egui::vec2(0.0, 2.0));
    assert_eq!(stamp_pages(&h), Vec::<usize>::new());
}

/// Where pages touch (the gaps hidden), a right-click on the row they share is the first page's:
/// the menu is anchored there. Both pages used to take it, and the second's anchor stood.
#[test]
fn a_right_click_where_two_pages_touch_is_the_first_pages() {
    let (mut h, seam) = touching_pages(|_| {});
    right_click(&mut h, seam);
    assert_eq!(h.state().views[0].comments.context_at.map(|(page, _)| page), Some(0));
}

/// With Prepare open, a click that began off the page (in the gutter beside it) is no page's: the
/// selected field stays selected, as the Link tool's and comments' selections do.
#[test]
fn with_prepare_a_click_that_began_off_the_page_keeps_the_selected_field() {
    let mut h = open(comment_and_checkbox_pdf(), "150", false);
    assert!(h.state_mut().execute("form.prepare"));
    h.run_steps(4);
    let on_box = at(&h, 110.0, 210.0);
    mouse_click(&mut h, on_box);
    assert!(h.state().views[0].prepare.selected.is_some(), "the field is selected");
    let left = page(&h).min.x;
    slipping_click(&mut h, egui::pos2(left - 2.0, on_box.y), egui::pos2(left + 3.0, on_box.y));
    assert!(h.state().views[0].prepare.selected.is_some(), "a click that began off the page leaves it selected");
}

/// With Prepare open on a document that can't be changed, a right-click on a field selects nothing
/// and opens the page's menu, whose field entries would only be refused.
#[test]
fn with_prepare_on_a_locked_document_a_right_click_on_a_field_selects_nothing() {
    let locked = crate::editing::encrypted(comment_and_checkbox_pdf(), pdfcraft_cos::Algorithm::Aes256, "", "owner", -1 ^ 8);
    let mut h = open(locked, "150", false);
    // Prepare doesn't open on such a document; it stays open on a tab switched to one.
    h.state_mut().left = pdfcraft_ui_egui::LeftPanel::Tool("form");
    h.state_mut().left_open = true;
    h.run_steps(4);
    assert!(h.state().is_preparing(), "Prepare is open");
    let id = h.state().views[0].id;
    assert!(!h.state().session.get(id).unwrap().allows_modification(), "the document can't be changed");
    let on_box = at(&h, 110.0, 210.0);
    right_click(&mut h, on_box);
    assert!(h.state().views[0].prepare.selected.is_none(), "no field is selected");
    assert!(h.query_by_label("Duplicate…").is_none(), "no field menu");
}

/// With Prepare open and a field selected, a right-click on the empty page opens the page's menu, and
/// one on the field the field's: the menu is what the right-click went to. The field's menu opened
/// wherever the page was right-clicked.
#[test]
fn with_prepare_a_right_click_beside_the_selected_field_opens_the_pages_menu() {
    let mut h = open(comment_and_checkbox_pdf(), "150", false);
    assert!(h.state_mut().execute("form.prepare"));
    h.run_steps(4);
    let on_box = at(&h, 110.0, 210.0);
    mouse_click(&mut h, on_box);
    assert!(h.state().views[0].prepare.selected.is_some(), "the field is selected");
    let blank = at(&h, 60.0, 300.0);
    right_click(&mut h, blank);
    assert!(h.query_by_label("Duplicate…").is_none(), "no field menu on the empty page");
    assert!(h.query_by_label("Marquee Zoom").is_some(), "the page's menu");
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    right_click(&mut h, on_box);
    assert!(h.query_by_label("Duplicate…").is_some(), "the field's menu on the field");
}

/// With a Prepare field tool picked, a right-click on a field selects it and opens its menu, as with
/// Prepare's Select tool: the tool's hit leaves fields out (it draws new ones over them), but a
/// right-click reaches them.
#[test]
fn with_a_field_tool_a_right_click_on_a_field_opens_its_menu() {
    let mut h = open(comment_and_checkbox_pdf(), "150", false);
    assert!(h.state_mut().execute("form.prepare"));
    h.run_steps(4);
    assert!(h.state_mut().execute("form.add.text"));
    h.run_steps(4);
    let on_box = at(&h, 110.0, 210.0);
    right_click(&mut h, on_box);
    assert!(h.state().views[0].prepare.selected.is_some(), "the field is selected");
    assert!(h.query_by_label("Duplicate…").is_some(), "the field's menu");
}

/// With Edit text, a right-click on a picture under a link is the picture's (Edit text doesn't
/// follow links): it is selected for its menu.
#[test]
fn with_edit_text_a_right_click_on_a_picture_under_a_link_selects_the_picture() {
    let mut h = open(linked_picture_pdf(), "150", false);
    h.state_mut().quick_tool = QuickTool::EditText;
    h.run_steps(4);
    let on_link = at(&h, 150.0, 300.0);
    right_click(&mut h, on_link);
    let selected = h.state().views[0].image_selection.as_ref().map(|s| (s.page, s.index));
    assert_eq!(selected, Some((0, 0)), "the picture is selected for its menu");
}

/// With the gaps between pages hidden, a drag from the top of page 2 under page 1's open paragraph is
/// page 2's: its area selection takes the line there. The paragraph's frame, which reaches past
/// page 1's edge, took the press and did nothing with it.
#[test]
fn a_drag_past_a_pages_edge_under_its_open_paragraph_is_the_next_pages() {
    let mut h = open(bottom_line_pages_pdf(), "100", false);
    h.state_mut().views[0].show_page_gaps = false;
    h.state_mut().quick_tool = QuickTool::EditText;
    h.run_steps(4);
    let (one, two) = {
        let v = &h.state().views[0];
        (v.page_screen_rect(0).expect("page 1 on screen"), v.page_screen_rect(1).expect("page 2 on screen"))
    };
    let line = egui::pos2(one.min.x + 60.0, one.max.y - 8.0);
    mouse_click(&mut h, line);
    assert!(h.state().views[0].line_editor.is_some(), "the bottom line is open");
    let below = egui::pos2(one.min.x + 60.0, two.min.y + 3.0);
    mouse_drag(&mut h, below, egui::vec2(200.0, 30.0));
    // What the area selection took, Delete removes.
    h.key_press(egui::Key::Delete);
    h.run_steps(4);
    let s = h.state();
    assert!(s.session.get(s.views[0].id).unwrap().text_blocks(1).is_empty(), "page 2's area selection took its line");
}

/// With Edit text, a click on the empty page leaves the selected picture unselected, on a page whose
/// objects can't be selected together too, where no area selection starts to do it. It stayed.
#[test]
fn a_click_beside_the_picture_unselects_it_where_objects_cant_be_grouped() {
    let mut h = open(ungroupable_picture_pdf(), "150", false);
    h.state_mut().quick_tool = QuickTool::EditText;
    h.run_steps(4);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.page_images(0).len(), 1, "the picture");
    assert!(doc.editable_objects(0).is_err(), "and no objects to group");
    h.state_mut().views[0].image_selection = Some(pdfcraft_ui_egui::ImageSelection::at(0, 0));
    h.run_steps(2);
    let blank = at(&h, 150.0, 100.0);
    mouse_click(&mut h, blank);
    assert!(h.state().views[0].image_selection.is_none(), "the picture is unselected");
}

/// With the Select tool a drag that starts on a link lets the picked picture go, as a drag elsewhere
/// does, though the link's click picks none.
#[test]
fn a_drag_from_a_link_lets_the_picked_picture_go() {
    let mut h = open(linked_picture_pdf(), "150", false);
    let beside = at(&h, 70.0, 300.0);
    mouse_click(&mut h, beside);
    assert!(h.state().views[0].picked_image.is_some(), "the picture is picked");
    let on_link = at(&h, 150.0, 300.0);
    mouse_drag(&mut h, on_link, egui::vec2(30.0, 0.0));
    assert!(h.state().views[0].picked_image.is_none(), "the drag lets it go");
}

/// With the Edit panel open and the Note tool, a click on added text is the added text's: no note is
/// started under it, as before.
#[test]
fn a_note_tool_click_on_added_text_starts_no_note() {
    let mut h = open(comment_on_text_pdf(), "150", false);
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::AddText {
        page: 0,
        text: pdfcraft_engine::AddedText { rect: [40.0, 195.0, 140.0, 210.0], text: "Added".into(), ..Default::default() },
    }));
    assert!(h.state_mut().execute("edit.text"));
    h.run_steps(3);
    h.state_mut().quick_tool = QuickTool::Comment(CommentTool::Note);
    h.run_steps(3);
    let on_added = at(&h, 90.0, 202.0);
    mouse_click(&mut h, on_added);
    assert!(h.state().views[0].comments.composer.is_none(), "no note is started");
}

/// With Fill & Sign, a mark being moved on the page keeps the page's presses, as before: a press held
/// through its move and released places no new mark. (A move left over once the button is up ends:
/// `a_comment_move_whose_release_the_pages_missed_ends_once_the_button_is_up`.)
#[test]
fn a_fill_and_sign_click_places_nothing_while_a_mark_is_moved() {
    let mut h = open(comment_on_text_pdf(), "150", false);
    h.state_mut().quick_tool = QuickTool::Fill(FillTool::Check);
    h.run_steps(4);
    let marks = |h: &Harness<'_, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().info.annotations.len()
    };
    let before = marks(&h);
    let blank = at(&h, 150.0, 100.0);
    h.hover_at(blank);
    h.run_steps(1);
    h.drag_at(blank);
    h.run_steps(1);
    h.state_mut().views[0].comments.gesture = Some(pdfcraft_ui_egui::comments::Gesture::Move { page: 0, index: 0, from: blank });
    h.drop_at(blank);
    h.run_steps(4);
    assert_eq!(marks(&h), before, "no mark is placed");
}

/// A right-click between the pages anchors no page's menu, so Delete page and Paste act on the
/// current page. The menu used to keep the anchor of the last right-click on a page.
#[test]
fn a_right_click_between_the_pages_anchors_no_pages_menu() {
    let mut h = open(two_pages_pdf(), "100", false);
    h.run_steps(2);
    h.state_mut().views[0].comments.context_at = Some((0, [10.0, 10.0]));
    let (one, two) = {
        let v = &h.state().views[0];
        (v.page_screen_rect(0).expect("page 1 on screen"), v.page_screen_rect(1).expect("page 2 on screen"))
    };
    let gap = egui::pos2(one.center().x, (one.max.y + two.min.y) / 2.0);
    assert!(!one.contains(gap) && !two.contains(gap), "between the pages: {one:?} {two:?}");
    right_click(&mut h, gap);
    assert_eq!(h.state().views[0].comments.context_at, None);
}

/// Two 300×400 pages, a rectangle comment on the first.
fn two_pages_and_a_comment_pdf() -> Vec<u8> {
    pdf(&[
        CATALOG,
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [5 0 R] >>",
        "<< /Type /Page /Parent 2 0 R >>",
        "<< /Type /Annot /Subtype /Square /Rect [30 290 200 320] /C [1 0 0] /F 4 /T (Reviewer) >>",
    ])
}

/// A right-click between the pages selects no comment, as one on a page's empty part does not, so
/// the menu that opens is the current page's. The selected comment's menu used to open there.
#[test]
fn a_right_click_between_the_pages_opens_the_page_menu_not_the_selected_comments() {
    let mut h = open(two_pages_and_a_comment_pdf(), "100", false);
    h.state_mut().views[0].comments.selected = Some((0, 0));
    h.run_steps(2);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)), "the comment is selected");
    let (one, two) = {
        let v = &h.state().views[0];
        (v.page_screen_rect(0).expect("page 1 on screen"), v.page_screen_rect(1).expect("page 2 on screen"))
    };
    let gap = egui::pos2(one.center().x, (one.max.y + two.min.y) / 2.0);
    assert!(!one.contains(gap) && !two.contains(gap), "between the pages: {one:?} {two:?}");
    right_click(&mut h, gap);
    assert_eq!(h.state().views[0].comments.selected, None);
    assert!(h.query_by_label("Rotate view counterclockwise").is_some(), "the page menu opens");
    assert!(h.query_by_label("Reply").is_none(), "not the comment's");
}

/// Where pages touch, a right-click on the row they share is page 1's, whatever lies there on page 2:
/// page 2's picture across its top edge opens no menu, and page 1's menu pastes on page 1, where it
/// opened. The page view's own picture scan used to open page 2's picture's menu there, whose Paste
/// centred the image on page 1's point, which on page 2 lies at its foot.
#[test]
fn paste_from_a_right_click_where_two_pages_touch_keeps_to_the_first_page() {
    let (mut h, seam) = touching_pages(|app| {
        app.system_clipboard = false;
        app.quick_tool = QuickTool::EditText;
    });
    let clip = || pdfcraft_ui_egui::Clip::Image { width: 60, height: 40, rgba: [50u8, 150, 250, 255].repeat(60 * 40) };
    h.state_mut().clipboard_override = Some(clip());
    assert!(h.state_mut().paste_image_at(1, [100.0, 400.0]), "a picture across page 2's top edge");
    h.state_mut().clipboard_override = Some(clip());
    h.state_mut().views[0].image_selection = None;
    h.run_steps(4);
    right_click(&mut h, seam);
    assert_eq!(h.state().views[0].comments.context_at.map(|(page, _)| page), Some(0), "page 1 takes the right-click");
    assert!(h.state().views[0].image_selection.is_none(), "page 2's picture is not selected for its menu");
    h.get_by_label_contains("Paste").click();
    h.run_steps(3);
    let images = |page| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().page_images(page)
    };
    assert_eq!(images(1).len(), 1, "nothing is pasted on page 2");
    let pasted = images(0);
    assert_eq!(pasted.len(), 1, "the image is pasted on page 1");
    let r = pasted[0].rect;
    assert!((r[1] + r[3]) / 2.0 < 100.0, "at page 1's foot, where its menu opened: {r:?}");
}

/// With the camera tool, a click on another page moves the quadrilateral there, as a drag from it
/// does. The click was located by its press's origin, which egui has cleared by the release frame,
/// so it did nothing (on `main` too).
#[test]
fn a_camera_click_on_another_page_moves_the_quadrilateral_there() {
    let mut h = open(two_pages_pdf(), "100", false);
    h.state_mut().quick_tool = QuickTool::EnhanceCamera;
    h.run_steps(4);
    assert_eq!(h.state().views[0].camera.map(|q| q.page), Some(0), "it starts on the current page");
    let two = h.state().views[0].page_screen_rect(1).expect("page 2 on screen");
    mouse_click(&mut h, two.min + egui::vec2(100.0, 60.0));
    assert_eq!(h.state().views[0].camera.map(|q| q.page), Some(1));
}

/// A camera click is located where it came up, not where the pointer went after it in the same
/// frame: pressed on page 2 and released on page 1 (the gaps hidden), it moves nothing, though the
/// pointer is back on page 2 by the frame's end. The pointer's last position used to move the
/// quadrilateral to page 2.
#[test]
fn a_camera_click_released_off_its_page_moves_nothing() {
    let (mut h, seam) = touching_pages(|app| app.quick_tool = QuickTool::EnhanceCamera);
    assert_eq!(h.state().views[0].camera.map(|q| q.page), Some(0), "it starts on the current page");
    let (press, release, back) = (seam + egui::vec2(0.0, 2.0), seam - egui::vec2(0.0, 2.0), seam + egui::vec2(0.0, 40.0));
    h.hover_at(press);
    h.run_steps(1);
    h.drag_at(press);
    h.run_steps(1);
    let up = egui::Event::PointerButton { pos: release, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE };
    one_frame(&mut h, [up, egui::Event::PointerMoved(back)]);
    h.run_steps(2);
    assert_eq!(h.state().views[0].camera.map(|q| q.page), Some(0));
}
