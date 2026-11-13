use super::{auto, ok, page_text, workdir};
use serde_json::json;

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
