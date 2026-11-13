use super::{auto, err, ok, page_text, workdir};
use serde_json::json;

#[test]
fn mixed_text_tools_add_retype_undo_delete_and_save_both_ways() {
    if !pdfcraft_fonts::CRAFT_FONTS.iter().any(|face| face.covers("Hans")) {
        return;
    }
    let dir = workdir("mixed-text-tools");
    let mut a = auto(&dir);
    let mixed = "Agpq 中文 简体汉字 联络 编辑 保存 重开 123";
    for full in [false, true] {
        let id = ok(&mut a, "doc_create", json!({"from":"blank","width":600,"height":400}))["doc"].as_u64().unwrap();
        ok(&mut a, "page_add_text", json!({"doc":id,"page":1,"text":"Agpq 中文 123","at":[40,50],"width":520,"size":16}));
        let original = a.call("page_render", &json!({"doc":id,"page":1,"dpi":96})).unwrap();
        ok(&mut a, "content_update", json!({"doc":id,"page":1,"index":1,"text":mixed}));
        assert_eq!(page_text(&mut a, id), [mixed]);
        ok(&mut a, "edit_undo", json!({"doc":id}));
        assert_eq!(a.call("page_render", &json!({"doc":id,"page":1,"dpi":96})).unwrap(), original);
        ok(&mut a, "edit_redo", json!({"doc":id}));
        let path = format!("mixed-{full}.pdf");
        ok(&mut a, "doc_save", json!({"doc":id,"path":path}));
        ok(&mut a, "doc_close", json!({"doc":id}));
        let re = ok(&mut a, "doc_open", json!({"path":path}))["doc"].as_u64().unwrap();
        assert_eq!(page_text(&mut a, re), [mixed]);
        ok(&mut a, "content_update", json!({"doc":re,"page":1,"index":1,"text":format!("{mixed} 测试")}));
        let render = a.call("page_render", &json!({"doc":re,"page":1,"dpi":96})).unwrap();
        let contents = ok(&mut a, "content_list", json!({"doc":re}));
        let error = err(&mut a, "content_update", json!({"doc":re,"page":1,"index":1,"text":"中文 \u{10ffff}"}));
        assert!(error.to_string().contains("10FFFF"), "{error}");
        assert_eq!(ok(&mut a, "content_list", json!({"doc":re})), contents);
        let saved = ok(&mut a, "doc_save", json!({"doc":re,"full":full}));
        assert_eq!(saved["incremental"], !full);
        ok(&mut a, "doc_close", json!({"doc":re}));
        let saved = ok(&mut a, "doc_open", json!({"path":path}))["doc"].as_u64().unwrap();
        assert_eq!(page_text(&mut a, saved), [format!("{mixed} 测试")]);
        assert_eq!(a.call("page_render", &json!({"doc":saved,"page":1,"dpi":96})).unwrap(), render);
        ok(&mut a, "content_delete", json!({"doc":saved,"page":1,"index":1}));
        assert_eq!(page_text(&mut a, saved), [""]);
        ok(&mut a, "edit_undo", json!({"doc":saved}));
        assert_eq!(a.call("page_render", &json!({"doc":saved,"page":1,"dpi":96})).unwrap(), render);
        ok(&mut a, "doc_close", json!({"doc":saved,"discard_changes":true}));
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn mixed_text_existing_paragraph_keeps_neighbour_and_reopens() {
    if !pdfcraft_fonts::CRAFT_FONTS.iter().any(|face| face.covers("Hans")) {
        return;
    }
    let dir = workdir("mixed-existing-text");
    let mut a = auto(&dir);
    let id = ok(&mut a, "doc_open", json!({"path":"a.pdf"}))["doc"].as_u64().unwrap();
    let mixed = "Agpq 中文 简体汉字 联络 编辑 保存 重开 123";
    ok(&mut a, "text_paragraphs", json!({"doc":id,"page":1}));
    ok(&mut a, "text_edit", json!({"doc":id,"page":1,"paragraph":1,"text":mixed,"width":180}));
    assert_eq!(page_text(&mut a, id)[1..], ["Page 2", "Page 3"]);
    ok(&mut a, "doc_save", json!({"doc":id,"path":"edited.pdf"}));
    ok(&mut a, "doc_close", json!({"doc":id}));
    let mut re = ok(&mut a, "doc_open", json!({"path":"edited.pdf"}))["doc"].as_u64().unwrap();
    let text = page_text(&mut a, re)[0].replace('\n', "");
    assert_eq!(text.split_whitespace().collect::<String>(), mixed.split_whitespace().collect::<String>());
    assert_eq!(page_text(&mut a, re)[1..], ["Page 2", "Page 3"]);
    for full in [false, true] {
        let replacement = format!("{mixed} {}", if full { "再次编辑" } else { "测试" });
        ok(&mut a, "text_paragraphs", json!({"doc":re,"page":1}));
        ok(&mut a, "text_edit", json!({"doc":re,"page":1,"paragraph":1,"text":replacement,"width":180}));
        let render = a.call("page_render", &json!({"doc":re,"page":1,"dpi":96})).unwrap();
        ok(&mut a, "edit_undo", json!({"doc":re}));
        ok(&mut a, "edit_redo", json!({"doc":re}));
        assert_eq!(a.call("page_render", &json!({"doc":re,"page":1,"dpi":96})).unwrap(), render);
        assert_eq!(ok(&mut a, "doc_save", json!({"doc":re,"full":full}))["incremental"], !full);
        ok(&mut a, "doc_close", json!({"doc":re}));
        re = ok(&mut a, "doc_open", json!({"path":"edited.pdf"}))["doc"].as_u64().unwrap();
        assert_eq!(page_text(&mut a, re)[0].split_whitespace().collect::<String>(), replacement.split_whitespace().collect::<String>());
        assert_eq!(page_text(&mut a, re)[1..], ["Page 2", "Page 3"]);
        assert_eq!(a.call("page_render", &json!({"doc":re,"page":1,"dpi":96})).unwrap(), render);
    }
    std::fs::remove_dir_all(dir).unwrap();
}
