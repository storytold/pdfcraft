//! A new bookmark goes to where the reader is (#3865): the top of the window, or the corner of
//! the selected text, instead of the top of the page. Driven like a user would, with ⌘B.

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::Edit;
use pdfcraft_render::DestView;
use pdfcraft_ui_egui::PdfCraftApp;

/// Three 300×400 pages, each with a line of Helvetica text at (40, 150), the first optionally
/// turned by `rotate` (the others upright).
fn fixture(rotate: u16) -> Vec<u8> {
    fixture_of(&[(300, 400); 3], rotate)
}

/// Pages of the given sizes (points), each with a line of Helvetica text at (40, 150), the first
/// turned by `rotate`.
fn fixture_of(sizes: &[(u32, u32)], rotate: u16) -> Vec<u8> {
    let n = sizes.len();
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + i)).collect();
    let (content, font) = (3 + n, 4 + n);
    let mut objs = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string(), format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" "))];
    for (i, (w, h)) in sizes.iter().enumerate() {
        let r = if i == 0 { format!(" /Rotate {rotate}") } else { String::new() };
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R{r} /MediaBox [0 0 {w} {h}] /Contents {content} 0 R /Resources << /Font << /F1 {font} 0 R >> >> >>"
        ));
    }
    let body = "BT /F1 14 Tf 40 150 Td (The quick brown fox) Tj ET";
    objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string());
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

fn harness(rotate: u16) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("text.pdf", None, fixture(rotate)).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "bookmarks").unwrap();
        app
    });
    h.run_steps(4);
    crate::support::settle(&mut h);
    h
}

/// Make a bookmark with ⌘B, as the Bookmarks button does, and say what it goes to.
fn new_bookmark(h: &mut Harness<'static, PdfCraftApp>) -> (Option<usize>, DestView) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::B);
    h.run_steps(3);
    let id = h.state().views[0].id;
    let outline = &h.state().session.get(id).unwrap().info.outline;
    let made = outline.iter().find(|b| b.title == "Untitled").expect("the new bookmark");
    (made.page, made.view)
}

fn xyz(view: DestView) -> (Option<f32>, Option<f32>) {
    match view {
        DestView::Xyz { left, top, zoom } => {
            assert_eq!(zoom, None, "the reader's zoom is kept");
            (left, top)
        }
        other => panic!("expected /XYZ, got {other:?}"),
    }
}

fn close(got: Option<f32>, want: f32, what: &str) {
    let got = got.unwrap_or_else(|| panic!("{what} is set"));
    assert!((got - want).abs() < 1.0, "{what}: {got} vs {want}");
}

#[test]
fn a_new_bookmark_goes_to_the_top_of_the_window_not_the_top_of_the_page() {
    let mut h = harness(0);
    // A bookmark that scrolls page 2 to 250 pt up its height puts that line at the top of the
    // window; a bookmark made there then goes to the same place.
    h.state_mut().apply_edit(Edit::AddBookmark { parent: vec![], index: 0, title: "Mid".into(), page: 1, left: None, top: Some(250.0) });
    h.run_steps(3);
    h.get_by_label("Mid").click();
    crate::support::settle(&mut h);
    let (page, view) = new_bookmark(&mut h);
    assert_eq!(page, Some(1), "the page at the top of the window");
    let (left, top) = xyz(view);
    close(top, 250.0, "top");
    assert_eq!(left, None, "the horizontal position is the reader's");
}

#[test]
fn a_bookmark_made_at_the_start_goes_to_the_top_of_the_first_page() {
    let mut h = harness(0);
    let (page, view) = new_bookmark(&mut h);
    assert_eq!(page, Some(0));
    let (left, top) = xyz(view);
    close(top, 400.0, "top is the page's top edge");
    assert_eq!(left, None);
}

#[test]
fn a_bookmark_made_with_text_selected_goes_to_the_selections_corner() {
    let mut h = harness(0);
    // "quick brown": the selection starts after "The ", 40 pt in plus its width, on the line at 150.
    h.state_mut().views[0].select_text(0, 4, 15);
    h.run_steps(3);
    let (page, view) = new_bookmark(&mut h);
    assert_eq!(page, Some(0), "the selection's page");
    let (left, top) = xyz(view);
    let left = left.expect("left is set from the selection");
    let top = top.expect("top is set from the selection");
    assert!((66.0..=70.0).contains(&left), "left is where 'q' starts: {left}");
    assert!((155.0..=168.0).contains(&top), "top is above the baseline at 150: {top}");
}

#[test]
fn a_bookmark_made_on_a_turned_page_pins_the_axis_the_window_scrolls_along() {
    // Page 1 is turned a quarter: its displayed top is a user x, so that is what is pinned.
    let mut h = harness(90);
    let (page, view) = new_bookmark(&mut h);
    assert_eq!(page, Some(0));
    let (left, top) = xyz(view);
    close(left, 0.0, "left is the displayed top edge");
    assert_eq!(top, None, "the displayed horizontal position is the reader's");
}

/// Turn the view a quarter at a time (View ▸ Rotate View) and let the layout settle.
fn rotate_view(h: &mut Harness<'static, PdfCraftApp>, degrees: u16) {
    h.state_mut().views[0].rotation = degrees;
    h.run_steps(3);
    crate::support::settle(h);
}

#[test]
fn a_bookmark_made_in_a_turned_view_pins_the_axis_that_is_vertical_on_screen() {
    // At the start of the document the window's top edge is the first page's top as displayed.
    // Turned clockwise a quarter, that edge is the page's left; half a turn, its bottom; three
    // quarters, its right. The other coordinate is the reader's, so it stays unset.
    for (degrees, left, top) in [(90, Some(0.0), None), (180, None, Some(0.0)), (270, Some(300.0), None)] {
        let mut h = harness(0);
        rotate_view(&mut h, degrees);
        let (page, view) = new_bookmark(&mut h);
        assert_eq!(page, Some(0), "{degrees}");
        let (l, t) = xyz(view);
        match (left, top) {
            (Some(want), None) => {
                close(l, want, &format!("{degrees}: left"));
                assert_eq!(t, None, "{degrees}: top is the reader's");
            }
            (None, Some(want)) => {
                close(t, want, &format!("{degrees}: top"));
                assert_eq!(l, None, "{degrees}: left is the reader's");
            }
            _ => unreachable!("one axis is pinned"),
        }
    }
}

#[test]
fn a_bookmark_made_with_text_selected_in_a_turned_view_goes_to_the_corner_shown_top_left() {
    // The selection "quick brown" spans x from about 68 to 143 and y from just under the baseline
    // at 150 to just above its ascent. Its bounds' corner at the screen's top-left is the
    // upper-left (0), lower-left (90), lower-right (180) or upper-right (270) one in the page.
    for (degrees, right_end, upper) in [(0, false, true), (90, false, false), (180, true, false), (270, true, true)] {
        let mut h = harness(0);
        rotate_view(&mut h, degrees);
        h.state_mut().views[0].select_text(0, 4, 15);
        h.run_steps(3);
        let (page, view) = new_bookmark(&mut h);
        assert_eq!(page, Some(0), "{degrees}");
        let (left, top) = xyz(view);
        let (left, top) = (left.expect("left"), top.expect("top"));
        assert_eq!(left > 105.0, right_end, "{degrees}: left {left} is the {} end", if right_end { "right" } else { "left" });
        if upper {
            assert!((155.0..=168.0).contains(&top), "{degrees}: top {top} is above the baseline at 150");
        } else {
            assert!((140.0..=152.0).contains(&top), "{degrees}: top {top} is at the bottom of the text");
        }
    }
}

#[test]
fn a_bookmark_in_a_spread_of_unequal_pages_goes_to_the_page_crossing_the_top_of_the_window() {
    use pdfcraft_ui_egui::canvas::PageLayout;
    // Pages 1 and 2 share a row; page 1 is shorter, so it is centred and starts below page 2's top.
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("spread.pdf", None, fixture_of(&[(300, 200), (300, 400), (300, 400)], 0)).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "bookmarks").unwrap();
        app
    });
    h.state_mut().views[0].layout = PageLayout::TwoUp;
    h.run_steps(4);
    crate::support::settle(&mut h);
    // 20 pt below page 2's top is at the window's top, where page 1 has not started.
    h.state_mut().apply_edit(Edit::AddBookmark { parent: vec![], index: 0, title: "Right".into(), page: 1, left: None, top: Some(380.0) });
    h.run_steps(3);
    h.get_by_label("Right").click();
    crate::support::settle(&mut h);
    let (page, view) = new_bookmark(&mut h);
    assert_eq!(page, Some(1), "page 2 crosses the top of the window; page 1 starts below it");
    close(xyz(view).1, 380.0, "top");
}
