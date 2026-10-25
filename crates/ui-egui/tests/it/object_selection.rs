//! Multi-object editing through the real shell. Synthetic artwork and standard-font text only.
use egui::{Key, Modifiers, Pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_engine::{AddedText, Edit, EditableObject, ObjectKind};
use pdfcraft_ui_egui::PdfCraftApp;

fn fixture(clipping: bool) -> Vec<u8> {
    fixture_with_background(clipping, false, "")
}

fn fixture_with_background(clipping: bool, background: bool, artwork: &str) -> Vec<u8> {
    let stream = |body: &str, dict: &str| format!("<< {dict} /Length {} >>\nstream\n{body}\nendstream", body.len());
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 500] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Figure 6 0 R >> >> >>"
            .into(),
        stream(
            &format!(
                "{}q BT {} Tr /F1 18 Tf 40 430 Td (Group headline) Tj ET Q q 1 0 0 1 40 340 cm /Figure Do Q BT /F1 12 Tf 240 90 Td (Keep here) Tj ET{}",
                if background { "q /Background Do Q " } else { "" },
                if clipping { 4 } else { 0 },
                artwork
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
    harness_document(clipping, false, "")
}

fn harness_document(clipping: bool, background: bool, artwork: &'static str) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.session = pdfcraft_engine::Session::new().with_clock(|| 1_700_000_000);
        app.set_option("language", "en").unwrap();
        app.open_bytes("group.pdf", None, fixture_with_background(clipping, background, artwork)).unwrap();
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
/// The Edit panel shows no "{n} object(s) selected" line.
fn nothing_selected(h: &Harness<'static, PdfCraftApp>) -> bool {
    h.query_all_by_label_contains("object selected").next().is_none() && h.query_all_by_label_contains("objects selected").next().is_none()
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
fn shift_drag(h: &mut Harness<'static, PdfCraftApp>, a: Pos2, b: Pos2) {
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
    let mut h = harness_document(false, true, "");
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

#[test]
fn vector_paths_are_selected_by_their_painted_shape() {
    // A diagonal stroke from (300, 100) to (380, 180): its box is mostly empty page.
    let mut h = harness_document(false, false, " 0 0 1 RG 3 w 300 100 m 380 180 l S");
    let before = objects(&h);
    assert_eq!(before.len(), 5);
    let line = before.last().unwrap();
    assert_eq!((line.target.kind, line.content_type), (ObjectKind::Path, "path"));
    // Inside the line's box, away from its stroke, a press is on empty page.
    let p = at(&h, 370.0, 390.0);
    click(&mut h, p, Modifiers::NONE);
    assert!(nothing_selected(&h));
    click(&mut h, p, Modifiers::SHIFT);
    assert!(nothing_selected(&h));
    // A plain click on the stroke selects the line alone; a modifier click toggles it.
    let on_line = at(&h, 340.0, 360.0);
    click(&mut h, on_line, Modifiers::NONE);
    h.get_by_label("1 object selected");
    assert!(h.state().views[0].line_editor.is_none() && h.state().views[0].image_selection.is_none());
    click(&mut h, on_line, Modifiers::COMMAND);
    assert!(nothing_selected(&h));
    click(&mut h, on_line, Modifiers::SHIFT);
    h.get_by_label("1 object selected");
    click(&mut h, on_line, Modifiers::COMMAND);
    assert!(nothing_selected(&h));
    let (a, b) = (at(&h, 320.0, 300.0), at(&h, 395.0, 380.0));
    drag(&mut h, a, b);
    h.get_by_label("1 object selected");
    assert_eq!(objects(&h), before);
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
}

#[test]
fn a_vector_path_over_background_artwork_can_be_shift_clicked() {
    let mut h = harness_document(false, true, " 0 0 1 RG 3 w 300 100 m 380 180 l S");
    let on_line = at(&h, 340.0, 360.0);
    click(&mut h, on_line, Modifiers::SHIFT);
    h.get_by_label("1 object selected");
    // Off the stroke the press reaches the background Form, so the first one took the line.
    let p = at(&h, 370.0, 390.0);
    click(&mut h, p, Modifiers::SHIFT);
    h.get_by_label("2 objects selected");
}

#[test]
fn an_image_over_a_vector_panel_keeps_modifier_clicks() {
    // A panel (30..110 across, 120..170 down) under a second copy of the figure.
    let mut h = harness_document(false, false, " 0.8 0.9 1 rg 30 330 80 50 re f q 1 0 0 1 40 340 cm /Figure Do Q");
    assert_eq!(objects(&h).len(), 6);
    let p = at(&h, 60.0, 145.0);
    click(&mut h, p, Modifiers::SHIFT);
    h.get_by_label("1 object selected");
    // Had the first click taken the panel, this one would take it out again.
    let p = at(&h, 35.0, 125.0);
    click(&mut h, p, Modifiers::SHIFT);
    h.get_by_label("2 objects selected");
}

#[test]
fn a_page_covering_vector_background_stays_out_of_clicks_and_small_marquees() {
    // Painted last here; it is a background by its size, not its paint order.
    let mut h = harness_document(false, false, " 0.9 0.9 0.9 rg 0 0 400 500 re f");
    let before = objects(&h);
    assert_eq!(before.len(), 5);
    assert_eq!(before.last().unwrap().target.kind, pdfcraft_engine::ObjectKind::Path);
    // A plain click on it starts a marquee instead of selecting it.
    let p = at(&h, 300.0, 250.0);
    click(&mut h, p, Modifiers::NONE);
    assert!(nothing_selected(&h));
    // Around the figure only (40..100 across, 130..160 down).
    let (a, b) = (at(&h, 30.0, 120.0), at(&h, 110.0, 170.0));
    drag(&mut h, a, b);
    h.get_by_label("1 object selected");
    // A shift-press on empty page doesn't toggle the background...
    click(&mut h, p, Modifiers::SHIFT);
    h.get_by_label("1 object selected");
    // ...and an additive marquee picks it up only when drawn over most of the page.
    let (a, b) = (at(&h, 120.0, 120.0), at(&h, 240.0, 175.0));
    shift_drag(&mut h, a, b);
    h.get_by_label("2 objects selected");
    let (a, b) = (at(&h, 10.0, 10.0), at(&h, 395.0, 495.0));
    shift_drag(&mut h, a, b);
    h.get_by_label("5 objects selected");
    // Once selected, a shift-click on it takes it out again.
    click(&mut h, p, Modifiers::SHIFT);
    h.get_by_label("4 objects selected");
    assert_eq!(objects(&h), before);
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
}

fn near(a: [f64; 4], b: [f64; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01)
}
fn undo_label(h: &Harness<'static, PdfCraftApp>) -> Option<String> {
    let app = h.state();
    app.session.get(app.views[0].id).unwrap().can_undo().map(str::to_owned)
}
fn dirty(h: &Harness<'static, PdfCraftApp>) -> bool {
    let app = h.state();
    app.session.get(app.views[0].id).unwrap().dirty
}

#[test]
fn a_plain_drag_on_a_vector_path_moves_it_alone() {
    let mut h = harness_document(false, false, " 0 0 1 RG 3 w 300 100 m 380 180 l S");
    let before = objects(&h);
    // From a point on the stroke, 40 points left and 20 up.
    let (a, b) = (at(&h, 340.0, 360.0), at(&h, 300.0, 340.0));
    drag(&mut h, a, b);
    let after = objects(&h);
    for old in &before {
        let new = after.iter().find(|o| o.target == old.target).unwrap();
        let [dx, dy] = if old.target.kind == ObjectKind::Path { [-40.0, 20.0] } else { [0.0, 0.0] };
        assert!(near(new.rect, [old.rect[0] + dx, old.rect[1] + dy, old.rect[2] + dx, old.rect[3] + dy]), "{old:?} -> {new:?}");
    }
    assert_eq!(undo_label(&h).as_deref(), Some("Move object"));
    h.get_by_label("1 object selected");
}

#[test]
fn a_corner_handle_resizes_the_one_selected_path() {
    // A panel 300..360 across, 360..400 down.
    let mut h = harness_document(false, false, " 0.8 0.9 1 rg 300 100 60 40 re f");
    let before = objects(&h);
    let panel = before.last().unwrap().clone();
    assert_eq!(panel.target.kind, ObjectKind::Path);
    assert!(near(panel.rect, [300.0, 100.0, 360.0, 140.0]), "{panel:?}");
    let p = at(&h, 330.0, 380.0);
    click(&mut h, p, Modifiers::NONE);
    h.get_by_label("1 object selected");
    // Its top-right handle dragged past the opposite corner would turn it inside out: ignored.
    let (a, b) = (at(&h, 360.0, 360.0), at(&h, 280.0, 420.0));
    drag(&mut h, a, b);
    assert_eq!(objects(&h), before);
    assert!(!dirty(&h));
    h.get_by_label("1 object selected");
    // Dragged 20 points right and up: the bottom-left corner stays.
    let b = at(&h, 380.0, 340.0);
    drag(&mut h, a, b);
    let after = objects(&h);
    let resized = after.iter().find(|o| o.target == panel.target).unwrap();
    assert!(near(resized.rect, [300.0, 100.0, 380.0, 160.0]), "{resized:?}");
    for old in before.iter().filter(|o| o.target != panel.target) {
        assert!(after.iter().any(|o| o.target == old.target && near(o.rect, old.rect)), "{old:?}");
    }
    assert_eq!(undo_label(&h).as_deref(), Some("Resize object"));
    h.get_by_label("1 object selected");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(4);
    assert_eq!(objects(&h), before);
}

#[test]
fn delete_removes_the_selected_path_and_image_in_one_step() {
    let mut h = harness_document(false, false, " 0.8 0.9 1 rg 300 100 60 40 re f");
    let before = objects(&h);
    assert_eq!(before.len(), 5);
    // The figure, then the panel.
    for (x, y) in [(60.0, 145.0), (330.0, 380.0)] {
        let p = at(&h, x, y);
        click(&mut h, p, Modifiers::SHIFT);
    }
    h.get_by_label("2 objects selected");
    h.key_press(Key::Delete);
    h.run_steps(4);
    let after = objects(&h);
    assert_eq!(after.len(), 3);
    assert!(after.iter().all(|o| matches!(o.target.kind, ObjectKind::Text | ObjectKind::Added)), "{after:?}");
    assert_eq!(undo_label(&h).as_deref(), Some("Delete objects"));
    assert!(nothing_selected(&h));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(4);
    assert_eq!(objects(&h), before);
}

/// Images without a path (here Form artwork, the same object kind as a raster image) still go in
/// one splice, as the object_delete tool deletes them, not one rewrite of the page per image.
#[test]
fn delete_removes_selected_images_alone_in_one_splice() {
    // The figure again, 240..300 across and 270..300 down.
    let mut h = harness_document(false, false, " q 1 0 0 1 240 200 cm /Figure Do Q");
    let before = objects(&h);
    assert_eq!(before.iter().filter(|o| o.target.kind == ObjectKind::Image).count(), 2, "{before:?}");
    for (x, y) in [(60.0, 145.0), (270.0, 285.0)] {
        let p = at(&h, x, y);
        click(&mut h, p, Modifiers::SHIFT);
    }
    h.get_by_label("2 objects selected");
    h.key_press(Key::Delete);
    h.run_steps(4);
    let after = objects(&h);
    assert!(after.iter().all(|o| matches!(o.target.kind, ObjectKind::Text | ObjectKind::Added)), "{after:?}");
    assert_eq!(after.len(), before.len() - 2);
    assert_eq!(undo_label(&h).as_deref(), Some("Delete objects"));
    assert!(nothing_selected(&h));
    undo(&mut h);
    assert_eq!(objects(&h), before);
}

#[test]
fn delete_with_text_in_the_selection_explains_and_keeps_everything() {
    let mut h = harness_document(false, false, " 0.8 0.9 1 rg 300 100 60 40 re f");
    let before = objects(&h);
    // The headline, then the panel.
    for (x, y) in [(55.0, 70.0), (330.0, 380.0)] {
        let p = at(&h, x, y);
        click(&mut h, p, Modifiers::SHIFT);
    }
    h.get_by_label("2 objects selected");
    h.key_press(Key::Backspace);
    h.run_steps(4);
    assert_eq!(objects(&h), before);
    assert!(!dirty(&h));
    h.get_by_label("2 objects selected");
    assert!(h.state().toast.as_ref().unwrap().0.contains("Text and added items can't be deleted with this selection."));
}

#[test]
fn text_over_a_vector_path_still_opens_for_typing() {
    // A panel 30..230 across, 40..80 down, around the headline.
    let mut h = harness_document(false, false, " 0.9 0.9 0.6 rg 30 420 200 40 re f");
    assert_eq!(objects(&h).len(), 5);
    let p = at(&h, 55.0, 70.0);
    click(&mut h, p, Modifiers::NONE);
    assert!(h.state().views[0].line_editor.is_some());
    assert!(nothing_selected(&h));
    // Beside the headline the press reaches the panel.
    let p = at(&h, 210.0, 50.0);
    click(&mut h, p, Modifiers::NONE);
    assert!(h.state().views[0].line_editor.is_none());
    h.get_by_label("1 object selected");
}

/// A paragraph's rewrap handle reaches a few pixels past its box (#2276). Over a vector path the
/// press there still rewraps the paragraph instead of taking the path, as text wins over paths.
#[test]
fn a_rewrap_handle_over_a_vector_path_rewraps_the_paragraph() {
    // A panel 30..230 across, 40..80 down, around the headline.
    let mut h = harness_document(false, false, " 0.9 0.9 0.6 rg 30 420 200 40 re f");
    let before = objects(&h);
    let headline = before.iter().find(|o| o.text.as_deref() == Some("Group headline")).unwrap().rect;
    let panel = before.last().unwrap().clone();
    assert_eq!(panel.target.kind, ObjectKind::Path);
    // 3 px outside the paragraph's box (2 px outside its text), near its top: on the right handle.
    let y = (500.0 - headline[3] + 4.0) as f32;
    let grip = at(&h, headline[2] as f32, y) + egui::vec2(5.0, 0.0);
    let to = at(&h, (headline[0] + 75.0) as f32, y) + egui::vec2(5.0, 0.0);
    drag(&mut h, grip, to);
    assert_eq!(undo_label(&h).as_deref(), Some("Edit text"));
    let after = objects(&h);
    assert!(after.iter().any(|o| o.target == panel.target && near(o.rect, panel.rect)), "the panel stays: {after:?}");
    assert!(nothing_selected(&h));
}

#[test]
fn a_resized_stroke_stays_selected() {
    // The diagonal's box is 298.5..381.5 across and 318.5..401.5 down.
    let mut h = harness_document(false, false, " 0 0 1 RG 3 w 300 100 m 380 180 l S");
    let on_line = at(&h, 340.0, 360.0);
    click(&mut h, on_line, Modifiers::NONE);
    h.get_by_label("1 object selected");
    // Its top-right handle, 10 points to the right. The pen scales too, so the new box lands
    // only near the one asked for.
    let (a, b) = (at(&h, 381.5, 318.5), at(&h, 391.5, 318.5));
    drag(&mut h, a, b);
    let line = objects(&h).last().unwrap().clone();
    assert_eq!(line.target.kind, ObjectKind::Path);
    assert!((line.rect[0] - 298.5).abs() < 0.5 && (line.rect[2] - 391.5).abs() < 0.5, "{line:?}");
    assert_eq!(undo_label(&h).as_deref(), Some("Resize object"));
    h.get_by_label("1 object selected");
}

#[test]
fn a_small_selected_path_still_moves_from_its_middle() {
    // A 10-point square, 300..310 across and 390..400 down: every point in it is near a corner.
    let mut h = harness_document(false, false, " 0 0 1 rg 300 100 10 10 re f");
    let p = at(&h, 305.0, 395.0);
    click(&mut h, p, Modifiers::NONE);
    h.get_by_label("1 object selected");
    let b = at(&h, 335.0, 395.0);
    drag(&mut h, p, b);
    let square = objects(&h).last().unwrap().clone();
    assert!(near(square.rect, [330.0, 100.0, 340.0, 110.0]), "{square:?}");
    assert_eq!(undo_label(&h).as_deref(), Some("Move object"));
    h.get_by_label("1 object selected");
}

#[test]
fn a_hairline_rule_lengthens_without_turning_over_or_thickening() {
    // A rule 50..350 across at 200 down: its box is a point tall.
    let mut h = harness_document(false, false, " 0 0 0 RG 1 w 50 300 m 350 300 l S");
    let p = at(&h, 200.0, 200.0);
    click(&mut h, p, Modifiers::NONE);
    h.get_by_label("1 object selected");
    let rule = objects(&h).last().unwrap().clone();
    assert!(near(rule.rect, [49.5, 299.5, 350.5, 300.5]), "{rule:?}");
    // Its right-hand handle, 30 points to the right with a little drift down.
    let (a, b) = (at(&h, 352.0, 197.0), at(&h, 382.0, 199.0));
    drag(&mut h, a, b);
    let rule = objects(&h).last().unwrap().clone();
    // The pen grows by the square root of the stretch only.
    assert!((rule.rect[0] - 49.5).abs() < 0.2 && (rule.rect[2] - 380.5).abs() < 0.2, "{rule:?}");
    assert!(rule.rect[3] - rule.rect[1] < 1.5 && ((rule.rect[1] + rule.rect[3]) / 2.0 - 300.0).abs() < 0.01, "{rule:?}");
    assert_eq!(undo_label(&h).as_deref(), Some("Resize object"));
}

#[test]
fn delete_leaves_pages_to_the_page_grid_and_the_pages_panel() {
    for organize in [false, true] {
        let mut h = harness_document(false, false, " 0 0 1 RG 3 w 300 100 m 380 180 l S");
        assert!(h.state_mut().apply_edit(Edit::InsertBlankPage { at: 1, width: 400.0, height: 500.0 }));
        h.state_mut().right = Some(pdfcraft_ui_egui::RightPanel::Pages);
        h.run_steps(4);
        let before = objects(&h);
        let on_line = at(&h, 340.0, 360.0);
        click(&mut h, on_line, Modifiers::NONE);
        h.get_by_label("1 object selected");
        if organize {
            // The selection stays behind the page grid, out of sight.
            assert!(h.state_mut().execute("page.organize"));
        } else {
            // The Pages panel thumbnail: in the window, outside the document area.
            let (v, window) = (h.state().views[0].viewport_rect(), egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(1400.0, 900.0)));
            let thumb = h.get_all_by_label("Page 2").map(|n| n.rect()).find(|r| !r.intersects(v) && window.contains(r.center())).unwrap();
            h.hover_at(thumb.center());
        }
        h.run_steps(3);
        h.state_mut().views[0].select_pages(&[1]);
        h.run_steps(2);
        assert_eq!(h.state().views[0].target_pages(), vec![1], "organize: {organize}");
        h.key_press(Key::Delete);
        h.run_steps(4);
        let app = h.state();
        assert_eq!(app.session.get(app.views[0].id).unwrap().info.pages.len(), 1, "organize: {organize}");
        assert_eq!(undo_label(&h).as_deref(), Some("Delete page"), "organize: {organize}");
        assert_eq!(objects(&h), before, "organize: {organize}");
    }
}

#[test]
fn delete_and_backspace_in_a_text_field_leave_the_selection_alone() {
    let mut h = harness_document(false, false, " 0 0 1 RG 3 w 300 100 m 380 180 l S");
    let before = objects(&h);
    let on_line = at(&h, 340.0, 360.0);
    click(&mut h, on_line, Modifiers::NONE);
    h.get_by_label("1 object selected");
    // The page number box in the toolbar.
    assert!(h.state_mut().execute("view.focus_page_input"));
    h.run_steps(2);
    let page_input = egui::Id::new(("page-input", h.state().views[0].id.0));
    assert!(h.ctx.memory(|m| m.has_focus(page_input)));
    for key in [Key::Backspace, Key::Delete] {
        h.key_press(key);
        h.run_steps(3);
    }
    assert_eq!(objects(&h), before);
    assert!(!dirty(&h));
    h.get_by_label("1 object selected");
}

#[test]
fn delete_in_the_frame_of_an_undo_leaves_the_restored_objects_alone() {
    // A panel (the first path) and a diagonal line (the second), apart.
    let mut h = harness_document(false, false, " 0.8 0.9 1 rg 300 100 60 40 re f 0 0 1 RG 3 w 300 200 m 380 280 l S");
    let before = objects(&h);
    let panel = at(&h, 330.0, 380.0);
    click(&mut h, panel, Modifiers::NONE);
    h.get_by_label("1 object selected");
    h.key_press(Key::Delete);
    h.run_steps(4);
    assert_eq!(objects(&h).len(), before.len() - 1);
    // The line is now the first path. Undo makes the panel the first path again, in the same
    // frame as a Delete for the selection made before the undo.
    let on_line = at(&h, 340.0, 260.0);
    click(&mut h, on_line, Modifiers::NONE);
    h.get_by_label("1 object selected");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.key_press(Key::Delete);
    h.run_steps(4);
    assert_eq!(objects(&h), before);
}

fn right_click(h: &mut Harness<'static, PdfCraftApp>, p: Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(2);
}
fn menu_item(h: &mut Harness<'static, PdfCraftApp>, submenu: &str, item: &str) {
    h.get_by_label(submenu).hover();
    h.run_steps(3);
    h.get_by_label(item).click();
    h.run_steps(4);
}
fn disabled(h: &Harness<'static, PdfCraftApp>, label: &str) -> bool {
    h.get_by_label(label).accesskit_node().is_disabled()
}
fn fills(h: &Harness<'static, PdfCraftApp>) -> Vec<[f64; 3]> {
    let app = h.state();
    app.session.get(app.views[0].id).unwrap().page_paths(0).unwrap().iter().map(|p| p.fill_rgb.unwrap()).collect()
}
fn undo(h: &mut Harness<'static, PdfCraftApp>) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(4);
}

#[test]
fn bring_to_front_in_the_page_menu_restacks_the_path_and_keeps_it_selected() {
    // A pale panel (300..360 across, 360..400 down) partly under a blue one (320..380, 350..390).
    let mut h = harness_document(false, false, " 0.8 0.9 1 rg 300 100 60 40 re f 0 0 1 rg 320 110 60 40 re f");
    let before = objects(&h);
    let [pale, blue] = fills(&h).try_into().unwrap();
    // Beside the blue panel.
    let p = at(&h, 310.0, 390.0);
    click(&mut h, p, Modifiers::NONE);
    h.get_by_label("1 object selected");
    right_click(&mut h, p);
    assert!(!disabled(&h, "Arrange ⏵"));
    menu_item(&mut h, "Arrange ⏵", "Bring to front");
    assert_eq!(fills(&h), [blue, pale]);
    assert_eq!(undo_label(&h).as_deref(), Some("Bring to front"));
    h.get_by_label("1 object selected");
    // The selection followed the pale panel to its new place in the list.
    h.hover_at(p);
    h.run_steps(2);
    h.key_press(Key::Delete);
    h.run_steps(4);
    assert_eq!(fills(&h), [blue]);
    undo(&mut h);
    undo(&mut h);
    assert_eq!(objects(&h), before);
}

#[test]
fn align_left_in_the_page_menu_lines_up_the_selection_and_keeps_it() {
    // A panel 300..360 across, 360..400 down; the figure is 40..100 across.
    let mut h = harness_document(false, false, " 0.8 0.9 1 rg 300 100 60 40 re f");
    let before = objects(&h);
    let panel = at(&h, 330.0, 380.0);
    for p in [at(&h, 60.0, 145.0), panel] {
        click(&mut h, p, Modifiers::SHIFT);
    }
    h.get_by_label("2 objects selected");
    right_click(&mut h, panel);
    // Distributing takes three objects, arranging one.
    assert!(disabled(&h, "Distribute ⏵") && disabled(&h, "Arrange ⏵"));
    menu_item(&mut h, "Align ⏵", "Left");
    let after = objects(&h);
    for old in &before {
        let new = after.iter().find(|o| o.target == old.target).unwrap();
        let dx = if old.target.kind == ObjectKind::Path { -260.0 } else { 0.0 };
        assert!(near(new.rect, [old.rect[0] + dx, old.rect[1], old.rect[2] + dx, old.rect[3]]), "{old:?} -> {new:?}");
    }
    assert_eq!(undo_label(&h).as_deref(), Some("Align objects"));
    // Again: the engine's refusal is said, and the selection stays.
    right_click(&mut h, panel);
    menu_item(&mut h, "Align ⏵", "Left");
    assert!(h.state().toast.as_ref().unwrap().0.contains("Align objects failed: the objects are already aligned"));
    assert_eq!(objects(&h), after);
    // The figure is still selected along with the panel: Delete takes both.
    h.get_by_label("2 objects selected");
    h.hover_at(panel);
    h.run_steps(2);
    h.key_press(Key::Delete);
    h.run_steps(4);
    assert!(objects(&h).iter().all(|o| matches!(o.target.kind, ObjectKind::Text | ObjectKind::Added)));
    undo(&mut h);
    undo(&mut h);
    assert_eq!(objects(&h), before);
}

#[test]
fn delete_in_the_page_menu_removes_the_selection_in_one_step() {
    let mut h = harness_document(false, false, " 0.8 0.9 1 rg 300 100 60 40 re f");
    let before = objects(&h);
    let panel = at(&h, 330.0, 380.0);
    for p in [at(&h, 60.0, 145.0), panel] {
        click(&mut h, p, Modifiers::SHIFT);
    }
    h.get_by_label("2 objects selected");
    right_click(&mut h, panel);
    h.get_all_by_label("Delete").last().unwrap().click();
    h.run_steps(4);
    let after = objects(&h);
    assert_eq!(after.len(), 3);
    assert!(after.iter().all(|o| matches!(o.target.kind, ObjectKind::Text | ObjectKind::Added)), "{after:?}");
    assert_eq!(undo_label(&h).as_deref(), Some("Delete objects"));
    assert!(nothing_selected(&h));
    undo(&mut h);
    assert_eq!(objects(&h), before);
}

/// The menu's Delete acts whenever the key does: on text, but not on text with a path.
#[test]
fn the_page_menu_leaves_text_out_of_arrange_and_deletes_it_only_without_paths() {
    let mut h = harness_document(false, false, " 0.8 0.9 1 rg 300 100 60 40 re f");
    let before = objects(&h);
    let (headline, panel) = (at(&h, 55.0, 70.0), at(&h, 330.0, 380.0));
    click(&mut h, headline, Modifiers::SHIFT);
    h.get_by_label("1 object selected");
    right_click(&mut h, headline);
    assert!(disabled(&h, "Arrange ⏵") && disabled(&h, "Align ⏵"));
    h.get_all_by_label("Delete").last().unwrap().click();
    h.run_steps(4);
    let after = objects(&h);
    assert!(after.iter().all(|o| o.text.as_deref() != Some("Group headline")), "{after:?}");
    assert_eq!(after.len(), before.len() - 1);
    assert!(nothing_selected(&h));
    undo(&mut h);
    assert_eq!(objects(&h), before);
    for p in [headline, panel] {
        click(&mut h, p, Modifiers::SHIFT);
    }
    h.get_by_label("2 objects selected");
    right_click(&mut h, panel);
    assert!(h.get_all_by_label("Delete").last().unwrap().accesskit_node().is_disabled());
    assert_eq!(objects(&h), before);
}

#[test]
fn the_fill_colour_swatch_recolours_the_selected_path_in_one_step() {
    let mut h = harness_document(false, false, " 0.8 0.9 1 rg 300 100 60 40 re f");
    let before = fills(&h);
    let p = at(&h, 330.0, 380.0);
    click(&mut h, p, Modifiers::NONE);
    h.get_by_label("1 object selected");
    // The panel only fills.
    assert!(disabled(&h, "Stroke colour"));
    h.get_by_label("Fill colour").click();
    h.run_steps(2);
    h.get_by_label("Red").click();
    h.run_steps(4);
    let [fill] = fills(&h).try_into().unwrap();
    assert!(fill.iter().zip([0.89, 0.13, 0.13]).all(|(a, b)| (a - b).abs() < 0.01), "{fill:?}");
    assert_eq!(undo_label(&h).as_deref(), Some("Change colour"));
    h.get_by_label("1 object selected");
    undo(&mut h);
    assert_eq!(fills(&h), before);
}

#[test]
fn an_arrange_with_nothing_to_pass_explains_and_changes_nothing() {
    let mut h = harness_document(false, false, " 0.8 0.9 1 rg 300 100 60 40 re f");
    let p = at(&h, 330.0, 380.0);
    click(&mut h, p, Modifiers::NONE);
    right_click(&mut h, p);
    menu_item(&mut h, "Arrange ⏵", "Bring to front");
    assert!(h.state().toast.as_ref().unwrap().0.contains("Bring to front failed: nothing above this object overlaps it"));
    assert!(!dirty(&h));
    h.get_by_label("1 object selected");
}

#[test]
fn escape_closes_the_colour_grid_and_keeps_the_selection() {
    let mut h = harness_document(false, false, " 0.8 0.9 1 rg 300 100 60 40 re f");
    let p = at(&h, 330.0, 380.0);
    click(&mut h, p, Modifiers::NONE);
    h.get_by_label("Fill colour").click();
    h.run_steps(2);
    h.get_by_label("Red");
    // The panel draws before the page: the grid takes the key, or the page would clear too.
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(h.query_by_label("Red").is_none());
    h.get_by_label("1 object selected");
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(nothing_selected(&h));
}

#[test]
fn an_arranged_copy_stacked_on_its_twin_stays_selected() {
    // The figure drawn twice in the same place, as a duplicated logo is.
    let mut h = harness_document(false, false, " q 1 0 0 1 40 340 cm /Figure Do Q");
    let p = at(&h, 60.0, 145.0);
    // A plain click would hand the image to the single-image tool.
    click(&mut h, p, Modifiers::SHIFT);
    h.get_by_label("1 object selected");
    right_click(&mut h, p);
    menu_item(&mut h, "Arrange ⏵", "Send to back");
    assert_eq!(undo_label(&h).as_deref(), Some("Send to back"));
    h.get_by_label("1 object selected");
    // The selection followed the copy down: it has its twin to pass again.
    right_click(&mut h, p);
    menu_item(&mut h, "Arrange ⏵", "Bring to front");
    assert_eq!(undo_label(&h).as_deref(), Some("Bring to front"));
    h.get_by_label("1 object selected");
}

#[test]
fn an_aligned_paragraph_with_a_twin_elsewhere_stays_selected() {
    // A second "Keep here", the same size, at the left (40..95 across, 241..253 down).
    let mut h = harness_document(false, false, " BT /F1 12 Tf 40 250 Td (Keep here) Tj ET 0.8 0.9 1 rg 300 100 60 40 re f");
    let panel = at(&h, 330.0, 380.0);
    for p in [at(&h, 55.0, 247.0), panel] {
        click(&mut h, p, Modifiers::SHIFT);
    }
    h.get_by_label("2 objects selected");
    right_click(&mut h, panel);
    menu_item(&mut h, "Align ⏵", "Left");
    assert_eq!(undo_label(&h).as_deref(), Some("Align objects"));
    h.get_by_label("2 objects selected");
}
