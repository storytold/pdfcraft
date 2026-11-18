//! Exercise the public tools: stored values alone do not prove drawable PDF text.
use super::*;

const TURKISH: &str = "ç Ç ğ Ğ ı I i İ ö Ö ş Ş ü Ü â Â î Î û Û";

fn blank(a: &mut Automation) -> u64 {
    json(a, "doc_create", json!({"from":"blank","width":700,"height":500}))["doc"].as_u64().unwrap()
}

fn no_substitution(result: &Value) {
    assert!(result["warnings"].as_array().is_none_or(Vec::is_empty), "no Turkish glyph may be replaced: {result}");
}

fn extracted(a: &mut Automation, id: u64) -> String {
    json(a, "text_extract", json!({"doc":id}))["pages"][0]["text"].as_str().unwrap().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn save_reopen(a: &mut Automation, id: u64, file: &str, flatten: bool) -> u64 {
    json(a, "doc_save", json!({"doc":id,"path":file,"flatten_fill_sign":flatten}));
    json(a, "doc_open", json!({"path":file}))["doc"].as_u64().unwrap()
}

#[test]
fn turkish_field_value_and_flattened_appearance_are_both_exact() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let id = blank(&mut a);
    json(&mut a, "form_add_field", json!({"doc":id,"page":1,"type":"text","name":"Türkçe","rect":[30,80,670,150]}));
    let result = json(&mut a, "form_fill", json!({"doc":id,"values":{"Türkçe":TURKISH}}));
    no_substitution(&result);
    json(&mut a, "edit_undo", json!({"doc":id}));
    assert!(!json(&mut a, "form_fields", json!({"doc":id})).to_string().contains(TURKISH));
    json(&mut a, "edit_redo", json!({"doc":id}));
    let reopened = save_reopen(&mut a, id, "Türkçe form.pdf", false);
    let fields = json(&mut a, "form_fields", json!({"doc":reopened}));
    assert!(fields.to_string().contains(TURKISH), "{fields}");
    json(&mut a, "doc_flatten", json!({"doc":reopened,"comments":false,"fields":true}));
    assert_eq!(extracted(&mut a, reopened), TURKISH);
    let saved = save_reopen(&mut a, reopened, "Düz form.pdf", false);
    assert_eq!(extracted(&mut a, saved), TURKISH);
}

#[test]
fn turkish_textbox_and_callout_flatten_to_exact_page_text() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    for kind in ["textbox", "callout"] {
        let id = blank(&mut a);
        let mut args = json!({"doc":id,"page":1,"type":kind,"rect":[30,80,670,200],"contents":TURKISH,"font_size":16});
        if kind == "callout" {
            args["to"] = json!([20, 40]);
        }
        no_substitution(&json(&mut a, "comment_add", args));
        let reopened = save_reopen(&mut a, id, &format!("Türkçe {kind}.pdf"), false);
        json(&mut a, "doc_flatten", json!({"doc":reopened,"comments":true,"fields":false}));
        assert_eq!(extracted(&mut a, reopened), TURKISH, "{kind}");
        let saved = save_reopen(&mut a, reopened, &format!("Düz {kind}.pdf"), false);
        assert_eq!(extracted(&mut a, saved), TURKISH, "{kind}");
    }
}

#[test]
fn turkish_fill_sign_stays_exact_when_baked_on_save() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let id = blank(&mut a);
    no_substitution(&json(&mut a, "fill_sign_add", json!({"doc":id,"page":1,"type":"text","text":TURKISH,"at":[30,80],"font_size":16})));
    let reopened = save_reopen(&mut a, id, "Türkçe Fill Sign.pdf", true);
    assert_eq!(extracted(&mut a, reopened), TURKISH);
}

#[test]
fn turkish_header_footer_keeps_every_standard_family_and_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    for family in ["Helvetica", "Times", "Courier"] {
        let id = blank(&mut a);
        json(&mut a, "doc_header_footer", json!({"doc":id,"header_center":TURKISH,"font":family,"font_size":14}));
        assert_eq!(extracted(&mut a, id), TURKISH);
        let reopened = save_reopen(&mut a, id, &format!("Üst bilgi {family}.pdf"), false);
        assert_eq!(extracted(&mut a, reopened), TURKISH);
    }
}

#[test]
fn turkish_watermark_roundtrips_without_question_marks() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let id = blank(&mut a);
    json(&mut a, "doc_watermark", json!({"doc":id,"text":TURKISH,"rotation":0,"font_size":14}));
    assert_eq!(extracted(&mut a, id), TURKISH);
    json(&mut a, "edit_undo", json!({"doc":id}));
    assert_eq!(extracted(&mut a, id), "");
    json(&mut a, "edit_redo", json!({"doc":id}));
    assert_eq!(extracted(&mut a, id), TURKISH);
    let reopened = save_reopen(&mut a, id, "Türkçe filigran.pdf", false);
    assert_eq!(extracted(&mut a, reopened), TURKISH);
}

#[test]
fn turkish_multiline_choice_and_list_fields_flatten_with_their_display_text() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    for kind in ["text", "combo", "list"] {
        let id = blank(&mut a);
        let value = if kind == "text" { "İstanbul\nIğdır Şişli" } else { "İstanbul Iğdır Şişli" };
        no_substitution(&json(
            &mut a,
            "form_add_field",
            json!({"doc":id,"page":1,"type":kind,"name":"şehir","rect":[30,80,670,200],"multiline":true,"options":[value]}),
        ));
        no_substitution(&json(&mut a, "form_fill", json!({"doc":id,"values":{"şehir":value}})));
        let reopened = save_reopen(&mut a, id, &format!("şehir {kind}.pdf"), false);
        json(&mut a, "doc_flatten", json!({"doc":reopened,"comments":false,"fields":true}));
        assert_eq!(extracted(&mut a, reopened), value.split_whitespace().collect::<Vec<_>>().join(" "), "{kind}");
    }
}

#[test]
fn unsupported_combining_marks_keep_warnings_and_cannot_be_flattened() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    for field in [false, true] {
        let id = blank(&mut a);
        let value = "I\u{1ab0}";
        let result = if field {
            json(&mut a, "form_add_field", json!({"doc":id,"page":1,"type":"text","name":"NFD","rect":[30,80,670,200]}));
            json(&mut a, "form_fill", json!({"doc":id,"values":{"NFD":value}}))
        } else {
            json(&mut a, "comment_add", json!({"doc":id,"page":1,"type":"textbox","rect":[30,80,670,200],"contents":value}))
        };
        assert!(!result["warnings"].as_array().unwrap().is_empty());
        assert!(a.call("doc_flatten", &json!({"doc":id,"comments":!field,"fields":field})).is_err());
    }
}

#[test]
fn turkish_fill_sign_measures_tabs_as_the_spaces_it_draws() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let id = blank(&mut a);
    let value = "İstanbul\tIğdır\nPlain ASCII";
    no_substitution(&json(&mut a, "fill_sign_add", json!({"doc":id,"page":1,"type":"text","text":value,"at":[30,80],"font_size":16})));
    let reopened = save_reopen(&mut a, id, "Sekmeli Türkçe.pdf", true);
    assert_eq!(extracted(&mut a, reopened), "İstanbul Iğdır Plain ASCII");
}
