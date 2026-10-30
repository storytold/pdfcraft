//! Optional XFA rows through the same tool table used by CLI and MCP.
use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// Call the public automation table and require its JSON result for lifecycle assertions.
fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

/// Exercise the zero-row lifecycle through tools, checking rendered pixels as well as persisted values.
#[test]
fn zero_instances_tools_render_fill_undo_save_and_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    std::fs::write(dir.join("optional.pdf"), pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::optional_rows(0))).unwrap();
    let mut a = Automation::new().with_root(dir).unwrap().with_clock(|| 1_700_000_000);
    let id = tool(&mut a, "doc_open", json!({"path":"optional.pdf"}))["doc"].as_u64().unwrap();
    let rows = |a: &mut Automation, id| {
        tool(a, "form_fields", json!({"doc":id}))["fields"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["name"].as_str().is_some_and(|n| n.starts_with("entry")))
            .count()
    };
    assert_eq!(rows(&mut a, id), 0);
    let (_, _, empty) = a.render_pixels(id, 1, 1.0).unwrap();
    let click = tool(&mut a, "js_run", json!({"doc":id,"field":"add","script":""}));
    assert!(click["error"].is_null(), "{click:?}");
    assert_eq!(rows(&mut a, id), 1);
    tool(&mut a, "form_fill", json!({"doc":id,"values":{"entry":"saved row"}}));
    let (_, _, filled) = a.render_pixels(id, 1, 1.0).unwrap();
    assert_ne!(filled, empty);
    tool(&mut a, "js_run", json!({"doc":id,"field":"remove","script":""}));
    assert_eq!(rows(&mut a, id), 0);
    assert_eq!(a.render_pixels(id, 1, 1.0).unwrap().2, empty, "no ghost row or reserved space");
    tool(&mut a, "edit_undo", json!({"doc":id}));
    assert_eq!(a.render_pixels(id, 1, 1.0).unwrap().2, filled);
    tool(&mut a, "edit_redo", json!({"doc":id}));
    tool(&mut a, "doc_save", json!({"doc":id,"path":"saved.pdf"}));
    let reopened = tool(&mut a, "doc_open", json!({"path":"saved.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(rows(&mut a, reopened), 0);
    assert_eq!(a.render_pixels(reopened, 1, 1.0).unwrap().2, empty);
    tool(&mut a, "js_run", json!({"doc":reopened,"field":"add","script":""}));
    assert_eq!(rows(&mut a, reopened), 1);
}

/// #4744 through the CLI/MCP table: a script writes one initial row without losing its sibling;
/// explicitly removing both rows stays removed on save/reopen.
#[test]
fn initial_instances_survive_script_edits_and_explicit_removal_through_tools() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    let xml = pdfcraft_xfa::fixtures::optional_rows(2)
        .replace("<value><text>default</text></value>", "")
        .replace(
            "<field name=\"other\" w=\"200pt\" h=\"20pt\"><ui><textEdit/></ui></field>",
            r#"<field name="other" w="200pt" h="20pt"><ui><textEdit/></ui>
            <event activity="initialize"><script contentType="application/x-javascript">var first = xfa.resolveNode("form.row[0].entry"); if (first) first.rawValue = "first";</script></event></field>"#,
        );
    std::fs::write(dir.join("initial.pdf"), pdfcraft_xfa::fixtures::shell(&xml)).unwrap();
    let mut a = Automation::new().with_root(dir).unwrap().with_clock(|| 1_700_000_000);
    let rows = |a: &mut Automation, id| {
        tool(a, "form_fields", json!({"doc":id}))["fields"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["name"].as_str().is_some_and(|name| name.starts_with("entry")))
            .count()
    };
    let id = tool(&mut a, "doc_open", json!({"path":"initial.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(rows(&mut a, id), 2);
    tool(&mut a, "form_fill", json!({"doc":id,"values":{"other":"unrelated"}}));
    assert_eq!(rows(&mut a, id), 2);
    tool(&mut a, "doc_save", json!({"doc":id,"path":"initial-saved.pdf"}));
    let reopened = tool(&mut a, "doc_open", json!({"path":"initial-saved.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(rows(&mut a, reopened), 2);
    for expected in [1, 0] {
        tool(&mut a, "js_run", json!({"doc":reopened,"field":"remove","script":""}));
        assert_eq!(rows(&mut a, reopened), expected);
    }
    tool(&mut a, "doc_save", json!({"doc":reopened,"path":"removed.pdf"}));
    let removed = tool(&mut a, "doc_open", json!({"path":"removed.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(rows(&mut a, removed), 0);
}

/// Removing an optional row must not make a growable kept section indivisible or lose its
/// enclosing border/fill. Render both areas through edits, undo/redo and save/reopen.
#[test]
fn removing_a_row_from_a_kept_section_preserves_pagination_through_tools() {
    let temp = tempfile::tempdir().unwrap();
    let xml = r#"<template><subform name="form" layout="tb" w="300pt">
        <field name="remove" w="100pt" h="40pt"><ui><button/></ui>
            <event activity="click"><script contentType="application/x-javascript">box.section._optional.removeInstance(0);</script></event>
        </field>
        <field name="intro" w="300pt" h="260pt"><ui><textEdit/></ui></field>
        <subform name="box" layout="tb" w="300pt">
          <border><edge thickness="1pt"/><fill><color value="0,0,255"/></fill></border>
          <subform name="section" layout="lr-tb" w="300pt"><keep intact="contentArea"/>
            <field name="first" w="300pt" h="180pt"><ui><textEdit/></ui></field>
            <field name="second" w="300pt" h="180pt"><ui><textEdit/></ui></field>
            <field name="third" w="300pt" h="180pt"><ui><textEdit/></ui></field>
            <subform name="optional" w="300pt" h="240pt"><occur min="0" initial="1" max="1"/>
                <field name="extra" w="300pt" h="240pt"><ui><textEdit/></ui><value><text>Optional row</text></value></field>
            </subform>
          </subform>
        </subform>
        <field name="footer" w="300pt" h="300pt"><ui><textEdit/></ui><value><text>Footer</text></value></field>
    </subform></template>"#;
    std::fs::write(temp.path().join("kept.pdf"), pdfcraft_xfa::fixtures::shell(xml)).unwrap();
    let mut a = Automation::new().with_root(temp.path()).unwrap().with_clock(|| 1_700_000_000);
    let id = tool(&mut a, "doc_open", json!({"path":"kept.pdf"}))["doc"].as_u64().unwrap();
    let pages = |a: &mut Automation, id| tool(a, "doc_info", json!({"doc":id}))["pages"].as_array().unwrap().len();
    assert_eq!(pages(&mut a, id), 2);
    let before = [a.render_pixels(id, 1, 1.0).unwrap(), a.render_pixels(id, 2, 1.0).unwrap()];
    for (_, _, pixels) in &before {
        assert!(pixels.as_chunks::<4>().0.iter().filter(|p| p[..3] == [0, 0, 255]).count() > 1000, "parent fill must appear on both pages");
    }
    let result = tool(&mut a, "js_run", json!({"doc":id,"field":"remove","script":""}));
    assert!(result["error"].is_null(), "{result}");
    assert_eq!(pages(&mut a, id), 2, "removal must not introduce a keep-induced page");
    let after = [a.render_pixels(id, 1, 1.0).unwrap(), a.render_pixels(id, 2, 1.0).unwrap()];
    for (_, _, pixels) in &after {
        assert!(pixels.as_chunks::<4>().0.iter().filter(|p| p[..3] == [0, 0, 255]).count() > 1000, "removal must retain both parent fills");
    }
    assert_ne!(before, after);
    tool(&mut a, "edit_undo", json!({"doc":id}));
    assert_eq!([a.render_pixels(id, 1, 1.0).unwrap(), a.render_pixels(id, 2, 1.0).unwrap()], before);
    tool(&mut a, "edit_redo", json!({"doc":id}));
    assert_eq!(pages(&mut a, id), 2);
    assert_eq!([a.render_pixels(id, 1, 1.0).unwrap(), a.render_pixels(id, 2, 1.0).unwrap()], after);
    tool(&mut a, "doc_save", json!({"doc":id,"path":"saved-kept.pdf"}));
    let reopened = tool(&mut a, "doc_open", json!({"path":"saved-kept.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(pages(&mut a, reopened), 2);
    assert_eq!([a.render_pixels(reopened, 1, 1.0).unwrap(), a.render_pixels(reopened, 2, 1.0).unwrap()], after);
}
