//! End-to-end touch screen interactions in the document viewer and Preferences.

use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Pos2, TouchDeviceId, TouchId, TouchPhase, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{PdfCraftApp, TouchMode};

use crate::support::input::one_frame;

fn text_pdf() -> Vec<u8> {
    let body = "BT /F1 14 Tf 20 320 Td (The quick brown fox) Tj 0 -24 Td (jumps over the lazy dog) Tj ET";
    format!(
        "%PDF-1.7\n\
         1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 /MediaBox [0 0 300 400] >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj\n\
         4 0 obj << /Length {} >> stream\n{body}\nendstream endobj\n\
         5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >> endobj\n\
         6 0 obj << /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj\n\
         trailer << /Size 7 /Root 1 0 R >>\n\
         %%EOF",
        body.len()
    )
    .into_bytes()
}

fn square_pdf() -> Vec<u8> {
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [5 0 R] >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
        "<< /Type /Annot /Subtype /Square /Rect [80 220 180 320] /C [0.9 0.2 0.2] >>".into(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.drain(..).enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    out
}

fn touch_harness(pdf: Vec<u8>, options: &[(&str, &str)]) -> Harness<'static, PdfCraftApp> {
    let opts: Vec<(String, String)> = options.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect();
    let mut h = Harness::builder().with_size(vec2(1000.0, 700.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.set_option("left", "closed").unwrap();
        app.open_bytes("touch.pdf", None, pdf).unwrap();
        for (k, v) in opts {
            app.set_option(&k, &v).unwrap();
        }
        app
    });
    crate::support::settle(&mut h);
    h
}

fn touch_down(p: Pos2) -> [Event; 3] {
    [
        Event::Touch { device_id: TouchDeviceId(1), id: TouchId(1), phase: TouchPhase::Start, pos: p, force: None },
        Event::PointerMoved(p),
        Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
    ]
}

fn touch_move(p: Pos2) -> [Event; 2] {
    [Event::Touch { device_id: TouchDeviceId(1), id: TouchId(1), phase: TouchPhase::Move, pos: p, force: None }, Event::PointerMoved(p)]
}

fn touch_up(p: Pos2) -> [Event; 3] {
    [
        Event::Touch { device_id: TouchDeviceId(1), id: TouchId(1), phase: TouchPhase::End, pos: p, force: None },
        Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE },
        Event::PointerGone,
    ]
}

#[test]
fn preferences_choose_touch_mode_and_control_option_works() {
    let mut h = Harness::builder().with_size(vec2(1200.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.set_option("dialog", "preferences").unwrap();
        app
    });
    h.run_steps(4);
    assert_eq!(h.state().touch_mode, TouchMode::Auto);
    h.get_by_label("Touch mode");

    for (label, expected) in [("Always", TouchMode::Always), ("Never", TouchMode::Never), ("Auto-detect", TouchMode::Auto)] {
        h.get_by_role_and_label(Role::RadioButton, label).scroll_to_me();
        h.run_steps(3);
        h.get_by_role_and_label(Role::RadioButton, label).click();
        h.run_steps(2);
        assert_eq!(h.state().touch_mode, expected, "{label}");
    }

    h.state_mut().set_option("touch_mode", "ALWAYS").unwrap();
    assert_eq!(h.state().touch_mode, TouchMode::Always);
    let saved = h.state().persist();
    assert!(saved.contains(r#""touch_mode":"always""#), "{saved}");

    let mut restored = PdfCraftApp::new();
    restored.restore(&saved);
    assert_eq!(restored.touch_mode, TouchMode::Always);

    let mut invalid = PdfCraftApp::new();
    invalid.restore(&saved.replace(r#""touch_mode":"always""#, r#""touch_mode":"invalid""#));
    assert_eq!(invalid.touch_mode, TouchMode::Auto);
    assert!(invalid.set_option("touch_mode", "invalid").unwrap_err().contains("auto, always, or never"));
}

#[test]
fn touch_drag_over_text_in_select_mode_pans_with_fling_and_selects_nothing() {
    let mut h = touch_harness(text_pdf(), &[("layout", "continuous"), ("zoom", "200"), ("animation_effects", "on"), ("touch_mode", "auto")]);
    crate::support::wait_until(&mut h, "text layer", |app| app.views[0].glyph_screen_pos(0, 5).is_some());

    let start = h.state().views[0].glyph_screen_pos(0, 5).expect("glyph 5 pos");
    let top_before = h.state().views[0].page_screen_rect(0).expect("page 0").min.y;

    one_frame(&mut h, touch_down(start));
    let mut cur = start;
    for _ in 0..8 {
        cur.y -= 28.0;
        one_frame(&mut h, touch_move(cur));
    }
    assert_eq!(h.state().views[0].selected_text(), None, "swiping over text in Select tool must not select characters");

    one_frame(&mut h, touch_up(cur));
    let top_at_lift = h.state().views[0].page_screen_rect(0).expect("page 0").min.y;
    assert!(top_at_lift < top_before - 50.0, "touch drag scrolled page: before={top_before}, lift={top_at_lift}");
    assert_eq!(h.state().views[0].selected_text(), None, "lifting after a touch pan must leave no selection or handles behind");

    if !cfg!(any(target_os = "macos", target_arch = "wasm32")) {
        h.run_steps(6);
        let top_after_fling = h.state().views[0].page_screen_rect(0).expect("page 0").min.y;
        assert!(top_after_fling < top_at_lift - 2.0, "fling should glide after lift: lift={top_at_lift}, glide={top_after_fling}");

        // Touching down stops the active fling immediately.
        one_frame(&mut h, touch_down(start));
        let top_stopped = h.state().views[0].page_screen_rect(0).expect("page 0").min.y;
        one_frame(&mut h, touch_up(start));
        h.run_steps(4);
        let top_settled = h.state().views[0].page_screen_rect(0).expect("page 0").min.y;
        assert!((top_settled - top_stopped).abs() < 2.0, "new touch stops fling: stopped={top_stopped}, settled={top_settled}");
    }
}

#[test]
fn touch_long_press_selects_word_keeps_handles_after_lift_and_handle_drag_extends_selection() {
    let mut h = touch_harness(text_pdf(), &[("layout", "continuous"), ("zoom", "150"), ("touch_mode", "auto")]);
    crate::support::wait_until(&mut h, "text layer", |app| app.views[0].glyph_screen_pos(0, 18).is_some());

    // Glyph 5 is 'u' in "quick" ("The quick brown fox").
    let press_pos = h.state().views[0].glyph_screen_pos(0, 5).expect("u of quick");
    one_frame(&mut h, touch_down(press_pos));
    // Hold steady past LONG_PRESS_SECS (0.35s) at 1/60s per step (30 steps = 0.50s).
    for _ in 0..30 {
        one_frame(&mut h, touch_move(press_pos));
    }
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick"), "long-press selects word under finger");

    // Lift finger and let idle frames run: selection and touch UI handles remain available.
    one_frame(&mut h, touch_up(press_pos));
    h.run_steps(5);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick"), "lifting finger keeps long-pressed word selection");

    // Glyph 8 is 'k' at the end of "quick"; End handle pin sits 9 pt below its bottom-right edge.
    let end_glyph = h.state().views[0].glyph_screen_rect(0, 8).expect("k of quick");
    let end_pin = pos2(end_glyph.right(), end_glyph.bottom() + 9.0);
    // Glyph 18 is 'x' in "fox".
    let fox_end = h.state().views[0].glyph_screen_pos(0, 18).expect("x of fox");
    let page_top_before = h.state().views[0].page_screen_rect(0).expect("page 0").min.y;

    one_frame(&mut h, touch_down(end_pin));
    for step in 1..=6 {
        let t = step as f32 / 6.0;
        let p = end_pin.lerp(fox_end, t);
        one_frame(&mut h, touch_move(p));
    }
    one_frame(&mut h, touch_up(fox_end));
    h.run_steps(2);

    let selected = h.state().views[0].selected_text().unwrap_or_default();
    assert!(selected.contains("quick brown"), "dragging end handle should extend selection, got {selected:?}");
    let page_top_after = h.state().views[0].page_screen_rect(0).expect("page 0").min.y;
    assert!((page_top_after - page_top_before).abs() < 1.0, "dragging handle must not pan page");
}

#[test]
fn touch_drag_on_movable_comment_moves_comment_without_panning_page() {
    let mut h = touch_harness(square_pdf(), &[("layout", "continuous"), ("zoom", "200"), ("touch_mode", "auto")]);
    let r0 = h.state().views[0].page_screen_rect(0).expect("page 0");
    let k = r0.width() / 300.0;
    let center = pos2(r0.left() + 130.0 * k, r0.top() + (400.0 - 270.0) * k);
    let doc_id = h.state().views[0].id;
    let rect_before = h.state().session.get(doc_id).unwrap().info.annotations[0].rect;
    let top_before = r0.min.y;

    let target = center + vec2(0.0, 40.0 * k);
    one_frame(&mut h, touch_down(center));
    for step in 1..=6 {
        one_frame(&mut h, touch_move(center.lerp(target, step as f32 / 6.0)));
    }
    one_frame(&mut h, touch_up(target));
    h.run_steps(4);

    let rect_after = h.state().session.get(doc_id).unwrap().info.annotations[0].rect;
    let top_after = h.state().views[0].page_screen_rect(0).expect("page 0").min.y;
    assert_ne!(rect_after, rect_before, "comment rect should move with touch drag");
    assert!((top_after - top_before).abs() < 1.0, "moving a comment must not pan the page: before={top_before}, after={top_after}");
}
