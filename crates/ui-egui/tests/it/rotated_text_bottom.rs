use super::{editor_point, fixture, fixture_editor_harness, open_fixture_editor};
use egui_kittest::{Harness, kittest::Queryable};
use pdfcraft_ui_egui::PdfCraftApp;

fn label(angle: i32, y: f64, text: &str) -> Vec<u8> {
    use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream, write_incremental};
    let x = if angle == 180 { 580.0 } else { 100.0 };
    let mut doc = Document::open(std::sync::Arc::new(fixture(angle, x, false))).unwrap();
    doc.update_dict(ObjRef::new(2, 0), |d| d.set(b"MediaBox".to_vec(), Object::Array(vec![0, 0, 600, 1200].into_iter().map(Object::Int).collect())))
        .unwrap();
    let matrix = if angle == 180 { "-1 0 0 -1" } else { "0 1 -1 0" };
    doc.set(
        ObjRef::new(5, 0),
        Object::Stream(Stream::from_raw(Dict::new(), format!("BT /F1 14 Tf {matrix} {x} {y} Tm ({text}) Tj ET").into_bytes())),
    );
    write_incremental(&doc, &Default::default()).unwrap()
}

fn geometry(h: &Harness<'_, PdfCraftApp>, text: &str) {
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    let block = &doc.text_blocks(0)[0];
    let page = h.state().views[0].page_screen_rect(0).unwrap();
    let info = &doc.info.pages[0];
    let clip = page.intersect(h.state().views[0].viewport_rect());
    let field = h.get_by_label("Text editing line").rect();
    let shape = h
        .output()
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text() == text => Some(t),
            _ => None,
        })
        .unwrap();
    assert!(shape.galley.rows.len() > 1, "the unchanged label wraps");
    assert!(clip.expand(1.0).contains_rect(field), "the complete wrapped field is on-page from its first painted frame: {field:?} vs {clip:?}");
    // Placement must use exactly the height that TextEdit renders, not a differently styled measurement.
    let height = shape.galley.rect.height();
    assert!((field.height() - height).abs() < 1.0, "measured field height equals rendered galley height");
    let source_top = info.user_to_view(block.rect[0] as f32, block.rect[3] as f32)[1] * (page.width() / info.width) + page.top();
    let expected_top = source_top.clamp(clip.top(), (clip.bottom() - height.min(clip.height())).max(clip.top()));
    assert!((field.top() - expected_top).abs() < 1.0, "placement uses the rendered wrapped height without a frame of lag");
    for row in &shape.galley.rows {
        let rect = egui::Rect::from_min_size(shape.pos + row.pos.to_vec2(), row.size);
        assert!(clip.expand(1.0).contains_rect(rect) && field.expand(1.0).contains_rect(rect), "every row stays visible: {rect:?}");
    }
    fn cover(shape: &egui::Shape, field: egui::Rect) -> bool {
        match shape {
            egui::Shape::Vec(v) => v.iter().any(|s| cover(s, field)),
            egui::Shape::Rect(r) => {
                r.fill.a() == 255 && r.rect.min.distance(field.expand(1.0).min) < 1.0 && r.rect.max.distance(field.expand(1.0).max) < 1.0
            }
            _ => false,
        }
    }
    assert!(h.output().shapes.iter().any(|s| cover(&s.shape, field)), "the field cover follows the moved field");
    assert!(
        h.output().shapes.iter().any(|s| match &s.shape {
            egui::Shape::Rect(r) =>
                r.stroke.width == 1.5
                    && (r.rect.top() - (field.top() - 2.0)).abs() < 1.0
                    && (r.rect.bottom() - (field.top() + height + 2.0)).abs() < 1.0,
            _ => false,
        }),
        "the frame stroke follows the measured height and position"
    );
}

fn wrapped_near_bottom(angle: i32) {
    let shots = std::env::var_os("PDFCRAFT_ROTATED_TEXT_SHOTS").is_some();
    let _gpu = shots.then(crate::gpu);
    let text = "Margin label ".repeat(7);
    // Derive the visible-page bottom from the real layout, rather than assuming toolbar sizes.
    let probe = open_fixture_editor(label(180, 1000.0, &text), "200");
    let doc = probe.state().session.get(probe.state().views[0].id).unwrap();
    let page = probe.state().views[0].page_screen_rect(0).unwrap();
    let clip = page.intersect(probe.state().views[0].viewport_rect());
    let info = &doc.info.pages[0];
    let scale = page.width() / info.width;
    let desired_y = f64::from(info.view_to_user(0.0, (clip.bottom() - 40.0 - page.top()) / scale)[1]);
    let y = if angle == 90 {
        let mut inspect = pdfcraft_engine::Session::new();
        let id = inspect.open("measure.pdf", None, std::sync::Arc::new(label(90, 0.0, &text)), None).unwrap();
        let rect = inspect.get(id).unwrap().text_blocks(0)[0].rect;
        desired_y - (rect[3] - rect[1])
    } else {
        desired_y
    };
    let bytes = label(angle, y, &text);
    let mut h = fixture_editor_harness(bytes.clone(), "200");
    let at = editor_point(&h);
    h.hover_at(at);
    h.run_steps(1);
    let mut checked = 0;
    for _ in 0..4 {
        crate::support::input::one_frame(
            &mut h,
            [
                egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
                egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE },
            ],
        );
        if h.query_by_label("Text editing line").is_some() {
            geometry(&h, &text);
            checked += 1;
        }
    }
    assert!(checked > 0, "actual double clicks open and paint the editor");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::End);
    h.run_steps(2);
    geometry(&h, &text);
    let field = h.get_by_label("Text editing line").rect();
    let shape = h
        .output()
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text() == text => Some(t),
            _ => None,
        })
        .unwrap();
    let id = h.ctx.memory(|m| m.focused()).unwrap();
    let state = egui::TextEdit::load_state(&h.ctx, id).unwrap();
    let cursor = state.cursor.char_range().unwrap().primary;
    assert_eq!(usize::from(cursor.index), text.chars().count());
    let caret = shape.galley.pos_from_cursor(cursor).translate(shape.pos.to_vec2());
    let clip = h.state().views[0].page_screen_rect(0).unwrap().intersect(h.state().views[0].viewport_rect());
    assert!(clip.expand(1.0).contains_rect(caret) && field.expand(1.0).contains_rect(caret), "the end caret is on-page and inside the field");
    assert!(!h.state().views[0].line_editor.as_ref().unwrap().has_unsaved_text());
    assert_eq!(h.state().views[0].line_editor.as_ref().unwrap().style(), pdfcraft_engine::BlockStyle::default());
    let doc_id = h.state().views[0].id;
    assert!(!h.state().session.get(doc_id).unwrap().dirty);
    if shots {
        h.render().unwrap().save(format!("/tmp/5771-bottom-{angle}.png")).unwrap();
    }
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Enter);
    h.run_steps(3);
    assert!(h.state().views[0].line_editor.is_none());
    assert!(h.state().session.get(doc_id).unwrap().can_undo().is_none());
    assert_eq!(h.state().session.save_bytes(doc_id).unwrap().as_slice(), bytes.as_slice());
}
#[test]
fn unchanged_wrapped_90_degree_label_stays_above_the_page_bottom() {
    wrapped_near_bottom(90)
}
#[test]
fn unchanged_wrapped_180_degree_label_stays_above_the_page_bottom() {
    wrapped_near_bottom(180)
}
