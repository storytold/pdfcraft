//! Prepare a form in the real shell (egui_kittest): field tools, placing, selecting, moving,
//! Field Properties and deleting.

use crate::support::AppHarness as _;
use craft::ui::dialog::StackDialog;
use craft::ui::runtime::env::WindowKey;
use craft::ui::runtime::toast;
use egui::Pos2;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable};
use pdfcraft_ui_egui::field_table_ui::FieldTableDialog;
use pdfcraft_ui_egui::prepare::FieldPropsDialog;
use pdfcraft_ui_egui::{PdfCraftApp, QuickTool};

fn harness() -> Harness<'static, PdfCraftApp> {
    harness_with("form.pdf", include_bytes!("../data/form.pdf").to_vec())
}

fn harness_with(name: &'static str, bytes: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    harness_on(crate::control::new_app(), name, bytes)
}

/// The form fixture in an app whose pickers answer from `files`.
fn harness_with_files(files: std::sync::Arc<craft::host::ScriptedFiles>) -> Harness<'static, PdfCraftApp> {
    harness_on(crate::control::app_with_files(files), "form.pdf", include_bytes!("../data/form.pdf").to_vec())
}

fn harness_on(app: PdfCraftApp, name: &'static str, bytes: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            cc.egui_ctx.options_mut(|options| options.warn_on_id_clash = true);
            let mut app = app;
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), name, None, bytes).unwrap();
            app.set_option(&egui::Context::default(), "zoom", "150").unwrap();
            app
        }
    });
    crate::support::settle(&mut h);
    h
}

/// A point on page 1 at (x, y) points from its top-left.
fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = r.width() / 300.0;
    r.min + egui::vec2(x * k, y * k)
}

fn drag(h: &mut Harness<'static, PdfCraftApp>, a: Pos2, b: Pos2) {
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(a + (b - a) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(4);
}

fn names(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().form.iter().map(|f| f.name.clone()).collect()
}

fn rect_of(h: &Harness<'static, PdfCraftApp>, name: &str) -> [f64; 4] {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == name).unwrap().widgets[0].rect
}

/// Opens Field Properties of widget 0 of `name` through the control key, as a script or the Fields
/// panel does.
fn open_props(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    h.set_option("dialog", &format!("field-properties:{name}")).unwrap();
    h.run_steps(2);
}

/// Whether Field Properties is open.
fn props_open(h: &Harness<'static, PdfCraftApp>) -> bool {
    h.state().dialogs.is_open::<FieldPropsDialog>()
}

/// The values Field Properties holds, as the dialog host reports them to automation.
fn props(h: &Harness<'static, PdfCraftApp>) -> serde_json::Value {
    h.state().dialogs.get::<FieldPropsDialog>().and_then(|d| d.json_fields()).expect("Field Properties is open")
}

/// Whether the text field table's window is open.
fn table_open(h: &Harness<'static, PdfCraftApp>) -> bool {
    h.state().dialogs.is_open::<FieldTableDialog>()
}

/// The values the text field table's window holds, as the dialog host reports them to automation.
fn table(h: &Harness<'static, PdfCraftApp>) -> serde_json::Value {
    h.state().dialogs.get::<FieldTableDialog>().and_then(|d| d.json_fields()).expect("the table's window is open")
}

/// What the text box `label` holds.
fn text_of(h: &Harness<'static, PdfCraftApp>, label: &str) -> Option<String> {
    h.get_by_role_and_label(Role::TextInput, label).value()
}

/// Selects all of the text box `label` and types `text` over it.
fn retype(h: &mut Harness<'static, PdfCraftApp>, label: &str, text: &str) {
    h.get_by_role_and_label(Role::TextInput, label).click();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get_by_role_and_label(Role::TextInput, label).type_text(text);
    h.run_steps(2);
}

/// Opens the list `label` and picks `option`. The open list is the last thing in the tree, so an
/// option that a row of the dialog also says is still the list's.
fn pick(h: &mut Harness<'static, PdfCraftApp>, label: &str, option: &str) {
    h.get_by_label(label).click();
    h.run_steps(2);
    h.get_all_by_label(option).last().expect("the list offers the option").click();
    h.run_steps(2);
}

/// Clicks the tab `label` of Field Properties.
fn tab(h: &mut Harness<'static, PdfCraftApp>, label: &str) {
    h.get_by_role_and_label(Role::Tab, label).click();
    h.run_steps(2);
}

/// The label of the document's next undo step.
fn undo_label(h: &Harness<'static, PdfCraftApp>) -> Option<String> {
    h.state().session.get(h.state().views[0].id).unwrap().can_undo().map(str::to_owned)
}

/// What every notice card the main window shows says first, oldest first: a posted notice's text is
/// its card's title.
fn notices(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    toast::cards(&h.ctx, &WindowKey::main()).iter().map(|card| card.toast.title.as_ref().unwrap_or(&card.toast.message).as_str().to_owned()).collect()
}

/// The row of the calculation order page for the field `name`, whatever its position and value: the row says the
/// field's name and value, its number is the row's lead.
fn order_row<'h>(h: &'h Harness<'static, PdfCraftApp>, name: &str) -> egui_kittest::Node<'h> {
    let pattern = format!("{name}: ");
    h.get_by(move |node| node.label().is_some_and(|label| label.starts_with(&pattern)))
}

/// Whether a card of the calculation order page says `text`, as its heading or one of its lines: a card
/// with lines is named by its heading and lines together.
fn card_says(h: &Harness<'static, PdfCraftApp>, text: &str) -> bool {
    h.query_by_label_contains(text).is_some()
}

fn text_center_near(h: &Harness<'static, PdfCraftApp>, label: &str, expected: Pos2) -> Option<Pos2> {
    h.output()
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == label => Some(text.visual_bounding_rect().center()),
            _ => None,
        })
        .min_by(|a, b| a.distance(expected).total_cmp(&b.distance(expected)))
}

#[test]
fn shift_selected_small_widgets_have_blue_tags_and_visible_outlines() {
    use egui::{Color32, Event, Modifiers, PointerButton, Shape, StrokeKind};
    use pdfcraft_engine::{Edit, NewField, Session};

    let mut session = Session::new();
    let bytes = session.create_blank(300.0, 400.0, 1).unwrap();
    let id = session.open("selection.pdf", None, bytes, None).unwrap();
    for (name, rect, kind) in [
        ("first", [40.0, 290.0, 54.0, 304.0], NewField::CheckBox),
        ("second", [90.0, 290.0, 104.0, 304.0], NewField::CheckBox),
        ("text", [40.0, 230.0, 230.0, 254.0], NewField::Text { multiline: false }),
    ] {
        session.apply(id, Edit::AddField { name: Some(name.into()), page: 0, rect, kind }).unwrap();
    }
    let mut h = harness_with("selection.pdf", session.save_bytes(id).unwrap().as_ref().clone());
    assert!(h.execute("form.prepare"));
    h.run_steps(3);
    for (x, modifiers) in [(47.0, Modifiers::NONE), (97.0, Modifiers::SHIFT)] {
        let pos = at(&h, x, 103.0);
        h.event(Event::ModifiersChanged(modifiers));
        h.hover_at(pos);
        h.run_steps(1);
        for pressed in [true, false] {
            h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers });
            h.run_steps(1);
        }
    }
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
    assert_eq!(h.state().views[0].session.prepare.names().len(), 2);
    let shapes = &h.output().shapes;
    // The name tag over a widget: the smallest filled rectangle under its centre, with where it was
    // painted in the frame.
    let tag = |center: Pos2| {
        shapes
            .iter()
            .enumerate()
            .filter(|(_, s)| s.clip_rect.contains(center))
            .filter_map(|(i, s)| match &s.shape {
                Shape::Rect(r) if r.rect.contains(center) && r.fill != Color32::TRANSPARENT => Some((i, r.rect.area(), r.fill)),
                _ => None,
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _, fill)| (i, fill))
    };
    let (first, second, unselected) = (at(&h, 47.0, 103.0), at(&h, 97.0, 103.0), at(&h, 135.0, 158.0));
    let (first_tag, second_tag, other_tag) = (tag(first), tag(second), tag(unselected));
    let ink = first_tag.expect("the selected small widget has a name tag").1;
    assert_eq!(second_tag.expect("the second selected widget has a name tag").1, ink, "selected tags share their ink");
    assert_ne!(other_tag.expect("the unselected field has a name tag").1, ink, "an unselected tag differs from a selected one");
    let outline = |center: Pos2, ink: Color32| {
        shapes.iter().position(|s| {
            matches!(&s.shape, Shape::Rect(r)
            if r.rect.contains(center) && r.stroke.color == ink && r.stroke_kind == StrokeKind::Outside)
        })
    };
    for (center, tag) in [(first, first_tag), (second, second_tag)] {
        let outline = outline(center, ink);
        assert!(outline > tag.map(|t| t.0), "the selected outline must be outside and painted after the tag");
    }
    assert!(outline(unselected, ink).is_none(), "unselected text field has no selection outline");
    // The anchor, the first one picked, shows a box of corner handles only.
    let handles: Vec<Pos2> = ["Top left corner", "Top right corner", "Bottom right corner", "Bottom left corner"]
        .into_iter()
        .map(|label| h.get_by_role_and_label(Role::Button, label).rect().center())
        .collect();
    for edge in ["Top edge", "Right edge", "Bottom edge", "Left edge"] {
        assert!(h.query_by_role_and_label(Role::Button, edge).is_none(), "no edge handle {edge}");
    }
    assert_eq!(handles.len(), 4, "only the primary has resize handles, at its corners");
    let distinct = |value: fn(&Pos2) -> f32| {
        let mut v: Vec<i32> = handles.iter().map(|p| value(p).round() as i32).collect();
        v.sort_unstable();
        v.dedup();
        v.len()
    };
    assert_eq!((distinct(|p| p.x), distinct(|p| p.y)), (2, 2), "four corners of one box: {handles:?}");
    let box_rect = egui::Rect::from_points(&handles);
    assert!(box_rect.contains(first) && !box_rect.contains(second), "handles surround only the primary: {box_rect:?}");
}

#[test]
fn placing_moving_editing_and_deleting_a_field() {
    let mut h = harness();
    let before = names(&h).len();
    assert!(h.execute("form.add.text"));
    h.run_steps(2);
    assert!(h.query_all_by_label("Prepare a form").count() >= 1, "the panel title shows");
    // Drag out a text field in the empty lower part of the page.
    let (a, b) = (at(&h, 40.0, 300.0), at(&h, 200.0, 322.0));
    drag(&mut h, a, b);
    let all = names(&h);
    assert_eq!(all.len(), before + 1, "{all:?}");
    assert_eq!(all.last().map(String::as_str), Some("Text1"));
    assert_eq!(h.state().quick_tool, QuickTool::Select, "back to Select after one field");
    h.run_steps(2);
    assert_eq!(h.state().views[0].session.prepare.selected.as_ref().map(|s| s.0.as_str()), Some("Text1"), "the new field is selected");
    let r = rect_of(&h, "Text1");
    assert!((r[2] - r[0] - 160.0).abs() < 2.0 && (r[3] - r[1] - 22.0).abs() < 2.0, "{r:?}");
    // Move it 20 pt right.
    let c = at(&h, 120.0, 311.0);
    let d = c + egui::vec2(at(&h, 20.0, 0.0).x - at(&h, 0.0, 0.0).x, 0.0);
    drag(&mut h, c, d);
    let m = rect_of(&h, "Text1");
    assert!((m[0] - r[0] - 20.0).abs() < 1.5, "moved: {r:?} → {m:?}");
    // Field Properties: rename and make it required (one undo step).
    h.execute("form.field.properties");
    h.run_steps(2);
    h.get_by_role_and_label(Role::Dialog, "Text Field Properties");
    assert!(props_open(&h));
    retype(&mut h, "Name:", "email");
    h.get_by_label("Required").click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(names(&h).contains(&"email".to_string()));
    {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        assert_eq!(doc.can_undo(), Some("Change field properties"));
        assert!(doc.form.iter().find(|f| f.name == "email").unwrap().has(pdfcraft_engine::field_flags::REQUIRED));
        assert_eq!(s.views[0].session.prepare.selected.as_ref().map(|s| s.0.as_str()), Some("email"));
    }
    // Delete removes the selected field.
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert_eq!(names(&h).len(), before);
    // A click with the check box tool places a default-size one.
    h.execute("form.add.checkbox");
    h.run_steps(1);
    let p = at(&h, 40.0, 250.0);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(4);
    let r = rect_of(&h, "Check Box1");
    assert_eq!((r[2] - r[0], r[3] - r[1]), (14.0, 14.0));
    h.run_steps(2);
    assert_eq!(h.state().views[0].session.prepare.selected.as_ref().map(|s| s.0.as_str()), Some("Check Box1"), "the clicked check box is selected");
}

#[test]
fn format_validate_and_calculate_tabs() {
    use pdfcraft_engine::form_scripts::{Calculate, Format, Validate};
    let mut h = harness();
    h.execute("form.prepare");
    h.run_steps(2);
    open_props(&mut h, "city");
    tab(&mut h, "Format");
    pick(&mut h, "Select format category:", "Number");
    pick(&mut h, "Currency symbol:", "$");
    h.get_by_label_contains("Example: -$1,234.50");
    tab(&mut h, "Validate");
    choose(&mut h, "Field value is in range:");
    // The range opens as 0 to 100; unticking "To:" leaves it open above.
    h.get_by_role_and_label(Role::CheckBox, "To:").click();
    h.run_steps(2);
    tab(&mut h, "Calculate");
    choose(&mut h, NOTATION);
    retype_box(&mut h, NOTATION, "name");
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let f = s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "city").unwrap().clone();
    assert!(matches!(f.actions.format, Format::Number { decimals: 2, .. }));
    assert_eq!(f.actions.validate, Validate::Range { min: Some(0.0), max: None });
    assert_eq!(f.actions.calculate, Calculate::Notation("name".into()));
}

/// The radio of the Calculate tab's custom calculation script (#6630).
const CUSTOM: &str = "Custom calculation script:";

/// What the Calculate tab says when an edit would take the custom script past 256 KiB.
const REFUSAL: &str = "A script edited here can be at most 256 KiB long; that change wasn't made.";

/// The longest custom script the Calculate tab edits, in bytes.
const MAX_EDITED_SCRIPT: usize = 256 * 1024;

/// The harness in Prepare Form, with `city`'s calculation set to `js`, a script of its own.
fn with_calculate_script(js: &str) -> Harness<'static, PdfCraftApp> {
    let mut h = harness();
    set_calculate_script(&mut h, js);
    h.execute("form.prepare");
    h.run_steps(2);
    h
}

/// Sets `city`'s calculate script, as form_set_script does.
fn set_calculate_script(h: &mut Harness<'static, PdfCraftApp>, js: &str) {
    let edit = pdfcraft_engine::Edit::SetFieldScript { name: "city".into(), event: "calculate".into(), script: Some(js.into()) };
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx_h, edit));
    h.run_steps(2);
}

/// Opens Field Properties of `name` on the Calculate tab.
fn open_calculate(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    open_props(h, name);
    tab(h, "Calculate");
}

/// Clicks the dialog button `label` (OK or Cancel).
fn press(h: &mut Harness<'static, PdfCraftApp>, label: &str) {
    h.get_by_label(label).click();
    h.run_steps(3);
}

/// Clicks the Calculate tab's radio `label`.
fn choose(h: &mut Harness<'static, PdfCraftApp>, label: &str) {
    h.get_by_role_and_label(Role::RadioButton, label).click();
    h.run_steps(2);
}

/// Whether the Calculate tab's radio `label` is selected.
fn chosen(h: &Harness<'static, PdfCraftApp>, label: &str) -> bool {
    use egui_kittest::kittest::NodeT;
    h.get_by_role_and_label(Role::RadioButton, label).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
}

/// `city`'s calculation as the document has it: the recognised one and the script of its own.
fn city_calculation(h: &Harness<'static, PdfCraftApp>) -> (pdfcraft_engine::form_scripts::Calculate, Option<String>) {
    let s = h.state();
    let f = s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "city").unwrap().clone();
    (f.actions.calculate, f.actions.scripts.calculate)
}

/// The Calculate tab's box for a formula in simplified field notation.
const NOTATION: &str = "Simplified field notation:";
/// The Actions tab's box for a Run a JavaScript action's script.
const ACTION_SCRIPT: &str = "Script:";

/// What each multi-line box labelled `label` holds.
fn boxes(h: &Harness<'static, PdfCraftApp>, label: &str) -> Vec<String> {
    h.query_all_by_role_and_label(Role::MultilineTextInput, label).filter_map(|n| n.value()).collect()
}

/// What each custom calculation script box on the tab holds.
fn script_boxes(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    boxes(h, CUSTOM)
}

/// The script box's description: what a screen reader says about it besides its text.
fn script_box_description(h: &Harness<'static, PdfCraftApp>) -> Option<String> {
    use egui_kittest::kittest::NodeT;
    h.get_by_role_and_label(Role::MultilineTextInput, CUSTOM).accesskit_node().description()
}

/// Selects everything in the multi-line box labelled `label` and types `text` over it.
fn retype_box(h: &mut Harness<'static, PdfCraftApp>, label: &str, text: &str) {
    h.get_by_role_and_label(Role::MultilineTextInput, label).click();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get_by_role_and_label(Role::MultilineTextInput, label).type_text(text);
    h.run_steps(2);
}

/// Selects everything in the custom calculation script box and types `text` over it.
fn retype_script(h: &mut Harness<'static, PdfCraftApp>, text: &str) {
    retype_box(h, CUSTOM, text);
}

/// Pastes `text` into the script box where its cursor is.
fn paste_script(h: &mut Harness<'static, PdfCraftApp>, text: &str) {
    h.get_by_role_and_label(Role::MultilineTextInput, CUSTOM).click();
    h.run_steps(1);
    h.event(egui::Event::Paste(text.into()));
    h.run_steps(2);
}

/// A field whose calculation is a script of its own shows it on the Calculate tab as a custom
/// calculation script, not as "Value is not calculated" (#6630); another choice and back brings
/// the script back, and OK without a change keeps it.
#[test]
fn the_calculate_tab_shows_a_custom_calculation_script() {
    let js = "event.value = \"*\" + this.getField(\"name\").value + \"*\";";
    let mut h = with_calculate_script(js);
    open_calculate(&mut h, "city");
    assert!(chosen(&h, CUSTOM), "the script is the field's calculation");
    assert!(!chosen(&h, "Value is not calculated"));
    assert_eq!(script_boxes(&h), [js]);
    assert_eq!(script_box_description(&h), None, "nothing to say about it");
    choose(&mut h, "Simplified field notation:");
    assert!(!script_boxes(&h).contains(&js.to_owned()));
    choose(&mut h, CUSTOM);
    assert_eq!(script_boxes(&h), [js]);
    let undo = undo_label(&h);
    press(&mut h, "OK");
    assert_eq!(undo_label(&h), undo, "an untouched tab changes nothing");
    assert_eq!(city_calculation(&h), (pdfcraft_engine::form_scripts::Calculate::None, Some(js.to_owned())));
}

/// Only a script that is no sum or notation opens as a custom script: a sum stays a sum, and a
/// field with no calculation opens on "Value is not calculated", with no script box.
#[test]
fn a_sum_or_no_calculation_does_not_open_as_a_custom_script() {
    let mut h = with_calculate_script("AFSimple_Calculate(\"SUM\", new Array(\"name\"));");
    open_calculate(&mut h, "city");
    assert!(chosen(&h, "Value is the"), "the sum is recognised");
    assert!(!chosen(&h, CUSTOM));
    assert!(script_boxes(&h).is_empty());
    press(&mut h, "Cancel");
    open_calculate(&mut h, "name");
    assert!(chosen(&h, "Value is not calculated"));
    assert!(!chosen(&h, CUSTOM));
    assert!(script_boxes(&h).is_empty());
}

/// An edited custom script is written as typed, and "Value is not calculated" removes it.
#[test]
fn an_edited_custom_script_is_written_and_no_calculation_removes_it() {
    use pdfcraft_engine::form_scripts::Calculate;
    let mut h = with_calculate_script("event.value = 1;");
    open_calculate(&mut h, "city");
    let edited = "event.value = this.getField(\"name\").value; // città";
    retype_script(&mut h, edited);
    assert_eq!(script_boxes(&h), [edited]);
    press(&mut h, "OK");
    assert_eq!(city_calculation(&h), (Calculate::None, Some(edited.to_owned())));
    open_calculate(&mut h, "city");
    choose(&mut h, "Value is not calculated");
    press(&mut h, "OK");
    assert_eq!(city_calculation(&h), (Calculate::None, None));
}

/// An edit is taken up to exactly 256 KiB, counted in bytes as the JavaScript engine counts them;
/// one past it, by a byte or by a multi-byte paste of fewer characters, is refused, the box keeps
/// what it had, and the refusal is said beside the box and in its description until the next edit
/// is taken.
#[test]
fn an_edit_past_256_kib_is_refused_and_the_next_edit_is_taken() {
    use pdfcraft_engine::form_scripts::Calculate;
    let js = "event.value = 1;";
    let mut h = with_calculate_script(js);
    open_calculate(&mut h, "city");
    let refused = |h: &Harness<'static, PdfCraftApp>| (h.query_by_label(REFUSAL).is_some(), script_box_description(h));
    assert_eq!(refused(&h), (false, None));
    // 100 Ki euro signs are 300 KiB: fewer characters than the limit, more bytes.
    for paste in ["x".repeat(300 * 1024), "€".repeat(100 * 1024)] {
        paste_script(&mut h, &paste);
        assert_eq!(refused(&h), (true, Some(REFUSAL.to_owned())), "{} bytes", paste.len());
        assert_eq!(script_boxes(&h), [js], "the box keeps what it had");
    }
    // Exactly at the limit is taken, and the refusal is no longer said.
    let full = format!("/*{}*/", "x".repeat(MAX_EDITED_SCRIPT - 4));
    retype_script(&mut h, &full);
    assert_eq!(refused(&h), (false, None));
    assert!(script_boxes(&h) == [full.as_str()], "the box holds the full 256 KiB");
    // One byte more is not.
    h.key_press(egui::Key::End);
    h.get_by_role_and_label(Role::MultilineTextInput, CUSTOM).type_text(" ");
    h.run_steps(2);
    assert!(refused(&h).0, "256 KiB and a byte");
    assert!(script_boxes(&h) == [full.as_str()]);
    press(&mut h, "OK");
    assert!(city_calculation(&h) == (Calculate::None, Some(full)));
}

/// The refusal is forgotten when another calculation is chosen or the dialog is opened again, and
/// choosing Custom again brings back the field's own script, not an edit that was left.
#[test]
fn a_refusal_is_forgotten_on_another_choice_or_reopening() {
    use pdfcraft_engine::form_scripts::Calculate;
    let js = "event.value = 1;";
    let mut h = with_calculate_script(js);
    open_calculate(&mut h, "city");
    retype_script(&mut h, "event.value = 2;");
    paste_script(&mut h, &"x".repeat(300 * 1024));
    assert!(h.query_by_label(REFUSAL).is_some());
    choose(&mut h, "Simplified field notation:");
    assert!(h.query_by_label(REFUSAL).is_none(), "another choice forgets it");
    choose(&mut h, CUSTOM);
    assert!(h.query_by_label(REFUSAL).is_none());
    assert_eq!(script_box_description(&h), None);
    assert_eq!(script_boxes(&h), [js], "the field's own script, not the edit left behind");
    paste_script(&mut h, &"x".repeat(300 * 1024));
    assert!(h.query_by_label(REFUSAL).is_some());
    press(&mut h, "Cancel");
    open_calculate(&mut h, "city");
    assert!(h.query_by_label(REFUSAL).is_none(), "opening the dialog again forgets it");
    press(&mut h, "OK");
    assert_eq!(city_calculation(&h), (Calculate::None, Some(js.to_owned())));
}

/// A script past 256 KiB isn't edited: the box shows its start, read-only (cut at a character, not
/// inside one), says why, and OK keeps the whole script.
#[test]
fn a_script_past_256_kib_shows_its_start_and_is_kept() {
    use pdfcraft_engine::form_scripts::Calculate;
    // 18 + 3 × 100 Ki + 2 bytes: 256 KiB falls inside a euro sign.
    let long = format!("event.value = 1;/*{}*/", "€".repeat(100 * 1024));
    let mut h = with_calculate_script(&long);
    open_calculate(&mut h, "city");
    assert!(chosen(&h, CUSTOM));
    let note = "This script is too long to edit here (300 KiB); its start is shown. It is kept as it is.";
    assert!(h.query_by_label(note).is_some());
    assert_eq!(script_box_description(&h).as_deref(), Some(note));
    let start = &long[..MAX_EDITED_SCRIPT - 1];
    assert!(script_boxes(&h) == [start], "the box shows the start, up to the last whole character");
    h.get_by_role_and_label(Role::MultilineTextInput, CUSTOM).click();
    h.run_steps(1);
    h.get_by_role_and_label(Role::MultilineTextInput, CUSTOM).type_text("x");
    h.run_steps(2);
    assert!(script_boxes(&h) == [start], "and can't be typed in");
    press(&mut h, "OK");
    assert!(city_calculation(&h) == (Calculate::None, Some(long)));
}

/// Renaming a field in Field Properties renames it in the formulas that read it, in the same undo
/// step, and says how many changed.
#[test]
fn renaming_a_field_says_how_many_calculations_now_use_the_new_name() {
    use pdfcraft_engine::form_scripts::Calculate;
    let mut h = harness();
    h.execute("form.prepare");
    h.run_steps(2);
    let rename = |h: &mut Harness<'static, PdfCraftApp>, from: &str, to: &str| {
        open_props(h, from);
        retype(h, "Name:", to);
        h.get_by_label("OK").click();
        h.run_steps(3);
    };
    open_calculate(&mut h, "city");
    choose(&mut h, NOTATION);
    retype_box(&mut h, NOTATION, "name * 2");
    h.get_by_label("OK").click();
    h.run_steps(3);
    let calculation = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "city").unwrap().actions.calculate.clone()
    };
    let shown = notices(&h);
    rename(&mut h, "name", "full_name");
    assert_eq!(calculation(&h), Calculate::Notation("full_name * 2".into()));
    let after = notices(&h);
    assert_eq!(after.len(), shown.len() + 1, "one new card: {after:?}");
    assert_eq!(after.last().map(String::as_str), Some("1 calculation now uses the new name."));
    rename(&mut h, "city", "town");
    assert!(names(&h).contains(&"town".to_string()));
    // Cards shown earlier may have timed out since; none is new.
    let now = notices(&h);
    let count = |cards: &[String], card: &String| cards.iter().filter(|c| *c == card).count();
    assert!(
        now.iter().all(|card| count(&now, card) <= count(&after, card)),
        "no formula reads the calculated field itself, so no new card: {after:?} → {now:?}"
    );
    h.execute("edit.undo");
    h.execute("edit.undo");
    h.run_steps(2);
    assert!(names(&h).contains(&"name".to_string()));
    assert_eq!(calculation(&h), Calculate::Notation("name * 2".into()), "undone with the rename");
}

#[test]
fn appearance_tab_restyles_the_field() {
    use pdfcraft_engine::{BorderStyle, FieldFont};
    let mut h = harness();
    h.execute("form.prepare");
    h.run_steps(2);
    let before = {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().field_look("city").unwrap()
    };
    open_props(&mut h, "city");
    tab(&mut h, "Appearance");
    h.get_by_label("Line Style:");
    fn no_grid_id_warning(shape: &egui::Shape) -> bool {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().all(no_grid_id_warning),
            egui::Shape::Text(text) => !text.galley.job.text.contains("Grid ID"),
            _ => true,
        }
    }
    // Check egui's actual painted diagnostics, rather than reproducing the ID formula.
    assert!(h.output().shapes.iter().all(|shape| no_grid_id_warning(&shape.shape)), "palette grids must not share an ID");
    assert_eq!(h.get_all_by_label("No color").count(), 2);
    // Each "No color" toggles its own colour: the first is the border's, the second the fill's.
    h.get_all_by_label("No color").next().unwrap().click();
    h.run_steps(2);
    h.get_all_by_label("No color").nth(1).unwrap().click();
    h.run_steps(2);
    pick(&mut h, "Line Style:", "Dashed");
    pick(&mut h, "Font:", "Courier");
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let l = s.session.get(s.views[0].id).unwrap().field_look("city").unwrap();
    assert_eq!(l.border, if before.border.is_some() { None } else { Some([0.0; 3]) });
    assert_eq!(l.fill, if before.fill.is_some() { None } else { Some([0.0; 3]) });
    assert_eq!((l.style, l.font), (BorderStyle::Dashed, FieldFont::Courier));
}

#[test]
fn appearance_tab_keeps_auto_font_size() {
    let mut h = harness();
    h.execute("form.prepare");
    h.run_steps(2);
    open_props(&mut h, "city");
    tab(&mut h, "Appearance");
    let size = |h: &Harness<'static, PdfCraftApp>| props(h)["font_size"].as_f64().expect("a font size");
    if size(&h) == 0.0 {
        // Already Auto: turn it off so that choosing Auto is a change.
        h.get_by_label("Auto").click();
        h.run_steps(2);
        assert_ne!(size(&h), 0.0);
    }
    h.get_by_label("Auto").click();
    h.run_steps(3);
    assert_eq!(size(&h), 0.0, "Auto (0) must survive a frame");
}

#[test]
fn position_tab_rotates_a_field() {
    let mut h = harness();
    open_props(&mut h, "city");
    let widget = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        let f = s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "city").unwrap();
        f.widgets[0].clone()
    };
    let before = widget(&h);
    assert_eq!(before.rotation, 0);
    tab(&mut h, "Position");
    // Picked from the list, as a person does.
    pick(&mut h, "Rotation:", "90°");
    h.get_by_label("OK").click();
    h.run_steps(3);
    let turned = widget(&h);
    assert_eq!(turned.rotation, 90);
    // The field stays where it was placed; only its content turns (#4296).
    assert!(turned.rect.iter().zip(before.rect).all(|(a, b)| (a - b).abs() < 0.05), "{:?} moved from {:?}", turned.rect, before.rect);
    h.execute("edit.undo");
    h.run_steps(3);
    let back = widget(&h);
    assert_eq!(back.rotation, 0);
    assert!(back.rect.iter().zip(before.rect).all(|(a, b)| (a - b).abs() < 0.05), "{:?} vs {:?}", back.rect, before.rect);
}

#[test]
fn options_tab_sets_the_check_box_style() {
    // #94: Check Box Style on the Options tab.
    use pdfcraft_engine::CheckStyle;
    let mut h = harness();
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::AddField {
            page: 0,
            rect: [40.0, 300.0, 56.0, 316.0],
            kind: pdfcraft_engine::NewField::CheckBox,
            name: Some("agree".into()),
        },
    );
    open_props(&mut h, "agree");
    tab(&mut h, "Options");
    h.get_by_value("Check");
    pick(&mut h, "Check Box Style:", "Cross");
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().field_check_style("agree"), Some(CheckStyle::Cross));
}

#[test]
fn button_label_is_editable_without_renaming_the_field() {
    use pdfcraft_engine::{Edit, NewField};
    let mut h = harness();
    // The byte comparisons below must not vary with the save's /ModDate.
    let session = &mut h.state_mut().session;
    *session = std::mem::take(session).with_clock(|| 1_700_000_000);
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(
        &ctx_h,
        Edit::AddField {
            page: 0,
            rect: [40.0, 300.0, 190.0, 330.0],
            kind: NewField::Button { caption: String::new() },
            name: Some("clear_button".into()),
        },
    );
    open_props(&mut h, "clear_button");
    tab(&mut h, "Options");
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Label").click();
    h.run_steps(1);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Label").type_text("Clear Form");
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);

    let app = h.state();
    let id = app.views[0].id;
    let field = app.session.get(id).unwrap().form.iter().find(|f| f.name == "clear_button").unwrap();
    let bytes = app.session.save_bytes(id).unwrap();
    let doc = pdfcraft_cos::Document::open(bytes).unwrap();
    let widget = doc.get(field.widgets[0].obj);
    let mk = doc.resolve(widget.as_dict().unwrap().get(b"MK").unwrap());
    assert_eq!(doc.resolve(mk.as_dict().unwrap().get(b"CA").unwrap()).as_string().unwrap().to_text(), "Clear Form");
    assert_eq!(app.session.get(id).unwrap().can_undo(), Some("Change field properties"));

    // Read the existing label back, and leave both Cancel and an unchanged OK harmless.
    let before = app.session.save_bytes(id).unwrap();
    open_props(&mut h, "clear_button");
    tab(&mut h, "Options");
    assert_eq!(text_of(&h, "Label").as_deref(), Some("Clear Form"));
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Label").click();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Label").type_text("Discard this");
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(h.state().session.save_bytes(id).unwrap(), before);
    open_props(&mut h, "clear_button");
    assert!(!h.get_by_label("OK").accesskit_node().is_disabled());
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert!(!props_open(&h));
    assert_eq!(h.state().session.save_bytes(id).unwrap(), before);
}

/// Option actions stay available without having to hover a row first.
#[test]
fn choice_option_actions_are_available_without_hover() {
    use pdfcraft_engine::{Edit, NewField};
    let mut h = harness();
    let ctx = h.ctx.clone();
    h.state_mut().apply_edit(
        &ctx,
        Edit::AddField {
            page: 0,
            rect: [40.0, 260.0, 240.0, 310.0],
            kind: NewField::Combo { options: vec!["Apple".into(), "Banana".into()], editable: false },
            name: Some("fruit".into()),
        },
    );
    open_props(&mut h, "fruit");
    tab(&mut h, "Options");
    h.get_by_label("Delete Apple");
    h.get_by_label("Delete Banana");
    h.get_by_label("Up").click();
    h.run_steps(2);
    h.get_by_label("Delete Apple").click();
    h.run_steps(2);
    assert!(h.query_by_label("Delete Apple").is_none());
    h.get_by_label("Delete Banana");
}

/// A drop-down and a list box each take an alignment from the Options tab, keep their items,
/// value and flags, and write it to the file.
#[test]
fn dropdown_alignment_is_editable_and_preserved() {
    choice_alignment(pdfcraft_engine::NewField::Combo { options: vec!["One".into(), "Two".into()], editable: false }, "Center", 1);
    choice_alignment(pdfcraft_engine::NewField::List { options: vec!["One".into(), "Two".into()], multi: true }, "Left", 0);
}

fn choice_alignment(kind: pdfcraft_engine::NewField, label: &str, quadding: i64) {
    use pdfcraft_engine::{Edit, FieldProps, FieldValue};
    let mut h = harness();
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, Edit::AddField { page: 0, rect: [40.0, 260.0, 240.0, 310.0], kind, name: Some("aligned_choice".into()) });
    h.state_mut().apply_edit(
        &egui::Context::default(),
        Edit::SetFieldProps { name: "aligned_choice".into(), props: Box::new(FieldProps { quadding: Some(2), ..Default::default() }) },
    );
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, Edit::SetFieldValue { name: "aligned_choice".into(), value: FieldValue::Choice(vec!["Two".into()]) });
    let field = |h: &Harness<'static, PdfCraftApp>| {
        let app = h.state();
        app.session.get(app.views[0].id).unwrap().form.iter().find(|f| f.name == "aligned_choice").unwrap().clone()
    };
    let original = field(&h);
    open_props(&mut h, "aligned_choice");
    tab(&mut h, "Options");
    h.get_by_label("Alignment:");
    // The imported right alignment is displayed and unchanged until a choice is made.
    h.get_by_value("Right").click();
    h.run_steps(2);
    h.get_by_label(label).click();
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(field(&h).quadding, 2, "cancel keeps the existing /Q");
    open_props(&mut h, "aligned_choice");
    tab(&mut h, "Options");
    h.get_by_value("Right").click();
    h.run_steps(2);
    h.get_by_label(label).click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    let changed = field(&h);
    assert_eq!(changed.quadding, quadding);
    assert_eq!((changed.options, changed.value, changed.flags), (original.options, original.value, original.flags));
    let app = h.state();
    let id = app.views[0].id;
    assert_eq!(app.session.get(id).unwrap().can_undo(), Some("Change field properties"));
    let bytes = app.session.save_bytes(id).unwrap();
    let mut reopened = pdfcraft_engine::Session::new();
    let reopened_id = reopened.open("saved.pdf", None, bytes, None).unwrap();
    assert_eq!(reopened.get(reopened_id).unwrap().form.iter().find(|f| f.name == "aligned_choice").unwrap().quadding, quadding);
    h.execute("edit.undo");
    h.run_steps(3);
    assert_eq!(field(&h).quadding, 2);
    h.execute("edit.redo");
    h.run_steps(3);
    assert_eq!(field(&h).quadding, quadding);
}

#[test]
fn options_tab_sets_flags_alignment_and_defaults() {
    use pdfcraft_engine::field_flags as ff;
    let mut h = harness();
    open_props(&mut h, "city");
    tab(&mut h, "Options");
    h.get_by_label("Check spelling").click();
    h.get_by_label("Password").click();
    h.run_steps(1);
    pick(&mut h, "Alignment:", "Right");
    retype(&mut h, "Default Value:", "Paris");
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let f = s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "city").cloned().unwrap();
    assert!(f.has(ff::PASSWORD) && f.has(ff::DO_NOT_SPELL_CHECK), "{:b}", f.flags);
    assert_eq!((f.quadding, f.default.clone()), (2, vec!["Paris".to_string()]));
    assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), Some("Change field properties"));
}

#[test]
fn the_fields_panel_orders_tabs_manually() {
    let mut h = harness();
    h.execute("form.prepare");
    h.set_option("panel", "fields").unwrap();
    h.run_steps(3);
    let order = |h: &Harness<'static, PdfCraftApp>| -> Vec<String> {
        let s = h.state();
        let form = &s.session.get(s.views[0].id).unwrap().form;
        let mut v: Vec<(usize, String)> = form.iter().filter_map(|f| f.widgets.iter().map(|w| w.tab).min().map(|t| (t, f.name.clone()))).collect();
        v.sort();
        v.into_iter().map(|x| x.1).collect()
    };
    let before = order(&h);
    assert!(before.len() >= 2);
    let second = before[1].clone();
    h.get_by_label(&format!("Earlier in tab order: {second}")).click();
    h.run_steps(3);
    let after = order(&h);
    assert_eq!(after[0], second, "{before:?} → {after:?}");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Set tab order"));
}

#[test]
fn tab_numbers_follow_fill_order_without_dirtying_the_document() {
    let mut h = harness();
    h.execute("form.prepare");
    h.run_steps(2);
    h.get_by_label("Show tab numbers").click();
    h.run_steps(3);

    let (order, rects, undo) = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let order = pdfcraft_ui_egui::forms_ui::tab_order(&doc.form, &doc.info);
        let rects = order
            .iter()
            .map(|(fi, wi)| {
                let field = doc.form.get(*fi).expect("the traversal field exists");
                pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, field, *wi).unwrap()
            })
            .collect::<Vec<_>>();
        (order, rects, doc.can_undo())
    };
    assert!(h.state().views[0].session.prepare.show_tab_numbers);
    assert_eq!(undo, None, "displaying the numbers does not edit the PDF");
    let size_widgets = order
        .iter()
        .filter(|(fi, _)| h.state().session.get(h.state().views[0].id).unwrap().form.get(*fi).is_some_and(|field| field.name == "size"))
        .count();
    assert_eq!(size_widgets, 2, "each radio widget has its own tab stop");

    let drawn: Vec<(String, egui::Pos2, egui::Rect, egui::Rect)> = h
        .output()
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => {
                Some((text.galley.text().to_owned(), text.visual_bounding_rect().center(), text.visual_bounding_rect(), clipped.clip_rect))
            }
            _ => None,
        })
        .collect();
    for (i, rect) in rects.iter().enumerate() {
        let expected = i.saturating_add(1).to_string();
        let badge_center = rect.min + egui::vec2(3.0 + 4.0 + 3.0, 3.0 + 2.0 + 5.0);
        let badge = drawn.iter().find(|(label, center, _, _)| label == &expected && center.distance(badge_center) < 8.0);
        let Some((_, _, text_rect, clip_rect)) = badge else {
            panic!("widget {:?} should show tab number {expected} near {:?}; drawn: {drawn:?}", order.get(i), rect.min);
        };
        assert!(clip_rect.contains_rect(*text_rect), "the whole tab number {expected} should be visible");
    }
}

#[test]
fn tab_badge_tracks_its_widget_during_a_drag() {
    let mut h = harness();
    h.execute("form.prepare");
    h.run_steps(2);
    h.get_by_label("Show tab numbers").click();
    h.run_steps(3);

    let (name, widget, rect) = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let order = pdfcraft_ui_egui::forms_ui::tab_order(&doc.form, &doc.info);
        let (fi, wi) = *order.first().expect("the form has a fillable widget");
        let field = doc.form.get(fi).expect("the first fillable field exists");
        (field.name.clone(), wi, pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, field, wi).unwrap())
    };
    let start = rect.center();
    let badge_before = text_center_near(&h, "1", rect.min + egui::vec2(10.0, 10.0)).expect("the first badge is painted");

    h.hover_at(start);
    h.run_steps(1);
    h.drag_at(start);
    h.run_steps(1);
    h.drop_at(start);
    h.run_steps(3);
    assert_eq!(h.state().views[0].session.prepare.selected.as_ref().map(|selected| (selected.0.as_str(), selected.1)), Some((name.as_str(), widget)));

    let delta = egui::vec2(24.0, 16.0);
    h.hover_at(start);
    h.run_steps(1);
    h.drag_at(start);
    h.run_steps(1);
    h.hover_at(start + delta);
    h.run_steps(3);

    let badge_during_drag = text_center_near(&h, "1", badge_before + delta).expect("the badge stays painted during the drag");
    assert!(badge_during_drag.distance(badge_before + delta) < 8.0, "badge {badge_before:?} moved to {badge_during_drag:?}");
    assert!(badge_during_drag.distance(badge_before) > 12.0, "the badge follows the dragged widget");
    h.drop_at(start + delta);
    h.run_steps(3);
}

#[test]
fn duplicating_a_field_onto_every_page() {
    let mut h = harness();
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::InsertBlankPage { at: 1, width: 300.0, height: 400.0 });
    h.execute("form.prepare");
    h.run_steps(3);
    let (n, p) = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == "city").unwrap();
        (doc.info.pages.len(), pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen").center())
    };
    assert!(n >= 2, "the fixture has {n} pages");
    h.hover_at(p);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
    h.get_by_label("Duplicate…").click();
    h.run_steps(2);
    h.get_by_role_and_label(Role::Dialog, "Duplicate Field");
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    let f = doc.form.iter().find(|f| f.name == "city").unwrap();
    let mut pages: Vec<usize> = f.widgets.iter().filter_map(|w| w.page).collect();
    pages.sort();
    assert_eq!(pages, (0..n).collect::<Vec<_>>());
    assert_eq!(doc.can_undo(), Some("Duplicate field"));
}

#[test]
fn aligning_distributing_and_sizing_several_fields() {
    use pdfcraft_ui_egui::prepare::{Arrange, arrange};
    let mut h = harness();
    for (name, x, y, w) in [("a", 20.0, 300.0, 60.0), ("b", 120.0, 280.0, 80.0), ("c", 260.0, 260.0, 30.0)] {
        let ctx_h = h.ctx.clone();
        h.state_mut().apply_edit(
            &ctx_h,
            pdfcraft_engine::Edit::AddField {
                page: 0,
                rect: [x, y, x + w, y + 20.0],
                kind: pdfcraft_engine::NewField::Text { multiline: false },
                name: Some(name.into()),
            },
        );
    }
    h.run_steps(2);
    let form = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(h.state().views[0].id).unwrap().form.as_ref().clone();
    let rect = |h: &Harness<'static, PdfCraftApp>, n: &str| form(h).iter().find(|f| f.name == n).unwrap().widgets[0].rect;
    let a = ("a".to_string(), 0);
    let others = vec![("b".to_string(), 0), ("c".to_string(), 0)];
    // Align tops with a.
    let e = arrange(&form(&h), &a, &others, Arrange::AlignTop).unwrap();
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, e);
    assert!(["b", "c"].iter().all(|n| rect(&h, n)[3] == 320.0));
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Align fields"));
    // Distribute horizontally: equal gaps between a, b and c.
    let e = arrange(&form(&h), &a, &others, Arrange::DistributeH).unwrap();
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, e);
    let (ra, rb, rc) = (rect(&h, "a"), rect(&h, "b"), rect(&h, "c"));
    assert!(((rb[0] - ra[2]) - (rc[0] - rb[2])).abs() < 1e-6, "{ra:?} {rb:?} {rc:?}");
    // Same width as a.
    let e = arrange(&form(&h), &a, &others, Arrange::SameWidth).unwrap();
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, e);
    assert!(["b", "c"].iter().all(|n| (rect(&h, n)[2] - rect(&h, n)[0] - 60.0).abs() < 1e-6));
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Match field sizes"));
    assert!(arrange(&form(&h), &a, &[("b".into(), 0)], Arrange::DistributeV).is_none(), "distributing needs three");
}

#[test]
fn deleting_several_selected_fields_and_detecting_fields_from_the_panel() {
    let mut h = harness();
    for (name, x) in [("a", 20.0), ("b", 120.0), ("c", 220.0)] {
        let ctx_h = h.ctx.clone();
        h.state_mut().apply_edit(
            &ctx_h,
            pdfcraft_engine::Edit::AddField {
                page: 0,
                rect: [x, 300.0, x + 60.0, 320.0],
                kind: pdfcraft_engine::NewField::Text { multiline: false },
                name: Some(name.into()),
            },
        );
    }
    assert!(h.execute("form.prepare"));
    h.run_steps(2);
    // #93: field detection is in the panel, not only run when a form has no fields.
    h.get_by_label("Detect form fields");
    // #95: Delete removes every selected field, as one undoable step.
    {
        let p = &mut h.state_mut().views[0].session.prepare;
        p.selected = Some(("a".into(), 0));
        p.also = vec![("b".into(), 0), ("c".into(), 0)];
    }
    h.run_steps(1);
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    let left = names(&h);
    assert!(!["a", "b", "c"].iter().any(|n| left.iter().any(|l| l == n)), "{left:?}");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Delete 3 fields"));
    assert!(h.state().views[0].session.prepare.selected.is_none() && h.state().views[0].session.prepare.also.is_empty());
}

#[test]
fn preview_fills_the_form_and_locked_fields_keep_their_properties() {
    let mut h = harness();
    assert!(h.execute("form.prepare"));
    h.run_steps(2);
    assert!(h.state().is_preparing());
    h.get_by_label("Preview").click();
    h.run_steps(2);
    assert!(!h.state().is_preparing(), "Preview fills the form");
    h.get_by_label("Edit fields").click();
    h.run_steps(2);
    assert!(h.state().is_preparing());
    // Lock a field: its properties are then greyed out until unlocked.
    open_props(&mut h, "city");
    h.get_by_label("Locked").click();
    h.run_steps(1);
    h.get_by_label("OK").click();
    h.run_steps(3);
    let locked = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "city").unwrap().locked()
    };
    assert!(locked(&h));
    open_props(&mut h, "city");
    h.get_by_label_contains("This field is locked");
    h.get_by_label("Locked").click();
    h.run_steps(1);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(!locked(&h));
}

#[test]
fn image_fields_take_a_picture_when_clicked() {
    // A picture to choose: the picker answers with it.
    let path = std::env::temp_dir().join(format!("pdfcraft-image-field-{}.png", std::process::id()));
    image::RgbImage::from_pixel(8, 4, image::Rgb([200, 30, 30])).save(&path).unwrap();
    let files = std::sync::Arc::new(craft::host::ScriptedFiles::answering([craft::host::Picked::Paths(vec![path])]));
    let mut h = harness_with_files(files);
    assert!(h.execute("form.add.image"));
    h.run_steps(2);
    let (a, b) = (at(&h, 40.0, 220.0), at(&h, 140.0, 290.0));
    drag(&mut h, a, b);
    assert_eq!(names(&h).last().map(String::as_str), Some("Image1"));
    // Fill it in as a reader would.
    h.get_by_label("Preview").click();
    h.run_steps(2);
    let c = at(&h, 90.0, 255.0);
    h.hover_at(c);
    h.run_steps(1);
    h.drag_at(c);
    h.run_steps(1);
    h.drop_at(c);
    h.run_steps(4);
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), Some("Set the image of Image1"));
}

/// Adds an action on the Actions tab: picks its trigger and kind, types its script or value, and
/// presses Add.
fn add_action(h: &mut Harness<'static, PdfCraftApp>, trigger: &str, kind: &str, text: &str) {
    pick(h, "Select Trigger:", trigger);
    pick(h, "Select Action:", kind);
    if kind == "Run a JavaScript" {
        retype_box(h, ACTION_SCRIPT, text);
    } else {
        retype(h, "Value:", text);
    }
    h.get_by_label("Add").click();
    h.run_steps(2);
}

#[test]
fn actions_tab_adds_and_removes_actions() {
    use pdfcraft_engine::{FieldAction, FieldTrigger};
    let mut h = harness();
    h.execute("form.prepare");
    h.run_steps(2);
    open_props(&mut h, "city");
    tab(&mut h, "Actions");
    h.get_by_label("Select Trigger:");
    h.get_by_label("No actions");
    add_action(&mut h, "On Blur", "Open a web link", "https://example.org");
    h.get_by_label("On Blur: Open a web link: https://example.org");
    add_action(&mut h, "Mouse Up", "Run a JavaScript", "app.alert('x')");
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let acts = s.session.get(s.views[0].id).unwrap().field_actions("city");
    assert_eq!(
        acts,
        [
            (FieldTrigger::MouseUp, FieldAction::JavaScript("app.alert('x')".into())),
            (FieldTrigger::OnBlur, FieldAction::Uri("https://example.org".into()))
        ]
    );
}

#[test]
fn actions_tab_edits_the_script_of_an_existing_action() {
    use pdfcraft_engine::{FieldAction, FieldTrigger};
    let mut h = harness();
    h.execute("form.prepare");
    h.run_steps(2);
    let script = "if (event.value > 10) {\n  app.alert('Too many');\n}";
    let actions = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().field_actions("city")
    };
    let open = |h: &mut Harness<'static, PdfCraftApp>| {
        open_props(h, "city");
        tab(h, "Actions");
    };
    // A script on Mouse Up and a link on Blur, saved with the field.
    open(&mut h);
    add_action(&mut h, "Mouse Up", "Run a JavaScript", script);
    add_action(&mut h, "On Blur", "Open a web link", "https://example.org");
    h.get_by_label("OK").click();
    h.run_steps(3);
    let saved = actions(&h);
    // Edit shows the script exactly as stored; Discard leaves everything as it was.
    open(&mut h);
    h.get_all_by_label("Edit").last().unwrap().click();
    h.run_steps(2);
    h.get_by_label("Edit Action");
    assert_eq!(boxes(&h, ACTION_SCRIPT), [script]);
    retype_box(&mut h, ACTION_SCRIPT, "app.alert('changed');");
    h.get_by_label("Discard").click();
    h.run_steps(2);
    h.get_by_label("Add an Action");
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(actions(&h), saved);
    // Update replaces that script only: same trigger, the link untouched.
    open(&mut h);
    h.get_all_by_label("Edit").last().unwrap().click();
    h.run_steps(2);
    retype_box(&mut h, ACTION_SCRIPT, "app.alert('changed');");
    h.get_by_label("Update").click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(
        actions(&h),
        [
            (FieldTrigger::MouseUp, FieldAction::JavaScript("app.alert('changed');".into())),
            (FieldTrigger::OnBlur, FieldAction::Uri("https://example.org".into()))
        ]
    );
}

/// One page with a push button "go": Mouse Up runs a script with a link chained after it, On Blur
/// runs another script.
fn chained_action_fixture() -> Vec<u8> {
    pdf_of(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (go) /Rect [20 300 100 320] /P 3 0 R /A 5 0 R /AA << /Bl << /S /JavaScript /JS (this.dirty = true;) >> >> >>",
        "<< /S /JavaScript /JS (app.alert\\(1\\);) /Next 6 0 R >>",
        "<< /S /URI /URI (https://example.org) >>",
    ])
}

/// A PDF of `objs`, numbered from 1, with the first as its catalog.
fn pdf_of(objs: &[&str]) -> Vec<u8> {
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

/// Two one-button radio groups placed apart on page 1: "Group1" (export `A`) and "Group2" (`B`).
fn two_radio_groups_fixture() -> Vec<u8> {
    let widget = |parent: u32, x: u32, state: &str| {
        format!(
            "<< /Type /Annot /Subtype /Widget /Parent {parent} 0 R /Rect [{x} 300 {} 314] /P 3 0 R /AS /Off /AP << /N << /{state} 8 0 R /Off 8 0 R >> >> >>",
            x + 14
        )
    };
    let (a, b) = (widget(4, 40, "A"), widget(6, 120, "B"));
    pdf_of(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 6 0 R] >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [5 0 R 7 0 R] >>",
        "<< /FT /Btn /Ff 49152 /T (Group1) /Kids [5 0 R] >>",
        &a,
        "<< /FT /Btn /Ff 49152 /T (Group2) /Kids [7 0 R] >>",
        &b,
        "<< /Length 3 >>\nstream\n0 g\nendstream",
    ])
}

/// #4949: renaming a radio button to another group's name moves it into that group, after the
/// group's own buttons. The moved button stays the selected one, so a nudge moves it and not the
/// group's first button, and undo and redo take the rename and the nudge back and forth.
#[test]
fn a_radio_button_joining_a_group_by_name_stays_selected() {
    let mut h = harness_with("radios.pdf", two_radio_groups_fixture());
    h.execute("form.prepare");
    h.run_steps(2);
    let rects = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        let form = &s.session.get(s.views[0].id).unwrap().form;
        form.iter().flat_map(|f| f.widgets.iter().map(|w| w.rect)).collect::<Vec<_>>()
    };
    let start = rects(&h);
    open_props(&mut h, "Group2");
    retype(&mut h, "Name:", "Group1");
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(names(&h), ["Group1"]);
    assert_eq!(h.state().views[0].session.prepare.selected, Some(("Group1".to_string(), 1)), "the moved button");
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(2);
    let nudged = rects(&h);
    assert_eq!(nudged[0], start[0], "the group's own button stays");
    assert!((nudged[1][0] - start[1][0] - 1.0).abs() < 0.01, "{:?} → {:?}", start[1], nudged[1]);
    h.execute("edit.undo");
    h.execute("edit.undo");
    h.run_steps(2);
    assert_eq!(names(&h), ["Group1", "Group2"]);
    assert_eq!(rects(&h), start);
    h.execute("edit.redo");
    h.execute("edit.redo");
    h.run_steps(2);
    assert_eq!(names(&h), ["Group1"]);
    assert_eq!(rects(&h), nudged);
}

#[test]
fn actions_tab_shows_a_chain_and_keeps_it_when_the_first_action_is_replaced() {
    use egui_kittest::kittest::NodeT;
    let mut h = harness_with("chain.pdf", chained_action_fixture());
    h.execute("form.prepare");
    h.run_steps(2);
    let open = |h: &mut Harness<'static, PdfCraftApp>| {
        open_props(h, "go");
        tab(h, "Actions");
    };
    open(&mut h);
    h.get_by_label("Mouse Up: Run a JavaScript, then 1 more action");
    // The menu bar's Edit menu and the Edit tab come first; the rows' follow (Mouse Up, On Blur).
    assert_eq!(h.get_all_by_label("Edit").count(), 4);
    h.get_all_by_label("Edit").nth(2).unwrap().click();
    h.run_steps(2);
    h.get_by_label("Edit Action");
    // While one script is open, no other Edit can overwrite it.
    assert!(h.get_all_by_label("Edit").skip(2).all(|n| n.accesskit_node().is_disabled()));
    h.get_by_label("Discard").click();
    h.run_steps(2);
    assert!(h.get_all_by_label("Edit").skip(2).all(|n| !n.accesskit_node().is_disabled()));
    // Replace the Mouse Up script with a link: the chained link stays after it.
    add_action(&mut h, "Mouse Up", "Open a web link", "https://example.com");
    h.get_by_label("OK").click();
    h.run_steps(3);
    open(&mut h);
    h.get_by_label("Mouse Up: Open a web link: https://example.com, then 1 more action");
}

/// Text field table: pick the tool and drag out an area in the empty lower part of page 1, 200 by
/// 120 points from (40, 240) as displayed. The table's window opens for it.
fn draw_table(h: &mut Harness<'static, PdfCraftApp>) {
    assert!(h.execute("form.add.text_table"));
    h.run_steps(2);
    let (a, b) = (at(h, 40.0, 240.0), at(h, 240.0, 360.0));
    drag(h, a, b);
    assert!(table_open(h));
}

#[test]
fn a_text_field_table_is_drawn_set_up_and_added_in_one_step() {
    let mut h = harness();
    let before = names(&h);
    draw_table(&mut h);
    assert_eq!(h.state().quick_tool, QuickTool::Select, "back to Select once the area is drawn");
    // The table starts with four rows and one column: two rows fewer and one column more, by the
    // number fields' arrows.
    let step = |h: &mut Harness<'static, PdfCraftApp>, label: &str, key: egui::Key, times: usize| {
        h.get_by_role_and_label(Role::SpinButton, label).focus();
        h.run_steps(1);
        for _ in 0..times {
            h.key_press(key);
            h.run_steps(2);
        }
    };
    step(&mut h, "Rows", egui::Key::ArrowDown, 2);
    step(&mut h, "Columns", egui::Key::ArrowUp, 1);
    retype(&mut h, "Column 1", "desc_N");
    retype(&mut h, "Column 2", "price_N");
    assert_eq!(table(&h)["rows"], 2);
    h.get_by_label("Add fields").click();
    h.run_steps(3);
    assert!(!table_open(&h), "added, the window closes");
    let added: Vec<_> = names(&h).into_iter().filter(|n| !before.contains(n)).collect();
    assert_eq!(added, ["desc_1", "price_1", "desc_2", "price_2"]);
    // Two equal columns of 100 points, 60 high, the first row at the top: user space has y up from
    // 400, and the area is the 200 by 120 points drawn from (40, 240) as displayed.
    let r = rect_of(&h, "desc_1");
    assert!([40.0, 100.0, 140.0, 160.0].iter().zip(r).all(|(w, g)| (w - g).abs() < 0.5), "{r:?}");
    let r = rect_of(&h, "price_2");
    assert!([140.0, 40.0, 240.0, 100.0].iter().zip(r).all(|(w, g)| (w - g).abs() < 0.5), "{r:?}");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Add text field table"));
    h.execute("edit.undo");
    h.run_steps(2);
    assert_eq!(names(&h), before, "one undo takes the whole table back");
}

#[test]
fn a_table_whose_names_are_taken_stays_open_saying_so() {
    let mut h = harness();
    draw_table(&mut h);
    h.get_by_label("Add fields").click();
    h.run_steps(3);
    assert!(names(&h).contains(&"Table1_4".to_owned()), "{:?}", names(&h));
    // The next table takes the next free names on its own.
    draw_table(&mut h);
    assert_eq!(text_of(&h, "Column 1").as_deref(), Some("Table2_N"));
    // Named like the first, it can't be added: the window says why and keeps what was set up.
    retype(&mut h, "Column 1", "Table1_N");
    // Tab leaves the box, so the Escape below is the window's and not the box's.
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
    let count = names(&h).len();
    h.get_by_label("a field named \"Table1_1\" already exists");
    h.get_by_label("Add fields").click();
    h.run_steps(3);
    assert_eq!(names(&h).len(), count, "nothing added");
    assert!(table_open(&h), "still open");
    // Escape closes it, adding nothing.
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(!table_open(&h));
    assert_eq!(names(&h).len(), count);
}

/// Enter commits a number being typed. It used to add the table there and then, before the rest
/// of it was set up.
#[test]
fn enter_in_a_number_field_does_not_add_the_table() {
    let mut h = harness();
    let before = names(&h).len();
    draw_table(&mut h);
    h.get_by_role_and_label(Role::SpinButton, "Rows").focus();
    h.run_steps(2);
    h.key_press(egui::Key::Backspace);
    h.event(egui::Event::Text("6".into()));
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(names(&h).len(), before, "nothing added");
    assert!(table_open(&h), "the window is open");
    assert_eq!(table(&h)["rows"], 6, "the number is in");
}

#[test]
fn the_table_window_closes_when_its_document_is_left() {
    let mut h = harness();
    let blank = pdfcraft_engine::Session::new().create_blank(300.0, 400.0, 1).unwrap().to_vec();
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "other.pdf", None, blank).unwrap();
    h.state_mut().active = Some(0);
    h.run_steps(4);
    let before = names(&h).len();
    draw_table(&mut h);
    // The other document's tab is still in reach of the pointer: choosing it closes the window.
    let tab = h.get_all_by_label("other.pdf").next().map(|n| n.rect()).expect("its tab");
    h.hover_at(tab.center());
    h.run_steps(1);
    h.drag_at(tab.center());
    h.run_steps(1);
    h.drop_at(tab.center());
    h.run_steps(3);
    assert_eq!(h.state().active, Some(1));
    assert!(!table_open(&h));
    assert!(h.query_by_label("Text Field Table").is_none(), "the window's title is no longer shown");
    h.state_mut().active = Some(0);
    h.run_steps(2);
    assert_eq!(names(&h).len(), before, "nothing added to either document");
    // So does leaving Prepare a form.
    draw_table(&mut h);
    h.state_mut().set_left_open(false);
    h.run_steps(2);
    assert!(!table_open(&h));
}

/// With its panel closed after the tool was picked, the form is still prepared and a table can be
/// drawn: its window opens Prepare a form again, which it belongs to. Drawing sends the tool back
/// to Select, so the window used to close itself at once, nothing being prepared any more.
#[test]
fn a_table_drawn_with_the_panel_closed_opens_it_again() {
    let mut h = harness();
    let before = names(&h).len();
    assert!(h.execute("form.add.text_table"));
    h.run_steps(2);
    h.state_mut().set_left_open(false);
    h.run_steps(2);
    assert!(h.state().is_preparing(), "the picked tool keeps the form prepared");
    let (a, b) = (at(&h, 40.0, 240.0), at(&h, 240.0, 360.0));
    drag(&mut h, a, b);
    h.run_steps(3);
    assert!(table_open(&h), "the window stays open");
    assert!(h.state().left_open() && h.state().is_preparing(), "Prepare a form is open again");
    h.get_by_label("Add fields").click();
    h.run_steps(3);
    assert!(names(&h).len() > before, "the table is added");
}

/// The window belongs to the page as it was when it opened: rotated (or moved, deleted, undone)
/// under it, the page would put the area the table was set up for somewhere else.
#[test]
fn the_table_window_closes_when_the_document_changes_under_it() {
    let mut h = harness();
    let before = names(&h);
    draw_table(&mut h);
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
    h.run_steps(2);
    assert!(!table_open(&h), "closed by the rotation");
    assert_eq!(names(&h), before, "nothing added");
}

/// Escape in a name field leaves the field and takes back what was typed, as it does in any field;
/// it closed the window too, and the column setup with it. A second Escape closes the window.
#[test]
fn escape_in_a_name_field_leaves_the_field_not_the_table() {
    let mut h = harness();
    draw_table(&mut h);
    // The app's own text boxes (zoom, page number) are inputs too: take the one in the window.
    let field = h.get_by_role_and_label(Role::TextInput, "Column 1").rect();
    h.hover_at(field.center());
    h.drag_at(field.center());
    h.run_steps(1);
    h.drop_at(field.center());
    h.run_steps(2);
    h.event(egui::Event::Text("x".into()));
    h.run_steps(1);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(table_open(&h), "still open");
    assert_eq!(text_of(&h, "Column 1").as_deref(), Some("Table1_N"), "what was typed is taken back");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(!table_open(&h), "a second Escape closes it");
}

/// A click puts the default 180 × 110 table's corner at the pointer; near the page's edges it is
/// moved in so that all of it is on the page (here 5 pt from the right, 100 pt from the bottom).
#[test]
fn a_table_clicked_near_the_page_edge_stays_on_the_page() {
    let mut h = harness();
    assert!(h.execute("form.add.text_table"));
    h.run_steps(2);
    let p = at(&h, 295.0, 300.0);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
    assert!(table_open(&h), "the table's window");
    // The table's cells fill its area, so the added fields together are the area.
    let before = names(&h);
    h.get_by_label("Add fields").click();
    h.run_steps(3);
    let rects: Vec<[f64; 4]> = names(&h).iter().filter(|n| !before.contains(n)).map(|n| rect_of(&h, n)).collect();
    assert!(!rects.is_empty(), "the table was added");
    let r = rects.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]);
    // In user space, y up from the bottom of the 300 × 400 page.
    assert!([120.0, 0.0, 300.0, 110.0].iter().zip(r).all(|(w, g)| (w - g).abs() < 0.5), "on the page: {r:?}");
}

/// Prepare a form ▸ Calculation order opens as a page of its own, with the form still being
/// prepared, says when nothing calculates, and Back returns to Prepare a form.
#[test]
fn the_calculation_order_is_a_page_of_prepare_a_form() {
    let mut h = harness();
    h.execute("form.prepare");
    h.run_steps(3);
    assert!(h.query_by_label("Field list").is_some());
    h.get_by_label("Calculation order").click();
    h.run_steps(3);
    assert!(h.state().is_preparing(), "the fields stay editable, so a row can select one");
    // The page replaces the Prepare a form tools.
    assert!(h.query_by_label("Field list").is_none());
    assert!(h.query_by_label_contains("No field in this form calculates its value yet").is_some());
    h.get_by_label("Back to Prepare a form").click();
    h.run_steps(3);
    assert!(h.query_by_label("Field list").is_some());
    assert!(h.query_by_label_contains("No field in this form calculates its value yet").is_none());
    assert!(h.query_by_label("Back to Prepare a form").is_none());
    // Opened from Preview, the page edits the fields again.
    h.get_by_label("Preview").click();
    h.run_steps(3);
    assert!(h.state().form_preview);
    h.get_by_label("Calculation order").click();
    h.run_steps(3);
    assert!(h.state().is_preparing() && !h.state().form_preview, "Preview ends, so a row can select its field");
    // Opened (from ⌘K) while the page grid shows, the grid closes and the page shows.
    h.execute("page.organize");
    h.run_steps(3);
    assert!(h.state().views[0].organize);
    h.execute("form.calculation_order");
    h.run_steps(3);
    assert!(!h.state().views[0].organize);
    assert!(h.query_by_label_contains("No field in this form calculates its value yet").is_some());
}

/// Prepare a form ▸ Calculation order: a card says which field shows an old value and why, each
/// row shows its field's value as the field does, the selected field's arrows, Alt+↑ and a drag
/// move it, Fix order puts each field after the fields it reads and the card's Undo takes that
/// back, every change is one undo step, and a row selects its field.
#[test]
fn the_calculation_order_is_moved_dragged_and_fixed() {
    use egui::{Key, Modifiers};
    use pdfcraft_engine::form_scripts::{CalcOp, Calculate, Format};
    use pdfcraft_engine::{Edit, FieldProps, FieldValue, NewField};
    let mut h = harness();
    for (i, name) in ["qty", "sub", "total", "other"].into_iter().enumerate() {
        let y = 200.0 + 30.0 * i as f64;
        let kind = NewField::Text { multiline: false };
        let ctx_h = h.ctx.clone();
        h.state_mut().apply_edit(&ctx_h, Edit::AddField { page: 0, rect: [20.0, y, 120.0, y + 20.0], kind, name: Some(name.into()) });
    }
    let calculate = |name: &str, c: Calculate| Edit::SetFieldProps {
        name: name.into(),
        props: Box::new(FieldProps { calculate: Some(c), ..Default::default() }),
    };
    // Defined backwards: total, which reads sub, calculates first.
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, calculate("total", Calculate::Simple { op: CalcOp::Sum, fields: vec!["sub".into()] }));
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, calculate("sub", Calculate::Notation("qty * 2".into())));
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, calculate("other", Calculate::Notation("qty + 1".into())));
    // other shows its value as currency.
    let currency = Format::Number { decimals: 2, sep: 0, neg: 0, currency: "$".into(), prepend: true };
    h.state_mut().apply_edit(
        &egui::Context::default(),
        Edit::SetFieldProps { name: "other".into(), props: Box::new(FieldProps { format: Some(currency), ..Default::default() }) },
    );
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, Edit::SetFieldValue { name: "qty".into(), value: FieldValue::Text("3".into()) });
    h.execute("form.prepare");
    h.run_steps(3);
    h.get_by_label("Calculation order").click();
    h.run_steps(3);
    let order = |h: &Harness<'static, PdfCraftApp>| -> Vec<String> {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().calculation_order().fields.into_iter().map(|f| f.name).collect()
    };
    assert_eq!(order(&h), ["total", "sub", "other"]);
    // The card says what is wrong, and each row shows its value as the field does: total
    // calculated before sub, so it still has sub's old value.
    assert!(card_says(&h, "1 field shows an old value"));
    assert!(card_says(&h, "total calculates before sub, which it reads."));
    for row in ["total: 0", "sub: 6", "other: $4.00"] {
        assert!(h.query_by_label(row).is_some(), "{row}");
    }
    // The selected field's arrows move it.
    assert!(h.query_by_label("Later in calculation order: total").is_none(), "arrows only on the selected field or the one pointed at");
    order_row(&h, "total").click();
    h.run_steps(3);
    h.get_by_label("Later in calculation order: total").click();
    h.run_steps(3);
    assert_eq!(order(&h), ["sub", "total", "other"]);
    assert_eq!(undo_label(&h).as_deref(), Some("Set calculation order"));
    // Alt+↑ (Option on macOS) moves it back, as one more undo step.
    h.key_press_modifiers(Modifiers::ALT, Key::ArrowUp);
    h.run_steps(3);
    assert_eq!(order(&h), ["total", "sub", "other"]);
    assert_eq!(undo_label(&h).as_deref(), Some("Set calculation order"));
    h.execute("edit.undo");
    h.run_steps(3);
    assert_eq!(order(&h), ["sub", "total", "other"], "the key press was one undo step");
    h.execute("edit.redo");
    h.run_steps(3);
    assert_eq!(order(&h), ["total", "sub", "other"]);
    // Dragging other onto the first row's upper half puts it first.
    let from = order_row(&h, "other").rect().center();
    let first = order_row(&h, "total").rect();
    let to = first.center() - egui::vec2(0.0, first.height() / 4.0);
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(from + (to - from) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(4);
    assert_eq!(order(&h), ["other", "total", "sub"]);
    assert_eq!(undo_label(&h).as_deref(), Some("Set calculation order"));
    // Fix order moves only total, after sub, and the card says the order is right.
    h.get_by_label("Fix order").click();
    h.run_steps(3);
    assert_eq!(order(&h), ["other", "sub", "total"]);
    assert_eq!(undo_label(&h).as_deref(), Some("Set calculation order"));
    assert!(h.query_by_label("Every field calculates after the fields it reads.").is_some());
    assert!(h.query_by_label("Fix order").is_none());
    // Its Undo takes Fix order back.
    h.get_by_label("Undo").click();
    h.run_steps(3);
    assert_eq!(order(&h), ["other", "total", "sub"]);
    // A row selects its field.
    order_row(&h, "sub").click();
    h.run_steps(2);
    assert_eq!(h.state().views[0].session.prepare.selected.as_ref().map(|s| s.0.as_str()), Some("sub"));
}

/// Fields that read each other get a card of their own, which Fix order leaves alone, and its
/// link opens the first one's formula.
#[test]
fn a_loop_has_its_own_card_that_opens_the_formula() {
    use pdfcraft_engine::form_scripts::Calculate;
    use pdfcraft_engine::{Edit, FieldProps, NewField};
    let mut h = harness();
    for (i, (name, formula)) in [("a", "b + 1"), ("b", "a + 1")].into_iter().enumerate() {
        let y = 200.0 + 30.0 * i as f64;
        let kind = NewField::Text { multiline: false };
        let ctx_h = h.ctx.clone();
        h.state_mut().apply_edit(&ctx_h, Edit::AddField { page: 0, rect: [20.0, y, 120.0, y + 20.0], kind, name: Some(name.into()) });
        let props = Box::new(FieldProps { calculate: Some(Calculate::Notation(formula.into())), ..Default::default() });
        let ctx_h = h.ctx.clone();
        h.state_mut().apply_edit(&ctx_h, Edit::SetFieldProps { name: name.into(), props });
    }
    h.execute("form.prepare");
    h.run_steps(3);
    h.get_by_label("Calculation order").click();
    h.run_steps(3);
    assert!(card_says(&h, "a and b read each other"));
    assert!(h.query_by_label("Every field outside a loop calculates after the fields it reads.").is_some());
    assert!(h.query_by_label("Fix order").is_none(), "no order fixes a loop");
    h.get_by_label("Edit a's formula").click();
    h.run_steps(3);
    // Field Properties of a is open on the Calculate tab, which shows a's formula.
    assert!(props_open(&h));
    assert_eq!(boxes(&h, NOTATION), ["b + 1"]);
}

/// A radio group `shirt` of three buttons, S, M and L, with L chosen, while preparing the form.
fn radio_group_harness() -> Harness<'static, PdfCraftApp> {
    let mut h = harness();
    for (x, export) in [(20.0, "S"), (60.0, "M"), (100.0, "L")] {
        let ctx_h = h.ctx.clone();
        h.state_mut().apply_edit(
            &ctx_h,
            pdfcraft_engine::Edit::AddField {
                page: 0,
                rect: [x, 300.0, x + 14.0, 314.0],
                kind: pdfcraft_engine::NewField::Radio { group: Some("shirt".into()), export: export.into() },
                name: Some("shirt".into()),
            },
        );
    }
    h.state_mut().apply_edit(
        &egui::Context::default(),
        pdfcraft_engine::Edit::SetFieldValue { name: "shirt".into(), value: pdfcraft_engine::FieldValue::Radio(Some("L".into())) },
    );
    assert!(h.execute("form.prepare"));
    h.run_steps(3);
    h
}

fn buttons(h: &Harness<'static, PdfCraftApp>) -> (Vec<String>, Vec<String>) {
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    let f = doc.form.iter().find(|f| f.name == "shirt").expect("the group is still there");
    (f.widgets.iter().filter_map(|w| w.on_state.clone()).collect(), f.value.clone())
}

/// #4609: Delete removes the one radio button clicked, not its whole group, as in Acrobat.
#[test]
fn deleting_one_radio_button_keeps_the_rest_of_its_group() {
    let mut h = radio_group_harness();
    assert_eq!(buttons(&h).0, ["S", "M", "L"]);
    let p = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == "shirt").unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 1).expect("on screen").center()
    };
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
    assert_eq!(h.state().views[0].session.prepare.selected, Some(("shirt".into(), 1)), "the middle button is selected");
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert_eq!(buttons(&h), (vec!["S".into(), "L".into()], vec!["L".into()]), "only M is gone; L stays chosen");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Delete field"));
    h.execute("edit.undo");
    h.run_steps(2);
    assert_eq!(buttons(&h).0, ["S", "M", "L"]);
}

/// Two buttons of one group selected together both go, whichever was selected first.
#[test]
fn deleting_two_radio_buttons_of_a_group_at_once() {
    let mut h = radio_group_harness();
    {
        let p = &mut h.state_mut().views[0].session.prepare;
        p.selected = Some(("shirt".into(), 0));
        p.also = vec![("shirt".into(), 2)];
    }
    h.run_steps(1);
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert_eq!(buttons(&h), (vec!["M".into()], vec![]), "M is left, and nothing is chosen");
    // Selecting every button deletes the group.
    {
        let p = &mut h.state_mut().views[0].session.prepare;
        p.selected = Some(("shirt".into(), 0));
    }
    h.run_steps(1);
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert!(!names(&h).iter().any(|n| n == "shirt"));
}

/// Two calculated fields that share a name (a malformed form) each show their own formula: the
/// first dup sums src, the second x.
#[test]
fn fields_sharing_a_name_each_show_their_own_formula() {
    let widget = |name: &str, y: u32, reads: &str| {
        format!(
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name}) /Rect [50 {y} 250 {}] /P 3 0 R /AA << /C << /S /JavaScript /JS (AFSimple_Calculate\\(\"SUM\", \"{reads}\"\\);) >> >> >>",
            y + 20
        )
    };
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [5 0 R 6 0 R 7 0 R 8 0 R] >>".into(),
        "<< /Fields [5 0 R 6 0 R 7 0 R 8 0 R] /DA (/Helv 0 Tf 0 g) /CO [6 0 R 7 0 R 5 0 R] >>".into(),
        widget("dup", 700, "x"),
        widget("dup", 660, "src"),
        widget("src", 620, "x"),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (x) /Rect [50 580 250 600] /P 3 0 R /V (3) >>".into(),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        bytes.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "dup.pdf", None, bytes.clone()).unwrap();
            app
        }
    });
    h.run_steps(4);
    h.execute("form.prepare");
    h.run_steps(3);
    h.get_by_label("Calculation order").click();
    h.run_steps(3);
    // The two dup rows, first and third in the order: rows say their field's name and value, the
    // number is the row's lead.
    for (row, formula) in [(0, "= sum(src)"), (1, "= sum(x)")] {
        h.query_all_by(|node| node.label().is_some_and(|label| label == "dup" || label.starts_with("dup: "))).nth(row).expect("a dup row").hover();
        h.run_steps(30);
        assert!(
            h.query_by(|node| node.role() == Role::Tooltip && node.label().is_some_and(|label| label.contains(formula))).is_some(),
            "dup row {row} shows {formula}"
        );
    }
}

/// Field Properties refuses a notation it couldn't store as written, and stays open with what was
/// typed into it: the field keeps its formula and nothing in the dialog is lost. Once the notation
/// is fixed, OK applies it and closes.
#[test]
fn a_notation_that_cannot_be_stored_keeps_field_properties_open() {
    use pdfcraft_engine::form_scripts::Calculate;
    use pdfcraft_engine::{Edit, FieldProps, NewField};
    let mut h = harness();
    for (i, name) in ["qty", "sub"].into_iter().enumerate() {
        let y = 200.0 + 30.0 * i as f64;
        let kind = NewField::Text { multiline: false };
        let ctx_h = h.ctx.clone();
        h.state_mut().apply_edit(&ctx_h, Edit::AddField { page: 0, rect: [20.0, y, 120.0, y + 20.0], kind, name: Some(name.into()) });
    }
    let props = Box::new(FieldProps { calculate: Some(Calculate::Notation("qty * 2".into())), ..Default::default() });
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, Edit::SetFieldProps { name: "sub".into(), props });
    let formula = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "sub").unwrap().actions.calculate.clone()
    };
    open_props(&mut h, "sub");
    assert!(props_open(&h), "Field Properties is open");
    retype(&mut h, "Tooltip:", "Subtotal");
    tab(&mut h, "Calculate");
    retype_box(&mut h, NOTATION, "qty EVCALC 2");
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(props_open(&h), "the dialog stays open");
    assert_eq!(boxes(&h, NOTATION), ["qty EVCALC 2"], "as typed");
    tab(&mut h, "General");
    assert_eq!(text_of(&h, "Tooltip:").as_deref(), Some("Subtotal"), "as typed");
    assert_eq!(formula(&h), Calculate::Notation("qty * 2".into()), "the field keeps its formula");
    tab(&mut h, "Calculate");
    retype_box(&mut h, NOTATION, "qty * 3");
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(!props_open(&h), "applied and closed");
    assert_eq!(formula(&h), Calculate::Notation("qty * 3".into()));
}

/// A formula that reads a field the form doesn't have gets a card of its own, in place of "every
/// field calculates after the fields it reads", and its link opens that formula. A script that
/// builds the names it reads gets a note.
#[test]
fn a_formula_reading_a_missing_field_has_its_own_card() {
    use pdfcraft_engine::form_scripts::Calculate;
    use pdfcraft_engine::{Edit, FieldProps, NewField};
    let mut h = harness();
    for (i, name) in ["qty", "discount", "sub", "rows"].into_iter().enumerate() {
        let y = 200.0 + 30.0 * i as f64;
        let kind = NewField::Text { multiline: false };
        let ctx_h = h.ctx.clone();
        h.state_mut().apply_edit(&ctx_h, Edit::AddField { page: 0, rect: [20.0, y, 120.0, y + 20.0], kind, name: Some(name.into()) });
    }
    let props = Box::new(FieldProps { calculate: Some(Calculate::Notation("qty - discount".into())), ..Default::default() });
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, Edit::SetFieldProps { name: "sub".into(), props });
    let script = Some("event.value = getField(\"row\" + 1).value;".to_owned());
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, Edit::SetFieldScript { name: "rows".into(), event: "calculate".into(), script });
    h.execute("form.prepare");
    h.run_steps(3);
    h.get_by_label("Calculation order").click();
    h.run_steps(3);
    assert!(h.query_by_label("Every field calculates after the fields it reads.").is_some(), "nothing is missing yet");
    assert!(h.query_by_label("rows builds some field names as it runs, so those reads aren't checked.").is_some());
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, Edit::DeleteField { name: "discount".into() });
    h.run_steps(3);
    assert!(card_says(&h, "1 formula reads a field the form doesn't have"));
    assert!(card_says(&h, "sub reads discount, which the form doesn't have."));
    assert!(h.query_by_label("Every field calculates after the fields it reads.").is_none(), "no all-clear while a formula is broken");
    // Reordered, the order can be undone from the missing-field card; the all-clear still stays away.
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, Edit::SetCalculationOrder { names: vec!["rows".into(), "sub".into()] });
    h.run_steps(3);
    assert!(h.query_by_label("Every field calculates after the fields it reads.").is_none(), "still no all-clear");
    h.get_by_label("Undo").click();
    h.run_steps(3);
    let order: Vec<String> = {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().calculation_order().fields.into_iter().map(|f| f.name).collect()
    };
    assert_eq!(order, ["sub", "rows"], "Undo put the order back");
    h.get_by_label("Edit sub's formula").click();
    h.run_steps(3);
    // Field Properties of sub is open on the Calculate tab, which shows sub's formula.
    assert!(props_open(&h));
    assert_eq!(boxes(&h, NOTATION), ["qty - discount"]);
}

#[test]
fn button_export_properties_apply_cancel_and_undo() {
    let mut h = harness();
    h.execute("form.prepare");
    h.run_steps(2);
    open_props(&mut h, "agree");
    tab(&mut h, "Options");
    let original = text_of(&h, "Export value").expect("the export value");
    retype(&mut h, "Export value", "Accepted");
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    open_props(&mut h, "agree");
    tab(&mut h, "Options");
    assert_eq!(text_of(&h, "Export value").as_deref(), Some(original.as_str()));
    retype(&mut h, "Export value", "Accepted");
    h.get_by_label("OK").click();
    h.run_steps(3);
    {
        let s = h.state();
        let f = s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "agree").unwrap();
        assert_eq!(f.export_of(0), Some("Accepted"));
    }
    h.execute("edit.undo");
    h.run_steps(3);
    {
        let s = h.state();
        let f = s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "agree").unwrap();
        assert_eq!(f.export_of(0), Some(original.as_str()));
    }
}

/// The screen rectangle of widget 0 of field `name` on the first view.
fn field_rect(h: &Harness<'static, PdfCraftApp>, name: &str) -> egui::Rect {
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    let f = doc.form.iter().find(|f| f.name == name).unwrap();
    pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen")
}

/// A right-click while a field is dragged leaves the dragged field selected, so the drag moves it
/// and not the field right-clicked. The right-click used to select the other field, and the release
/// moved that one.
#[test]
fn a_right_click_during_a_field_drag_moves_the_dragged_field() {
    let mut h = harness();
    h.execute("form.prepare");
    h.run_steps(3);
    let (name, city) = (field_rect(&h, "name"), field_rect(&h, "city"));
    let a = name.center();
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    h.hover_at(a + egui::vec2(0.0, 30.0));
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: city.center(), button: egui::PointerButton::Secondary, pressed, modifiers: Default::default() });
    }
    h.run_steps(1);
    let end = a + egui::vec2(0.0, 60.0);
    h.hover_at(end);
    h.run_steps(1);
    h.drop_at(end);
    h.run_steps(3);
    assert_eq!(field_rect(&h, "city"), city, "the field right-clicked stays where it was");
    assert_ne!(field_rect(&h, "name"), name, "the dragged field moved");
}

/// With Prepare open, a drag with a shape tool on the page's empty part draws Prepare's selection
/// rectangle and no comment, as before: Prepare takes the press nobody holds. It used to draw both.
#[test]
fn with_prepare_open_a_shape_tool_drag_draws_no_comment() {
    let mut h = harness();
    h.execute("form.prepare");
    h.run_steps(3);
    assert!(h.execute("comment.square"));
    h.run_steps(3);
    assert!(h.state().is_preparing(), "picking the tool leaves Prepare open");
    // Below the fields.
    let (from, to) = (at(&h, 40.0, 260.0), at(&h, 200.0, 360.0));
    drag(&mut h, from, to);
    let s = h.state();
    let squares = s.session.get(s.views[0].id).unwrap().info.annotations.iter().filter(|a| a.subtype == "Square").count();
    assert_eq!(squares, 0, "no rectangle under Prepare's selection rectangle");
}

/// With Prepare open, a double-click on a field is the field's (it opens its properties) and leaves
/// a polygon being clicked out unfinished, as before. It used to finish the polygon too.
#[test]
fn with_prepare_open_a_double_click_on_a_field_leaves_the_polygon_unfinished() {
    let mut h = harness();
    h.execute("form.prepare");
    h.run_steps(3);
    assert!(h.execute("comment.polygon"));
    h.run_steps(3);
    // Three points below the fields.
    for (x, y) in [(40.0, 260.0), (200.0, 260.0), (120.0, 360.0)] {
        let p = at(&h, x, y);
        h.hover_at(p);
        h.run_steps(1);
        h.drag_at(p);
        h.run_steps(1);
        h.drop_at(p);
        h.run_steps(3);
    }
    let on_field = field_rect(&h, "name").center();
    crate::support::input::double_click(&mut h, on_field);
    let s = h.state();
    let polygons = s.session.get(s.views[0].id).unwrap().info.annotations.iter().filter(|a| a.subtype == "Polygon").count();
    assert_eq!(polygons, 0, "the polygon is not finished");
    assert!(!s.views[0].session.comments.tool.drawing().is_empty(), "it is still being clicked out");
}
