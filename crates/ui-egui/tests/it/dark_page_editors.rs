//! Dark-page editing through the real shell, including focus, caret and draft lifetime.

use egui::{Color32, Pos2, Rect};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_render::ReadingColors;
use pdfcraft_ui_egui::PdfCraftApp;

#[derive(Clone, Copy, Debug)]
enum Mode {
    Original,
    Dark,
    OnlyBlack,
    Yellow,
}

fn harness(form: bool) -> Harness<'static, PdfCraftApp> {
    harness_bytes(if form { include_bytes!("../data/form.pdf").to_vec() } else { mixed_text() })
}

fn harness_bytes(bytes: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("editors.pdf", None, bytes).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    settle(&mut h);
    h
}

fn mixed_text() -> Vec<u8> {
    text_fixture("BT /F1 18 Tf 20 300 Td (Black ) Tj 0 0.2 0.5 rg (Blue) Tj ET")
}

fn text_fixture(body: &str) -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_owned(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer << /Root 1 0 R /Size 6 >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    bytes
}

fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    for _ in 0..200 {
        h.run_steps(2);
        if !h.state().render_pending() {
            h.run_steps(2);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("editor preview did not settle");
}

fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let view = &h.state().views[0];
    let page = &h.state().session.get(view.id).unwrap().info.pages[0];
    let rect = view.page_screen_rect(0).unwrap();
    egui::pos2(rect.left() + x / page.width * rect.width(), rect.bottom() - y / page.height * rect.height())
}

fn click(h: &mut Harness<'static, PdfCraftApp>, point: Pos2) {
    h.hover_at(point);
    h.run_steps(1);
    h.drag_at(point);
    h.run_steps(1);
    h.drop_at(point);
    h.run_steps(3);
}

fn mode(h: &mut Harness<'static, PdfCraftApp>, mode: Mode) {
    let reading = match mode {
        Mode::OnlyBlack => ReadingColors::parse("white-on-black", None, None, Some(true), Some(false)).unwrap(),
        Mode::Yellow => ReadingColors::YELLOW_ON_BLACK,
        _ => ReadingColors::default(),
    };
    h.state_mut().set_reading_colors(reading);
    let dark = !matches!(mode, Mode::Original);
    if h.state().views[0].dark_pages != dark {
        assert!(h.state_mut().execute("view.dark_pages"));
    }
    settle(h);
}

fn focused(h: &Harness<'static, PdfCraftApp>, text: &str) -> Rect {
    let node = h
        .query_all_by_role(egui::accesskit::Role::TextInput)
        .chain(h.query_all_by_role(egui::accesskit::Role::MultilineTextInput))
        .find(|node| node.is_focused())
        .expect("editor keeps focus");
    assert_eq!(node.value().as_deref(), Some(text));
    node.rect()
}

fn assert_editor(h: &mut Harness<'static, PdfCraftApp>, text: &str, mode: Mode) {
    h.ctx.all_styles_mut(|style| style.visuals.text_cursor.blink = false);
    h.run_steps(1);
    let rect = focused(h, text);
    let expected_text = match mode {
        Mode::Original => Color32::BLACK,
        Mode::Dark => Color32::from_gray(230),
        Mode::OnlyBlack => Color32::WHITE,
        Mode::Yellow => Color32::YELLOW,
    };
    let colors: Vec<_> = h
        .output()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text_shape) if text_shape.galley.text() == text => {
                Some(text_shape.galley.job.sections.iter().map(|s| s.format.color).collect::<Vec<_>>())
            }
            _ => None,
        })
        .flatten()
        .collect();
    assert!(colors.contains(&expected_text), "actual editable text follows {mode:?}: {colors:?}");
    let expected_caret = if matches!(mode, Mode::Original) { Color32::BLACK } else { Color32::WHITE };
    assert!(
        h.output().shapes.iter().any(|shape| match &shape.shape {
            egui::Shape::LineSegment { points, stroke } =>
                stroke.color == expected_caret
                    && (points[0].x - points[1].x).abs() < 0.1
                    && (points[0].y - points[1].y).abs() > 4.0
                    && points.iter().all(|p| rect.expand(3.0).contains(*p)),
            _ => false,
        }),
        "caret is visible against the editor background in {mode:?}"
    );
    let pixels = h.render().unwrap();
    let point = rect.right_bottom() - egui::vec2(5.0, 5.0);
    let paper = pixels.get_pixel(point.x as u32, point.y as u32).0;
    assert!(
        paper[..3].iter().all(|channel| if matches!(mode, Mode::Original) { *channel > 230 } else { *channel < 60 }),
        "editor paper follows {mode:?}: {paper:?}"
    );
    if let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
        pixels.save(format!("{dir}/focused-{text}-{mode:?}.png")).unwrap();
    }
}

#[test]
fn formatting_text_colors_never_flashes_the_editor_background() {
    let _gpu = crate::gpu();
    for display in [Mode::Original, Mode::Dark, Mode::Yellow] {
        let mut h = harness_bytes(text_fixture("0.08 0.1 0.12 rg 10 280 260 50 re f BT /F1 18 Tf 20 300 Td 1 g (Color test) Tj ET"));
        mode(&mut h, display);
        assert!(h.state_mut().execute("edit.edit_text"));
        h.run_steps(2);
        let point = at(&h, 40.0, 306.0);
        click(&mut h, point);
        settle(&mut h);
        let editor = focused(&h, "Color test");
        let sample = editor.right_bottom() - egui::vec2(5.0, 5.0);
        let before = h.render().unwrap().get_pixel(sample.x as u32, sample.y as u32).0;
        for (label, color) in [("Gray", [0.5, 0.5, 0.5]), ("Light blue", [0.36, 0.75, 0.98]), ("Black", [0.0; 3])] {
            // The colours are in the palette the Colour button opens.
            h.get_by_label("Colour").click();
            h.run_steps(2);
            h.get_by_label(label).click();
            // Check the commit frame and the frames before any replacement raster settles.
            for frame in 0..8 {
                h.run_steps(1);
                let editor = focused(&h, "Color test");
                let shot = h.render().unwrap();
                let after = shot.get_pixel(sample.x as u32, sample.y as u32).0;
                assert_eq!(after, before, "{label} must retain the page background in {display:?}");
                // The paragraph stays in sight while the page is drawn again with its new format:
                // the editor's own text, the look it was applied with, or the new page raster.
                let ink = (editor.left() as u32..editor.right() as u32)
                    .flat_map(|x| (editor.top() as u32..editor.bottom() as u32).map(move |y| (x, y)))
                    .filter(|&(x, y)| {
                        let p = shot.get_pixel(x, y).0;
                        p[..3].iter().zip(&before[..3]).any(|(a, b)| a.abs_diff(*b) >= 12)
                    })
                    .count();
                assert!(ink > 30, "{label}, frame {frame}: the paragraph vanished in {display:?} ({ink} ink pixels)");
            }
            let doc = h.state().session.get(h.state().views[0].id).unwrap();
            assert_eq!(doc.text_blocks(0)[0].color, color);
            assert_eq!(doc.can_undo(), Some("Edit text"));
        }
        if let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
            h.render().unwrap().save(format!("{dir}/text-color-background-{display:?}.png")).unwrap();
        }
        h.state_mut().undo();
        settle(&mut h);
        assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().text_blocks(0)[0].color, [0.36, 0.75, 0.98]);
    }
}

#[test]
fn dark_pages_focused_added_and_fill_text_keep_drafts_contrast_and_cancel() {
    let _gpu = crate::gpu();
    for command in ["edit.text", "sign.fill.text"] {
        let mut h = harness(false);
        assert!(h.state_mut().execute(command));
        h.run_steps(2);
        let point = at(&h, 30.0, 180.0);
        click(&mut h, point);
        h.event(egui::Event::Text("Draft".into()));
        h.run_steps(2);
        let source = h.state().session.get(h.state().views[0].id).unwrap().bytes.clone();
        for display in [Mode::Dark, Mode::Original, Mode::OnlyBlack, Mode::Yellow, Mode::Dark] {
            mode(&mut h, display);
            assert_editor(&mut h, "Draft", display);
            let doc = h.state().session.get(h.state().views[0].id).unwrap();
            assert_eq!(doc.bytes, source);
            assert!(!doc.dirty && doc.can_undo().is_none());
        }
        h.key_press(egui::Key::Escape);
        h.run_steps(3);
        assert!(h.state().views[0].content.draft.is_none() && h.state().views[0].fill_text.is_none());
        assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().bytes, source);
        click(&mut h, point);
        h.event(egui::Event::Text("Applied".into()));
        h.run_steps(2);
        if command == "edit.text" {
            h.get_by_label("Done adding text").click();
            h.run_steps(3);
        } else {
            let away = at(&h, 30.0, 120.0);
            click(&mut h, away);
        }
        let doc = h.state().session.get(h.state().views[0].id).unwrap();
        assert!(doc.dirty && doc.can_undo().is_some());
        if command == "edit.text" {
            assert!(doc.text_blocks(0).iter().any(|b| b.text.contains("Applied")));
        } else {
            assert!(doc.info.annotations.iter().any(|a| a.contents.as_deref() == Some("Applied")));
        }
    }
}

#[test]
fn dark_pages_focused_form_keeps_draft_contrast_tab_and_escape() {
    let _gpu = crate::gpu();
    let mut h = harness(true);
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    let field = doc.form.iter().find(|f| f.name == "name").unwrap();
    let rect = pdfcraft_ui_egui::forms_ui::field_screen_rect(&h.state().views[0], &doc.info, field, 0).unwrap();
    click(&mut h, rect.center());
    h.event(egui::Event::Text("Draft".into()));
    h.run_steps(2);
    let source = h.state().session.get(h.state().views[0].id).unwrap().bytes.clone();
    for display in [Mode::Dark, Mode::Original, Mode::OnlyBlack, Mode::Yellow] {
        mode(&mut h, display);
        assert_editor(&mut h, "Draft", display);
        assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().bytes, source);
    }
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.state().views[0].forms.focus.is_none());
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().bytes, source);
    click(&mut h, rect.center());
    h.event(egui::Event::Text("Accepted".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(3);
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert_eq!(doc.form.iter().find(|f| f.name == "name").unwrap().value, ["Accepted"]);
    assert!(doc.dirty && doc.can_undo().is_some());
    assert_eq!(h.state().views[0].forms.focus.as_ref().unwrap().name, "agree", "Tab moves to the next field");
}

#[test]
fn dark_pages_existing_mixed_text_keeps_source_styles_and_applies_once() {
    let _gpu = crate::gpu();
    let mut h = harness(false);
    h.state_mut().execute("edit.edit_text");
    h.run_steps(2);
    let point = at(&h, 40.0, 306.0);
    click(&mut h, point);
    assert_eq!(h.state().views[0].line_editor.as_ref().unwrap().text, "Black Blue");
    let source = h.state().session.get(h.state().views[0].id).unwrap().bytes.clone();
    for display in [Mode::Dark, Mode::Original, Mode::OnlyBlack, Mode::Yellow] {
        mode(&mut h, display);
        assert_editor(&mut h, "Black Blue", display);
        if matches!(display, Mode::OnlyBlack) {
            assert!(h.output().shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(t) if t.galley.text() == "Black Blue" && t.galley.job.sections.iter().any(|s| s.format.color == Color32::from_rgb(0, 51, 128)))), "only-black preserves colored source spans");
        }
        assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().bytes, source);
    }
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.state().views[0].line_editor.is_none());
    // Esc leaves the paragraph with its box selected; a second Esc clears that, so the click
    // opens the text again.
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    click(&mut h, point);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("Changed".into()));
    h.run_steps(2);
    mode(&mut h, Mode::Dark);
    assert_eq!(h.state().views[0].line_editor.as_ref().unwrap().text, "Changed");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Enter);
    h.run_steps(3);
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert_eq!(doc.text_blocks(0)[0].text, "Changed");
    assert_eq!(doc.can_undo(), Some("Edit text"));
    h.state_mut().undo();
    h.run_steps(3);
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().text_blocks(0)[0].text, "Black Blue");
}

#[test]
fn dark_pages_pale_source_text_and_caret_remain_visible_on_a_banner() {
    let _gpu = crate::gpu();
    let mut h = harness_bytes(text_fixture("0 0 0 rg 10 280 200 60 re f BT 1 1 1 rg /F1 18 Tf 20 300 Td (Pale) Tj ET"));
    h.state_mut().execute("edit.edit_text");
    h.run_steps(2);
    let point = at(&h, 40.0, 306.0);
    click(&mut h, point);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("Pale draft".into()));
    h.run_steps(2);
    let source = h.state().session.get(h.state().views[0].id).unwrap().bytes.clone();
    for display in [Mode::Dark, Mode::Original, Mode::Dark] {
        mode(&mut h, display);
        h.ctx.all_styles_mut(|s| s.visuals.text_cursor.blink = false);
        h.run_steps(1);
        let rect = focused(&h, "Pale draft");
        let color = if matches!(display, Mode::Dark) { Color32::from_gray(25) } else { Color32::WHITE };
        assert!(h.output().shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == "Pale draft" && t.galley.job.sections.iter().all(|s| s.format.color == color))));
        let caret = if matches!(display, Mode::Dark) { Color32::BLACK } else { Color32::WHITE };
        assert!(h.output().shapes.iter().any(|s| matches!(&s.shape, egui::Shape::LineSegment { points, stroke } if stroke.color == caret && points.iter().all(|p| rect.expand(3.0).contains(*p)))));
        let shot = h.render().unwrap();
        let paper = at(&h, 180.0, 325.0);
        let pixel = shot.get_pixel(paper.x as u32, paper.y as u32).0;
        assert!(
            pixel[..3].iter().all(|v| if matches!(display, Mode::Dark) { *v > 215 } else { *v < 10 }),
            "banner background stays visible: {pixel:?}"
        );
        if let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
            shot.save(format!("{dir}/pale-editor-{display:?}.png")).unwrap();
        }
        assert_eq!(h.state().views[0].line_editor.as_ref().unwrap().look.color, [1.0, 1.0, 1.0]);
        assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().bytes, source);
    }
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
}
