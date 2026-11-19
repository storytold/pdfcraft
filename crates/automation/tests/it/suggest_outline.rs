//! #6020: `bookmark_suggest` proposes an outline for an untagged document from how its text looks,
//! uses the tags once there are some, and its proposal goes back in through `bookmark_import`.

use std::path::PathBuf;

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pdfcraft-suggest-outline-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ok(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args) {
        Ok(mut c) => match c.remove(0) {
            Content::Json(v) => v,
            other => panic!("{tool}: expected JSON, got {other:?}"),
        },
        Err(e) => panic!("{tool} {args}: {e}"),
    }
}

const BODY: &str = "Body text runs on for a good many characters across the page.";

/// A chapter heading (Helvetica-Bold 20) set on two lines, body text (Helvetica 11), a bold
/// caption, a section heading (Helvetica-Bold 14), and a second chapter on page 2.
fn report_pdf() -> Vec<u8> {
    let body = |y: u32| format!("BT /FR 11 Tf 72 {y} Td ({BODY}) Tj ET ");
    let page1 = format!(
        "BT /FB 20 Tf 72 740 Td (Getting started) Tj 0 -22 Td (with the toolkit) Tj ET {}{}\
         BT /FB 14 Tf 72 640 Td (Figure 1 The toolkit) Tj ET {}\
         BT /FB 14 Tf 72 590 Td (Installing) Tj ET {}{}",
        body(690),
        body(676),
        body(620),
        body(566),
        body(552)
    );
    let page2 = format!("BT /FB 20 Tf 72 740 Td (Reference) Tj ET {}{}", body(700), body(686));
    two_pages(&page1, &page2)
}

/// Two untagged pages drawing `page1` and `page2` with `/FB` (Helvetica-Bold) and `/FR`
/// (Helvetica).
fn two_pages(page1: &str, page2: &str) -> Vec<u8> {
    let contents = [page1, page2];
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 612 792] /Resources << /Font << /FB 7 0 R /FR 8 0 R >> >> >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>".into(),
    ];
    objs.extend(contents.iter().map(|c| format!("<< /Length {} >>\nstream\n{c}\nendstream", c.len())));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>".into());
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into());
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        bytes.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    bytes
}

/// (title, page, children's titles) for each top-level node.
fn outline(nodes: &Value) -> Vec<(String, u64, Vec<String>)> {
    let title = |n: &Value| n["title"].as_str().unwrap().to_string();
    nodes
        .as_array()
        .unwrap()
        .iter()
        .map(|n| (title(n), n["page"].as_u64().unwrap(), n["children"].as_array().map_or(Vec::new(), |c| c.iter().map(title).collect())))
        .collect()
}

#[test]
fn suggest_from_layout_then_import_and_undo() {
    let dir = tmp("layout");
    std::fs::write(dir.join("report.pdf"), report_pdf()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = ok(&mut a, "doc_open", json!({ "path": "report.pdf" }))["doc"].as_u64().unwrap();
    let s = ok(&mut a, "bookmark_suggest", json!({ "doc": doc }));
    assert_eq!(s["source"], "layout");
    assert_eq!(s["truncated"], false);
    assert_eq!((s["headings"].as_u64(), s["pages"].as_u64(), s["pages_read"].as_u64()), (Some(3), Some(2), Some(2)));
    let expected =
        vec![("Getting started with the toolkit".to_string(), 1, vec!["Installing".to_string()]), ("Reference".to_string(), 2, Vec::new())];
    assert_eq!(outline(&s["bookmarks"]), expected, "the caption is left out and the wrapped heading is one bookmark");
    let view = &s["bookmarks"][0]["view"];
    assert_eq!(view["type"], "XYZ");
    assert_eq!(view["values"][2], Value::Null, "the zoom is the reader's");
    assert!(view["values"][1].as_f64().is_some_and(|top| top > 740.0), "goes to the top of the heading: {view}");
    // It changed nothing; the reviewed list goes in as one undoable step.
    assert_eq!(ok(&mut a, "bookmark_list", json!({ "doc": doc }))["bookmarks"], json!([]));
    ok(&mut a, "bookmark_import", json!({ "doc": doc, "bookmarks": s["bookmarks"], "replace": true }));
    let back = ok(&mut a, "bookmark_export", json!({ "doc": doc }));
    assert_eq!(outline(&back["bookmarks"]), expected);
    ok(&mut a, "edit_undo", json!({ "doc": doc }));
    assert_eq!(ok(&mut a, "bookmark_list", json!({ "doc": doc }))["bookmarks"], json!([]));
}

#[test]
fn suggest_uses_the_tags_once_there_are_some() {
    let dir = tmp("structure");
    std::fs::write(dir.join("report.pdf"), report_pdf()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = ok(&mut a, "doc_open", json!({ "path": "report.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "accessibility_autotag_apply", json!({ "doc": doc }));
    let s = ok(&mut a, "bookmark_suggest", json!({ "doc": doc }));
    assert_eq!(s["source"], "structure");
    let titles: Vec<&str> = s["bookmarks"].as_array().unwrap().iter().map(|n| n["title"].as_str().unwrap()).collect();
    assert!(titles.first().is_some_and(|t| t.starts_with("Getting started")), "{s}");
}

#[test]
fn text_in_one_style_gets_no_suggestion_and_a_note() {
    let dir = tmp("plain");
    let page = format!("BT /FR 11 Tf 72 700 Td ({BODY}) Tj 0 -14 Td ({BODY}) Tj ET");
    std::fs::write(dir.join("plain.pdf"), two_pages(&page, &page)).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = ok(&mut a, "doc_open", json!({ "path": "plain.pdf" }))["doc"].as_u64().unwrap();
    let s = ok(&mut a, "bookmark_suggest", json!({ "doc": doc }));
    assert_eq!((s["source"].as_str(), s["headings"].as_u64(), &s["bookmarks"]), (Some("layout"), Some(0), &json!([])));
    assert!(s["note"].as_str().is_some_and(|n| n.contains("bookmark_from_text_styles")), "{s}");
}

/// A suggested bookmark has no level to edit: bookmark_import refuses one, and the nesting changes
/// by moving a bookmark into or out of another's children, as the tool description says.
#[test]
fn a_suggestion_is_renested_through_children_not_levels() {
    let dir = tmp("renest");
    std::fs::write(dir.join("report.pdf"), report_pdf()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = ok(&mut a, "doc_open", json!({ "path": "report.pdf" }))["doc"].as_u64().unwrap();
    let mut bookmarks = ok(&mut a, "bookmark_suggest", json!({ "doc": doc }))["bookmarks"].clone();
    let mut leveled = bookmarks.clone();
    leveled[0]["level"] = json!(2);
    let refused = a.call("bookmark_import", &json!({ "doc": doc, "bookmarks": leveled, "replace": true }));
    assert!(refused.is_err_and(|e| e.to_string().contains("unknown key")), "a level was taken");
    let child = bookmarks[0]["children"].as_array_mut().unwrap().remove(0);
    bookmarks.as_array_mut().unwrap().push(child);
    ok(&mut a, "bookmark_import", json!({ "doc": doc, "bookmarks": bookmarks, "replace": true }));
    let back = ok(&mut a, "bookmark_export", json!({ "doc": doc }));
    let top: Vec<String> = outline(&back["bookmarks"]).into_iter().map(|(t, _, _)| t).collect();
    assert_eq!(top, ["Getting started with the toolkit", "Reference", "Installing"]);
    let tool = pdfcraft_automation::tools().into_iter().find(|t| t.name == "bookmark_suggest").unwrap();
    assert!(tool.description.contains("children") && !tool.description.contains("levels or"), "{}", tool.description);
}
