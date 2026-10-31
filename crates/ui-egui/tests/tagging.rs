//! The Tags panel and the Reading Order tool in the real shell (egui_kittest): drawing a
//! rectangle and tagging it, the order list, highlighting a tag's content and selecting a tag
//! from the page, and the structural buttons and drag and drop of the tag tree.

use egui::Pos2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{PdfCraftApp, QuickTool, RightPanel};

fn pdf(objects: &[String]) -> Vec<u8> {
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

fn stream(data: &str) -> String {
    format!("<< /Length {} >>\nstream\n{data}\nendstream", data.len())
}

const FONT: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";

/// A heading, a paragraph and a rule, untagged.
fn untagged() -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        stream("BT /F1 24 Tf 20 250 Td (Report) Tj ET\nBT /F1 12 Tf 20 200 Td (The year in review.) Tj ET\n20 150 260 2 re f"),
        FONT.into(),
    ])
}

/// Document › [P 8, Sect 9 › [P 10, Figure 11]].
fn tagged() -> Vec<u8> {
    pdf(&tagged_objects())
}

fn tagged_objects() -> Vec<String> {
    vec![
        "<< /Type /Catalog /Pages 2 0 R /MarkInfo << /Marked true >> /StructTreeRoot 6 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /StructParents 0 /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        stream(
            "/P << /MCID 0 >> BDC BT /F1 24 Tf 20 250 Td (Intro) Tj ET EMC\n\
             /P << /MCID 1 >> BDC BT /F1 12 Tf 20 200 Td (Body) Tj ET EMC\n\
             /Figure << /MCID 2 >> BDC 20 60 120 60 re f EMC",
        ),
        FONT.into(),
        "<< /Type /StructTreeRoot /K 7 0 R /ParentTree 12 0 R /ParentTreeNextKey 1 >>".into(),
        "<< /Type /StructElem /S /Document /P 6 0 R /K [8 0 R 9 0 R] >>".into(),
        "<< /Type /StructElem /S /P /P 7 0 R /Pg 3 0 R /K 0 >>".into(),
        "<< /Type /StructElem /S /Sect /P 7 0 R /Pg 3 0 R /K [10 0 R 11 0 R] >>".into(),
        "<< /Type /StructElem /S /P /P 9 0 R /K 1 >>".into(),
        "<< /Type /StructElem /S /Figure /P 9 0 R /K 2 >>".into(),
        "<< /Nums [0 [8 0 R 10 0 R 11 0 R]] >>".into(),
    ]
}

fn harness(name: &'static str, bytes: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes(name, None, bytes.clone()).unwrap();
        app
    });
    h.run_steps(4);
    h
}

/// A point of page 1 in PDF user space, on screen.
fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let s = h.state();
    let page = &s.session.get(s.views[0].id).unwrap().info.pages[0];
    let xf = pdfcraft_ui_egui::canvas::PageXform {
        rect: s.views[0].page_screen_rect(0).expect("on screen"),
        rot: s.views[0].rotation,
        pw: page.width,
        ph: page.height,
    };
    let p = page.user_to_view(x, y);
    xf.norm_to_screen(p[0] / xf.pw, p[1] / xf.ph)
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

/// Scroll a control into view and click it (the side panels scroll).
fn press(h: &mut Harness<'static, PdfCraftApp>, label: &str) {
    h.get_by_label(label).scroll_to_me();
    // The scroll is animated: let it finish before reading where the control is.
    h.run_steps(30);
    h.get_by_label(label).click();
    h.run_steps(3);
}

fn tags(h: &Harness<'static, PdfCraftApp>) -> Vec<(u32, String, usize)> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().tags().into_iter().map(|t| (t.object.num, t.kind, t.depth)).collect()
}

fn undo_label(h: &Harness<'static, PdfCraftApp>) -> Option<String> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().can_undo().map(str::to_owned)
}

#[test]
fn the_reading_order_tool_tags_what_a_rectangle_covers() {
    let mut h = harness("report.pdf", untagged());
    assert!(h.state_mut().execute("a11y.reading_order"));
    h.run_steps(3);
    assert_eq!(h.state().quick_tool, QuickTool::ReadingOrder);
    assert_eq!(h.state().right, Some(RightPanel::ReadingOrder));
    // Untagged content is listed for selection.
    h.get_by_label("Text · Report");
    h.get_by_label("Text · The year in review.");
    // A rectangle around the heading selects it; <H1> tags it.
    let (a, b) = (at(&h, 10.0, 290.0), at(&h, 200.0, 240.0));
    drag(&mut h, a, b);
    assert_eq!(h.state().reading_order_selection(), (Some(0), vec![0]));
    h.get_by_label("<H1>").click();
    h.run_steps(3);
    assert_eq!(tags(&h).iter().map(|(_, k, _)| k.as_str()).collect::<Vec<_>>(), ["Document", "H1"]);
    assert_eq!(undo_label(&h).as_deref(), Some("Tag content as H1"));
    // A click on the paragraph selects it; the rule becomes an artifact from the untagged list.
    let p = at(&h, 60.0, 204.0);
    click(&mut h, p);
    assert_eq!(h.state().reading_order_selection().1, vec![1]);
    h.get_by_label("<P>").click();
    h.run_steps(3);
    h.get_by_label("Graphic").click();
    h.run_steps(1);
    h.get_by_label("Mark as artifact").click();
    h.run_steps(3);
    assert!(h.query_by_label("Untagged content").is_none(), "nothing is left untagged");
    // The order list numbers the regions.
    h.get_by_label("1  <H1>  Report");
    h.get_by_label("2  <P>  The year in review.");
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.structure_problems(), Vec::<String>::new());
    // Putting the tool down with the panel's close button.
    let close = h
        .get_all_by_label("Close")
        .max_by(|a, b| a.rect().center().x.total_cmp(&b.rect().center().x))
        .expect("the panel's close button")
        .rect()
        .center();
    click(&mut h, close);
    assert_eq!(h.state().quick_tool, QuickTool::Select);
}

#[test]
fn the_order_list_moves_regions_and_selects_their_tags() {
    let mut h = harness("tagged.pdf", tagged());
    assert!(h.state_mut().execute("a11y.reading_order"));
    h.run_steps(3);
    h.get_by_label("3  <Figure>").click();
    h.run_steps(2);
    assert_eq!(h.state().selected_tag(), Some((11, 0)), "the Tags panel follows the order list");
    h.get_by_label("Move up").click();
    h.run_steps(3);
    assert_eq!(undo_label(&h).as_deref(), Some("Move object"));
    h.get_by_label("2  <Figure>");
    // Drag the first row below the last: the intro now reads last.
    let (from, to) = {
        let first = h.get_by_label("1  <P>  Intro").rect();
        let last = h.get_by_label("3  <P>  Body").rect();
        (first.center(), Pos2::new(last.center().x, last.bottom() - 2.0))
    };
    drag(&mut h, from, to);
    h.get_by_label("3  <P>  Intro");
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().structure_problems(), Vec::<String>::new());
}

#[test]
fn the_tags_panel_highlights_content_and_a_click_on_the_page_selects_its_tag() {
    let mut h = harness("tagged.pdf", tagged());
    assert!(h.state_mut().execute("a11y.tags"));
    h.run_steps(3);
    h.get_by_label("Highlight content");
    // A click on the figure's rectangle selects the Figure tag.
    let p = at(&h, 80.0, 90.0);
    click(&mut h, p);
    assert_eq!(h.state().selected_tag(), Some((11, 0)));
    let p = at(&h, 30.0, 255.0);
    click(&mut h, p);
    assert_eq!(h.state().selected_tag(), Some((8, 0)));
    // With highlighting off, a click on the page no longer selects.
    h.get_by_label("Highlight content").click();
    h.run_steps(2);
    let p = at(&h, 80.0, 90.0);
    click(&mut h, p);
    assert_eq!(h.state().selected_tag(), Some((8, 0)));
}

#[test]
fn a_click_that_selects_a_tag_does_not_follow_the_link_over_it() {
    let mut objects = tagged_objects();
    objects[2] = objects[2].replace("/StructParents 0", "/StructParents 0 /Annots [13 0 R]");
    objects.push("<< /Type /Annot /Subtype /Link /Rect [15 240 120 280] /Border [0 0 0] /A << /S /URI /URI (https://example.com/) >> >>".into());
    let mut h = harness("linked.pdf", pdf(&objects));
    assert!(h.state_mut().execute("a11y.tags"));
    h.run_steps(3);
    h.get_by_label("Highlight content");
    let p = at(&h, 30.0, 255.0);
    click(&mut h, p);
    assert_eq!(h.state().selected_tag(), Some((8, 0)));
    assert!(h.state().pending_link.is_none(), "the link isn't followed");
}

#[test]
fn the_reading_order_selection_starts_over_in_another_document() {
    let mut h = harness("report.pdf", untagged());
    assert!(h.state_mut().execute("a11y.reading_order"));
    h.run_steps(3);
    let p = at(&h, 60.0, 204.0);
    click(&mut h, p);
    assert_eq!(h.state().reading_order_selection(), (Some(0), vec![1]));
    h.state_mut().open_bytes("other.pdf", None, tagged()).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().reading_order_selection(), (None, Vec::new()));
}

#[test]
fn the_tags_panel_adds_deletes_and_moves_tags() {
    let mut h = harness("tagged.pdf", tagged());
    assert!(h.state_mut().execute("a11y.tags"));
    h.run_steps(3);
    // New tag: a child of the selected tag, of the chosen type (P by default).
    h.get_by_label("Sect · 9").click();
    h.run_steps(1);
    press(&mut h, "New tag");
    assert_eq!(undo_label(&h).as_deref(), Some("New tag"));
    let added = tags(&h).last().cloned().unwrap();
    assert_eq!((added.1.as_str(), added.2), ("P", 2));
    // The new tag is selected and scrolled into view.
    h.run_steps(2);
    assert_eq!(h.state().selected_tag(), Some((added.0, 0)));
    h.get_by_label(&format!("P · {}", added.0));
    press(&mut h, "Delete tag");
    assert_eq!(undo_label(&h).as_deref(), Some("Delete tag"));
    // Out of its section, and back into the paragraph before it.
    h.get_by_label("Figure · 11").click();
    h.run_steps(1);
    press(&mut h, "Move out of parent tag");
    assert_eq!(tags(&h), [(7, "Document".into(), 0), (8, "P".into(), 1), (9, "Sect".into(), 1), (10, "P".into(), 2), (11, "Figure".into(), 1)]);
    press(&mut h, "Move into previous tag");
    assert_eq!(tags(&h).last(), Some(&(11, "Figure".into(), 2)));
    // Unwrap the section: its paragraph and figure move up into the document.
    h.get_by_label("Sect · 9").click();
    h.run_steps(1);
    press(&mut h, "Remove tag, keep its contents");
    assert_eq!(tags(&h), [(7, "Document".into(), 0), (8, "P".into(), 1), (10, "P".into(), 1), (11, "Figure".into(), 1)]);
    // Drag the figure onto the top third of the first paragraph: it goes before it.
    let (from, to) = {
        let figure = h.get_by_label("Figure · 11").rect();
        let first = h.get_by_label("P · 8").rect();
        (figure.center(), Pos2::new(first.center().x, first.top() + 2.0))
    };
    drag(&mut h, from, to);
    assert_eq!(tags(&h), [(7, "Document".into(), 0), (11, "Figure".into(), 1), (8, "P".into(), 1), (10, "P".into(), 1)]);
    assert_eq!(undo_label(&h).as_deref(), Some("Move object"));
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().structure_problems(), Vec::<String>::new());
}
