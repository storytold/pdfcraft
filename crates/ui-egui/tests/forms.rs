//! Filling in a form in the real shell (egui_kittest): typing, Tab, check boxes, radios, choices.

use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
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
