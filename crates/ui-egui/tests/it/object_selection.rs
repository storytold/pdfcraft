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
    let stream = |body: &str, dict: &str| format!("<< {dict} /Length {} >>\nstream\n{body}\nendstream", body.len());
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 500] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Figure 6 0 R >> >> >>"
            .into(),
        stream(
            &format!(
                "{}q BT {} Tr /F1 18 Tf 40 430 Td (Group headline) Tj ET Q q 1 0 0 1 40 340 cm /Figure Do Q BT /F1 12 Tf 240 90 Td (Keep here) Tj ET",
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
    harness_document(clipping, false)
}

fn harness_document(clipping: bool, background: bool) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.session = pdfcraft_engine::Session::new().with_clock(|| 1_700_000_000);
        app.set_option("language", "en").unwrap();
        app.open_bytes("group.pdf", None, fixture_with_background(clipping, background)).unwrap();
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
    let mut h = harness_document(false, true);
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

/// A press with Shift while a paragraph with changes is open is held until the button is up: it
/// says why, the paragraph keeps its text, and nothing under the press is moved or selected, not
/// even once the press turns into a drag.
#[test]
fn a_shift_click_with_a_changed_paragraph_open_shows_the_notice_and_moves_nothing() {
    let mut h = harness();
    let before = objects(&h);
    let p = at(&h, 55.0, 70.0);
    click(&mut h, p, Modifiers::NONE);
    h.state_mut().views[0].line_editor.as_mut().unwrap().text = "A draft still being edited".into();
    let (a, b) = (at(&h, 60.0, 145.0), at(&h, 120.0, 200.0));
    h.hover_at(a);
    h.run_steps(2);
    h.event_modifiers(
        egui::Event::PointerButton { pos: a, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::SHIFT },
        Modifiers::SHIFT,
    );
    h.run_steps(2);
    let notice = "Finish or discard the text being edited before selecting several objects.";
    assert!(h.state().toast.as_ref().is_some_and(|t| t.0.contains(notice)), "the notice says why");
    for k in 1..=3 {
        h.hover_at(a + (b - a) * (k as f32 / 3.0));
        h.run_steps(2);
    }
    h.drop_at(b);
    h.run_steps(4);
    assert_eq!(h.state().views[0].line_editor.as_ref().map(|e| e.text.as_str()), Some("A draft still being edited"));
    assert_eq!(objects(&h), before, "nothing moved");
    assert!(h.state().views[0].image_selection.is_none() && h.state().views[0].content.selected.is_none(), "nothing selected");
    assert!(
        h.query_all_by_label_contains("object selected").next().is_none() && h.query_all_by_label_contains("objects selected").next().is_none(),
        "no objects selected"
    );
}

/// A press beside a picture, within the reach the objects selected together are found in but
/// outside the picture itself, is nobody's: it starts an area selection. It used to start nothing.
#[test]
fn a_press_just_outside_a_picture_starts_an_area_selection() {
    let mut h = harness();
    // The figure is x 40..100, y 130..160 from the page's top; the added note x 130..230, y 130..144.
    let (a, b) = (at(&h, 38.5, 140.0), at(&h, 235.0, 175.0));
    h.hover_at(a);
    h.run_steps(2);
    h.drag_at(a);
    h.run_steps(2);
    h.hover_at(b);
    h.run_steps(2);
    h.drop_at(b);
    h.run_steps(4);
    h.get_by_label("2 objects selected");
}

/// A double-click on one of the objects selected together leaves the group and reaches that one:
/// added text opens for typing.
#[test]
fn a_double_click_on_a_grouped_object_reaches_it() {
    let mut h = harness();
    marquee(&mut h);
    // The added note is x 130..230, y 130..144 from the page's top.
    let note = at(&h, 180.0, 137.0);
    crate::support::input::double_click(&mut h, note);
    h.run_steps(2);
    assert!(h.state().views[0].content.draft.as_ref().is_some_and(|d| d.index == Some(0)), "the added note opens for typing");
}

/// A double-click on a paragraph selected with others opens the one its press was on, though the
/// button came up just off it.
#[test]
fn a_double_click_on_a_grouped_paragraph_released_just_off_it_opens_it() {
    let mut h = harness();
    marquee(&mut h);
    let (inside, outside) = {
        let app = h.state();
        let view = &app.views[0];
        let xf = view.page_xform(0).unwrap();
        let doc = app.session.get(view.id).unwrap();
        let block = doc.text_blocks(0).into_iter().find(|b| b.text.contains("Group headline")).expect("the headline");
        let frame = xf.user_rect(&doc.info, 0, block.rect.map(|v| v as f32));
        (egui::pos2(frame.center().x, frame.top() + 1.0), egui::pos2(frame.center().x, frame.top() - 4.0))
    };
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    h.hover_at(inside);
    h.run_steps(1);
    crate::support::input::one_frame(&mut h, [button(inside, true), button(inside, false)]);
    crate::support::input::one_frame(&mut h, [button(inside, true), egui::Event::PointerMoved(outside), button(outside, false)]);
    h.run_steps(2);
    assert!(h.state().views[0].line_editor.is_some(), "the paragraph pressed is open for typing");
}

/// A press counts its own modifiers, whatever the frame's are: Shift in the press event adds to the
/// selection, though the frame's modifiers show none.
#[test]
fn a_press_counts_the_modifiers_of_its_own_event() {
    let mut h = harness();
    let p = at(&h, 55.0, 70.0);
    click(&mut h, p, Modifiers::NONE);
    assert!(h.state().views[0].line_editor.is_some());
    let p = at(&h, 60.0, 145.0);
    let button = |pressed| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::SHIFT };
    h.hover_at(p);
    h.run_steps(2);
    crate::support::input::one_frame(&mut h, [button(true), button(false)]);
    h.run_steps(4);
    h.get_by_label("2 objects selected");
}

/// A double-click on a paragraph selected with others opens it for typing (the user's ruling).
#[test]
fn a_double_click_on_a_grouped_paragraph_opens_it() {
    let mut h = harness();
    marquee(&mut h);
    // "Group headline" is at x 40, its baseline y 430 of 500.
    let headline = at(&h, 80.0, 65.0);
    crate::support::input::double_click(&mut h, headline);
    h.run_steps(2);
    assert!(h.state().views[0].line_editor.is_some(), "the paragraph is open for typing");
}

fn texts(h: &Harness<'static, PdfCraftApp>, view: usize) -> Vec<String> {
    let app = h.state();
    app.session.get(app.views[view].id).unwrap().editable_objects(0).unwrap().into_iter().filter_map(|o| o.text).collect()
}

fn can_undo(h: &Harness<'static, PdfCraftApp>, view: usize) -> Option<String> {
    let app = h.state();
    app.session.get(app.views[view].id).unwrap().can_undo().map(str::to_string)
}

fn keys(h: &mut Harness<'static, PdfCraftApp>, modifiers: Modifiers, key: Key) {
    h.key_press_modifiers(modifiers, key);
    h.run_steps(4);
}

/// âŒ˜A in Edit a PDF selects every object on the page, as Acrobat does: paragraphs, artwork and
/// added content. The arrow keys then move them all and Delete removes them all, each as one step.
#[test]
fn command_a_selects_every_object_on_the_page_to_move_and_delete_together() {
    let mut h = harness();
    let before = objects(&h);
    assert_eq!(before.len(), 4);
    keys(&mut h, Modifiers::COMMAND, Key::A);
    h.get_by_label("4 objects selected");
    assert!(h.state().views[0].selected_text().is_none_or(|t| t.is_empty()), "objects, not the page's text");
    keys(&mut h, Modifiers::SHIFT, Key::ArrowRight);
    assert_eq!(can_undo(&h, 0).as_deref(), Some("Move objects"));
    for (old, new) in before.iter().zip(objects(&h)) {
        assert!((new.rect[0] - old.rect[0] - 10.0).abs() < 0.01, "{old:?} -> {new:?}");
    }
    h.get_by_label("4 objects selected");
    keys(&mut h, Modifiers::NONE, Key::Delete);
    assert!(objects(&h).is_empty(), "{:?}", objects(&h));
    assert_eq!(can_undo(&h, 0).as_deref(), Some("Delete content"));
    keys(&mut h, Modifiers::COMMAND, Key::Z);
    assert_eq!(objects(&h).len(), 4, "one undo brings every object back");
}

/// In a paragraph open for typing, âŒ˜A selects its text, as before.
#[test]
fn command_a_in_an_open_paragraph_selects_its_text_not_the_objects() {
    let mut h = harness();
    let p = at(&h, 55.0, 70.0);
    click(&mut h, p, Modifiers::NONE);
    assert!(h.state().views[0].line_editor.is_some());
    keys(&mut h, Modifiers::COMMAND, Key::A);
    assert!(h.query_all_by_label_contains("objects selected").next().is_none());
    assert!(h.state().views[0].line_editor.is_some(), "the paragraph stays open");
}

/// âŒ˜C, âŒ˜X and âŒ˜V take the objects selected together: a paste onto their own page lands 18 pt down
/// and right, selected; a paste into another document draws them there; a cut is one step.
#[test]
fn objects_copy_cut_and_paste_with_the_keys_on_the_page_and_into_another_document() {
    let mut h = harness();
    h.state_mut().system_clipboard = false;
    let unchanged = can_undo(&h, 0);
    keys(&mut h, Modifiers::COMMAND, Key::A);
    keys(&mut h, Modifiers::COMMAND, Key::C);
    assert_eq!(can_undo(&h, 0), unchanged, "a copy changes nothing");
    assert!(h.state().object_clipboard.is_some());
    keys(&mut h, Modifiers::COMMAND, Key::V);
    assert_eq!(can_undo(&h, 0).as_deref(), Some("Paste"));
    let pasted = texts(&h, 0);
    for text in ["Group headline", "Keep here", "Added note"] {
        assert_eq!(pasted.iter().filter(|t| t.contains(text)).count(), 2, "{text}: {pasted:?}");
    }
    assert!(h.query_all_by_label_contains("objects selected").next().is_some(), "the pasted objects are selected");
    let moved = objects(&h).into_iter().filter(|o| o.text.as_deref() == Some("Keep here")).map(|o| o.rect).collect::<Vec<_>>();
    assert!((moved[1][0] - moved[0][0] - 18.0).abs() < 0.01 && (moved[0][1] - moved[1][1] - 18.0).abs() < 0.01, "{moved:?}");

    // Into another open document, where it was: exactly over its own copy of the same page.
    h.state_mut().open_bytes("other.pdf", None, fixture(false)).unwrap();
    h.run_steps(4);
    let other = h.state().active.unwrap();
    assert_eq!(other, 1);
    h.state_mut().quick_tool = pdfcraft_ui_egui::QuickTool::EditText;
    h.run_steps(2);
    keys(&mut h, Modifiers::COMMAND, Key::V);
    assert_eq!(can_undo(&h, other).as_deref(), Some("Paste"));
    let there = texts(&h, other);
    assert!(["Group headline", "Keep here", "Added note"].iter().all(|t| there.iter().any(|x| x.contains(t))), "{there:?}");

    // Cut: copied, then deleted in one step; a paste brings it back.
    keys(&mut h, Modifiers::COMMAND, Key::A);
    keys(&mut h, Modifiers::COMMAND, Key::X);
    assert!(texts(&h, other).is_empty(), "{:?}", texts(&h, other));
    assert_eq!(can_undo(&h, other).as_deref(), Some("Delete content"));
    keys(&mut h, Modifiers::COMMAND, Key::V);
    assert!(texts(&h, other).iter().any(|t| t.contains("Added note")));
}

/// What another application (or another copy) put on the clipboard since wins ⌘V: the objects
/// paste only while it still holds the text their copy put there.
#[test]
fn a_newer_copy_elsewhere_wins_over_the_copied_objects() {
    let mut h = harness();
    h.state_mut().system_clipboard = false;
    keys(&mut h, Modifiers::COMMAND, Key::A);
    keys(&mut h, Modifiers::COMMAND, Key::C);
    let copied = h.state().object_clipboard.as_ref().unwrap().clip().text();
    assert!(copied.contains("Group headline") && copied.contains("Added note"), "{copied}");
    let before = can_undo(&h, 0);
    h.event(egui::Event::Paste("a word copied in another application".into()));
    h.run_steps(4);
    assert_eq!(can_undo(&h, 0), before, "the newer text is not the objects");
    h.event(egui::Event::Paste(copied.replace('\n', "\r\n")));
    h.run_steps(4);
    assert_eq!(can_undo(&h, 0).as_deref(), Some("Paste"));
}

/// A picture the platform answers a native Edit ▸ Paste with lands on the page, as a text answer
/// does, the context menu that was open closed by Edit ▸ Paste: here a picture copied since the
/// objects were.
#[test]
fn a_picture_for_a_native_edit_paste_with_a_context_menu_open_lands_on_the_page() {
    let mut h = harness();
    h.state_mut().system_clipboard = false;
    keys(&mut h, Modifiers::COMMAND, Key::A);
    keys(&mut h, Modifiers::COMMAND, Key::C);
    assert!(h.state().object_clipboard.is_some(), "the objects are copied, so Edit ▸ Paste is on");
    let p = at(&h, 55.0, 70.0);
    h.hover_at(p);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
        h.run_steps(1);
    }
    h.run_steps(2);
    assert!(egui::Popup::is_any_open(&h.ctx), "the context menu is open");
    let before = can_undo(&h, 0);
    let ctx = h.ctx.clone();
    h.state_mut().run_menu_command(&ctx, &craft_menu::CommandRef::new("edit.paste"));
    h.step();
    assert!(crate::support::asked_for_the_clipboard(&h), "Edit ▸ Paste asked the platform for the clipboard");
    assert!(!egui::Popup::is_any_open(&h.ctx), "Edit ▸ Paste closed the context menu");
    h.event(egui::Event::PasteImage(std::sync::Arc::new(egui::ColorImage::filled([30, 20], egui::Color32::GRAY))));
    h.run_steps(3);
    assert_ne!(can_undo(&h, 0), before, "the picture was dropped");
    assert_eq!(can_undo(&h, 0).as_deref(), Some("Add image"), "the picture landed on the page");
}
