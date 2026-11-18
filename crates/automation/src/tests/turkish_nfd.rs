//! Authoring must shape decomposed Turkish without rewriting the user's Unicode.
use super::*;

const NFD: &str = "c\u{327} C\u{327} g\u{306} G\u{306} ı I i I\u{307} o\u{308} O\u{308} s\u{327} S\u{327} u\u{308} U\u{308} a\u{302} A\u{302} i\u{302} I\u{302} u\u{302} U\u{302}";

fn blank(a: &mut Automation) -> u64 {
    json(a, "doc_create", json!({"from":"blank","width":800,"height":600}))["doc"].as_u64().unwrap()
}

fn text(a: &mut Automation, id: u64) -> String {
    json(a, "text_extract", json!({"doc":id}))["pages"][0]["text"].as_str().unwrap().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn no_warning(value: Value) {
    assert!(value["warnings"].as_array().is_none_or(Vec::is_empty), "{value}");
}

fn reopen(a: &mut Automation, id: u64, file: &str, flatten_fill_sign: bool) -> u64 {
    json(a, "doc_save", json!({"doc":id,"path":file,"flatten_fill_sign":flatten_fill_sign}));
    json(a, "doc_open", json!({"path":file}))["doc"].as_u64().unwrap()
}

#[test]
fn nfd_added_text_retypes_undoes_searches_and_reopens_without_normalizing() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let id = blank(&mut a);
    json(&mut a, "page_add_text", json!({"doc":id,"page":1,"text":"Neighbor ASCII","at":[30,35],"size":16}));
    json(&mut a, "page_add_text", json!({"doc":id,"page":1,"text":NFD,"at":[30,100],"width":740,"size":16}));
    assert_eq!(text(&mut a, id), format!("Neighbor ASCII {NFD}"));
    let before = a.render_pixels(id, 1, 1.0).unwrap();
    let updated = "I\u{307}stanbul C\u{327}ag\u{306}rı S\u{327}is\u{327}li";
    json(&mut a, "content_update", json!({"doc":id,"page":1,"index":2,"text":updated}));
    assert_eq!(text(&mut a, id), format!("Neighbor ASCII {updated}"));
    let found = json(&mut a, "text_find", json!({"doc":id,"query":"Şişli","language":"tr","whole_words":true}));
    assert_eq!(found["count"], 1, "{found}");
    assert_eq!(found["matches"][0]["text"], "S\u{327}is\u{327}li");
    json(&mut a, "edit_undo", json!({"doc":id}));
    assert_eq!(a.render_pixels(id, 1, 1.0).unwrap(), before);
    assert_eq!(text(&mut a, id), format!("Neighbor ASCII {NFD}"));
    json(&mut a, "edit_redo", json!({"doc":id}));
    let after = a.render_pixels(id, 1, 1.0).unwrap();
    let re = reopen(&mut a, id, "Ayrışık – Çağrı.pdf", false);
    assert_eq!(text(&mut a, re), format!("Neighbor ASCII {updated}"));
    assert_eq!(a.render_pixels(re, 1, 1.0).unwrap(), after);
    assert!(json(&mut a, "content_list", json!({"doc":re})).to_string().contains(updated));
}

#[test]
fn nfd_field_preserves_logical_value_and_flattened_unicode_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let id = blank(&mut a);
    json(&mut a, "form_add_field", json!({"doc":id,"page":1,"type":"text","name":"NFD","rect":[30,80,770,180]}));
    no_warning(json(&mut a, "form_fill", json!({"doc":id,"values":{"NFD":NFD}})));
    let re = reopen(&mut a, id, "Ayrışık form.pdf", false);
    assert!(json(&mut a, "form_fields", json!({"doc":re})).to_string().contains(NFD));
    json(&mut a, "doc_flatten", json!({"doc":re,"comments":false,"fields":true}));
    assert_eq!(text(&mut a, re), NFD);
    let saved = reopen(&mut a, re, "Ayrışık düz form.pdf", false);
    assert_eq!(text(&mut a, saved), NFD);
}

#[test]
fn nfd_textbox_and_callout_keep_contents_and_flatten_without_substitution() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    for kind in ["textbox", "callout"] {
        let id = blank(&mut a);
        let mut args = json!({"doc":id,"page":1,"type":kind,"rect":[30,80,770,220],"contents":NFD,"font_size":16});
        if kind == "callout" {
            args["to"] = json!([20, 40]);
        }
        no_warning(json(&mut a, "comment_add", args));
        let re = reopen(&mut a, id, &format!("Ayrışık {kind}.pdf"), false);
        assert!(json(&mut a, "comment_list", json!({"doc":re})).to_string().contains(NFD));
        json(&mut a, "doc_flatten", json!({"doc":re,"comments":true,"fields":false}));
        assert_eq!(text(&mut a, re), NFD, "{kind}");
        let saved = reopen(&mut a, re, &format!("Ayrışık düz {kind}.pdf"), false);
        assert_eq!(text(&mut a, saved), NFD, "{kind}");
    }
}

#[test]
fn nfd_fill_sign_keeps_original_text_when_baked() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let id = blank(&mut a);
    no_warning(json(&mut a, "fill_sign_add", json!({"doc":id,"page":1,"type":"text","text":NFD,"at":[30,80],"font_size":16})));
    let re = reopen(&mut a, id, "Ayrışık Fill Sign.pdf", true);
    assert_eq!(text(&mut a, re), NFD);
}

#[test]
fn nfd_header_footer_keeps_all_three_standard_families_and_unicode() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    for family in ["Helvetica", "Times", "Courier"] {
        let id = blank(&mut a);
        json(&mut a, "doc_header_footer", json!({"doc":id,"header_center":NFD,"font":family,"font_size":14}));
        assert_eq!(text(&mut a, id), NFD);
        let re = reopen(&mut a, id, &format!("Ayrışık üst bilgi {family}.pdf"), false);
        assert_eq!(text(&mut a, re), NFD);
    }
}

#[test]
fn nfd_watermark_undo_redo_and_roundtrip_keep_unicode() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let id = blank(&mut a);
    json(&mut a, "doc_watermark", json!({"doc":id,"text":NFD,"rotation":0,"font_size":14}));
    assert_eq!(text(&mut a, id), NFD);
    json(&mut a, "edit_undo", json!({"doc":id}));
    assert_eq!(text(&mut a, id), "");
    json(&mut a, "edit_redo", json!({"doc":id}));
    let re = reopen(&mut a, id, "Ayrışık filigran.pdf", false);
    assert_eq!(text(&mut a, re), NFD);
}
