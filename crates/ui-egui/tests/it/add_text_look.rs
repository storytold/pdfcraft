//! Edit a PDF ▸ Add text in the real shell (egui_kittest): a new box starts with the look of the
//! page's text nearest to where it is placed, unless a style was chosen in the Format panel.

use egui::Pos2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::{AddedContent, AddedText, FontFamily};
use pdfcraft_ui_egui::PdfCraftApp;

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

/// A 300 × 400 page: a heading in dark red 18 pt Times-Bold 70 pt from the top, a line of
/// invisible text (as an OCR layer has) 200 pt from the top, and a note in navy 9 pt Courier
/// 360 pt from the top.
fn styled_page() -> Vec<u8> {
    // Each line in a q/Q pair of its own: the colour and the rendering mode (Tr) outlast ET.
    let body = "q BT 0.6 0 0 rg /F1 18 Tf 40 330 Td (Quarterly summary) Tj ET Q\n\
                q BT 3 Tr /F2 14 Tf 40 200 Td (scanned words) Tj ET Q\n\
                q BT 0 0 0.5 rg /F3 9 Tf 40 40 Td (Reference 17-B) Tj ET Q";
    let content = format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len() + 1);
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R /F3 7 0 R >> >> >>",
        content.as_str(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Times-Bold /Encoding /WinAnsiEncoding >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>",
    ])
}

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(0.25).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("styled.pdf", None, styled_page()).unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    crate::support::settle(&mut h);
    h
}

/// A point on the page, in points from its top left corner.
fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = r.width() / 300.0;
    r.min + egui::vec2(x * k, y * k)
}

fn click(h: &mut Harness<'static, PdfCraftApp>, p: Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
}

/// The text added to the page, in the order it was added.
fn added_text(h: &Harness<'static, PdfCraftApp>) -> Vec<AddedText> {
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    doc.added()
        .iter()
        .filter_map(|a| match &a.content {
            AddedContent::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

/// Family, bold, italic, size and colour.
fn look(t: &AddedText) -> (FontFamily, bool, bool, f64, [f64; 3]) {
    (t.family, t.bold, t.italic, t.size, t.color)
}

/// Text added under the heading looks like the heading, and text added above the note looks like
/// the note: each box takes the family, weight, size and colour of the paragraph nearest to it.
/// The invisible line is nearer to the first box, but nobody sees its look, so it doesn't count.
#[test]
fn a_new_text_box_takes_the_look_of_the_nearest_visible_text() {
    let mut h = harness();
    assert!(h.state_mut().execute("edit.text"));
    h.run_steps(2);
    let p = at(&h, 40.0, 160.0);
    click(&mut h, p);
    h.event(egui::Event::Text("Draft".into()));
    h.run_steps(2);
    // A click elsewhere keeps the first box and starts the second.
    let p = at(&h, 40.0, 320.0);
    click(&mut h, p);
    h.event(egui::Event::Text("Filed".into()));
    h.run_steps(2);
    h.get_by_label("Done adding text").click();
    h.run_steps(3);
    let added = added_text(&h);
    let texts: Vec<&str> = added.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(texts, ["Draft", "Filed"]);
    assert_eq!(look(&added[0]), (FontFamily::Times, true, false, 18.0, [0.6, 0.0, 0.0]), "the heading's look");
    assert_eq!(look(&added[1]), (FontFamily::Courier, false, false, 9.0, [0.0, 0.0, 0.5]), "the note's look");
}

/// A style chosen in the Format panel before placing the box is used as it is.
#[test]
fn a_style_chosen_in_the_format_panel_is_kept() {
    let mut h = harness();
    assert!(h.state_mut().execute("edit.text"));
    h.run_steps(2);
    h.get_by_label("Bold").click();
    h.run_steps(2);
    let p = at(&h, 40.0, 160.0);
    click(&mut h, p);
    h.event(egui::Event::Text("Draft".into()));
    h.run_steps(2);
    h.get_by_label("Done adding text").click();
    h.run_steps(3);
    let added = added_text(&h);
    assert_eq!(added.len(), 1);
    assert_eq!(added[0].text, "Draft");
    assert_eq!(look(&added[0]), (FontFamily::Helvetica, true, false, 12.0, [0.0, 0.0, 0.0]), "the chosen style");
}
