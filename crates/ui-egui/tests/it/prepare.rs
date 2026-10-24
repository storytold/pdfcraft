//! Prepare a form in the real shell (egui_kittest): field tools, placing, selecting, moving,
//! Field Properties and deleting.

use egui::Pos2;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{Queryable, by};
use pdfcraft_engine::field_table::TableColumn;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp, QuickTool};

fn harness() -> Harness<'static, PdfCraftApp> {
    harness_with("form.pdf", include_bytes!("../data/form.pdf").to_vec())
}

fn harness_with(name: &'static str, bytes: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        cc.egui_ctx.options_mut(|options| options.warn_on_id_clash = true);
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes(name, None, bytes).unwrap();
        app.set_option("zoom", "150").unwrap();
        app
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

#[test]
fn placing_moving_editing_and_deleting_a_field() {
    let mut h = harness();
    let before = names(&h).len();
    assert!(h.state_mut().execute("form.add.text"));
    h.run_steps(2);
    h.get_by_label("Prepare a form");
    // Drag out a text field in the empty lower part of the page.
    let (a, b) = (at(&h, 40.0, 300.0), at(&h, 200.0, 322.0));
    drag(&mut h, a, b);
    let all = names(&h);
    assert_eq!(all.len(), before + 1, "{all:?}");
    assert_eq!(all.last().map(String::as_str), Some("Text1"));
    assert_eq!(h.state().quick_tool, QuickTool::Select, "back to Select after one field");
    h.run_steps(2);
    assert_eq!(h.state().views[0].prepare.selected.as_ref().map(|s| s.0.as_str()), Some("Text1"), "the new field is selected");
    let r = rect_of(&h, "Text1");
    assert!((r[2] - r[0] - 160.0).abs() < 2.0 && (r[3] - r[1] - 22.0).abs() < 2.0, "{r:?}");
    // Move it 20 pt right.
    let c = at(&h, 120.0, 311.0);
    let d = c + egui::vec2(at(&h, 20.0, 0.0).x - at(&h, 0.0, 0.0).x, 0.0);
    drag(&mut h, c, d);
    let m = rect_of(&h, "Text1");
    assert!((m[0] - r[0] - 20.0).abs() < 1.5, "moved: {r:?} → {m:?}");
    // Field Properties: rename and make it required (one undo step).
    h.state_mut().execute("form.field.properties");
    h.run_steps(2);
    h.get_by_label("Text Field Properties");
    {
        let d = h.state_mut().field_props.as_mut().expect("open");
        d.name = "email".into();
        d.required = true;
    }
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(names(&h).contains(&"email".to_string()));
    {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        assert_eq!(doc.can_undo(), Some("Change field properties"));
        assert!(doc.form.iter().find(|f| f.name == "email").unwrap().has(pdfcraft_engine::field_flags::REQUIRED));
        assert_eq!(s.views[0].prepare.selected.as_ref().map(|s| s.0.as_str()), Some("email"));
    }
    // Delete removes the selected field.
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert_eq!(names(&h).len(), before);
    // A click with the check box tool places a default-size one.
    h.state_mut().execute("form.add.checkbox");
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
}

/// The arrow keys nudge the selected field a point, Shift and an arrow ten points (#2608), as they
/// do for a selected object or image and as dragging it does. One press is one undo step, the page
/// does not turn with them, and the field stays selected for the next press.
#[test]
fn arrow_keys_nudge_the_selected_field_instead_of_turning_the_page() {
    let mut h = harness();
    h.state_mut().execute("form.add.text");
    h.run_steps(1);
    // A click places a default-size text field and selects it.
    let p = at(&h, 40.0, 250.0);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(4);
    let id = h.state().views[0].id;
    let name = h.state().views[0].prepare.selected.clone().expect("the placed field is selected").0;
    let near = |a: f64, b: f64| (a - b).abs() < 0.01;
    let before = rect_of(&h, &name);
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(2);
    let right = rect_of(&h, &name);
    assert!(near(right[0], before[0] + 1.0) && near(right[2], before[2] + 1.0), "{before:?} → {right:?}");
    assert!(near(right[2] - right[0], before[2] - before[0]), "moved, not resized");
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Change field properties"));
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::ArrowUp);
    h.run_steps(2);
    let up = rect_of(&h, &name);
    assert!(near(up[1], right[1] + 10.0) && near(up[3], right[3] + 10.0), "{right:?} → {up:?}");
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(2);
    h.key_press(egui::Key::ArrowLeft);
    h.run_steps(2);
    let back = rect_of(&h, &name);
    assert!(near(back[0], up[0] - 1.0) && near(back[1], up[1] - 1.0), "{up:?} → {back:?}");
    assert!(h.state().views[0].prepare.selected.is_some(), "it stays selected for the next nudge");
    assert_eq!(h.state().views[0].current, 0, "the arrows moved the field, not the page");
}

#[test]
fn format_validate_and_calculate_tabs() {
    use pdfcraft_engine::form_scripts::{Calculate, Format, Validate};
    let mut h = harness();
    h.state_mut().execute("form.prepare");
    h.run_steps(2);
    h.state_mut().open_field_props("city", 0);
    h.run_steps(2);
    h.get_by_label("Format").click();
    h.run_steps(2);
    h.get_by_label("Select format category:");
    {
        let d = h.state_mut().field_props.as_mut().unwrap();
        d.format = Format::Number { decimals: 2, sep: 0, neg: 0, currency: "$".into(), prepend: true };
        d.validate = Validate::Range { min: Some(0.0), max: None };
        d.calculate = Calculate::Notation("name".into());
    }
    h.run_steps(2);
    h.get_by_label_contains("Example: -$1,234.50");
    h.get_by_label("Calculate").click();
    h.run_steps(2);
    h.get_by_label("Simplified field notation:");
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let f = s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "city").unwrap().clone();
    assert!(matches!(f.actions.format, Format::Number { decimals: 2, .. }));
    assert_eq!(f.actions.validate, Validate::Range { min: Some(0.0), max: None });
    assert_eq!(f.actions.calculate, Calculate::Notation("name".into()));
}

/// Renaming a field in Field Properties renames it in the formulas that read it, in the same undo
/// step, and says how many changed.
#[test]
fn renaming_a_field_says_how_many_calculations_now_use_the_new_name() {
    use pdfcraft_engine::form_scripts::Calculate;
    let mut h = harness();
    h.state_mut().execute("form.prepare");
    h.run_steps(2);
    let rename = |h: &mut Harness<'static, PdfCraftApp>, from: &str, to: &str| {
        h.state_mut().open_field_props(from, 0);
        h.run_steps(2);
        h.state_mut().field_props.as_mut().expect("open").name = to.into();
        h.get_by_label("OK").click();
        h.run_steps(3);
    };
    h.state_mut().open_field_props("city", 0);
    h.run_steps(2);
    h.state_mut().field_props.as_mut().expect("open").calculate = Calculate::Notation("name * 2".into());
    h.get_by_label("OK").click();
    h.run_steps(3);
    let calculation = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "city").unwrap().actions.calculate.clone()
    };
    h.state_mut().toast = None;
    rename(&mut h, "name", "full_name");
    assert_eq!(calculation(&h), Calculate::Notation("full_name * 2".into()));
    assert_eq!(h.state().toast.as_ref().map(|t| t.0.as_str()), Some("1 calculation now uses the new name."));
    h.state_mut().toast = None;
    rename(&mut h, "city", "town");
    assert!(names(&h).contains(&"town".to_string()));
    assert_eq!(h.state().toast, None, "no formula reads the calculated field itself");
    h.state_mut().execute("edit.undo");
    h.state_mut().execute("edit.undo");
    h.run_steps(2);
    assert!(names(&h).contains(&"name".to_string()));
    assert_eq!(calculation(&h), Calculate::Notation("name * 2".into()), "undone with the rename");
}

#[test]
fn appearance_tab_restyles_the_field() {
    use pdfcraft_engine::{BorderStyle, FieldFont};
    let mut h = harness();
    h.state_mut().execute("form.prepare");
    h.run_steps(2);
    h.state_mut().open_field_props("city", 0);
    h.run_steps(2);
    h.get_by_label("Appearance").click();
    h.run_steps(2);
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
    let before = h.state().field_props.as_ref().unwrap().look.unwrap();
    assert_eq!(h.get_all_by_label("No color").count(), 2);
    h.get_all_by_label("No color").next().unwrap().click();
    h.run_steps(2);
    let border = h.state().field_props.as_ref().unwrap().look.as_ref().unwrap().border;
    assert_eq!(border, if before.border.is_some() { None } else { Some([0.0; 3]) });
    assert_eq!(h.state().field_props.as_ref().unwrap().look.as_ref().unwrap().fill, before.fill);
    h.get_all_by_label("No color").nth(1).unwrap().click();
    h.run_steps(2);
    let look = h.state().field_props.as_ref().unwrap().look.as_ref().unwrap();
    assert_eq!(look.border, border);
    assert_eq!(look.fill, if before.fill.is_some() { None } else { Some([0.0; 3]) });
    {
        let d = h.state_mut().field_props.as_mut().unwrap();
        let l = d.look.as_mut().expect("a look");
        l.style = BorderStyle::Dashed;
        l.font = FieldFont::Courier;
        l.fill = Some([1.0, 1.0, 0.8]);
    }
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let l = s.session.get(s.views[0].id).unwrap().field_look("city").unwrap();
    assert_eq!((l.style, l.font, l.fill), (BorderStyle::Dashed, FieldFont::Courier, Some([1.0, 1.0, 0.8])));
}

#[test]
fn appearance_tab_keeps_auto_font_size() {
    let mut h = harness();
    h.state_mut().execute("form.prepare");
    h.run_steps(2);
    h.state_mut().open_field_props("city", 0);
    h.run_steps(2);
    h.get_by_label("Appearance").click();
    h.run_steps(2);
    h.state_mut().field_props.as_mut().unwrap().font_size = 0.0;
    h.run_steps(3);
    assert_eq!(h.state().field_props.as_ref().unwrap().font_size, 0.0, "Auto (0) must survive a frame");
}

#[test]
fn position_tab_rotates_a_field() {
    let mut h = harness();
    h.state_mut().open_field_props("city", 0);
    h.run_steps(2);
    let widget = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        let f = s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "city").unwrap();
        f.widgets[0].clone()
    };
    let before = widget(&h);
    assert_eq!(before.rotation, 0);
    assert!(h.state().field_props.as_ref().unwrap().props().is_none(), "an unchanged draft is not an edit");
    h.get_by_label("Position").click();
    h.run_steps(2);
    h.get_by_label("Rotation:");
    h.state_mut().field_props.as_mut().unwrap().rotation = 90;
    assert!(h.state().field_props.as_ref().unwrap().props().is_some());
    h.get_by_label("OK").click();
    h.run_steps(3);
    let turned = widget(&h);
    assert_eq!(turned.rotation, 90);
    let [x0, y0, x1, y1] = before.rect;
    let (cx, cy, w, ht) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, x1 - x0, y1 - y0);
    let expect = [cx - ht / 2.0, cy - w / 2.0, cx + ht / 2.0, cy + w / 2.0];
    assert!(turned.rect.iter().zip(expect).all(|(a, b)| (a - b).abs() < 0.05), "{:?} vs {expect:?}", turned.rect);
    h.state_mut().execute("edit.undo");
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
    h.state_mut().apply_edit(pdfcraft_engine::Edit::AddField {
        page: 0,
        rect: [40.0, 300.0, 56.0, 316.0],
        kind: pdfcraft_engine::NewField::CheckBox,
        name: Some("agree".into()),
    });
    h.state_mut().open_field_props("agree", 0);
    h.run_steps(2);
    h.get_by_label("Options").click();
    h.run_steps(2);
    h.get_by_label("Check Box Style:");
    assert_eq!(h.state().field_props.as_ref().unwrap().check_style, Some(CheckStyle::Check));
    h.state_mut().field_props.as_mut().unwrap().check_style = Some(CheckStyle::Cross);
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
    h.state_mut().apply_edit(Edit::AddField {
        page: 0,
        rect: [40.0, 300.0, 190.0, 330.0],
        kind: NewField::Button { caption: String::new() },
        name: Some("clear_button".into()),
    });
    h.state_mut().open_field_props("clear_button", 0);
    h.run_steps(2);
    h.get_by_label("Options").click();
    h.run_steps(2);
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
    h.state_mut().open_field_props("clear_button", 0);
    h.run_steps(2);
    h.get_by_label("Options").click();
    h.run_steps(2);
    assert_eq!(h.state().field_props.as_ref().unwrap().caption, "Clear Form");
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Label").click();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Label").type_text("Discard this");
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(h.state().session.save_bytes(id).unwrap(), before);
    h.state_mut().open_field_props("clear_button", 0);
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert_eq!(h.state().session.save_bytes(id).unwrap(), before);
}

#[test]
fn dropdown_alignment_is_editable_and_preserved() {
    choice_alignment(pdfcraft_engine::NewField::Combo { options: vec!["One".into(), "Two".into()], editable: false }, "Center", 1);
}

#[test]
fn list_box_alignment_is_editable_and_preserved() {
    choice_alignment(pdfcraft_engine::NewField::List { options: vec!["One".into(), "Two".into()], multi: true }, "Left", 0);
}

fn choice_alignment(kind: pdfcraft_engine::NewField, label: &str, quadding: i64) {
    use pdfcraft_engine::{Edit, FieldProps, FieldValue};
    let mut h = harness();
    h.state_mut().apply_edit(Edit::AddField { page: 0, rect: [40.0, 260.0, 240.0, 310.0], kind, name: Some("aligned_choice".into()) });
    h.state_mut()
        .apply_edit(Edit::SetFieldProps { name: "aligned_choice".into(), props: Box::new(FieldProps { quadding: Some(2), ..Default::default() }) });
    h.state_mut().apply_edit(Edit::SetFieldValue { name: "aligned_choice".into(), value: FieldValue::Choice(vec!["Two".into()]) });
    let field = |h: &Harness<'static, PdfCraftApp>| {
        let app = h.state();
        app.session.get(app.views[0].id).unwrap().form.iter().find(|f| f.name == "aligned_choice").unwrap().clone()
    };
    let original = field(&h);
    h.state_mut().open_field_props("aligned_choice", 0);
    h.run_steps(2);
    h.get_by_label("Options").click();
    h.run_steps(2);
    h.get_by_label("Alignment:");
    // The imported right alignment is displayed and unchanged until a choice is made.
    h.get_by_value("Right").click();
    h.run_steps(2);
    h.get_by_label(label).click();
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(field(&h).quadding, 2, "cancel keeps the existing /Q");
    h.state_mut().open_field_props("aligned_choice", 0);
    h.run_steps(2);
    h.get_by_label("Options").click();
    h.run_steps(2);
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
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    assert_eq!(field(&h).quadding, 2);
    h.state_mut().execute("edit.redo");
    h.run_steps(3);
    assert_eq!(field(&h).quadding, quadding);
}

#[test]
fn options_tab_sets_flags_alignment_and_defaults() {
    use pdfcraft_engine::field_flags as ff;
    let mut h = harness();
    h.state_mut().open_field_props("city", 0);
    h.run_steps(2);
    h.get_by_label("Options").click();
    h.run_steps(2);
    h.get_by_label("Check spelling").click();
    h.get_by_label("Password").click();
    h.run_steps(1);
    {
        let d = h.state_mut().field_props.as_mut().unwrap();
        d.quadding = 2;
        d.default = "Paris".into();
    }
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
    h.state_mut().execute("form.prepare");
    h.state_mut().set_option("panel", "fields").unwrap();
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
fn duplicating_a_field_onto_every_page() {
    let mut h = harness();
    h.state_mut().apply_edit(pdfcraft_engine::Edit::InsertBlankPage { at: 1, width: 300.0, height: 400.0 });
    h.state_mut().execute("form.prepare");
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
    h.get_by_label("Duplicate Field");
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
        h.state_mut().apply_edit(pdfcraft_engine::Edit::AddField {
            page: 0,
            rect: [x, y, x + w, y + 20.0],
            kind: pdfcraft_engine::NewField::Text { multiline: false },
            name: Some(name.into()),
        });
    }
    h.run_steps(2);
    let form = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(h.state().views[0].id).unwrap().form.as_ref().clone();
    let rect = |h: &Harness<'static, PdfCraftApp>, n: &str| form(h).iter().find(|f| f.name == n).unwrap().widgets[0].rect;
    let a = ("a".to_string(), 0);
    let others = vec![("b".to_string(), 0), ("c".to_string(), 0)];
    // Align tops with a.
    let e = arrange(&form(&h), &a, &others, Arrange::AlignTop).unwrap();
    h.state_mut().apply_edit(e);
    assert!(["b", "c"].iter().all(|n| rect(&h, n)[3] == 320.0));
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Align fields"));
    // Distribute horizontally: equal gaps between a, b and c.
    let e = arrange(&form(&h), &a, &others, Arrange::DistributeH).unwrap();
    h.state_mut().apply_edit(e);
    let (ra, rb, rc) = (rect(&h, "a"), rect(&h, "b"), rect(&h, "c"));
    assert!(((rb[0] - ra[2]) - (rc[0] - rb[2])).abs() < 1e-6, "{ra:?} {rb:?} {rc:?}");
    // Same width as a.
    let e = arrange(&form(&h), &a, &others, Arrange::SameWidth).unwrap();
    h.state_mut().apply_edit(e);
    assert!(["b", "c"].iter().all(|n| (rect(&h, n)[2] - rect(&h, n)[0] - 60.0).abs() < 1e-6));
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Match field sizes"));
    assert!(arrange(&form(&h), &a, &[("b".into(), 0)], Arrange::DistributeV).is_none(), "distributing needs three");
}

#[test]
fn deleting_several_selected_fields_and_detecting_fields_from_the_panel() {
    let mut h = harness();
    for (name, x) in [("a", 20.0), ("b", 120.0), ("c", 220.0)] {
        h.state_mut().apply_edit(pdfcraft_engine::Edit::AddField {
            page: 0,
            rect: [x, 300.0, x + 60.0, 320.0],
            kind: pdfcraft_engine::NewField::Text { multiline: false },
            name: Some(name.into()),
        });
    }
    assert!(h.state_mut().execute("form.prepare"));
    h.run_steps(2);
    // #93: field detection is in the panel, not only run when a form has no fields.
    h.get_by_label("Detect form fields");
    // #95: Delete removes every selected field, as one undoable step.
    {
        let p = &mut h.state_mut().views[0].prepare;
        p.selected = Some(("a".into(), 0));
        p.also = vec![("b".into(), 0), ("c".into(), 0)];
    }
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    let left = names(&h);
    assert!(!["a", "b", "c"].iter().any(|n| left.iter().any(|l| l == n)), "{left:?}");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Delete 3 fields"));
    assert!(h.state().views[0].prepare.selected.is_none() && h.state().views[0].prepare.also.is_empty());
}

#[test]
fn preview_fills_the_form_and_locked_fields_keep_their_properties() {
    let mut h = harness();
    assert!(h.state_mut().execute("form.prepare"));
    h.run_steps(2);
    assert!(h.state().is_preparing());
    h.get_by_label("Preview").click();
    h.run_steps(2);
    assert!(!h.state().is_preparing(), "Preview fills the form");
    h.get_by_label("Edit fields").click();
    h.run_steps(2);
    assert!(h.state().is_preparing());
    // Lock a field: its properties are then greyed out until unlocked.
    h.state_mut().open_field_props("city", 0);
    h.run_steps(2);
    h.get_by_label("Locked").click();
    h.run_steps(1);
    h.get_by_label("OK").click();
    h.run_steps(3);
    let locked = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "city").unwrap().locked()
    };
    assert!(locked(&h));
    h.state_mut().open_field_props("city", 0);
    h.run_steps(2);
    h.get_by_label_contains("This field is locked");
    h.get_by_label("Locked").click();
    h.run_steps(1);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(!locked(&h));
}

#[test]
fn image_fields_take_a_picture_when_clicked() {
    let mut h = harness();
    assert!(h.state_mut().execute("form.add.image"));
    h.run_steps(2);
    let (a, b) = (at(&h, 40.0, 220.0), at(&h, 140.0, 290.0));
    drag(&mut h, a, b);
    assert_eq!(names(&h).last().map(String::as_str), Some("Image1"));
    // A picture to choose (the picker is bypassed in tests).
    let path = std::env::temp_dir().join(format!("pdfcraft-image-field-{}.png", std::process::id()));
    image::RgbImage::from_pixel(8, 4, image::Rgb([200, 30, 30])).save(&path).unwrap();
    h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
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

#[test]
fn actions_tab_adds_and_removes_actions() {
    use pdfcraft_engine::{FieldAction, FieldTrigger};
    let mut h = harness();
    h.state_mut().execute("form.prepare");
    h.run_steps(2);
    h.state_mut().open_field_props("city", 0);
    h.run_steps(2);
    h.get_by_label("Actions").click();
    h.run_steps(2);
    h.get_by_label("Select Trigger:");
    h.get_by_label("No actions");
    {
        let d = h.state_mut().field_props.as_mut().unwrap();
        d.new_action.trigger = FieldTrigger::OnBlur;
        d.new_action.kind = 1;
        d.new_action.text = "https://example.org".into();
    }
    h.run_steps(3);
    h.get_by_label("Add").click();
    h.run_steps(2);
    h.get_by_label("Open a web link: https://example.org");
    {
        let d = h.state_mut().field_props.as_mut().unwrap();
        d.new_action.trigger = FieldTrigger::MouseUp;
        d.new_action.kind = 0;
        d.new_action.text = "app.alert('x')".into();
    }
    h.run_steps(3);
    h.get_by_label("Add").click();
    h.run_steps(2);
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
    h.state_mut().execute("form.prepare");
    h.run_steps(2);
    let script = "if (event.value > 10) {\n  app.alert('Too many');\n}";
    let actions = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().field_actions("city")
    };
    let open = |h: &mut Harness<'static, PdfCraftApp>| {
        h.state_mut().open_field_props("city", 0);
        h.run_steps(2);
        h.get_by_label("Actions").click();
        h.run_steps(2);
    };
    // A script on Mouse Up and a link on Blur, saved with the field.
    open(&mut h);
    for (trigger, kind, text) in [(FieldTrigger::MouseUp, 0, script), (FieldTrigger::OnBlur, 1, "https://example.org")] {
        let d = h.state_mut().field_props.as_mut().unwrap();
        d.new_action = pdfcraft_ui_egui::prepare::ActionDraft { trigger, kind, text: text.into() };
        h.run_steps(3);
        h.get_by_label("Add").click();
        h.run_steps(2);
    }
    h.get_by_label("OK").click();
    h.run_steps(3);
    let saved = actions(&h);
    // Edit shows the script exactly as stored; Discard leaves everything as it was.
    open(&mut h);
    h.get_all_by_label("Edit").last().unwrap().click();
    h.run_steps(2);
    h.get_by_label("Edit Action");
    assert_eq!(h.state().field_props.as_ref().unwrap().new_action.text, script);
    h.state_mut().field_props.as_mut().unwrap().new_action.text = "app.alert('changed');".into();
    h.run_steps(2);
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
    h.state_mut().field_props.as_mut().unwrap().new_action.text = "app.alert('changed');".into();
    h.run_steps(2);
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
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (go) /Rect [20 300 100 320] /P 3 0 R /A 5 0 R /AA << /Bl << /S /JavaScript /JS (this.dirty = true;) >> >> >>",
        "<< /S /JavaScript /JS (app.alert\\(1\\);) /Next 6 0 R >>",
        "<< /S /URI /URI (https://example.org) >>",
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

#[test]
fn actions_tab_shows_a_chain_and_keeps_it_when_the_first_action_is_replaced() {
    use egui_kittest::kittest::NodeT;
    use pdfcraft_engine::FieldTrigger;
    let mut h = harness_with("chain.pdf", chained_action_fixture());
    h.state_mut().execute("form.prepare");
    h.run_steps(2);
    let open = |h: &mut Harness<'static, PdfCraftApp>| {
        h.state_mut().open_field_props("go", 0);
        h.run_steps(2);
        h.get_by_label("Actions").click();
        h.run_steps(2);
    };
    open(&mut h);
    h.get_by_label("Run a JavaScript, then 1 more action");
    // The first Edit is the menu bar's; the rows' follow (Mouse Up, On Blur).
    assert_eq!(h.get_all_by_label("Edit").count(), 3);
    h.get_all_by_label("Edit").nth(1).unwrap().click();
    h.run_steps(2);
    assert_eq!(h.state().field_props.as_ref().unwrap().editing, Some(FieldTrigger::MouseUp));
    // While one script is open, no other Edit can overwrite it.
    assert!(h.get_all_by_label("Edit").skip(1).all(|n| n.accesskit_node().is_disabled()));
    h.get_by_label("Discard").click();
    h.run_steps(2);
    assert!(h.get_all_by_label("Edit").skip(1).all(|n| !n.accesskit_node().is_disabled()));
    // Replace the Mouse Up script with a link: the chained link stays after it.
    {
        let d = h.state_mut().field_props.as_mut().unwrap();
        d.actions.retain(|(t, _)| *t != FieldTrigger::MouseUp);
        d.new_action = pdfcraft_ui_egui::prepare::ActionDraft { trigger: FieldTrigger::MouseUp, kind: 1, text: "https://example.com".into() };
    }
    h.run_steps(3);
    h.get_by_label("Add").click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    open(&mut h);
    h.get_by_label("Open a web link: https://example.com, then 1 more action");
}

/// Text field table: pick the tool and drag out an area in the empty lower part of page 1, 200 by
/// 120 points from (40, 240) as displayed. The table's window opens for it.
fn draw_table(h: &mut Harness<'static, PdfCraftApp>) {
    assert!(h.state_mut().execute("form.add.text_table"));
    h.run_steps(2);
    let (a, b) = (at(h, 40.0, 240.0), at(h, 240.0, 360.0));
    drag(h, a, b);
    assert_eq!(h.state().dialog, Some(Dialog::FieldTable));
}

fn columns(patterns: &[(&str, f64)]) -> Vec<TableColumn> {
    patterns.iter().map(|(p, w)| TableColumn { pattern: (*p).to_owned(), width: *w }).collect()
}

#[test]
fn a_text_field_table_is_drawn_set_up_and_added_in_one_step() {
    let mut h = harness();
    let before = names(&h);
    draw_table(&mut h);
    assert_eq!(h.state().quick_tool, QuickTool::Select, "back to Select once the area is drawn");
    {
        let s = h.state_mut();
        let id = s.views[0].id;
        let d = s.field_table_draft.as_mut().expect("the table's window is open");
        assert_eq!((d.doc, d.page), (id, 0));
        let r = d.table.rect;
        assert!([40.0, 240.0, 240.0, 360.0].iter().zip(r).all(|(w, g)| (w - g).abs() < 0.5), "drawn as displayed: {r:?}");
        d.table.rows = 2;
        d.table.columns = columns(&[("desc_N", 3.0), ("price_N", 1.0)]);
    }
    h.run_steps(2);
    h.get_by_label("Add fields").click();
    h.run_steps(3);
    assert_eq!((h.state().dialog, h.state().field_table_draft.is_some()), (None, false), "added, the window closes");
    let added: Vec<_> = names(&h).into_iter().filter(|n| !before.contains(n)).collect();
    assert_eq!(added, ["desc_1", "price_1", "desc_2", "price_2"]);
    // 150 and 50 points wide, 60 high, the first row at the top: user space has y up from 400.
    let r = rect_of(&h, "desc_1");
    assert!([40.0, 100.0, 190.0, 160.0].iter().zip(r).all(|(w, g)| (w - g).abs() < 0.5), "{r:?}");
    let r = rect_of(&h, "price_2");
    assert!([190.0, 40.0, 240.0, 100.0].iter().zip(r).all(|(w, g)| (w - g).abs() < 0.5), "{r:?}");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Add text field table"));
    h.state_mut().execute("edit.undo");
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
    assert_eq!(h.state().field_table_draft.as_ref().unwrap().table.columns[0].pattern, "Table2_N");
    // Named like the first, it can't be added: the window says why and keeps what was set up.
    h.state_mut().field_table_draft.as_mut().unwrap().table.columns[0].pattern = "Table1_N".into();
    h.run_steps(2);
    let count = names(&h).len();
    h.get_by_label("a field named \"Table1_1\" already exists");
    h.get_by_label("Add fields").click();
    h.run_steps(3);
    assert_eq!(names(&h).len(), count, "nothing added");
    assert_eq!(h.state().dialog, Some(Dialog::FieldTable), "still open");
    // Escape closes it, adding nothing.
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!((h.state().dialog, h.state().field_table_draft.is_some(), names(&h).len()), (None, false, count));
}

/// Enter commits a number being typed. It used to add the table there and then, before the rest
/// of it was set up.
#[test]
fn enter_in_a_number_field_does_not_add_the_table() {
    let mut h = harness();
    let before = names(&h).len();
    draw_table(&mut h);
    let rows = h.query_all(by().role(Role::SpinButton)).next().map(|n| n.rect()).expect("the Rows field");
    h.hover_at(rows.center());
    h.drag_at(rows.center());
    h.run_steps(1);
    h.drop_at(rows.center());
    h.run_steps(2);
    h.key_press(egui::Key::Backspace);
    h.event(egui::Event::Text("6".into()));
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(names(&h).len(), before, "nothing added");
    assert_eq!(h.state().field_table_draft.as_ref().map(|d| d.table.rows), Some(6), "the number is in, the window open");
}

/// With 26 or more columns each pair of columns has less than twice the least share a divider
/// keeps, and dragging one panicked in `f64::clamp`.
#[test]
fn dragging_a_column_divider_between_many_columns() {
    let mut h = harness();
    draw_table(&mut h);
    h.state_mut().field_table_draft.as_mut().unwrap().table.columns = columns(&[("c{c}_N", 1.0); 30]);
    h.run_steps(2);
    let (above, below) = (h.get_by_label("Distribute evenly").rect(), h.get_by_label("Field names").rect());
    let p = egui::pos2(above.center().x - 120.0, (above.bottom() + below.top()) / 2.0);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    for dx in [-10.0, -20.0] {
        h.hover_at(p + egui::vec2(dx, 0.0));
        h.run_steps(1);
    }
    h.drop_at(p + egui::vec2(-20.0, 0.0));
    h.run_steps(2);
    let widths: Vec<f64> = h.state().field_table_draft.as_ref().unwrap().table.columns.iter().map(|c| c.width).collect();
    assert!(widths.iter().any(|w| (w - 1.0).abs() > 1e-9), "a divider moved: {widths:?}");
    assert!((widths.iter().sum::<f64>() - 30.0).abs() < 1e-9 && widths.iter().all(|w| *w > 0.0), "{widths:?}");
}

#[test]
fn the_table_window_closes_when_its_document_is_left() {
    let mut h = harness();
    let blank = pdfcraft_engine::Session::new().create_blank(300.0, 400.0, 1).unwrap().to_vec();
    h.state_mut().open_bytes("other.pdf", None, blank).unwrap();
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
    assert_eq!((h.state().dialog, h.state().field_table_draft.is_some()), (None, false));
    h.state_mut().active = Some(0);
    h.run_steps(2);
    assert_eq!(names(&h).len(), before, "nothing added to either document");
    // So does leaving Prepare a form.
    draw_table(&mut h);
    h.state_mut().left_open = false;
    h.run_steps(2);
    assert_eq!((h.state().dialog, h.state().field_table_draft.is_some()), (None, false));
}

/// With its panel closed after the tool was picked, the form is still prepared and a table can be
/// drawn: its window opens Prepare a form again, which it belongs to. Drawing sends the tool back
/// to Select, so the window used to close itself at once, nothing being prepared any more.
#[test]
fn a_table_drawn_with_the_panel_closed_opens_it_again() {
    let mut h = harness();
    let before = names(&h).len();
    assert!(h.state_mut().execute("form.add.text_table"));
    h.run_steps(2);
    h.state_mut().left_open = false;
    h.run_steps(2);
    assert!(h.state().is_preparing(), "the picked tool keeps the form prepared");
    let (a, b) = (at(&h, 40.0, 240.0), at(&h, 240.0, 360.0));
    drag(&mut h, a, b);
    h.run_steps(3);
    assert_eq!((h.state().dialog, h.state().field_table_draft.is_some()), (Some(Dialog::FieldTable), true), "the window stays open");
    assert!(h.state().left_open && h.state().is_preparing(), "Prepare a form is open again");
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
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
    h.run_steps(2);
    assert_eq!((h.state().dialog, h.state().field_table_draft.is_some()), (None, false), "closed by the rotation");
    assert_eq!(names(&h), before, "nothing added");
}

/// Escape in a name field leaves the field, as it does in any field; it closed the window too,
/// and the column setup with it.
#[test]
fn escape_in_a_name_field_leaves_the_field_not_the_table() {
    let mut h = harness();
    draw_table(&mut h);
    // The app's own text boxes (zoom, page number) are inputs too: take the one in the window.
    let window = h.get_by_label("Text Field Table").rect();
    let field =
        h.query_all(by().role(Role::TextInput)).map(|n| n.rect()).find(|r| window.contains(r.center())).expect("the first column's name field");
    h.hover_at(field.center());
    h.drag_at(field.center());
    h.run_steps(1);
    h.drop_at(field.center());
    h.run_steps(2);
    h.event(egui::Event::Text("x".into()));
    h.run_steps(1);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(Dialog::FieldTable), "still open");
    assert_eq!(h.state().field_table_draft.as_ref().map(|d| d.table.columns[0].pattern.as_str()), Some("Table1_Nx"), "what was typed stays");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!((h.state().dialog, h.state().field_table_draft.is_some()), (None, false), "a second Escape closes it");
}

/// A click puts the default 180 × 110 table's corner at the pointer; near the page's edges it is
/// moved in so that all of it is on the page (here 5 pt from the right, 100 pt from the bottom).
#[test]
fn a_table_clicked_near_the_page_edge_stays_on_the_page() {
    let mut h = harness();
    assert!(h.state_mut().execute("form.add.text_table"));
    h.run_steps(2);
    let p = at(&h, 295.0, 300.0);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
    let r = h.state().field_table_draft.as_ref().expect("the table's window").table.rect;
    assert!([120.0, 290.0, 300.0, 400.0].iter().zip(r).all(|(w, g)| (w - g).abs() < 0.5), "on the 300 × 400 page: {r:?}");
}

/// Prepare a form ▸ Calculation order opens as a page of its own, with the form still being
/// prepared, says when nothing calculates, and Back returns to Prepare a form.
#[test]
fn the_calculation_order_is_a_page_of_prepare_a_form() {
    use pdfcraft_ui_egui::LeftPanel;
    let mut h = harness();
    h.state_mut().execute("form.prepare");
    h.run_steps(3);
    assert!(h.query_by_label("Field list").is_some());
    h.get_by_label("Calculation order").click();
    h.run_steps(3);
    assert_eq!(h.state().left, LeftPanel::CalculationOrder);
    assert!(h.state().is_preparing(), "the fields stay editable, so a row can select one");
    // The page replaces the Prepare a form tools.
    assert!(h.query_by_label("Field list").is_none());
    assert!(h.query_by_label_contains("No field in this form calculates its value yet").is_some());
    h.get_by_label("Back to Prepare a form").click();
    h.run_steps(3);
    assert_eq!(h.state().left, LeftPanel::Tool("form"));
    assert!(h.query_by_label("Field list").is_some());
    assert!(h.query_by_label("Back to Prepare a form").is_none());
    // Opened from Preview, the page edits the fields again.
    h.get_by_label("Preview").click();
    h.run_steps(3);
    assert!(h.state().form_preview);
    h.get_by_label("Calculation order").click();
    h.run_steps(3);
    assert!(h.state().is_preparing() && !h.state().form_preview, "Preview ends, so a row can select its field");
    // Opened (from ⌘K) while the page grid shows, the grid closes and the page shows.
    h.state_mut().execute("page.organize");
    h.run_steps(3);
    assert!(h.state().views[0].organize);
    h.state_mut().execute("form.calculation_order");
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
        h.state_mut().apply_edit(Edit::AddField { page: 0, rect: [20.0, y, 120.0, y + 20.0], kind, name: Some(name.into()) });
    }
    let calculate = |name: &str, c: Calculate| Edit::SetFieldProps {
        name: name.into(),
        props: Box::new(FieldProps { calculate: Some(c), ..Default::default() }),
    };
    // Defined backwards: total, which reads sub, calculates first.
    h.state_mut().apply_edit(calculate("total", Calculate::Simple { op: CalcOp::Sum, fields: vec!["sub".into()] }));
    h.state_mut().apply_edit(calculate("sub", Calculate::Notation("qty * 2".into())));
    h.state_mut().apply_edit(calculate("other", Calculate::Notation("qty + 1".into())));
    // other shows its value as currency.
    let currency = Format::Number { decimals: 2, sep: 0, neg: 0, currency: "$".into(), prepend: true };
    h.state_mut()
        .apply_edit(Edit::SetFieldProps { name: "other".into(), props: Box::new(FieldProps { format: Some(currency), ..Default::default() }) });
    h.state_mut().apply_edit(Edit::SetFieldValue { name: "qty".into(), value: FieldValue::Text("3".into()) });
    h.state_mut().execute("form.prepare");
    h.run_steps(3);
    h.get_by_label("Calculation order").click();
    h.run_steps(3);
    let order = |h: &Harness<'static, PdfCraftApp>| -> Vec<String> {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().calculation_order().fields.into_iter().map(|f| f.name).collect()
    };
    let undo_label = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(h.state().views[0].id).unwrap().can_undo().map(str::to_owned);
    assert_eq!(order(&h), ["total", "sub", "other"]);
    // The card says what is wrong, and each row shows its value as the field does: total
    // calculated before sub, so it still has sub's old value.
    assert!(h.query_by_label("1 field shows an old value").is_some());
    assert!(h.query_by_label("total calculates before sub, which it reads.").is_some());
    for row in ["1. total: 0", "2. sub: 6", "3. other: $4.00"] {
        assert!(h.query_by_label(row).is_some(), "{row}");
    }
    // The selected field's arrows move it.
    assert!(h.query_by_label("Later in calculation order: total").is_none(), "arrows only on the selected field or the one pointed at");
    h.get_by_label("1. total: 0").click();
    h.run_steps(3);
    h.get_by_label("Later in calculation order: total").click();
    h.run_steps(3);
    assert_eq!(order(&h), ["sub", "total", "other"]);
    assert_eq!(undo_label(&h).as_deref(), Some("Set calculation order"));
    // Alt+↑ moves it back.
    h.key_press_modifiers(Modifiers::ALT, Key::ArrowUp);
    h.run_steps(3);
    assert_eq!(order(&h), ["total", "sub", "other"]);
    // Dragging other onto the first row puts it first.
    let from = h.get_by_label("3. other: $4.00").rect().center();
    let to = h.get_by_label("1. total: 6").rect().center();
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
    h.get_by_label("3. sub: 6").click();
    h.run_steps(2);
    assert_eq!(h.state().views[0].prepare.selected.as_ref().map(|s| s.0.as_str()), Some("sub"));
}

/// Fields that read each other get a card of their own, which Fix order leaves alone, and its
/// link opens the first one's formula.
#[test]
fn a_loop_has_its_own_card_that_opens_the_formula() {
    use pdfcraft_engine::form_scripts::Calculate;
    use pdfcraft_engine::{Edit, FieldProps, NewField};
    use pdfcraft_ui_egui::prepare::FieldTab;
    let mut h = harness();
    for (i, (name, formula)) in [("a", "b + 1"), ("b", "a + 1")].into_iter().enumerate() {
        let y = 200.0 + 30.0 * i as f64;
        let kind = NewField::Text { multiline: false };
        h.state_mut().apply_edit(Edit::AddField { page: 0, rect: [20.0, y, 120.0, y + 20.0], kind, name: Some(name.into()) });
        let props = Box::new(FieldProps { calculate: Some(Calculate::Notation(formula.into())), ..Default::default() });
        h.state_mut().apply_edit(Edit::SetFieldProps { name: name.into(), props });
    }
    h.state_mut().execute("form.prepare");
    h.run_steps(3);
    h.get_by_label("Calculation order").click();
    h.run_steps(3);
    assert!(h.query_by_label("a and b read each other").is_some());
    assert!(h.query_by_label("Every field outside a loop calculates after the fields it reads.").is_some());
    assert!(h.query_by_label("Fix order").is_none(), "no order fixes a loop");
    h.get_by_label("Edit a's formula").click();
    h.run_steps(3);
    let draft = h.state().field_props.as_ref().map(|d| (d.field.clone(), d.tab));
    assert_eq!(draft, Some(("a".to_owned(), FieldTab::Calculate)));
}
