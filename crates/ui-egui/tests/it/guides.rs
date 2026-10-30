//! Guides dragged out of the rulers (`view.guides`), end to end: the drag goes to the ruler and
//! the guide, never to the page under it.
use std::sync::{Arc, Mutex};

use egui::{Modifiers, PointerButton, Pos2, Rect, pos2};
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::control::ControlClient;
use serde_json::{Value, json};

/// One 600 × 800 pt page with a line of text in the middle, so a stray page drag would select it.
const PAGE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length 44 >>
stream
BT /F1 24 Tf 100 400 Td (Guides here) Tj ET
endstream
endobj
5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn harness(tool: &'static str) -> (Harness<'static, PdfCraftApp>, ControlClient) {
    let slot: Arc<Mutex<Option<ControlClient>>> = Arc::default();
    let s = slot.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 900.0)).build_eframe(move |cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        *s.lock().unwrap() = Some(app.attach_control(&cc.egui_ctx));
        app.open_bytes("guides.pdf", None, PAGE.to_vec()).unwrap();
        for (k, v) in [("left", "closed"), ("panel", "none"), ("zoom", "50"), ("rulers", "on"), ("quick", tool)] {
            app.set_option(k, v).unwrap();
        }
        app
    });
    h.run_steps(8);
    let client = slot.lock().unwrap().take().unwrap();
    (h, client)
}

fn state(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient) -> Value {
    let rx = c.send("ui.state", json!({}));
    let mut reply = None;
    assert!(crate::support::step_until(h, |_| {
        reply = rx.try_recv().ok();
        reply.is_some()
    }));
    reply.unwrap().unwrap()["guides"]["document"].clone()
}

fn set(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, key: &str, value: &str) {
    let rx = c.send("ui.set", json!({ "key": key, "value": value }));
    let mut reply = None;
    assert!(crate::support::step_until(h, |_| {
        reply = rx.try_recv().ok();
        reply.is_some()
    }));
    reply.unwrap().unwrap_or_else(|e| panic!("ui.set {key}: {e}"));
    h.run_steps(2);
}

/// A primary-button drag from `from` to `to` in small steps, as a hand moves.
fn drag(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    for i in 1..=8 {
        h.hover_at(from + (to - from) * (i as f32 / 8.0));
        h.run_steps(1);
    }
    h.event(egui::Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn page(h: &Harness<'static, PdfCraftApp>) -> Rect {
    h.state().views[0].page_screen_rect(0).expect("the page is on screen")
}

/// The middle of the top ruler, above the page.
fn top_ruler(h: &Harness<'static, PdfCraftApp>) -> Pos2 {
    let viewport = h.state().views[0].viewport_rect();
    pos2(page(h).center().x, viewport.top() + pdfcraft_ui_egui::ruler::HORIZONTAL_RULER_HEIGHT / 2.0)
}

#[test]
fn a_drag_out_of_the_top_ruler_drops_a_horizontal_guide_where_it_is_released() {
    let (mut h, c) = harness("select");
    assert_eq!(state(&mut h, &c)["count"], 0);
    let p = page(&h);
    let drop = pos2(p.center().x, p.top() + 100.0);
    let ruler = top_ruler(&h);
    drag(&mut h, ruler, drop);
    let doc = state(&mut h, &c);
    assert_eq!(doc["count"], 1, "{doc}");
    let line = &doc["on_screen"][0];
    assert_eq!((line["page"].as_u64(), line["axis"].as_str(), line["every_page"].as_bool()), (Some(1), Some("horizontal"), Some(false)));
    let y = line["screen"][0][1].as_f64().unwrap();
    assert!((y - f64::from(drop.y)).abs() < 1.5, "the guide lies where it was dropped: {y} against {}", drop.y);
    // 100 screen points below the top edge, in points at the page's zoom (600 pt wide on screen).
    let expected = 100.0 * 600.0 / f64::from(p.width());
    let at = line["from_top_or_left_pt"].as_f64().unwrap();
    assert!((at - expected).abs() < 1.0, "{at} pt from the top, not {expected}");
}

/// A press on a guide belongs to the guide: with the Hand tool a drag there moves the guide and
/// does not pan the page, and dropping it on the ruler removes it.
#[test]
fn a_guide_takes_the_drag_from_the_page_and_a_drop_on_the_ruler_removes_it() {
    let (mut h, c) = harness("hand");
    set(
        &mut h,
        &c,
        "document-guides",
        r#"{"format": "pdfcraft-guides", "version": 1, "name": "test", "unit": "pt", "guide": [{"axis": "horizontal", "at": 200, "pages": "all"}]}"#,
    );
    let doc = state(&mut h, &c);
    assert_eq!(doc["count"], 1, "{doc}");
    let y = doc["on_screen"][0]["screen"][0][1].as_f64().unwrap() as f32;
    let before = page(&h);
    let x = before.center().x;
    drag(&mut h, pos2(x, y), pos2(x, y + 60.0));
    assert_eq!(page(&h), before, "the Hand tool did not pan the page: the guide had the press");
    let moved = state(&mut h, &c);
    let new_y = moved["on_screen"][0]["screen"][0][1].as_f64().unwrap() as f32;
    assert!((new_y - (y + 60.0)).abs() < 1.5, "the guide moved with the drag: {y} to {new_y}");
    let ruler = top_ruler(&h);
    drag(&mut h, pos2(x, new_y), ruler);
    assert_eq!(state(&mut h, &c)["count"], 0, "dropped on the ruler, the guide is gone");
    assert_eq!(page(&h), before);
}

/// Where a drag across the line of text starts and ends: at 150 pt from the left edge, through the
/// middle of the text, 80 screen points to the right.
fn across_the_text(h: &Harness<'static, PdfCraftApp>) -> (Pos2, Pos2) {
    let p = page(h);
    let scale = p.width() / 600.0;
    let from = pos2(p.left() + 150.0 * scale, p.top() + 392.0 * scale);
    (from, from + egui::vec2(80.0, 0.0))
}

/// With the Select tool a drag across the text selects it, but the same drag starting on a guide
/// moves the guide and selects nothing: the page leaves a press on a guide to the guide.
#[test]
fn a_drag_that_starts_on_a_guide_selects_no_text() {
    let (mut h, _c) = harness("select");
    h.run_steps(8);
    let (from, to) = across_the_text(&h);
    drag(&mut h, from, to);
    assert!(h.state().views[0].selected_text().is_some_and(|t| !t.trim().is_empty()), "the same drag without a guide selects text");

    let (mut h, c) = harness("select");
    h.run_steps(8);
    set(
        &mut h,
        &c,
        "document-guides",
        r#"{"format": "pdfcraft-guides", "version": 1, "name": "test", "unit": "pt", "guide": [{"axis": "vertical", "at": 150, "pages": [1]}]}"#,
    );
    let (from, to) = across_the_text(&h);
    drag(&mut h, from, to);
    assert_eq!(h.state().views[0].selected_text(), None, "a drag on the guide selects no text");
    let doc = state(&mut h, &c);
    let x = doc["on_screen"][0]["screen"][0][0].as_f64().unwrap() as f32;
    assert!((x - to.x).abs() < 1.5, "the guide moved with the drag to {x}, not {}", to.x);
}
