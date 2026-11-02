//! Filling in a form in the real shell (egui_kittest): typing, Tab, check boxes, radios, choices.

use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("data/form.pdf").to_vec()).unwrap();
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

fn value(h: &Harness<'static, PdfCraftApp>, name: &str) -> Vec<String> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == name).unwrap().value.clone()
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
fn dragging_over_another_fields_text_focuses_it_and_selects_from_the_press() {
    // Issue #2213: with the caret in one field, a drag over another field's text selected
    // nothing; the field opened only on a click.
    let mut h = harness();
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("Ada Lovelace".into()));
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    click_field(&mut h, "city", 0);
    assert_eq!(h.state().views[0].forms.focus.as_ref().map(|f| f.name.as_str()), Some("city"));
    let (rect, id) = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == "name").unwrap();
        let rect = pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen");
        (rect, egui::Id::new(("form-field", s.views[0].id.0, "name", 0usize)))
    };
    // From inside the first word to past the end of the text.
    let (from, to) = (egui::pos2(rect.left() + 30.0, rect.center().y), egui::pos2(rect.right() - 8.0, rect.center().y));
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(from.lerp(to, k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(3);
    assert_eq!(h.state().views[0].forms.focus.as_ref().map(|f| f.name.as_str()), Some("name"), "the press moved the focus");
    let n = "Ada Lovelace".chars().count();
    let range = egui::TextEdit::load_state(&h.ctx, id).and_then(|s| s.cursor.char_range()).expect("a selection in the field");
    // "Ad|a": not the caret at the end that a newly focused editor gets on its own.
    assert_eq!(range.secondary.index.0, 2, "anchored where the drag started: {range:?}");
    assert_eq!(range.primary.index.0, n, "and extended to where it ended: {range:?}");
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

/// The calendar used to keep the month it opened on, so a date typed while it was open showed
/// only after leaving the field and coming back.
#[test]
fn the_calendar_follows_a_complete_date_typed_while_it_is_open() {
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
    // Half a date leaves the calendar where it is.
    h.event(egui::Event::Text("03/1".into()));
    h.run_steps(2);
    h.get_by_label(&format!("{month} {y}"));
    // The whole date moves it there.
    h.event(egui::Event::Text("4/2025".into()));
    h.run_steps(2);
    h.get_by_label("March 2025");
    // The arrows still turn the months, and the calendar stays on their month while the text
    // doesn't change.
    h.get_by_label("›").click();
    h.run_steps(3);
    h.get_by_label("April 2025");
}

/// A date format with its fields side by side ("yyyymmdd") refused every typed date, and its
/// calendar opened on today's month.
#[test]
fn an_undelimited_date_field_takes_a_typed_date_and_its_calendar_opens_on_it() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness();
    h.state_mut().apply_edit(pdfcraft_engine::Edit::AddField {
        page: 0,
        rect: [50.0, 40.0, 200.0, 60.0],
        kind: pdfcraft_engine::NewField::Date,
        name: Some("due".into()),
    });
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldProps {
        name: "due".into(),
        props: Box::new(pdfcraft_engine::FieldProps {
            format: Some(pdfcraft_engine::form_scripts::Format::Date("yyyymmdd".into())),
            ..Default::default()
        }),
    }));
    h.run_steps(4);
    click_field(&mut h, "due", 0);
    h.event(egui::Event::Text("20250314".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(4);
    assert_eq!(h.state().toast, None, "the typed date was not refused");
    assert_eq!(value(&h, "due"), ["20250314"]);
    click_field(&mut h, "due", 0);
    h.get_by_label("March 2025");
}

#[test]
fn lithuanian_calendar_localizes_months_and_weekdays_without_changing_field_values() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness();
    h.state_mut().set_option("language", "lt").unwrap();
    h.state_mut().apply_edit(pdfcraft_engine::Edit::AddField {
        page: 0,
        rect: [50.0, 40.0, 200.0, 60.0],
        kind: pdfcraft_engine::NewField::Date,
        name: Some("due".into()),
    });
    h.run_steps(4);
    click_field(&mut h, "due", 0);
    let (y, m, _) = h.state().session.today();
    let months = ["sausis", "vasaris", "kovas", "balandis", "gegužė", "birželis", "liepa", "rugpjūtis", "rugsėjis", "spalis", "lapkritis", "gruodis"];
    h.get_by_label(&format!("{y} m. {}", months[(m - 1) as usize]));
    for day in ["Sk", "Pr", "An", "Tr", "Kt", "Pn", "Št"] {
        h.get_by_label(day);
    }
    h.get_by_label("›").click();
    h.run_steps(2);
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    h.get_by_label(&format!("{ny} m. {}", months[(nm - 1) as usize]));
    h.get_by_label("15").click();
    h.run_steps(4);
    assert_eq!(value(&h, "due"), vec![format!("{nm:02}/15/{ny}")], "the PDF field keeps its own date format");
}

/// The value of `name` in a saved PDF, read back by opening it in a fresh app.
fn saved_value(path: &std::path::Path, name: &str) -> Vec<String> {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.open_bytes("saved.pdf", None, std::fs::read(path).unwrap()).unwrap();
    app.session.get(app.views[0].id).unwrap().form.iter().find(|f| f.name == name).unwrap().value.clone()
}

/// Step frames until the save has written `path`, instead of assuming it is there after a fixed
/// number of frames. The write is atomic, so the file appears only when it is complete.
fn wait_for_file(h: &mut Harness<'static, PdfCraftApp>, path: &std::path::Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !path.exists() {
        assert!(std::time::Instant::now() < deadline, "the save never wrote {}", path.display());
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // One more frame so the app has seen its own save before the test looks at its state.
    h.run_steps(1);
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
fn tabbing_back_to_the_same_field_then_saving_keeps_the_committed_value() {
    // A form with one text field wraps Tab back to that field. Multiple widgets of one
    // field share its value too: opening the next widget must see the accepted edit.
    for (case, shared_widgets, backwards, same_frame) in [
        ("single-forward", false, false, false),
        ("single-backward", false, true, false),
        ("shared-forward", true, false, false),
        ("shared-backward", true, true, false),
        ("same-frame", false, false, true),
    ] {
        let dir = scratch(case);
        let out = dir.join("saved.pdf");
        let bytes = if shared_widgets {
            pdf_from_objects(&[
                "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>",
                "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>",
                "<< /Type /Page /Parent 2 0 R /Annots [6 0 R 7 0 R] >>",
                "<< /FT /Tx /T (qty) /V (4) /Kids [6 0 R 7 0 R] >>",
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
                "<< /Type /Annot /Subtype /Widget /Parent 4 0 R /Rect [20 350 280 372] /P 3 0 R /F 4 >>",
                "<< /Type /Annot /Subtype /Widget /Parent 4 0 R /Rect [20 250 280 272] /P 3 0 R /F 4 >>",
            ])
        } else {
            validated_form()
        };
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("tab.pdf", None, bytes).unwrap();
            app.set_option("left", "closed").unwrap();
            app.set_option("zoom", "150").unwrap();
            app
        });
        h.run_steps(6);
        click_field(&mut h, "qty", 0);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.event(egui::Event::Text("42".into()));
        h.run_steps(2);
        h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
        let modifiers = if backwards { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE };
        if same_frame {
            one_frame(&mut h, vec![key(egui::Key::Tab, modifiers), key(egui::Key::S, egui::Modifiers::COMMAND)]);
        } else {
            h.key_press_modifiers(modifiers, egui::Key::Tab);
            h.run_steps(3);
            h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::S);
        }
        wait_for_file(&mut h, &out);
        assert_eq!(saved_value(&out, "qty"), ["42"], "{case}: saving cannot restore the old value");
        assert_eq!(value(&h, "qty"), ["42"], "{case}");
        // One typing action is one history entry, even after saving. Escape closes the
        // untouched editor before undoing the document rather than the TextEdit buffer.
        h.key_press(egui::Key::Escape);
        h.state_mut().undo();
        assert_eq!(value(&h, "qty"), ["4"], "{case}: a single undo restores the original");
        h.state_mut().redo();
        assert_eq!(value(&h, "qty"), ["42"], "{case}: redo restores the committed value");
        let _ = std::fs::remove_dir_all(&dir);
    }
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
    pdf_from_objects(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (qty) /V (4) /Rect [20 350 280 372] /P 3 0 R /F 4 /AA << /V << /S /JavaScript /JS (if (event.value > 100) { app.alert('At most 100'); event.rc = false; }) >> >> >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
    ])
}

#[test]
fn pressing_another_text_field_validates_the_refused_value_once() {
    // The press commits `qty`; its release must not commit (and alert) a second time.
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("order.pdf", None, validated_form_with_note()).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    h.run_steps(6);
    click_field(&mut h, "qty", 0);
    h.event(egui::Event::Text("500".into()));
    h.run_steps(2);
    let p = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == "note").unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen").center()
    };
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(2);
    assert!(h.state().toast.is_some(), "the press committed 4500 and the script refused it");
    h.state_mut().toast = None;
    h.drop_at(p);
    h.run_steps(3);
    assert_eq!(h.state().toast, None, "the release did not validate it again");
    let focus = h.state().views[0].forms.focus.clone().expect("the refused field stays open");
    assert_eq!((focus.name.as_str(), focus.text.as_str()), ("qty", "4500"));
}

/// `validated_form` plus a plain text field, `note`, under `qty`.
fn validated_form_with_note() -> Vec<u8> {
    pdf_from_objects(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 6 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 6 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (qty) /V (4) /Rect [20 350 280 372] /P 3 0 R /F 4 /AA << /V << /S /JavaScript /JS (if (event.value > 100) { app.alert('At most 100'); event.rc = false; }) >> >> >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (note) /Rect [20 300 280 322] /P 3 0 R /F 4 >>",
    ])
}

fn pdf_from_objects(objs: &[&str]) -> Vec<u8> {
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

/// Fields that sit on text, where the text layer would show its I-beam: a checked box drawn
/// with an "X" (appearance text is page text), an empty check box and a text field over printed
/// words (character-sheet dots are set on printed "OOOOO"), and a printed word with no field.
fn fields_over_text() -> Vec<u8> {
    let content = "BT /F1 12 Tf 20 250 Td (Strength) Tj ET BT /F1 12 Tf 20 200 Td (Background) Tj ET BT /F1 12 Tf 20 150 Td (Notes) Tj ET";
    let check = "q BT /Helv 11 Tf 2 2 Td (X) Tj ET Q";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R 6 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 8 0 R /ZaDb 9 0 R >> >> >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 7 0 R /Resources << /Font << /F1 8 0 R >> >> /Annots [4 0 R 5 0 R 6 0 R] >>".to_string(),
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /V /Yes /AS /Yes /Rect [20 300 32 312] /P 3 0 R /F 4 /DA (/Helv 11 Tf 0 g) /AP << /N << /Yes 10 0 R >> >> >>".to_string(),
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (strength) /V /Off /AS /Off /Rect [20 246 70 262] /P 3 0 R /F 4 /MK << /CA (l) >> /DA (/ZaDb 11 Tf 0 g) >>".to_string(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (background) /Rect [20 196 200 214] /P 3 0 R /F 4 >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len() + 1),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /ZapfDingbats >>".to_string(),
        format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 12 12] /Resources << /Font << /Helv 8 0 R >> >> /Length {} >>\nstream\n{check}\nendstream",
            check.len() + 1
        ),
    ];
    pdf_from_objects(&objs.iter().map(String::as_str).collect::<Vec<_>>())
}

#[test]
fn buttons_over_text_show_the_pointing_hand_not_the_i_beam() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("sheet.pdf", None, fields_over_text()).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    h.run_steps(6);
    let (agree, strength, background) = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let at = |name: &str| {
            let f = doc.form.iter().find(|f| f.name == name).unwrap();
            pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen")
        };
        (at("agree"), at("strength"), at("background"))
    };
    let cursor_at = |h: &mut Harness<'static, PdfCraftApp>, p: egui::Pos2| {
        h.hover_at(p);
        h.run_steps(2);
        h.output().platform_output.cursor_icon
    };
    // "Notes" (baseline y 150) lies 50 pt below "Background" (y 200), with no field on it.
    // Wait for the page text: the I-beam over it shows the text layer is live.
    let scale = background.width() / 180.0;
    let notes = egui::pos2(background.left() + 12.0 * scale, background.bottom() + (196.0 - 154.0) * scale);
    let mut over_notes = egui::CursorIcon::Default;
    for _ in 0..100 {
        over_notes = cursor_at(&mut h, notes);
        if over_notes == egui::CursorIcon::Text {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(over_notes, egui::CursorIcon::Text, "printed text off any field shows the I-beam");
    use egui::CursorIcon::{PointingHand, Text};
    let over_fields = [agree, strength, background].map(|r| cursor_at(&mut h, r.center()));
    assert_eq!(over_fields, [PointingHand, PointingHand, Text], "check boxes are buttons, not text to select; a text field keeps the I-beam");
    // Clicking still toggles them.
    click_field(&mut h, "agree", 0);
    assert!(value(&h, "agree").is_empty());
    click_field(&mut h, "strength", 0);
    assert_eq!(value(&h, "strength"), ["Yes"]);
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
    refused_value_stops_save(egui::Key::Enter, "enter-refused");
}

#[test]
fn a_value_refused_on_tab_keeps_save_in_the_same_frame_from_running() {
    refused_value_stops_save(egui::Key::Tab, "tab-refused");
}

fn refused_value_stops_save(commit_key: egui::Key, case: &str) {
    let dir = scratch(case);
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
    one_frame(&mut h, vec![key(commit_key, egui::Modifiers::NONE), key(egui::Key::S, egui::Modifiers::COMMAND)]);
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

#[test]
fn calculated_field_and_script_side_effect_on_another_page_re_render() {
    let mut h = harness();
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::InsertBlankPage { at: 1, width: 300.0, height: 400.0 }));
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::AddField {
        page: 1,
        rect: [20.0, 20.0, 100.0, 40.0],
        kind: pdfcraft_engine::NewField::Text { multiline: false },
        name: Some("page2_field".into()),
    }));
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldScript {
        name: "page2_field".into(),
        event: "calculate".into(),
        script: Some("event.value = 'from_' + getField('name').value;".into()),
    }));
    for _ in 0..60 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("hello".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
    assert_eq!(value(&h, "page2_field"), ["from_hello"]);
    // Wait for all re-renders to complete.
    for _ in 0..60 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(value(&h, "name"), ["hello"]);
    assert_eq!(value(&h, "page2_field"), ["from_hello"]);
}

#[test]
fn overlay_clears_after_fresh_tiles_zoom_then_edit_and_rejected_edit() {
    let mut h = harness();
    // 1. Fresh tiles: edit puts field in unbaked, and after tiles arrive, unbaked clears.
    click_field(&mut h, "name", 0);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("GoodVal".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
    assert_eq!(value(&h, "name"), ["GoodVal"]);
    for _ in 0..60 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!h.state().views[0].forms.unbaked.contains(&("name".to_string(), 0)));

    // 2. Rejected edit: typing rejected by validate script leaves unbaked clean.
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldScript {
        name: "city".into(),
        event: "validate".into(),
        script: Some("event.rc = false;".into()),
    }));
    click_field(&mut h, "city", 0);
    h.event(egui::Event::Text("BadVal".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
    assert!(!h.state().views[0].forms.unbaked.contains(&("city".to_string(), 0)));
    h.key_press(egui::Key::Escape);
    h.run_steps(2);

    // 3. Zoom then edit: zoom to another scale, let tiles settle, then edit.
    h.state_mut().views[0].zoom_step(true);
    for _ in 0..60 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    click_field(&mut h, "name", 0);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("ZoomedVal".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
    assert_eq!(value(&h, "name"), ["ZoomedVal"]);
    for _ in 0..60 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!h.state().views[0].forms.unbaked.contains(&("name".to_string(), 0)));
}

/// A field whose /P is a page number past the document's end (#1307) is listed under
/// "Unplaced" instead of panicking while the Fields panel labels its page.
#[test]
fn the_fields_panel_lists_a_field_on_a_missing_page_as_unplaced() {
    use egui_kittest::kittest::Queryable;
    let bytes = pdf_from_objects(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (orphan) /Rect [10 10 100 30] /P 99 /F 4 >>",
    ]);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("orphan.pdf", None, bytes).unwrap();
        app.set_option("panel", "fields").unwrap();
        app
    });
    h.run_steps(6);
    h.get_by_label("Unplaced");
    assert_eq!(h.query_all_by_label("orphan").count(), 1);
}

#[test]
fn list_box_clicks_select_directly_and_ctrl_click_toggles_multi() {
    let bytes = pdf_from_objects(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 6 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 400 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (ListOne) /Opt [(Red) (Green) (Blue)] /Rect [50 250 200 320] /P 3 0 R /F 4 >>",
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (ListMulti) /Ff 2097152 /Opt [(Mon) (Tue) (Wed) (Thu) (Fri)] /Rect [50 100 200 170] /P 3 0 R /F 4 >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
    ]);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("lists.pdf", None, bytes).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("zoom", "100").unwrap();
        app
    });
    h.run_steps(4);

    // Click row 1 (Green) in ListOne
    let r_one = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == "ListOne").unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen")
    };
    let zoom_one = r_one.height() / 70.0;
    let p_green = egui::pos2(r_one.center().x, r_one.min.y + 19.0 * zoom_one);
    h.hover_at(p_green);
    h.run_steps(1);
    h.drag_at(p_green);
    h.run_steps(1);
    h.drop_at(p_green);
    h.run_steps(3);

    assert_eq!(value(&h, "ListOne"), ["Green"], "clicking row 1 in ListOne selected Green");

    // Click row 1 (Tue) in ListMulti
    let r_multi = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == "ListMulti").unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen")
    };
    let zoom_multi = r_multi.height() / 70.0;
    let p_tue = egui::pos2(r_multi.center().x, r_multi.min.y + 19.0 * zoom_multi);
    h.hover_at(p_tue);
    h.run_steps(1);
    h.drag_at(p_tue);
    h.run_steps(1);
    h.drop_at(p_tue);
    h.run_steps(3);

    assert_eq!(value(&h, "ListMulti"), ["Tue"], "clicking row 1 in ListMulti selected Tue");

    // Ctrl/Cmd-click row 3 (Thu) in ListMulti
    let p_thu = egui::pos2(r_multi.center().x, r_multi.min.y + 42.0 * zoom_multi);
    h.hover_at(p_thu);
    h.run_steps(1);
    h.event(egui::Event::PointerMoved(p_thu));
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::COMMAND));
    h.event(egui::Event::PointerButton { pos: p_thu, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::COMMAND });
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: p_thu, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::COMMAND });
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.run_steps(3);

    assert_eq!(value(&h, "ListMulti"), ["Tue", "Thu"], "Ctrl-clicking Thu toggled it on, keeping Tue");
}

/// Auto-size (`0 Tf`, inherited from the AcroForm) single-line and multiline fields, and a fixed 10 pt one.
fn auto_size_form() -> Vec<u8> {
    pdf_from_objects(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 6 0 R 7 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 6 0 R 7 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (short) /Rect [20 350 220 364] /P 3 0 R /F 4 >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (notes) /Ff 4096 /Rect [20 270 140 320] /P 3 0 R /F 4 >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (fixed) /DA (/Helv 10 Tf 0 g) /Rect [20 220 120 234] /P 3 0 R /F 4 >>",
    ])
}

fn auto_size_harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("auto.pdf", None, auto_size_form()).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    h.run_steps(6);
    h
}

/// The on-screen size for `text` in the field, and how wide (single-line) or tall (multiline,
/// wrapped at the box) that text is in the screen font, against the room inside the box (pt).
fn on_screen_fit(h: &Harness<'static, PdfCraftApp>, name: &str, text: &str) -> (f32, f32, f32) {
    let s = h.state();
    let f = s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == name).unwrap().clone();
    let w = f.widgets[0].clone();
    let painter = egui::Painter::new(h.ctx.clone(), egui::LayerId::background(), egui::Rect::EVERYTHING);
    let size = pdfcraft_ui_egui::forms_ui::field_text_size(&painter, &f, &w, text);
    let (bw, bh) = ((w.rect[2] - w.rect[0]) as f32 - 6.0, (w.rect[3] - w.rect[1]) as f32 - 6.0);
    let font = egui::FontId::proportional(size);
    if f.has(pdfcraft_engine::field_flags::MULTILINE) {
        (size, painter.layout(text.to_string(), font, egui::Color32::BLACK, bw).size().y, bh)
    } else {
        (size, painter.layout_no_wrap(text.to_string(), font, egui::Color32::BLACK).size().x, bw)
    }
}

#[test]
fn auto_size_fields_shrink_and_wrap_typed_text_to_fit_their_box() {
    let mut h = auto_size_harness();
    let long = "Weekly progress and diligence: CoBank refi, IL and Redwood Falls";
    click_field(&mut h, "short", 0);
    h.event(egui::Event::Text(long.into()));
    h.run_steps(2);
    let typed = h.state().views[0].forms.focus.clone().expect("the editor is open").text;
    assert_eq!(typed, long);
    let (size, used, room) = on_screen_fit(&h, "short", &typed);
    assert!(size > 4.0 && size < 8.0 / 1.15, "a long value shrinks in the short box: {size} pt");
    assert!(used <= room + 0.5, "and fits its width: {used} > {room}");
    let (short_size, ..) = on_screen_fit(&h, "short", "Hi");
    assert!(short_size > size, "short text keeps a larger size: {short_size}");

    let (size, used, room) = on_screen_fit(&h, "notes", long);
    assert!(used <= room + 0.5, "multiline text wraps and shrinks to the box height: {used} > {room} at {size} pt");
    assert!(size >= 6.0, "without shrinking further than it must: {size} pt");

    // An explicit size is kept, as in Acrobat: the text may then run past the box.
    assert_eq!(on_screen_fit(&h, "fixed", long).0, 10.0);
}
