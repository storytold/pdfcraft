//! End-to-end bulk Field Properties through the same tools used by CLI and MCP.

use pdfcraft_automation::{Automation, Content, ToolError};
use pdfcraft_engine::{DocId, Edit, FieldFont, FieldProps, FieldValue, NewField, Session, field_flags};
use serde_json::{Value, json};

fn call(a: &mut Automation, args: Value) -> Value {
    match a.call("form_set_props", &args).unwrap().remove(0) {
        Content::Json(v) => v,
        other => panic!("expected JSON: {other:?}"),
    }
}

fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(v) => v,
        other => panic!("expected JSON: {other:?}"),
    }
}

fn setup(test: &str) -> (Automation, u64, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("pdfcraft-form-bulk-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::new();
    let bytes = s.create_blank(300.0, 400.0, 2).unwrap();
    let id = s.open("form.pdf", None, bytes, None).unwrap();
    for (name, page, x) in [("alpha", 0, 30.0), ("beta", 1, 40.0)] {
        s.apply(id, Edit::AddField { page, rect: [x, 300.0, x + 180.0, 324.0], kind: NewField::Text { multiline: false }, name: Some(name.into()) })
            .unwrap();
        s.apply(id, Edit::SetFieldValue { name: name.into(), value: FieldValue::Text(name.into()) }).unwrap();
        let mut look = s.get(id).unwrap().field_look(name).unwrap();
        look.fill = Some(if name == "alpha" { [1.0, 0.9, 0.8] } else { [0.8, 0.9, 1.0] });
        look.font = if name == "alpha" { FieldFont::Courier } else { FieldFont::Times };
        s.apply(
            id,
            Edit::SetFieldProps {
                name: name.into(),
                props: Box::new(FieldProps { look: Some(look), tooltip: Some(name.into()), ..Default::default() }),
            },
        )
        .unwrap();
    }
    std::fs::write(dir.join("form.pdf"), s.save_bytes(id).unwrap().as_slice()).unwrap();
    // Saves stamp /ModDate from the clock: pin it, or two saves a second apart differ.
    let mut a = Automation::new().with_root(&dir).unwrap().with_clock(|| 1_700_000_000);
    let id = tool(&mut a, "doc_open", json!({"path": "form.pdf"}))["doc"].as_u64().unwrap();
    (a, id, dir)
}

fn bytes(a: &Automation, id: u64) -> Vec<u8> {
    a.session().save_bytes(DocId(id)).unwrap().as_ref().clone()
}

#[test]
fn bulk_properties_are_one_undo_step_preserve_other_looks_and_survive_save() {
    let (mut a, id, dir) = setup("roundtrip");
    let before = bytes(&a, id);
    let original =
        [a.session().get(DocId(id)).unwrap().field_look("alpha").unwrap(), a.session().get(DocId(id)).unwrap().field_look("beta").unwrap()];
    let out = call(&mut a, json!({"doc": id, "fields": ["alpha", "beta"], "required": true, "appearance": {"width": 4, "border": "#FF0000"}}));
    assert_eq!(out["fields"], json!(["alpha", "beta"]));
    let doc = a.session().get(DocId(id)).unwrap();
    assert_eq!(doc.can_undo(), Some("Change field properties"));
    for (name, look) in [("alpha", original[0]), ("beta", original[1])] {
        let changed = doc.field_look(name).unwrap();
        assert_eq!(changed.width, 4.0);
        assert_eq!(changed.border, Some([1.0, 0.0, 0.0]));
        assert_eq!((changed.fill, changed.font, changed.text), (look.fill, look.font, look.text));
        let f = doc.form.iter().find(|f| f.name == name).unwrap();
        assert!(f.has(field_flags::REQUIRED));
        assert_eq!(f.value, [name]);
        assert_eq!(f.tooltip.as_deref(), Some(name));
    }
    tool(&mut a, "edit_undo", json!({"doc": id}));
    assert_eq!(bytes(&a, id), before);
    tool(&mut a, "edit_redo", json!({"doc": id}));
    tool(&mut a, "doc_save", json!({"doc": id, "path": "saved.pdf"}));
    let reopened = tool(&mut a, "doc_open", json!({"path": "saved.pdf"}))["doc"].as_u64().unwrap();
    let doc = a.session().get(DocId(reopened)).unwrap();
    assert_eq!(doc.field_look("beta").unwrap().font, original[1].font);
    assert_eq!(doc.field_look("beta").unwrap().width, 4.0);
    assert_eq!(doc.form.iter().find(|f| f.name == "beta").unwrap().value, ["beta"]);
    let image = a.call("page_render", &json!({"doc": reopened, "page": 2, "dpi": 72})).unwrap();
    assert!(image.iter().any(|c| matches!(c, Content::Png { width: 300, height: 400, .. })));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn bulk_property_arguments_are_strict_and_fail_without_any_edit() {
    let (mut a, id, dir) = setup("invalid");
    let before = bytes(&a, id);
    let cases = [
        json!({"doc": id, "required": true}),
        json!({"doc": id, "field": "alpha", "fields": ["beta"], "required": true}),
        json!({"doc": id, "fields": [], "required": true}),
        json!({"doc": id, "fields": ["alpha", "alpha"], "required": true}),
        json!({"doc": id, "fields": ["alpha", 7], "required": true}),
        json!({"doc": id, "fields": ["alpha", "beta"], "name": "same"}),
        json!({"doc": id, "fields": ["alpha", "beta"], "rotation": 90}),
        json!({"doc": id, "fields": ["alpha", "beta"], "rect": [0, 0, 20, 20]}),
        json!({"doc": id, "fields": ["alpha", "beta"]}),
        json!({"doc": id, "fields": ["alpha", "beta"], "required": "yes"}),
        json!({"doc": id, "fields": ["alpha", "beta"], "appearance": {"style": "invalid"}}),
        json!({"doc": id, "fields": ["alpha", "beta"], "appearance": {"width": -1}}),
        json!({"doc": id, "fields": ["alpha", "beta"], "appearance": {"width": "thick"}}),
        json!({"doc": id, "fields": ["alpha", "beta"], "appearance": {"style": 3}}),
        json!({"doc": id, "fields": ["alpha", "beta"], "appearance": {"font": true}}),
        json!({"doc": id, "fields": ["alpha", "beta"], "appearance": {"unknown": true}}),
        json!({"doc": id, "fields": ["alpha", "beta"], "requred": true}),
        json!({"doc": id, "fields": vec!["alpha"; 1001], "required": true}),
    ];
    for args in cases {
        assert!(matches!(a.call("form_set_props", &args), Err(ToolError::InvalidArgs(_))), "{args}");
        assert_eq!(bytes(&a, id), before, "{args}");
    }
    assert!(matches!(a.call("form_set_props", &json!({"doc": id, "fields": ["alpha", "missing"], "required": true})), Err(ToolError::Failed(_))));
    assert_eq!(bytes(&a, id), before);
    assert!(a.session().get(DocId(id)).unwrap().can_undo().is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_locked_last_field_rolls_back_earlier_changes_and_unlocks_with_the_batch() {
    let (mut a, id, dir) = setup("locked");
    call(&mut a, json!({"doc": id, "field": "beta", "locked": true}));
    let before = bytes(&a, id);
    assert!(matches!(
        a.call("form_set_props", &json!({"doc": id, "fields": ["alpha", "beta"], "tooltip": "Shared help"})),
        Err(ToolError::Failed(_))
    ));
    assert_eq!(bytes(&a, id), before);
    assert_eq!(a.session().get(DocId(id)).unwrap().can_undo(), Some("Change field properties"));
    call(&mut a, json!({"doc": id, "fields": ["alpha", "beta"], "locked": false, "tooltip": "Shared help"}));
    assert!(a.session().get(DocId(id)).unwrap().form.iter().all(|f| !f.locked() && f.tooltip.as_deref() == Some("Shared help")));
    tool(&mut a, "edit_undo", json!({"doc": id}));
    assert_eq!(bytes(&a, id), before);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn bulk_options_apply_per_field_and_refuse_an_invalid_comb_atomically() {
    let (mut a, id, dir) = setup("options");
    let before = bytes(&a, id);
    assert!(matches!(a.call("form_set_props", &json!({"doc": id, "fields": ["alpha", "beta"], "flags": {"comb": true}})), Err(ToolError::Failed(_))));
    assert_eq!(bytes(&a, id), before);
    call(&mut a, json!({"doc": id, "fields": ["alpha", "beta"], "max_length": 20, "align": "center", "flags": {"comb": true, "spell_check": false}}));
    let doc = a.session().get(DocId(id)).unwrap();
    assert!(doc.form.iter().all(|f| f.max_len == Some(20) && f.quadding == 1 && f.has(field_flags::COMB) && f.has(field_flags::DO_NOT_SPELL_CHECK)));
    assert_eq!(doc.form.iter().map(|f| f.value[0].as_str()).collect::<Vec<_>>(), ["alpha", "beta"]);
    tool(&mut a, "edit_undo", json!({"doc": id}));
    assert_eq!(bytes(&a, id), before);
    std::fs::remove_dir_all(dir).unwrap();
}
