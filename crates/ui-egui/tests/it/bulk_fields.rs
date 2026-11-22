//! Shared properties and multi-selection through the real Prepare-a-form shell.

use crate::support::AppHarness as _;
use craft::ui::dialog::StackDialog;
use egui::accesskit::Role;
use egui_kittest::{Harness, kittest::Queryable};
use pdfcraft_engine::{Edit, FieldFont, FieldProps, FieldValue, NewField, Session, field_flags};
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::bulk_fields::SharedFieldPropsDialog;
use pdfcraft_ui_egui::prepare::FieldPropsDialog;

fn fixture() -> Vec<u8> {
    let mut s = Session::new();
    let bytes = s.create_blank(300.0, 400.0, 2).unwrap();
    let id = s.open("form.pdf", None, bytes, None).unwrap();
    for (name, page, rect, kind) in [
        ("alpha", 0, [40.0, 290.0, 230.0, 314.0], NewField::Text { multiline: false }),
        ("beta", 0, [40.0, 240.0, 230.0, 264.0], NewField::Text { multiline: false }),
        ("consent", 0, [40.0, 180.0, 54.0, 194.0], NewField::CheckBox),
        ("gamma", 1, [40.0, 290.0, 230.0, 314.0], NewField::Text { multiline: false }),
    ] {
        s.apply(id, Edit::AddField { name: Some(name.into()), page, rect, kind }).unwrap();
    }
    for (name, value, fill, font, width, size, required) in [
        ("alpha", "Ada", [1.0, 0.9, 0.8], FieldFont::Courier, 1.0, 11.0, true),
        ("beta", "Lovelace", [0.8, 0.9, 1.0], FieldFont::Times, 3.0, 18.0, false),
    ] {
        s.apply(id, Edit::SetFieldValue { name: name.into(), value: FieldValue::Text(value.into()) }).unwrap();
        let mut look = s.get(id).unwrap().field_look(name).unwrap();
        look.fill = Some(fill);
        look.font = font;
        look.width = width;
        s.apply(
            id,
            Edit::SetFieldProps {
                name: name.into(),
                props: Box::new(FieldProps {
                    tooltip: Some(format!("Help for {name}")),
                    look: Some(look),
                    required: Some(required),
                    font_size: Some(size),
                    ..Default::default()
                }),
            },
        )
        .unwrap();
    }
    s.save_bytes(id).unwrap().as_ref().clone()
}

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            // Saves stamp /ModDate from the clock; pinned, two saves compare equal across a second.
            app.session = std::mem::take(&mut app.session).with_clock(|| 1_700_000_000);
            app.open_bytes(&egui::Context::default(), "form.pdf", None, fixture()).unwrap();
            app.set_option(&egui::Context::default(), "zoom", "150").unwrap();
            app.execute(&egui::Context::default(), "form.prepare");
            app
        }
    });
    h.run_steps(4);
    h
}

fn doc<'a>(h: &'a Harness<'static, PdfCraftApp>) -> &'a pdfcraft_engine::Document {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap()
}

fn bytes(h: &Harness<'static, PdfCraftApp>) -> Vec<u8> {
    h.state().session.save_bytes(h.state().views[0].id).unwrap().as_ref().clone()
}

fn select(h: &mut Harness<'static, PdfCraftApp>, names: &[&str]) {
    let p = &mut h.state_mut().views[0].session.prepare;
    p.selected = names.first().map(|n| (n.to_string(), 0));
    p.also = names.iter().skip(1).map(|n| (n.to_string(), 0)).collect();
    h.execute("form.field.properties");
    h.run_steps(4);
}

/// The values the Shared Field Properties dialog holds, as the dialog host reports them to
/// automation: each property is `{ value, mixed, apply }`.
fn shared(h: &Harness<'static, PdfCraftApp>) -> serde_json::Value {
    h.state().dialogs.get::<SharedFieldPropsDialog>().and_then(|d| d.json_fields()).expect("Shared Field Properties is open")
}

/// Whether the Shared Field Properties dialog is open.
fn shared_open(h: &Harness<'static, PdfCraftApp>) -> bool {
    h.state().dialogs.is_open::<SharedFieldPropsDialog>()
}

/// Whether the dialog shows a refusal containing `words`. The General tab's hint also says
/// "locked", so it is not a refusal.
fn refused_with(h: &Harness<'static, PdfCraftApp>, words: &str) -> bool {
    use egui_kittest::kittest::NodeT;
    h.query_all_by_label_contains(words).any(|n| n.accesskit_node().label().is_some_and(|l| !l.starts_with("To change locked fields")))
}

/// Types and confirms a value in the line thickness field.
fn type_in_thickness(h: &mut Harness<'static, PdfCraftApp>, text: &str) {
    h.get_by_role_and_label(Role::Group, "Line Thickness:").get_by_role(Role::SpinButton).focus();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text(text.into()));
    h.run_steps(1);
    h.key_press(egui::Key::Tab);
    h.run_steps(4);
}

/// Ticks the apply box in the header of the property group `label`: the group and its header text share the label,
/// so the box is found inside the group.
fn click_gate(h: &mut Harness<'static, PdfCraftApp>, label: &str) {
    h.get_by_role_and_label(Role::Group, label).get_by_role_and_label(Role::CheckBox, "Enable").click();
}

/// Ticks the Tooltip property of the dialog and types `text` into its text box.
fn set_tooltip(h: &mut Harness<'static, PdfCraftApp>, text: &str) {
    click_gate(h, "Tooltip:");
    h.run_steps(4);
    h.get_by_role_and_label(Role::TextInput, "Tooltip").click();
    h.run_steps(1);
    h.get_by_role_and_label(Role::TextInput, "Tooltip").type_text(text);
    h.run_steps(4);
}

#[test]
fn shared_properties_preserve_mixed_values_and_undo_the_whole_change() {
    let mut h = harness();
    let before = bytes(&h);
    let original = [doc(&h).field_look("alpha").unwrap(), doc(&h).field_look("beta").unwrap()];
    select(&mut h, &["alpha", "beta"]);
    h.get_by_role_and_label(Role::Dialog, "Shared Field Properties");
    h.get_by_label("2 fields selected");
    assert!(shared(&h)["required"]["mixed"].as_bool().unwrap());
    // Ticking a property applies the value the first field has: alpha is required.
    click_gate(&mut h, "Required");
    h.run_steps(4);
    assert_eq!(shared(&h)["required"]["apply"], true);
    assert_eq!(shared(&h)["required"]["value"], true);
    h.get_by_label("Appearance").click();
    h.run_steps(4);
    // Below the fold of the dialog's scroll area with the roomier rows (#487).
    h.get_by_role_and_label(Role::Group, "Line Thickness:").scroll_to_me();
    h.run_steps(4);
    click_gate(&mut h, "Line Thickness:");
    h.run_steps(4);
    assert_eq!(shared(&h)["width"]["apply"], true, "thickness click enables the property");
    type_in_thickness(&mut h, "4");
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert!(!shared_open(&h));
    for (name, look, value) in [("alpha", original[0], "Ada"), ("beta", original[1], "Lovelace")] {
        let field = doc(&h).form.iter().find(|f| f.name == name).unwrap();
        assert!(field.has(field_flags::REQUIRED));
        assert_eq!(field.value, [value]);
        assert_eq!(field.tooltip.as_deref(), Some(format!("Help for {name}").as_str()));
        let new = doc(&h).field_look(name).unwrap();
        assert_eq!(new.width, 4.0);
        assert_eq!((new.fill, new.font, new.text, new.border), (look.fill, look.font, look.text, look.border));
    }
    assert_eq!(doc(&h).can_undo(), Some("Change field properties"));
    assert_eq!(h.state().views[0].session.prepare.names(), ["alpha", "beta"]);
    h.execute("edit.undo");
    h.run_steps(4);
    assert_eq!(bytes(&h), before);
    h.execute("edit.redo");
    h.run_steps(4);
    assert_eq!(doc(&h).field_look("beta").unwrap().width, 4.0);
    // The regenerated look also survives a real Save/open round trip.
    let mut s = Session::new();
    let id = s.open("saved.pdf", None, std::sync::Arc::new(bytes(&h)), None).unwrap();
    assert_eq!(s.get(id).unwrap().field_look("beta").unwrap().fill, original[1].fill);
    assert_eq!(s.get(id).unwrap().form.iter().find(|f| f.name == "beta").unwrap().value, ["Lovelace"]);
}

#[test]
fn cancelling_shared_properties_discards_only_the_draft() {
    let mut h = harness();
    let before = bytes(&h);
    select(&mut h, &["alpha", "beta", "consent"]);
    h.get_by_label("Appearance").click();
    h.run_steps(4);
    click_gate(&mut h, "Fill Color:");
    h.run_steps(4);
    h.get_by_role_and_label(Role::Group, "Fill Color:").get_by_label("No color").click();
    h.run_steps(4);
    assert_eq!(shared(&h)["fill"]["apply"], true);
    assert!(shared(&h)["fill"]["value"].is_null(), "no fill is the draft's value");
    h.get_by_label("Cancel").click();
    h.run_steps(4);
    assert_eq!(bytes(&h), before);
    assert!(!shared_open(&h));
    assert_eq!(h.state().views[0].session.prepare.names(), ["alpha", "beta", "consent"]);
}

#[test]
fn locked_field_refuses_the_entire_batch_and_keeps_the_draft_for_correction() {
    let mut h = harness();
    let ctx_h = h.ctx.clone();
    h.state_mut()
        .apply_edit(&ctx_h, Edit::SetFieldProps { name: "beta".into(), props: Box::new(FieldProps { locked: Some(true), ..Default::default() }) });
    let before = bytes(&h);
    select(&mut h, &["alpha", "beta"]);
    set_tooltip(&mut h, "Shared help");
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(bytes(&h), before, "alpha must not be committed before beta refuses");
    assert!(shared_open(&h), "the dialog stays open for the correction");
    assert!(refused_with(&h, "locked"));
    // Unlocking applies the first field's value: alpha is not locked.
    click_gate(&mut h, "Locked");
    h.run_steps(4);
    assert_eq!(shared(&h)["locked"]["value"], false);
    // The field shows the first field's own text (the tooltip is mixed), so typing adds to it.
    assert!(shared(&h)["tooltip"]["value"].as_str().is_some_and(|v| v.ends_with("Shared help")), "what was typed is kept");
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert!(!shared_open(&h));
    assert!(
        doc(&h)
            .form
            .iter()
            .filter(|f| ["alpha", "beta"].contains(&f.name.as_str()))
            .all(|f| !f.locked() && f.tooltip.as_deref().is_some_and(|t| t.ends_with("Shared help")))
    );
    h.execute("edit.undo");
    h.run_steps(4);
    assert_eq!(bytes(&h), before, "unlock and changes undo together");
}

#[test]
fn shared_properties_cannot_apply_to_a_different_active_document() {
    let mut h = harness();
    let before = bytes(&h);
    select(&mut h, &["alpha", "beta"]);
    click_gate(&mut h, "Required");
    h.run_steps(4);
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "other.pdf", None, fixture()).unwrap();
    h.run_steps(4);
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(bytes(&h), before);
    let id = h.state().views[1].id;
    assert!(!h.state().session.get(id).unwrap().dirty);
    assert!(shared_open(&h));
    assert!(refused_with(&h, "document changed"));
}

#[test]
fn deleting_a_field_while_properties_are_open_does_not_partly_apply_the_draft() {
    let mut h = harness();
    select(&mut h, &["alpha", "beta"]);
    set_tooltip(&mut h, "New help");
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, Edit::DeleteField { name: "beta".into() });
    let before = bytes(&h);
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(bytes(&h), before);
    assert!(refused_with(&h, "no longer exists"));
}

#[test]
fn select_all_in_prepare_a_form_selects_only_the_current_pages_fields() {
    let mut h = harness();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(4);
    assert_eq!(h.state().views[0].session.prepare.names(), ["alpha", "beta", "consent"]);
    assert!(!doc(&h).dirty);
    h.state_mut().views[0].go_to_page(1);
    h.run_steps(4);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(4);
    assert_eq!(h.state().views[0].session.prepare.names(), ["gamma"]);
}

#[test]
fn fields_panel_selects_multiple_fields_and_opens_their_shared_properties() {
    let mut h = harness();
    h.get_by_label("alpha").click();
    h.run_steps(4);
    h.get_by_label("beta").click_modifiers(egui::Modifiers::SHIFT);
    h.run_steps(4);
    assert_eq!(h.state().views[0].session.prepare.names(), ["alpha", "beta"]);
    h.get_by_label("Properties…").click();
    h.run_steps(4);
    h.get_by_role_and_label(Role::Dialog, "Shared Field Properties");
    h.get_by_label("Cancel").click();
    h.run_steps(4);
    h.get_by_label("beta").click_modifiers(egui::Modifiers::COMMAND);
    h.run_steps(4);
    assert_eq!(h.state().views[0].session.prepare.names(), ["alpha"]);
}

#[test]
fn double_clicking_a_field_in_the_panel_opens_its_individual_properties() {
    let mut h = harness();
    for _ in 0..2 {
        h.get_by_label("beta").click();
        h.run_steps(4);
    }
    h.run_steps(4);
    h.get_by_role_and_label(Role::Dialog, "Text Field Properties");
    assert!(h.state().dialogs.is_open::<FieldPropsDialog>());
    let draft = h.state().dialogs.get::<FieldPropsDialog>().and_then(|d| d.json_fields()).expect("Field Properties is open");
    assert_eq!(draft["name"], "beta");
}

#[test]
fn selecting_several_widgets_of_one_radio_group_does_not_duplicate_field_edits() {
    let mut h = harness();
    for (export, x) in [("A", 100.0), ("B", 150.0)] {
        let ctx_h = h.ctx.clone();
        h.state_mut().apply_edit(
            &ctx_h,
            Edit::AddField {
                page: 0,
                rect: [x, 180.0, x + 14.0, 194.0],
                kind: NewField::Radio { group: Some("group".into()), export: export.into() },
                name: None,
            },
        );
    }
    let p = &mut h.state_mut().views[0].session.prepare;
    p.selected = Some(("group".into(), 0));
    p.also = vec![("group".into(), 1), ("alpha".into(), 0)];
    h.execute("form.field.properties");
    h.run_steps(4);
    assert_eq!(shared(&h)["fields"], serde_json::json!(["group", "alpha"]), "the group is listed once");
    h.get_by_role_and_label(Role::Group, "Tooltip:");
    set_tooltip(&mut h, "Shared help");
    h.get_by_label("OK").click();
    h.run_steps(4);
    let group = doc(&h).form.iter().find(|f| f.name == "group").unwrap();
    assert_eq!(group.widgets.len(), 2);
    assert_eq!(group.tooltip.as_deref(), Some("Shared help"));
}

#[test]
fn shared_properties_preserve_unchecked_changes_made_after_the_dialog_opened() {
    let mut h = harness();
    select(&mut h, &["alpha", "beta"]);
    h.get_by_label("Appearance").click();
    h.run_steps(4);
    h.get_by_role_and_label(Role::Group, "Line Thickness:").scroll_to_me();
    h.run_steps(4);
    click_gate(&mut h, "Line Thickness:");
    h.run_steps(4);
    type_in_thickness(&mut h, "2");
    assert_eq!(shared(&h)["width"]["value"], 2.0);
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(
        &ctx_h,
        Edit::SetFieldProps {
            name: "beta".into(),
            props: Box::new(FieldProps {
                appearance: Some(pdfcraft_engine::FieldLookPatch { fill: Some(Some([0.3, 0.7, 0.1])), ..Default::default() }),
                ..Default::default()
            }),
        },
    );
    let before = bytes(&h);
    h.run_steps(4);
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(doc(&h).field_look("beta").unwrap().fill, Some([0.3, 0.7, 0.1]));
    assert_eq!(doc(&h).field_look("beta").unwrap().width, 2.0);
    h.execute("edit.undo");
    h.run_steps(4);
    assert_eq!(bytes(&h), before);
}
