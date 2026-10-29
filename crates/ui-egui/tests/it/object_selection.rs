//! Multi-object editing through the real shell. Synthetic artwork and standard-font text only.
use egui::{Key, Modifiers, Pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::{AddedText, Edit, EditableObject};
use pdfcraft_ui_egui::PdfCraftApp;

fn fixture(clipping: bool) -> Vec<u8> {
    fixture_with_background(clipping, false)
}

fn fixture_with_background(clipping: bool, background: bool) -> Vec<u8> {
    fixture_with_text(clipping, background, "40 430 Td (Group headline) Tj")
}

fn fixture_with_text(clipping: bool, background: bool, text: &str) -> Vec<u8> {
    let stream = |body: &str, dict: &str| format!("<< {dict} /Length {} >>\nstream\n{body}\nendstream", body.len());
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 500] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Figure 6 0 R >> >> >>"
            .into(),
        stream(
            &format!(
                "{}q BT {} Tr /F1 18 Tf {text} ET Q q 1 0 0 1 40 340 cm /Figure Do Q BT /F1 12 Tf 240 90 Td (Keep here) Tj ET",
                if background { "q /Background Do Q " } else { "" },
                if clipping { 4 } else { 0 }
            ),
            "",
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        stream("1 0.2 0.1 rg 0 0 60 30 re f", "/Type /XObject /Subtype /Form /BBox [0 0 60 30]"),
    ];
    if background {
        objects[2] = objects[2].replace("/Figure 6 0 R", "/Figure 6 0 R /Background 7 0 R");
        objects.push(stream("1 1 1 rg 0 0 400 500 re f", "/Type /XObject /Subtype /Form /BBox [0 0 400 500]"));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

fn harness() -> Harness<'static, PdfCraftApp> {
    harness_with(false)
}

fn harness_with(clipping: bool) -> Harness<'static, PdfCraftApp> {
    harness_document(fixture(clipping), false)
}

fn harness_document(bytes: Vec<u8>, capture: bool) -> Harness<'static, PdfCraftApp> {
    let mut builder = Harness::builder().with_size(egui::vec2(1400.0, 900.0));
    if capture {
        builder = builder.wgpu();
    }
    let mut h = builder.build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.session = pdfcraft_engine::Session::new().with_clock(|| 1_700_000_000);
        app.set_option("language", "en").unwrap();
        app.open_bytes("group.pdf", None, bytes).unwrap();
        let id = app.views[0].id;
        app.session
            .apply(
                id,
                Edit::AddText { page: 0, text: AddedText { text: "Added note".into(), rect: [130.0, 340.0, 230.0, 370.0], ..Default::default() } },
            )
            .unwrap();
        let saved = app.session.save_bytes(id).unwrap();
        app.session.mark_saved(id, saved, None).unwrap();
        app.set_option("zoom", "100").unwrap();
        assert!(app.execute("edit.edit_text"));
        app
    });
    crate::support::settle(&mut h);
    h
}

fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).unwrap();
    r.min + egui::vec2(x, y) * (r.width() / 400.0)
}
fn objects(h: &Harness<'static, PdfCraftApp>) -> Vec<EditableObject> {
    let app = h.state();
    app.session.get(app.views[0].id).unwrap().editable_objects(0).unwrap()
}
fn click(h: &mut Harness<'static, PdfCraftApp>, p: Pos2, mods: Modifiers) {
    h.hover_at(p);
    h.run_steps(2);
    for pressed in [true, false] {
        h.event_modifiers(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: mods }, mods);
    }
    h.run_steps(4);
}
fn marquee(h: &mut Harness<'static, PdfCraftApp>) {
    let (a, b) = (at(h, 25.0, 45.0), at(h, 235.0, 175.0));
    h.hover_at(a);
    h.run_steps(2);
    h.drag_at(a);
    h.run_steps(2);
    h.hover_at(b);
    h.run_steps(2);
    h.drop_at(b);
    h.run_steps(4);
    h.get_by_label("3 objects selected");
}
fn drag(h: &mut Harness<'static, PdfCraftApp>, a: Pos2, b: Pos2) {
    h.hover_at(a);
    h.run_steps(2);
    h.drag_at(a);
    h.run_steps(2);
    h.hover_at(b);
    h.run_steps(2);
    h.drop_at(b);
    h.run_steps(4);
}

#[test]
fn marquee_and_mixed_group_drag_move_once_and_undo_together() {
    let mut h = harness();
    let before = objects(&h);
    marquee(&mut h);
    let (a, b) = (at(&h, 55.0, 70.0), at(&h, 80.0, 100.0));
    drag(&mut h, a, b);
    let after = objects(&h);
    for old in &before {
        let new = after.iter().find(|o| o.target == old.target).unwrap();
        let shift = if old.text.as_deref() == Some("Keep here") { [0.0, 0.0] } else { [25.0, -30.0] };
        for i in 0..4 {
            assert!((new.rect[i] - old.rect[i] - shift[i % 2]).abs() < 0.01, "{old:?} -> {new:?}");
        }
    }
    h.get_by_label("3 objects selected");
    let app = h.state();
    assert_eq!(app.session.get(app.views[0].id).unwrap().can_undo(), Some("Move objects"));
    if let Ok(path) = std::env::var("PDFCRAFT_GROUP_SHOT") {
        let _gpu = crate::gpu();
        h.render().unwrap().save(path).unwrap();
    }
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(4);
    assert_eq!(objects(&h), before);
    assert!(
        h.query_all_by_label_contains("object selected").next().is_none() && h.query_all_by_label_contains("objects selected").next().is_none(),
        "no objects selected"
    );
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run_steps(4);
    assert_eq!(objects(&h), after);
}

#[test]
fn modifier_clicks_extend_an_ordinary_selection_and_toggle_objects() {
    let mut h = harness();
    let p = at(&h, 55.0, 70.0);
    click(&mut h, p, Modifiers::NONE);
    assert!(h.state().views[0].line_editor.is_some());
    let p = at(&h, 60.0, 145.0);
    click(&mut h, p, Modifiers::SHIFT);
    h.get_by_label("2 objects selected");
    assert!(h.state().views[0].line_editor.is_none());
    let p = at(&h, 155.0, 140.0);
    click(&mut h, p, Modifiers::COMMAND);
    h.get_by_label("3 objects selected");
    let p = at(&h, 60.0, 145.0);
    click(&mut h, p, Modifiers::CTRL);
    h.get_by_label("2 objects selected");
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(
        h.query_all_by_label_contains("object selected").next().is_none() && h.query_all_by_label_contains("objects selected").next().is_none(),
        "no objects selected"
    );
}

#[test]
fn escaping_a_group_drag_and_switching_tools_do_not_edit() {
    let mut h = harness();
    let before = objects(&h);
    marquee(&mut h);
    let (a, b) = (at(&h, 55.0, 70.0), at(&h, 90.0, 110.0));
    h.hover_at(a);
    h.run_steps(2);
    h.drag_at(a);
    h.run_steps(2);
    h.hover_at(b);
    h.run_steps(2);
    assert_eq!(objects(&h), before, "only a preview while held");
    h.key_press(Key::Escape);
    h.run_steps(2);
    h.drop_at(b);
    h.run_steps(4);
    assert_eq!(objects(&h), before);
    marquee(&mut h);
    assert!(h.state_mut().execute("edit.text"));
    h.run_steps(4);
    assert_eq!(objects(&h), before);
}

#[test]
fn a_changed_text_editor_is_kept_when_group_selection_is_requested() {
    let mut h = harness();
    let before = objects(&h);
    let p = at(&h, 55.0, 70.0);
    click(&mut h, p, Modifiers::NONE);
    h.state_mut().views[0].line_editor.as_mut().unwrap().text = "A draft still being edited".into();
    let p = at(&h, 60.0, 145.0);
    click(&mut h, p, Modifiers::SHIFT);
    assert_eq!(h.state().views[0].line_editor.as_ref().unwrap().text, "A draft still being edited");
    assert_eq!(objects(&h), before);
}

#[test]
fn a_document_change_invalidates_old_selection_before_the_next_drag() {
    let mut h = harness();
    marquee(&mut h);
    let id = h.state().views[0].id;
    h.state_mut()
        .session
        .apply(id, Edit::AddText { page: 0, text: AddedText { text: "A new item".into(), rect: [40.0, 210.0, 150.0, 240.0], ..Default::default() } })
        .unwrap();
    h.run_steps(4);
    assert!(
        h.query_all_by_label_contains("object selected").next().is_none() && h.query_all_by_label_contains("objects selected").next().is_none(),
        "no objects selected"
    );
    assert_eq!(objects(&h).len(), 5);
}

#[test]
fn refusing_one_object_keeps_the_whole_group_and_its_selection() {
    let mut h = harness_with(true);
    let before = objects(&h);
    marquee(&mut h);
    let (a, b) = (at(&h, 55.0, 70.0), at(&h, 80.0, 100.0));
    drag(&mut h, a, b);
    assert_eq!(objects(&h), before);
    h.get_by_label("3 objects selected");
    assert!(h.state().toast.as_ref().unwrap().0.contains("Text used as a clipping path cannot be moved with this selection."));
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
}

#[test]
fn a_group_drag_released_beyond_the_page_finishes_once() {
    let mut h = harness();
    let before = objects(&h);
    marquee(&mut h);
    let (a, b) = (at(&h, 55.0, 70.0), at(&h, 415.0, 100.0));
    drag(&mut h, a, b);
    let after = objects(&h);
    for (old, new) in before.iter().zip(&after) {
        let delta = if old.text.as_deref() == Some("Keep here") { 0.0 } else { 360.0 };
        assert!((new.rect[0] - old.rect[0] - delta).abs() < 0.01);
    }
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(4);
    assert_eq!(objects(&h), before);
}

#[test]
fn group_selection_can_be_extended_by_an_additive_marquee() {
    let mut h = harness();
    let p = at(&h, 55.0, 70.0);
    click(&mut h, p, Modifiers::SHIFT);
    h.get_by_label("1 object selected");
    let (a, b) = (at(&h, 25.0, 120.0), at(&h, 235.0, 175.0));
    h.hover_at(a);
    h.run_steps(2);
    h.event_modifiers(
        egui::Event::PointerButton { pos: a, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::SHIFT },
        Modifiers::SHIFT,
    );
    h.run_steps(2);
    h.hover_at(b);
    h.run_steps(2);
    h.drop_at(b);
    h.run_steps(4);
    h.get_by_label("3 objects selected");
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
}

#[test]
fn group_drag_respects_document_rotation_and_view_rotation() {
    for document_rotation in [0, 90, 180, 270] {
        let mut h = harness();
        assert!(h.state_mut().apply_edit(Edit::RotatePages { pages: vec![0], degrees: document_rotation }));
        h.state_mut().views[0].rotate_view(true);
        h.run_steps(4);
        let before = objects(&h);
        let targets: Vec<_> = before.iter().filter(|o| o.text.as_deref() != Some("Keep here")).map(|o| o.target).collect();
        for target in &targets {
            let app = h.state();
            let doc = app.session.get(app.views[0].id).unwrap();
            let o = before.iter().find(|o| o.target == *target).unwrap();
            let p = pdfcraft_ui_egui::canvas::PageXform {
                rect: app.views[0].page_screen_rect(0).unwrap(),
                rot: 90,
                pw: doc.info.pages[0].width,
                ph: doc.info.pages[0].height,
            }
            .user_rect(&doc.info, 0, o.rect.map(|v| v as f32))
            .center();
            click(&mut h, p, Modifiers::SHIFT);
        }
        h.get_by_label("3 objects selected");
        let app = h.state();
        let doc = app.session.get(app.views[0].id).unwrap();
        let xf = pdfcraft_ui_egui::canvas::PageXform {
            rect: app.views[0].page_screen_rect(0).unwrap(),
            rot: 90,
            pw: doc.info.pages[0].width,
            ph: doc.info.pages[0].height,
        };
        let start =
            xf.user_rect(&doc.info, 0, before.iter().find(|o| o.text.as_deref() == Some("Group headline")).unwrap().rect.map(|v| v as f32)).center();
        let end = start + egui::vec2(21.0, 33.0);
        let from = xf.screen_to_view(start);
        let to = xf.screen_to_view(end);
        let delta = doc.object_move_offset(0, [f64::from(to.0 - from.0), f64::from(to.1 - from.1)]).unwrap();
        drag(&mut h, start, end);
        for (old, new) in before.iter().zip(objects(&h)) {
            let expected = if targets.contains(&old.target) { delta } else { [0.0, 0.0] };
            for i in 0..4 {
                assert!((new.rect[i] - old.rect[i] - expected[i % 2]).abs() < 0.01, "rotation {document_rotation}");
            }
        }
    }
}

#[test]
fn group_drags_cancel_on_focus_loss_tab_switch_home_and_modal_dialogs() {
    for case in 0..5 {
        let mut h = harness();
        let before = objects(&h);
        marquee(&mut h);
        let (a, b) = (at(&h, 55.0, 70.0), at(&h, 90.0, 110.0));
        h.hover_at(a);
        h.run_steps(2);
        h.drag_at(a);
        h.run_steps(2);
        h.hover_at(b);
        h.run_steps(2);
        match case {
            0 => h.input_mut().focused = false,
            1 => {
                h.state_mut().open_bytes("other.pdf", None, fixture(false)).unwrap();
            }
            2 => h.state_mut().active = None,
            3 => h.state_mut().dialog = Some(pdfcraft_ui_egui::Dialog::About),
            _ => h.state_mut().request_document_url("https://example.invalid/", pdfcraft_ui_egui::LinkOrigin::Link),
        }
        h.run_steps(2);
        h.drop_at(b);
        h.run_steps(2);
        h.input_mut().focused = true;
        h.state_mut().active = Some(0);
        h.state_mut().dialog = None;
        h.state_mut().pending_link = None;
        h.run_steps(4);
        assert_eq!(objects(&h), before, "cancelled gesture case {case}");
        assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty, "case {case}");
        h.get_by_label("3 objects selected");
    }
}

#[test]
fn modifier_selection_picks_foreground_text_and_added_items_over_background_artwork() {
    let mut h = harness_document(fixture_with_background(false, true), false);
    let before = objects(&h);
    assert_eq!(before.len(), 5);
    for (x, y) in [(55.0, 70.0), (150.0, 135.0), (60.0, 145.0)] {
        let p = at(&h, x, y);
        click(&mut h, p, Modifiers::SHIFT);
    }
    h.get_by_label("3 objects selected");
    let (a, b) = (at(&h, 55.0, 70.0), at(&h, 80.0, 100.0));
    drag(&mut h, a, b);
    let after = objects(&h);
    for old in before {
        let new = after.iter().find(|o| o.target == old.target).unwrap();
        let stationary = old.text.as_deref() == Some("Keep here") || (old.target.kind == pdfcraft_engine::ObjectKind::Image && old.target.index == 0);
        let shift = if stationary { [0.0, 0.0] } else { [25.0, -30.0] };
        assert!(new.rect.iter().enumerate().all(|(i, value)| (value - old.rect[i] - shift[i % 2]).abs() < 0.01), "{old:?} -> {new:?}");
    }
}

/// A taller headline frame surrounds the figure without changing either object's visible ink.
fn overlapping_frame_harness(rotation: i64, turn_view: bool, capture: bool) -> Harness<'static, PdfCraftApp> {
    let mut h = harness_document(fixture(false), capture);
    let id = h.state().views[0].id;
    let blocks = h.state().session.get(id).unwrap().text_blocks(0);
    let (block, headline) = blocks.iter().enumerate().find(|(_, b)| b.text == "Group headline").unwrap();
    let style = pdfcraft_engine::BlockStyle { vertical_frame: Some([300.0, 450.0]), ..Default::default() };
    assert!(h.state_mut().apply_edit(Edit::EditTextBlock { page: 0, block, text: headline.editor_text.clone(), style }));
    if rotation != 0 {
        assert!(h.state_mut().apply_edit(Edit::RotatePages { pages: vec![0], degrees: rotation }));
    }
    if turn_view {
        h.state_mut().views[0].rotate_view(true);
    }
    h.run_steps(4);
    let headline = h.state().session.get(id).unwrap().text_blocks(0).into_iter().find(|b| b.text == "Group headline").unwrap();
    let frame = headline.editing_rect();
    assert!(frame[0] <= 40.0 && frame[2] >= 100.0 && frame[1] < 340.0 && frame[3] > 370.0, "the image is inside the chosen frame");
    assert!(headline.rect[1] > 370.0, "the visible headline does not overlap the image");
    h
}

/// Locate synthetic PDF coordinates after both document and view rotations.
fn frame_point(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let app = h.state();
    let view = &app.views[0];
    let doc = app.session.get(view.id).unwrap();
    pdfcraft_ui_egui::canvas::PageXform {
        rect: view.page_screen_rect(0).unwrap(),
        rot: view.rotation,
        pw: doc.info.pages[0].width,
        ph: doc.info.pages[0].height,
    }
    .user_rect(&doc.info, 0, [x, y, x, y])
    .center()
}

/// Modifier selection must reach visible artwork rather than the surrounding empty text frame.
#[test]
fn modifier_click_reaches_an_image_inside_a_paragraph_frame() {
    for (rotation, turn_view) in [(0, false), (90, true)] {
        let mut h = overlapping_frame_harness(rotation, turn_view, false);
        let before = objects(&h);
        let image = frame_point(&h, 70.0, 355.0);
        click(&mut h, image, Modifiers::SHIFT);
        h.get_by_label("1 object selected");
        h.key_press(Key::Delete);
        h.run_steps(4);
        let expected: Vec<_> = before.iter().filter(|o| o.target.kind != pdfcraft_engine::ObjectKind::Image).cloned().collect();
        assert_eq!(objects(&h), expected, "only the clicked image is deleted, with rotation {rotation}");
        h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
        h.run_steps(4);
        assert_eq!(objects(&h), before, "the selected image's deletion is one undoable edit");
    }
}

/// Ordinary selection must also pass through the paragraph's empty interior to the image tool.
#[test]
fn plain_click_reaches_an_image_inside_a_paragraph_frame() {
    let snapshot = std::env::var_os("PDFCRAFT_FRAME_IMAGE_SHOT");
    let _gpu = snapshot.as_ref().map(|_| crate::gpu());
    let mut h = overlapping_frame_harness(0, false, snapshot.is_some());
    let before = objects(&h);
    let image = frame_point(&h, 70.0, 355.0);
    click(&mut h, image, Modifiers::NONE);
    assert!(h.state().views[0].image_selection.is_some(), "the visible image is selected");
    assert!(h.state().views[0].line_editor.is_none(), "the empty paragraph frame does not open its editor");
    assert_eq!(objects(&h), before);
    if let Some(path) = snapshot {
        for _ in 0..60 {
            h.run_steps(2);
            if !h.state().render_pending() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!h.state().render_pending(), "the capture shows the current document");
        h.render().unwrap().save(path).unwrap();
    }
}

/// A selected paragraph's expanded bounds cannot steal an unselected image's drag.
#[test]
fn dragging_an_image_inside_a_selected_paragraph_frame_keeps_the_paragraph_still() {
    for (rotation, turn_view) in [(0, false), (90, true)] {
        let mut h = overlapping_frame_harness(rotation, turn_view, false);
        let before = objects(&h);
        let headline = frame_point(&h, 75.0, 435.0);
        click(&mut h, headline, Modifiers::SHIFT);
        h.get_by_label("1 object selected");
        let (from, to) = (frame_point(&h, 70.0, 355.0), frame_point(&h, 95.0, 325.0));
        drag(&mut h, from, to);
        assert_eq!(objects(&h), before, "a first drag over the unselected image must not move the paragraph");
        // Existing image interaction selects on click, then moves on a subsequent drag.
        click(&mut h, from, Modifiers::NONE);
        assert!(h.state().views[0].image_selection.is_some());
        assert!(h.state().views[0].line_editor.is_none());
        drag(&mut h, from, to);
        let after = objects(&h);
        for old in &before {
            let new = after.iter().find(|o| o.target == old.target).unwrap();
            let shift = if old.target.kind == pdfcraft_engine::ObjectKind::Image { [25.0, -30.0] } else { [0.0, 0.0] };
            assert!(new.rect.iter().enumerate().all(|(i, value)| (value - old.rect[i] - shift[i % 2]).abs() < 0.01), "{old:?} -> {new:?}");
        }
    }
}

/// A marquee still uses the full frame, and its otherwise empty interior still moves the selection.
#[test]
fn marquee_selects_empty_paragraph_frame_space_and_the_selected_group_still_moves() {
    let mut h = overlapping_frame_harness(0, false, false);
    let before = objects(&h);
    let (from, to) = (frame_point(&h, 20.0, 290.0), frame_point(&h, 60.0, 320.0));
    drag(&mut h, from, to);
    h.get_by_label("1 object selected");
    let (from, to) = (frame_point(&h, 120.0, 320.0), frame_point(&h, 145.0, 290.0));
    drag(&mut h, from, to);
    let after = objects(&h);
    for old in &before {
        let new = after.iter().find(|o| o.target == old.target).unwrap();
        let shift = if old.text.as_deref() == Some("Group headline") { [25.0, -30.0] } else { [0.0, 0.0] };
        assert!(new.rect.iter().enumerate().all(|(i, value)| (value - old.rect[i] - shift[i % 2]).abs() < 0.01), "{old:?} -> {new:?}");
    }
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(4);
    assert_eq!(objects(&h), before);
    marquee(&mut h);
    let (from, to) = (frame_point(&h, 70.0, 355.0), frame_point(&h, 95.0, 325.0));
    drag(&mut h, from, to);
    let after = objects(&h);
    for old in &before {
        let new = after.iter().find(|o| o.target == old.target).unwrap();
        let shift = if old.text.as_deref() == Some("Keep here") { [0.0, 0.0] } else { [25.0, -30.0] };
        assert!(new.rect.iter().enumerate().all(|(i, value)| (value - old.rect[i] - shift[i % 2]).abs() < 0.01), "{old:?} -> {new:?}");
    }
    h.get_by_label("3 objects selected");
}

/// Conservation control: the image's cropped-away area still belongs to the empty paragraph frame.
#[test]
fn a_cropped_image_leaves_its_hidden_area_available_to_the_paragraph_frame() {
    let mut h = overlapping_frame_harness(0, false, false);
    assert!(h.state_mut().apply_edit(Edit::EditPageImage {
        page: 0,
        index: 0,
        change: pdfcraft_engine::ImageEdit::Crop([80.0, 340.0, 100.0, 370.0])
    }));
    h.run_steps(4);
    let before = objects(&h);
    let hidden = frame_point(&h, 70.0, 355.0);
    click(&mut h, hidden, Modifiers::NONE);
    assert!(h.state().views[0].line_editor.is_some(), "clipped-away artwork does not block the empty frame");
    assert!(h.state().views[0].image_selection.is_none());
    h.key_press(Key::Escape);
    h.run_steps(4);
    click(&mut h, hidden, Modifiers::SHIFT);
    h.get_by_label("1 object selected");
    h.key_press(Key::ArrowRight);
    h.run_steps(4);
    let after = objects(&h);
    for old in &before {
        let new = after.iter().find(|o| o.target == old.target).unwrap();
        let shift = if old.text.as_deref() == Some("Group headline") { [1.0, 0.0] } else { [0.0, 0.0] };
        assert!(new.rect.iter().enumerate().all(|(i, value)| (value - old.rect[i] - shift[i % 2]).abs() < 0.01), "{old:?} -> {new:?}");
    }
    let visible = frame_point(&h, 90.0, 355.0);
    click(&mut h, visible, Modifiers::NONE);
    assert!(h.state().views[0].image_selection.is_some(), "the remaining visible strip is still selectable");
    assert!(h.state().views[0].line_editor.is_none());
}

/// Reserve blank space around short text at 30 degrees. Return a world-axis bounding-box
/// corner outside both the oriented frame (including its edge grips) and the visible ink.
fn oriented_frame_corner_harness(rotation: i64, turn_view: bool, capture: bool) -> (Harness<'static, PdfCraftApp>, [f32; 2], [f32; 2]) {
    let (sin, cos) = 30.0_f64.to_radians().sin_cos();
    let content = format!("{cos:.9} {sin:.9} {:.9} {cos:.9} 180 230 Tm (Turned) Tj", -sin);
    let mut h = harness_document(fixture_with_text(false, false, &content), capture);
    let id = h.state().views[0].id;
    let blocks = h.state().session.get(id).unwrap().text_blocks(0);
    let (block, turned) = blocks.iter().enumerate().find(|(_, b)| b.text == "Turned").unwrap();
    let top = turned.ink_frame[3];
    let style = pdfcraft_engine::BlockStyle { width: Some(160.0), vertical_frame: Some([top - 120.0, top]), ..Default::default() };
    assert!(h.state_mut().apply_edit(Edit::EditTextBlock { page: 0, block, text: turned.editor_text.clone(), style }));
    if rotation != 0 {
        assert!(h.state_mut().apply_edit(Edit::RotatePages { pages: vec![0], degrees: rotation }));
    }
    if turn_view {
        h.state_mut().views[0].rotate_view(true);
    }
    h.run_steps(4);
    let turned = h.state().session.get(id).unwrap().text_blocks(0).into_iter().find(|b| b.text == "Turned").unwrap();
    let [left, bottom, right, top] = turned.editing_rect();
    let corner = [left + 12.0, bottom + 12.0];
    assert!(corner[0] < right && corner[1] < top, "the point is inside the world-axis frame bounds");
    assert!(corner[1] < turned.rect[1] - 20.0, "the point is well outside the ink box");
    let frame = turned.editing_frame();
    assert!(corner[0] * cos + corner[1] * sin < frame[0] - 20.0, "the point is beyond the oriented frame and edge grips");
    let local = [(frame[0] + frame[2]) / 2.0, (frame[1] + frame[3]) / 2.0];
    let interior = [local[0] * cos - local[1] * sin, local[0] * sin + local[1] * cos];
    (h, corner.map(|v| v as f32), interior.map(|v| v as f32))
}

/// A modifier click outside the rotated frame must not toggle the paragraph through its AABB.
#[test]
fn oriented_frame_corner_modifier_click_is_empty_but_the_interior_selects_text() {
    for (rotation, turn_view) in [(0, false), (90, true)] {
        let (mut h, corner, interior) = oriented_frame_corner_harness(rotation, turn_view, false);
        let before = objects(&h);
        let p = frame_point(&h, corner[0], corner[1]);
        click(&mut h, p, Modifiers::SHIFT);
        assert!(h.query_all_by_label_contains("object selected").next().is_none(), "blank corner does not select text");
        h.key_press(Key::ArrowRight);
        h.run_steps(4);
        assert_eq!(objects(&h), before, "an empty modifier click cannot move the paragraph with the arrow keys");
        let p = frame_point(&h, interior[0], interior[1]);
        click(&mut h, p, Modifiers::SHIFT);
        h.get_by_label("1 object selected");
        h.key_press(Key::Delete);
        h.run_steps(4);
        assert!(!objects(&h).iter().any(|o| o.text.as_deref() == Some("Turned")), "the oriented interior still selects the paragraph");
        h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
        h.run_steps(4);
        assert_eq!(objects(&h), before);
    }
}

/// A plain drag starts a marquee in the blank corner. Marquee intersection and an already
/// selected group's drag still use rectangular bounds, so both paragraphs move together.
#[test]
fn oriented_frame_corner_starts_a_marquee_and_keeps_selected_union_dragging() {
    let snapshot = std::env::var_os("PDFCRAFT_ORIENTED_CORNER_SHOT");
    let _gpu = snapshot.as_ref().map(|_| crate::gpu());
    for (rotation, turn_view) in [(0, false), (90, true)] {
        let capture = rotation == 0 && snapshot.is_some();
        let (mut h, corner, _) = oriented_frame_corner_harness(rotation, turn_view, capture);
        let before = objects(&h);
        let (from, to) = (frame_point(&h, corner[0], corner[1]), frame_point(&h, 310.0, 75.0));
        drag(&mut h, from, to);
        h.get_by_label("2 objects selected");
        assert_eq!(objects(&h), before, "the marquee selects without editing either paragraph");
        if capture {
            for _ in 0..60 {
                h.run_steps(2);
                if !h.state().render_pending() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(!h.state().render_pending(), "the capture shows the current document");
            h.render().unwrap().save(snapshot.as_ref().unwrap()).unwrap();
        }
        let to = frame_point(&h, corner[0] + 20.0, corner[1] - 15.0);
        drag(&mut h, from, to);
        let after = objects(&h);
        for old in &before {
            let new = after.iter().find(|o| o.target == old.target).unwrap();
            let shift = if old.target.kind == pdfcraft_engine::ObjectKind::Text { [20.0, -15.0] } else { [0.0, 0.0] };
            assert!(new.rect.iter().enumerate().all(|(i, value)| (value - old.rect[i] - shift[i % 2]).abs() < 0.01), "{old:?} -> {new:?}");
        }
        h.get_by_label("2 objects selected");
    }
}
