//! Filling in a form in the real shell (egui_kittest): typing, Tab, check boxes, radios, choices.

use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

fn harness() -> Harness<'static, PdfCraftApp> {
    harness_bytes(include_bytes!("data/form.pdf").to_vec())
}

fn harness_bytes(bytes: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, bytes).unwrap();
        app.set_option("left", "closed").unwrap();
        // The whole 300×400 pt page on screen.
        app.set_option("zoom", "150").unwrap();
        app
    });
    for _ in 0..60 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h
}

const PRINTED_SQUARE: [f64; 4] = [24.0, 18.0, 42.0, 36.0];

/// A synthetic printed square, detected from page content rather than a seeded cache.
fn printed_form() -> Harness<'static, PdfCraftApp> {
    use pdfcraft_cos::{Document, Object, SaveOptions, Stream, write_incremental};
    let mut doc = Document::open(std::sync::Arc::new(include_bytes!("data/form.pdf").to_vec())).unwrap();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let stream = doc.add(Object::Stream(Stream::flate(Default::default(), b"24 18 18 18 re S")));
    doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Ref(stream))).unwrap();
    harness_bytes(write_incremental(&doc, &SaveOptions::default()).unwrap())
}

fn square_center(h: &Harness<'static, PdfCraftApp>, rect: [f64; 4]) -> egui::Pos2 {
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    pdfcraft_ui_egui::forms_ui::square_screen_rect(&s.views[0], &doc.info, 0, rect).unwrap().center()
}

fn click_square(h: &mut Harness<'static, PdfCraftApp>) {
    let p = square_center(h, PRINTED_SQUARE);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(4);
}

fn value(h: &Harness<'static, PdfCraftApp>, name: &str) -> Vec<String> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == name).unwrap().value.clone()
}

/// Hover the centre of a field's widget (without clicking).
fn hover_field(h: &mut Harness<'static, PdfCraftApp>, name: &str, widget: usize) {
    let p = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == name).unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, widget).expect("on screen").center()
    };
    h.hover_at(p);
    h.run_steps(4);
}

/// Click the centre of a field's widget.
fn click_field(h: &mut Harness<'static, PdfCraftApp>, name: &str, widget: usize) {
    let p = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == name).unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, widget).expect("on screen").center()
    };
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
}

#[test]
fn typing_into_a_text_field_and_tabbing_on() {
    let mut h = harness();
    click_field(&mut h, "name", 0);
    assert!(h.state().views[0].forms.focus.is_some(), "the editor opened");
    h.event(egui::Event::Text("Ada Lovelace".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(4);
    assert_eq!(value(&h, "name"), ["Ada Lovelace"]);
    assert_eq!(
        h.state().views[0].forms.focus.as_ref().map(|f| f.name.as_str()),
        Some("country"),
        "Tab moves to the next field that takes typing or a choice"
    );
    // The country list is open; Escape closes it. Escape also abandons a draft.
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    click_field(&mut h, "city", 0);
    assert_eq!(h.state().views[0].forms.focus.as_ref().map(|f| f.name.as_str()), Some("city"));
    h.event(egui::Event::Text("Paris".into()));
    h.run_steps(1);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(value(&h, "city").is_empty());
    assert!(h.state().views[0].forms.focus.is_none());
}

#[test]
fn check_boxes_radios_and_choices() {
    let mut h = harness();
    click_field(&mut h, "agree", 0);
    assert_eq!(value(&h, "agree"), ["Yes"]);
    click_field(&mut h, "agree", 0);
    assert!(value(&h, "agree").is_empty());
    click_field(&mut h, "size", 1);
    assert_eq!(value(&h, "size"), ["L"]);
    click_field(&mut h, "size", 0);
    assert_eq!(value(&h, "size"), ["S"]);
    click_field(&mut h, "country", 0);
    egui_kittest::kittest::Queryable::get_by_label(&h, "France").click();
    h.run_steps(4);
    assert_eq!(value(&h, "country"), ["fr"]);
    // Clear form takes everything back; undo restores it.
    assert!(h.state_mut().execute("form.clear"));
    h.run_steps(2);
    assert!(value(&h, "country").is_empty() && value(&h, "size").is_empty());
    h.state_mut().undo();
    h.run_steps(2);
    assert_eq!(value(&h, "country"), ["fr"]);
}

#[test]
fn tabbing_into_a_filled_field_selects_it_so_typing_replaces() {
    let mut h = harness();
    click_field(&mut h, "country", 0);
    egui_kittest::kittest::Queryable::get_by_label(&h, "Canada").click();
    h.run_steps(3);
    click_field(&mut h, "city", 0);
    h.event(egui::Event::Text("Paris".into()));
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(value(&h, "city"), ["Paris"]);
    // name → (Tab) country → (Tab) city, whose text is selected on entry.
    click_field(&mut h, "name", 0);
    h.key_press(egui::Key::Tab);
    h.run_steps(3);
    h.key_press(egui::Key::Tab);
    h.run_steps(4);
    assert_eq!(h.state().views[0].forms.focus.as_ref().map(|f| f.name.as_str()), Some("city"));
    h.event(egui::Event::Text("Lyon".into()));
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(value(&h, "city"), ["Lyon"]);
}

#[test]
fn date_fields_offer_a_calendar() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness();
    h.state_mut().apply_edit(pdfcraft_engine::Edit::AddField {
        page: 0,
        rect: [50.0, 40.0, 200.0, 60.0],
        kind: pdfcraft_engine::NewField::Date,
        name: Some("due".into()),
    });
    h.run_steps(4);
    click_field(&mut h, "due", 0);
    let (y, m, _) = h.state().session.today();
    let month =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"][(m - 1) as usize];
    h.get_by_label(&format!("{month} {y}"));
    h.get_by_label("›").click();
    h.run_steps(2);
    h.get_by_label("15").click();
    h.run_steps(4);
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    assert_eq!(value(&h, "due"), vec![format!("{nm:02}/15/{ny}")], "picked in the field's format");
}

#[test]
fn toggle_popup_stays_open_across_the_gap() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness();
    hover_field(&mut h, "agree", 0);
    let (field, popup) = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == "agree").unwrap();
        (pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).unwrap(), s.views[0].forms.offer_rect.unwrap())
    };
    // Traverse the gap in small increments, instead of teleporting to the button.
    let gap = egui::pos2((field.right() + popup.left()) / 2.0, field.center().y);
    assert!(!field.contains(gap) && !popup.contains(gap));
    let start = field.center();
    for step in 1..=20 {
        h.hover_at(start + (gap - start) * (step as f32 / 20.0));
        h.run_steps(1);
        assert!(h.state().views[0].forms.offer.is_some());
    }
    h.get_by_label("Check").click();
    h.run_steps(4);
    assert_eq!(value(&h, "agree"), ["Yes"]);
    h.hover_at(field.left_top() - egui::vec2(20.0, 20.0));
    h.run_steps(2);
    assert!(h.state().views[0].forms.offer.is_none());
}

#[test]
fn printed_square_preserves_other_stamps() {
    use pdfcraft_engine::{Edit, FillMark, NewAnnotation, Shape, StampKind, Style};
    let mut h = printed_form();
    for shape in
        [Shape::Mark { rect: PRINTED_SQUARE, mark: FillMark::Cross }, Shape::Stamp { rect: PRINTED_SQUARE, stamp: StampKind::Approved, by: None }]
    {
        let style = Style::default_for(&shape);
        assert!(h.state_mut().apply_edit(Edit::AddAnnotation(NewAnnotation {
            page: 0,
            shape,
            style,
            contents: String::new(),
            author: "Reviewer".into()
        })));
    }
    h.run_steps(3);
    let stamps = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        let mut names: Vec<String> = s.session.get(s.views[0].id).unwrap().info.annotations.iter().filter_map(|a| a.stamp.clone()).collect();
        names.sort();
        names
    };
    assert_eq!(stamps(&h), ["Approved", "PCCross"]);
    click_square(&mut h);
    assert_eq!(stamps(&h), ["Approved", "PCCheck", "PCCross"]);
    // Detection and the scoped comment refresh both retain the check's identity.
    click_square(&mut h);
    assert_eq!(stamps(&h), ["Approved", "PCCross"]);
}

#[test]
fn printed_square_commits_the_active_text_draft() {
    let mut h = printed_form();
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("Ada Lovelace".into()));
    h.run_steps(2);
    assert_eq!(h.state().views[0].forms.focus.as_ref().unwrap().text, "Ada Lovelace");
    click_square(&mut h);
    assert_eq!(value(&h, "name"), ["Ada Lovelace"]);
    let s = h.state();
    assert!(s.session.get(s.views[0].id).unwrap().info.annotations.iter().any(|a| a.stamp.as_deref() == Some("PCCheck")));
    h.state_mut().undo();
    h.run_steps(2);
    assert_eq!(value(&h, "name"), ["Ada Lovelace"], "undo removes only the check");
    h.state_mut().undo();
    h.run_steps(2);
    assert!(value(&h, "name").is_empty(), "the preceding step committed the text");
}

#[test]
fn selecting_added_content_over_a_printed_square_does_not_toggle_checks() {
    use pdfcraft_engine::{AddedText, Edit};
    let mut h = printed_form();
    assert!(
        h.state_mut()
            .apply_edit(Edit::AddText { page: 0, text: AddedText { rect: [24.0, 18.0, 65.0, 36.0], text: "Text".into(), ..Default::default() } })
    );
    h.state_mut().set_option("tool", "edit").unwrap();
    h.run_steps(4);
    click_square(&mut h);
    let s = h.state();
    assert_eq!(s.views[0].content.selected, Some((0, 0)), "the added text is selected");
    assert!(s.session.get(s.views[0].id).unwrap().info.annotations.is_empty(), "selecting text did not stamp a check");
}

#[test]
fn dropping_a_comment_on_a_printed_square_finishes_the_move() {
    use pdfcraft_engine::{Edit, NewAnnotation, Shape, Style};
    let mut h = printed_form();
    let start_rect = [80.0, 18.0, 98.0, 36.0];
    let shape = Shape::Rectangle { rect: start_rect };
    let style = Style::default_for(&shape);
    let annotation = NewAnnotation { page: 0, shape, style, contents: String::new(), author: String::new() };
    assert!(h.state_mut().apply_edit(Edit::AddAnnotation(annotation)));
    h.run_steps(3);
    let (start, end) = (square_center(&h, start_rect), square_center(&h, PRINTED_SQUARE));
    h.hover_at(start);
    h.run_steps(2);
    h.drag_at(start);
    h.run_steps(1);
    h.hover_at(start + egui::vec2(-12.0, 0.0));
    h.run_steps(2);
    assert!(h.state().views[0].comments.gesture.is_some(), "the move started");
    h.drop_at(end);
    h.run_steps(4);
    let s = h.state();
    assert!(s.views[0].comments.gesture.is_none(), "the drop finished the move");
    let annotations = &s.session.get(s.views[0].id).unwrap().info.annotations;
    assert_eq!(annotations.len(), 1, "the drag did not add a check");
    for (actual, expected) in annotations[0].rect.into_iter().zip(PRINTED_SQUARE) {
        assert!((f64::from(actual) - expected).abs() < 0.001, "the comment moved onto the square");
    }
}

/// The value of `name` in a saved PDF, read back by opening it in a fresh app.
fn saved_value(path: &std::path::Path, name: &str) -> Vec<String> {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.open_bytes("saved.pdf", None, std::fs::read(path).unwrap()).unwrap();
    app.session.get(app.views[0].id).unwrap().form.iter().find(|f| f.name == name).unwrap().value.clone()
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-forms-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn saving_while_still_typing_in_a_field_saves_the_text() {
    // Issue #166: ⌘S with the field still focused saved it blank.
    let dir = scratch("save-focused");
    let out = dir.join("saved.pdf");
    let mut h = harness();
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("Browser Form 123".into()));
    h.run_steps(2);
    assert!(h.state().views[0].forms.focus.is_some(), "still typing");
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::S);
    h.run_steps(3);
    assert_eq!(saved_value(&out, "name"), ["Browser Form 123"], "the saved PDF has the typed text");
    assert_eq!(value(&h, "name"), ["Browser Form 123"]);
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert!(!doc.dirty, "and the document is saved, not left with an unsaved change");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn closing_while_still_typing_in_a_field_asks_to_save() {
    let mut h = harness();
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("Not lost".into()));
    h.run_steps(2);
    h.state_mut().request_close_tab(0);
    assert!(h.state().close_request.is_some(), "typed text is an unsaved change: the user is asked");
    assert_eq!(h.state().views.len(), 1);
}

#[test]
fn close_all_saves_text_typed_in_a_tab_that_is_no_longer_active() {
    let dir = scratch("close-all");
    let out = dir.join("first.pdf");
    let mut h = harness();
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("Typed in tab one".into()));
    h.run_steps(2);
    // Another document becomes active while the first one's field still has its draft.
    h.state_mut().open_bytes("other.pdf", None, include_bytes!("data/form.pdf").to_vec()).unwrap();
    h.run_steps(2);
    h.state_mut().close_all();
    assert!(h.state().close_request.is_some(), "the first document has an unsaved change");
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    let ctx = h.ctx.clone();
    h.state_mut().resolve_close(&ctx, Some(true));
    assert_eq!(saved_value(&out, "name"), ["Typed in tab one"]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A key press as one event (kittest's `key_press` runs a frame per queued event).
fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
}

/// Deliver `events` together, in one frame.
fn one_frame(h: &mut Harness<'static, PdfCraftApp>, events: Vec<egui::Event>) {
    h.input_mut().events.extend(events);
    h.run_steps(1);
}

/// Type `text` into field `name`, then press ⌘S in the same frame as `last` (more input).
fn type_then_save_in_one_frame(h: &mut Harness<'static, PdfCraftApp>, name: &str, text: &str, last: egui::Event, out: &std::path::Path) {
    click_field(h, name, 0);
    h.event(egui::Event::Text(text.into()));
    h.run_steps(2);
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    one_frame(h, vec![last, key(egui::Key::S, egui::Modifiers::COMMAND)]);
    h.run_steps(3);
}

#[test]
fn typing_and_saving_in_the_same_frame_saves_the_last_keystrokes() {
    let dir = scratch("same-frame");
    let out = dir.join("saved.pdf");
    let mut h = harness();
    type_then_save_in_one_frame(&mut h, "name", "Browser", egui::Event::Text(" Form 123".into()), &out);
    assert_eq!(saved_value(&out, "name"), ["Browser Form 123"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn escape_and_save_in_the_same_frame_discards_the_draft() {
    let dir = scratch("escape");
    let out = dir.join("saved.pdf");
    let mut h = harness();
    let before = value(&h, "name");
    type_then_save_in_one_frame(&mut h, "name", "Discard me", key(egui::Key::Escape, egui::Modifiers::NONE), &out);
    assert_eq!(value(&h, "name"), before, "Escape cancels typing, even with ⌘S right after it");
    assert!(h.state().views[0].forms.focus.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unrelated_command_leaves_the_field_being_typed_in_alone() {
    let mut h = harness();
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("Still typing".into()));
    h.run_steps(2);
    assert!(h.state_mut().execute("view.theme.dark"));
    h.run_steps(2);
    let focus = h.state().views[0].forms.focus.clone().expect("the editor stays open");
    assert_eq!(focus.text, "Still typing");
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty, "nothing was committed");
}

#[test]
fn exporting_form_data_while_typing_includes_the_text() {
    let dir = scratch("export");
    let out = dir.join("data.xfdf");
    let mut h = harness();
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("Exported value".into()));
    h.run_steps(2);
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    h.state_mut().export_data_dialog(false, true);
    assert!(std::fs::read_to_string(&out).unwrap().contains("Exported value"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// One text field, `qty`, whose validation script refuses values over 100.
fn validated_form() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (qty) /V (4) /Rect [20 350 280 372] /P 3 0 R /F 4 /AA << /V << /S /JavaScript /JS (if (event.value > 100) { app.alert('At most 100'); event.rc = false; }) >> >> >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

#[test]
fn a_rejected_value_keeps_the_typing_and_stops_the_save() {
    let dir = scratch("rejected");
    let out = dir.join("saved.pdf");
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("order.pdf", None, validated_form()).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    h.run_steps(6);
    click_field(&mut h, "qty", 0);
    h.event(egui::Event::Text("500".into()));
    h.run_steps(2);
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::S);
    h.run_steps(4);
    assert!(!out.exists(), "nothing is saved without the typed value");
    assert_eq!(value(&h, "qty"), ["4"], "the field keeps its valid value");
    let focus = h.state().views[0].forms.focus.clone().expect("the editor stays open");
    assert_eq!(focus.text, "4500", "with the text (typed after the 4), so the user can fix it");
    // And closing asks rather than dropping it.
    h.state_mut().request_close_tab(0);
    assert!(h.state().close_request.is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_save_shortcut_pressed_while_typing_saves_only_the_document_it_was_pressed_in() {
    let dir = scratch("deferred-doc");
    let out = dir.join("saved.pdf");
    let mut h = harness();
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("Tab one".into()));
    h.run_steps(2);
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    one_frame(&mut h, vec![key(egui::Key::S, egui::Modifiers::COMMAND)]);
    assert!(!out.exists(), "⌘S while typing waits a frame for the field");
    // Before the deferred ⌘S runs, another document becomes active.
    h.state_mut().open_bytes("other.pdf", None, include_bytes!("data/form.pdf").to_vec()).unwrap();
    h.run_steps(3);
    assert!(!out.exists(), "the other document is not saved by a shortcut pressed in the first");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_value_refused_on_enter_keeps_save_in_the_same_frame_from_running() {
    let dir = scratch("enter-refused");
    let out = dir.join("saved.pdf");
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("order.pdf", None, validated_form()).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    h.run_steps(6);
    click_field(&mut h, "qty", 0);
    h.event(egui::Event::Text("500".into()));
    h.run_steps(2);
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    one_frame(&mut h, vec![key(egui::Key::Enter, egui::Modifiers::NONE), key(egui::Key::S, egui::Modifiers::COMMAND)]);
    h.run_steps(3);
    assert_eq!(value(&h, "qty"), ["4"]);
    assert!(!out.exists(), "nothing is saved right after the field refused its value");
    // The refused text is back in the field, so a later ⌘S stops again instead of saving 4.
    assert_eq!(h.state().views[0].forms.focus.as_ref().map(|f| f.text.as_str()), Some("4500"));
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::S);
    h.run_steps(4);
    assert!(!out.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn flattening_while_typing_flattens_the_typed_text() {
    let mut h = harness();
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("Flat Ada".into()));
    h.run_steps(2);
    assert!(h.state_mut().execute("form.flatten"));
    h.run_steps(2);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert!(doc.form.iter().all(|f| f.name != "name"), "the field is flattened");
    let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), Default::default());
    let text = r
        .render(pdfcraft_render::RenderRequest { page: 0, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() })
        .text
        .map(|t| t.plain_text())
        .unwrap_or_default();
    assert!(text.contains("Flat Ada"), "into the page: {text:?}");
}

#[cfg(not(target_arch = "wasm32"))]
mod revert_tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct RevertDir(PathBuf);
    impl RevertDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            for _ in 0..128 {
                let path = std::env::temp_dir().join(format!("pdfcraft-revert-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
                match std::fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("creating Revert test directory: {error}"),
                }
            }
            panic!("no unused Revert test directory after 128 attempts");
        }
    }
    impl Drop for RevertDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reverting_rejected_input_saves_the_original_field_and_appearance() {
        let dir = RevertDir::new();
        let path = dir.0.join("saved.pdf");
        let mut h = harness();
        assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldValue {
            name: "name".into(),
            value: pdfcraft_engine::FieldValue::Text("Saved original".into()),
        }));
        h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
        assert!(h.state_mut().save_active(pdfcraft_ui_egui::SaveTarget::InPlace));
        let saved = std::fs::read(&path).unwrap();
        assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldProps {
            name: "name".into(),
            props: Box::new(pdfcraft_engine::FieldProps {
                validate: Some(pdfcraft_engine::form_scripts::Validate::Range { min: Some(0.0), max: Some(10.0) }),
                ..Default::default()
            }),
        }));
        h.run_steps(3);
        click_field(&mut h, "name", 0);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.event(egui::Event::Text("99".into()));
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::S);
        h.run_steps(4);
        assert_eq!(h.state().views[0].forms.focus.as_ref().unwrap().text, "99");
        assert_eq!(std::fs::read(&path).unwrap(), saved, "rejection leaves the saved file untouched");
        assert!(h.state_mut().execute("file.revert"));
        h.run_steps(2);
        h.get_all_by_label("Revert").last().expect("confirmation button").click();
        h.run_steps(4);
        assert!(h.state().views[0].forms.focus.is_none() && h.state().views[0].pending_edit.is_none());
        assert_eq!(h.state().first_dirty(), None, "Revert discards rejected typing too");
        assert_eq!(value(&h, "name"), ["Saved original"]);
        assert!(h.state_mut().save_active(pdfcraft_ui_egui::SaveTarget::InPlace));
        assert_eq!(saved_value(&path, "name"), ["Saved original"]);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes, saved, "later Save cannot resurrect the discarded draft");
        let mut renderer = pdfcraft_render::PageRenderer::new(std::sync::Arc::new(bytes), Default::default());
        let output = renderer.render(pdfcraft_render::RenderRequest { page: 0, kind: pdfcraft_render::RequestKind::Text, ..Default::default() });
        assert!(output.error.is_none(), "saved appearance is readable: {:?}", output.error);
        assert!(output.text.unwrap().plain_text().contains("Saved original"), "saved AP still draws the original value");
    }
}
