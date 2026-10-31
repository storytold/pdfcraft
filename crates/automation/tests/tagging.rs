//! The Tags panel and Reading Order tool, driven headlessly through the automation tools: tag an
//! untagged page from scratch, edit an existing tag tree, and check after saving that the
//! structure is consistent and the page draws what it drew.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdfcraft_automation::{Automation, Content, ToolError};
use serde_json::{Value, json};

fn pdf(objects: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

fn stream(data: &str) -> String {
    format!("<< /Length {} >>\nstream\n{data}\nendstream", data.len())
}

#[track_caller]
fn workdir(test: &str) -> PathBuf {
    let at = std::panic::Location::caller();
    let dir = std::env::temp_dir().join(format!("pdfcraft-tagging-{test}-{}-{}", std::process::id(), at.line()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn auto(dir: &Path) -> Automation {
    Automation::new().with_root(dir).unwrap().with_clock(|| 1_700_000_000)
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

/// The saved file's structure problems and painting operators per page.
fn saved_state(path: &Path) -> (Vec<String>, Vec<String>) {
    let doc = pdfcraft_cos::Document::open(Arc::new(std::fs::read(path).unwrap())).unwrap();
    let painting = pdfcraft_model::pages(&doc)
        .iter()
        .map(|p| {
            let c = doc.resolve(p.dict.get(b"Contents").unwrap());
            let pdfcraft_cos::Object::Stream(s) = &*c else { return String::new() };
            let ops = pdfcraft_content::parse(&s.decoded().unwrap()).ops;
            let kept: Vec<_> = ops.into_iter().filter(|o| !matches!(o.op.as_slice(), b"BDC" | b"BMC" | b"EMC")).collect();
            String::from_utf8_lossy(&pdfcraft_content::serialize_ops(&kept)).into_owned()
        })
        .collect();
    (pdfcraft_engine::a11y::structure_problems(&doc), painting)
}

/// An untagged page: a heading, a paragraph, a rule and a link.
fn untagged() -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Annots [6 0 R] /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        stream("BT /F1 18 Tf 20 260 Td (Report) Tj ET\nBT /F1 10 Tf 20 230 Td (The year in review.) Tj ET\n20 200 260 2 re f"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /Annot /Subtype /Link /Rect [20 226 120 242] /Border [0 0 0] >>".into(),
    ])
}

#[test]
fn an_untagged_page_is_tagged_end_to_end_with_the_reading_order_tools() {
    let dir = workdir("untagged");
    std::fs::write(dir.join("in.pdf"), untagged()).unwrap();
    let before = saved_state(&dir.join("in.pdf")).1;
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "in.pdf" }))["doc"].as_u64().unwrap();
    let found = ok(&mut a, "accessibility_page_content", json!({ "doc": doc, "page": 1, "only_untagged": true }));
    assert_eq!(found["units"].as_array().unwrap().len(), 3, "{found}");
    assert_eq!(found["units"][0]["text"], "Report");
    assert_eq!(found["units"][2]["kind"], "graphic");
    assert_eq!(found["annotations"][0]["subtype"], "Link");
    // Top-left-origin points: the heading is near the top.
    let heading = found["units"][0]["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>();
    assert!(heading[1] < 45.0 && heading[0] > 19.0, "{heading:?}");

    let created = ok(&mut a, "accessibility_create_tag", json!({ "doc": doc, "page": 1, "units": [0], "type": "H1" }));
    let types: Vec<&str> = created["created"].as_array().unwrap().iter().map(|t| t["type"].as_str().unwrap()).collect();
    assert_eq!(types, ["Document", "H1"], "the first tag brings the tag tree");
    // The paragraph by rectangle, in view coordinates.
    let p = ok(&mut a, "accessibility_create_tag", json!({ "doc": doc, "page": 1, "rect": [10, 55, 290, 80], "type": "P" }));
    assert_eq!(p["created"][0]["type"], "P");
    assert_eq!(p["reading_order"].as_array().unwrap().len(), 2);
    ok(&mut a, "accessibility_mark_content_artifact", json!({ "doc": doc, "page": 1, "units": [2] }));
    let link = ok(&mut a, "accessibility_tag_annotation", json!({ "doc": doc, "page": 1, "annotation": 6 }));
    assert_eq!(link["created"][0]["type"], "Link");

    let left = ok(&mut a, "accessibility_page_content", json!({ "doc": doc, "page": 1, "only_untagged": true }));
    assert_eq!((left["units"].as_array().unwrap().len(), left["annotations"].as_array().unwrap().len()), (0, 0), "{left}");
    let order = ok(&mut a, "accessibility_reading_order", json!({ "doc": doc, "page": 1 }));
    let regions: Vec<(&str, &str)> =
        order["regions"].as_array().unwrap().iter().map(|r| (r["type"].as_str().unwrap(), r["text"].as_str().unwrap())).collect();
    assert_eq!(regions, [("H1", "Report"), ("P", "The year in review."), ("Link", "")]);
    assert_eq!(order["untagged"], 0);
    let check = ok(&mut a, "accessibility_check", json!({ "doc": doc, "rules": ["tagged-pdf", "tagged-content", "tagged-annotations"] }));
    assert_eq!(check["failed"], 0, "{check}");

    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "out.pdf" }));
    let (problems, after) = saved_state(&dir.join("out.pdf"));
    assert_eq!(problems, Vec::<String>::new());
    assert_eq!(after, before, "the page draws what it drew");
    // Every step is one undo step.
    for label in ["Tag annotation", "Mark content as artifact", "Tag content as P", "Tag content as H1"] {
        assert_eq!(ok(&mut a, "edit_undo", json!({ "doc": doc }))["undone"], label);
    }
    let back = ok(&mut a, "accessibility_page_content", json!({ "doc": doc, "page": 1, "only_untagged": true }));
    assert_eq!(back["units"].as_array().unwrap().len(), 3);
    assert_eq!(ok(&mut a, "accessibility_tags", json!({ "doc": doc }))["count"], 0);
}

/// Document › [P, Sect › [P, Figure]] over the same page drawn tagged.
fn tagged() -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /MarkInfo << /Marked true >> /StructTreeRoot 6 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /StructParents 0 /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        stream(
            "/P << /MCID 0 >> BDC BT /F1 12 Tf 20 260 Td (Intro) Tj ET EMC\n\
             /P << /MCID 1 >> BDC BT /F1 12 Tf 20 230 Td (Body) Tj ET EMC\n\
             /Figure << /MCID 2 >> BDC 20 120 100 50 re f EMC",
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /StructTreeRoot /K 7 0 R /ParentTree 12 0 R /ParentTreeNextKey 1 >>".into(),
        "<< /Type /StructElem /S /Document /P 6 0 R /K [8 0 R 9 0 R] >>".into(),
        "<< /Type /StructElem /S /P /P 7 0 R /Pg 3 0 R /K 0 >>".into(),
        "<< /Type /StructElem /S /Sect /P 7 0 R /Pg 3 0 R /K [10 0 R 11 0 R] >>".into(),
        "<< /Type /StructElem /S /P /P 9 0 R /K 1 >>".into(),
        "<< /Type /StructElem /S /Figure /P 9 0 R /K 2 >>".into(),
        "<< /Nums [0 [8 0 R 10 0 R 11 0 R]] >>".into(),
    ])
}

fn tag_numbers(a: &mut Automation, doc: u64) -> Vec<(u64, String, u64)> {
    ok(a, "accessibility_tags", json!({ "doc": doc }))["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["object"].as_u64().unwrap(), t["type"].as_str().unwrap().to_owned(), t["depth"].as_u64().unwrap()))
        .collect()
}

#[test]
fn tags_panel_tools_add_move_delete_and_artifact_tags() {
    let dir = workdir("tagged");
    std::fs::write(dir.join("in.pdf"), tagged()).unwrap();
    let before = saved_state(&dir.join("in.pdf")).1;
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "in.pdf" }))["doc"].as_u64().unwrap();
    let sect = ok(&mut a, "accessibility_new_tag", json!({ "doc": doc, "type": "Aside" }))["created"][0].clone();
    assert_eq!((sect["type"].as_str(), sect["depth"].as_u64()), (Some("Aside"), Some(1)), "into the document by default");
    let aside = sect["object"].as_u64().unwrap();
    // Across parents: the figure leaves its section for the new tag.
    ok(&mut a, "accessibility_move_tag_to", json!({ "doc": doc, "object": 11, "placement": "into", "anchor": aside }));
    ok(&mut a, "accessibility_move_tag_to", json!({ "doc": doc, "object": aside, "placement": "before", "anchor": 8 }));
    assert_eq!(
        tag_numbers(&mut a, doc),
        [
            (7, "Document".into(), 0),
            (aside, "Aside".into(), 1),
            (11, "Figure".into(), 2),
            (8, "P".into(), 1),
            (9, "Sect".into(), 1),
            (10, "P".into(), 2)
        ]
    );
    let order = ok(&mut a, "accessibility_reading_order", json!({ "doc": doc, "page": 1 }));
    let regions: Vec<u64> = order["regions"].as_array().unwrap().iter().map(|r| r["tag"]["object"].as_u64().unwrap()).collect();
    assert_eq!(regions, [11, 8, 10], "reading order follows the tree");
    // Highlight and hit-test go both ways (view coordinates: y grows downwards).
    let at = ok(&mut a, "accessibility_tag_at", json!({ "doc": doc, "page": 1, "x": 60, "y": 150 }));
    assert_eq!((at["tag"]["object"].as_u64(), at["type"].as_str()), (Some(11), Some("Figure")));
    let boxes = ok(&mut a, "accessibility_tag_boxes", json!({ "doc": doc, "page": 1, "object": aside }));
    assert_eq!(boxes["rects"], json!([[20.0, 130.0, 120.0, 180.0]]));
    // Refusals say why and change nothing.
    let count = tag_numbers(&mut a, doc).len();
    for (tool, args) in [
        ("accessibility_move_tag_to", json!({ "doc": doc, "object": 9, "placement": "into", "anchor": 10 })),
        ("accessibility_new_tag", json!({ "doc": doc, "type": "NotAType" })),
        ("accessibility_create_tag", json!({ "doc": doc, "page": 1, "units": [99], "type": "P" })),
    ] {
        assert!(matches!(a.call(tool, &args), Err(ToolError::Failed(_))), "{tool}");
    }
    assert!(matches!(a.call("accessibility_create_tag", &json!({ "doc": doc, "page": 1, "type": "P" })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(
        a.call("accessibility_move_tag_to", &json!({ "doc": doc, "object": 9, "placement": "sideways", "anchor": 8 })),
        Err(ToolError::InvalidArgs(_))
    ));
    assert_eq!(tag_numbers(&mut a, doc).len(), count);
    // The aside with its figure becomes an artifact; the section is unwrapped; the intro deleted.
    ok(&mut a, "accessibility_tag_to_artifact", json!({ "doc": doc, "object": aside }));
    ok(&mut a, "accessibility_delete_tag", json!({ "doc": doc, "object": 9, "keep_contents": true }));
    ok(&mut a, "accessibility_delete_tag", json!({ "doc": doc, "object": 8 }));
    assert_eq!(tag_numbers(&mut a, doc), [(7, "Document".into(), 0), (10, "P".into(), 1)]);
    let content = ok(&mut a, "accessibility_page_content", json!({ "doc": doc, "page": 1 }));
    let states: Vec<&str> = content["units"].as_array().unwrap().iter().map(|u| u["state"].as_str().unwrap()).collect();
    assert_eq!(states, ["orphan", "tagged", "artifact"]);
    assert_eq!(ok(&mut a, "edit_undo", json!({ "doc": doc }))["undone"], "Delete tag");
    assert_eq!(ok(&mut a, "edit_undo", json!({ "doc": doc }))["undone"], "Remove tag, keep its contents");
    ok(&mut a, "edit_redo", json!({ "doc": doc }));
    ok(&mut a, "edit_redo", json!({ "doc": doc }));
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "out.pdf" }));
    let (problems, after) = saved_state(&dir.join("out.pdf"));
    assert_eq!(problems, Vec::<String>::new());
    assert_eq!(after, before);
}

/// A tagged form in a layout field tagging used to refuse: the root's /K an array, the parent
/// tree split into a leaf with /Limits whose page array is indirect, and no /ParentTreeNextKey.
fn tagged_form() -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /MarkInfo << /Marked true >> /StructTreeRoot 6 0 R /AcroForm << /Fields [11 0 R 12 0 R] >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /StructParents 0 /Annots [11 0 R 12 0 R] /Resources << /Font << /F1 5 0 R >> >> >>"
            .into(),
        stream("/H1 << /MCID 0 >> BDC BT /F1 16 Tf 20 260 Td (Sign up) Tj ET EMC\n/P << /MCID 1 >> BDC BT /F1 10 Tf 20 230 Td (Name and email) Tj ET EMC"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /StructTreeRoot /K [7 0 R] /ParentTree 10 0 R >>".into(),
        "<< /Type /StructElem /S /Document /P 6 0 R /K [8 0 R 9 0 R] >>".into(),
        "<< /Type /StructElem /S /H1 /P 7 0 R /Pg 3 0 R /K 0 >>".into(),
        "<< /Type /StructElem /S /P /P 7 0 R /Pg 3 0 R /K 1 >>".into(),
        "<< /Kids [13 0 R] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [20 200 140 215] /P 3 0 R >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (email) /Rect [160 200 280 215] /P 3 0 R >>".into(),
        "<< /Limits [0 0] /Nums [0 14 0 R] >>".into(),
        "[8 0 R 9 0 R]".into(),
    ])
}

#[test]
fn form_fields_are_tagged_into_an_existing_tag_tree_end_to_end() {
    let dir = workdir("form");
    std::fs::write(dir.join("in.pdf"), tagged_form()).unwrap();
    let before = saved_state(&dir.join("in.pdf")).1;
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "in.pdf" }))["doc"].as_u64().unwrap();
    let tagged = ok(&mut a, "accessibility_autotag_form_fields", json!({ "doc": doc }));
    assert_eq!((tagged["status"].as_str(), tagged["remaining_findings"].as_u64()), (Some("passed"), Some(0)), "{tagged}");
    let order = ok(&mut a, "accessibility_reading_order", json!({ "doc": doc, "page": 1 }));
    let types: Vec<&str> = order["regions"].as_array().unwrap().iter().map(|r| r["type"].as_str().unwrap()).collect();
    assert_eq!(types, ["H1", "P", "Form", "Form"], "after the page's content, in /Annots order");
    assert!(matches!(a.call("accessibility_autotag_form_fields", &json!({ "doc": doc })), Err(ToolError::Failed(_))), "nothing is left to tag");
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "out.pdf" }));
    let (problems, after) = saved_state(&dir.join("out.pdf"));
    assert_eq!(problems, Vec::<String>::new());
    assert_eq!(after, before, "the page draws what it drew");
    let reopened = ok(&mut a, "doc_open", json!({ "path": "out.pdf" }))["doc"].as_u64().unwrap();
    let check = ok(&mut a, "accessibility_check", json!({ "doc": reopened, "rules": ["tagged-pdf", "tagged-form-fields"] }));
    assert_eq!(check["failed"], 0, "{check}");
    assert_eq!(ok(&mut a, "edit_undo", json!({ "doc": doc }))["undone"], "Tag form fields");
}
