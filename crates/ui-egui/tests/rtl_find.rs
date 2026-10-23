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
    assert!(h.ctx.memory(|m| m.has_focus(egui::Id::new("find-input"))), "the find field has focus");
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
    assert!(h.ctx.memory(|m| m.has_focus(egui::Id::new("search-panel-input"))), "the search panel field has focus");
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
fn find_bar_undo_keeps_typing_order() {
    let mut h = harness();
    h.state_mut().views[0].open_find();
    h.run_steps(2);
    let phrase = "مؤشرات الأداء";
    type_chars(&mut h, phrase);
    // egui records an undo point only after the text has sat still for a second.
    h.run_steps(6);
    type_chars(&mut h, "ء");
    assert_eq!(query(&h), format!("{phrase}ء"));
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run_steps(2);
    let q = query(&h);
    assert_eq!(q, phrase, "undo restores the typed phrase, got {q:?}");
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z);
    h.run_steps(2);
    assert_eq!(query(&h), format!("{phrase}ء"), "redo puts the last typed letter back in typing order");
}

#[test]
fn find_bar_word_backspace_removes_the_last_typed_word() {
    let mut h = harness();
    h.state_mut().views[0].open_find();
    h.run_steps(2);
    type_chars(&mut h, "مر حب");
    assert_eq!(query(&h), "مر حب");
    // Alt+Backspace is the word delete on every platform egui ships (alt or ctrl).
    h.key_press_modifiers(egui::Modifiers::ALT, egui::Key::Backspace);
    h.run_steps(2);
    assert_eq!(query(&h), "مر ", "word backspace removed the last typed word, got {:?}", query(&h));
}

/// Dark pixels in each column of the find bar. The search icon and the buttons are in this
/// picture too, so a before-and-after difference is what belongs to the query.
fn find_bar_ink(h: &mut Harness<'static, PdfCraftApp>) -> Vec<u32> {
    let bar = h.ctx.memory(|m| m.area_rect(egui::Id::new("find-bar"))).expect("the find bar is on screen");
    let image = h.render().expect("the find bar renders");
    let ppp = h.ctx.pixels_per_point();
    let x0 = (bar.left() * ppp).floor().max(0.0) as u32;
    let y0 = (bar.top() * ppp).floor().max(0.0) as u32;
    let x1 = ((bar.right() * ppp).ceil() as u32).min(image.width());
    let y1 = ((bar.bottom() * ppp).ceil() as u32).min(image.height());
    let mut columns = vec![0u32; image.width() as usize];
    for x in x0..x1 {
        let mut ink = 0u32;
        for y in y0..y1 {
            let [r, g, b, a] = image.get_pixel(x, y).0;
            if a > 0 && r < 160 && g < 160 && b < 160 {
                ink = ink.saturating_add(1);
            }
        }
        if let Some(slot) = columns.get_mut(x as usize) {
            *slot = ink;
        }
    }
    columns
}

/// The query's pixels, not the search icon: columns that grew after typing. Both words have to
/// be in that band. A frozen stack of glyphs is only a few columns wide.
#[test]
fn find_bar_paints_both_words_across_the_line() {
    let mut h = harness();
    h.state_mut().views[0].open_find();
    h.run_steps(2);
    let before = find_bar_ink(&mut h);
    type_chars(&mut h, "مؤشرات الأداء");
    let after = find_bar_ink(&mut h);
    let mut gained = Vec::new();
    for (x, (earlier, later)) in before.iter().zip(after.iter()).enumerate() {
        if later.saturating_sub(*earlier) > 2 {
            gained.push(x as u32);
        }
    }
    let (Some(&first), Some(&last)) = (gained.first(), gained.last()) else {
        panic!("typing did not add ink to the find bar");
    };
    let width = last.saturating_sub(first);
    assert!(width > 40, "the query is only {width}px wide, so the words collapsed");
    let mid = first + width / 2;
    let left = gained.iter().filter(|x| **x < mid).count();
    let right = gained.iter().filter(|x| **x > mid).count();
    assert!(left > 4 && right > 4, "the query's ink is not on both sides: left {left} right {right} width {width}");
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

/// The Search panel field is a second widget on the same query. Its undo must restore
/// typing order too, and must not depend on the find bar having been used first.
#[test]
fn search_panel_undo_keeps_typing_order() {
    let mut h = harness();
    assert!(h.state_mut().execute("edit.advanced_search"));
    h.run_steps(2);
    assert!(h.ctx.memory(|m| m.has_focus(egui::Id::new("search-panel-input"))), "the search panel field has focus");
    let phrase = "مؤشرات الأداء";
    type_chars(&mut h, phrase);
    h.run_steps(6);
    type_chars(&mut h, "ء");
    assert_eq!(query(&h), format!("{phrase}ء"));
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run_steps(2);
    assert_eq!(query(&h), phrase, "search panel undo stored the on-screen order, got {:?}", query(&h));
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z);
    h.run_steps(2);
    assert_eq!(query(&h), format!("{phrase}ء"), "search panel redo dropped the last letter");
}

/// Backspace and Delete on an empty find field must not insert or panic.
#[test]
fn find_bar_backspace_and_delete_on_empty_keep_it_empty() {
    let mut h = harness();
    h.state_mut().views[0].open_find();
    h.run_steps(2);
    h.key_press(egui::Key::Backspace);
    h.run_steps(1);
    h.key_press(egui::Key::Delete);
    h.run_steps(1);
    assert_eq!(query(&h), "");
    type_chars(&mut h, "مر");
    assert_eq!(query(&h), "مر");
}
