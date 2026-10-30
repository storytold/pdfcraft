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

/// Border policy reaches ordinary PDF output without changing pagination, and survives
/// field edits, undo/redo and save/reopen through the same API as the UI and CLI.
#[test]
fn horizontal_border_fragments_keep_policy_through_save_and_undo() {
    for policy in ["close", "open"] {
        let temp = tempfile::tempdir().unwrap();
        let xml = format!(
            r#"<template><subform name="form" layout="tb" w="100pt">
            <pageSet><pageArea><contentArea x="10pt" y="10pt" w="100pt" h="60pt"/>
                <medium short="140pt" long="90pt"/></pageArea></pageSet>
            <field name="intro" w="80pt" h="40pt"><ui><textEdit/></ui></field>
            <subform name="section" layout="lr-tb" w="100pt">
                <border break="{policy}"><edge thickness="2pt"><color value="255,0,0"/></edge>
                    <fill><color value="0,0,255"/></fill></border>
                <field name="first" w="80pt" h="20pt"><ui><textEdit/></ui></field>
                <field name="second" w="80pt" h="20pt"><ui><textEdit/></ui></field>
            </subform>
        </subform></template>"#
        );
        std::fs::write(temp.path().join("border.pdf"), pdfcraft_xfa::fixtures::shell(&xml)).unwrap();
        let mut a = Automation::new().with_root(temp.path()).unwrap().with_clock(|| 1_700_000_000);
        let id = tool(&mut a, "doc_open", json!({"path":"border.pdf"}))["doc"].as_u64().unwrap();
        let snapshot = |a: &mut Automation, id| {
            assert_eq!(tool(a, "doc_info", json!({"doc":id}))["pages"].as_array().unwrap().len(), 2);
            [a.render_pixels(id, 1, 1.0).unwrap(), a.render_pixels(id, 2, 1.0).unwrap()]
        };
        let before = snapshot(&mut a, id);
        for (page, y) in [(0, 69), (1, 10)] {
            let (width, _, pixels) = &before[page];
            for x in 95..105 {
                let at = (y * *width as usize + x) * 4;
                let expected = if policy == "close" { [255, 0, 0] } else { [0, 0, 255] };
                assert_eq!(&pixels[at..at + 3], &expected, "{policy}, page {page}, at {x},{y}");
            }
        }
        tool(&mut a, "form_fill", json!({"doc":id,"values":{"first":"Edited"}}));
        let edited = snapshot(&mut a, id);
        assert!(edited != before);
        tool(&mut a, "edit_undo", json!({"doc":id}));
        assert!(snapshot(&mut a, id) == before, "{policy}: undo changed the original rendering");
        tool(&mut a, "edit_redo", json!({"doc":id}));
        assert!(snapshot(&mut a, id) == edited, "{policy}: redo changed the edited rendering");
        tool(&mut a, "doc_save", json!({"doc":id,"path":"saved-border.pdf"}));
        let reopened = tool(&mut a, "doc_open", json!({"path":"saved-border.pdf"}))["doc"].as_u64().unwrap();
        assert!(snapshot(&mut a, reopened) == edited, "{policy}: reopening changed the rendering");
    }
}

/// The same public lifecycle in every supported container strategy. Positioned copies
/// deliberately overlap; every copy must still be an independently editable field.
#[test]
fn repeated_instances_survive_the_tool_lifecycle_in_every_layout() {
    for mode in ["position", "area", "tb", "lr-tb", "rl-tb", "table", "row"] {
        let temp = tempfile::tempdir().unwrap();
        let child_layout = if mode == "table" { "row" } else { "tb" };
        let repeated = format!(
            r#"<subform name="rep" layout="{child_layout}" w="80pt"><occur min="0" initial="0" max="3"/>
            <field name="entry" w="80pt"><ui><textEdit multiLine="1"/></ui><value><text>default</text></value></field></subform>"#
        );
        let container = if mode == "area" {
            format!(r#"<area name="box">{repeated}</area>"#)
        } else {
            format!(r#"<subform name="box" layout="{mode}" w="300pt" columnWidths="80pt 80pt 80pt">{repeated}</subform>"#)
        };
        let (container, manager) = if mode == "row" {
            (format!(r#"<subform name="table" layout="table" w="300pt" columnWidths="80pt 80pt 80pt">{container}</subform>"#), "table.box._rep")
        } else {
            (container, "box._rep")
        };
        let xml = format!(
            r#"<template><subform name="form" layout="tb" w="300pt">{container}
            <field name="add" w="80pt" h="20pt"><ui><button/></ui><caption><value><text>Add</text></value></caption>
                <event activity="click"><script contentType="application/x-javascript">{manager}.addInstance(1);</script></event></field>
            <field name="remove" w="80pt" h="20pt"><ui><button/></ui><caption><value><text>Remove</text></value></caption>
                <event activity="click"><script contentType="application/x-javascript">{manager}.removeInstance(0);</script></event></field>
            </subform></template>"#
        );
        std::fs::write(temp.path().join("instances.pdf"), pdfcraft_xfa::fixtures::shell(&xml)).unwrap();
        let mut a = Automation::new().with_root(temp.path()).unwrap().with_clock(|| 1_700_000_000);
        let id = tool(&mut a, "doc_open", json!({"path":"instances.pdf"}))["doc"].as_u64().unwrap();
        let fields = |a: &mut Automation, id| tool(a, "form_fields", json!({"doc":id}))["fields"].as_array().unwrap().clone();
        let entries =
            |a: &mut Automation, id| fields(a, id).into_iter().filter(|f| f["name"].as_str().unwrap().starts_with("entry")).collect::<Vec<_>>();
        let empty = (fields(&mut a, id), a.render_pixels(id, 1, 1.0).unwrap());
        assert!(entries(&mut a, id).is_empty(), "{mode}");
        let values = ["first", "second\nline two\nline three\nline four\nline five\nline six\nline seven", "third\nlast line"];
        for (i, value) in values.iter().enumerate() {
            let click = tool(&mut a, "js_run", json!({"doc":id,"field":"add","script":""}));
            assert!(click["error"].is_null(), "{mode}: {click}");
            let current = entries(&mut a, id);
            assert_eq!(current.len(), i + 1, "{mode}");
            let name = current[i]["name"].as_str().unwrap();
            tool(&mut a, "form_fill", json!({"doc":id,"values":{name: value}}));
        }
        let filled = entries(&mut a, id);
        assert_eq!(filled.iter().map(|f| f["value"].as_str().unwrap().to_owned()).collect::<Vec<_>>(), values, "{mode}");
        let height = |field: &Value| field["rect"][3].as_f64().unwrap() - field["rect"][1].as_f64().unwrap();
        assert!(height(&filled[1]) > height(&filled[0]) + 20.0, "{mode}: editing must grow the later instance independently");
        let populated = (fields(&mut a, id), a.render_pixels(id, 1, 1.0).unwrap());
        assert!(populated.1 != empty.1, "{mode}: filled form must render differently");
        // A fourth add must respect max without changing values or geometry.
        tool(&mut a, "js_run", json!({"doc":id,"field":"add","script":""}));
        assert_eq!(fields(&mut a, id), populated.0, "{mode}");
        tool(&mut a, "doc_save", json!({"doc":id,"path":"populated.pdf"}));
        let reopened = tool(&mut a, "doc_open", json!({"path":"populated.pdf"}))["doc"].as_u64().unwrap();
        assert!(
            (fields(&mut a, reopened), a.render_pixels(reopened, 1, 1.0).unwrap()) == populated,
            "{mode}: populated reopen changed fields/pixels"
        );
        for removed in 1..=3 {
            let before = (fields(&mut a, reopened), a.render_pixels(reopened, 1, 1.0).unwrap());
            let click = tool(&mut a, "js_run", json!({"doc":reopened,"field":"remove","script":""}));
            assert!(click["error"].is_null(), "{mode}: {click}");
            let remaining = entries(&mut a, reopened);
            assert_eq!(remaining.iter().map(|f| f["value"].as_str().unwrap().to_owned()).collect::<Vec<_>>(), values[removed..], "{mode}");
            let after = (fields(&mut a, reopened), a.render_pixels(reopened, 1, 1.0).unwrap());
            tool(&mut a, "edit_undo", json!({"doc":reopened}));
            assert!((fields(&mut a, reopened), a.render_pixels(reopened, 1, 1.0).unwrap()) == before, "{mode}: undo changed fields/pixels");
            tool(&mut a, "edit_redo", json!({"doc":reopened}));
            assert!((fields(&mut a, reopened), a.render_pixels(reopened, 1, 1.0).unwrap()) == after, "{mode}: redo changed fields/pixels");
        }
        assert!((fields(&mut a, reopened), a.render_pixels(reopened, 1, 1.0).unwrap()) == empty, "{mode}: final removal changed empty fields/pixels");
        tool(&mut a, "doc_save", json!({"doc":reopened,"path":"empty.pdf"}));
        let last = tool(&mut a, "doc_open", json!({"path":"empty.pdf"}))["doc"].as_u64().unwrap();
        assert!((fields(&mut a, last), a.render_pixels(last, 1, 1.0).unwrap()) == empty, "{mode}: empty reopen changed fields/pixels");
        tool(&mut a, "js_run", json!({"doc":last,"field":"add","script":""}));
        let added = entries(&mut a, last);
        assert_eq!(added.len(), 1, "{mode}");
        assert_eq!(added[0]["value"], "default", "{mode}");
    }
}
