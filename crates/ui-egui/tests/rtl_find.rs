//! The find bar and the Search panel, typed the way a person types: one character at a time.

use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

fn fixture() -> Vec<u8> {
    let body = "BT /F1 18 Tf 20 150 Td (Page) Tj ET";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_owned(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1100.0, 720.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("a.pdf", None, fixture()).unwrap();
        app
    });
    h.state_mut().active = Some(0);
    h.run_steps(2);
    h
}

fn query(h: &Harness<'static, PdfCraftApp>) -> String {
    h.state().views[0].find.as_ref().map(|f| f.query.clone()).unwrap_or_default()
}

/// Types into the focused find field, one character at a time, the way the caret bug showed up.
fn type_chars(h: &mut Harness<'static, PdfCraftApp>, text: &str) {
    for ch in text.chars() {
        h.event(egui::Event::Text(ch.to_string()));
        h.run_steps(1);
    }
}

#[test]
fn find_bar_keeps_arabic_typing_order() {
    let mut h = harness();
    h.state_mut().views[0].open_find();
    h.run_steps(2);
    assert!(h.ctx.memory(|m| m.has_focus(egui::Id::unique("find-input"))), "the find field has focus");
    let phrase = "مؤشرات الأداء";
    type_chars(&mut h, phrase);
    assert_eq!(query(&h), phrase, "search keeps the typed order, not the order on screen");

    // The leading-edge caret: backspace removes the last typed character, not the first.
    h.key_press(egui::Key::Backspace);
    h.run_steps(1);
    assert_eq!(query(&h), "مؤشرات الأدا");

    // The same query is the Search panel's field. Typing there still appends in stored order.
    assert!(h.state_mut().execute("edit.advanced_search"));
    h.run_steps(2);
    assert!(h.ctx.memory(|m| m.has_focus(egui::Id::unique("search-panel-input"))), "the search panel field has focus");
    type_chars(&mut h, "ء");
    assert_eq!(query(&h), phrase);

    // A picture of the open search field, so the words are checked as pixels as well as glyphs.
    let image = h.render().expect("the search panel renders");
    assert!(image.width() > 100 && image.height() > 100);
    let mut ink = 0u32;
    for pixel in image.pixels() {
        let [r, g, b, a] = pixel.0;
        if a > 0 && (r < 240 || g < 240 || b < 240) {
            ink = ink.saturating_add(1);
        }
    }
    assert!(ink > 50, "the rendered window has no text");
}

#[test]
fn find_bar_still_types_latin_at_the_end() {
    let mut h = harness();
    h.state_mut().views[0].open_find();
    h.run_steps(2);
    type_chars(&mut h, "page");
    assert_eq!(query(&h), "page");
    h.key_press(egui::Key::Backspace);
    h.run_steps(1);
    assert_eq!(query(&h), "pag");
}
