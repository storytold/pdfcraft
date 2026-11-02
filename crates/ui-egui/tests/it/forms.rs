//! Filling in a form in the real shell (egui_kittest): typing, Tab, check boxes, radios, choices.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
        app.set_option("left", "closed").unwrap();
        // The whole 300×400 pt page on screen.
        app.set_option("zoom", "150").unwrap();
        app
    });
    crate::support::settle(&mut h);
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
    click_at(h, p);
}

fn click_at(h: &mut Harness<'static, PdfCraftApp>, p: egui::Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
}

/// Rest the pointer on the centre of a field's widget long enough for a tooltip.
fn rest_on_field(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    let p = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == name).unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen").center()
    };
    h.hover_at(p);
    for _ in 0..40 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
}

/// #3854: resting on a field shows its tooltip (`/TU`); a field without one shows none.
#[test]
fn hovering_a_field_shows_its_tooltip() {
    let mut h = harness();
    assert!(h.query_by_label("Your name").is_none(), "no tooltip before the hover");
    rest_on_field(&mut h, "name");
    assert!(h.query_all_by_label("Your name").next().is_some(), "the name field's /TU shows on hover");
    rest_on_field(&mut h, "city");
    assert!(h.query_by_label("Your name").is_none(), "another field doesn't keep the tooltip");
    // Nor shows one of its own: none has been shown since the pointer left `name` (well past the
    // tooltip delay), so this isn't only the old label going away.
    assert!(egui::Tooltip::seconds_since_last_tooltip(&h.ctx) > 0.3, "a field without /TU shows no tooltip");
}

/// Give the `city` field these properties, as the Properties dialog would.
fn set_city(h: &mut Harness<'static, PdfCraftApp>, props: pdfcraft_engine::FieldProps) {
    h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldProps { name: "city".into(), props: Box::new(props) });
    h.run_steps(3);
}

/// A read-only field still shows its tooltip: it can't be filled, but it can be identified.
#[test]
fn a_read_only_fields_tooltip_shows_on_hover() {
    let mut h = harness();
    set_city(&mut h, pdfcraft_engine::FieldProps { tooltip: Some("Where you live".into()), read_only: Some(true), ..Default::default() });
    rest_on_field(&mut h, "city");
    assert!(h.query_all_by_label("Where you live").next().is_some());
}

/// A `/TU` of only spaces shows no tooltip (no empty box under the pointer).
#[test]
fn a_blank_tooltip_shows_nothing() {
    let mut h = harness();
    set_city(&mut h, pdfcraft_engine::FieldProps { tooltip: Some("   ".into()), ..Default::default() });
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.form.iter().find(|f| f.name == "city").unwrap().tooltip.as_deref(), Some("   "), "the blank /TU is in the file");
    rest_on_field(&mut h, "city");
    assert!(egui::Tooltip::seconds_since_last_tooltip(&h.ctx).is_infinite(), "no tooltip was shown");
}

/// A text field that doesn't scroll (`DoNotScroll`) takes no more than it shows, measured as its
/// appearance draws it: a long paste keeps what fits, typing into a full field adds nothing, and
/// a field without the flag takes it all.
#[test]
fn a_field_that_doesnt_scroll_stops_taking_text_when_full() {
    let mut h = harness();
    set_city(
        &mut h,
        pdfcraft_engine::FieldProps { flags: vec![(pdfcraft_engine::field_flags::DO_NOT_SCROLL, true)], font_size: Some(12.0), ..Default::default() },
    );
    let text_box = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().field_text_box("city", 0).expect("the city field's text box")
    };
    let typing = |h: &Harness<'static, PdfCraftApp>| h.state().views[0].forms.focus.as_ref().map(|f| f.text.clone()).unwrap_or_default();
    click_field(&mut h, "city", 0);
    h.event(egui::Event::Text("W".repeat(80)));
    h.run_steps(2);
    let kept = typing(&h);
    let tb = text_box(&h);
    assert!(!kept.is_empty() && kept.len() < 80, "{} of 80 kept", kept.len());
    assert!(tb.fits(&kept) && !tb.fits(&format!("{kept}W")), "as much as fits: {}", kept.len());
    h.event(egui::Event::Text("WWW".into()));
    h.run_steps(2);
    assert_eq!(typing(&h), kept, "a full field takes no more");
    h.key_press(egui::Key::Tab);
    h.run_steps(4);
    assert_eq!(value(&h, "city"), [kept]);
    // A field without the flag takes the whole paste.
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("W".repeat(80)));
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(4);
    assert_eq!(value(&h, "name"), ["W".repeat(80)]);
}

/// A multi-line field that doesn't scroll, with an automatic size, takes lines until even the
/// smallest size it shrinks to can't show them all.
#[test]
fn a_multi_line_auto_sized_field_that_doesnt_scroll_stops_when_full() {
    let mut h = harness();
    set_city(
        &mut h,
        pdfcraft_engine::FieldProps {
            flags: vec![(pdfcraft_engine::field_flags::DO_NOT_SCROLL, true)],
            multiline: Some(true),
            font_size: Some(0.0),
            ..Default::default()
        },
    );
    let da = {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "city").unwrap().da.clone()
    };
    assert!(da.contains(" 0 Tf"), "an automatic size: {da}");
    click_field(&mut h, "city", 0);
    h.event(egui::Event::Text("line\n".repeat(60)));
    h.run_steps(2);
    let kept = h.state().views[0].forms.focus.as_ref().map(|f| f.text.clone()).unwrap_or_default();
    let tb = {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().field_text_box("city", 0).expect("the city field's text box")
    };
    let lines = kept.lines().count();
    assert!(lines > 1 && lines < 60, "{lines} of 60 lines kept");
    assert!(tb.fits(&kept) && !tb.fits(&format!("{kept}\nline")), "as many lines as fit at 4 pt: {lines}");
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
    assert_eq!(h.state().views[0].forms.focus.as_ref().map(|f| f.name.as_str()), Some("agree"), "Tab moves to the next field");
    // Escape leaves the check box. It also abandons a draft.
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

fn focused(h: &Harness<'static, PdfCraftApp>) -> Option<(String, usize)> {
    h.state().views[0].forms.focus.as_ref().map(|f| (f.name.clone(), f.widget))
}

/// A text field, a check box, another text field and a pair of radio buttons, one under the
/// other, then two empty pages.
fn check_box_harness() -> Harness<'static, PdfCraftApp> {
    let bytes = pdf_from_objects(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R 6 0 R 10 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 7 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R 9 0 R 13 0 R] /Count 3 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R 11 0 R 12 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (first) /Rect [20 350 280 372] /P 3 0 R /F 4 >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /V /Off /AS /Off /Rect [20 310 36 326] /P 3 0 R /F 4 /AP << /N << /Yes 8 0 R /Off 8 0 R >> >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (last) /Rect [20 270 280 292] /P 3 0 R /F 4 >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 16 16] /Length 0 >>\nstream\n\nendstream",
        "<< /Type /Page /Parent 2 0 R >>",
        "<< /FT /Btn /T (pick) /Ff 32768 /V /Off /Kids [11 0 R 12 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /Parent 10 0 R /AS /Off /Rect [20 230 36 246] /P 3 0 R /F 4 /AP << /N << /A 8 0 R /Off 8 0 R >> >> >>",
        "<< /Type /Annot /Subtype /Widget /Parent 10 0 R /AS /Off /Rect [60 230 76 246] /P 3 0 R /F 4 /AP << /N << /B 8 0 R /Off 8 0 R >> >> >>",
        "<< /Type /Page /Parent 2 0 R >>",
    ]);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("agree.pdf", None, bytes).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    h.run_steps(6);
    h
}

#[test]
fn tab_and_space_fill_in_a_check_box_between_text_fields() {
    // Issue #2223: Tab went from `first` straight to `last`, and Space could not tick `agree`.
    let mut h = check_box_harness();
    click_field(&mut h, "first", 0);
    h.event(egui::Event::Text("Ada".into()));
    h.run_steps(1);
    h.key_press(egui::Key::Tab);
    h.run_steps(3);
    assert_eq!(value(&h, "first"), ["Ada"]);
    assert_eq!(focused(&h), Some(("agree".into(), 0)), "Tab stops on the check box");
    assert!(h.ctx.egui_wants_keyboard_input(), "the check box has the keyboard, not the page");
    h.key_press(egui::Key::Space);
    h.run_steps(3);
    assert_eq!(value(&h, "agree"), ["Yes"], "Space ticks it");
    assert_eq!(focused(&h), Some(("agree".into(), 0)), "and it keeps the focus");
    h.key_press(egui::Key::Space);
    h.run_steps(3);
    assert!(value(&h, "agree").is_empty(), "Space again clears it");
    h.key_press(egui::Key::Tab);
    h.run_steps(3);
    assert_eq!(focused(&h), Some(("last".into(), 0)));
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Tab);
    h.run_steps(3);
    assert_eq!(focused(&h), Some(("agree".into(), 0)), "Shift+Tab comes back to it");
}

#[test]
fn clicking_a_check_box_while_typing_keeps_the_text_and_focuses_the_box() {
    // Issue #2223: the click ticked the box but left no field focused, so Tab started over.
    let mut h = check_box_harness();
    click_field(&mut h, "first", 0);
    h.event(egui::Event::Text("Ada".into()));
    h.run_steps(1);
    click_field(&mut h, "agree", 0);
    assert_eq!(value(&h, "agree"), ["Yes"]);
    assert_eq!(value(&h, "first"), ["Ada"], "the text typed before the click is kept");
    assert_eq!(focused(&h), Some(("agree".into(), 0)), "the click focuses the check box");
    h.key_press(egui::Key::Tab);
    h.run_steps(3);
    assert_eq!(focused(&h), Some(("last".into(), 0)), "so Tab goes on from there");
}

#[test]
fn page_keys_still_work_with_a_check_box_or_radio_button_focused() {
    // The widget that takes Space for a focused box must not swallow the page keys.
    let mut h = check_box_harness();
    click_field(&mut h, "agree", 0);
    assert_eq!(focused(&h), Some(("agree".into(), 0)));
    h.key_press(egui::Key::End);
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 2, "End goes to the last page");
    assert!(h.state().views[0].page_screen_rect(0).is_none(), "where the check box is out of view");
    // Scrolled away, the box lets go: back on its page, Space no longer toggles it.
    h.key_press(egui::Key::Home);
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 0);
    assert_eq!(focused(&h), None, "the box let go of the focus");
    h.key_press(egui::Key::Space);
    h.run_steps(3);
    assert_eq!(value(&h, "agree"), ["Yes"]);
    // A radio button keeps the arrows for its group, so → selects B instead of turning the page.
    click_field(&mut h, "pick", 0);
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(3);
    assert_eq!(value(&h, "pick"), ["B"]);
    assert_eq!(h.state().views[0].current, 0, "the arrow stayed in the group");
    // Once scrolled away, an arrow scrolls again instead of changing the choice.
    h.key_press(egui::Key::End);
    h.run_steps(4);
    h.key_press(egui::Key::Home);
    h.run_steps(4);
    h.key_press(egui::Key::ArrowUp);
    h.run_steps(3);
    assert_eq!((focused(&h), value(&h, "pick")), (None, vec!["B".to_string()]));
}

#[test]
fn radio_buttons_take_tab_space_and_arrows() {
    let mut h = harness();
    click_field(&mut h, "agree", 0);
    h.key_press(egui::Key::Tab);
    h.run_steps(3);
    assert_eq!(focused(&h), Some(("size".into(), 0)));
    h.key_press(egui::Key::Space);
    h.run_steps(3);
    assert_eq!(value(&h, "size"), ["S"], "Space selects the focused button");
    h.key_press(egui::Key::Space);
    h.run_steps(3);
    assert_eq!(value(&h, "size"), ["S"], "and doesn't turn it off again");
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(3);
    assert_eq!(focused(&h), Some(("size".into(), 1)), "an arrow moves within the group");
    assert_eq!(value(&h, "size"), ["L"], "and selects the button it lands on");
    h.key_press(egui::Key::ArrowLeft);
    h.run_steps(3);
    assert_eq!((focused(&h), value(&h, "size")), (Some(("size".into(), 0)), vec!["S".to_string()]));
    h.key_press(egui::Key::Tab);
    h.run_steps(3);
    h.key_press(egui::Key::Tab);
    h.run_steps(3);
    assert_eq!(focused(&h), Some(("country".into(), 0)), "past the group's second button");
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
    // name → (Tab) agree → (Tab) size, both buttons → (Tab) country → (Tab) city, whose text is
    // selected on entry.
    click_field(&mut h, "name", 0);
    h.key_press(egui::Key::Tab);
    h.run_steps(3);
    assert_eq!(focused(&h), Some(("agree".into(), 0)));
    for _ in 0..3 {
        h.key_press(egui::Key::Tab);
        h.run_steps(3);
    }
    assert_eq!(focused(&h), Some(("country".into(), 0)));
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
        (rect, egui::Id::unique(("form-field", s.views[0].id.0, "name", 0usize)))
    };
    // From inside the first word ("Ad|a" at the field's auto size, 13.9 pt) to past the end of the text.
    let (from, to) = (egui::pos2(rect.left() + 38.0, rect.center().y), egui::pos2(rect.right() - 8.0, rect.center().y));
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

/// #4274: the calendar had no way to pick today; from another month the only way back was the
/// arrows. Today is picked in each field's own format, whichever month the calendar shows, and
/// is one edit as a clicked day is.
#[test]
fn the_calendar_picks_today_in_the_fields_format_from_any_month() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness();
    // 2023-11-14 on the injected clock.
    let s = h.state_mut();
    s.session = std::mem::take(&mut s.session).with_clock(|| 1_700_000_000);
    assert_eq!(h.state().session.today(), (2023, 11, 14));
    for (name, rect) in [("due", [50.0, 40.0, 200.0, 60.0]), ("start", [50.0, 80.0, 200.0, 100.0])] {
        h.state_mut().apply_edit(pdfcraft_engine::Edit::AddField { page: 0, rect, kind: pdfcraft_engine::NewField::Date, name: Some(name.into()) });
    }
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldProps {
        name: "start".into(),
        props: Box::new(pdfcraft_engine::FieldProps {
            format: Some(pdfcraft_engine::form_scripts::Format::Date("yyyy-mm-dd".into())),
            ..Default::default()
        }),
    }));
    h.run_steps(4);
    // Two months on, and two months back: Today still picks the 14th of November.
    for (name, arrow, shown, want) in [("due", "›", "January 2024", "11/14/2023"), ("start", "‹", "September 2023", "2023-11-14")] {
        click_field(&mut h, name, 0);
        h.get_by_label("November 2023");
        for _ in 0..2 {
            h.get_by_label(arrow).click();
            h.run_steps(2);
        }
        h.get_by_label(shown);
        if let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
            h.render().unwrap().save(format!("{dir}/calendar-today-{name}.png")).unwrap();
        }
        h.get_by_label("Today").click();
        h.run_steps(4);
        assert_eq!(value(&h, name), [want], "today in {name}'s format");
        assert!(h.state().views[0].forms.focus.is_none(), "picking closes the field, as a clicked day does");
        assert!(h.query_by_label("Today").is_none(), "and its calendar");
    }
    // One edit each, as a clicked day is.
    h.state_mut().undo();
    h.run_steps(2);
    assert_eq!(value(&h, "start"), Vec::<String>::new());
    assert_eq!(value(&h, "due"), ["11/14/2023"]);
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

/// A fresh folder of its own for one test. The tests of this binary share one process id and run
/// in parallel, and two of them name their folder "same-frame": a number taken per call keeps one
/// test from clearing the folder another is saving into.
fn scratch(name: &str) -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("pdfcraft-forms-{name}-{}-{n}", std::process::id()));
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
    h.state_mut().open_bytes("other.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
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
    h.state_mut().export_data_dialog(pdfcraft_ui_egui::DataKind::FormData);
    h.run_steps(2);
    h.get_by_label("Export").click();
    h.run_steps(2);
    assert!(std::fs::read_to_string(&out).unwrap().contains("Exported value"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// #5215: Export form data asks for the format in PdfCraft (the macOS save panel shows no list of
/// file types) and writes the format chosen there. The file has no extension, so only the
/// dialog's choice can make it CSV.
#[test]
fn export_form_data_asks_for_the_format_and_writes_that_format() {
    let dir = scratch("export-format");
    let out = dir.join("data");
    let mut h = harness();
    assert!(h.state_mut().execute("form.export_data"));
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::ExportData(pdfcraft_ui_egui::DataKind::FormData)));
    let format = |h: &Harness<'static, PdfCraftApp>, value: &str| {
        h.query_all_by_role(egui::accesskit::Role::ComboBox).any(|n| n.value().as_deref() == Some(value))
    };
    assert!(format(&h, "XFDF (.xfdf)"), "XFDF is the default for an AcroForm");
    h.get_by(|n| n.role() == egui::accesskit::Role::ComboBox && n.value().is_some_and(|v| v == "XFDF (.xfdf)")).click();
    h.run_steps(2);
    for offered in ["FDF (.fdf)", "XML (.xml)", "Text (.txt)"] {
        h.get_by_label(offered);
    }
    h.get_by_label("CSV (.csv)").click();
    h.run_steps(2);
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    h.get_by_label("Export").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog, None, "the dialog closes once the file is chosen");
    let written = std::fs::read_to_string(&out).unwrap();
    assert!(!written.trim_start().starts_with('<') && written.lines().next().is_some_and(|l| l.contains(',')), "CSV, not XML: {written:?}");
    // The choice is remembered for the next export.
    assert!(h.state_mut().execute("form.export_data"));
    h.run_steps(2);
    assert!(format(&h, "CSV (.csv)"), "the last format chosen is offered first");
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog, None);
    // Export comments, which has no CSV, shows its own default; cancelling it keeps CSV.
    h.state_mut().export_data_dialog(pdfcraft_ui_egui::DataKind::Comments);
    h.run_steps(2);
    assert!(format(&h, "XFDF (.xfdf)"), "comments are XFDF or FDF");
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(h.state_mut().execute("form.export_data"));
    h.run_steps(2);
    assert!(format(&h, "CSV (.csv)"), "a cancelled comments export does not replace the form data's format");
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A format the export does not offer is refused, not written as other data: comments have no
/// CSV, and CSV would hold the form's values instead.
#[test]
fn export_data_refuses_a_format_its_kind_does_not_offer() {
    let dir = scratch("export-refused");
    let out = dir.join("comments.csv");
    let mut h = harness();
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    h.state_mut().export_data_as(pdfcraft_ui_egui::DataKind::Comments, pdfcraft_engine::DataFormat::Csv);
    h.run_steps(2);
    assert!(!out.exists(), "nothing is written");
    let toast = h.state().toast.as_ref().map(|(m, _)| m.clone()).unwrap_or_default();
    assert_eq!(toast, "CSV can't hold comments");
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
    h.state_mut().open_bytes("other.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
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
    crate::support::settle(&mut h);
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("hello".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
    assert_eq!(value(&h, "page2_field"), ["from_hello"]);
    // Wait for all re-renders to complete.
    crate::support::settle(&mut h);
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
    crate::support::settle(&mut h);
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
    crate::support::settle(&mut h);
    click_field(&mut h, "name", 0);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("ZoomedVal".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
    assert_eq!(value(&h, "name"), ["ZoomedVal"]);
    crate::support::settle(&mut h);
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

/// Text typed into an auto-size field shrinks to fit the box's width, while short text keeps a
/// larger size; a multiline auto-size field wraps and shrinks only as far as the box's height
/// needs, and a fixed 10 pt field keeps its size.
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

/// Auto-size fields 500 pt wide: a four-cell comb 30 pt tall, and single-line fields 100 and 400 pt tall.
fn tall_auto_size_form() -> Vec<u8> {
    pdf_from_objects(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 6 0 R 7 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 6 0 R 7 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (comb) /Ff 16777216 /MaxLen 4 /Rect [50 740 550 770] /P 3 0 R /F 4 >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (tall) /Rect [50 600 550 700] /P 3 0 R /F 4 >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (huge) /Rect [50 100 550 500] /P 3 0 R /F 4 >>",
    ])
}

/// The editor shows auto-size text at the size the page appearance draws it: a comb field keeps
/// the appearance's 12 pt cap, and a tall single-line field follows its height, at 100% zoom
/// too, up to the editor's screen-font bound.
#[test]
fn the_field_editor_sizes_auto_size_text_as_the_page_does() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("tall.pdf", None, tall_auto_size_form()).unwrap();
        app
    });
    h.run_steps(6);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    let field = |name: &str| doc.form.iter().find(|f| f.name == name).unwrap().clone();
    let painter = egui::Painter::new(h.ctx.clone(), egui::LayerId::background(), egui::Rect::EVERYTHING);
    let screen = |name: &str, zoom: f32| {
        let f = field(name);
        pdfcraft_ui_egui::forms_ui::field_screen_font(&painter, &f, &f.widgets[0], "Hi", zoom)
    };
    // 100%: 96 screen points per 72 page points.
    let full = 96.0 / 72.0;

    let comb = field("comb");
    let page = pdfcraft_engine::field_comb_auto_text_size(30.0 - 6.0);
    assert_eq!(page, 12.0);
    assert_eq!(pdfcraft_ui_egui::forms_ui::field_text_size(&painter, &comb, &comb.widgets[0], ""), 12.0, "a comb field");
    assert_eq!(screen("comb", full), 12.0 * full);

    let tall = (100.0 - 6.0) / 1.15;
    assert!((screen("tall", full) - tall as f32 * full).abs() < 0.01, "a 100 pt field at 100%: {}", screen("tall", full));
    assert_eq!(screen("huge", full), pdfcraft_ui_egui::forms_ui::MAX_SCREEN_FONT, "past the screen-font bound");
    assert_eq!(screen("huge", 0.5), ((400.0 - 6.0) / 1.15) as f32 * 0.5, "below it at 50%");
}

/// Where a mode bar tab is, by its label.
fn mode_tab(h: &Harness<'static, PdfCraftApp>, label: &str) -> egui::Rect {
    use egui_kittest::kittest::Queryable;
    h.get_all_by_role_and_label(egui::accesskit::Role::Button, label).map(|n| n.rect()).find(|r| r.bottom() < 120.0).expect("a mode tab")
}

/// Scroll the page until `what` (on screen) lies across the mode bar, then the cursor over each
/// tab it lies under: with the All tools panel closed, Edit, Convert and E-Sign (the name field
/// starts right of All tools).
fn mode_tab_cursors(
    h: &mut Harness<'static, PdfCraftApp>,
    what: impl Fn(&Harness<'static, PdfCraftApp>) -> egui::Rect,
) -> [(&'static str, egui::CursorIcon); 3] {
    let page = |h: &Harness<'static, PdfCraftApp>| h.state().views[0].page_screen_rect(0).expect("on screen");
    let y = what(h).center().y - page(h).top();
    let tabs = mode_tab(h, "Convert").center().y;
    h.state_mut().views[0].goto = Some((0, 0.0));
    h.run_steps(4);
    let top = page(h).top();
    h.state_mut().views[0].goto = Some((0, (top - (tabs - y)) / page(h).height()));
    h.run_steps(4);
    let under = what(h);
    ["Edit", "Convert", "E-Sign"].map(|label| {
        let at = mode_tab(h, label).center();
        assert!(under.contains(at), "{under:?} lies under {label} at {at:?}");
        h.hover_at(at);
        h.run_steps(2);
        (label, h.output().platform_output.cursor_icon)
    })
}

const HAND_ON_EVERY_TAB: [(&str, egui::CursorIcon); 3] =
    [("Edit", egui::CursorIcon::PointingHand), ("Convert", egui::CursorIcon::PointingHand), ("E-Sign", egui::CursorIcon::PointingHand)];

/// A text field scrolled up under the mode bar leaves its tabs the hand (#2755). The field gave
/// them its I-beam, and its open editor floated over them and took their clicks.
#[test]
fn mode_tabs_keep_the_hand_over_a_text_field_scrolled_under_them() {
    let mut h = harness();
    // The page across the window, so the name field reaches under the tabs.
    h.state_mut().views[0].fit = pdfcraft_ui_egui::canvas::Fit::Width;
    h.run_steps(4);
    let name = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == "name").unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen")
    };
    assert_eq!(mode_tab_cursors(&mut h, name), HAND_ON_EVERY_TAB, "the field under the tabs");
    // Open it, where it shows.
    h.state_mut().views[0].goto = Some((0, 0.0));
    h.run_steps(4);
    click_field(&mut h, "name", 0);
    assert!(h.state().views[0].forms.focus.is_some(), "the editor opened");
    assert_eq!(mode_tab_cursors(&mut h, name), HAND_ON_EVERY_TAB, "the open field under the tabs");
    let convert = mode_tab(&h, "Convert").center();
    click_at(&mut h, convert);
    assert_eq!(h.state().mode, pdfcraft_ui_egui::Mode::Convert, "the tab takes the click, not the field's editor");
}

/// Two combo boxes of Apple and Banana: `fruit` editable (Combo and Edit, ISO 32000-2 Table 233),
/// `veg` a drop-down list only.
fn combo_form() -> Vec<u8> {
    pdf_from_objects(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 6 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 6 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (fruit) /Ff 393216 /Opt [(Apple) (Banana)] /Rect [20 350 280 372] /P 3 0 R /F 4 >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (veg) /Ff 131072 /Opt [(Apple) (Banana)] /Rect [20 300 280 322] /P 3 0 R /F 4 >>",
    ])
}

fn combo_harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("combo.pdf", None, combo_form()).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    crate::support::settle(&mut h);
    h
}

/// An editable combo box takes a typed value that isn't among its options, and still takes a
/// pick from its list; the typed value is what Save writes.
#[test]
fn an_editable_combo_box_takes_a_typed_value() {
    let dir = scratch("editable-combo");
    let out = dir.join("saved.pdf");
    let mut h = combo_harness();
    click_field(&mut h, "fruit", 0);
    assert_eq!(focused(&h), Some(("fruit".into(), 0)));
    h.event(egui::Event::Text("Cherry".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert_eq!(value(&h, "fruit"), ["Cherry"], "Enter commits the typed value");
    assert!(h.state().views[0].forms.focus.is_none(), "and closes the field");
    // Reopened, it shows the typed value, and the list still picks an option.
    click_field(&mut h, "fruit", 0);
    assert_eq!(h.state().views[0].forms.focus.as_ref().map(|f| f.text.as_str()), Some("Cherry"));
    h.get_by_label("Banana").click();
    h.run_steps(4);
    assert_eq!(value(&h, "fruit"), ["Banana"], "a pick from the list");
    // Typed again, Tab commits it, and Save writes it.
    click_field(&mut h, "fruit", 0);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("Damson".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(4);
    assert_eq!(value(&h, "fruit"), ["Damson"], "Tab commits the typed value");
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::S);
    wait_for_file(&mut h, &out);
    assert_eq!(saved_value(&out, "fruit"), ["Damson"]);
    // Undo takes the typing back as one step.
    h.state_mut().undo();
    h.run_steps(2);
    assert_eq!(value(&h, "fruit"), ["Banana"]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A combo box without the Edit flag is a drop-down list only: typing changes nothing.
#[test]
fn a_combo_box_without_edit_takes_no_typing() {
    let mut h = combo_harness();
    click_field(&mut h, "veg", 0);
    assert_eq!(focused(&h), Some(("veg".into(), 0)));
    h.event(egui::Event::Text("Cherry".into()));
    h.run_steps(2);
    assert_eq!(h.state().views[0].forms.focus.as_ref().map(|f| f.text.as_str()), Some(""), "no text box took the typing");
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(4);
    assert!(value(&h, "veg").is_empty(), "nothing typed reaches the field");
    // Tab went on to `fruit`, whose open list covers `veg`: close it first.
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    click_field(&mut h, "veg", 0);
    h.get_by_label("Apple").click();
    h.run_steps(4);
    assert_eq!(value(&h, "veg"), ["Apple"], "its list still picks");
}

/// A button script's `ShowHideFileAttachment` opens the attachments panel, and running it again
/// closes it.
#[test]
fn a_form_button_shows_and_hides_the_attachments_panel() {
    let mut h = harness();
    h.state_mut().set_option("panel", "none").unwrap();
    h.run_steps(2);
    let id = h.state().views[0].id;
    h.state_mut().run_button_script(id, "size", "app.execMenuItem('ShowHideFileAttachment');");
    h.run_steps(2);
    assert_eq!(h.state().right, Some(pdfcraft_ui_egui::RightPanel::Attachments));
    // Show/Hide, as its name says: asked again, it closes.
    h.state_mut().run_button_script(id, "size", "app.execMenuItem('ShowHideFileAttachment');");
    h.run_steps(2);
    assert_eq!(h.state().right, None);
}
