//! Filling in a form in the real shell (egui_kittest): typing, Tab, check boxes, radios, choices.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

use std::sync::Arc;

use crate::support::AppHarness as _;
use craft::host::{AppInfo, Host, Picked, ScriptedFiles};
use craft::ui::runtime::env::WindowKey;
use craft::ui::runtime::toast;

/// The app with `name` open from `bytes` at `zoom` percent, on a host whose file picker answers the
/// first save or export with `out`, as the user choosing that file would. The host keeps what the
/// app writes in the returned store, which is where a test reads it back.
fn scripted_harness(
    name: &'static str,
    bytes: Vec<u8>,
    zoom: &'static str,
    out: &std::path::Path,
) -> (Harness<'static, PdfCraftApp>, Arc<ScriptedFiles>) {
    let files = Arc::new(ScriptedFiles::answering([Picked::Paths(vec![out.to_path_buf()])]));
    let host = Arc::new(Host::builder(AppInfo { id: "pdfcraft", name: "PdfCraft", version: "0" }).files(files.clone()).build());
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = PdfCraftApp::with_host(host, crate::control::services());
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), name, None, bytes).unwrap();
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "zoom", zoom).unwrap();
            app
        }
    });
    crate::support::settle(&mut h);
    (h, files)
}

/// The text of the newest notice on screen, if one is shown.
fn notice(h: &Harness<'static, PdfCraftApp>) -> Option<String> {
    toast::cards(&h.ctx, &WindowKey::main())
        .last()
        .map(|card| card.toast.title.as_ref().map_or_else(|| card.toast.message.as_str(), |t| t.as_str()).to_owned())
}

fn undo(h: &mut Harness<'static, PdfCraftApp>) {
    let ctx = h.ctx.clone();
    h.state_mut().undo(&ctx);
}

fn redo(h: &mut Harness<'static, PdfCraftApp>) {
    let ctx = h.ctx.clone();
    h.state_mut().redo(&ctx);
}

/// Runs the button script of field `field` in document `id`, with the harness's context.
fn run_button_script(h: &mut Harness<'static, PdfCraftApp>, id: pdfcraft_engine::DocId, field: &str, script: &str) {
    let ctx = h.ctx.clone();
    h.state_mut().run_button_script(&ctx, id, field, script);
}

/// How many notices the main window shows.
fn card_count(h: &Harness<'static, PdfCraftApp>) -> usize {
    toast::cards(&h.ctx, &WindowKey::main()).len()
}

/// Whether the "Save changes?" prompt is on screen: an alert dialog, the framework's role for the question.
fn asks_to_save(h: &Harness<'static, PdfCraftApp>) -> bool {
    h.query_by(|n| n.role() == egui::accesskit::Role::AlertDialog && n.label().is_some_and(|l| l.starts_with("Save changes to “"))).is_some()
}

/// The unsaved-changes question's Save answer in the platform's words (production's wording, on every platform).
fn save_answer() -> &'static str {
    "Save"
}

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            // The whole 300×400 pt page on screen.
            app.set_option(&egui::Context::default(), "zoom", "150").unwrap();
            app
        }
    });
    crate::support::settle(&mut h);
    h
}

fn harness_with_bytes(name: &'static str, bytes: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), name, None, bytes).unwrap();
            app.set_option(&egui::Context::default(), "zoom", "150").unwrap();
            app
        }
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

/// Give the `city` field these properties, as the Properties dialog would.
fn set_city(h: &mut Harness<'static, PdfCraftApp>, props: pdfcraft_engine::FieldProps) {
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::SetFieldProps { name: "city".into(), props: Box::new(props) });
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

#[test]
fn read_only_widgets_are_skipped_by_tab_navigation() {
    let bytes = pdf_from_objects(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R 7 0 R 8 0 R 9 0 R] >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 120] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 6 0 R 10 0 R 7 0 R 8 0 R 9 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (first) /Rect [20 80 150 100] /P 3 0 R /F 4 >>",
        "<< /FT /Tx /T (locked) /Kids [6 0 R 10 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /Parent 5 0 R /Ff 1 /Rect [20 50 150 70] /P 3 0 R /F 4 >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (empty) /Rect [20 50 20 50] /P 3 0 R /F 4 >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (off-page) /Rect [220 50 240 70] /P 3 0 R /F 4 >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (last) /Rect [20 20 150 40] /P 3 0 R /F 4 >>",
        "<< /Type /Annot /Subtype /Widget /Parent 5 0 R /Rect [20 35 150 45] /P 3 0 R /F 4 >>",
    ]);
    let mut h = harness_with_bytes("readonly-widget.pdf", bytes);
    {
        let doc = h.state().session.get(h.state().views[0].id).unwrap();
        let locked = doc.form.iter().find(|field| field.name == "locked").unwrap();
        assert!(!locked.read_only(), "the field itself is editable");
        assert!(locked.widgets[0].read_only, "only its widget is read-only");
        let order = pdfcraft_ui_egui::forms_ui::tab_order(&doc.form, &doc.info)
            .into_iter()
            .filter_map(|(fi, wi)| doc.form.get(fi).map(|field| (field.name.as_str(), wi)))
            .collect::<Vec<_>>();
        assert_eq!(order, vec![("first", 0), ("locked", 1), ("last", 0)], "read-only, empty and off-page widgets are excluded");
    }
    click_field(&mut h, "first", 0);
    h.key_press(egui::Key::Tab);
    h.run_steps(3);
    assert_eq!(focused(&h), Some(("locked".into(), 1)), "the writable widget in the same field remains in tab order");
    h.key_press(egui::Key::Tab);
    h.run_steps(3);
    assert_eq!(focused(&h), Some(("last".into(), 0)));
}

/// A `/TU` of only spaces shows no tooltip (no empty box under the pointer).
#[test]
fn a_blank_tooltip_shows_nothing() {
    use egui::accesskit::Role;
    let mut h = harness();
    set_city(&mut h, pdfcraft_engine::FieldProps { tooltip: Some("   ".into()), ..Default::default() });
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.form.iter().find(|f| f.name == "city").unwrap().tooltip.as_deref(), Some("   "), "the blank /TU is in the file");
    rest_on_field(&mut h, "city");
    assert_eq!(h.query_all_by_role(Role::Tooltip).count(), 0, "a blank /TU shows no tooltip");
    // The same rest does show a tooltip for a field whose /TU has text, so the absence above means something.
    set_city(&mut h, pdfcraft_engine::FieldProps { tooltip: Some("Where you live".into()), ..Default::default() });
    rest_on_field(&mut h, "city");
    assert!(h.query_all_by_role(Role::Tooltip).count() > 0, "a /TU with text shows a tooltip");
    assert!(h.query_all_by_label("Where you live").next().is_some(), "with the /TU text");
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
    let typing = |h: &Harness<'static, PdfCraftApp>| h.state().views[0].session.forms.focus.as_ref().map(|f| f.text.clone()).unwrap_or_default();
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
    let kept = h.state().views[0].session.forms.focus.as_ref().map(|f| f.text.clone()).unwrap_or_default();
    let tb = {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().field_text_box("city", 0).expect("the city field's text box")
    };
    let lines = kept.lines().count();
    assert!(lines > 1 && lines < 60, "{lines} of 60 lines kept");
    assert!(tb.fits(&kept) && !tb.fits(&format!("{kept}\nline")), "as many lines as fit at 4 pt: {lines}");
}

/// #4296: a field turned a quarter (`/MK /R` 90) is typed into where it is, with its text running
/// up the field as the page draws it: a click near the field's top puts the caret at the end of
/// the text, where the turned text ends, not at its start.
#[test]
fn a_turned_field_places_the_caret_along_its_turned_text() {
    let mut h = harness();
    // Tall and narrow, with room up it for the text.
    let props = pdfcraft_engine::FieldProps { rect: Some((0, [240.0, 20.0, 260.0, 170.0])), font_size: Some(10.0), ..Default::default() };
    set_city(&mut h, props);
    set_city(&mut h, pdfcraft_engine::FieldProps { rotation: Some((0, 90)), ..Default::default() });
    click_field(&mut h, "city", 0);
    assert_eq!(h.state().views[0].session.forms.focus.as_ref().map(|f| f.name.as_str()), Some("city"));
    h.event(egui::Event::Text("Paris".into()));
    h.run_steps(2);
    let field = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == "city").unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen")
    };
    click_at(&mut h, field.left_top() + egui::vec2(3.0, 2.0));
    h.event(egui::Event::Text("X".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert_eq!(value(&h, "city"), ["ParisX"]);
}

#[test]
fn typing_into_a_text_field_and_tabbing_on() {
    let mut h = harness();
    click_field(&mut h, "name", 0);
    assert!(h.state().views[0].session.forms.focus.is_some(), "the editor opened");
    h.event(egui::Event::Text("Ada Lovelace".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(4);
    assert_eq!(value(&h, "name"), ["Ada Lovelace"]);
    assert_eq!(h.state().views[0].session.forms.focus.as_ref().map(|f| f.name.as_str()), Some("agree"), "Tab moves to the next field");
    // Escape leaves the check box. It also abandons a draft.
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    click_field(&mut h, "city", 0);
    assert_eq!(h.state().views[0].session.forms.focus.as_ref().map(|f| f.name.as_str()), Some("city"));
    h.event(egui::Event::Text("Paris".into()));
    h.run_steps(1);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(value(&h, "city").is_empty());
    assert!(h.state().views[0].session.forms.focus.is_none());
}

/// One step of a row of `field_keys_and_clicks_set_field_values`.
#[derive(Clone, Copy)]
enum Act {
    Click(&'static str, usize),
    Type(&'static str),
    Press(egui::Key),
    ShiftTab,
    Pick(&'static str),
}

/// One row of `field_keys_and_clicks_set_field_values`: the steps run on a fresh form, then what the
/// form holds. A `None` focus is not checked.
struct Row {
    case: &'static str,
    boxes: bool,
    acts: Vec<Act>,
    values: Vec<(&'static str, Vec<&'static str>)>,
    focus: Option<(&'static str, usize)>,
    keyboard: bool,
}

fn row(case: &'static str, boxes: bool, acts: &[Act], values: &[(&'static str, &[&'static str])], focus: Option<(&'static str, usize)>) -> Row {
    Row { case, boxes, acts: acts.to_vec(), values: values.iter().map(|(n, v)| (*n, v.to_vec())).collect(), focus, keyboard: false }
}

/// Clicks and keys on check boxes, radio buttons, choices and the text fields between them set the
/// field values and move the focus as filling in a form does.
#[test]
fn field_keys_and_clicks_set_field_values() {
    use Act::{Click, Pick, Press, ShiftTab, Type};
    use egui::Key::{ArrowLeft, ArrowRight, Space, Tab};
    let rows = [
        row("a click ticks a check box", false, &[Click("agree", 0)], &[("agree", &["Yes"])], None),
        row("a second click clears it", false, &[Click("agree", 0), Click("agree", 0)], &[("agree", &[])], None),
        row("a click picks a radio button", false, &[Click("size", 1)], &[("size", &["L"])], None),
        row("another click moves the pick", false, &[Click("size", 1), Click("size", 0)], &[("size", &["S"])], None),
        row("a click in the list picks a choice", false, &[Click("country", 0), Pick("France")], &[("country", &["fr"])], None),
        row("Tab goes from a check box to the radio group", false, &[Click("agree", 0), Press(Tab)], &[], Some(("size", 0))),
        row("Space selects the focused radio button", false, &[Click("agree", 0), Press(Tab), Press(Space)], &[("size", &["S"])], Some(("size", 0))),
        row(
            "and doesn't turn it off again",
            false,
            &[Click("agree", 0), Press(Tab), Press(Space), Press(Space)],
            &[("size", &["S"])],
            Some(("size", 0)),
        ),
        row(
            "an arrow moves within the group and selects",
            false,
            &[Click("agree", 0), Press(Tab), Press(Space), Press(ArrowRight)],
            &[("size", &["L"])],
            Some(("size", 1)),
        ),
        row(
            "the opposite arrow comes back",
            false,
            &[Click("agree", 0), Press(Tab), Press(Space), Press(ArrowRight), Press(ArrowLeft)],
            &[("size", &["S"])],
            Some(("size", 0)),
        ),
        row(
            "Tab goes past the group's second button",
            false,
            &[Click("agree", 0), Press(Tab), Press(Space), Press(ArrowRight), Press(ArrowLeft), Press(Tab), Press(Tab)],
            &[],
            Some(("country", 0)),
        ),
        // Issue #2223: Tab went from `first` straight to `last`, and Space could not tick `agree`.
        Row {
            keyboard: true,
            ..row("Tab stops on a check box", true, &[Click("first", 0), Type("Ada"), Press(Tab)], &[("first", &["Ada"])], Some(("agree", 0)))
        },
        row(
            "Space ticks it and it keeps the focus",
            true,
            &[Click("first", 0), Type("Ada"), Press(Tab), Press(Space)],
            &[("agree", &["Yes"])],
            Some(("agree", 0)),
        ),
        row(
            "Space again clears it",
            true,
            &[Click("first", 0), Type("Ada"), Press(Tab), Press(Space), Press(Space)],
            &[("agree", &[])],
            Some(("agree", 0)),
        ),
        row(
            "Tab goes on to the next text field",
            true,
            &[Click("first", 0), Type("Ada"), Press(Tab), Press(Space), Press(Space), Press(Tab)],
            &[],
            Some(("last", 0)),
        ),
        row(
            "Shift+Tab comes back to the check box",
            true,
            &[Click("first", 0), Type("Ada"), Press(Tab), Press(Space), Press(Space), Press(Tab), ShiftTab],
            &[],
            Some(("agree", 0)),
        ),
        // Issue #2223: the click ticked the box but left no field focused, so Tab started over.
        row(
            "a click on a check box while typing keeps the text and focuses the box",
            true,
            &[Click("first", 0), Type("Ada"), Click("agree", 0)],
            &[("agree", &["Yes"]), ("first", &["Ada"])],
            Some(("agree", 0)),
        ),
        row("so Tab goes on from there", true, &[Click("first", 0), Type("Ada"), Click("agree", 0), Press(Tab)], &[], Some(("last", 0))),
    ];
    for r in rows {
        let mut h = if r.boxes { check_box_harness() } else { harness() };
        for act in &r.acts {
            match *act {
                Act::Click(name, widget) => click_field(&mut h, name, widget),
                Act::Type(text) => {
                    h.event(egui::Event::Text(text.into()));
                    h.run_steps(1);
                }
                Act::Press(key) => {
                    h.key_press(key);
                    h.run_steps(3);
                }
                Act::ShiftTab => {
                    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Tab);
                    h.run_steps(3);
                }
                Act::Pick(label) => {
                    h.get_by_label(label).click();
                    h.run_steps(4);
                }
            }
        }
        for (name, want) in &r.values {
            assert_eq!(value(&h, name), *want, "{}: {name}", r.case);
        }
        if let Some((name, widget)) = r.focus {
            assert_eq!(focused(&h), Some((name.to_string(), widget)), "{}", r.case);
        }
        if r.keyboard {
            assert!(h.ctx.egui_wants_keyboard_input(), "{}: the check box has the keyboard, not the page", r.case);
        }
    }

    // Clear form takes everything back; undo restores it.
    let mut h = harness();
    click_field(&mut h, "country", 0);
    h.get_by_label("France").click();
    h.run_steps(4);
    assert_eq!(value(&h, "country"), ["fr"]);
    click_field(&mut h, "size", 1);
    assert!(h.execute("form.clear"));
    h.run_steps(2);
    assert!(value(&h, "country").is_empty() && value(&h, "size").is_empty());
    undo(&mut h);
    h.run_steps(2);
    assert_eq!(value(&h, "country"), ["fr"]);
}

fn focused(h: &Harness<'static, PdfCraftApp>) -> Option<(String, usize)> {
    h.state().views[0].session.forms.focus.as_ref().map(|f| (f.name.clone(), f.widget))
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "agree.pdf", None, bytes).unwrap();
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "zoom", "150").unwrap();
            app
        }
    });
    h.run_steps(6);
    h
}

#[test]
fn tab_uses_the_first_remaining_widget_when_focus_leaves_the_order() {
    let mut h = check_box_harness();
    click_field(&mut h, "pick", 0);
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::SetFieldProps {
            name: "pick".into(),
            props: Box::new(pdfcraft_engine::FieldProps { rect: Some((0, [320.0, 230.0, 336.0, 246.0])), ..Default::default() }),
        },
    );
    h.run_steps(3);

    h.key_press(egui::Key::Tab);
    h.run_steps(3);
    assert_eq!(focused(&h), Some(("pick".into(), 1)), "Tab replaces the off-page focus with the group's remaining widget");
}

#[test]
fn shift_tab_uses_the_previous_remaining_widget_when_focus_leaves_the_order() {
    let mut h = check_box_harness();
    click_field(&mut h, "last", 0);
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::SetFieldProps {
            name: "last".into(),
            props: Box::new(pdfcraft_engine::FieldProps { rect: Some((0, [320.0, 270.0, 340.0, 292.0])), ..Default::default() }),
        },
    );
    h.run_steps(3);

    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Tab);
    h.run_steps(3);
    assert_eq!(focused(&h), Some(("agree".into(), 0)), "Shift+Tab restores focus to the last stop before the removed widget");
}

#[test]
fn shift_tab_wraps_to_the_last_widget_when_focus_leaves_the_order() {
    let mut h = check_box_harness();
    click_field(&mut h, "first", 0);
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::SetFieldProps {
            name: "first".into(),
            props: Box::new(pdfcraft_engine::FieldProps { rect: Some((0, [320.0, 350.0, 340.0, 372.0])), ..Default::default() }),
        },
    );
    h.run_steps(3);

    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Tab);
    h.run_steps(3);
    assert_eq!(focused(&h), Some(("pick".into(), 1)), "Shift+Tab wraps to the last remaining stop");
}

#[test]
fn tab_and_shift_tab_follow_widget_order_across_pages() {
    let mut h = check_box_harness();
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::AddField {
            page: 1,
            rect: [20.0, 350.0, 180.0, 372.0],
            kind: pdfcraft_engine::NewField::Text { multiline: false },
            name: Some("second_page".into()),
        },
    );
    h.run_steps(4);
    let order = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        pdfcraft_ui_egui::forms_ui::tab_order(&doc.form, &doc.info)
            .into_iter()
            .filter_map(|(fi, wi)| doc.form.get(fi).map(|field| (field.name.clone(), wi)))
            .collect::<Vec<_>>()
    };
    assert_eq!(order.first().map(|entry| entry.0.as_str()), Some("first"));
    assert_eq!(order.last().map(|entry| entry.0.as_str()), Some("second_page"));
    click_field(&mut h, "first", 0);
    for expected in order.iter().skip(1) {
        h.key_press(egui::Key::Tab);
        h.run_steps(4);
        assert_eq!(focused(&h), Some(expected.clone()), "forward traversal: {order:?}");
    }
    for expected in order.iter().rev().skip(1) {
        h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Tab);
        h.run_steps(4);
        assert_eq!(focused(&h), Some(expected.clone()), "reverse traversal: {order:?}");
    }
}

#[test]
fn page_keys_still_work_with_a_check_box_or_radio_button_focused() {
    // The widget that takes Space for a focused box must not swallow the page keys.
    let mut h = check_box_harness();
    click_field(&mut h, "agree", 0);
    assert_eq!(focused(&h), Some(("agree".into(), 0)));
    h.key_press(egui::Key::End);
    h.run_steps(4);
    assert_eq!(h.state().views[0].current(), 2, "End goes to the last page");
    assert!(h.state().views[0].page_screen_rect(0).is_none(), "where the check box is out of view");
    // Scrolled away, the box lets go: back on its page, Space no longer toggles it.
    h.key_press(egui::Key::Home);
    h.run_steps(4);
    assert_eq!(h.state().views[0].current(), 0);
    assert_eq!(focused(&h), None, "the box let go of the focus");
    h.key_press(egui::Key::Space);
    h.run_steps(3);
    assert_eq!(value(&h, "agree"), ["Yes"]);
    // A radio button keeps the arrows for its group, so → selects B instead of turning the page.
    click_field(&mut h, "pick", 0);
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(3);
    assert_eq!(value(&h, "pick"), ["B"]);
    assert_eq!(h.state().views[0].current(), 0, "the arrow stayed in the group");
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
    assert_eq!(h.state().views[0].session.forms.focus.as_ref().map(|f| f.name.as_str()), Some("city"));
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
    assert_eq!(h.state().views[0].session.forms.focus.as_ref().map(|f| f.name.as_str()), Some("city"));
    let rect = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == "name").unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen")
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
    assert_eq!(h.state().views[0].session.forms.focus.as_ref().map(|f| f.name.as_str()), Some("name"), "the press moved the focus");
    let n = "Ada Lovelace".chars().count();
    let sel = h.state().views[0].session.forms.focus.as_ref().expect("focused field").selection;
    // "Ad|a": not the caret at the end that a newly focused editor gets on its own.
    assert_eq!(sel.anchor, 2, "anchored where the drag started: {sel:?}");
    assert_eq!(sel.caret, n, "and extended to where it ended: {sel:?}");
}

#[test]
fn date_fields_offer_a_calendar() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness();
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::AddField { page: 0, rect: [50.0, 40.0, 200.0, 60.0], kind: pdfcraft_engine::NewField::Date, name: Some("due".into()) },
    );
    h.run_steps(4);
    click_field(&mut h, "due", 0);
    let (y, m, _) = h.state().session.today();
    let month =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"][(m - 1) as usize];
    h.get_by_label(&format!("{month} {y}"));
    h.get_by_label("Next").click();
    h.run_steps(2);
    h.get_by_label_contains(" 15, ").click();
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
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::AddField { page: 0, rect: [50.0, 40.0, 200.0, 60.0], kind: pdfcraft_engine::NewField::Date, name: Some("due".into()) },
    );
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
    h.get_by_label("Next").click();
    h.run_steps(3);
    h.get_by_label("April 2025");
}

/// A date format with its fields side by side ("yyyymmdd") refused every typed date, and its
/// calendar opened on today's month.
#[test]
fn an_undelimited_date_field_takes_a_typed_date_and_its_calendar_opens_on_it() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness();
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::AddField { page: 0, rect: [50.0, 40.0, 200.0, 60.0], kind: pdfcraft_engine::NewField::Date, name: Some("due".into()) },
    );
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::SetFieldProps {
            name: "due".into(),
            props: Box::new(pdfcraft_engine::FieldProps {
                format: Some(pdfcraft_engine::form_scripts::Format::Date("yyyymmdd".into())),
                ..Default::default()
            }),
        }
    ));
    h.run_steps(4);
    click_field(&mut h, "due", 0);
    h.event(egui::Event::Text("20250314".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(4);
    assert_eq!(card_count(&h), 0, "the typed date was not refused");
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
        let ctx_h = h.ctx.clone();
        h.state_mut()
            .apply_edit(&ctx_h, pdfcraft_engine::Edit::AddField { page: 0, rect, kind: pdfcraft_engine::NewField::Date, name: Some(name.into()) });
    }
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::SetFieldProps {
            name: "start".into(),
            props: Box::new(pdfcraft_engine::FieldProps {
                format: Some(pdfcraft_engine::form_scripts::Format::Date("yyyy-mm-dd".into())),
                ..Default::default()
            }),
        }
    ));
    h.run_steps(4);
    // Two months on, and two months back: Today still picks the 14th of November.
    for (name, arrow, shown, want) in [("due", "Next", "January 2024", "11/14/2023"), ("start", "Previous", "September 2023", "2023-11-14")] {
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
        assert!(h.state().views[0].session.forms.focus.is_none(), "picking closes the field, as a clicked day does");
        assert!(h.query_by_label("Today").is_none(), "and its calendar");
    }
    // One edit each, as a clicked day is.
    undo(&mut h);
    h.run_steps(2);
    assert_eq!(value(&h, "start"), Vec::<String>::new());
    assert_eq!(value(&h, "due"), ["11/14/2023"]);
}

#[test]
fn lithuanian_calendar_localizes_months_and_weekdays_without_changing_field_values() {
    if crate::support::skip_without_catalogs("lithuanian_calendar_localizes_months_and_weekdays_without_changing_field_values") {
        return;
    }
    use egui_kittest::kittest::Queryable;
    let mut h = harness();
    h.set_option("language", "lt").unwrap();
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::AddField { page: 0, rect: [50.0, 40.0, 200.0, 60.0], kind: pdfcraft_engine::NewField::Date, name: Some("due".into()) },
    );
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

/// The value of `name` in the PDF saved at `path` in `files`, read back by opening it in a fresh app.
fn saved_value(files: &ScriptedFiles, path: &std::path::Path, name: &str) -> Vec<String> {
    let mut app = crate::control::new_app();
    app.set_option(&egui::Context::default(), "language", "en").unwrap();
    let bytes = files.file(path).unwrap_or_else(|| panic!("nothing was saved at {}", path.display()));
    app.open_bytes(&egui::Context::default(), "saved.pdf", None, bytes).unwrap();
    app.session.get(app.views[0].id).unwrap().form.iter().find(|f| f.name == name).unwrap().value.clone()
}

/// Step frames until the save has written `path` in `files`, instead of assuming it is there after
/// a fixed number of frames. The write is whole, so the file appears only when it is complete.
fn wait_for_file(h: &mut Harness<'static, PdfCraftApp>, files: &ScriptedFiles, path: &std::path::Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while files.file(path).is_none() {
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
    let (mut h, files) = scripted_harness("form.pdf", include_bytes!("../data/form.pdf").to_vec(), "150", &out);
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("Browser Form 123".into()));
    h.run_steps(2);
    assert!(h.state().views[0].session.forms.focus.is_some(), "still typing");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::S);
    h.run_steps(3);
    assert_eq!(saved_value(&files, &out, "name"), ["Browser Form 123"], "the saved PDF has the typed text");
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
        let (mut h, files) = scripted_harness("tab.pdf", bytes, "150", &out);
        click_field(&mut h, "qty", 0);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.event(egui::Event::Text("42".into()));
        h.run_steps(2);
        let modifiers = if backwards { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE };
        if same_frame {
            one_frame(&mut h, vec![key(egui::Key::Tab, modifiers), key(egui::Key::S, egui::Modifiers::COMMAND)]);
        } else {
            h.key_press_modifiers(modifiers, egui::Key::Tab);
            h.run_steps(3);
            h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::S);
        }
        wait_for_file(&mut h, &files, &out);
        assert_eq!(saved_value(&files, &out, "qty"), ["42"], "{case}: saving cannot restore the old value");
        assert_eq!(value(&h, "qty"), ["42"], "{case}");
        // One typing action is one history entry, even after saving. Escape closes the
        // untouched editor before undoing the document rather than the TextEdit buffer.
        h.key_press(egui::Key::Escape);
        undo(&mut h);
        assert_eq!(value(&h, "qty"), ["4"], "{case}: a single undo restores the original");
        redo(&mut h);
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
    let ctx_h = h.ctx.clone();
    h.state_mut().request_close_tab(&ctx_h, 0);
    h.run_steps(2);
    assert!(asks_to_save(&h), "typed text is an unsaved change: the user is asked");
    assert_eq!(h.state().views.len(), 1);
}

#[test]
fn close_all_saves_text_typed_in_a_tab_that_is_no_longer_active() {
    let dir = scratch("close-all");
    let out = dir.join("first.pdf");
    let (mut h, files) = scripted_harness("form.pdf", include_bytes!("../data/form.pdf").to_vec(), "150", &out);
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("Typed in tab one".into()));
    h.run_steps(2);
    // Another document becomes active while the first one's field still has its draft.
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "other.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
    h.run_steps(2);
    let ctx_h = h.ctx.clone();
    h.state_mut().close_all(&ctx_h);
    h.run_steps(2);
    assert!(asks_to_save(&h), "the first document has an unsaved change");
    h.get_all_by_label(save_answer()).last().expect("the prompt's Save button").click();
    wait_for_file(&mut h, &files, &out);
    assert_eq!(saved_value(&files, &out, "name"), ["Typed in tab one"]);
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
fn type_then_save_in_one_frame(h: &mut Harness<'static, PdfCraftApp>, name: &str, text: &str, last: egui::Event) {
    click_field(h, name, 0);
    h.event(egui::Event::Text(text.into()));
    h.run_steps(2);
    one_frame(h, vec![last, key(egui::Key::S, egui::Modifiers::COMMAND)]);
    h.run_steps(3);
}

#[test]
fn typing_and_saving_in_the_same_frame_saves_the_last_keystrokes() {
    let dir = scratch("same-frame");
    let out = dir.join("saved.pdf");
    let (mut h, files) = scripted_harness("form.pdf", include_bytes!("../data/form.pdf").to_vec(), "150", &out);
    type_then_save_in_one_frame(&mut h, "name", "Browser", egui::Event::Text(" Form 123".into()));
    assert_eq!(saved_value(&files, &out, "name"), ["Browser Form 123"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn escape_and_save_in_the_same_frame_discards_the_draft() {
    let dir = scratch("escape");
    let out = dir.join("saved.pdf");
    let (mut h, _files) = scripted_harness("form.pdf", include_bytes!("../data/form.pdf").to_vec(), "150", &out);
    let before = value(&h, "name");
    type_then_save_in_one_frame(&mut h, "name", "Discard me", key(egui::Key::Escape, egui::Modifiers::NONE));
    assert_eq!(value(&h, "name"), before, "Escape cancels typing, even with ⌘S right after it");
    assert!(h.state().views[0].session.forms.focus.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unrelated_command_leaves_the_field_being_typed_in_alone() {
    let mut h = harness();
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("Still typing".into()));
    h.run_steps(2);
    assert!(h.execute("view.theme.dark"));
    h.run_steps(2);
    let focus = h.state().views[0].session.forms.focus.clone().expect("the editor stays open");
    assert_eq!(focus.text, "Still typing");
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty, "nothing was committed");
}

#[test]
fn exporting_form_data_while_typing_includes_the_text() {
    let dir = scratch("export");
    let out = dir.join("data.xfdf");
    let (mut h, files) = scripted_harness("form.pdf", include_bytes!("../data/form.pdf").to_vec(), "150", &out);
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("Exported value".into()));
    h.run_steps(2);
    let ctx_h = h.ctx.clone();
    h.state_mut().export_data_dialog(&ctx_h, false, true);
    wait_for_file(&mut h, &files, &out);
    assert!(String::from_utf8_lossy(&files.file(&out).unwrap_or_default()).contains("Exported value"));
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "order.pdf", None, validated_form_with_note()).unwrap();
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "zoom", "150").unwrap();
            app
        }
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
    assert_eq!(card_count(&h), 1, "the press committed 4500 and the script refused it");
    h.drop_at(p);
    h.run_steps(3);
    assert_eq!(card_count(&h), 1, "the release did not validate it again");
    let focus = h.state().views[0].session.forms.focus.clone().expect("the refused field stays open");
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "sheet.pdf", None, fields_over_text()).unwrap();
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "zoom", "150").unwrap();
            app
        }
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
    let (mut h, files) = scripted_harness("order.pdf", validated_form(), "150", &out);
    click_field(&mut h, "qty", 0);
    h.event(egui::Event::Text("500".into()));
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::S);
    h.run_steps(4);
    assert!(files.file(&out).is_none(), "nothing is saved without the typed value");
    assert_eq!(value(&h, "qty"), ["4"], "the field keeps its valid value");
    let focus = h.state().views[0].session.forms.focus.clone().expect("the editor stays open");
    assert_eq!(focus.text, "4500", "with the text (typed after the 4), so the user can fix it");
    // And closing asks rather than dropping it.
    let ctx_h = h.ctx.clone();
    h.state_mut().request_close_tab(&ctx_h, 0);
    h.run_steps(2);
    assert!(asks_to_save(&h));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_save_shortcut_pressed_while_typing_saves_only_the_document_it_was_pressed_in() {
    let dir = scratch("deferred-doc");
    let out = dir.join("saved.pdf");
    let (mut h, files) = scripted_harness("form.pdf", include_bytes!("../data/form.pdf").to_vec(), "150", &out);
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("Tab one".into()));
    h.run_steps(2);
    one_frame(&mut h, vec![key(egui::Key::S, egui::Modifiers::COMMAND)]);
    assert!(files.file(&out).is_none(), "⌘S while typing waits a frame for the field");
    // Before the deferred ⌘S runs, another document becomes active.
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "other.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
    h.run_steps(3);
    assert!(files.file(&out).is_none(), "the other document is not saved by a shortcut pressed in the first");
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
    let (mut h, files) = scripted_harness("order.pdf", validated_form(), "150", &out);
    click_field(&mut h, "qty", 0);
    h.event(egui::Event::Text("500".into()));
    h.run_steps(2);
    one_frame(&mut h, vec![key(commit_key, egui::Modifiers::NONE), key(egui::Key::S, egui::Modifiers::COMMAND)]);
    h.run_steps(3);
    assert_eq!(value(&h, "qty"), ["4"]);
    assert!(files.file(&out).is_none(), "nothing is saved right after the field refused its value");
    // The refused text is back in the field, so a later ⌘S stops again instead of saving 4.
    assert_eq!(h.state().views[0].session.forms.focus.as_ref().map(|f| f.text.as_str()), Some("4500"));
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::S);
    h.run_steps(4);
    assert!(files.file(&out).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn flattening_while_typing_flattens_the_typed_text() {
    let mut h = harness();
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("Flat Ada".into()));
    h.run_steps(2);
    assert!(h.execute("form.flatten"));
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
        let (mut h, files) = scripted_harness("form.pdf", include_bytes!("../data/form.pdf").to_vec(), "150", &path);
        let ctx_h = h.ctx.clone();
        assert!(h.state_mut().apply_edit(
            &ctx_h,
            pdfcraft_engine::Edit::SetFieldValue { name: "name".into(), value: pdfcraft_engine::FieldValue::Text("Saved original".into()) }
        ));
        let ctx_h = h.ctx.clone();
        // A document with no file asks where to save: the scripted answer arrives on a later pass.
        h.state_mut().save_active(&ctx_h, pdfcraft_ui_egui::SaveTarget::InPlace);
        crate::support::wait_until(&mut h, "the save", |s| s.views.first().and_then(|v| s.session.get(v.id)).is_some_and(|d| !d.dirty));
        let saved = files.file(&path).expect("the save wrote the file");
        let ctx_h = h.ctx.clone();
        assert!(h.state_mut().apply_edit(
            &ctx_h,
            pdfcraft_engine::Edit::SetFieldProps {
                name: "name".into(),
                props: Box::new(pdfcraft_engine::FieldProps {
                    validate: Some(pdfcraft_engine::form_scripts::Validate::Range { min: Some(0.0), max: Some(10.0) }),
                    ..Default::default()
                }),
            }
        ));
        h.run_steps(3);
        click_field(&mut h, "name", 0);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.event(egui::Event::Text("99".into()));
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::S);
        h.run_steps(4);
        assert_eq!(h.state().views[0].session.forms.focus.as_ref().unwrap().text, "99");
        assert_eq!(files.file(&path), Some(saved.clone()), "rejection leaves the saved file untouched");
        assert!(h.execute("file.revert"));
        h.run_steps(2);
        h.get_all_by_label("Revert").last().expect("confirmation button").click();
        h.run_steps(4);
        assert!(h.state().views[0].session.forms.focus.is_none() && h.state().views[0].session.pending_edit.is_none());
        assert_eq!(h.state().first_dirty(), None, "Revert discards rejected typing too");
        assert_eq!(value(&h, "name"), ["Saved original"]);
        let ctx_h = h.ctx.clone();
        assert!(h.state_mut().save_active(&ctx_h, pdfcraft_ui_egui::SaveTarget::InPlace));
        assert_eq!(saved_value(&files, &path, "name"), ["Saved original"]);
        let bytes = files.file(&path).expect("the save wrote the file");
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
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::InsertBlankPage { at: 1, width: 300.0, height: 400.0 }));
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::AddField {
            page: 1,
            rect: [20.0, 20.0, 100.0, 40.0],
            kind: pdfcraft_engine::NewField::Text { multiline: false },
            name: Some("page2_field".into()),
        }
    ));
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::SetFieldScript {
            name: "page2_field".into(),
            event: "calculate".into(),
            script: Some("event.value = 'from_' + getField('name').value;".into()),
        }
    ));
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
    assert!(!h.state().views[0].session.forms.unbaked.contains(&("name".to_string(), 0)));

    // 2. Rejected edit: typing rejected by validate script leaves unbaked clean.
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::SetFieldScript { name: "city".into(), event: "validate".into(), script: Some("event.rc = false;".into()) }
    ));
    click_field(&mut h, "city", 0);
    h.event(egui::Event::Text("BadVal".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
    assert!(!h.state().views[0].session.forms.unbaked.contains(&("city".to_string(), 0)));
    h.key_press(egui::Key::Escape);
    h.run_steps(2);

    // 3. Zoom then edit: zoom to another scale, let tiles settle, then edit.
    h.state_mut().views[0].view.zoom_step(true);
    crate::support::settle(&mut h);
    // The larger page can put the field above the viewport; scroll back to the page top.
    h.state_mut().views[0].view.go_to(0);
    crate::support::settle(&mut h);
    assert!(h.state().views[0].viewport_rect().contains(field_rect(&h, "name").center()));
    click_field(&mut h, "name", 0);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("ZoomedVal".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
    assert_eq!(value(&h, "name"), ["ZoomedVal"]);
    crate::support::settle(&mut h);
    assert!(!h.state().views[0].session.forms.unbaked.contains(&("name".to_string(), 0)));
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "orphan.pdf", None, bytes).unwrap();
            app.set_option(&egui::Context::default(), "panel", "fields").unwrap();
            app
        }
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "lists.pdf", None, bytes).unwrap();
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "zoom", "100").unwrap();
            app
        }
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

/// The selected option positions of `name` (`/I`, or `/V` without a usable one).
fn selected(h: &Harness<'static, PdfCraftApp>, name: &str) -> Vec<usize> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == name).unwrap().selected.clone()
}

/// ISO 32000-2 Table 234: `/I` tells apart options that share an export value. A list box whose
/// One-A and One-B both export `1` shows the one `/I` names, and a click picks the row clicked:
/// `/V (1)` with `/I` naming it, so the pick survives a save and reopen.
#[test]
fn a_list_box_click_picks_one_of_two_options_with_the_same_export_value() {
    let bytes = pdf_from_objects(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /NeedAppearances true /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 400 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (dup) /Opt [[(1) (One-A)] [(1) (One-B)] [(2) (Two)]] /V (1) /I [1] /Rect [50 250 200 320] /P 3 0 R /F 4 >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
    ]);
    let dir = scratch("duplicate-exports");
    let out = dir.join("saved.pdf");
    let (mut h, files) = scripted_harness("dup.pdf", bytes, "100", &out);
    assert_eq!(selected(&h, "dup"), [1], "One-B is selected, not One-A too");

    let r = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == "dup").unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen")
    };
    let zoom = r.height() / 70.0;
    // Row 0 (One-A), then row 1 (One-B): each click changes the selection though /V stays (1).
    click_at(&mut h, egui::pos2(r.center().x, r.min.y + 8.0 * zoom));
    assert_eq!(value(&h, "dup"), ["1"]);
    assert_eq!(selected(&h, "dup"), [0], "clicking One-A picks One-A");
    click_at(&mut h, egui::pos2(r.center().x, r.min.y + 19.0 * zoom));
    assert_eq!(selected(&h, "dup"), [1], "clicking One-B picks One-B again");
    click_at(&mut h, egui::pos2(r.center().x, r.min.y + 8.0 * zoom));
    assert_eq!(selected(&h, "dup"), [0]);

    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::S);
    wait_for_file(&mut h, &files, &out);
    let mut app = crate::control::new_app();
    app.open_bytes(&egui::Context::default(), "saved.pdf", None, files.file(&out).unwrap()).unwrap();
    let reopened = app.session.get(app.views[0].id).unwrap().form.iter().find(|f| f.name == "dup").unwrap().clone();
    assert_eq!(reopened.value, ["1"], "/V is the one export value");
    assert_eq!(reopened.selected, [0], "/I keeps One-A through the save");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Same-name list boxes are one field (ISO 32000-2 §12.7.3.1), and a twin can list the options
/// sharing an export value in another order: clicking One-B in the second gives both One-B, not
/// the option at the same position in the first.
#[test]
fn a_click_in_a_twin_list_box_with_options_in_another_order_picks_the_row_clicked() {
    let bytes = pdf_from_objects(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 6 0 R] /NeedAppearances true /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 400 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 6 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (dup) /Opt [[(1) (One-A)] [(1) (One-B)] [(2) (Two)]] /Rect [50 250 200 320] /P 3 0 R /F 4 >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (dup) /Opt [[(1) (One-B)] [(1) (One-A)] [(2) (Two)]] /Rect [50 100 200 170] /P 3 0 R /F 4 >>",
    ]);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "twins.pdf", None, bytes).unwrap();
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "zoom", "100").unwrap();
            app
        }
    });
    h.run_steps(4);
    let twins = |h: &Harness<'static, PdfCraftApp>| -> Vec<Vec<usize>> {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().form.iter().filter(|f| f.name == "dup").map(|f| f.selected.clone()).collect()
    };
    let r = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().filter(|f| f.name == "dup").nth(1).unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen")
    };
    let zoom = r.height() / 70.0;
    // Row 0 of the second list box: One-B.
    click_at(&mut h, egui::pos2(r.center().x, r.min.y + 8.0 * zoom));
    assert_eq!(value(&h, "dup"), ["1"]);
    assert_eq!(twins(&h), [vec![1], vec![0]], "One-B in both: second in the first, first in the twin");
    // The arrows step through the focused twin's own rows: ↓ from One-B there is One-A, which is
    // first in the first field, not Two (the row after One-B in the first field).
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(3);
    assert_eq!(twins(&h), [vec![0], vec![1]], "One-A in both after ↓ in the twin");
    h.key_press(egui::Key::ArrowUp);
    h.run_steps(3);
    assert_eq!(twins(&h), [vec![1], vec![0]], "back to One-B after ↑ in the twin");
}

/// ISO 32000-2 Table 233: a list box without MultiSelect selects "at most one item". Given
/// `/V [(TW) (JP)]` it shows Taiwan alone, and a click writes the one option clicked.
#[test]
fn a_single_select_list_box_given_two_values_shows_and_takes_one() {
    let bytes = pdf_from_objects(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /NeedAppearances true /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 400 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (pair) /Opt [[(TW) (Taiwan)] [(JP) (Japan)] [(US) (United States)]] /V [(TW) (JP)] /Rect [50 250 200 320] /P 3 0 R /F 4 >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
    ]);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "pair.pdf", None, bytes).unwrap();
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "zoom", "100").unwrap();
            app
        }
    });
    h.run_steps(4);
    assert_eq!((value(&h, "pair"), selected(&h, "pair")), (vec!["TW".to_string()], vec![0]), "Taiwan alone");

    let r = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == "pair").unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen")
    };
    let zoom = r.height() / 70.0;
    // Row 1 (Japan), which /V also named: clicking it makes it the one value.
    click_at(&mut h, egui::pos2(r.center().x, r.min.y + 19.0 * zoom));
    assert_eq!((value(&h, "pair"), selected(&h, "pair")), (vec!["JP".to_string()], vec![1]));
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "auto.pdf", None, auto_size_form()).unwrap();
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "zoom", "150").unwrap();
            app
        }
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
    let typed = h.state().views[0].session.forms.focus.clone().expect("the editor is open").text;
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "tall.pdf", None, tall_auto_size_form()).unwrap();
            app
        }
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

/// The one-page PDF `guide_fixture` attaches as `Guide.pdf`.
const GUIDE_INNER: &str = "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
                 3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF";

/// A page with a push button `go`, and three attached files: a one-page PDF filed under the key
/// `Guide.pdf` (its own file name `inner.pdf`), `notes.txt`, and `fake.pdf`, which holds text.
fn guide_fixture() -> Vec<u8> {
    let inner = GUIDE_INNER;
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> /Names << /EmbeddedFiles << /Names [(Guide.pdf) 5 0 R (fake.pdf) 9 0 R (notes.txt) 7 0 R] >> >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [4 0 R] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (go) /Rect [20 20 120 50] /P 3 0 R >>".into(),
        "<< /Type /Filespec /F (inner.pdf) /UF (inner.pdf) /EF << /F 6 0 R >> >>".into(),
        format!("<< /Type /EmbeddedFile /Length {} >>\nstream\n{inner}\nendstream", inner.len()),
        "<< /Type /Filespec /F (notes.txt) /UF (notes.txt) /EF << /F 8 0 R >> >>".into(),
        "<< /Type /EmbeddedFile /Length 5 >>\nstream\nhello\nendstream".into(),
        "<< /Type /Filespec /F (fake.pdf) /UF (fake.pdf) /EF << /F 10 0 R >> >>".into(),
        "<< /Type /EmbeddedFile /Length 12 >>\nstream\nnot a PDF at\nendstream".into(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// An attachment opens in a tab only when its bytes are a PDF, whatever its name says: a
/// `fake.pdf` holding text is offered to save instead. A request goes back to the tab opened from
/// that attachment, and not to another open PDF of the same name whose bytes start with it.
#[test]
fn a_form_button_opens_only_a_real_pdf_and_returns_to_the_tab_it_opened() {
    let saved = std::env::temp_dir().join(format!("pdfcraft-fake-{}.pdf", std::process::id()));
    let files = Arc::new(ScriptedFiles::answering([Picked::Paths(vec![saved.clone()])]));
    let host_files = files.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::app_with_files(host_files);
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "form.pdf", None, guide_fixture()).unwrap();
            // Another document named like the attachment, its bytes the attachment's and an update.
            app.open_bytes(&egui::Context::default(), "inner.pdf", None, format!("{GUIDE_INNER}\n% updated later\n").into_bytes()).unwrap();
            app.active = Some(0);
            app
        }
    });
    h.run_steps(4);
    let tabs = |h: &Harness<'static, PdfCraftApp>| h.state().views.len();
    let id = h.state().views[0].id;
    // A text file named .pdf is offered to save, not opened.
    let _ = std::fs::remove_file(&saved);
    run_button_script(&mut h, id, "go", "this.exportDataObject({cName: 'fake.pdf', nLaunch: 2});");
    wait_for_file(&mut h, &files, &saved);
    assert_eq!(tabs(&h), 2, "no tab for a file that isn't a PDF: {:?}", notice(&h));
    assert_eq!(files.file(&saved).as_deref(), Some(&b"not a PDF at"[..]), "offered to save instead");
    let _ = std::fs::remove_file(&saved);
    // The real attached PDF opens in a tab of its own, though another inner.pdf starts with it.
    h.state_mut().active = Some(0);
    run_button_script(&mut h, id, "go", "this.exportDataObject({cName: 'Guide.pdf', nLaunch: 2});");
    crate::support::wait_until(&mut h, "the attachment to open", |app| app.active == Some(2));
    assert_eq!(tabs(&h), 3, "a tab of its own");
    assert_eq!(h.state().active, Some(2));
    // Asked again, it goes back to the tab it opened.
    h.state_mut().active = Some(0);
    run_button_script(&mut h, id, "go", "this.exportDataObject({cName: 'Guide.pdf', nLaunch: 2});");
    crate::support::wait_until(&mut h, "the attachment tab to activate", |app| app.active == Some(2));
    assert_eq!((tabs(&h), h.state().active), (3, Some(2)), "the tab it opened, not the other inner.pdf");
}

/// A button script's `exportDataObject` opens the attached PDF in a new tab, goes back to that
/// tab when asked again, and names a missing file in a toast, a long name shortened.
#[test]
fn a_form_button_opens_its_attached_instructions_in_a_new_tab() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "form.pdf", None, guide_fixture()).unwrap();
            app
        }
    });
    h.run_steps(4);
    let tabs = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.views.iter().filter_map(|v| s.session.get(v.id)).map(|d| d.name.clone()).collect::<Vec<_>>()
    };
    let id = h.state().views[0].id;
    // By its key in the document's attached files, as forms name it.
    run_button_script(&mut h, id, "go", "this.exportDataObject({cName: 'Guide.pdf', nLaunch: 2});");
    crate::support::wait_until(&mut h, "the attachment to open", |app| app.active == Some(1));
    assert_eq!(tabs(&h), ["form.pdf", "inner.pdf"], "the attached PDF opens in its own tab");
    // Asked again from the form, it goes back to that tab rather than opening another.
    h.state_mut().active = Some(0);
    run_button_script(&mut h, id, "go", "this.exportDataObject({cName: 'Guide.pdf', nLaunch: 2});");
    crate::support::wait_until(&mut h, "the attachment tab to activate", |app| app.active == Some(1));
    assert_eq!(tabs(&h), ["form.pdf", "inner.pdf"], "no second copy");
    assert_eq!(h.state().active, Some(1), "its tab is shown");
    // A file the document doesn't have is said so.
    h.state_mut().active = Some(0);
    run_button_script(&mut h, id, "go", "this.exportDataObject({cName: 'nope.pdf', nLaunch: 2});");
    assert!(
        crate::support::step_until(&mut h, |h| notice(h).is_some_and(|t| t.contains("nope.pdf"))),
        "the missing attachment notice: {:?}",
        notice(&h)
    );
    assert_eq!(tabs(&h).len(), 2);
    // A name of the form's own is shortened in the message, keeping its end. (Past a line's
    // length, the script engine cuts it first, as it cuts every name a script passes.)
    run_button_script(&mut h, id, "go", "this.exportDataObject({cName: new Array(3001).join('x') + '.pdf', nLaunch: 2});");
    assert!(
        crate::support::step_until(&mut h, |h| notice(h).is_some_and(|t| t.contains("x.pdf"))),
        "the shortened attachment notice: {:?}",
        notice(&h)
    );
    let toast = notice(&h).unwrap_or_default();
    assert!(toast.contains('…') && toast.contains("x.pdf"), "{toast}");
    assert!(toast.chars().count() < 200, "{} characters", toast.chars().count());
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "combo.pdf", None, combo_form()).unwrap();
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "zoom", "150").unwrap();
            app
        }
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
    let (mut h, files) = scripted_harness("combo.pdf", combo_form(), "150", &out);
    click_field(&mut h, "fruit", 0);
    assert_eq!(focused(&h), Some(("fruit".into(), 0)));
    h.event(egui::Event::Text("Cherry".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert_eq!(value(&h, "fruit"), ["Cherry"], "Enter commits the typed value");
    assert!(h.state().views[0].session.forms.focus.is_none(), "and closes the field");
    // Reopened, it shows the typed value, and the list still picks an option.
    click_field(&mut h, "fruit", 0);
    assert_eq!(h.state().views[0].session.forms.focus.as_ref().map(|f| f.text.as_str()), Some("Cherry"));
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
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::S);
    wait_for_file(&mut h, &files, &out);
    assert_eq!(saved_value(&files, &out, "fruit"), ["Damson"]);
    // Undo takes the typing back as one step.
    undo(&mut h);
    h.run_steps(2);
    assert_eq!(value(&h, "fruit"), ["Banana"]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// #4296: an editable combo box turned a quarter (`/MK /R` 90, ISO 32000-2 Table 192) is typed
/// into as a turned text field is: a click near the field's top puts the caret at the end of the
/// text, where the text running up the field ends, not at its start.
#[test]
fn a_turned_editable_combo_box_places_the_caret_along_its_turned_text() {
    let pdf = pdf_from_objects(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (fruit) /Ff 393216 /Opt [(Apple) (Banana)] /Rect [240 20 260 170] /MK << /R 90 >> /P 3 0 R /F 4 >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
    ]);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "turned-combo.pdf", None, pdf.clone()).unwrap();
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "zoom", "150").unwrap();
            app
        }
    });
    crate::support::settle(&mut h);
    click_field(&mut h, "fruit", 0);
    assert_eq!(focused(&h), Some(("fruit".into(), 0)));
    h.event(egui::Event::Text("Paris".into()));
    h.run_steps(2);
    let field = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == "fruit").unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen")
    };
    // Near the top the caret goes to the end of the turned text, near the bottom to its start.
    click_at(&mut h, field.left_top() + egui::vec2(3.0, 2.0));
    h.event(egui::Event::Text("X".into()));
    h.run_steps(2);
    click_at(&mut h, field.left_bottom() + egui::vec2(3.0, -2.0));
    h.event(egui::Event::Text("Y".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert_eq!(value(&h, "fruit"), ["YParisX"]);
}

/// A combo box without the Edit flag is a drop-down list only: typing changes nothing.
#[test]
fn a_combo_box_without_edit_takes_no_typing() {
    let mut h = combo_harness();
    click_field(&mut h, "veg", 0);
    assert_eq!(focused(&h), Some(("veg".into(), 0)));
    h.event(egui::Event::Text("Cherry".into()));
    h.run_steps(2);
    assert_eq!(h.state().views[0].session.forms.focus.as_ref().map(|f| f.text.as_str()), Some(""), "no text box took the typing");
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
    h.set_option("panel", "none").unwrap();
    h.run_steps(2);
    let id = h.state().views[0].id;
    run_button_script(&mut h, id, "size", "app.execMenuItem('ShowHideFileAttachment');");
    crate::support::wait_until(&mut h, "attachments panel to open", |app| crate::ui::right(app) == Some(pdfcraft_ui_egui::RightPanel::Attachments));
    // Show/Hide, as its name says: asked again, it closes.
    run_button_script(&mut h, id, "size", "app.execMenuItem('ShowHideFileAttachment');");
    crate::support::wait_until(&mut h, "attachments panel to close", |app| crate::ui::right(app).is_none());
}

/// Which widgets of field `name` are on (their `/AS` is their on state).
fn on_widgets(h: &Harness<'static, PdfCraftApp>, name: &str) -> Vec<bool> {
    let s = h.state();
    let f = s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == name).unwrap().clone();
    (0..f.widgets.len()).map(|i| f.widget_on(i)).collect()
}

/// Radio groups whose buttons share an on state: `plain` (Radio | NoToggleToOff, on states Yes,
/// Yes, No), `unison` (the same with RadiosInUnison) and `free` (Radio alone, S and M).
fn shared_state_radios() -> Harness<'static, PdfCraftApp> {
    let kid = |n: u32, parent: u32, x: u32, state: &str| {
        format!(
            "<< /Type /Annot /Subtype /Widget /Parent {parent} 0 R /AS /Off /Rect [{x} {} {} {}] /P 3 0 R /F 4 /AP << /N << /{state} 5 0 R /Off 5 0 R >> >> >>",
            350 - n * 40,
            x + 16,
            366 - n * 40
        )
    };
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R 10 0 R 14 0 R] /DA (/Helv 0 Tf 0 g) >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [7 0 R 8 0 R 9 0 R 11 0 R 12 0 R 13 0 R 15 0 R 16 0 R] >>".into(),
        "<< >>".into(),
        "<< /Type /XObject /Subtype /Form /BBox [0 0 16 16] /Length 0 >>\nstream\n\nendstream".into(),
        "<< /FT /Btn /T (plain) /Ff 49152 /V /Off /Kids [7 0 R 8 0 R 9 0 R] >>".into(),
        kid(0, 6, 20, "Yes"),
        kid(0, 6, 60, "Yes"),
        kid(0, 6, 100, "No"),
        "<< /FT /Btn /T (unison) /Ff 33603584 /V /Off /Kids [11 0 R 12 0 R 13 0 R] >>".into(),
        kid(1, 10, 20, "Yes"),
        kid(1, 10, 60, "Yes"),
        kid(1, 10, 100, "No"),
        "<< /FT /Btn /T (free) /Ff 32768 /V /Off /Kids [15 0 R 16 0 R] >>".into(),
        kid(2, 14, 20, "S"),
        kid(2, 14, 60, "M"),
    ];
    let objs: Vec<&str> = objs.iter().map(String::as_str).collect();
    let bytes = pdf_from_objects(&objs);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "radios.pdf", None, bytes).unwrap();
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "zoom", "150").unwrap();
            app
        }
    });
    h.run_steps(6);
    h
}

/// ISO 32000-2 Table 229: without RadiosInUnison, radio buttons sharing an on state are mutually
/// exclusive, so a click turns on only the button clicked; with it they turn on together.
/// NoToggleToOff keeps a clicked selected button on; without it the click deselects it.
#[test]
fn clicking_a_radio_button_that_shares_its_on_state() {
    let mut h = shared_state_radios();
    click_field(&mut h, "plain", 0);
    assert_eq!((on_widgets(&h, "plain"), value(&h, "plain")), (vec![true, false, false], vec!["Yes".to_string()]), "only the clicked Yes");
    click_field(&mut h, "plain", 1);
    assert_eq!(on_widgets(&h, "plain"), [false, true, false], "the other Yes takes over");
    click_field(&mut h, "plain", 1);
    assert_eq!(on_widgets(&h, "plain"), [false, true, false], "NoToggleToOff: clicking it again changes nothing");
    click_field(&mut h, "plain", 2);
    assert_eq!((on_widgets(&h, "plain"), value(&h, "plain")), (vec![false, false, true], vec!["No".to_string()]));
    click_field(&mut h, "unison", 0);
    assert_eq!(on_widgets(&h, "unison"), [true, true, false], "RadiosInUnison: both Yes buttons");
    click_field(&mut h, "free", 0);
    assert_eq!(value(&h, "free"), ["S"]);
    click_field(&mut h, "free", 0);
    assert!(value(&h, "free").is_empty(), "without NoToggleToOff a second click deselects it");
    // Undo takes the last click back; the exclusive pair stays as it was.
    undo(&mut h);
    h.run_steps(2);
    assert_eq!(value(&h, "free"), ["S"]);
    assert_eq!(on_widgets(&h, "plain"), [false, false, true]);
}

/// A radio button read-only on its own (its widget's `/Ff`, ISO 32000-2 Table 227) is still
/// focused by the arrows and Space, but neither tries to turn it on: the form would refuse the
/// edit and show a failure for a button the user can't change. A click says it is read-only.
#[test]
fn a_read_only_radio_button_is_focused_but_not_selected() {
    let kid = |x: u32, state: &str, extra: &str| {
        format!(
            "<< /Type /Annot /Subtype /Widget /Parent 6 0 R /AS /Off {extra}/Rect [{x} 350 {} 366] /P 3 0 R /F 4 /AP << /N << /{state} 5 0 R /Off 5 0 R >> >> >>",
            x + 16
        )
    };
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] /DA (/Helv 0 Tf 0 g) >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [7 0 R 8 0 R] >>".into(),
        "<< >>".into(),
        "<< /Type /XObject /Subtype /Form /BBox [0 0 16 16] /Length 0 >>\nstream\n\nendstream".into(),
        "<< /FT /Btn /T (size) /Ff 32768 /V /Off /Kids [7 0 R 8 0 R] >>".into(),
        kid(20, "S", ""),
        kid(60, "L", "/Ff 1 "),
    ];
    let objs: Vec<&str> = objs.iter().map(String::as_str).collect();
    let bytes = pdf_from_objects(&objs);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "radios.pdf", None, bytes).unwrap();
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "zoom", "150").unwrap();
            app
        }
    });
    h.run_steps(6);
    click_field(&mut h, "size", 0);
    assert_eq!(value(&h, "size"), ["S"]);
    let toast = |h: &Harness<'static, PdfCraftApp>| notice(h);
    assert_eq!(toast(&h), None);
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(3);
    assert_eq!(focused(&h), Some(("size".into(), 1)), "the arrow still moves onto the read-only button");
    assert_eq!((value(&h, "size"), toast(&h)), (vec!["S".to_string()], None), "and leaves the selection alone");
    h.key_press(egui::Key::Space);
    h.run_steps(3);
    assert_eq!((value(&h, "size"), toast(&h)), (vec!["S".to_string()], None), "Space doesn't select it");
    // A click says why nothing happens, as for any read-only field, and is no failed edit.
    click_field(&mut h, "size", 1);
    assert_eq!((value(&h, "size"), toast(&h)), (vec!["S".to_string()], Some("size is read-only".to_string())), "nor does a click");
}

/// Check box fields of two kids each: `pair` with the on states A and B, `same` with Yes twice.
fn check_box_kids() -> Harness<'static, PdfCraftApp> {
    let kid = |parent: u32, y: u32, x: u32, state: &str| {
        format!(
            "<< /Type /Annot /Subtype /Widget /Parent {parent} 0 R /AS /Off /Rect [{x} {y} {} {}] /P 3 0 R /F 4 /AP << /N << /{state} 5 0 R /Off 5 0 R >> >> >>",
            x + 16,
            y + 16
        )
    };
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R 9 0 R] /DA (/Helv 0 Tf 0 g) >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [7 0 R 8 0 R 10 0 R 11 0 R] >>".into(),
        "<< >>".into(),
        "<< /Type /XObject /Subtype /Form /BBox [0 0 16 16] /Length 0 >>\nstream\n\nendstream".into(),
        "<< /FT /Btn /T (pair) /V /Off /Kids [7 0 R 8 0 R] >>".into(),
        kid(6, 350, 20, "A"),
        kid(6, 350, 60, "B"),
        "<< /FT /Btn /T (same) /V /Off /Kids [10 0 R 11 0 R] >>".into(),
        kid(9, 300, 20, "Yes"),
        kid(9, 300, 60, "Yes"),
    ];
    let objs: Vec<&str> = objs.iter().map(String::as_str).collect();
    let bytes = pdf_from_objects(&objs);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "kids.pdf", None, bytes).unwrap();
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "zoom", "150").unwrap();
            app
        }
    });
    h.run_steps(6);
    h
}

/// ISO 32000-2 §12.7.5.2.3: a check box field's value is the on state of the kid that is on, and
/// its kids can have different on states (Table 230). Clicking B after A ticks B and clears A;
/// clicking B again clears the field; kids sharing an on state tick together (EXAMPLE 2).
#[test]
fn clicking_check_box_kids_with_different_on_states() {
    let mut h = check_box_kids();
    click_field(&mut h, "pair", 0);
    assert_eq!((on_widgets(&h, "pair"), value(&h, "pair")), (vec![true, false], vec!["A".to_string()]));
    click_field(&mut h, "pair", 1);
    assert_eq!((on_widgets(&h, "pair"), value(&h, "pair")), (vec![false, true], vec!["B".to_string()]), "B ticks and A clears");
    click_field(&mut h, "pair", 1);
    assert_eq!((on_widgets(&h, "pair"), value(&h, "pair")), (vec![false, false], vec![]), "clicking B again clears the field");
    click_field(&mut h, "pair", 1);
    click_field(&mut h, "pair", 0);
    assert_eq!((on_widgets(&h, "pair"), value(&h, "pair")), (vec![true, false], vec!["A".to_string()]), "the tick moves back to A");
    click_field(&mut h, "same", 0);
    assert_eq!(on_widgets(&h, "same"), [true, true], "kids sharing Yes tick together");
    click_field(&mut h, "same", 1);
    assert_eq!(on_widgets(&h, "same"), [false, false], "and untick together");
    // Space on the focused kid toggles that kid too.
    click_field(&mut h, "pair", 1);
    assert_eq!(value(&h, "pair"), ["B"]);
    h.key_press(egui::Key::Space);
    h.run_steps(3);
    assert!(value(&h, "pair").is_empty(), "Space clears the ticked kid");
}

/// Click at `p` with `modifiers` held.
fn click_with(h: &mut Harness<'static, PdfCraftApp>, p: egui::Pos2, modifiers: egui::Modifiers) {
    h.event(egui::Event::ModifiersChanged(modifiers));
    h.hover_at(p);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers });
        h.run_steps(1);
    }
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.run_steps(3);
}

/// A list box opened at 100% zoom on its own page, and its rectangle on screen.
fn list_box(objects: &[&str], name: &str) -> (Harness<'static, PdfCraftApp>, egui::Rect) {
    let bytes = pdf_from_objects(objects);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "list.pdf", None, bytes).unwrap();
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "zoom", "100").unwrap();
            app
        }
    });
    h.run_steps(4);
    let r = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == name).unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen")
    };
    (h, r)
}

/// A Shift-click picks the rows from the one clicked last: after Ctrl-clicking row 3 and then
/// row 1, Shift on row 4 picks rows 1 to 4, not 3 to 4. The rows share one export value, so the
/// value can't say which came last, and `/I` lists them in ascending order.
#[test]
fn a_shift_click_in_a_list_box_extends_from_the_row_clicked_last() {
    let (mut h, r) = list_box(
        &[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /NeedAppearances true /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 400 400] >>",
            "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
            "<< /Type /Annot /Subtype /Widget /FT /Ch /T (days) /Ff 2097152 /Opt [[(x) (Mon)] [(x) (Tue)] [(x) (Wed)] [(x) (Thu)] [(x) (Fri)] [(x) (Sat)]] /Rect [50 220 200 320] /P 3 0 R /F 4 >>",
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        ],
        "days",
    );
    let zoom = r.height() / 100.0;
    let row = |k: f32| egui::pos2(r.center().x, r.min.y + (8.0 + 11.5 * k) * zoom);
    click_with(&mut h, row(3.0), egui::Modifiers::COMMAND);
    click_with(&mut h, row(1.0), egui::Modifiers::COMMAND);
    assert_eq!(selected(&h, "days"), [1, 3]);
    click_with(&mut h, row(4.0), egui::Modifiers::SHIFT);
    assert_eq!(selected(&h, "days"), [1, 2, 3, 4], "from row 1, the one clicked last");
}

/// A single-select list box holding an array `/V` is written as one value when its shown row is
/// clicked again, not left as an array because the row was already selected.
#[test]
fn clicking_the_shown_row_of_a_single_select_list_holding_an_array_writes_one_value() {
    let (mut h, r) = list_box(
        &[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /NeedAppearances true /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 400 400] >>",
            "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
            "<< /Type /Annot /Subtype /Widget /FT /Ch /T (pair) /Opt [[(TW) (Taiwan)] [(JP) (Japan)] [(US) (United States)]] /V [(TW) (JP)] /Rect [50 250 200 320] /P 3 0 R /F 4 >>",
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        ],
        "pair",
    );
    let field = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "pair").unwrap().clone()
    };
    assert!(field(&h).single_value_is_array, "loaded from an array");
    let zoom = r.height() / 70.0;
    click_at(&mut h, egui::pos2(r.center().x, r.min.y + 8.0 * zoom));
    let f = field(&h);
    assert_eq!((f.value, f.selected, f.single_value_is_array), (vec!["TW".to_string()], vec![0], false), "Taiwan, now one value");
}

/// Where the first widget of field `name` is on screen in the first view.
fn field_rect(h: &Harness<'static, PdfCraftApp>, name: &str) -> egui::Rect {
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    let f = doc.form.iter().find(|f| f.name == name).unwrap();
    pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen")
}

/// A drop-down's list opens on the field's left edge and no wider than the field, without
/// showing the field's name, and clicking an item chooses it.
#[test]
fn a_drop_downs_list_opens_as_wide_as_the_field_without_its_internal_name() {
    let mut h = harness();
    click_field(&mut h, "country", 0);
    let country = field_rect(&h, "country");
    assert!(egui_kittest::kittest::Queryable::query_by_label(&h, "country").is_none(), "the field's name isn't shown");
    let item = egui_kittest::kittest::Queryable::get_by_label(&h, "France").rect();
    assert!((item.left() - country.left()).abs() <= 2.0 && item.right() <= country.right() + 1.0, "{item:?} in {country:?}");
    egui_kittest::kittest::Queryable::get_by_label(&h, "France").click();
    h.run_steps(4);
    assert_eq!(value(&h, "country"), ["fr"]);
}

/// Where `text` is drawn in the last frame, if anywhere.
fn text_rect(h: &Harness<'static, PdfCraftApp>, text: &str) -> Option<egui::Rect> {
    /// The rectangle of the text shape in `shape` that reads exactly `text`, searching nested
    /// shape lists.
    fn walk(shape: &egui::Shape, text: &str) -> Option<egui::Rect> {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| walk(s, text)),
            egui::Shape::Text(t) if t.galley.text() == text => Some(egui::Rect::from_min_size(t.pos, t.galley.size())),
            _ => None,
        }
    }
    h.output().shapes.iter().find_map(|c| walk(&c.shape, text))
}

/// Gives the "country" drop-down 30 options, "0000" to "2900", more than its list shows at once.
fn long_country(h: &mut Harness<'static, PdfCraftApp>) {
    let options: Vec<String> = (0..30).map(|i| format!("{i:02}00")).collect();
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::SetFieldProps {
            name: "country".into(),
            props: Box::new(pdfcraft_engine::FieldProps { options: Some(options), ..Default::default() }),
        }
    ));
    h.run_steps(3);
}

/// A centered drop-down (quadding 1) centers the items of its open list too.
#[test]
fn a_drop_downs_list_aligns_its_items_as_the_field_does() {
    let mut h = harness();
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::SetFieldProps {
            name: "country".into(),
            props: Box::new(pdfcraft_engine::FieldProps { quadding: Some(1), ..Default::default() }),
        }
    ));
    h.run_steps(3);
    click_field(&mut h, "country", 0);
    let country = field_rect(&h, "country");
    let france = text_rect(&h, "France").expect("drawn");
    assert!((france.center().x - country.center().x).abs() <= 2.0, "centered as the field is: {france:?} in {country:?}");
}

/// Escape in an open drop-down closes its list and nothing else: the find bar, which Escape
/// closes when no field is open, stays open.
#[test]
fn escape_closes_an_open_drop_downs_list_and_leaves_the_page_alone() {
    let mut h = harness();
    long_country(&mut h);
    h.state_mut().views[0].open_find();
    h.run_steps(3);
    assert!(h.state().views[0].find.is_some());
    click_field(&mut h, "country", 0);
    assert!(egui_kittest::kittest::Queryable::query_by_label(&h, "0000").is_some(), "the list is open");
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(egui_kittest::kittest::Queryable::query_by_label(&h, "0000").is_none(), "Escape closed the list");
    assert!(h.state().views[0].session.forms.focus.is_none(), "and the field");
    assert!(h.state().views[0].find.is_some(), "the find bar stays open");
}

/// An auto-sized drop-down's list draws its items at the size the field shows its value at, so a
/// long chosen item fits the list as it fits the field instead of growing past it.
#[test]
fn an_auto_sized_drop_downs_list_shows_its_items_at_the_fields_text_size() {
    let mut h = harness();
    let long = "The Commonwealth of the Northern Mariana Islands";
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::SetFieldProps {
            name: "country".into(),
            props: Box::new(pdfcraft_engine::FieldProps { options: Some(vec!["Canada".into(), long.into()]), ..Default::default() }),
        }
    ));
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::SetFieldValue { name: "country".into(), value: pdfcraft_engine::FieldValue::Choice(vec![long.into()]) }
    ));
    h.run_steps(3);
    click_field(&mut h, "country", 0);
    let field = field_rect(&h, "country");
    let item = text_rect(&h, long).expect("the long item in the open list");
    assert!(item.width() <= field.width(), "the item fits as the field's value does: {} wide in a {} field", item.width(), field.width());
}
