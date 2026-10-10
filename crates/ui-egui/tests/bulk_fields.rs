//! Shared properties and multi-selection through the real Prepare-a-form shell.

use egui_kittest::{Harness, kittest::Queryable};
use pdfcraft_engine::{Edit, FieldFont, FieldProps, FieldValue, NewField, Session, field_flags};
use pdfcraft_ui_egui::{Dialog, PdfCraftApp, bulk_fields::Tab};

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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(|_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        // Saves stamp /ModDate from the clock; pinned, two saves compare equal across a second.
        app.session = std::mem::take(&mut app.session).with_clock(|| 1_700_000_000);
        app.open_bytes("form.pdf", None, fixture()).unwrap();
        app.set_option("zoom", "150").unwrap();
        app.execute("form.prepare");
        app
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
    let p = &mut h.state_mut().views[0].prepare;
    p.selected = names.first().map(|n| (n.to_string(), 0));
    p.also = names.iter().skip(1).map(|n| (n.to_string(), 0)).collect();
    h.state_mut().execute("form.field.properties");
    h.run_steps(4);
}

#[test]
fn shared_properties_preserve_mixed_values_and_undo_the_whole_change() {
    let mut h = harness();
    let before = bytes(&h);
    let original = [doc(&h).field_look("alpha").unwrap(), doc(&h).field_look("beta").unwrap()];
    select(&mut h, &["alpha", "beta"]);
    h.get_by_label("Shared Field Properties");
    h.get_by_label("2 fields selected");
    assert!(h.state().bulk_field_props.as_ref().unwrap().required.mixed);
    h.get_by_label("Required").click();
    h.run_steps(4);
    h.state_mut().bulk_field_props.as_mut().unwrap().required.value = true;
    h.get_by_label("Appearance").click();
    h.run_steps(4);
    h.get_by_label("Line Thickness:").click();
    h.run_steps(4);
    h.state_mut().bulk_field_props.as_mut().unwrap().width.value = 4.0;
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert!(h.state().dialog.is_none());
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
    assert_eq!(h.state().views[0].prepare.names(), ["alpha", "beta"]);
    h.state_mut().execute("edit.undo");
    h.run_steps(4);
    assert_eq!(bytes(&h), before);
    h.state_mut().execute("edit.redo");
    h.run_steps(4);
    assert_eq!(doc(&h).field_look("beta").unwrap().width, 4.0);
    // The regenerated look also survives a real Save/open round trip.
    let mut s = Session::new();
    let id = s.open("saved.pdf", None, std::sync::Arc::new(bytes(&h)), None).unwrap();
    assert_eq!(s.get(id).unwrap().field_look("beta").unwrap().fill, original[1].fill);
    assert_eq!(s.get(id).unwrap().form.iter().find(|f| f.name == "beta").unwrap().value, ["Lovelace"]);
}

#[test]
fn visiting_shared_property_tabs_without_changes_does_not_dirty_the_document() {
    let mut h = harness();
    let before = bytes(&h);
    select(&mut h, &["alpha", "beta"]);
    for tab in ["Appearance", "Options", "General"] {
        h.get_by_label(tab).click();
        h.run_steps(4);
    }
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(bytes(&h), before);
    assert!(!doc(&h).dirty);
    assert!(doc(&h).can_undo().is_none());
}

#[test]
fn cancelling_shared_properties_discards_only_the_draft() {
    let mut h = harness();
    let before = bytes(&h);
    select(&mut h, &["alpha", "beta", "consent"]);
    let d = h.state_mut().bulk_field_props.as_mut().unwrap();
    d.fill.apply = true;
    d.fill.value = None;
    h.get_by_label("Cancel").click();
    h.run_steps(4);
    assert_eq!(bytes(&h), before);
    assert!(h.state().bulk_field_props.is_none());
    assert_eq!(h.state().views[0].prepare.names(), ["alpha", "beta", "consent"]);
}

#[test]
fn locked_field_refuses_the_entire_batch_and_keeps_the_draft_for_correction() {
    let mut h = harness();
    h.state_mut().apply_edit(Edit::SetFieldProps { name: "beta".into(), props: Box::new(FieldProps { locked: Some(true), ..Default::default() }) });
    let before = bytes(&h);
    select(&mut h, &["alpha", "beta"]);
    let d = h.state_mut().bulk_field_props.as_mut().unwrap();
    d.tooltip.apply = true;
    d.tooltip.value = "Shared help".into();
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(bytes(&h), before, "alpha must not be committed before beta refuses");
    assert_eq!(h.state().dialog, Some(Dialog::BulkFieldProps));
    assert!(h.state().bulk_field_props.as_ref().unwrap().error.as_ref().unwrap().contains("locked"));
    let d = h.state_mut().bulk_field_props.as_mut().unwrap();
    d.locked.apply = true;
    d.locked.value = false;
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert!(h.state().dialog.is_none());
    assert!(
        doc(&h)
            .form
            .iter()
            .filter(|f| ["alpha", "beta"].contains(&f.name.as_str()))
            .all(|f| !f.locked() && f.tooltip.as_deref() == Some("Shared help"))
    );
    h.state_mut().execute("edit.undo");
    h.run_steps(4);
    assert_eq!(bytes(&h), before, "unlock and changes undo together");
}

#[test]
fn shared_properties_cannot_apply_to_a_different_active_document() {
    let mut h = harness();
    let before = bytes(&h);
    select(&mut h, &["alpha", "beta"]);
    let d = h.state_mut().bulk_field_props.as_mut().unwrap();
    d.required.apply = true;
    d.required.value = true;
    h.state_mut().open_bytes("other.pdf", None, fixture()).unwrap();
    h.run_steps(4);
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(bytes(&h), before);
    let id = h.state().views[1].id;
    assert!(!h.state().session.get(id).unwrap().dirty);
    assert!(h.state().bulk_field_props.as_ref().unwrap().error.as_ref().unwrap().contains("document changed"));
}

#[test]
fn deleting_a_field_while_properties_are_open_does_not_partly_apply_the_draft() {
    let mut h = harness();
    select(&mut h, &["alpha", "beta"]);
    let d = h.state_mut().bulk_field_props.as_mut().unwrap();
    d.tooltip.apply = true;
    d.tooltip.value = "New help".into();
    h.state_mut().apply_edit(Edit::DeleteField { name: "beta".into() });
    let before = bytes(&h);
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(bytes(&h), before);
    assert!(h.state().bulk_field_props.as_ref().unwrap().error.as_ref().unwrap().contains("no longer exists"));
}

#[test]
fn select_all_in_prepare_a_form_selects_only_the_current_pages_fields() {
    let mut h = harness();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(4);
    assert_eq!(h.state().views[0].prepare.names(), ["alpha", "beta", "consent"]);
    assert!(!doc(&h).dirty);
    h.state_mut().views[0].go_to_page(1);
    h.run_steps(4);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(4);
    assert_eq!(h.state().views[0].prepare.names(), ["gamma"]);
}

#[test]
fn fields_panel_selects_multiple_fields_and_opens_their_shared_properties() {
    let mut h = harness();
    h.get_by_label("alpha").click();
    h.run_steps(4);
    h.get_by_label("beta").click_modifiers(egui::Modifiers::SHIFT);
    h.run_steps(4);
    assert_eq!(h.state().views[0].prepare.names(), ["alpha", "beta"]);
    h.get_by_label("Properties…").click();
    h.run_steps(4);
    h.get_by_label("Shared Field Properties");
    h.get_by_label("Cancel").click();
    h.run_steps(4);
    h.get_by_label("beta").click_modifiers(egui::Modifiers::COMMAND);
    h.run_steps(4);
    assert_eq!(h.state().views[0].prepare.names(), ["alpha"]);
}

#[test]
fn double_clicking_a_field_in_the_panel_opens_its_individual_properties() {
    let mut h = harness();
    for _ in 0..2 {
        h.get_by_label("beta").click();
        h.run_steps(4);
    }
    h.run_steps(4);
    h.get_by_label("Text Field Properties");
    assert_eq!(h.state().field_props.as_ref().unwrap().field, "beta");
}

#[test]
fn dragging_an_empty_page_area_selects_fields_without_moving_them() {
    let mut h = harness();
    let before = bytes(&h);
    let r = h.state().views[0].page_screen_rect(0).unwrap();
    let scale = r.width() / 300.0;
    let a = r.min + egui::vec2(30.0, 75.0) * scale;
    let b = r.min + egui::vec2(240.0, 165.0) * scale;
    h.hover_at(a);
    h.run_steps(4);
    h.drag_at(a);
    h.run_steps(4);
    for step in 1..=4 {
        h.hover_at(a + (b - a) * (step as f32 / 4.0));
        h.run_steps(4);
    }
    h.drop_at(b);
    h.run_steps(4);
    assert_eq!(h.state().views[0].prepare.names(), ["alpha", "beta"]);
    assert_eq!(bytes(&h), before);
}

#[test]
fn selecting_several_widgets_of_one_radio_group_does_not_duplicate_field_edits() {
    let mut h = harness();
    for (export, x) in [("A", 100.0), ("B", 150.0)] {
        h.state_mut().apply_edit(Edit::AddField {
            page: 0,
            rect: [x, 180.0, x + 14.0, 194.0],
            kind: NewField::Radio { group: Some("group".into()), export: export.into() },
            name: None,
        });
    }
    let p = &mut h.state_mut().views[0].prepare;
    p.selected = Some(("group".into(), 0));
    p.also = vec![("group".into(), 1), ("alpha".into(), 0)];
    h.state_mut().execute("form.field.properties");
    h.run_steps(4);
    let d = h.state().bulk_field_props.as_ref().unwrap();
    assert_eq!(d.names, ["group", "alpha"]);
    assert_eq!(d.tab, Tab::General);
    let d = h.state_mut().bulk_field_props.as_mut().unwrap();
    d.tooltip.apply = true;
    d.tooltip.value = "Shared help".into();
    let edits = d.clone().edits(doc(&h)).unwrap();
    assert_eq!(edits.len(), 2);
    h.get_by_label("OK").click();
    h.run_steps(4);
    let group = doc(&h).form.iter().find(|f| f.name == "group").unwrap();
    assert_eq!(group.widgets.len(), 2);
    assert_eq!(group.tooltip.as_deref(), Some("Shared help"));
}

#[test]
fn escape_cancels_a_selection_rectangle_and_restores_the_previous_selection() {
    let mut h = harness();
    h.state_mut().views[0].prepare.selected = Some(("consent".into(), 0));
    let before = bytes(&h);
    let r = h.state().views[0].page_screen_rect(0).unwrap();
    let scale = r.width() / 300.0;
    let a = r.min + egui::vec2(30.0, 75.0) * scale;
    let b = r.min + egui::vec2(240.0, 165.0) * scale;
    h.hover_at(a);
    h.run_steps(4);
    h.drag_at(a);
    h.run_steps(4);
    h.hover_at(b);
    h.run_steps(4);
    h.key_press(egui::Key::Escape);
    h.run_steps(4);
    h.drop_at(b);
    h.run_steps(4);
    assert_eq!(h.state().views[0].prepare.names(), ["consent"]);
    assert_eq!(bytes(&h), before);
}

#[test]
fn shared_properties_preserve_unchecked_changes_made_after_the_dialog_opened() {
    let mut h = harness();
    select(&mut h, &["alpha", "beta"]);
    let d = h.state_mut().bulk_field_props.as_mut().unwrap();
    d.width.apply = true;
    d.width.value = 2.0;
    h.state_mut().apply_edit(Edit::SetFieldProps {
        name: "beta".into(),
        props: Box::new(FieldProps {
            appearance: Some(pdfcraft_engine::FieldLookPatch { fill: Some(Some([0.3, 0.7, 0.1])), ..Default::default() }),
            ..Default::default()
        }),
    });
    let before = bytes(&h);
    h.run_steps(4);
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(doc(&h).field_look("beta").unwrap().fill, Some([0.3, 0.7, 0.1]));
    assert_eq!(doc(&h).field_look("beta").unwrap().width, 2.0);
    h.state_mut().execute("edit.undo");
    h.run_steps(4);
    assert_eq!(bytes(&h), before);
}
