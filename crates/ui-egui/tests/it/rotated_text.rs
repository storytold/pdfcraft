use egui_kittest::{Harness, kittest::Queryable};
use pdfcraft_ui_egui::PdfCraftApp;

fn fixture(angle: i32, x: f64, two_rows: bool) -> Vec<u8> {
    use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream, write_incremental};
    let mut doc = Document::open(std::sync::Arc::new(super::fixture(1))).unwrap();
    doc.update_dict(ObjRef::new(2, 0), |d| d.set(b"MediaBox".to_vec(), Object::Array(vec![0, 0, 600, 800].into_iter().map(Object::Int).collect())))
        .unwrap();
    let matrix = match angle {
        90 => "0 1 -1 0",
        180 => "-1 0 0 -1",
        270 => "0 -1 1 0",
        _ => "1 0 0 1",
    };
    let second = if two_rows { " 0 -20 Td (Second row) Tj" } else { "" };
    let body = format!("BT /F1 14 Tf {matrix} {x} 400 Tm (Inscription 25/26) Tj{second} ET");
    doc.set(ObjRef::new(5, 0), Object::Stream(Stream::from_raw(Dict::new(), body.into_bytes())));
    write_incremental(&doc, &Default::default()).unwrap()
}

/// Open through the reporter's two double-clicks, without setting a draft directly.
fn open_editor(angle: i32, x: f64) -> Harness<'static, PdfCraftApp> {
    open_editor_rows(angle, x, false)
}

fn open_editor_rows(angle: i32, x: f64, two_rows: bool) -> Harness<'static, PdfCraftApp> {
    open_fixture_editor(fixture(angle, x, two_rows), "117")
}

fn open_fixture_editor(bytes: Vec<u8>, zoom: &str) -> Harness<'static, PdfCraftApp> {
    let zoom = zoom.to_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("rotated.pdf", None, bytes).unwrap();
        app.set_option("zoom", &zoom).unwrap();
        app
    });
    h.run_steps(6);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(3);
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    let block = doc.text_blocks(0)[0].clone();
    let page = h.state().views[0].page_screen_rect(0).unwrap();
    let info = &doc.info.pages[0];
    let point = |x: f64, y: f64| {
        let p = info.user_to_view(x as f32, y as f32);
        page.min + egui::vec2(p[0], p[1]) * (page.width() / info.width)
    };
    let source = egui::Rect::from_two_pos(point(block.rect[0], block.rect[1]), point(block.rect[2], block.rect[3]));
    let at = source.intersect(page).intersect(h.state().views[0].viewport_rect()).center();
    for _ in 0..2 {
        crate::support::input::double_click(&mut h, at);
        h.run_steps(3);
    }
    assert!(h.state().views[0].line_editor.is_some(), "real double clicks open the paragraph");
    h
}

const ORIGINAL: &str = "Inscription 25/26";
const DRAFT: &str = "Inscription 25/27";

fn assert_readable(h: &Harness<'_, PdfCraftApp>, text: &str) {
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    let page = h.state().views[0].page_screen_rect(0).unwrap();
    let scale = page.width() / doc.info.pages[0].width;
    let font_height = 14.0 * scale;
    let field = h.get_by_label("Text editing line").rect();
    assert!(field.width() > font_height * 6.0, "the field uses the source's long side: {field:?}");
    assert!(page.intersect(h.state().views[0].viewport_rect()).expand(1.0).contains_rect(field), "the field stays on its page: {field:?}");
    let shape = h
        .output()
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text() == text => Some(t),
            _ => None,
        })
        .expect("the source or typed text is painted in the horizontal editor");
    assert_eq!(shape.angle, 0.0, "the editor is deliberately horizontal; Apply preserves rotation");
    assert_eq!(shape.galley.rows.len(), 1, "this single source line is not stacked");
    let row = &shape.galley.rows[0];
    assert!(row.size.y > font_height * 0.8 && row.size.y < font_height * 1.5, "row height follows the 14pt font: {:?}", row.size);
    assert!(row.glyphs.windows(2).all(|pair| pair[1].pos.x > pair[0].pos.x), "glyph advances stay positive along the baseline");
    let baseline = row.glyphs[0].pos.y;
    assert!(baseline > font_height * 0.6 && baseline < font_height * 1.2, "the caret/glyph baseline uses the source ascent: {baseline}");
    assert!(row.visuals.mesh.vertices.iter().any(|v| v.color.a() != 0), "the unchanged horizontal editor also shows its text");
}

fn type_draft(h: &mut Harness<'static, PdfCraftApp>) {
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get_by_label("Text editing line").type_text(DRAFT);
    h.run_steps(4);
    assert_eq!(h.state().views[0].line_editor.as_ref().unwrap().text, DRAFT);
}

fn readable_editor(angle: i32, x: f64) {
    // Hold the GPU lock until after the harness drops its renderer, when taking manual shots.
    let shots = std::env::var_os("PDFCRAFT_ROTATED_TEXT_SHOTS").is_some();
    let _gpu = shots.then(crate::gpu);
    let mut h = open_editor(angle, x);
    assert_readable(&h, ORIGINAL);
    type_draft(&mut h);
    assert_readable(&h, DRAFT);
    if shots {
        h.render().unwrap().save(format!("/tmp/5763-editor-{angle}-{x}.png")).unwrap();
    }
}

/// These are positive controls: main already writes turned text correctly. Keep that guarantee
/// while fixing only the UI layout; validate the saved matrix by interpreting it on reopen.
fn apply_keeps_source_matrix(angle: i32) {
    let mut h = open_editor(angle, 100.0);
    type_draft(&mut h);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Enter);
    h.run_steps(4);
    assert!(h.state().views[0].line_editor.is_none(), "Apply succeeds");
    let id = h.state().views[0].id;
    let bytes = h.state().session.save_bytes(id).unwrap();
    let mut reopened = pdfcraft_engine::Session::new();
    let id = reopened.open("again.pdf", None, bytes, None).unwrap();
    let b = &reopened.get(id).unwrap().text_blocks(0)[0];
    assert_eq!(b.text, DRAFT);
    assert!((b.rotation.to_radians() - f64::from(angle).to_radians()).cos() > 0.999, "saved matrix keeps angle {angle}: {}", b.rotation);
}

#[test]
fn rotated_text_90_is_readable() {
    readable_editor(90, 100.0);
}
#[test]
fn rotated_text_180_is_readable() {
    readable_editor(180, 100.0);
}
#[test]
fn rotated_text_270_is_readable() {
    readable_editor(270, 100.0);
}
#[test]
fn rotated_text_field_stays_on_its_page_at_the_right_edge() {
    readable_editor(90, 590.0);
}
#[test]
fn rotated_text_90_apply_keeps_source_matrix() {
    apply_keeps_source_matrix(90);
}
#[test]
fn rotated_text_180_apply_keeps_source_matrix() {
    apply_keeps_source_matrix(180);
}
#[test]
fn rotated_text_270_apply_keeps_source_matrix() {
    apply_keeps_source_matrix(270);
}

fn two_rows_stay_readable(angle: i32) {
    let shots = std::env::var_os("PDFCRAFT_ROTATED_TEXT_SHOTS").is_some();
    let _gpu = shots.then(crate::gpu);
    let mut h = open_editor_rows(angle, 100.0, true);
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().text_blocks(0)[0].rows.len(), 2, "one source paragraph has two rows");
    for text in ["Inscription 25/26\nSecond row", "Inscription 25/27\nSecond row"] {
        if text.contains("25/27") {
            h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
            h.get_by_label("Text editing line").type_text(text);
            h.run_steps(4);
        }
        let doc = h.state().session.get(h.state().views[0].id).unwrap();
        let page = h.state().views[0].page_screen_rect(0).unwrap();
        let scale = page.width() / doc.info.pages[0].width;
        let field = h.get_by_label("Text editing line").rect();
        assert!(field.width() > 14.0 * scale * 6.0, "two rows use their baseline width");
        assert!(page.intersect(h.state().views[0].viewport_rect()).expand(1.0).contains_rect(field), "both rows stay on-page");
        let shape = h
            .output()
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == text => Some(t),
                _ => None,
            })
            .expect("both source rows share one editor");
        assert_eq!(shape.galley.rows.len(), 2);
        let rows = &shape.galley.rows;
        let row_text = |i: usize| rows[i].glyphs.iter().map(|g| g.chr).collect::<String>();
        assert_eq!(row_text(0).trim_end_matches('\n'), text.lines().next().unwrap(), "first row keeps reading order");
        assert_eq!(row_text(1), "Second row", "second row keeps reading order");
        let baseline = |i: usize| rows[i].pos.y + rows[i].glyphs[0].pos.y;
        assert!((baseline(1) - baseline(0) - 20.0 * scale).abs() < 1.5, "20pt source leading is preserved");
        assert!(rows[1].pos.y > rows[0].pos.y + rows[0].size.y, "rows do not overlap");
        for row in rows {
            assert!(row.size.y < 14.0 * scale * 1.5, "each row keeps the source font height");
            assert!(row.glyphs.windows(2).all(|p| p[1].pos.x > p[0].pos.x), "glyphs do not stack");
        }
    }
    if shots {
        h.render().unwrap().save(format!("/tmp/5763-editor-{angle}-two-rows.png")).unwrap();
    }
}

#[test]
fn rotated_text_90_two_rows_keep_order_spacing_and_page_bounds() {
    two_rows_stay_readable(90);
}
#[test]
fn rotated_text_270_two_rows_keep_order_spacing_and_page_bounds() {
    two_rows_stay_readable(270);
}

#[test]
fn unchanged_long_turned_text_wraps_with_a_visible_end_caret_and_no_edit() {
    let shots = std::env::var_os("PDFCRAFT_ROTATED_TEXT_SHOTS").is_some();
    let _gpu = shots.then(crate::gpu);
    use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream, write_incremental};
    let text = "Margin label ".repeat(9);
    let mut source = Document::open(std::sync::Arc::new(fixture(90, 100.0, false))).unwrap();
    source
        .update_dict(ObjRef::new(2, 0), |d| d.set(b"MediaBox".to_vec(), Object::Array(vec![0, 0, 300, 800].into_iter().map(Object::Int).collect())))
        .unwrap();
    source.set(
        ObjRef::new(5, 0),
        Object::Stream(Stream::from_raw(Dict::new(), format!("BT /F1 14 Tf 0 1 -1 0 100 60 Tm ({text}) Tj ET").into_bytes())),
    );
    let bytes = write_incremental(&source, &Default::default()).unwrap();
    let mut h = open_fixture_editor(bytes.clone(), "200");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::End);
    h.run_steps(3);
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
    assert!(shape.galley.rows.len() > 1, "unchanged long turned text wraps to its horizontal field");
    for row in &shape.galley.rows {
        for glyph in &row.glyphs {
            let rect =
                egui::Rect::from_min_size(shape.pos + row.pos.to_vec2() + egui::vec2(glyph.pos.x, 0.0), egui::vec2(glyph.advance_width, row.size.y));
            assert!(field.expand(1.5).contains_rect(rect), "every glyph is in the field: {rect:?} vs {field:?}");
        }
    }
    let focused = h.ctx.memory(|m| m.focused()).unwrap();
    let state = egui::TextEdit::load_state(&h.ctx, focused).unwrap();
    let cursor = state.cursor.char_range().unwrap().primary;
    assert_eq!(usize::from(cursor.index), text.chars().count(), "Ctrl+End reaches the tail without typing");
    let caret = shape.galley.pos_from_cursor(cursor).translate(shape.pos.to_vec2());
    assert!(field.expand(1.5).contains_rect(caret), "the end caret is visible: {caret:?} vs {field:?}");
    let ed = h.state().views[0].line_editor.as_ref().unwrap();
    assert!(!ed.has_unsaved_text());
    assert_eq!(ed.style(), pdfcraft_engine::BlockStyle::default());
    let id = h.state().views[0].id;
    assert!(!h.state().session.get(id).unwrap().dirty);
    if shots {
        h.render().unwrap().save("/tmp/5771-long-unchanged.png").unwrap();
    }
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Enter);
    h.run_steps(3);
    assert!(h.state().views[0].line_editor.is_none());
    assert!(h.state().session.get(id).unwrap().can_undo().is_none(), "wrapping is not an edit");
    assert_eq!(h.state().session.save_bytes(id).unwrap().as_slice(), bytes.as_slice(), "unchanged Apply writes no PDF changes");
}

#[test]
fn unchanged_turned_text_hides_its_source_before_any_typing() {
    let mut h = open_editor(90, 100.0);
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    let block = doc.text_blocks(0)[0].clone();
    let bytes = doc.bytes.clone();
    let page = h.state().views[0].page_screen_rect(0).unwrap();
    let info = &doc.info.pages[0];
    let point = |x: f64, y: f64| {
        let p = info.user_to_view(x as f32, y as f32);
        page.min + egui::vec2(p[0], p[1]) * (page.width() / info.width)
    };
    let source = egui::Rect::from_two_pos(point(block.rect[0], block.rect[1]), point(block.rect[2], block.rect[3]));
    fn hidden(shape: &egui::Shape, source: egui::Rect, ctx: &egui::Context) -> bool {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().any(|s| hidden(s, source, ctx)),
            egui::Shape::Rect(r) => {
                r.fill.a() == 255 && r.rect.contains_rect(source) && r.rect.min.distance(source.min) < 3.0 && r.rect.max.distance(source.max) < 3.0
            }
            egui::Shape::Mesh(m) => {
                ctx.tex_manager().read().meta(m.texture_id).is_some_and(|m| m.name == "text-editor-background")
                    && m.calc_bounds().contains_rect(source)
            }
            _ => false,
        }
    }
    for _ in 0..20 {
        assert!(
            h.output().shapes.iter().any(|s| hidden(&s.shape, source, &h.ctx)),
            "source is covered while pending or replaced by the textless raster when ready"
        );
        h.run_steps(1);
    }
    assert!(!h.state().views[0].line_editor.as_ref().unwrap().has_unsaved_text());
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().bytes, bytes);
}
