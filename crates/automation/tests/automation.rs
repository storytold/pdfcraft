//! End-to-end tests of the automation tools and the MCP server, on synthetic PDFs.

use std::path::{Path, PathBuf};

#[cfg(feature = "mcp")]
use pdfcraft_automation::mcp::McpServer;
use pdfcraft_automation::{Automation, Content, ToolError, tools};
use serde_json::{Value, json};

/// A PDF with `n` 200×300 pt pages reading "Page 1", "Page 2", …
fn fixture(n: usize) -> Vec<u8> {
    let mut objs: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")).into_bytes());
    objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i).into_bytes());
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes());
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn remote_link_pdf(file: &str, destination: Option<&str>) -> Vec<u8> {
    // canonicalize() uses a Windows device prefix; a PDF file specification uses the ordinary drive path.
    #[cfg(windows)]
    let file = file.strip_prefix(r"\\?\").filter(|path| path.as_bytes().get(1) == Some(&b':')).unwrap_or(file);
    let file = file.replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)");
    let destination = destination.map_or_else(String::new, |d| format!("/D {d}"));
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>".to_vec(),
        format!("<< /Type /Annot /Subtype /Link /Rect [10 10 80 30] /A << /S /GoToR /F ({file}) {destination} >> >>").into_bytes(),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        bytes.extend_from_slice(object);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

fn protected_fixture(password: &str) -> Vec<u8> {
    use pdfcraft_engine::{Edit, Protection, Session};
    let mut session = Session::new();
    let id = session.open("target.pdf", None, std::sync::Arc::new(fixture(1)), None).unwrap();
    session.apply(id, Edit::Protect(Protection { open_password: Some(password.into()), ..Default::default() })).unwrap();
    session.save_bytes(id).unwrap().as_ref().clone()
}

/// A fresh directory with `a.pdf` (3 pages) and `b.pdf` (2 pages).
#[track_caller]
fn workdir(test: &str) -> PathBuf {
    let caller = std::panic::Location::caller();
    let dir = std::env::temp_dir().join(format!("pdfcraft-automation-{test}-{}-{}-{}", std::process::id(), caller.line(), caller.column(),));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.pdf"), fixture(3)).unwrap();
    std::fs::write(dir.join("b.pdf"), fixture(2)).unwrap();
    dir
}

#[test]
fn workdir_call_sites_do_not_delete_each_other() {
    let first = workdir("same-tag");
    let sentinel = first.join("sentinel");
    std::fs::write(&sentinel, b"first call site").unwrap();

    let second = workdir("same-tag");
    let paths_differ = first != second;
    let sentinel_bytes = std::fs::read(&sentinel).ok();
    let first_a = std::fs::read(first.join("a.pdf")).ok();
    let first_b = std::fs::read(first.join("b.pdf")).ok();
    let second_a = std::fs::read(second.join("a.pdf")).ok();
    let second_b = std::fs::read(second.join("b.pdf")).ok();

    let _ = std::fs::remove_dir_all(&first);
    if paths_differ {
        let _ = std::fs::remove_dir_all(&second);
    }

    assert!(paths_differ, "distinct call sites shared {first:?}");
    assert_eq!(sentinel_bytes.as_deref(), Some(&b"first call site"[..]));
    assert_eq!(first_a, Some(fixture(3)));
    assert_eq!(first_b, Some(fixture(2)));
    assert_eq!(second_a, Some(fixture(3)));
    assert_eq!(second_b, Some(fixture(2)));
}

fn auto(dir: &Path) -> Automation {
    Automation::new().with_root(dir).unwrap().with_clock(|| 1_700_000_000)
}

/// Full-screen presentation settings (`/Trans`, `/Dur`) reach `doc_info`, 1-based pages, only
/// for the pages that carry them.
#[test]
fn doc_info_lists_page_transitions_and_display_times() {
    let dir = workdir("transitions");
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 300] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Trans << /S /Fade /D 0.5 >> /Dur 4 >>".to_vec(),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        bytes.extend_from_slice(object);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        bytes.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    std::fs::write(dir.join("slides.pdf"), bytes).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "slides.pdf" }))["doc"].as_u64().unwrap();
    let info = ok(&mut a, "doc_info", json!({ "doc": doc }));
    let t = info["transitions"].as_array().unwrap();
    assert_eq!(t.len(), 1, "{}", info["transitions"]);
    assert_eq!(
        (t[0]["page"].as_u64(), t[0]["style"].as_str(), t[0]["duration"].as_f64(), t[0]["display"].as_f64()),
        (Some(2), Some("Fade"), Some(0.5), Some(4.0)),
        "{}",
        t[0]
    );
}

/// Page transitions through the tools: set on chosen pages, written to the file as `/Trans` and
/// `/Dur`, removed again, bad values refused, and one undo step. (Read through `pdfcraft_cos`,
/// so this does not lean on `doc_info` listing them.)
#[test]
fn page_transitions_are_set_and_removed_through_the_tools() {
    use pdfcraft_cos::Object;
    /// `key` of 0-based `page` in the saved file `name`, resolved.
    fn entry(dir: &Path, name: &str, page: usize, key: &[u8]) -> Option<Object> {
        let d = pdfcraft_cos::Document::open(std::sync::Arc::new(std::fs::read(dir.join(name)).ok()?)).ok()?;
        let root = d.root()?;
        let pages = d.get(root).as_dict()?.reference(b"Pages")?;
        let kids = d.get(pages).as_dict()?.get(b"Kids")?.clone();
        let kid = d.resolve(&kids).as_array()?.get(page)?.clone();
        let o = d.resolve(&kid).as_dict()?.get(key)?.clone();
        Some((*d.resolve(&o)).clone())
    }
    let dir = workdir("transitions-set");
    std::fs::write(dir.join("a.pdf"), fixture(3)).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "page_transition_set", json!({ "doc": doc, "pages": [2, 3], "style": "Wipe", "duration": 2, "direction": 270, "display": 5 }));
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "out/t1.pdf" }));
    let t = entry(&dir, "out/t1.pdf", 1, b"Trans").expect("/Trans on page 2");
    let t = t.as_dict().expect("a dictionary");
    assert_eq!(
        (t.name(b"S"), t.get(b"D").and_then(Object::as_f64), t.get(b"Di").and_then(Object::as_f64)),
        (Some(&b"Wipe"[..]), Some(2.0), Some(270.0)),
        "{t:?}"
    );
    assert_eq!(entry(&dir, "out/t1.pdf", 1, b"Dur").and_then(|o| o.as_f64()), Some(5.0));
    assert!(entry(&dir, "out/t1.pdf", 2, b"Trans").is_some(), "page 3 too");
    assert!(entry(&dir, "out/t1.pdf", 0, b"Trans").is_none() && entry(&dir, "out/t1.pdf", 0, b"Dur").is_none(), "page 1 was not asked for");
    ok(
        &mut a,
        "page_transition_set",
        json!({ "doc": doc, "pages": [3], "style": "Fly", "motion": "outward", "direction": "none", "scale": 0.5, "opaque": true }),
    );
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "out/t2.pdf" }));
    let t = entry(&dir, "out/t2.pdf", 2, b"Trans").expect("/Trans on page 3");
    let t = t.as_dict().expect("a dictionary");
    assert_eq!(
        (t.name(b"S"), t.name(b"M"), t.name(b"Di"), t.get(b"SS").and_then(Object::as_f64), t.get(b"B")),
        (Some(&b"Fly"[..]), Some(&b"O"[..]), Some(&b"None"[..]), Some(0.5), Some(&Object::Bool(true))),
        "{t:?}"
    );
    assert!(entry(&dir, "out/t2.pdf", 2, b"Dur").is_none(), "display time left out removes /Dur");
    assert!(a.call("page_transition_set", &json!({ "doc": doc, "style": "Swirl" })).is_err(), "an unknown style is refused");
    assert!(a.call("page_transition_set", &json!({ "doc": doc, "style": "Wipe", "direction": 45 })).is_err());
    assert!(a.call("page_transition_set", &json!({ "doc": doc, "pages": [9], "style": "Fade" })).is_err(), "no such page");
    ok(&mut a, "page_transition_set", json!({ "doc": doc, "style": "none" }));
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "out/t3.pdf" }));
    assert!((0..3).all(|p| entry(&dir, "out/t3.pdf", p, b"Trans").is_none() && entry(&dir, "out/t3.pdf", p, b"Dur").is_none()), "all removed");
    ok(&mut a, "edit_undo", json!({ "doc": doc }));
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "out/t4.pdf" }));
    assert!(
        entry(&dir, "out/t4.pdf", 1, b"Trans").is_some() && entry(&dir, "out/t4.pdf", 2, b"Trans").is_some(),
        "one undo step brings both pages back"
    );
}

#[test]
fn autotag_preview_proposes_text_without_changing_the_pdf() {
    let dir = workdir("autotag-preview");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let before = std::fs::read(dir.join("a.pdf")).unwrap();

    let preview = ok(&mut a, "accessibility_autotag_preview", json!({ "doc": doc, "page": 1 }));
    assert_eq!(preview["page"], 1);
    assert_eq!(preview["candidates"][0]["type"], "P");
    assert_eq!(preview["candidates"][0]["text"], "Page 1");
    assert_eq!(preview["candidates"][0]["needs_review"], true);
    assert_eq!(preview["unclassified_images"], 0);
    assert_eq!(preview["unclassified_text_blocks"], 0);
    assert_eq!(preview["existing_structure"], false);
    assert_eq!(std::fs::read(dir.join("a.pdf")).unwrap(), before);
    assert!(matches!(
        a.call("accessibility_autotag_preview", &json!({ "doc": doc, "page": 4 })),
        Err(ToolError::InvalidArgs(_) | ToolError::Failed(_))
    ));

    let blank = ok(&mut a, "doc_create", json!({ "from": "blank", "width": 300, "height": 400 }))["doc"].as_u64().unwrap();
    ok(&mut a, "page_add_text", json!({ "doc": blank, "page": 1, "text": "Annual report", "at": [20, 25], "size": 24 }));
    ok(&mut a, "page_add_text", json!({ "doc": blank, "page": 1, "text": "First paragraph", "at": [20, 100], "size": 12 }));
    ok(&mut a, "page_add_text", json!({ "doc": blank, "page": 1, "text": "Second paragraph", "at": [20, 140], "size": 12 }));
    let preview = ok(&mut a, "accessibility_autotag_preview", json!({ "doc": blank, "page": 1 }));
    assert!(preview["candidates"].as_array().unwrap().iter().any(|c| c["text"] == "Annual report" && c["type"] == "H1"));
    assert!(preview["candidates"].as_array().unwrap().iter().any(|c| c["text"] == "First paragraph" && c["type"] == "P"));
}

#[test]
fn render_pixels_agrees_with_page_render_and_skips_the_png() {
    let dir = workdir("render-pixels");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();

    let (width, height, pixels) = a.render_pixels(doc, 1, 1.0).unwrap();
    let png = a.call("page_render", &json!({ "doc": doc, "page": 1, "dpi": 72 })).unwrap();
    let Content::Png { width: png_width, height: png_height, .. } = png[0] else { panic!("expected an image") };
    assert_eq!((width, height), (png_width, png_height), "scale 1.0 is 72 dpi, like the tool");
    assert_eq!(pixels.as_ref().len(), width as usize * height as usize * 4, "RGBA, four bytes a pixel");
    assert!(pixels.as_ref().as_chunks::<4>().0.iter().any(|p| p[..3] != [255, 255, 255]), "the page text is drawn, not a blank sheet");

    let (double_width, _, _) = a.render_pixels(doc, 1, 2.0).unwrap();
    assert_eq!(double_width, width * 2, "the scale is pixels per point");

    for scale in [0.0_f32, -1.0, f32::NAN, f32::INFINITY, 1.0e6] {
        assert!(matches!(a.render_pixels(doc, 1, scale), Err(ToolError::InvalidArgs(_))), "scale {scale} must be refused");
    }
    assert!(matches!(a.render_pixels(doc, 0, 1.0), Err(ToolError::InvalidArgs(_))), "pages are 1-based");
    assert!(matches!(a.render_pixels(doc, 99, 1.0), Err(ToolError::InvalidArgs(_))), "a page past the end");
    assert!(matches!(a.render_pixels(9_999, 1, 1.0), Err(ToolError::Failed(_))), "a document that is not open");
}

#[test]
fn autotag_apply_writes_reviewable_tags_and_survives_save_reopen() {
    let dir = workdir("autotag-apply");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "b.pdf" }))["doc"].as_u64().unwrap();
    let before = ok(&mut a, "accessibility_check", json!({ "doc": doc, "rules": ["tagged-pdf", "tagged-content"] }));
    assert_eq!(before["failed"], 2);
    let rendered_before = a.call("page_render", &json!({ "doc": doc, "page": 1, "dpi": 72 })).unwrap();

    let applied = ok(&mut a, "accessibility_autotag_apply", json!({ "doc": doc }));
    assert_eq!(applied["count"], 3, "Document plus one paragraph per page");
    assert_eq!(applied["tags"][1]["type"], "P");
    let after = ok(&mut a, "accessibility_check", json!({ "doc": doc, "rules": ["tagged-pdf", "tagged-content"] }));
    assert_eq!(after["passed"], 2);
    assert_eq!(a.call("page_render", &json!({ "doc": doc, "page": 1, "dpi": 72 })).unwrap(), rendered_before);
    assert!(matches!(a.call("accessibility_autotag_apply", &json!({ "doc": doc })), Err(ToolError::Failed(_))));

    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "tagged.pdf" }));
    let saved = std::fs::read(dir.join("tagged.pdf")).unwrap();
    let reopened = pdfcraft_cos::Document::open(std::sync::Arc::new(saved)).unwrap();
    let pages = pdfcraft_model::pages(&reopened);
    assert_eq!(pages.len(), 2);
    assert!(pages.iter().all(|page| page.dict.contains(b"StructParents")));
}

#[test]
fn autotag_form_fields_writes_reviewable_widget_tags() {
    let dir = workdir("autotag-form-fields");
    std::fs::write(dir.join("field.pdf"), form_pdf("name")).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "field.pdf" }))["doc"].as_u64().unwrap();
    let tagged = ok(&mut a, "accessibility_autotag_form_fields", json!({ "doc": doc }));
    assert_eq!(tagged["status"], "passed");
    assert_eq!(tagged["remaining_findings"], 0);
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "tagged.pdf" }));
    let reopened = ok(&mut a, "doc_open", json!({ "path": "tagged.pdf" }))["doc"].as_u64().unwrap();
    let check = ok(&mut a, "accessibility_check", json!({ "doc": reopened, "rules": ["tagged-form-fields"] }));
    let result = check["results"].as_array().unwrap().iter().find(|result| result["rule"] == "tagged-form-fields").unwrap();
    assert_eq!(result["status"], "passed");
}

/// A one-page PDF whose form holds one text field named `name`.
fn form_pdf(name: &str) -> Vec<u8> {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>".into(),
        format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name}) /V (x) /Rect [60 60 200 80] /P 3 0 R >>"),
    ];
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

#[test]
fn accessibility_check_locates_field_description_on_page() {
    let dir = workdir("accessibility-field-location");
    std::fs::write(dir.join("field.pdf"), form_pdf("name")).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "field.pdf" }))["doc"].as_u64().unwrap();
    let report = ok(&mut a, "accessibility_check", json!({ "doc": doc, "rules": ["field-descriptions"] }));
    let rule = report["results"].as_array().unwrap().iter().find(|r| r["rule"] == "field-descriptions").unwrap();
    assert_eq!(rule["status"], "failed");
    assert_eq!(rule["findings"][0]["page"], 1);
    assert_eq!(rule["findings"][0]["bbox_user_space"], json!([60.0, 60.0, 200.0, 80.0]));
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

fn err(a: &mut Automation, tool: &str, args: Value) -> ToolError {
    a.call(tool, &args).unwrap_err()
}

fn page_text(a: &mut Automation, doc: u64) -> Vec<String> {
    ok(a, "text_extract", json!({ "doc": doc }))["pages"].as_array().unwrap().iter().map(|p| p["text"].as_str().unwrap().trim().to_string()).collect()
}

#[test]
fn tool_table_is_well_formed() {
    let all = tools();
    let mut names = std::collections::HashSet::new();
    for t in &all {
        assert!(names.insert(t.name), "duplicate tool {}", t.name);
        assert!(t.name.len() <= 64 && t.name.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "bad MCP tool name {}", t.name);
        assert_eq!(t.input_schema["type"], "object", "{}", t.name);
        let props = t.input_schema["properties"].as_object().unwrap();
        for r in t.input_schema["required"].as_array().unwrap() {
            assert!(props.contains_key(r.as_str().unwrap()), "{}: required {r} is not a property", t.name);
        }
        assert!(!(t.read_only && t.destructive), "{} is both read-only and destructive", t.name);
        for k in ["oneOf", "anyOf", "allOf"] {
            assert!(t.input_schema.get(k).is_none(), "{}: top-level {k} in input_schema is rejected by MCP clients such as Claude", t.name);
        }
        if let Some(c) = t.command {
            assert!(pdfcraft_engine::commands::command(c).is_some(), "{} names unregistered command {c}", t.name);
        }
    }
}

#[test]
fn explicit_font_and_disjoint_ranges_save_reopen_through_tools() {
    let dir = workdir("font-ranges");
    std::fs::write(dir.join("inter.ttf"), include_bytes!("../../../assets/fonts/Inter-Regular.ttf")).unwrap();
    let mut a = auto(&dir);
    std::fs::write(dir.join("broken.otf"), b"not a font").unwrap();
    let found = ok(&mut a, "text_font_files", json!({}));
    assert_eq!(found["fonts"].as_array().unwrap().len(), 1);
    assert_eq!(found["fonts"][0]["font"], "Inter-Regular");
    assert!(a.call("text_font_files", &json!({"directories":["../outside"]})).is_err());
    let id = ok(&mut a, "doc_open", json!({"path":"a.pdf"}))["doc"].as_u64().unwrap();
    let font = ok(&mut a, "text_font_import", json!({"doc":id,"page":1,"path":"inter.ttf"}));
    let resource = font["resource"].as_str().unwrap();
    assert_eq!(font["font"], "Inter-Regular");
    assert_eq!(page_text(&mut a, id), ["Page 1", "Page 2", "Page 3"]);
    let draft = ok(
        &mut a,
        "text_edit_layout",
        json!({"doc":id,"page":1,"paragraph":1,"text":"Aé ΩЖ","font_resource":resource,"ranges":[{"range":[0,3],"size":16,"color":"#cc0000"},{"range":[4,8],"size":20,"color":"#003399"}]}),
    );
    assert!(draft["draft_rect"].is_array());
    assert_eq!(page_text(&mut a, id), ["Page 1", "Page 2", "Page 3"]);
    ok(
        &mut a,
        "text_edit",
        json!({"doc":id,"page":1,"paragraph":1,"text":"Aé ΩЖ","font_resource":resource,"ranges":[{"range":[0,3],"size":16,"color":"#cc0000"},{"range":[4,8],"size":20,"color":"#003399"}]}),
    );
    let lines = ok(&mut a, "text_lines", json!({"doc":id,"page":1}));
    assert!(
        lines["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["text"].as_str().is_some_and(|s| s.trim_end() == "Aé") && l["size"].as_f64() == Some(16.0)),
        "{lines}"
    );
    let rendered = a.call("page_render", &json!({"doc":id,"page":1,"dpi":72})).unwrap();
    ok(&mut a, "doc_save", json!({"doc":id,"path":"edited.pdf"}));
    let reopened = ok(&mut a, "doc_open", json!({"path":"edited.pdf"}))["doc"].as_u64().unwrap();
    let saved = ok(&mut a, "text_lines", json!({"doc":reopened,"page":1}));
    assert_eq!(lines, saved);
    assert_eq!(rendered, a.call("page_render", &json!({"doc":reopened,"page":1,"dpi":72})).unwrap());
    let before = saved.clone();
    assert!(a.call("text_edit", &json!({"doc":reopened,"page":1,"paragraph":1,"text":"é test","ranges":[{"range":[1,2],"size":18}]})).is_err());
    assert_eq!(ok(&mut a, "text_lines", json!({"doc":reopened,"page":1})), before);
    assert!(a.call("text_font_import", &json!({"doc":id,"page":1,"path":"../outside.ttf"})).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn added_arabic_text_keeps_copy_search_and_editor_order_after_retyping_and_reopen() {
    if pdfcraft_fonts::document_arabic_font().is_none() {
        eprintln!("set CRAFT_FONTS_DIR to exercise added Arabic text");
        return;
    }
    let dir = workdir("arabic-copy-order");
    let mut a = auto(&dir);
    let id = ok(&mut a, "doc_create", json!({"from":"blank","width":600,"height":400}))["doc"].as_u64().unwrap();
    let original = "سَلَام 123 (PDF) - اختبار الكتابة العربية";
    ok(&mut a, "page_add_text", json!({"doc":id,"page":1,"text":original,"at":[40,50],"width":500,"size":16,"align":"right"}));
    assert_eq!(page_text(&mut a, id), [original]);
    assert_eq!(ok(&mut a, "text_lines", json!({"doc":id,"page":1}))["lines"][0]["text"], original);
    let found = ok(&mut a, "text_find", json!({"doc":id,"query":original}));
    assert_eq!(found["matches"].as_array().unwrap().len(), 1);
    let retyped = "مُرَاجَعَة PDF رقم ١٢٣";
    ok(&mut a, "content_update", json!({"doc":id,"page":1,"index":1,"text":retyped}));
    ok(&mut a, "doc_save", json!({"doc":id,"path":"retyped.pdf"}));
    let re = ok(&mut a, "doc_open", json!({"path":"retyped.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(page_text(&mut a, re), [retyped]);
    assert_eq!(ok(&mut a, "text_lines", json!({"doc":re,"page":1}))["lines"][0]["text"], retyped);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn arabic_actual_text_stays_logical_through_editor_tools_and_save_reopen() {
    use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};
    use std::sync::Arc;

    let dir = workdir("arabic-actual-text");
    let logical = "سَلَام 123 (PDF)";
    let hex: String = logical.encode_utf16().map(|u| format!("{u:04X}")).collect();
    let content = format!("/Span <</ActualText <FEFF{hex}>>> BDC BT /F1 24 Tf 20 150 Td (abcd) Tj (efgh) Tj ET EMC");
    let mut doc = Document::open(Arc::new(fixture(1))).unwrap();
    doc.set(ObjRef::new(5, 0), Object::Stream(Stream::flate(Dict::new(), content.as_bytes())));
    std::fs::write(dir.join("arabic.pdf"), write_full(&doc, &SaveOptions::default()).unwrap()).unwrap();

    let mut a = auto(&dir);
    let id = ok(&mut a, "doc_open", json!({"path":"arabic.pdf"}))["doc"].as_u64().unwrap();
    let lines = ok(&mut a, "text_lines", json!({"doc":id,"page":1}));
    assert_eq!(lines["lines"][0]["text"], logical);
    let render = a.call("page_render", &json!({"doc":id,"page":1,"dpi":72})).unwrap();
    ok(&mut a, "doc_save", json!({"doc":id,"path":"saved.pdf"}));
    let re = ok(&mut a, "doc_open", json!({"path":"saved.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "text_lines", json!({"doc":re,"page":1})), lines);
    assert_eq!(a.call("page_render", &json!({"doc":re,"page":1,"dpi":72})).unwrap(), render);
    ok(&mut a, "text_edit", json!({"doc":re,"page":1,"paragraph":1,"text":"Replacement"}));
    ok(&mut a, "doc_save", json!({"doc":re,"path":"edited.pdf"}));
    let edited = ok(&mut a, "doc_open", json!({"path":"edited.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "text_lines", json!({"doc":edited,"page":1}))["lines"][0]["text"], "Replacement");
    assert_eq!(page_text(&mut a, edited), ["Replacement"]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn arabic_ligature_actual_text_is_one_editable_word_through_save_reopen() {
    use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};
    use std::sync::Arc;

    let dir = workdir("arabic-ligature-actual-text");
    let mut doc = Document::open(Arc::new(fixture(1))).unwrap();
    // Synthetic glyph codes: A س, B م, X lam-alef. No installed Arabic font is needed.
    let cmap = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapType 2 def \
                1 begincodespacerange <00> <FF> endcodespacerange 3 beginbfchar \
                <41> <0633> <42> <0645> <58> <FEFB> \
                endbfchar endcmap CMapName currentdict /CMap defineresource pop end end";
    let unicode = doc.add(Object::Stream(Stream::flate(Dict::new(), cmap.as_bytes())));
    doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"ToUnicode".to_vec(), Object::Ref(unicode))).unwrap();
    let content = "BT /F1 24 Tf 20 150 Td (B) Tj /Span <</ActualText <FEFF06440627>>> BDC (X) Tj EMC (A) Tj ET";
    doc.set(ObjRef::new(5, 0), Object::Stream(Stream::flate(Dict::new(), content.as_bytes())));
    std::fs::write(dir.join("arabic.pdf"), write_full(&doc, &SaveOptions::default()).unwrap()).unwrap();

    let mut a = auto(&dir);
    let id = ok(&mut a, "doc_open", json!({"path":"arabic.pdf"}))["doc"].as_u64().unwrap();
    let lines = ok(&mut a, "text_lines", json!({"doc":id,"page":1}));
    assert_eq!(lines["lines"].as_array().unwrap().len(), 1);
    assert_eq!(lines["lines"][0]["text"], "سلام");
    assert_eq!(page_text(&mut a, id), ["سلام"]);
    assert_eq!(ok(&mut a, "text_find", json!({"doc":id,"query":"سلام"}))["matches"].as_array().unwrap().len(), 1);
    let render = a.call("page_render", &json!({"doc":id,"page":1,"dpi":72})).unwrap();
    ok(&mut a, "doc_save", json!({"doc":id,"path":"saved.pdf"}));
    let re = ok(&mut a, "doc_open", json!({"path":"saved.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "text_lines", json!({"doc":re,"page":1})), lines);
    assert_eq!(a.call("page_render", &json!({"doc":re,"page":1,"dpi":72})).unwrap(), render);
    ok(&mut a, "text_edit", json!({"doc":re,"page":1,"paragraph":1,"text":"Replacement"}));
    ok(&mut a, "doc_save", json!({"doc":re,"path":"edited.pdf"}));
    let edited = ok(&mut a, "doc_open", json!({"path":"edited.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(page_text(&mut a, edited), ["Replacement"]);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Each of these writes to its required `path` and replaces an existing file there, so the MCP
/// annotations must not tell clients the call is read-only or harmless (#130).
#[test]
fn file_writing_tools_are_not_read_only() {
    for name in ["doc_export_data", "accessibility_report", "image_save"] {
        let t = tools().into_iter().find(|t| t.name == name).unwrap();
        assert!(!t.read_only, "{name} writes a file but advertises read-only");
        assert!(t.destructive, "{name} overwrites its path but advertises non-destructive");
    }
}

#[test]
fn open_inspect_render_and_find() {
    let dir = workdir("inspect");
    let mut a = auto(&dir);
    let opened = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }));
    assert_eq!(opened["pages"], 3);
    assert_eq!(opened["editable"], true);
    let doc = opened["doc"].as_u64().unwrap();

    let info = ok(&mut a, "doc_info", json!({ "doc": doc }));
    assert_eq!(info["pages"].as_array().unwrap().len(), 3);
    assert_eq!(info["pages"][0]["width"], 200.0);
    assert_eq!(info["pages"][1]["label"], "2");

    // The fixture names /Helvetica without embedding it, so an agent can see both that a face is
    // standing in and why that face. `substitution` is null only when nothing stands in.
    let font = &info["fonts"][0];
    assert_eq!((&font["name"], &font["embedded"]), (&json!("Helvetica"), &json!(false)));
    assert_eq!(font["substitute"], json!("Helvetica"));
    assert!(font["substitution"].as_str().is_some_and(|w| w.contains("standard 14")), "{font}");

    assert_eq!(page_text(&mut a, doc), ["Page 1", "Page 2", "Page 3"]);

    let found = ok(&mut a, "text_find", json!({ "doc": doc, "query": "page  2" }));
    assert_eq!(found["count"], 1);
    assert_eq!(found["matches"][0]["page"], 2);
    let rect = found["matches"][0]["rects"][0].as_array().unwrap();
    assert!(rect[1].as_f64().unwrap() > 100.0 && rect[3].as_f64().unwrap() < 160.0, "rect is top-left based: {rect:?}");

    let png = a.call("page_render", &json!({ "doc": doc, "page": 1, "dpi": 72 })).unwrap();
    let Content::Png { data, width, height } = &png[0] else { panic!("expected an image") };
    assert_eq!((*width, *height), (200, 300));
    assert_eq!(&data[..8], b"\x89PNG\r\n\x1a\n");
    let decoder = png::Decoder::new(std::io::Cursor::new(data.as_slice()));
    let info = decoder.read_info().unwrap().info().clone();
    assert_eq!((info.width, info.height), (200, 300));
}

#[test]
fn word_count_reads_selected_pages_and_unsaved_edits_without_mutating() {
    let dir = workdir("word-count");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let before = ok(&mut a, "doc_list", json!({}));
    let report = ok(&mut a, "text_word_count", json!({ "doc": doc }));
    assert_eq!(report["total"]["words"], 6);
    assert_eq!(report["pages"].as_array().unwrap().len(), 3);
    let subset = ok(&mut a, "text_word_count", json!({ "doc": doc, "pages": [3, 1], "exclude_tables": true }));
    assert_eq!(subset["total"]["words"], 4);
    assert_eq!(subset["pages"][0]["page"], 3);
    assert_eq!(subset["unclassified_structure"], true);
    assert_eq!(ok(&mut a, "doc_list", json!({})), before);
    assert!(a.call("text_word_count", &json!({ "doc": doc, "pages": [4] })).is_err());
    ok(&mut a, "page_delete", json!({ "doc": doc, "pages": [1] }));
    assert_eq!(ok(&mut a, "text_word_count", json!({ "doc": doc }))["total"]["words"], 4);
    ok(&mut a, "edit_undo", json!({ "doc": doc }));
    assert_eq!(ok(&mut a, "text_word_count", json!({ "doc": doc }))["total"]["words"], 6);
    let def = tools().into_iter().find(|t| t.name == "text_word_count").unwrap();
    assert!(def.read_only && !def.destructive);
}

#[test]
fn text_editor_background_is_read_only_and_removes_source_glyphs() {
    let dir = workdir("text-preview");
    let mut a = auto(&dir);
    std::fs::write(dir.join("text-preview.pdf"), fixture(2)).unwrap();
    let doc = ok(&mut a, "doc_open", json!({ "path": "text-preview.pdf" }))["doc"].as_u64().unwrap();
    let args = json!({ "doc": doc, "page": 1, "dpi": 72 });
    let before = a.call("page_render", &args).unwrap();
    let state = ok(&mut a, "doc_info", json!({ "doc": doc }));
    let preview = a.call("page_render", &json!({ "doc": doc, "page": 1, "dpi": 72, "hide_text_block": 1 })).unwrap();
    assert_ne!(preview, before);
    let Content::Png { data, .. } = &preview[0] else { panic!("expected PNG") };
    let mut reader = png::Decoder::new(std::io::Cursor::new(data)).read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    assert!(pixels[..info.buffer_size()].iter().all(|b| *b == 255), "no source glyphs remain on the white page");
    assert_eq!(a.call("page_render", &args).unwrap(), before);
    assert_eq!(ok(&mut a, "doc_info", json!({ "doc": doc })), state);
    assert_eq!(page_text(&mut a, doc), ["Page 1", "Page 2"]);
    for block in [-1, 0, 2, i64::MAX] {
        assert!(a.call("page_render", &json!({ "doc": doc, "page": 1, "hide_text_block": block })).is_err());
    }
}

#[test]
fn added_text_editor_background_hides_only_the_item_without_editing() {
    let dir = workdir("added-text-preview");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let render = json!({"doc":doc,"page":1,"dpi":72});
    let before = a.call("page_render", &render).unwrap();
    ok(&mut a, "page_add_text", json!({"doc":doc,"page":1,"text":"Retype this label","at":[20,20],"width":160}));
    let drawn = a.call("page_render", &render).unwrap();
    assert_ne!(drawn, before);
    let state = ok(&mut a, "doc_info", json!({"doc":doc}));
    let items = ok(&mut a, "content_list", json!({"doc":doc}));
    let preview = a.call("page_render", &json!({"doc":doc,"page":1,"dpi":72,"hide_added_text":1})).unwrap();
    assert_eq!(preview, before, "native text and the page background stay intact");
    assert_eq!(a.call("page_render", &render).unwrap(), drawn);
    assert_eq!(ok(&mut a, "doc_info", json!({"doc":doc})), state);
    assert_eq!(ok(&mut a, "content_list", json!({"doc":doc})), items);
    for index in [-1, 0, 2, i64::MAX] {
        assert!(a.call("page_render", &json!({"doc":doc,"page":1,"hide_added_text":index})).is_err());
    }
    assert!(a.call("page_render", &json!({"doc":doc,"page":1,"hide_added_text":1,"hide_text_block":1})).is_err());
}

#[test]
fn edit_undo_redo_and_save_round_trip() {
    let dir = workdir("edit");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();

    let s = ok(&mut a, "page_delete", json!({ "doc": doc, "pages": [2] }));
    assert_eq!((s["pages"].as_u64(), s["dirty"].as_bool()), (Some(2), Some(true)));
    assert_eq!(page_text(&mut a, doc), ["Page 1", "Page 3"]);

    ok(&mut a, "edit_undo", json!({ "doc": doc }));
    assert_eq!(page_text(&mut a, doc), ["Page 1", "Page 2", "Page 3"]);
    ok(&mut a, "edit_redo", json!({ "doc": doc }));
    assert_eq!(page_text(&mut a, doc), ["Page 1", "Page 3"]);

    ok(&mut a, "page_insert_file", json!({ "doc": doc, "path": "b.pdf", "pages": [2], "at": 1 }));
    ok(&mut a, "page_insert_blank", json!({ "doc": doc, "at": 4 }));
    ok(&mut a, "page_move", json!({ "doc": doc, "pages": [3], "to": 1 }));
    ok(&mut a, "page_rotate", json!({ "doc": doc, "pages": [1], "degrees": 90 }));
    ok(&mut a, "doc_set_info", json!({ "doc": doc, "key": "Title", "value": "Automated" }));
    assert_eq!(page_text(&mut a, doc), ["Page 3", "Page 2", "Page 1", ""]);

    let saved = ok(&mut a, "doc_save", json!({ "doc": doc, "path": "out/edited.pdf" }));
    assert_eq!(saved["incremental"], false);
    assert_eq!(saved["document"]["dirty"], false);

    let mut b = auto(&dir);
    let re = ok(&mut b, "doc_open", json!({ "path": "out/edited.pdf" }))["doc"].as_u64().unwrap();
    let info = ok(&mut b, "doc_info", json!({ "doc": re }));
    assert_eq!(info["title"], "Automated");
    assert_eq!(info["pages"][0]["rotation"], 90);
    assert_eq!(page_text(&mut b, re), ["Page 3", "Page 2", "Page 1", ""]);

    // Saving in place appends an incremental update.
    ok(&mut b, "doc_set_info", json!({ "doc": re, "key": "Author", "value": "Agent" }));
    let before = std::fs::metadata(dir.join("out/edited.pdf")).unwrap().len();
    let again = ok(&mut b, "doc_save", json!({ "doc": re }));
    assert_eq!(again["incremental"], true);
    let after = std::fs::read(dir.join("out/edited.pdf")).unwrap();
    assert!(after.len() as u64 > before);
    assert_eq!(after.windows(5).filter(|w| w == b"%%EOF").count(), 2);
}

/// Issue #1329: an in-place save says it was incremental only when it kept the file it was
/// saving over: protecting or sanitizing rewrites the file, and the result says so.
#[test]
fn an_in_place_save_reports_a_required_rewrite_as_not_incremental() {
    let dir = workdir("save-incremental-flag");
    let original = std::fs::read(dir.join("a.pdf")).unwrap();
    let cases = [
        ("page_rotate", json!({ "pages": [1], "degrees": 90 }), true),
        ("doc_protect", json!({ "open_password": "openme" }), false),
        ("doc_remove_hidden", json!({}), false),
    ];
    for (tool, args, incremental) in cases {
        std::fs::write(dir.join("a.pdf"), &original).unwrap();
        let mut a = auto(&dir);
        let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
        if tool == "doc_remove_hidden" {
            // Removing absent categories is a no-op. Give this rewrite test real metadata
            // to remove, including an earlier on-disk revision that must not survive.
            ok(&mut a, "doc_set_info", json!({ "doc": doc, "key": "Title", "value": "Private title" }));
            ok(&mut a, "doc_save", json!({ "doc": doc }));
        }
        let mut args = args;
        args["doc"] = json!(doc);
        ok(&mut a, tool, args);
        let saved = ok(&mut a, "doc_save", json!({ "doc": doc }));
        let kept = std::fs::read(dir.join("a.pdf")).unwrap().starts_with(&original);
        assert_eq!((saved["incremental"].as_bool(), kept), (Some(incremental), incremental), "{tool}");
    }
}

/// #1664 repairs the missing page tree on open. The former #1698 failure fixture now
/// saves successfully, keeping its page and pixels through a full rewrite.
#[test]
fn a_recovered_page_tree_survives_a_full_save() {
    let dir = workdir("save-verify-before-write");
    let objs: [&[u8]; 4] = [
        b"<< /Type /Catalog /Pages 3 0 R >>",
        b"<< /Type /Page /MediaBox [0 0 200 300] /Contents 4 0 R >>",
        b"[ 0 ]",
        b"<< /Length 18 >>\nstream\n0 0 m 100 100 l S\nendstream",
    ];
    let mut original = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(original.len());
        original.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        original.extend_from_slice(o);
        original.extend_from_slice(b"\nendobj\n");
    }
    let xref = original.len();
    original.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for o in offsets {
        original.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    original.extend_from_slice(format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    std::fs::write(dir.join("damaged.pdf"), &original).unwrap();

    let mut a = auto(&dir);
    let opened = ok(&mut a, "doc_open", json!({ "path": "damaged.pdf" }));
    assert_eq!(opened["pages"], 1, "{opened}");
    let doc = opened["doc"].as_u64().unwrap();
    let (width, height, pixels) = a.render_pixels(doc, 1, 1.0).unwrap();
    assert!(pixels.as_ref().as_chunks::<4>().0.iter().any(|p| p[..3] != [255, 255, 255]), "the original page is not blank");
    let saved = ok(&mut a, "doc_save", json!({ "doc": doc, "full": true }));
    assert_eq!(saved["document"]["dirty"], false);
    let on_disk = std::fs::read(dir.join("damaged.pdf")).unwrap();
    assert_ne!(on_disk, original, "a full save writes the repaired page tree");
    let mut reopened = auto(&dir);
    let opened = ok(&mut reopened, "doc_open", json!({ "path": "damaged.pdf" }));
    assert_eq!(opened["pages"], 1);
    let again = opened["doc"].as_u64().unwrap();
    let (new_width, new_height, new_pixels) = reopened.render_pixels(again, 1, 1.0).unwrap();
    assert_eq!((new_width, new_height), (width, height));
    assert_eq!(new_pixels.as_ref(), pixels.as_ref(), "saving must preserve the recovered page's drawing");
}

/// organize.field-name-conflicts: combining files whose forms share a field name keeps the
/// fields separate by renaming, says so in the result, and can be told to merge instead; any
/// other policy is an argument error.
#[test]
fn combine_renames_colliding_field_names() {
    let dir = workdir("combine-field-conflicts");
    std::fs::write(dir.join("form-a.pdf"), form_pdf("Text1")).unwrap();
    std::fs::write(dir.join("form-b.pdf"), form_pdf("Text1")).unwrap();
    let mut a = auto(&dir);
    let combined = ok(&mut a, "doc_combine", json!({ "paths": ["form-a.pdf", "form-b.pdf"], "open": true }));
    assert_eq!(combined["renamed_fields"], json!([["Text1", "Text1-1"]]), "{combined}");
    let merged = ok(&mut a, "doc_combine", json!({ "paths": ["form-a.pdf", "form-b.pdf"], "field_conflicts": "merge" }));
    assert!(merged.get("renamed_fields").is_none(), "merge renames nothing: {merged}");
    let bad = a.call("doc_combine", &json!({ "paths": ["form-a.pdf", "form-b.pdf"], "field_conflicts": "bogus" }));
    assert!(matches!(bad, Err(ToolError::InvalidArgs(_))));
}

/// #569: images and text files combine with PDFs, each made into a PDF first.
#[test]
fn combine_takes_images_and_text_files() {
    let dir = workdir("combine-images");
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, 4, 2);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().unwrap().write_image_data(&[90; 24]).unwrap();
    }
    std::fs::write(dir.join("scan.png"), png).unwrap();
    std::fs::write(dir.join("notes.txt"), "Some notes").unwrap();
    std::fs::write(dir.join("broken.png"), b"\x89PNG\r\n\x1a\nbroken").unwrap();
    std::fs::write(dir.join("report.docx"), b"PK\x03\x04").unwrap();
    let mut a = auto(&dir);
    let combined = ok(&mut a, "doc_combine", json!({ "paths": ["a.pdf", "scan.png", "notes.txt"], "pages": [null, null, "1"], "open": true }));
    let doc = combined["document"]["doc"].as_u64().unwrap();
    assert_eq!(combined["document"]["pages"], 5);
    assert_eq!(page_text(&mut a, doc), ["Page 1", "Page 2", "Page 3", "", "Some notes"]);
    let marks = ok(&mut a, "bookmark_list", json!({ "doc": doc }))["bookmarks"].clone();
    let titles: Vec<&str> = marks.as_array().unwrap().iter().map(|b| b["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["a", "scan", "notes"]);
    // A file that can't be made into a PDF stops the combine, and says which.
    for bad in ["broken.png", "report.docx"] {
        let refused = a.call("doc_combine", &json!({ "paths": ["a.pdf", bad] }));
        assert!(matches!(&refused, Err(ToolError::Failed(e)) if e.contains(bad)), "{bad}: {refused:?}");
    }
}

#[test]
fn combine_extract_and_split() {
    let dir = workdir("organize");
    let mut a = auto(&dir);
    let combined = ok(&mut a, "doc_combine", json!({ "paths": ["a.pdf", "b.pdf"], "out": "ab.pdf", "open": true }));
    let doc = combined["document"]["doc"].as_u64().unwrap();
    assert_eq!(combined["document"]["pages"], 5);
    assert_eq!(page_text(&mut a, doc), ["Page 1", "Page 2", "Page 3", "Page 1", "Page 2"]);
    // Chosen pages per file, in the order given.
    let some = ok(&mut a, "doc_combine", json!({ "paths": ["a.pdf", "b.pdf"], "pages": ["3, 1", null], "open": true }));
    assert_eq!(page_text(&mut a, some["document"]["doc"].as_u64().unwrap()), ["Page 3", "Page 1", "Page 1", "Page 2"]);
    assert!(matches!(a.call("doc_combine", &json!({ "paths": ["a.pdf", "b.pdf"], "pages": ["9", null] })), Err(ToolError::Failed(_))));
    assert!(matches!(a.call("doc_combine", &json!({ "paths": ["a.pdf", "b.pdf"], "pages": ["1"] })), Err(ToolError::InvalidArgs(_))));

    // A file split around another, as the Combine files grid makes it: copied once, one bookmark.
    let split = ok(
        &mut a,
        "doc_combine",
        json!({ "paths": ["a.pdf", "b.pdf", "a.pdf"], "pages": ["1", null, "3, 2"], "groups": [4, null, 4], "open": true }),
    );
    let split_doc = split["document"]["doc"].as_u64().unwrap();
    assert_eq!(page_text(&mut a, split_doc), ["Page 1", "Page 1", "Page 2", "Page 3", "Page 2"]);
    let marks = ok(&mut a, "bookmark_list", json!({ "doc": split_doc }))["bookmarks"].clone();
    let marks: Vec<_> = marks.as_array().unwrap().iter().map(|b| (b["title"].as_str().unwrap().to_string(), b["page"].as_u64().unwrap())).collect();
    assert_eq!(marks, [("a".to_string(), 1), ("b".to_string(), 2)]);
    // Without groups the same paths are separate files, each with its own bookmark.
    let apart = ok(&mut a, "doc_combine", json!({ "paths": ["a.pdf", "b.pdf", "a.pdf"], "pages": ["1", null, "3, 2"], "open": true }));
    let apart = ok(&mut a, "bookmark_list", json!({ "doc": apart["document"]["doc"].as_u64().unwrap() }))["bookmarks"].clone();
    assert_eq!(apart.as_array().unwrap().len(), 3);
    // A group names one file; groups must be in step with paths and whole numbers.
    let not_same = a.call("doc_combine", &json!({ "paths": ["a.pdf", "b.pdf"], "groups": [1, 1] }));
    assert!(matches!(not_same, Err(ToolError::InvalidArgs(e)) if e.contains("group 1")));
    for groups in [json!([1]), json!([1, -1]), json!([1, 1.5]), json!(["1", null]), json!(7)] {
        let bad = a.call("doc_combine", &json!({ "paths": ["a.pdf", "b.pdf"], "groups": groups }));
        assert!(matches!(bad, Err(ToolError::InvalidArgs(_))), "{groups}");
    }
    // The largest group number still leaves a key for a file of its own, or says why not.
    let max = ok(&mut a, "doc_combine", json!({ "paths": ["a.pdf", "b.pdf"], "groups": [u64::MAX - 1, null], "open": true }));
    assert_eq!(max["document"]["pages"], 5);
    let full = a.call("doc_combine", &json!({ "paths": ["a.pdf", "b.pdf"], "groups": [u64::MAX, null] }));
    assert!(matches!(full, Err(ToolError::InvalidArgs(_))));

    let ex = ok(&mut a, "page_extract", json!({ "doc": doc, "pages": [2, 4] }));
    let ex_doc = ex["document"]["doc"].as_u64().unwrap();
    assert_eq!(page_text(&mut a, ex_doc), ["Page 2", "Page 1"]);
    assert!(ex.get("path").is_none());

    let split = ok(&mut a, "doc_split", json!({ "doc": doc, "every": 2, "out_dir": "parts" }));
    let files = split["files"].as_array().unwrap();
    assert_eq!(files.len(), 3);
    assert_eq!((files[2]["first_page"].as_u64(), files[2]["last_page"].as_u64()), (Some(5), Some(5)));
    for f in files {
        assert!(Path::new(f["path"].as_str().unwrap()).starts_with(dir.canonicalize().unwrap()));
    }
}

#[test]
fn errors_are_specific_and_safe() {
    let dir = workdir("errors");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();

    let err = |a: &mut Automation, tool: &str, args: Value| a.call(tool, &args).unwrap_err();
    assert!(matches!(err(&mut a, "nope", json!({})), ToolError::UnknownTool(_)));
    assert!(matches!(err(&mut a, "page_delete", json!({ "doc": doc, "pages": [9] })), ToolError::InvalidArgs(m) if m.contains("3 pages")));
    assert!(matches!(err(&mut a, "page_delete", json!({ "doc": doc, "pages": [0] })), ToolError::InvalidArgs(_)));
    assert!(matches!(err(&mut a, "page_delete", json!({ "doc": doc, "page": [1] })), ToolError::InvalidArgs(m) if m.contains("unknown argument")));
    assert!(matches!(err(&mut a, "page_rotate", json!({ "doc": doc, "pages": [1], "degrees": 45 })), ToolError::InvalidArgs(_)));
    assert!(matches!(err(&mut a, "doc_info", json!({ "doc": 99 })), ToolError::Failed(_)));
    assert!(matches!(err(&mut a, "edit_undo", json!({ "doc": doc })), ToolError::Failed(_)));

    // Unsaved changes are never dropped silently.
    ok(&mut a, "page_delete", json!({ "doc": doc, "pages": [1] }));
    assert!(matches!(err(&mut a, "doc_close", json!({ "doc": doc })), ToolError::Failed(m) if m.contains("unsaved")));
    ok(&mut a, "doc_close", json!({ "doc": doc, "discard_changes": true }));
    assert_eq!(ok(&mut a, "doc_list", json!({}))["documents"], json!([]));

    // The root confines reads and writes.
    let outside = std::env::temp_dir().join("pdfcraft-automation-outside.pdf");
    std::fs::write(&outside, fixture(1)).unwrap();
    assert!(matches!(err(&mut a, "doc_open", json!({ "path": outside.to_str().unwrap() })), ToolError::Failed(m) if m.contains("outside")));
    assert!(
        matches!(err(&mut a, "doc_open", json!({ "path": "../pdfcraft-automation-outside.pdf" })), ToolError::Failed(m) if m.contains("outside"))
    );
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    assert!(a.call("doc_save", &json!({ "doc": doc, "path": "new/../../escape.pdf" })).is_err());
    assert!(a.call("doc_save", &json!({ "doc": doc, "path": outside.to_str().unwrap() })).is_err());
    let _ = std::fs::remove_file(outside);
}

/// `root/` (with `inside.pdf`) next to `outside/` (with the file `secret.pdf` and the folder
/// `sub`), all in a fresh temporary directory. Returns (base, canonical root).
fn sandbox(test: &str) -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("pdfcraft-automation-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("root")).unwrap();
    std::fs::create_dir_all(base.join("outside/sub")).unwrap();
    std::fs::write(base.join("root/inside.pdf"), fixture(1)).unwrap();
    std::fs::write(base.join("outside/secret.pdf"), fixture(1)).unwrap();
    let root = base.join("root").canonicalize().unwrap();
    (base, root)
}

/// A link `root/<name>` to the directory `target`: a symlink on Unix, a junction on Windows
/// (which needs no privilege).
fn link_dir(root: &Path, name: &str, target: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, root.join(name)).unwrap();
    #[cfg(windows)]
    {
        // Rebuilt from components so every separator is `\` (cmd reads `/x` as a switch).
        let (link, target): (PathBuf, PathBuf) = (root.join(name).components().collect(), target.components().collect());
        let status = std::process::Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(link).arg(target).output().unwrap();
        assert!(status.status.success(), "mklink /J failed: {}", String::from_utf8_lossy(&status.stderr));
    }
}

#[test]
fn root_refusals_do_not_reveal_what_exists_outside() {
    // Regression test for #136: every path outside the root gets the same refusal, whether it
    // exists, is a file or a folder, or passes through a missing folder.
    let (base, root) = sandbox("root-oracle");
    let mut a = auto(&root);
    let refusal = |p: &str| ToolError::Failed(format!("{p} is outside the allowed directory {}", root.display()));
    let abs = |rel: &str| base.join(rel).to_str().unwrap().to_owned();

    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut reads = vec![
        "../outside/secret.pdf".to_owned(),
        "../outside/nope.pdf".into(),
        "../outside/secret.pdf/x".into(),
        "../outside/sub".into(),
        "../outside/sub/x".into(),
        "../nowhere/at/all.pdf".into(),
        "missing/../../outside/secret.pdf".into(),
        "missing/../../outside/nope.pdf".into(),
        "../../../../../../../../../../../../../../../../../../../../../../../../x.pdf".into(),
        abs("outside/secret.pdf"),
        abs("outside/nope.pdf"),
        abs("outside/sub/x"),
        abs("nowhere/x.pdf"),
        abs("nowhere/../outside/nope.pdf"),
        abs("outside/../outside/secret.pdf"),
        abs("root/../outside/secret.pdf"),
    ];
    #[cfg(windows)]
    {
        reads.extend([
            r"..\outside\secret.pdf".to_owned(),
            r"..\outside/nope.pdf".into(),
            "../outside/secret.pdf.".into(),
            "../outside/secret.pdf ".into(),
            // Another network share or device namespace is refused by name, without contacting
            // it (`.invalid` never resolves, so a regression fails instead of reaching a host).
            r"\\pdfcraft-test.invalid\share\secret.pdf".into(),
            "//pdfcraft-test.invalid/share/secret.pdf".into(),
            r"\\?\UNC\pdfcraft-test.invalid\share\secret.pdf".into(),
            r"\\.\pipe\pdfcraft-test".into(),
            r"\\?\GLOBALROOT\Device\Null".into(),
        ]);
        let other = base.join("outside/secret.pdf").canonicalize().unwrap();
        reads.push(other.to_str().unwrap().to_owned()); // the verbatim \\?\C:\… form
        if let Some(drive) = (b'D'..=b'Z').rev().map(|d| format!("{}:\\", d as char)).find(|d| !Path::new(d).exists()) {
            reads.push(format!("{drive}secret.pdf")); // a drive that doesn't exist
        }
    }
    for p in &reads {
        assert_eq!(a.call("doc_open", &json!({ "path": p })).unwrap_err(), refusal(p), "reading {p}");
    }

    let writes = [
        "../outside/sub/../y.pdf".to_owned(),
        "../outside/nosub/../y.pdf".into(),
        "../outside/y.pdf".into(),
        "../outside/nosub/y.pdf".into(),
        "../outside/secret.pdf".into(),
        "../outside/secret.pdf/y.pdf".into(),
        "new/../../escape.pdf".into(),
        abs("outside/nosub/deeper/y.pdf"),
    ];
    let doc = ok(&mut a, "doc_open", json!({ "path": "inside.pdf" }))["doc"].as_u64().unwrap();
    for p in &writes {
        assert_eq!(a.call("doc_save", &json!({ "doc": doc, "path": p })).unwrap_err(), refusal(p), "writing {p}");
    }
    #[cfg(windows)]
    {
        // In a verbatim path `/` is not a separator, so `x/../..` can't climb out of it either.
        for tail in [r"\x/../../outside/v.pdf", r"\x/../../outside/secret.pdf"] {
            let p = format!("{}{tail}", root.display());
            if let Ok(c) = a.call("doc_save", &json!({ "doc": doc, "path": p })) {
                let Content::Json(v) = &c[0] else { panic!("{p}: expected JSON") };
                assert!(Path::new(v["path"].as_str().unwrap()).starts_with(&root), "{p} wrote {v}");
            }
            // (What lands outside the root, if anything, is checked below.)
        }
    }

    // A link inside the root that leads out of it is refused the same way, below it too.
    link_dir(&base.join("root"), "link", &base.join("outside"));
    for p in ["link/secret.pdf", "link/nope.pdf", "link/sub/x", "link"] {
        assert_eq!(a.call("doc_open", &json!({ "path": p })).unwrap_err(), refusal(p), "reading {p}");
    }
    for p in ["link/new.pdf", "link/nosub/new.pdf", "link/secret.pdf"] {
        assert_eq!(a.call("doc_save", &json!({ "doc": doc, "path": p })).unwrap_err(), refusal(p), "writing {p}");
    }
    // So is a link whose target is gone: whether a link's target exists stays hidden too.
    std::fs::create_dir_all(base.join("outside/gone")).unwrap();
    link_dir(&base.join("root"), "broken", &base.join("outside/gone"));
    std::fs::remove_dir(base.join("outside/gone")).unwrap();
    for p in ["broken", "broken/x.pdf", "broken/x/y.pdf"] {
        assert_eq!(a.call("doc_open", &json!({ "path": p })).unwrap_err(), refusal(p), "reading {p}");
        assert_eq!(a.call("doc_save", &json!({ "doc": doc, "path": p })).unwrap_err(), refusal(p), "writing {p}");
    }

    // Nothing was written outside the root, and the outside files are untouched.
    let mut left: Vec<String> =
        std::fs::read_dir(base.join("outside")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    assert_eq!(left, ["secret.pdf", "sub"]);
    assert_eq!(std::fs::read(base.join("outside/secret.pdf")).unwrap(), fixture(1));
    assert!(!base.join("escape.pdf").exists() && !base.join("y.pdf").exists());

    // Inside the root nothing changes: missing files say so, and existing ones open and save,
    // however the path is spelled.
    let missing = a.call("doc_open", &json!({ "path": "missing-inside.pdf" })).unwrap_err();
    assert!(
        matches!(&missing, ToolError::Failed(m) if m.starts_with("missing-inside.pdf not found (looked for ") && !m.contains("outside")),
        "{missing:?}"
    );
    let not_dir = a.call("doc_open", &json!({ "path": "inside.pdf/x" })).unwrap_err();
    assert!(matches!(&not_dir, ToolError::Failed(m) if !m.contains("outside")), "{not_dir:?}");
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut opens = vec![
        "inside.pdf".to_owned(),
        "./inside.pdf".into(),
        "sub/../inside.pdf".into(),
        "../root/inside.pdf".into(),
        "../outside/../root/inside.pdf".into(),
        "../nowhere/../root/inside.pdf".into(),
        abs("root/inside.pdf"),
        abs("outside/../root/inside.pdf"),
        abs("nowhere/../root/inside.pdf"),
        root.join("inside.pdf").to_str().unwrap().to_owned(),
    ];
    #[cfg(windows)]
    {
        let plain = abs("root/inside.pdf");
        opens.extend([plain.to_lowercase(), plain.to_uppercase(), r"..\root\inside.pdf".into()]);
    }
    for p in &opens {
        ok(&mut a, "doc_open", json!({ "path": p }));
    }
    for p in ["new.pdf", "fresh/dir/new.pdf", "fresh/../also-new.pdf"] {
        ok(&mut a, "doc_save", json!({ "doc": doc, "path": p }));
    }
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": abs("root/abs-new.pdf") }));
    for f in ["new.pdf", "fresh/dir/new.pdf", "also-new.pdf", "abs-new.pdf"] {
        assert!(root.join(f).is_file(), "{f} was written inside the root");
    }

    // Without a root, paths are used as given.
    let mut free = Automation::new();
    ok(&mut free, "doc_open", json!({ "path": abs("outside/secret.pdf") }));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn writing_to_a_folder_touches_nothing_beside_it() {
    // "." names the root itself. Saving there used to stage its temporary file next to the
    // root, outside it, overwriting and then deleting any file of that name. Staging names are
    // random now and a failed rename removes the staging file, so the "is a folder" refusal is
    // what this checks; the listings and the file at the old staging name are canaries.
    let (base, root) = sandbox("root-itself");
    let mut a = auto(&root);
    let beside = base.join(".root.pdfcraft-tmp");
    std::fs::write(&beside, "SENTINEL").unwrap();
    std::fs::create_dir_all(root.join("folder")).unwrap();
    let listing = |dir: &Path| {
        let mut names: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        names.sort();
        names
    };
    let (base_before, folder_before) = (listing(&base), listing(&root.join("folder")));
    let doc = ok(&mut a, "doc_open", json!({ "path": "inside.pdf" }))["doc"].as_u64().unwrap();
    for p in [".", "", "folder", "folder/"] {
        let e = a.call("doc_save", &json!({ "doc": doc, "path": p })).unwrap_err();
        assert!(matches!(&e, ToolError::Failed(m) if m.contains("is a folder")), "{p:?}: {e:?}");
    }
    let png = vec![1, 2, 3];
    assert!(a.write_output(".", &png).is_err());
    assert_eq!(std::fs::read_to_string(&beside).unwrap(), "SENTINEL");
    assert_eq!(listing(&base), base_before, "nothing was left beside the root");
    assert_eq!(listing(&root.join("folder")), folder_before, "nothing was left in the folder");
    // `image_save` adds an extension when the path has none, which turned "." into `root.png`
    // beside the root.
    ok(&mut a, "doc_export_images", json!({ "doc": doc, "folder": "src", "dpi": 18 }));
    let pic = ok(&mut a, "doc_create", json!({ "from": "images", "paths": ["src/inside_page_1.png"] }))["doc"].as_u64().unwrap();
    for p in [".", "", "folder", "src/.."] {
        let e = a.call("image_save", &json!({ "doc": pic, "page": 1, "image": 1, "path": p })).unwrap_err();
        assert!(matches!(&e, ToolError::Failed(m) if m.contains("is a folder")), "{p:?}: {e:?}");
    }
    assert!(!base.join("root.png").exists() && !root.join("folder.png").exists());
    ok(&mut a, "image_save", json!({ "doc": pic, "page": 1, "image": 1, "path": "folder/picture" }));
    assert!(root.join("folder/picture.png").is_file());
    // Folder outputs may still name the root.
    ok(&mut a, "doc_split", json!({ "doc": doc, "every": 1, "out_dir": "." }));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn file_names_from_documents_stay_in_the_output_folder() {
    // Folder outputs name their files after the document. A document name with separators,
    // `..` or (on Windows) a drive letter used to take those files out of the folder, and out
    // of the root.
    let (base, root) = sandbox("doc-names");
    let mut a = auto(&root);
    let names = ["../../escape", "../../escape.pdf", "x/../../../escape.pdf", "/tmp/escape.pdf", r"x\C:escape.pdf", "C:escape.pdf", "..", "."];
    for (i, name) in names.iter().enumerate() {
        let doc = ok(&mut a, "doc_create", json!({ "from": "blank", "pages": 2, "name": name }))["doc"].as_u64().unwrap();
        let out = format!("out{i}");
        let mut files: Vec<String> = Vec::new();
        let r = ok(&mut a, "doc_export_images", json!({ "doc": doc, "folder": out, "dpi": 10 }));
        files.extend(r["files"].as_array().unwrap().iter().map(|f| f.as_str().unwrap().to_owned()));
        let r = ok(&mut a, "page_extract", json!({ "doc": doc, "pages": [1], "separate": true, "out_dir": out }));
        files.extend(r["files"].as_array().unwrap().iter().map(|f| f.as_str().unwrap().to_owned()));
        let r = ok(&mut a, "doc_split", json!({ "doc": doc, "every": 1, "out_dir": out }));
        files.extend(r["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap().to_owned()));
        assert_eq!(files.len(), 5, "{name:?}: {files:?}");
        for f in &files {
            let f = Path::new(f);
            assert_eq!(f.parent(), Some(root.join(&out).as_path()), "{name:?} wrote {}", f.display());
            assert!(f.is_file(), "{name:?}: {} exists", f.display());
        }
    }
    // The same for the images a page uses, with a document that has one.
    let doc = ok(&mut a, "doc_open", json!({ "path": "inside.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "doc_export_images", json!({ "doc": doc, "folder": "src", "dpi": 18 }));
    let pic =
        ok(&mut a, "doc_create", json!({ "from": "images", "paths": ["src/inside_page_1.png"], "name": "../../escape" }))["doc"].as_u64().unwrap();
    let r = ok(&mut a, "doc_export_all_images", json!({ "doc": pic, "folder": "all" }));
    let files = r["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{r}");
    for f in files {
        assert_eq!(Path::new(f["path"].as_str().unwrap()).parent(), Some(root.join("all").as_path()), "{r}");
    }
    let mut left: Vec<String> = std::fs::read_dir(&base).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    assert_eq!(left, ["outside", "root"]);

    // The root itself must be a folder.
    assert!(Automation::new().with_root(root.join("inside.pdf")).is_err());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn command_list_reports_enablement_and_tools() {
    let dir = workdir("commands");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let list = ok(&mut a, "command_list", json!({ "doc": doc }));
    let find = |id: &str| list["commands"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap().clone();
    assert_eq!(find("page.rotate")["tool"], "page_rotate");
    assert_eq!(find("page.rotate")["enabled"], true);
    assert_eq!(find("edit.undo")["enabled"], false);
    ok(&mut a, "page_rotate", json!({ "doc": doc, "pages": [1], "degrees": 90 }));
    let list = ok(&mut a, "command_list", json!({ "doc": doc }));
    let undo = list["commands"].as_array().unwrap().iter().find(|c| c["id"] == "edit.undo").unwrap().clone();
    assert_eq!(undo["enabled"], true);
    assert_eq!(undo["label"], "Undo Rotate page");
}

// ---- MCP ---------------------------------------------------------------------------------------

#[cfg(feature = "mcp")]
fn rpc(server: &mut McpServer, id: u64, method: &str, params: Value) -> Value {
    let line = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string();
    serde_json::from_str(&server.handle_line(&line).expect("a reply")).unwrap()
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_session_over_stdio() {
    let dir = workdir("mcp");
    let path = dir.join("a.pdf");
    let input = [
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } } }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "doc_open", "arguments": { "path": path } } }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "page_render", "arguments": { "doc": 1, "page": 2, "dpi": 36 } } }),
    ]
    .iter()
    .map(Value::to_string)
    .collect::<Vec<_>>()
    .join("\n");
    let mut out = Vec::new();
    McpServer::new(Automation::new()).serve(input.as_bytes(), &mut out).unwrap();
    let replies: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(replies.len(), 4, "the notification gets no reply");

    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "pdfcraft");
    assert_eq!(replies[1]["result"]["tools"].as_array().unwrap().len(), tools().len());
    assert_eq!(replies[2]["result"]["structuredContent"]["pages"], 3);
    assert_eq!(replies[2]["result"]["isError"], false);
    let img = &replies[3]["result"]["content"][0];
    assert_eq!((img["type"].as_str(), img["mimeType"].as_str()), (Some("image"), Some("image/png")));
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD.decode(img["data"].as_str().unwrap()).unwrap();
    assert_eq!(&png[1..4], b"PNG");
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_errors() {
    let mut s = McpServer::new(Automation::new());
    assert_eq!(rpc(&mut s, 1, "initialize", json!({ "protocolVersion": "1999-01-01" }))["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(rpc(&mut s, 2, "ping", json!({}))["result"], json!({}));
    assert_eq!(rpc(&mut s, 3, "prompts/list", json!({}))["error"]["code"], -32601);
    assert_eq!(rpc(&mut s, 6, "resources/read", json!({ "uri": "pdfcraft://doc/9/info" }))["error"]["code"], -32602);
    assert_eq!(rpc(&mut s, 4, "tools/call", json!({ "name": "nope" }))["error"]["code"], -32602);
    let failed = rpc(&mut s, 5, "tools/call", json!({ "name": "doc_open", "arguments": { "path": "/definitely/not/here.pdf" } }));
    assert_eq!(failed["result"]["isError"], true);
    assert!(failed["result"]["content"][0]["text"].as_str().unwrap().contains("here.pdf"));
    let bad: Value = serde_json::from_str(&s.handle_line("{not json").unwrap()).unwrap();
    assert_eq!(bad["error"]["code"], -32700);
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_resources_expose_open_documents() {
    let dir = workdir("mcp-resources");
    let mut s = McpServer::new(auto(&dir));
    assert!(rpc(&mut s, 1, "initialize", json!({}))["result"]["capabilities"]["resources"].is_object());
    assert_eq!(rpc(&mut s, 2, "resources/list", json!({}))["result"]["resources"].as_array().unwrap().len(), 2);
    assert_eq!(rpc(&mut s, 3, "resources/templates/list", json!({}))["result"]["resourceTemplates"].as_array().unwrap().len(), 4);
    rpc(&mut s, 4, "tools/call", json!({ "name": "doc_open", "arguments": { "path": "a.pdf" } }));
    let list = rpc(&mut s, 5, "resources/list", json!({}))["result"]["resources"].as_array().cloned().unwrap();
    assert_eq!(list.len(), 2 + 3 + 2, "info, text, three page images and two session resources");
    assert_eq!(list[0]["uri"], "pdfcraft://doc/1/info");
    let read = |s: &mut McpServer, uri: &str| rpc(s, 6, "resources/read", json!({ "uri": uri }))["result"]["contents"][0].clone();
    let text = read(&mut s, "pdfcraft://doc/1/text");
    assert_eq!(text["mimeType"], "text/plain");
    assert!(text["text"].as_str().unwrap().contains("Page 2\nPage 2"), "{text}");
    assert_eq!(read(&mut s, "pdfcraft://doc/1/page/3/text")["text"], "Page 3");
    let info: Value = serde_json::from_str(read(&mut s, "pdfcraft://doc/1/info")["text"].as_str().unwrap()).unwrap();
    assert_eq!(info["pages"].as_array().unwrap().len(), 3);
    let img = read(&mut s, "pdfcraft://doc/1/page/1/image?dpi=36");
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD.decode(img["blob"].as_str().unwrap()).unwrap();
    assert_eq!(&png[1..4], b"PNG");
    assert_eq!(rpc(&mut s, 7, "resources/read", json!({ "uri": "pdfcraft://doc/1/page/9/image" }))["error"]["code"], -32602);
}

/// A server in compact mode with `dir` as its root.
#[cfg(feature = "mcp")]
fn compact_server(dir: &Path) -> McpServer {
    McpServer::new(auto(dir)).with_compact(true)
}

/// Run a tool through the compact server's `tool_call`.
#[cfg(feature = "mcp")]
fn via_tool_call(s: &mut McpServer, id: u64, tool: &str, arguments: Value) -> Value {
    rpc(s, id, "tools/call", json!({ "name": "tool_call", "arguments": { "name": tool, "arguments": arguments } }))
}

#[cfg(feature = "mcp")]
fn tool_names(reply: &Value) -> Vec<String> {
    reply["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_default_lists_every_tool_and_has_no_meta_tools() {
    let mut s = McpServer::new(Automation::new());
    let names = tool_names(&rpc(&mut s, 1, "tools/list", json!({})));
    assert_eq!(names.len(), tools().len());
    assert!(!names.iter().any(|n| n == "tool_search" || n == "tool_call"));
    assert!(!rpc(&mut s, 2, "initialize", json!({}))["result"]["instructions"].as_str().unwrap().contains("tool_search"));
    // Without --compact the meta tools do not exist.
    assert_eq!(rpc(&mut s, 3, "tools/call", json!({ "name": "tool_search" }))["error"]["code"], -32602);
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_compact_lists_the_core_tools_and_two_meta_tools() {
    let mut s = McpServer::new(Automation::new()).with_compact(true);
    let list = rpc(&mut s, 1, "tools/list", json!({}));
    let mut expected: Vec<String> = pdfcraft_automation::mcp::COMPACT_CORE_TOOLS.iter().map(|n| n.to_string()).collect();
    expected.extend(["tool_search".to_string(), "tool_call".to_string()]);
    assert_eq!(tool_names(&list), expected);
    let all: Vec<&str> = tools().iter().map(|t| t.name).collect();
    for core in pdfcraft_automation::mcp::COMPACT_CORE_TOOLS {
        assert!(all.contains(core), "core tool {core} is not in the tool table");
    }
    // Core tools are the real definitions, and the list is far smaller than the full one.
    let full = rpc(&mut McpServer::new(Automation::new()), 2, "tools/list", json!({}));
    let core_open = list["result"]["tools"][0].clone();
    assert_eq!(Some(&core_open), full["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "doc_open"));
    assert!(list.to_string().len() * 5 < full.to_string().len(), "compact {} vs full {}", list.to_string().len(), full.to_string().len());
    for meta in &list["result"]["tools"].as_array().unwrap()[pdfcraft_automation::mcp::COMPACT_CORE_TOOLS.len()..] {
        assert_eq!(meta["inputSchema"]["type"], "object");
    }
    let instructions = rpc(&mut s, 3, "initialize", json!({}))["result"]["instructions"].as_str().unwrap().to_string();
    assert!(instructions.contains("tool_search") && instructions.contains("tool_call"), "{instructions}");
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_compact_tool_search_filters_by_query_and_category() {
    let mut s = McpServer::new(Automation::new()).with_compact(true);
    let mut search = |args: Value| rpc(&mut s, 1, "tools/call", json!({ "name": "tool_search", "arguments": args }));

    let everything = search(json!({}));
    assert_eq!(everything["result"]["isError"], false);
    assert_eq!(everything["result"]["structuredContent"]["count"], tools().len());
    let first = &everything["result"]["structuredContent"]["tools"][0];
    assert!(first["name"].is_string() && first["description"].is_string() && first["read_only"].is_boolean(), "{first}");

    let rotate = search(json!({ "query": "ROTATE" }))["result"]["structuredContent"].clone();
    let names: Vec<&str> = rotate["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"page_rotate"), "{names:?}");
    assert!(names.len() < tools().len());

    let docs = search(json!({ "category": "doc" }))["result"]["structuredContent"].clone();
    let expected = tools().iter().filter(|t| t.name.starts_with("doc_")).count();
    assert!(expected > 0);
    assert_eq!(docs["count"], expected);
    assert!(docs["tools"].as_array().unwrap().iter().all(|t| t["name"].as_str().unwrap().starts_with("doc_") && t["category"] == "doc"));

    let both = search(json!({ "category": "page", "query": "rotate" }))["result"]["structuredContent"].clone();
    assert!(both["tools"].as_array().unwrap().iter().all(|t| t["name"].as_str().unwrap().starts_with("page_")));
    assert!(both["tools"].as_array().unwrap().iter().any(|t| t["name"] == "page_rotate"));

    // No match is an empty result that lists the categories, not an error.
    let none = search(json!({ "category": "nonsense" }));
    assert_eq!(none["result"]["isError"], false);
    assert_eq!(none["result"]["structuredContent"]["count"], 0);
    assert!(none["result"]["structuredContent"]["categories"].as_array().unwrap().iter().any(|c| c == "page"));

    // One tool in full.
    let one = search(json!({ "name": "page_rotate" }))["result"]["structuredContent"]["tool"].clone();
    assert_eq!(one["name"], "page_rotate");
    assert_eq!(one["input_schema"]["type"], "object");
    let def = tools().into_iter().find(|t| t.name == "page_rotate").unwrap();
    assert_eq!(one["input_schema"], def.input_schema);
    assert_eq!(one["description"], def.description);

    // Bad arguments are tool errors.
    for bad in [json!({ "name": "no_such_tool" }), json!({ "query": 3 }), json!({ "category": ["doc"] }), json!("doc")] {
        let r = search(bad.clone());
        assert_eq!(r["result"]["isError"], true, "{bad}");
        assert!(r["result"]["content"][0]["text"].as_str().is_some_and(|t| !t.is_empty()));
    }
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_compact_tool_call_dispatches_to_any_tool() {
    let dir = workdir("mcp-compact");
    let mut s = compact_server(&dir);
    let opened = via_tool_call(&mut s, 1, "doc_open", json!({ "path": "a.pdf" }));
    assert_eq!(opened["result"]["isError"], false);
    assert_eq!(opened["result"]["structuredContent"]["pages"], 3);

    // A tool outside the core list, through the meta tool.
    let deleted = via_tool_call(&mut s, 2, "page_delete", json!({ "doc": 1, "pages": [3] }));
    assert_eq!(deleted["result"]["isError"], false, "{deleted}");
    assert_eq!(deleted["result"]["structuredContent"]["pages"], 2);

    // The same tool called by name directly still works, and sees the same session.
    let listed = rpc(&mut s, 3, "tools/call", json!({ "name": "doc_list", "arguments": {} }));
    assert_eq!(listed["result"]["isError"], false);
    assert_eq!(listed["result"]["structuredContent"]["documents"][0]["pages"], 2);

    // Images come back as images.
    let png = via_tool_call(&mut s, 4, "page_render", json!({ "doc": 1, "page": 1, "dpi": 36 }));
    assert_eq!(png["result"]["content"][0]["type"], "image");

    // `arguments` may be left out for tools that need none.
    assert_eq!(via_tool_call(&mut s, 5, "doc_list", Value::Null)["result"]["isError"], false);
    let no_args = rpc(&mut s, 6, "tools/call", json!({ "name": "tool_call", "arguments": { "name": "doc_list" } }));
    assert_eq!(no_args["result"]["isError"], false);

    // The result equals what the direct call returns.
    let direct = rpc(&mut s, 7, "tools/call", json!({ "name": "doc_info", "arguments": { "doc": 1 } }));
    let wrapped = via_tool_call(&mut s, 8, "doc_info", json!({ "doc": 1 }));
    assert_eq!(direct["result"], wrapped["result"]);
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_compact_tool_call_reports_errors_without_panicking() {
    let dir = workdir("mcp-compact-errors");
    let mut s = compact_server(&dir);
    let text = |r: &Value| r["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string();

    let unknown = via_tool_call(&mut s, 1, "no_such_tool", json!({}));
    assert_eq!(unknown["result"]["isError"], true);
    assert!(text(&unknown).contains("no_such_tool") && text(&unknown).contains("tool_search"), "{}", text(&unknown));

    // The tool's own validation errors pass through.
    let missing = via_tool_call(&mut s, 2, "doc_open", json!({}));
    assert_eq!(missing["result"]["isError"], true);
    let outside = via_tool_call(&mut s, 3, "doc_open", json!({ "path": "/definitely/not/here.pdf" }));
    assert_eq!(outside["result"]["isError"], true);
    let wrong_type = via_tool_call(&mut s, 4, "page_delete", json!({ "doc": "one", "pages": "all" }));
    assert_eq!(wrong_type["result"]["isError"], true);

    // Malformed meta-tool arguments.
    for args in [
        json!({}),
        json!({ "name": 7 }),
        json!({ "name": "doc_list", "arguments": [1] }),
        json!("doc_list"),
        Value::Null,
        json!({ "name": "tool_call" }),
        json!({ "name": "tool_search" }),
    ] {
        let r = rpc(&mut s, 5, "tools/call", json!({ "name": "tool_call", "arguments": args }));
        assert_eq!(r["result"]["isError"], true, "{args}");
        assert!(!text(&r).is_empty());
    }

    // Direct calls keep the usual protocol error for an unknown tool, and the session survives all of it.
    assert_eq!(rpc(&mut s, 6, "tools/call", json!({ "name": "nope" }))["error"]["code"], -32602);
    assert_eq!(via_tool_call(&mut s, 7, "doc_open", json!({ "path": "a.pdf" }))["result"]["isError"], false);
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_compact_tool_search_summarizes_the_first_sentence() {
    let mut s = McpServer::new(Automation::new()).with_compact(true);
    let mut summary = |tool: &str| {
        let found = rpc(
            &mut s,
            1,
            "tools/call",
            json!({ "name": "tool_search", "arguments": { "query": tool, "category": tool.split('_').next().unwrap() } }),
        );
        found["result"]["structuredContent"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == tool).unwrap()["description"]
            .as_str()
            .unwrap()
            .to_string()
    };
    // Abbreviations such as "e.g." do not end the summary.
    assert_eq!(summary("page_number"), "Label a range of pages (e.g. i, ii, iii for front matter, or A-1, A-2 for an appendix).");
    assert_eq!(
        summary("accessibility_fix"),
        "Apply the checker's automatic fix for a rule: primary-language (value: the language, e.g. en-US), title (value: the title; default the current title or file name; also shows it in the title bar) or tab-order (every page tabs in structure order)."
    );
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_compact_meta_tools_reject_unknown_arguments() {
    let dir = workdir("mcp-compact-keys");
    let mut s = compact_server(&dir);
    let list = rpc(&mut s, 1, "tools/list", json!({}));
    let metas = &list["result"]["tools"].as_array().unwrap()[pdfcraft_automation::mcp::COMPACT_CORE_TOOLS.len()..];
    assert_eq!(metas.len(), 2);
    for meta in metas {
        assert_eq!(meta["inputSchema"]["additionalProperties"], false, "{}", meta["name"]);
    }
    let text = |r: &Value| r["error"]["message"].as_str().or_else(|| r["result"]["content"][0]["text"].as_str()).unwrap_or_default().to_string();

    // A misspelled filter is an error, not an unfiltered listing.
    let typo = rpc(&mut s, 2, "tools/call", json!({ "name": "tool_search", "arguments": { "qurey": "rotate" } }));
    assert_eq!(typo["error"]["code"], -32602, "{typo}");
    assert!(text(&typo).contains("\"qurey\"") && text(&typo).contains("query"), "{}", text(&typo));
    let extra = rpc(&mut s, 3, "tools/call", json!({ "name": "tool_search", "arguments": { "query": "rotate", "extra": 1 } }));
    assert_eq!(extra["error"]["code"], -32602, "{extra}");
    assert!(text(&extra).contains("\"extra\""), "{}", text(&extra));

    // tool_call rejects extra outer keys before running anything.
    let misspelled =
        rpc(&mut s, 4, "tools/call", json!({ "name": "tool_call", "arguments": { "name": "doc_open", "argumentz": { "path": "a.pdf" } } }));
    assert_eq!(misspelled["error"]["code"], -32602, "{misspelled}");
    assert!(text(&misspelled).contains("\"argumentz\""), "{}", text(&misspelled));
    let extra = rpc(&mut s, 5, "tools/call", json!({ "name": "tool_call", "arguments": { "name": "doc_list", "unexpected": "value" } }));
    assert_eq!(extra["error"]["code"], -32602, "{extra}");
    assert!(text(&extra).contains("\"unexpected\""), "{}", text(&extra));
    assert_eq!(rpc(&mut s, 6, "tools/call", json!({ "name": "doc_list", "arguments": {} }))["result"]["structuredContent"]["documents"], json!([]));

    // Keys inside "arguments" are left to the target tool, which rejects its unknown ones itself.
    let inner = via_tool_call(&mut s, 7, "doc_list", json!({ "unexpected": "value" }));
    assert_eq!(inner["result"]["isError"], true, "{inner}");
    assert!(text(&inner).contains("doc_list: unknown argument \"unexpected\""), "{}", text(&inner));
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_compact_tool_call_keeps_the_root_confinement() {
    let root = workdir("mcp-compact-root");
    let outside_dir = workdir("mcp-compact-outside");
    let outside = outside_dir.join("a.pdf").to_string_lossy().into_owned();
    let text = |r: &Value| r["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string();

    // Direct call and tool_call both refuse a real PDF outside the root, with the same words.
    let mut confined = compact_server(&root);
    let direct = rpc(&mut confined, 1, "tools/call", json!({ "name": "doc_open", "arguments": { "path": outside } }));
    assert_eq!(direct["result"]["isError"], true, "{direct}");
    assert!(text(&direct).contains("is outside the allowed directory"), "{}", text(&direct));
    let wrapped = via_tool_call(&mut confined, 2, "doc_open", json!({ "path": outside }));
    assert_eq!(wrapped["result"]["isError"], true, "{wrapped}");
    assert_eq!(text(&wrapped), text(&direct));
    // Nothing was opened.
    let listed = rpc(&mut confined, 3, "tools/call", json!({ "name": "doc_list", "arguments": {} }));
    assert_eq!(listed["result"]["structuredContent"]["documents"], json!([]));

    // The same file opens through tool_call on a server without a root, so the refusal came from the root.
    let mut open = McpServer::new(Automation::new()).with_compact(true);
    let opened = via_tool_call(&mut open, 4, "doc_open", json!({ "path": outside }));
    assert_eq!(opened["result"]["isError"], false, "{opened}");
    assert_eq!(opened["result"]["structuredContent"]["pages"], 3);

    // And a file inside the root still opens through the confined server.
    assert_eq!(via_tool_call(&mut confined, 5, "doc_open", json!({ "path": "a.pdf" }))["result"]["isError"], false);
}

#[test]
fn parallel_text_extraction_keeps_page_order_and_follows_edits() {
    let dir = workdir("parallel");
    std::fs::write(dir.join("long.pdf"), fixture(40)).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "long.pdf" }))["doc"].as_u64().unwrap();
    let expected: Vec<String> = (1..=40).map(|i| format!("Page {i}")).collect();
    assert_eq!(page_text(&mut a, doc), expected);
    assert_eq!(ok(&mut a, "text_find", json!({ "doc": doc, "query": "page 3" }))["count"], 11); // 3, 30–39
    ok(&mut a, "page_delete", json!({ "doc": doc, "pages": [3] }));
    assert_eq!(ok(&mut a, "text_find", json!({ "doc": doc, "query": "page 3" }))["count"], 10);
    assert_eq!(page_text(&mut a, doc)[2], "Page 4");
}

#[test]
fn bookmarks_through_tools() {
    let dir = workdir("bookmarks");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "bookmark_list", json!({ "doc": doc }))["bookmarks"], json!([]));
    ok(&mut a, "bookmark_add", json!({ "doc": doc, "title": "Intro", "page": 1 }));
    ok(&mut a, "bookmark_add", json!({ "doc": doc, "title": "Body", "page": 2 }));
    ok(&mut a, "bookmark_add", json!({ "doc": doc, "title": "Detail", "page": 3, "parent": [2] }));
    ok(&mut a, "bookmark_move", json!({ "doc": doc, "path": [1], "parent": [2], "position": 1 })); // Intro under Body
    ok(&mut a, "bookmark_rename", json!({ "doc": doc, "path": [1, 2], "title": "Details" }));
    ok(&mut a, "bookmark_set_page", json!({ "doc": doc, "path": [1, 1], "page": 3 }));
    let list = ok(&mut a, "bookmark_list", json!({ "doc": doc }))["bookmarks"].clone();
    assert_eq!(list[0]["title"], "Body");
    assert_eq!(list[0]["children"][0]["title"], "Intro");
    assert_eq!(list[0]["children"][0]["page"], 3);
    assert_eq!(list[0]["children"][0]["path"], json!([1, 1]));
    assert_eq!(list[0]["children"][1]["title"], "Details");
    assert!(matches!(a.call("bookmark_delete", &json!({ "doc": doc, "path": [9] })), Err(ToolError::Failed(_))));
    assert!(matches!(a.call("bookmark_delete", &json!({ "doc": doc, "path": [0] })), Err(ToolError::InvalidArgs(_))));
    ok(&mut a, "bookmark_delete", json!({ "doc": doc, "path": [1, 1] }));
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "marked.pdf" }));
    let mut b = auto(&dir);
    let re = ok(&mut b, "doc_open", json!({ "path": "marked.pdf" }))["doc"].as_u64().unwrap();
    let list = ok(&mut b, "bookmark_list", json!({ "doc": re }))["bookmarks"].clone();
    assert_eq!((list[0]["title"].as_str(), list[0]["children"][0]["title"].as_str()), (Some("Body"), Some("Details")));
}

/// A flat outline of `n` top-level bookmarks, `Item 0` to `Item n-1`, on one page.
fn flat_outline_pdf(n: usize) -> Vec<u8> {
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /Outlines 3 0 R >>".into(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
        format!("<< /Type /Outlines /First 5 0 R /Last {} 0 R /Count {n} >>", 4 + n),
        "<< /Type /Page /Parent 2 0 R >>".into(),
    ];
    for k in 0..n {
        let prev = if k == 0 { String::new() } else { format!(" /Prev {} 0 R", 4 + k) };
        let next = if k + 1 == n { String::new() } else { format!(" /Next {} 0 R", 6 + k) };
        objs.push(format!("<< /Title (Item {k}) /Parent 3 0 R{prev}{next} /Dest [4 0 R /Fit] >>"));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

#[test]
fn bookmark_list_pages_reach_bookmarks_past_the_hundred_thousandth() {
    let dir = workdir("bookmarks-large");
    std::fs::write(dir.join("large.pdf"), flat_outline_pdf(100_005)).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "large.pdf" }))["doc"].as_u64().unwrap();
    // The default call is the first page of 100, with the offset of the next one.
    let first = ok(&mut a, "bookmark_list", json!({ "doc": doc }));
    assert_eq!((first["bookmarks"].as_array().unwrap().len(), first["next"].clone()), (100, json!(100)));
    assert_eq!((first["bookmarks"][0]["title"].clone(), first["bookmarks"][0]["path"].clone()), (json!("Item 0"), json!([1])));
    let late = ok(&mut a, "bookmark_list", json!({ "doc": doc, "offset": 100_000, "limit": 100 }));
    let titles: Vec<&str> = late["bookmarks"].as_array().unwrap().iter().filter_map(|b| b["title"].as_str()).collect();
    assert_eq!((titles.first().copied(), titles.last().copied(), late["next"].clone()), (Some("Item 100000"), Some("Item 100004"), json!(null)));
    assert!(matches!(a.call("bookmark_list", &json!({ "doc": doc, "limit": 1001 })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(a.call("bookmark_list", &json!({ "doc": doc, "offset": -1 })), Err(ToolError::InvalidArgs(_))));
}

/// Bookmarks and links say where on the page they go, in the same top-left points as rect, and a
/// link through a named destination says which page that is.
#[test]
fn destinations_are_listed_with_their_position() {
    let dir = workdir("destination-positions");
    let objs: Vec<&str> = vec![
        "<< /Type /Catalog /Pages 2 0 R /Outlines 5 0 R /Names << /Dests << /Names [(sec-b) [4 0 R /XYZ 72 500 0]] >> >> >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 612 792] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [8 0 R 9 0 R 10 0 R] >>",
        "<< /Type /Page /Parent 2 0 R >>",
        "<< /Type /Outlines /First 6 0 R /Last 7 0 R /Count 2 >>",
        "<< /Title (Section B) /Parent 5 0 R /Next 7 0 R /Dest [4 0 R /XYZ 72 500 0] >>",
        "<< /Title (Wide view) /Parent 5 0 R /Prev 6 0 R /Dest [4 0 R /FitH 400] >>",
        "<< /Type /Annot /Subtype /Link /Rect [72 700 300 716] /Border [0 0 0] /Dest [4 0 R /XYZ 72 500 2] >>",
        "<< /Type /Annot /Subtype /Link /Rect [72 670 300 686] /Border [0 0 0] /Dest (sec-b) >>",
        "<< /Type /Annot /Subtype /Link /Rect [72 640 300 656] /Border [0 0 0] /A << /S /URI /URI (https://example.com) >> >>",
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    std::fs::write(dir.join("dests.pdf"), pdf).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "dests.pdf" }))["doc"].as_u64().unwrap();
    let marks = ok(&mut a, "bookmark_list", json!({ "doc": doc }));
    // 792 − 500 = 292 points from the top; a zoom of 0 keeps the reader's zoom.
    assert_eq!(marks["bookmarks"][0]["destination"], json!({ "fit": "XYZ", "left": 72.0, "top": 292.0, "zoom": null }), "{marks}");
    assert_eq!(marks["bookmarks"][1]["destination"], json!({ "fit": "FitH", "left": null, "top": 392.0 }), "{marks}");
    let links = ok(&mut a, "link_list", json!({ "doc": doc }));
    let explicit = &links["links"][0];
    assert_eq!(
        (explicit["to_page"].clone(), explicit["destination"].clone()),
        (json!(2), json!({ "fit": "XYZ", "left": 72.0, "top": 292.0, "zoom": 2.0 }))
    );
    let named = &links["links"][1];
    assert_eq!((named["to_page"].clone(), named["named"].clone(), named.get("other")), (json!(2), json!(true), None), "{links}");
    assert_eq!(named["destination"]["top"], 292.0);
    assert_eq!((links["links"][2]["url"].clone(), links["links"][2].get("destination")), (json!("https://example.com"), None));
}

#[test]
fn bookmarks_from_structure_through_tools() {
    let dir = workdir("bookmarks-structure");
    std::fs::write(
        dir.join("tagged.pdf"),
        b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /StructTreeRoot 5 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 200] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
5 0 obj << /Type /StructTreeRoot /K 6 0 R /RoleMap << /Heading /H1 >> >> endobj
6 0 obj << /S /Document /K [7 0 R << /S /Sect /K 8 0 R >>] >> endobj
7 0 obj << /S /Heading /Pg 3 0 R /ActualText (Report) >> endobj
8 0 obj << /S /H2 /Pg 4 0 R /Alt (Findings) >> endobj
trailer << /Root 1 0 R >>
%%EOF"
            .as_slice(),
    )
    .unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "tagged.pdf" }))["doc"].as_u64().unwrap();
    let r = ok(&mut a, "bookmark_from_structure", json!({ "doc": doc }));
    let top = &r["bookmarks"][0];
    assert_eq!(
        (top["title"].as_str(), top["children"][0]["title"].as_str(), top["children"][0]["page"].as_u64()),
        (Some("Untitled"), Some("Report"), Some(1))
    );
    assert_eq!(top["children"][0]["children"][0]["title"], "Findings");
    assert_eq!(top["children"][0]["children"][0]["path"], json!([1, 1, 1]));
    assert_eq!(ok(&mut a, "edit_undo", json!({ "doc": doc }))["undone"], "New bookmarks from structure");
    // An untagged document: an error that says why.
    let plain = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    assert!(matches!(a.call("bookmark_from_structure", &json!({ "doc": plain })), Err(ToolError::Failed(m)) if m.contains("no tagged headings")));
}

#[test]
fn numbering_pages_through_tools() {
    let dir = workdir("labels");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let r = ok(&mut a, "page_number", json!({ "doc": doc, "from": 1, "to": 1, "style": "upper-roman" }));
    assert_eq!(r["labels"], json!(["I", "2", "3"]));
    let r = ok(&mut a, "page_number", json!({ "doc": doc, "from": 2, "to": 3, "prefix": "B-", "start": 5 }));
    assert_eq!(r["labels"], json!(["I", "B-5", "B-6"]));
    assert!(matches!(a.call("page_number", &json!({ "doc": doc, "from": 2, "to": 4 })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(a.call("page_number", &json!({ "doc": doc, "from": 1, "to": 1, "style": "klingon" })), Err(ToolError::InvalidArgs(_))));
}

#[test]
fn comments_through_tools() {
    let dir = workdir("comments");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    // "Page 2" sits at y = 150 (user space) = 150 from the top of the 300 pt page.
    let hl = ok(&mut a, "comment_add", json!({ "doc": doc, "page": 2, "type": "highlight", "find": "page 2", "contents": "check", "author": "Ada" }));
    assert_eq!(hl["comment"]["lines"], 1);
    let id = hl["comment"]["id"].as_str().unwrap().to_string();
    ok(&mut a, "comment_add", json!({ "doc": doc, "page": 2, "type": "note", "at": [150, 20], "contents": "Sticky" }));
    ok(&mut a, "comment_add", json!({ "doc": doc, "page": 1, "type": "rectangle", "rect": [10, 10, 60, 40], "color": "blue", "width": 3 }));
    ok(&mut a, "comment_add", json!({ "doc": doc, "page": 1, "type": "textbox", "rect": [10, 200, 190, 240], "contents": "Hello", "font_size": 10 }));
    ok(&mut a, "comment_add", json!({ "doc": doc, "page": 1, "type": "ink", "strokes": [[[10, 280], [50, 260], [90, 285]]] }));
    ok(&mut a, "comment_add", json!({ "doc": doc, "page": 3, "type": "arrow", "from": [10, 10], "to": [100, 100] }));
    ok(&mut a, "comment_reply", json!({ "doc": doc, "id": id, "text": "Looks right", "author": "Bob" }));
    ok(&mut a, "comment_set_status", json!({ "doc": doc, "id": id, "status": "accepted", "author": "Bob" }));

    let list = ok(&mut a, "comment_list", json!({ "doc": doc }));
    assert_eq!(list["count"], 6, "{list}");
    let c = list["comments"].as_array().unwrap().iter().find(|c| c["id"] == id.as_str()).unwrap().clone();
    assert_eq!((c["type"].as_str(), c["author"].as_str(), c["contents"].as_str()), (Some("Highlight"), Some("Ada"), Some("check")));
    assert_eq!(c["status"], "Accepted");
    assert_eq!(c["replies"].as_array().unwrap().len(), 1);
    assert_eq!(c["replies"][0]["contents"], "Looks right");
    // The highlight covers the found text, reported in top-left-origin points.
    let r: Vec<f64> = c["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert!(r[0] >= 15.0 && r[0] <= 25.0 && r[1] > 120.0 && r[3] < 160.0, "{r:?}");
    let rect = list["comments"].as_array().unwrap().iter().find(|c| c["type"] == "Square").unwrap().clone();
    assert_eq!(rect["color"], "#0078D6");
    assert_eq!(rect["rect"], json!([10.0, 10.0, 60.0, 40.0]));
    let note = list["comments"].as_array().unwrap().iter().find(|c| c["type"] == "Text").unwrap().clone();
    assert_eq!(note["rect"], json!([150.0, 20.0, 170.0, 40.0]), "note icon hangs from its top-left point");

    // Edit by page + index; several changes are one undo step.
    let (page, index) = (rect["page"].as_u64().unwrap(), rect["index"].as_u64().unwrap());
    ok(&mut a, "comment_edit", json!({ "doc": doc, "page": page, "index": index, "color": "#FF0000", "move": [5, 5], "contents": "moved" }));
    let list = ok(&mut a, "comment_list", json!({ "doc": doc, "page": 1 }));
    let rect = list["comments"].as_array().unwrap().iter().find(|c| c["type"] == "Square").unwrap().clone();
    assert_eq!((rect["color"].as_str(), rect["contents"].as_str()), (Some("#FF0000"), Some("moved")));
    assert_eq!(rect["rect"], json!([15.0, 15.0, 65.0, 45.0]));
    let undo = ok(&mut a, "edit_undo", json!({ "doc": doc }));
    assert_eq!(undo["undone"], "Edit comment");
    ok(&mut a, "edit_redo", json!({ "doc": doc }));
    // Rectangles, ovals and polygons take a fill (#686); "none" removes it. Lines have none.
    ok(&mut a, "comment_edit", json!({ "doc": doc, "page": page, "index": index, "fill": "yellow" }));
    ok(&mut a, "comment_edit", json!({ "doc": doc, "page": page, "index": index, "fill": "none" }));
    assert!(matches!(a.call("comment_edit", &json!({ "doc": doc, "page": page, "index": index, "fill": "mauve" })), Err(ToolError::InvalidArgs(_))));
    let arrow = ok(&mut a, "comment_list", json!({ "doc": doc, "page": 3 }))["comments"][0].clone();
    assert!(matches!(
        a.call("comment_edit", &json!({ "doc": doc, "page": arrow["page"], "index": arrow["index"], "fill": "red" })),
        Err(ToolError::Failed(_))
    ));

    // Editing a text box re-fits its rectangle to the new text: the wrap width and top edge
    // stay, the height follows the wrapped lines.
    let tb = list["comments"].as_array().unwrap().iter().find(|c| c["type"] == "FreeText").unwrap().clone();
    let long = "the quick brown fox jumps over the lazy dog ".repeat(3);
    ok(&mut a, "comment_edit", json!({ "doc": doc, "page": tb["page"], "index": tb["index"], "contents": long.trim_end() }));
    let list = ok(&mut a, "comment_list", json!({ "doc": doc, "page": 1 }));
    let tb = list["comments"].as_array().unwrap().iter().find(|c| c["type"] == "FreeText").unwrap().clone();
    let r: Vec<f64> = tb["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert_eq!((r[0], r[1], r[2] - r[0]), (10.0, 200.0, 180.0), "width and top edge stay: {r:?}");
    assert!(r[3] - r[1] > 40.0, "the box grew to fit the wrapped lines: {r:?}");

    // The rendered page shows the rectangle's border.
    let png = a.call("page_render", &json!({ "doc": doc, "page": 1, "dpi": 72 })).unwrap();
    assert!(matches!(png[0], Content::Png { .. }));

    ok(&mut a, "comment_delete", json!({ "doc": doc, "id": id }));
    assert_eq!(ok(&mut a, "comment_list", json!({ "doc": doc }))["count"], 5);
    assert!(matches!(a.call("comment_delete", &json!({ "doc": doc, "id": id })), Err(ToolError::Failed(_))));
    assert!(matches!(a.call("comment_add", &json!({ "doc": doc, "page": 1, "type": "highlight", "find": "nowhere" })), Err(ToolError::Failed(_))));
    assert!(matches!(a.call("comment_add", &json!({ "doc": doc, "page": 1, "type": "rectangle" })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(
        a.call("comment_add", &json!({ "doc": doc, "page": 1, "type": "note", "at": [1, 1], "color": "mauve" })),
        Err(ToolError::InvalidArgs(_))
    ));
    assert!(matches!(a.call("comment_edit", &json!({ "doc": doc, "page": 1, "index": 1 })), Err(ToolError::InvalidArgs(_))));

    // Comments survive a save and reopen.
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "commented.pdf" }));
    let mut b = auto(&dir);
    let re = ok(&mut b, "doc_open", json!({ "path": "commented.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut b, "comment_list", json!({ "doc": re }))["count"], 5);
}

/// #806: a reply to another reply stays in the listing, at any depth.
#[test]
fn nested_replies_stay_in_the_listing() {
    let dir = workdir("nested-replies");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let root = ok(&mut a, "comment_add", json!({ "doc": doc, "page": 1, "type": "note", "at": [40, 40], "contents": "ROOT_NOTE", "author": "A" }));
    let root_id = root["comment"]["id"].as_str().unwrap().to_string();
    ok(&mut a, "comment_reply", json!({ "doc": doc, "id": root_id, "text": "DIRECT_REPLY", "author": "B" }));
    let listed = ok(&mut a, "comment_list", json!({ "doc": doc }));
    let root_comment = listed["comments"].as_array().unwrap().iter().find(|c| c["contents"] == "ROOT_NOTE").unwrap().clone();
    let direct_id =
        root_comment["replies"].as_array().unwrap().iter().find(|r| r["contents"] == "DIRECT_REPLY").unwrap()["id"].as_str().unwrap().to_string();
    ok(&mut a, "comment_reply", json!({ "doc": doc, "id": direct_id, "text": "NESTED_REPLY", "author": "C" }));
    ok(&mut a, "comment_add", json!({ "doc": doc, "page": 1, "type": "note", "at": [120, 120], "contents": "CONTROL_NOTE", "author": "D" }));

    let list = ok(&mut a, "comment_list", json!({ "doc": doc }));
    assert_eq!(list["count"], 2, "the two notes stay roots: {list}");
    let root = list["comments"].as_array().unwrap().iter().find(|c| c["contents"] == "ROOT_NOTE").unwrap().clone();
    let replies: Vec<&str> = root["replies"].as_array().unwrap().iter().filter_map(|r| r["contents"].as_str()).collect();
    assert!(replies.contains(&"DIRECT_REPLY") && replies.contains(&"NESTED_REPLY"), "the nested reply is listed: {replies:?}");
}

#[test]
fn protecting_through_tools() {
    let dir = workdir("protect");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "doc_info", json!({ "doc": doc }))["security"]["protected"], false);
    assert!(matches!(a.call("doc_protect", &json!({ "doc": doc })), Err(ToolError::Failed(_))), "a password is required");
    let r = ok(
        &mut a,
        "doc_protect",
        json!({ "doc": doc, "open_password": "open", "permissions_password": "boss", "printing": "low", "changes": "comment-fill-sign" }),
    );
    assert_eq!(r["security"]["pending"], true);
    assert_eq!(
        (r["security"]["printing"].as_str(), r["security"]["annotate"].as_bool(), r["security"]["copy"].as_bool()),
        (Some("low"), Some(true), Some(false))
    );
    assert!(!r.to_string().contains("boss"), "passwords are never echoed");
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "locked.pdf" }));
    // Another session: the open password is required, and the restrictions hold.
    let mut b = auto(&dir);
    assert!(matches!(b.call("doc_open", &json!({ "path": "locked.pdf" })), Err(ToolError::Failed(_))));
    let re = ok(&mut b, "doc_open", json!({ "path": "locked.pdf", "password": "open" }))["doc"].as_u64().unwrap();
    assert!(matches!(b.call("page_delete", &json!({ "doc": re, "pages": [1] })), Err(ToolError::Failed(_))));
    assert!(matches!(b.call("doc_unprotect", &json!({ "doc": re })), Err(ToolError::Failed(_))));
    ok(&mut b, "comment_add", json!({ "doc": re, "page": 1, "type": "note", "at": [10, 10], "contents": "allowed" }));
    // Copying wasn't allowed either (#568): no text or images come out, though search works.
    for (tool, args) in [
        ("text_extract", json!({ "doc": re })),
        ("text_word_count", json!({ "doc": re })),
        ("text_paragraphs", json!({ "doc": re, "page": 1 })),
        ("doc_export_text", json!({ "doc": re, "path": "out.txt" })),
        ("doc_export_images", json!({ "doc": re, "folder": "pages" })),
        ("doc_export_all_images", json!({ "doc": re, "folder": "images" })),
        ("doc_export_office", json!({ "doc": re, "path": "out.docx" })),
    ] {
        match b.call(tool, &args) {
            Err(ToolError::Failed(m)) => assert!(m.contains("don't allow copying"), "{tool}: {m}"),
            other => panic!("{tool} is refused: {other:?}"),
        }
    }
    assert!(!dir.join("out.txt").exists() && !dir.join("pages").exists() && !dir.join("out.docx").exists());
    assert_eq!(ok(&mut b, "text_find", json!({ "doc": re, "query": "Page" }))["count"], 3);
    // With the permissions password everything is possible, including removing security.
    let owner = ok(&mut b, "doc_open", json!({ "path": "locked.pdf", "password": "boss" }))["doc"].as_u64().unwrap();
    assert!(ok(&mut b, "text_extract", json!({ "doc": owner }))["pages"][0]["text"].as_str().is_some_and(|t| t.contains("Page")));
    assert_eq!(ok(&mut b, "doc_unprotect", json!({ "doc": owner }))["security"]["protected"], false);
    ok(&mut b, "doc_save", json!({ "doc": owner, "path": "open.pdf" }));
    let mut c = auto(&dir);
    ok(&mut c, "doc_open", json!({ "path": "open.pdf" }));
}

/// A form secured with "fill-sign" changes and no open password (how secured forms are usually
/// distributed): its existing signature field can be signed without the permissions password,
/// the update keeps the encryption, and the signature validates after reopening. A new field
/// needs the permissions password.
#[test]
fn signing_an_encrypted_form_through_tools() {
    let dir = workdir("sign-encrypted");
    let mut a = auto(&dir);
    ok(&mut a, "sign_id_create", json!({ "name": "Ada Lovelace", "key": "p256", "password": "secret12", "path": "ada.p12" }));
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let field = ok(&mut a, "form_add_field", json!({ "doc": doc, "page": 1, "type": "signature", "rect": [20, 200, 180, 250] }))["field"]
        .as_str()
        .unwrap()
        .to_owned();
    ok(&mut a, "doc_protect", json!({ "doc": doc, "permissions_password": "boss", "changes": "fill-sign" }));
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "form.pdf" }));
    let mut b = auto(&dir);
    let form = ok(&mut b, "doc_open", json!({ "path": "form.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut b, "doc_info", json!({ "doc": form }))["security"]["protected"], true);
    match b.call(
        "sign_document",
        &json!({ "doc": form, "id": "ada.p12", "password": "secret12", "page": 1, "rect": [200, 200, 380, 250], "out": "new.pdf" }),
    ) {
        Err(ToolError::Failed(m)) => assert!(m.contains("adding new signature fields"), "{m}"),
        other => panic!("a new field needs the permissions password: {other:?}"),
    }
    let r = ok(&mut b, "sign_document", json!({ "doc": form, "id": "ada.p12", "password": "secret12", "field": field, "out": "signed.pdf" }));
    assert_eq!((r["signature"]["signer"].as_str(), r["signature"]["status"].as_str()), (Some("Ada Lovelace"), Some("unknown")));
    let original = std::fs::read(dir.join("form.pdf")).unwrap();
    let signed = std::fs::read(dir.join("signed.pdf")).unwrap();
    assert!(signed.starts_with(&original), "an incremental update");
    let mut c = auto(&dir);
    let re = ok(&mut c, "doc_open", json!({ "path": "signed.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut c, "doc_info", json!({ "doc": re }))["security"]["protected"], true, "still encrypted");
    ok(&mut c, "sign_trust", json!({ "paths": ["ada.p12"], "password": "secret12" }));
    let list = ok(&mut c, "sign_list", json!({ "doc": re }));
    assert_eq!((list["all_valid"].as_bool(), list["signatures"][0]["status"].as_str()), (Some(true), Some("valid")), "{list}");
}

#[test]
fn combine_opens_protected_files_with_their_passwords() {
    let dir = workdir("combine-passwords");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    // Opens with "open"; only "boss" may assemble pages.
    ok(&mut a, "doc_protect", json!({ "doc": doc, "open_password": "open", "permissions_password": "boss", "changes": "none" }));
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "locked.pdf" }));
    let combine = |a: &mut Automation, passwords: Value| {
        a.call("doc_combine", &json!({ "paths": ["locked.pdf", "b.pdf"], "passwords": passwords, "open": true }))
    };
    let err = |r: Result<_, ToolError>| match r {
        Err(ToolError::Failed(m)) => m,
        Err(other) => panic!("expected a failure, got {other:?}"),
        Ok(_) => panic!("expected a failure"),
    };
    assert!(err(combine(&mut a, Value::Null)).contains("password-protected"));
    assert!(err(combine(&mut a, json!(["wrong", null]))).contains("password is wrong"));
    assert!(err(combine(&mut a, json!(["open", null]))).contains("don't allow copying pages"));
    assert!(matches!(combine(&mut a, json!(["boss"])), Err(ToolError::InvalidArgs(_))), "one per path");
    let done = ok(&mut a, "doc_combine", json!({ "paths": ["locked.pdf", "b.pdf"], "passwords": ["boss", null], "open": true }));
    assert!(!done.to_string().contains("boss"), "passwords are never echoed");
    let out = done["document"]["doc"].as_u64().unwrap();
    assert_eq!(page_text(&mut a, out), ["Page 1", "Page 2", "Page 3", "Page 1", "Page 2"]);
    assert_eq!(ok(&mut a, "doc_info", json!({ "doc": out }))["security"]["protected"], false, "the result is not encrypted");
}

/// Restrictions exist only behind a permissions password: open_password alone encrypts and
/// restricts nothing, and asking for a restriction without one is refused rather than ignored (#134).
#[test]
fn protecting_with_an_open_password_alone_restricts_nothing() {
    let dir = workdir("protect-open-only");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    for (key, value) in [("copy", json!(false)), ("changes", json!("none")), ("printing", json!("none")), ("accessibility", json!(false))] {
        match a.call("doc_protect", &json!({ "doc": doc, "open_password": "openme", key: value })) {
            Err(ToolError::InvalidArgs(m)) => assert!(m.contains(key) && m.contains("permissions_password"), "{key}: {m}"),
            other => panic!("{key} without permissions_password: {other:?}"),
        }
    }
    assert_eq!(ok(&mut a, "doc_info", json!({ "doc": doc }))["security"]["protected"], false, "a refused call changes nothing");
    let r = ok(&mut a, "doc_protect", json!({ "doc": doc, "open_password": "openme" }));
    assert_eq!(
        (r["security"]["protected"].as_bool(), r["security"]["copy"].as_bool(), r["security"]["modify"].as_bool()),
        (Some(true), Some(true), Some(true))
    );
    assert_eq!(r["security"]["printing"], "high");
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "open-only.pdf" }));
    let mut b = auto(&dir);
    let re = ok(&mut b, "doc_open", json!({ "path": "open-only.pdf", "password": "openme" }))["doc"].as_u64().unwrap();
    let s = ok(&mut b, "doc_info", json!({ "doc": re }))["security"].clone();
    assert_eq!(
        (s["protected"].as_bool(), s["copy"].as_bool(), s["modify"].as_bool(), s["printing"].as_str()),
        (Some(true), Some(true), Some(true), Some("high"))
    );
    ok(&mut b, "page_delete", json!({ "doc": re, "pages": [1] }));
    // The schema says so too.
    let def = tools().into_iter().find(|t| t.name == "doc_protect").unwrap();
    assert!(def.description.contains("permissions_password"), "{}", def.description);
    for key in ["printing", "changes", "copy", "accessibility"] {
        let desc = def.input_schema["properties"][key]["description"].as_str().unwrap();
        assert!(desc.contains("permissions_password"), "{key}: {desc}");
    }
}

/// AES-256 keys on the SASLprep form of a password, which drops soft hyphens: a password made
/// only of them is no password at all (as the permissions password, it would give everyone owner
/// rights), so it is refused rather than written.
#[test]
fn protecting_with_a_password_encryption_ignores_is_refused() {
    let dir = workdir("protect-ignored");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let answers: Vec<String> = ["open_password", "permissions_password"]
        .into_iter()
        .map(|key| match a.call("doc_protect", &json!({ "doc": doc, key: "\u{AD}" })) {
            Err(ToolError::Failed(m)) => m,
            other => format!("{key}: {other:?}"),
        })
        .collect();
    assert!(answers.iter().all(|m| m.contains("soft hyphens")), "{answers:?}");
    assert_eq!(ok(&mut a, "doc_info", json!({ "doc": doc }))["security"]["protected"], false, "a refused call changes nothing");
}

#[test]
fn forms_through_tools() {
    let dir = workdir("forms");
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/demo/pdfcraft-showcase.pdf");
    if !src.exists() {
        eprintln!("skipped: run `cargo xtask demo-pdf` for the showcase form");
        return;
    }
    std::fs::copy(&src, dir.join("show.pdf")).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "show.pdf" }))["doc"].as_u64().unwrap();
    let list = ok(&mut a, "form_fields", json!({ "doc": doc }));
    let fields = list["fields"].as_array().unwrap().clone();
    assert!(fields.len() >= 5, "{list}");
    let pick = |t: &str| fields.iter().find(|f| f["type"] == t && f["read_only"] == false).cloned();
    let text = pick("text").expect("a text field");
    let mut values = serde_json::Map::new();
    values.insert(text["name"].as_str().unwrap().into(), json!("Filled by an agent"));
    if let Some(cb) = pick("checkbox") {
        values.insert(cb["name"].as_str().unwrap().into(), json!(true));
    }
    if let Some(combo) = pick("combo") {
        values.insert(combo["name"].as_str().unwrap().into(), combo["options"][1]["label"].clone());
    }
    let r = ok(&mut a, "form_fill", json!({ "doc": doc, "values": values }));
    assert_eq!(r["undo"], "Fill in form");
    let after = ok(&mut a, "form_fields", json!({ "doc": doc }));
    let get = |name: &str| after["fields"].as_array().unwrap().iter().find(|f| f["name"] == name).unwrap()["value"].clone();
    assert_eq!(get(text["name"].as_str().unwrap()), "Filled by an agent");
    // The page shows it.
    let page = text["page"].as_u64().unwrap();
    let found = ok(&mut a, "text_find", json!({ "doc": doc, "query": "Filled by an agent" }));
    assert_eq!(found["count"], 1, "rendered on page {page}");
    assert!(matches!(a.call("form_fill", &json!({ "doc": doc, "values": { "no such field": "x" } })), Err(ToolError::Failed(_))));
    ok(&mut a, "form_reset", json!({ "doc": doc }));
    let reset = ok(&mut a, "form_fields", json!({ "doc": doc }));
    let v = reset["fields"].as_array().unwrap().iter().find(|f| f["name"] == text["name"]).unwrap()["value"].clone();
    assert_ne!(v, "Filled by an agent");
}

/// A form whose fields are only page widgets (the `/Fields` list is empty) still lists, fills and
/// resets through the tools, and the leniency shows up as a repair note.
#[test]
fn orphan_form_fields_through_tools() {
    /// One page with two text fields that appear only as page widgets: the AcroForm lists none.
    fn orphan_form() -> Vec<u8> {
        let objs: Vec<&str> = vec![
            "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>",                                     // 1
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",                     // 2
            "<< /Type /Page /Parent 2 0 R /Annots [5 0 R 6 0 R] >>",                                 // 3
            "<< /Fields [] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 7 0 R >> >> >>",               // 4
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (alpha) /Rect [10 200 90 220] /P 3 0 R >>", // 5
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (beta) /Rect [10 150 90 170] /P 3 0 R >>",  // 6
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",                                // 7
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }
    let dir = workdir("orphan-forms");
    std::fs::write(dir.join("orphan.pdf"), orphan_form()).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "orphan.pdf" }))["doc"].as_u64().unwrap();
    let list = ok(&mut a, "form_fields", json!({ "doc": doc }));
    assert_eq!(list["count"], 2, "{list}");
    let r = ok(&mut a, "form_fill", json!({ "doc": doc, "values": { "alpha": "Filled by an agent" } }));
    assert_eq!(r["undo"], "Fill in alpha");
    let after = ok(&mut a, "form_fields", json!({ "doc": doc }));
    let alpha = after["fields"].as_array().unwrap().iter().find(|f| f["name"] == "alpha").unwrap();
    assert_eq!(alpha["value"], "Filled by an agent");
    // The page shows it.
    assert_eq!(ok(&mut a, "text_find", json!({ "doc": doc, "query": "Filled by an agent" }))["count"], 1);
    // The leniency is not silent: the repair note names the adopted fields.
    let info = ok(&mut a, "doc_info", json!({ "doc": doc }));
    assert!(info["repairs"].as_array().unwrap().iter().any(|r| r.as_str().unwrap_or("").contains("page annotations")), "{}", info["repairs"]);
    ok(&mut a, "form_reset", json!({ "doc": doc }));
    let reset = ok(&mut a, "form_fields", json!({ "doc": doc }));
    let v = reset["fields"].as_array().unwrap().iter().find(|f| f["name"] == "alpha").unwrap()["value"].clone();
    assert_ne!(v, "Filled by an agent");
}

#[test]
fn duplicating_and_cropping_through_tools() {
    let dir = workdir("boxes");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let r = ok(&mut a, "page_duplicate", json!({ "doc": doc, "pages": [2] }));
    assert_eq!(r["pages"], 4);
    assert_eq!(page_text(&mut a, doc), ["Page 1", "Page 2", "Page 2", "Page 3"]);
    // Crop page 1 by margins, page 2 to a rect drawn from the top-left.
    let r = ok(&mut a, "page_set_box", json!({ "doc": doc, "pages": [1], "margins": [10, 20, 30, 40] }));
    assert_eq!(r["page_sizes"][0], json!([160.0, 240.0]));
    let r = ok(&mut a, "page_set_box", json!({ "doc": doc, "pages": [2], "rect": [0, 0, 100, 150] }));
    assert_eq!(r["page_sizes"][1], json!([100.0, 150.0]));
    assert_eq!(page_text(&mut a, doc)[1], "Page 2", "the text at y = 150 is still inside");
    // Reset and errors.
    let r = ok(&mut a, "page_set_box", json!({ "doc": doc, "pages": [1] }));
    assert_eq!(r["page_sizes"][0], json!([200.0, 300.0]));
    assert!(matches!(a.call("page_set_box", &json!({ "doc": doc, "margins": [150, 0, 150, 0] })), Err(ToolError::Failed(_))));
    assert!(matches!(a.call("page_set_box", &json!({ "doc": doc, "rect": [0, 0, 10, 10] })), Err(ToolError::InvalidArgs(_))));
}

#[test]
fn headers_watermarks_and_backgrounds_through_tools() {
    let dir = workdir("marks");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "doc_header_footer", json!({ "doc": doc, "footer_center": "<<Page 1 of n>>", "header_right": "ACME" }));
    let texts = page_text(&mut a, doc);
    assert!(texts[2].contains("Page 3 of 3") && texts[0].contains("ACME"), "{texts:?}");
    ok(&mut a, "doc_watermark", json!({ "doc": doc, "pages": [1], "text": "DRAFT", "opacity": 0.2 }));
    assert!(page_text(&mut a, doc)[0].contains("DRAFT"));
    ok(&mut a, "doc_background", json!({ "doc": doc, "color": "yellow" }));
    ok(&mut a, "doc_remove_marks", json!({ "doc": doc, "kind": "watermark" }));
    assert!(!page_text(&mut a, doc)[0].contains("DRAFT"));
    assert!(matches!(a.call("doc_remove_marks", &json!({ "doc": doc, "kind": "watermark" })), Err(ToolError::Failed(_))));
    assert!(matches!(a.call("doc_header_footer", &json!({ "doc": doc })), Err(ToolError::Failed(_))), "no text");
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "marked.pdf" }));
}

#[test]
fn watermarks_move_from_the_page_centre_through_tools() {
    let dir = workdir("wm-offset");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    // The top-left corner of "DRAFT" on page 1, in points (y grows downwards).
    // Saved and reopened, so the search reads the file as another program would.
    let mut saved = 0;
    let mut corner = |a: &mut Automation| -> (f64, f64) {
        saved += 1;
        let path = format!("moved{saved}.pdf");
        ok(a, "doc_save", json!({ "doc": doc, "path": path }));
        let copy = ok(a, "doc_open", json!({ "path": path }))["doc"].as_u64().unwrap();
        let found = ok(a, "text_find", json!({ "doc": copy, "query": "DRAFT" }));
        let rect =
            found["matches"][0]["rects"][0].as_array().unwrap_or_else(|| panic!("DRAFT is on the page: {found}; text: {:?}", page_text(a, copy)));
        (rect[0].as_f64().unwrap(), rect[1].as_f64().unwrap())
    };
    // Both sit above the fixture's "Page 1" (at the page centre), or their letters would mix.
    ok(&mut a, "doc_watermark", json!({ "doc": doc, "pages": [1], "text": "DRAFT", "rotation": 0, "font_size": 20, "offset_y": 60 }));
    let centred = corner(&mut a);
    ok(
        &mut a,
        "doc_watermark",
        json!({ "doc": doc, "pages": [1], "text": "DRAFT", "rotation": 0, "font_size": 20, "offset_x": 30, "offset_y": 100, "replace": true }),
    );
    let moved = corner(&mut a);
    assert!((moved.0 - centred.0 - 30.0).abs() < 1.0, "right: {centred:?} → {moved:?}");
    assert!((centred.1 - moved.1 - 40.0).abs() < 1.0, "up is towards the top: {centred:?} → {moved:?}");
    // Huge offsets are clamped, and a value that isn't a number is refused.
    ok(&mut a, "doc_watermark", json!({ "doc": doc, "pages": [1], "text": "DRAFT", "offset_y": 1e300, "replace": true }));
    assert!(matches!(a.call("doc_watermark", &json!({ "doc": doc, "text": "X", "offset_x": "left" })), Err(ToolError::InvalidArgs(_))));
}

#[test]
fn exporting_images_and_text_through_tools() {
    let dir = workdir("export");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let r = ok(&mut a, "doc_export_images", json!({ "doc": doc, "folder": "out", "dpi": 72, "pages": [1, 3] }));
    assert_eq!(r["count"], 2);
    assert!(dir.join("out/a_page_3.png").exists());
    ok(&mut a, "doc_export_images", json!({ "doc": doc, "folder": "out", "dpi": 72, "pages": [2], "format": "jpeg", "quality": 70 }));
    assert!(std::fs::read(dir.join("out/a_page_2.jpg")).unwrap().starts_with(&[0xFF, 0xD8]));
    ok(&mut a, "doc_export_images", json!({ "doc": doc, "folder": "out", "dpi": 72, "pages": [2], "format": "tiff" }));
    let tif = std::fs::read(dir.join("out/a_page_2.tif")).unwrap();
    assert!(tif.starts_with(b"II*\0") || tif.starts_with(b"MM\0*"));
    assert!(matches!(a.call("doc_export_images", &json!({ "doc": doc, "folder": "out", "format": "webp" })), Err(ToolError::InvalidArgs(_))));
    ok(&mut a, "doc_export_text", json!({ "doc": doc, "path": "a.txt" }));
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "Page 1\n\u{c}Page 2\n\u{c}Page 3\n");
    assert!(a.call("doc_export_text", &json!({ "doc": doc, "path": "/etc/x.txt" })).is_err(), "confined to the root");
}

#[test]
fn accessibility_check_report_and_fixes_through_tools() {
    let dir = workdir("a11y");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let r = ok(&mut a, "accessibility_check", json!({ "doc": doc }));
    assert_eq!(r["results"].as_array().unwrap().len(), 32);
    let status =
        |r: &Value, id: &str| r["results"].as_array().unwrap().iter().find(|x| x["rule"] == id).unwrap()["status"].as_str().unwrap().to_owned();
    for id in ["tagged-pdf", "primary-language", "title", "tagged-content"] {
        assert_eq!(status(&r, id), "failed", "{id}");
    }
    assert_eq!((status(&r, "color-contrast"), status(&r, "scripts")), ("skipped".into(), "manual".into()));
    assert_eq!(status(&ok(&mut a, "accessibility_check", json!({ "doc": doc, "all": true })), "color-contrast"), "manual");
    let docs = ok(&mut a, "accessibility_check", json!({ "doc": doc, "categories": ["document"] }));
    assert_eq!(docs["skipped"], 25, "24 other rules and colour contrast");
    // Fixes.
    assert!(matches!(a.call("accessibility_fix", &json!({ "doc": doc, "rule": "primary-language" })), Err(ToolError::Failed(_))), "needs a language");
    assert_eq!(ok(&mut a, "accessibility_fix", json!({ "doc": doc, "rule": "primary-language", "value": "en-GB" }))["status"], "passed");
    assert_eq!(ok(&mut a, "accessibility_fix", json!({ "doc": doc, "rule": "title" }))["status"], "passed");
    assert_eq!(ok(&mut a, "doc_info", json!({ "doc": doc }))["title"], "a");
    assert_eq!(ok(&mut a, "edit_undo", json!({ "doc": doc }))["undone"], "Set document title");
    assert!(matches!(a.call("accessibility_fix", &json!({ "doc": doc, "rule": "tagged-pdf" })), Err(ToolError::Failed(_))));
    assert!(matches!(a.call("accessibility_check", &json!({ "doc": doc, "rules": ["nope"] })), Err(ToolError::InvalidArgs(_))));
    // The report.
    let r = ok(&mut a, "accessibility_report", json!({ "doc": doc, "path": "report.html" }));
    assert_eq!(r["failed"].as_u64(), Some(3), "tagging, tagged content and (undone) title");
    let html = std::fs::read_to_string(dir.join("report.html")).unwrap();
    assert!(html.contains("Accessibility Report") && html.contains("a.pdf"));
    ok(&mut a, "accessibility_report", json!({ "doc": doc, "path": "manual-report.html", "rules": ["scripts"] }));
    let manual_html = std::fs::read_to_string(dir.join("manual-report.html")).unwrap();
    assert!(manual_html.contains("<p>Needs manual check: 1</p>"));
    assert!(!manual_html.contains("no problems"));
    // Figures: list, describe, mark decorative.
    std::fs::write(
        dir.join("figures.pdf"),
        b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /MarkInfo << /Marked true >> /StructTreeRoot 5 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /StructParents 0 >> endobj
4 0 obj << /Length 74 >> stream
/Figure << /MCID 0 >> BDC 10 150 40 20 re f EMC /Figure << /MCID 1 >> BDC 100 100 50 30 re f EMC
endstream endobj
5 0 obj << /Type /StructTreeRoot /K 6 0 R /ParentTree 9 0 R >> endobj
6 0 obj << /S /Document /P 5 0 R /K [7 0 R 8 0 R] >> endobj
7 0 obj << /S /Figure /P 6 0 R /Pg 3 0 R /K 0 >> endobj
8 0 obj << /S /Figure /P 6 0 R /Pg 3 0 R /K 1 >> endobj
9 0 obj << /Nums [0 [7 0 R 8 0 R]] >> endobj
trailer << /Root 1 0 R >>
%%EOF"
            .as_slice(),
    )
    .unwrap();
    let fd = ok(&mut a, "doc_open", json!({ "path": "figures.pdf" }))["doc"].as_u64().unwrap();
    let tags = ok(&mut a, "accessibility_tags", json!({ "doc": fd }));
    assert_eq!(tags["count"], 3);
    assert_eq!(tags["tags"][1]["object"], 7);
    assert_eq!(tags["tags"][1]["type"], "Figure");
    assert_eq!(tags["tags"][1]["parent"], json!({ "object": 6, "generation": 0 }));
    assert_eq!(tags["tags"][1]["depth"], 1);
    let changed = ok(
        &mut a,
        "accessibility_set_tag_property",
        json!({
            "doc": fd, "object": 7, "generation": 0, "property": "title", "value": "First shape"
        }),
    );
    assert_eq!(changed["tags"][1]["title"], "First shape");
    assert_eq!(ok(&mut a, "edit_undo", json!({ "doc": fd }))["undone"], "Change tag properties");
    assert_eq!(ok(&mut a, "accessibility_tags", json!({ "doc": fd }))["tags"][1]["title"], Value::Null);
    ok(&mut a, "accessibility_set_tag_property", json!({ "doc": fd, "object": 8, "generation": 0, "property": "title", "value": "Second shape" }));
    let moved = ok(
        &mut a,
        "accessibility_move_tag",
        json!({ "doc": fd, "object": 8, "generation": 0, "anchor": 7, "anchor_generation": 0, "placement": "before" }),
    );
    assert_eq!((moved["tags"][1]["object"].as_u64(), moved["tags"][2]["object"].as_u64()), (Some(8), Some(7)));
    ok(&mut a, "doc_save", json!({ "doc": fd, "path": "reordered.pdf" }));
    let saved = pdfcraft_cos::Document::open(std::sync::Arc::new(std::fs::read(dir.join("reordered.pdf")).unwrap())).unwrap();
    assert_eq!(pdfcraft_engine::a11y::tags(&saved).into_iter().map(|tag| tag.title).collect::<Vec<_>>(), [None, Some("Second shape".into()), None]);
    assert_eq!(ok(&mut a, "edit_undo", json!({ "doc": fd }))["undone"], "Move object");
    assert_eq!(ok(&mut a, "accessibility_tags", json!({ "doc": fd }))["tags"][1]["object"], 7);
    assert!(matches!(
        a.call(
            "accessibility_move_tag",
            &json!({ "doc": fd, "object": 8, "generation": 0, "anchor": 7, "anchor_generation": 0, "placement": "sideways" })
        ),
        Err(ToolError::InvalidArgs(_))
    ));
    assert!(matches!(
        a.call(
            "accessibility_set_tag_property",
            &json!({
                "doc": fd, "object": 7, "generation": 1, "property": "title", "value": "Wrong generation"
            })
        ),
        Err(ToolError::Failed(_))
    ));
    let findings = ok(&mut a, "accessibility_check", json!({ "doc": fd, "rules": ["figures-alt-text"] }));
    let figures_rule = findings["results"].as_array().unwrap().iter().find(|x| x["rule"] == "figures-alt-text").unwrap();
    assert_eq!(figures_rule["findings"][0]["object"], 7);
    assert_eq!(figures_rule["findings"][0]["object_generation"], 0);
    assert_eq!(figures_rule["findings"][0]["bbox_user_space"], json!([10.0, 150.0, 50.0, 170.0]));
    assert_eq!(figures_rule["findings"][1]["object"], 8);
    assert_eq!(figures_rule["findings"][1]["object_generation"], 0);
    assert_eq!(figures_rule["findings"][1]["bbox_user_space"], json!([100.0, 100.0, 150.0, 130.0]));
    let figs = ok(&mut a, "accessibility_figures", json!({ "doc": fd }));
    assert_eq!(figs["count"], 2);
    assert_eq!(figs["figures"][0]["rect"], json!([10.0, 30.0, 50.0, 50.0]), "top-left-origin points");
    let first = figs["figures"][0]["figure"].as_u64().unwrap();
    let second = figs["figures"][1]["figure"].as_u64().unwrap();
    let r = ok(&mut a, "accessibility_set_alt", json!({ "doc": fd, "figure": first, "alt": "A small bar" }));
    assert_eq!(r["figures"][0]["alt"], "A small bar");
    let r = ok(&mut a, "accessibility_set_alt", json!({ "doc": fd, "figure": second, "decorative": true }));
    assert_eq!(r["count"], 1);
    let check = ok(&mut a, "accessibility_check", json!({ "doc": fd, "rules": ["figures-alt-text", "tagged-content"] }));
    assert_eq!(check["failed"], 0, "{check}");
    assert!(matches!(a.call("accessibility_set_alt", &json!({ "doc": fd, "figure": 6, "alt": "x" })), Err(ToolError::Failed(_))));
}

#[test]
fn marking_a_leaf_tag_as_an_artifact_through_automation_is_undoable_and_saved() {
    let dir = workdir("a11y-artifact");
    std::fs::write(
        dir.join("tagged.pdf"),
        b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /MarkInfo << /Marked true >> /StructTreeRoot 5 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /StructParents 0 >> endobj
4 0 obj << /Length 48 >> stream
/Figure << /MCID 0 >> BDC 10 150 40 20 re f EMC
endstream endobj
5 0 obj << /Type /StructTreeRoot /K 6 0 R /ParentTree 8 0 R >> endobj
6 0 obj << /S /Document /P 5 0 R /K [7 0 R] >> endobj
7 0 obj << /S /Figure /P 6 0 R /Pg 3 0 R /K 0 >> endobj
8 0 obj << /Nums [0 [7 0 R]] >> endobj
trailer << /Root 1 0 R >>
%%EOF",
    )
    .unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "tagged.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "accessibility_tags", json!({ "doc": doc }))["count"], 2);
    let tagged = ok(&mut a, "accessibility_mark_tag_artifact", json!({ "doc": doc, "object": 7, "generation": 0 }));
    assert_eq!(tagged["count"], 1);
    assert_eq!(ok(&mut a, "accessibility_check", json!({ "doc": doc, "rules": ["tagged-content"] }))["failed"], 0);
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "artifact.pdf" }));
    let saved = pdfcraft_cos::Document::open(std::sync::Arc::new(std::fs::read(dir.join("artifact.pdf")).unwrap())).unwrap();
    assert_eq!(pdfcraft_engine::a11y::tags(&saved).len(), 1);
    assert_eq!(ok(&mut a, "edit_undo", json!({ "doc": doc }))["undone"], "Mark content as artifact");
    assert_eq!(ok(&mut a, "accessibility_tags", json!({ "doc": doc }))["count"], 2);
    assert!(matches!(a.call("accessibility_mark_tag_artifact", &json!({ "doc": doc, "object": 6, "generation": 0 })), Err(ToolError::Failed(_))));
}

#[test]
fn accessibility_review_record_compares_two_versions_without_changing_them() {
    let dir = workdir("a11y-review-record");
    let source = std::fs::read(dir.join("a.pdf")).unwrap();
    let mut a = auto(&dir);
    let before = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let after = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "accessibility_fix", json!({ "doc": after, "rule": "primary-language", "value": "en-GB" }));

    let output = ok(&mut a, "accessibility_review_record", json!({ "before": before, "after": after, "path": "review.json" }));
    let file: Value = serde_json::from_slice(&std::fs::read(dir.join("review.json")).unwrap()).unwrap();
    assert_eq!(output["record"], file);
    assert_eq!(file["schema"], 1);
    assert_eq!(file["before"]["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(file["after"]["sha256"].as_str().unwrap().len(), 64);
    assert_ne!(file["before"]["sha256"], file["after"]["sha256"]);
    assert_eq!(file["changed"], json!(["primary-language"]));
    let language = file["rules"].as_array().unwrap().iter().find(|r| r["rule"] == "primary-language").unwrap();
    assert_eq!(language["before"]["status"], "failed");
    assert_eq!(language["after"]["status"], "passed");
    assert!(file["review_required"].as_array().unwrap().iter().any(|r| r == "tagged-pdf"));
    let html_output = ok(&mut a, "accessibility_review_html", json!({ "before": before, "after": after, "path": "review.html" }));
    assert_eq!(html_output["record"], file);
    let html = std::fs::read_to_string(dir.join("review.html")).unwrap();
    assert!(html.contains("<html lang=\"en\">") && html.contains("Accessibility checker outcomes"));
    assert!(html.contains("Primary language") && html.contains("<td>Changed</td>"));
    assert!(html.contains("Still requiring review") && html.contains("Tagged PDF"));
    assert!(html.contains(file["before"]["sha256"].as_str().unwrap()));
    assert!(html.contains("validate the final PDF independently"));
    assert_eq!(std::fs::read(dir.join("a.pdf")).unwrap(), source);
    assert!(matches!(
        a.call("accessibility_review_record", &json!({ "before": before, "after": before, "path": "same.json" })),
        Err(ToolError::InvalidArgs(_))
    ));
    assert!(!dir.join("same.json").exists());
    assert!(matches!(
        a.call("accessibility_review_html", &json!({ "before": before, "after": before, "path": "same.html" })),
        Err(ToolError::InvalidArgs(_))
    ));
    assert!(!dir.join("same.html").exists());
    assert!(a.call("accessibility_review_record", &json!({ "before": before, "after": after, "path": "../escape.json" })).is_err());
    assert!(a.call("accessibility_review_html", &json!({ "before": before, "after": after, "path": "../escape.html" })).is_err());
    assert!(!dir.parent().unwrap().join("escape.html").exists());
}

#[test]
fn accessibility_review_sidecar_tracks_human_decisions_for_exact_pdf_revision() {
    let dir = workdir("a11y-review-sidecar");
    let source = std::fs::read(dir.join("a.pdf")).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let decisions = json!([
        { "rule": "logical-reading-order", "decision": "needs_work", "page": 1, "note": "Check heading order" },
        { "rule": "figures-alt-text", "decision": "accepted", "page": 2 }
    ]);
    let saved = ok(&mut a, "accessibility_review_save", json!({ "doc": doc, "path": "review.json", "decisions": decisions }));
    assert_eq!(saved["review"]["schema"], 1);
    assert_eq!(saved["review"]["sha256"].as_str().unwrap().len(), 64);
    let original_sidecar = std::fs::read(dir.join("review.json")).unwrap();
    assert_eq!(ok(&mut a, "accessibility_review_load", json!({ "doc": doc, "path": "review.json" }))["review"], saved["review"]);
    let mut reopened = auto(&dir);
    let reopened_doc = ok(&mut reopened, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut reopened, "accessibility_review_load", json!({ "doc": reopened_doc, "path": "review.json" }))["review"], saved["review"]);
    assert!(matches!(
        reopened.call("accessibility_review_save", &json!({ "doc": reopened_doc, "path": "a.pdf", "decisions": [], "replace": true })),
        Err(ToolError::Failed(_))
    ));
    assert_eq!(std::fs::read(dir.join("a.pdf")).unwrap(), source);

    assert!(matches!(
        a.call(
            "accessibility_review_save",
            &json!({ "doc": doc, "path": "review.json", "decisions": [
        { "rule": "title", "decision": "accepted" }, { "rule": "title", "decision": "skipped" }
    ] })
        ),
        Err(ToolError::InvalidArgs(_))
    ));
    assert_eq!(std::fs::read(dir.join("review.json")).unwrap(), original_sidecar);
    ok(&mut a, "accessibility_fix", json!({ "doc": doc, "rule": "primary-language", "value": "en-GB" }));
    assert!(matches!(a.call("accessibility_review_load", &json!({ "doc": doc, "path": "review.json" })), Err(ToolError::Failed(_))));
    assert!(matches!(a.call("accessibility_review_save", &json!({ "doc": doc, "path": "review.json", "decisions": [] })), Err(ToolError::Failed(_))));
    assert_eq!(std::fs::read(dir.join("review.json")).unwrap(), original_sidecar);
    let replacement = ok(&mut a, "accessibility_review_save", json!({ "doc": doc, "path": "review.json", "decisions": decisions, "replace": true }));
    assert_ne!(replacement["review"]["sha256"], saved["review"]["sha256"]);
    assert_eq!(ok(&mut a, "accessibility_review_load", json!({ "doc": doc, "path": "review.json" }))["review"], replacement["review"]);
    assert_eq!(std::fs::read(dir.join("a.pdf")).unwrap(), source);
    assert!(a.call("accessibility_review_save", &json!({ "doc": doc, "path": "../escape.json", "decisions": [] })).is_err());
    assert!(!dir.parent().unwrap().join("escape.json").exists());

    // Malformed or oversized files are not sidecars: loading fails cleanly and
    // saving leaves them alone, without reading more than the size cap.
    let oversized = [b"{\"schema\": 1, \"pad\": \"".as_slice(), &vec![b'x'; (1 << 20) + 16], b"\"}"].concat();
    for (name, bytes) in [("malformed.json", b"{ not json".to_vec()), ("binary.json", vec![0xff, 0x00, 0xfe]), ("oversized.json", oversized)] {
        std::fs::write(dir.join(name), &bytes).unwrap();
        assert!(matches!(a.call("accessibility_review_load", &json!({ "doc": doc, "path": name })), Err(ToolError::Failed(_))), "{name}");
        assert!(
            matches!(
                a.call("accessibility_review_save", &json!({ "doc": doc, "path": name, "decisions": [], "replace": true })),
                Err(ToolError::Failed(_))
            ),
            "{name}"
        );
        assert_eq!(std::fs::read(dir.join(name)).unwrap(), bytes, "{name}");
    }
}

#[test]
fn editing_existing_text_through_tools() {
    let dir = workdir("edit-text");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let lines = ok(&mut a, "text_lines", json!({ "doc": doc, "page": 2 }));
    assert_eq!((lines["count"].as_u64(), lines["lines"][0]["text"].as_str()), (Some(1), Some("Page 2")));
    let r = ok(&mut a, "text_edit", json!({ "doc": doc, "page": 2, "line": 1, "text": "Section two" }));
    assert_eq!(r["line"]["text"], "Section two");
    assert_eq!(page_text(&mut a, doc)[1], "Section two");
    assert!(matches!(a.call("text_edit", &json!({ "doc": doc, "page": 2, "line": 9, "text": "x" })), Err(ToolError::InvalidArgs(_))));
    assert_eq!(ok(&mut a, "edit_undo", json!({ "doc": doc }))["undone"], "Edit text");
    let paras = ok(&mut a, "text_paragraphs", json!({ "doc": doc, "page": 3 }));
    assert_eq!(paras["paragraphs"][0]["text"], "Page 3");
    let r = ok(&mut a, "text_edit", json!({ "doc": doc, "page": 3, "paragraph": 1, "text": "Part three" }));
    assert_eq!(r["paragraph"]["text"], "Part three");
    assert!(matches!(a.call("text_edit", &json!({ "doc": doc, "page": 3, "paragraph": 4, "text": "x" })), Err(ToolError::InvalidArgs(_))));
    // Formatting only: font, size, colour, alignment.
    ok(
        &mut a,
        "text_edit",
        json!({ "doc": doc, "page": 3, "paragraph": 1, "font": "times", "bold": true, "size": 20, "color": "#cc0000", "align": "center" }),
    );
    let p = &ok(&mut a, "text_paragraphs", json!({ "doc": doc, "page": 3 }))["paragraphs"][0];
    assert_eq!((p["text"].as_str(), p["font"].as_str(), p["size"].as_f64()), (Some("Part three"), Some("Times-Bold"), Some(20.0)));
    // Moved 15 pt right and 10 pt down (dy is up; the listing's rect is measured from the top).
    let rect = |p: &serde_json::Value| -> Vec<f64> { p["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect() };
    let before = rect(p);
    ok(&mut a, "text_edit", json!({ "doc": doc, "page": 3, "paragraph": 1, "dx": 15, "dy": -10 }));
    let p = ok(&mut a, "text_paragraphs", json!({ "doc": doc, "page": 3 }))["paragraphs"][0].clone();
    let after = rect(&p);
    assert!((after[0] - before[0] - 15.0).abs() < 0.5 && (after[1] - before[1] - 10.0).abs() < 0.5, "{before:?} → {after:?}");
    assert_eq!(p["text"], "Part three");
    // A narrow width rewraps "Part three" onto two lines.
    ok(&mut a, "text_edit", json!({ "doc": doc, "page": 3, "paragraph": 1, "width": 50 }));
    let p = &ok(&mut a, "text_paragraphs", json!({ "doc": doc, "page": 3 }))["paragraphs"][0];
    assert_eq!(p["lines"].as_array().map(Vec::len), Some(2), "{p}");
}

/// One page whose content is one stream in three pieces (#155): a `TJ` array ends the middle
/// piece and its operator starts the last one.
fn split_streams_pdf() -> Vec<u8> {
    let piece = |s: &str| format!("<< /Length {} >>\nstream\n{s}\nendstream", s.len());
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents [4 0 R 5 0 R 6 0 R] /Resources << /Font << /F1 7 0 R >> >> >>".into(),
        piece("/P << /MCID 0"),
        piece(">> BDC BT /F1 12 Tf 72 700 Td (Target) Tj 0 -20 Td [(After) -20 (wards)]"),
        piece("TJ ET EMC"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

#[test]
fn a_line_split_across_content_streams_is_listed_and_edited() {
    let dir = workdir("split-streams");
    std::fs::write(dir.join("split.pdf"), split_streams_pdf()).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "split.pdf" }))["doc"].as_u64().unwrap();
    let lines = ok(&mut a, "text_lines", json!({ "doc": doc, "page": 1 }));
    let texts: Vec<&str> = lines["lines"].as_array().unwrap().iter().filter_map(|l| l["text"].as_str()).collect();
    assert_eq!(texts, ["Target", "Afterwards"], "{lines}");
    let r = ok(&mut a, "text_edit", json!({ "doc": doc, "page": 1, "line": 2, "text": "Later" }));
    assert_eq!(r["line"]["text"], "Later");
    assert_eq!(page_text(&mut a, doc)[0].split_whitespace().collect::<Vec<_>>(), ["Target", "Later"]);
}

#[test]
fn paragraph_bold_without_font_keeps_the_source_family() {
    let dir = workdir("paragraph-bold");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "text_edit", json!({ "doc": doc, "page": 2, "paragraph": 1, "font": "times" }));
    let bold = ok(&mut a, "text_edit", json!({ "doc": doc, "page": 2, "paragraph": 1, "bold": true }));
    assert_eq!(bold["paragraph"]["font"], "Times-Bold");
    let regular = ok(&mut a, "text_edit", json!({ "doc": doc, "page": 2, "paragraph": 1, "bold": false }));
    assert_eq!(regular["paragraph"]["font"], "Times-Roman");
    assert_eq!(regular["paragraph"]["text"], "Page 2");
    assert!(matches!(a.call("text_edit", &json!({ "doc": doc, "page": 2, "paragraph": 1, "bold": "yes" })), Err(ToolError::InvalidArgs(_))));
}

#[test]
fn editing_page_images_through_tools() {
    let dir = workdir("page-images");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "doc_export_images", json!({ "doc": doc, "folder": "src", "dpi": 18, "pages": [1, 2] }));
    let made = ok(&mut a, "doc_create", json!({ "from": "images", "paths": ["src/a_page_1.png"] }))["doc"].as_u64().unwrap();
    let list = ok(&mut a, "page_images", json!({ "doc": made, "page": 1 }));
    assert_eq!(list["count"], 1);
    let r = ok(&mut a, "image_edit", json!({ "doc": made, "page": 1, "image": 1, "action": "move", "rect": [5, 5, 25, 35] }));
    assert_eq!(r["undo"], "Move image");
    let moved = ok(&mut a, "page_images", json!({ "doc": made, "page": 1 }))["images"][0]["rect"].clone();
    assert_eq!(moved, json!([5.0, 5.0, 25.0, 35.0]));
    // Crop keeps part of it in place: the placement stays, visible_rect says what shows.
    let r = ok(&mut a, "image_edit", json!({ "doc": made, "page": 1, "image": 1, "action": "crop", "rect": [10, 10, 25, 30] }));
    assert_eq!(r["undo"], "Crop image");
    let cropped = ok(&mut a, "page_images", json!({ "doc": made, "page": 1 }))["images"][0].clone();
    assert_eq!(cropped["rect"], json!([5.0, 5.0, 25.0, 35.0]));
    assert_eq!(cropped["visible_rect"], json!([10.0, 10.0, 25.0, 30.0]));
    ok(&mut a, "image_edit", json!({ "doc": made, "page": 1, "image": 1, "action": "rotate" }));
    ok(&mut a, "image_edit", json!({ "doc": made, "page": 1, "image": 1, "action": "flip_horizontal" }));
    let saved = ok(&mut a, "image_save", json!({ "doc": made, "page": 1, "image": 1, "path": "out/picture" }));
    assert_eq!(saved["format"], "png");
    assert!(std::fs::read(dir.join("out/picture.png")).unwrap().starts_with(b"\x89PNG"));
    ok(&mut a, "image_edit", json!({ "doc": made, "page": 1, "image": 1, "action": "replace", "path": "src/a_page_2.png" }));
    assert_eq!(ok(&mut a, "page_images", json!({ "doc": made, "page": 1 }))["count"], 1);
    ok(&mut a, "image_edit", json!({ "doc": made, "page": 1, "image": 1, "action": "delete" }));
    assert_eq!(ok(&mut a, "page_images", json!({ "doc": made, "page": 1 }))["count"], 0);
    assert!(matches!(a.call("image_edit", &json!({ "doc": made, "page": 1, "image": 1, "action": "delete" })), Err(ToolError::InvalidArgs(_))));
}

#[test]
fn enhance_camera_image_through_tools() {
    let dir = workdir("enhance-camera");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "doc_export_images", json!({ "doc": doc, "folder": "src", "dpi": 72, "pages": [1] }));
    let photo = ok(&mut a, "doc_create", json!({ "from": "images", "paths": ["src/a_page_1.png"] }))["doc"].as_u64().unwrap();
    // A 200 × 300 pt page at one pixel per point; the corners, in any order, keep a 160 × 240 part.
    let r = ok(&mut a, "page_enhance_camera", json!({ "doc": photo, "page": 1, "corners": [180, 270, 20, 30, 20, 270, 180, 30] }));
    assert_eq!(r["undo"], "Enhance camera image");
    assert_eq!(r["pages"], 1);
    let images = ok(&mut a, "page_images", json!({ "doc": photo, "page": 1 }));
    assert_eq!(images["count"], 1);
    assert_eq!(images["images"][0]["pixels"], json!([160, 240]));
    assert_eq!(images["images"][0]["rect"], json!([0.0, 0.0, 160.0, 240.0]));
    for bad in [json!([0, 0, 10, 10, 20, 20, 30, 30]), json!([0, 0, 10]), json!("corners")] {
        let e = a.call("page_enhance_camera", &json!({ "doc": photo, "page": 1, "corners": bad }));
        assert!(matches!(e, Err(ToolError::InvalidArgs(_) | ToolError::Failed(_))), "{bad}: {e:?}");
    }
    // Without corners on a page with no photo, the edges can't be found: an error that says so.
    let e = a.call("page_enhance_camera", &json!({ "doc": photo, "page": 1 }));
    assert!(matches!(&e, Err(ToolError::Failed(m)) if m.contains("corners")), "{e:?}");
    assert!(matches!(
        a.call("page_enhance_camera", &json!({ "doc": photo, "page": 9, "corners": [0, 0, 9, 0, 9, 9, 0, 9] })),
        Err(ToolError::InvalidArgs(_))
    ));
}

#[test]
fn enhance_camera_image_finds_the_edges_by_itself() {
    let dir = workdir("enhance-camera-auto");
    // A 600 × 450 photo: a white sheet, tilted, on a dark table.
    let sheet = [[90.0, 60.0], [500.0, 30.0], [525.0, 405.0], [60.0, 375.0]];
    let inside = |x: f64, y: f64| {
        (0..4).all(|i| {
            let (a, b): ([f64; 2], [f64; 2]) = (sheet[i], sheet[(i + 1) % 4]);
            (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]) >= 0.0
        })
    };
    let img = image::RgbImage::from_fn(600, 450, |x, y| {
        if inside(x as f64 + 0.5, y as f64 + 0.5) { image::Rgb([240, 238, 232]) } else { image::Rgb([70, 60, 50]) }
    });
    img.save(dir.join("photo.jpg")).unwrap();
    let mut a = auto(&dir);
    let photo = ok(&mut a, "doc_create", json!({ "from": "images", "paths": ["photo.jpg"] }))["doc"].as_u64().unwrap();
    let r = ok(&mut a, "page_enhance_camera", json!({ "doc": photo, "page": 1 }));
    assert_eq!(r["detected"], true);
    assert_eq!(r["undo"], "Enhance camera image");
    // A clean-up level outside 0–100 is refused before anything changes.
    let e = a.call("page_enhance_camera", &json!({ "doc": photo, "page": 1, "corners": [0, 0, 100, 0, 100, 100, 0, 100], "level": 101 }));
    assert!(matches!(&e, Err(ToolError::InvalidArgs(m)) if m.contains("level")), "{e:?}");
    ok(&mut a, "page_enhance_camera", json!({ "doc": photo, "page": 1, "corners": [0, 0, 100, 0, 100, 100, 0, 100], "level": 80 }));
    // One point per pixel: the corners it used are the sheet's.
    let used: Vec<f64> = r["corners"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    for (k, p) in sheet.iter().enumerate() {
        let (x, y) = (used[2 * k], used[2 * k + 1]);
        assert!((x - p[0]).hypot(y - p[1]) < 4.0, "{used:?} vs {sheet:?}");
    }
}

#[test]
fn editing_form_artwork_through_tools() {
    use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_incremental};
    let dir = workdir("form-artwork");
    let mut cos = Document::open(std::sync::Arc::new(fixture(1))).unwrap();
    let page = pdfcraft_model::pages(&cos)[0].clone();
    let mut form = Dict::new();
    form.set(b"Subtype".to_vec(), Object::name("Form"));
    form.set(b"BBox".to_vec(), Object::Array([0, 0, 80, 40].map(Object::Int).to_vec()));
    let artwork = cos.add(Object::Stream(Stream::flate(form, b"0.2 0.5 0.9 rg 0 0 80 40 re f")));
    let contents = cos.add(Object::Stream(Stream::flate(Dict::new(), b"q 1 0 0 1 20 240 cm /Figure Do Q")));
    cos.update_dict(page.obj, |d| {
        d.set(b"Contents".to_vec(), Object::Ref(contents));
        let mut xo = Dict::new();
        xo.set(b"Figure".to_vec(), Object::Ref(artwork));
        let mut resources = Dict::new();
        resources.set(b"XObject".to_vec(), Object::Dict(xo));
        d.set(b"Resources".to_vec(), Object::Dict(resources));
    })
    .unwrap();
    std::fs::write(dir.join("figure.pdf"), write_incremental(&cos, &SaveOptions::default()).unwrap()).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({"path": "figure.pdf"}))["doc"].as_u64().unwrap();
    let before = ok(&mut a, "page_images", json!({"doc": doc, "page": 1}))["images"][0].clone();
    assert_eq!(before["kind"], "form");
    assert_eq!(before["pixels"], json!([0, 0]));
    assert_eq!(before["rect"], json!([20.0, 20.0, 100.0, 60.0]));
    let rendered_before = a.call("page_render", &json!({"doc": doc, "page": 1, "dpi": 72})).unwrap();
    assert!(a.call("image_save", &json!({"doc": doc, "page": 1, "image": 1, "path": "out/figure"})).is_err());
    ok(&mut a, "image_edit", json!({"doc": doc, "page": 1, "image": 1, "action": "move", "rect": [40, 50, 120, 90]}));
    assert_eq!(ok(&mut a, "page_images", json!({"doc": doc, "page": 1}))["images"][0]["rect"], json!([40.0, 50.0, 120.0, 90.0]));
    let rendered_moved = a.call("page_render", &json!({"doc": doc, "page": 1, "dpi": 72})).unwrap();
    assert_ne!(rendered_before, rendered_moved, "the rendered artwork actually moves");
    ok(&mut a, "doc_save", json!({"doc": doc, "path": "out/moved.pdf"}));
    let re = ok(&mut a, "doc_open", json!({"path": "out/moved.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "page_images", json!({"doc": re, "page": 1}))["images"][0]["rect"], json!([40.0, 50.0, 120.0, 90.0]));
    ok(&mut a, "image_edit", json!({"doc": re, "page": 1, "image": 1, "action": "delete"}));
    assert_eq!(ok(&mut a, "page_images", json!({"doc": re, "page": 1}))["count"], 0);
    ok(&mut a, "edit_undo", json!({"doc": re}));
    assert_eq!(ok(&mut a, "page_images", json!({"doc": re, "page": 1}))["count"], 1);
    ok(&mut a, "edit_undo", json!({"doc": doc}));
    assert_eq!(ok(&mut a, "page_images", json!({"doc": doc, "page": 1}))["images"][0], before);
    assert_eq!(a.call("page_render", &json!({"doc": doc, "page": 1, "dpi": 72})).unwrap(), rendered_before);
}

#[test]
fn auditing_space_through_tools() {
    let dir = workdir("audit");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let r = ok(&mut a, "doc_audit_space", json!({ "doc": doc }));
    let rows = r["categories"].as_array().unwrap();
    let total: f64 = rows.iter().map(|x| x["percent"].as_f64().unwrap()).sum();
    assert!((total - 100.0).abs() < 0.1, "{r}");
    let content = rows.iter().find(|x| x["category"] == "Content Streams").unwrap();
    assert!(content["bytes"].as_u64().unwrap() > 0, "{r}");
}

#[test]
fn exporting_all_images_through_tools() {
    let dir = workdir("export-all-images");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "doc_export_all_images", json!({ "doc": doc, "folder": "none" }))["count"], 0, "text only");
    // A PDF made from two page renders (PNG, JPEG) holds two images.
    ok(&mut a, "doc_export_images", json!({ "doc": doc, "folder": "src", "dpi": 36, "pages": [1] }));
    ok(&mut a, "doc_export_images", json!({ "doc": doc, "folder": "src", "dpi": 36, "pages": [2], "format": "jpeg" }));
    let made = ok(&mut a, "doc_create", json!({ "from": "images", "paths": ["src/a_page_1.png", "src/a_page_2.jpg"] }))["doc"].as_u64().unwrap();
    let r = ok(&mut a, "doc_export_all_images", json!({ "doc": made, "folder": "imgs" }));
    assert_eq!(r["count"], 2, "{r}");
    let files: Vec<&str> = r["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert!(files[0].ends_with("_Page_1_Image_0001.png") && files[1].ends_with("_Page_2_Image_0002.jpg"), "{files:?}");
    assert_eq!(std::fs::read(files[1]).unwrap(), std::fs::read(dir.join("src/a_page_2.jpg")).unwrap(), "JPEG unchanged");
    assert!(std::fs::read(files[0]).unwrap().starts_with(b"\x89PNG"));
    assert_eq!(ok(&mut a, "doc_export_all_images", json!({ "doc": made, "folder": "imgs2", "pages": [2] }))["count"], 1);
    assert_eq!(ok(&mut a, "doc_export_all_images", json!({ "doc": made, "folder": "imgs3", "min_size": 10000 }))["count"], 0);
}

#[test]
fn fill_and_sign_through_tools() {
    let dir = workdir("fill");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "fill_sign_add", json!({ "doc": doc, "page": 1, "type": "text", "at": [20, 40], "text": "Ada Lovelace" }));
    ok(&mut a, "fill_sign_add", json!({ "doc": doc, "page": 1, "type": "check", "at": [20, 80] }));
    ok(&mut a, "fill_sign_add", json!({ "doc": doc, "page": 1, "type": "date", "at": [20, 100] }));
    ok(&mut a, "fill_sign_add", json!({ "doc": doc, "page": 1, "type": "signature", "at": [20, 150], "text": "Ada Lovelace" }));
    assert!(matches!(
        a.call("fill_sign_add", &json!({ "doc": doc, "page": 1, "type": "initials", "at": [20, 190], "text": "   " })),
        Err(ToolError::InvalidArgs(_))
    ));
    let texts = page_text(&mut a, doc);
    assert!(texts[0].contains("Ada Lovelace") && texts[0].contains("11/14/2023"), "{texts:?}");
    // Preferences ▸ Date format, and a one-off pattern.
    let f = ok(&mut a, "fill_sign_date_format", json!({}));
    assert_eq!((f["format"].as_str(), f["today"].as_str()), (Some("m/d/yyyy"), Some("11/14/2023")), "{f}");
    assert_eq!(ok(&mut a, "fill_sign_date_format", json!({ "format": "yyyy.mm.dd." }))["today"], "2023.11.14.");
    ok(&mut a, "fill_sign_add", json!({ "doc": doc, "page": 1, "type": "date", "at": [20, 220] }));
    ok(&mut a, "fill_sign_add", json!({ "doc": doc, "page": 1, "type": "date", "at": [20, 240], "format": "d \\de mmmm" }));
    let texts = page_text(&mut a, doc);
    assert!(texts[0].contains("2023.11.14.") && texts[0].contains("14 de November"), "{texts:?}");
    assert!(matches!(a.call("fill_sign_date_format", &json!({ "format": "HH:MM" })), Err(ToolError::InvalidArgs(_))));
    assert_eq!(ok(&mut a, "fill_sign_date_format", json!({}))["format"], "yyyy.mm.dd.", "a rejected format keeps the previous one");
    // Month and weekday names in a chosen language, for the preference or one date.
    let f = ok(&mut a, "fill_sign_date_format", json!({ "format": "d. mmmm yyyy", "language": "cs" }));
    assert_eq!((f["language"].as_str(), f["today"].as_str()), (Some("cs"), Some("14. listopadu 2023")), "{f}");
    assert_eq!(f["languages"].as_array().unwrap().len(), pdfcraft_engine::dates::DATE_LANGUAGES.len(), "every date language");
    ok(&mut a, "fill_sign_add", json!({ "doc": doc, "page": 1, "type": "date", "at": [20, 280], "format": "d \\de mmmm", "language": "es" }));
    assert!(page_text(&mut a, doc)[0].contains("14 de noviembre"));
    assert!(matches!(a.call("fill_sign_date_format", &json!({ "format": "yyy" })), Err(ToolError::InvalidArgs(_))));
    // A date the PDF's text font can't hold is refused, not saved as "?".
    match a.call("fill_sign_add", &json!({ "doc": doc, "page": 1, "type": "date", "at": [20, 300], "format": "dddd", "language": "ja" })) {
        Err(ToolError::InvalidArgs(e)) => assert!(e.contains("火曜日") && e.contains("can't be written into the PDF yet"), "{e}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    let f = ok(&mut a, "fill_sign_date_format", json!({ "format": "dddd", "language": "ja" }));
    assert_eq!((f["today"].as_str(), f["unwritable"].as_str()), (Some("火曜日"), Some("火曜日")));
    ok(&mut a, "fill_sign_date_format", json!({ "format": "d. mmmm yyyy", "language": "cs" }));
    assert!(matches!(a.call("fill_sign_date_format", &json!({ "format": "dd", "language": "xx" })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(
        a.call("fill_sign_add", &json!({ "doc": doc, "page": 1, "type": "date", "at": [20, 300], "language": "xx" })),
        Err(ToolError::InvalidArgs(_))
    ));
    let f = ok(&mut a, "fill_sign_date_format", json!({ "language": "auto" }));
    assert_eq!((f["format"].as_str(), f["language"].as_str(), f["today"].as_str()), (Some("d. mmmm yyyy"), Some("auto"), Some("14. November 2023")));
    assert!(matches!(
        a.call("fill_sign_add", &json!({ "doc": doc, "page": 1, "type": "date", "at": [20, 260], "format": "Year" })),
        Err(ToolError::InvalidArgs(_))
    ));
    let list = ok(&mut a, "comment_list", json!({ "doc": doc }));
    assert_eq!(list["count"], 7);
    assert!(matches!(a.call("fill_sign_add", &json!({ "doc": doc, "page": 1, "type": "text", "at": [1, 1] })), Err(ToolError::InvalidArgs(_))));
}

#[test]
fn fill_sign_text_font_size_is_saved_and_validated() {
    let dir = workdir("fill-sign-text-size");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "fill_sign_add", json!({ "doc": doc, "page": 1, "type": "text", "at": [20, 40], "text": "Big", "font_size": 24 }));
    let comments = ok(&mut a, "comment_list", json!({ "doc": doc }));
    let rect = comments["comments"][0]["rect"].as_array().unwrap();
    let height = rect[3].as_f64().unwrap() - rect[1].as_f64().unwrap();
    assert!(height > 30.0, "24 pt text has a taller annotation: {comments}");
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "sized.pdf" }));
    let saved = std::fs::read(dir.join("sized.pdf")).unwrap();
    let pdf = pdfcraft_cos::Document::open(std::sync::Arc::new(saved)).unwrap();
    let page = pdfcraft_model::pages(&pdf).remove(0);
    let annotations = pdf.resolve(page.dict.get(b"Annots").unwrap());
    let annotation = pdf.resolve(annotations.as_array().unwrap().first().unwrap());
    let da = annotation.as_dict().unwrap().get(b"DA").unwrap().as_string().unwrap();
    assert!(da.bytes.windows(b"/Helv 24 Tf".len()).any(|w| w == b"/Helv 24 Tf"), "saved annotation uses the requested font size");
    for size in [0, 3, 301] {
        assert!(matches!(
            a.call("fill_sign_add", &json!({ "doc": doc, "page": 1, "type": "text", "at": [20, 80], "text": "No", "font_size": size })),
            Err(ToolError::InvalidArgs(_))
        ));
    }
    assert!(matches!(
        a.call("fill_sign_add", &json!({ "doc": doc, "page": 1, "type": "signature", "at": [20, 80], "text": "Ada", "font_size": 24 })),
        Err(ToolError::InvalidArgs(_))
    ));
}

#[test]
fn image_signatures_through_tools_preserve_transparency_and_survive_save() {
    let dir = workdir("image-signatures");
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, 120, 40);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let rgba: Vec<u8> = (0..40)
            .flat_map(|y| (0..120).flat_map(move |x| if (40..80).contains(&x) && (10..30).contains(&y) { [0, 0, 0, 255] } else { [0, 0, 0, 0] }))
            .collect();
        encoder.write_header().unwrap().write_image_data(&rgba).unwrap();
    }
    std::fs::write(dir.join("signature.png"), png).unwrap();
    std::fs::write(dir.join("broken.png"), b"broken").unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_create", json!({ "from": "blank", "width": 200, "height": 300 }))["doc"].as_u64().unwrap();
    for (kind, y) in [("signature", 60), ("initials", 120)] {
        ok(&mut a, "fill_sign_add", json!({ "doc": doc, "page": 1, "type": kind, "at": [20, y], "path": "signature.png" }));
    }
    let render = |a: &mut Automation, doc| {
        let output = a.call("page_render", &json!({ "doc": doc, "page": 1, "dpi": 72 })).unwrap();
        let Content::Png { data, .. } = &output[0] else { panic!() };
        image::load_from_memory(data).unwrap().to_rgba8()
    };
    let before = render(&mut a, doc);
    assert_eq!(before.get_pixel(25, 60).0, [255, 255, 255, 255], "transparent margin exposes the page");
    assert_eq!(before.get_pixel(65, 60).0, [0, 0, 0, 255], "signature ink is embedded");
    assert_eq!(ok(&mut a, "edit_undo", json!({ "doc": doc }))["undone"], "Add initials");
    assert_eq!(ok(&mut a, "comment_list", json!({ "doc": doc }))["count"], 1);
    ok(&mut a, "edit_redo", json!({ "doc": doc }));
    // The same resize edit the selection handles use must keep the imported appearance.
    for rect in [json!([20, 40, 20, 88]), json!([20, 40, 100000000, 88])] {
        assert!(a.call("comment_edit", &json!({ "doc": doc, "page": 1, "index": 1, "rect": rect })).is_err());
    }
    ok(&mut a, "comment_lock", json!({ "doc": doc, "page": 1, "index": 1 }));
    assert!(a.call("comment_edit", &json!({ "doc": doc, "page": 1, "index": 1, "rect": [20, 40, 164, 88] })).is_err());
    ok(&mut a, "comment_lock", json!({ "doc": doc, "page": 1, "index": 1, "locked": false }));
    assert_eq!(render(&mut a, doc), before, "invalid and locked resizes leave the image intact");
    ok(&mut a, "comment_edit", json!({ "doc": doc, "page": 1, "index": 1, "rect": [20, 40, 164, 88] }));
    let resized = render(&mut a, doc);
    assert_eq!(resized.get_pixel(110, 64).0, [0, 0, 0, 255], "resizing scales the original ink");
    assert_eq!(resized.get_pixel(25, 64).0, [255, 255, 255, 255], "resizing retains alpha");
    assert_eq!(ok(&mut a, "edit_undo", json!({ "doc": doc }))["undone"], "Resize comment");
    assert_eq!(render(&mut a, doc), before);
    ok(&mut a, "edit_redo", json!({ "doc": doc }));
    assert_eq!(render(&mut a, doc), resized);
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "signed.pdf" }));
    let reopened = ok(&mut a, "doc_open", json!({ "path": "signed.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "comment_list", json!({ "doc": reopened }))["count"], 2);
    assert_eq!(render(&mut a, reopened), resized, "resized appearances retain alpha and geometry after save");
    for args in [
        json!({ "type": "signature", "path": "broken.png" }),
        json!({ "type": "signature", "path": "signature.png", "text": "Ada" }),
        json!({ "type": "check", "path": "signature.png" }),
        json!({ "type": "signature", "path": "../outside.png" }),
    ] {
        let mut args = args;
        args["doc"] = json!(doc);
        args["page"] = json!(1);
        args["at"] = json!([20, 80]);
        assert!(a.call("fill_sign_add", &args).is_err(), "{args}");
    }
    assert_eq!(ok(&mut a, "comment_list", json!({ "doc": doc }))["count"], 2, "errors leave the PDF intact");
}

#[test]
fn scanned_signatures_through_tools_drop_the_paper_and_report_bad_arguments() {
    let dir = workdir("scanned-signatures");
    // A grey, unevenly lit sheet with a pen stroke for the signature, and a blank one.
    let photo = |ink: bool| {
        let mut png = Vec::new();
        let mut encoder = png::Encoder::new(&mut png, 400, 300);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let rgb: Vec<u8> = (0..300_u32)
            .flat_map(|y| {
                (0..400_u32).flat_map(move |x| {
                    let paper = (215 - x / 20) as u8;
                    if ink && (100..300).contains(&x) && (144..148).contains(&y) { [20, 20, 25] } else { [paper, paper, paper] }
                })
            })
            .collect();
        encoder.write_header().unwrap().write_image_data(&rgb).unwrap();
        png
    };
    std::fs::write(dir.join("photo.png"), photo(true)).unwrap();
    std::fs::write(dir.join("blank.png"), photo(false)).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_create", json!({ "from": "blank", "width": 200, "height": 300 }))["doc"].as_u64().unwrap();
    ok(&mut a, "fill_sign_add", json!({ "doc": doc, "page": 1, "type": "signature", "at": [20, 60], "path": "photo.png", "scan": true }));
    let output = a.call("page_render", &json!({ "doc": doc, "page": 1, "dpi": 72 })).unwrap();
    let Content::Png { data, .. } = &output[0] else { panic!() };
    let page = image::load_from_memory(data).unwrap().to_rgba8();
    let dark = (20..170).filter(|&x| page.get_pixel(x, 60).0[0] < 80).count();
    assert!(dark > 100, "the stroke is placed as ink across the signature width ({dark} px)");
    for (x, y) in [(22, 50), (100, 50), (100, 70)] {
        assert_eq!(page.get_pixel(x, y).0, [255, 255, 255, 255], "the grey paper is gone at {x},{y}");
    }
    ok(
        &mut a,
        "fill_sign_add",
        json!({ "doc": doc, "page": 1, "type": "initials", "at": [20, 150], "path": "photo.png", "scan": true, "sensitivity": 0 }),
    );
    for args in [
        json!({ "type": "signature", "path": "blank.png", "scan": true }),
        json!({ "type": "signature", "path": "photo.png", "scan": true, "sensitivity": 101 }),
        json!({ "type": "signature", "path": "photo.png", "scan": true, "sensitivity": -1 }),
        json!({ "type": "signature", "path": "photo.png", "sensitivity": 50 }),
        json!({ "type": "signature", "text": "Ada", "scan": true }),
        json!({ "type": "signature", "path": "photo.png", "scan": "yes" }),
    ] {
        let mut args = args;
        args["doc"] = json!(doc);
        args["page"] = json!(1);
        args["at"] = json!([20, 80]);
        let error = a.call("fill_sign_add", &args).unwrap_err();
        assert!(matches!(error, ToolError::InvalidArgs(_)), "{args}: {error:?}");
    }
    let Err(ToolError::InvalidArgs(no_ink)) =
        a.call("fill_sign_add", &json!({ "doc": doc, "page": 1, "type": "signature", "at": [20, 80], "path": "blank.png", "scan": true }))
    else {
        panic!()
    };
    assert!(no_ink.contains("No signature was found"), "{no_ink}");
    assert_eq!(ok(&mut a, "comment_list", json!({ "doc": doc }))["count"], 2, "errors leave the PDF intact");
}

#[test]
fn image_signature_preview_layers_are_read_only_and_survive_encrypted_save() {
    let dir = workdir("signature-preview");
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, 120, 40);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let rgba: Vec<u8> = (0..40)
            .flat_map(|y| (0..120).flat_map(move |x| if (40..80).contains(&x) && (10..30).contains(&y) { [20, 40, 60, 128] } else { [0, 0, 0, 0] }))
            .collect();
        encoder.write_header().unwrap().write_image_data(&rgba).unwrap();
    }
    std::fs::write(dir.join("signature.png"), png).unwrap();
    std::fs::write(dir.join("form.pdf"), fixture(1)).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "form.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "page_rotate", json!({ "doc": doc, "pages": [1], "degrees": 90 }));
    ok(&mut a, "fill_sign_add", json!({ "doc": doc, "page": 1, "type": "initials", "at": [20, 40], "path": "signature.png" }));
    let background = a.call("page_render", &json!({ "doc": doc, "page": 1, "dpi": 72 })).unwrap();
    let Content::Png { data: expected, .. } = &background[0] else { panic!() };
    let expected = image::load_from_memory(expected).unwrap().to_rgba8();
    ok(&mut a, "fill_sign_add", json!({ "doc": doc, "page": 1, "type": "signature", "at": [120, 100], "path": "signature.png" }));
    ok(&mut a, "comment_edit", json!({ "doc": doc, "page": 1, "index": 2, "opacity": 0.5 }));
    ok(&mut a, "doc_protect", json!({ "doc": doc, "open_password": "preview-test" }));
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "signed.pdf" }));
    let reopened = ok(&mut a, "doc_open", json!({ "path": "signed.pdf", "password": "preview-test" }))["doc"].as_u64().unwrap();
    for doc in [doc, reopened] {
        let before = ok(&mut a, "doc_info", json!({ "doc": doc }));
        let d = a.session().docs().iter().find(|d| d.id.0 == doc).unwrap();
        let generation = d.edit_generation();
        let bytes = d.bytes.clone();
        let output = a.call("comment_image_preview", &json!({ "doc": doc, "page": 1, "index": 2, "dpi": 72 })).unwrap();
        let [Content::Json(meta), Content::Png { data: background, .. }] = output.as_slice() else { panic!("labeled background layer") };
        let image = a.call("comment_image_preview", &json!({ "doc": doc, "page": 1, "index": 2, "layer": "image" })).unwrap();
        let [Content::Json(image_meta), Content::Png { data: signature, width, height }] = image.as_slice() else { panic!("labeled image layer") };
        assert_eq!(meta["layer"], "background");
        assert_eq!(image_meta["layer"], "image");
        assert_eq!(meta["rotation"], 90);
        assert_eq!(meta["image_rotation"], 0, "placed upright as displayed on the turned page");
        assert_eq!(meta["opacity"], 0.5);
        assert_eq!((*width, *height), (120, 40));
        assert_eq!(image::load_from_memory(background).unwrap().to_rgba8(), expected, "page text and the other signature remain");
        let signature = image::load_from_memory(signature).unwrap().to_rgba8();
        assert_eq!(signature.get_pixel(5, 20).0[3], 0);
        assert_eq!(signature.get_pixel(60, 20).0, [20, 40, 60, 128], "embedded alpha and colour survive reopen");
        assert_eq!(ok(&mut a, "doc_info", json!({ "doc": doc })), before);
        let d = a.session().docs().iter().find(|d| d.id.0 == doc).unwrap();
        assert_eq!(d.edit_generation(), generation);
        assert!(std::sync::Arc::ptr_eq(&bytes, &d.bytes));
        assert!(d.image_signature_preview(0, 1).unwrap().unwrap().render_background(f32::NAN).is_err());
        assert!(d.image_signature_preview(0, 999).is_err());
    }
    assert!(tools().iter().find(|t| t.name == "comment_image_preview").unwrap().read_only);
    for dpi in [0, 601] {
        assert!(matches!(
            a.call("comment_image_preview", &json!({ "doc": reopened, "page": 1, "index": 2, "dpi": dpi })),
            Err(ToolError::InvalidArgs(_))
        ));
    }
    ok(&mut a, "fill_sign_add", json!({ "doc": reopened, "page": 1, "type": "check", "at": [10, 10] }));
    assert!(a.call("comment_image_preview", &json!({ "doc": reopened, "page": 1, "index": 3 })).is_err());
}

#[test]
fn creating_and_reducing_through_tools() {
    let dir = workdir("create");
    std::fs::write(dir.join("notes.txt"), "Meeting notes\nAction items").unwrap();
    let mut a = auto(&dir);
    let blank = ok(&mut a, "doc_create", json!({ "from": "blank", "pages": 2 }));
    assert_eq!((blank["pages"].as_u64(), blank["dirty"].as_bool()), (Some(2), Some(true)));
    let t = ok(&mut a, "doc_create", json!({ "from": "text", "path": "notes.txt" }))["doc"].as_u64().unwrap();
    assert_eq!(page_text(&mut a, t), ["Meeting notes\nAction items"]);
    ok(&mut a, "doc_save", json!({ "doc": t, "path": "notes.pdf" }));
    // A new text PDF is already compact: Reduce writes nothing rather than a copy no smaller.
    let r = ok(&mut a, "doc_reduce", json!({ "doc": t, "path": "notes-small.pdf" }));
    assert!(r["bytes_after"].as_u64().unwrap() >= r["bytes_before"].as_u64().unwrap(), "{r}");
    assert_eq!(r["written"], false);
    assert!(!dir.join("notes-small.pdf").exists());
    assert!(matches!(a.call("doc_create", &json!({ "from": "images", "paths": ["notes.txt"] })), Err(ToolError::Failed(_))));
}

#[test]
fn creating_from_text_refuses_what_the_font_cannot_draw() {
    let dir = workdir("create-undrawable");
    std::fs::write(dir.join("kr.txt"), "ABC 제안서 시험").unwrap();
    let mut a = auto(&dir);
    let refused = |r: Result<Vec<Content>, ToolError>| match r {
        Err(ToolError::InvalidArgs(m) | ToolError::Failed(m)) => assert!(m.contains("U+C81C"), "{m}"),
        other => panic!("expected a refusal, got {other:?}"),
    };
    assert!(matches!(a.call("doc_create", &json!({ "from": "text", "text": "ABC 제안서 시험" })), Err(ToolError::InvalidArgs(_))));
    refused(a.call("doc_create", &json!({ "from": "text", "text": "ABC 제안서 시험" })));
    refused(a.call("doc_create", &json!({ "from": "text", "path": "kr.txt" })));
    // WinAnsi characters, line breaks and tabs are still fine.
    let d = ok(&mut a, "doc_create", json!({ "from": "text", "text": "Price: 5 € at café\r\n\tnext\u{c}page two" }))["doc"].as_u64().unwrap();
    assert_eq!(page_text(&mut a, d).len(), 2);
    // #959: page_add_text refuses it too.
    let doc = ok(&mut a, "doc_create", json!({ "from": "blank" }))["doc"].as_u64().unwrap();
    refused(a.call("page_add_text", &json!({ "doc": doc, "page": 1, "text": "ABC 제안서 시험", "at": [20, 20] })));
}

#[test]
fn creating_from_multiple_files_through_tools() {
    let dir = workdir("create-multiple");
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, 4, 2);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&[100; 4 * 2 * 3]).unwrap();
    }
    std::fs::write(dir.join("scan.png"), png).unwrap();
    std::fs::write(dir.join("notes.txt"), "hello").unwrap();
    std::fs::write(dir.join("report.docx"), b"PK\x03\x04").unwrap();
    let mut a = auto(&dir);

    // Combine: every file converted, in the order given, with a bookmark per file.
    let made = ok(
        &mut a,
        "doc_create_multiple",
        json!({ "paths": ["notes.txt", "a.pdf", "scan.png"], "pages": [null, "3", null], "out": "all.pdf", "open": true }),
    );
    let doc = made["document"]["doc"].as_u64().unwrap();
    assert_eq!(page_text(&mut a, doc), ["hello", "Page 3", ""]);
    let titles: Vec<String> = ok(&mut a, "bookmark_list", json!({ "doc": doc }))["bookmarks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["title"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(titles, ["notes", "a", "scan"]);
    assert!(dir.join("all.pdf").is_file());
    // A file that can't be converted fails the whole combine, naming it.
    let err = a.call("doc_create_multiple", &json!({ "paths": ["a.pdf", "report.docx"] })).unwrap_err();
    assert!(matches!(&err, ToolError::Failed(m) if m.contains("report.docx") && m.contains("can't be converted")), "{err}");
    assert!(matches!(a.call("doc_create_multiple", &json!({ "paths": [] })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(a.call("doc_create_multiple", &json!({ "paths": ["a.pdf"], "mode": "zip" })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(a.call("doc_create_multiple", &json!({ "paths": ["a.pdf"], "mode": "separate" })), Err(ToolError::InvalidArgs(_))));

    // Separate: one PDF per file; PDFs are skipped, a bad file doesn't stop the rest, and an
    // existing file is never overwritten.
    std::fs::create_dir_all(dir.join("out")).unwrap();
    std::fs::write(dir.join("out/notes.pdf"), b"mine").unwrap();
    let args = json!({ "paths": ["notes.txt", "report.docx", "a.pdf", "scan.png", "missing.txt"], "mode": "separate", "out_dir": "out" });
    let made = ok(&mut a, "doc_create_multiple", args);
    let files = made["files"].as_array().unwrap();
    assert!(files[0]["output"].as_str().unwrap().ends_with("notes (2).pdf"), "{files:?}");
    assert!(files[1]["error"].as_str().unwrap().contains("can't be converted"));
    assert_eq!(files[2]["skipped"], "already a PDF");
    assert!(files[3]["output"].as_str().unwrap().ends_with("scan.pdf"));
    assert!(files[4]["error"].is_string());
    assert_eq!(std::fs::read(dir.join("out/notes.pdf")).unwrap(), b"mine");
    let reopened = ok(&mut a, "doc_open", json!({ "path": "out/notes (2).pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(page_text(&mut a, reopened), ["hello"]);
    assert!(!dir.join("out/a.pdf").exists());
}

#[test]
fn create_multiple_has_no_file_count_limit() {
    let dir = workdir("create-many");
    let mut a = auto(&dir);
    // The same 3-page PDF listed 1,001 times: every listed file is a source, however many there are.
    let paths = vec!["a.pdf"; 1001];
    let made = ok(&mut a, "doc_create_multiple", json!({ "paths": paths, "out": "many.pdf", "open": true }));
    assert_eq!(made["document"]["pages"], 3 * 1001, "{made}");
    assert!(dir.join("many.pdf").is_file());
}

#[test]
fn creating_images_with_dpi_through_tools() {
    let dir = workdir("image-dpi");
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, 300, 150);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_pixel_dims(Some(png::PixelDimensions { xppu: 11811, yppu: 5906, unit: png::Unit::Meter }));
        enc.write_header().unwrap().write_image_data(&vec![100; 300 * 150 * 3]).unwrap();
    }
    std::fs::write(dir.join("scan.png"), png).unwrap();
    let mut a = auto(&dir);
    for (dpi, width, height) in [(None, 72.0, 72.0), (Some(72.0), 300.0, 150.0), (Some(300.0), 72.0, 36.0)] {
        let mut args = json!({ "from": "images", "paths": ["scan.png"] });
        if let Some(dpi) = dpi {
            args["dpi"] = json!(dpi);
        }
        let doc = ok(&mut a, "doc_create", args)["doc"].as_u64().unwrap();
        let info = ok(&mut a, "doc_info", json!({ "doc": doc }));
        assert!((info["pages"][0]["width"].as_f64().unwrap() - width).abs() < 0.02);
        assert!((info["pages"][0]["height"].as_f64().unwrap() - height).abs() < 0.02);
        ok(&mut a, "doc_save", json!({ "doc": doc, "path": "made.pdf" }));
        let reopened = ok(&mut a, "doc_open", json!({ "path": "made.pdf" }))["doc"].as_u64().unwrap();
        let render = a.call("page_render", &json!({ "doc": reopened, "page": 1, "dpi": 72 })).unwrap();
        let Content::Png { width: w, height: h, .. } = &render[0] else { panic!("expected PNG") };
        assert!((*w as f64 - width).abs() <= 1.0 && (*h as f64 - height).abs() <= 1.0);
    }
    for dpi in [0.0, -72.0, 1201.0] {
        assert!(a.call("doc_create", &json!({ "from": "images", "paths": ["scan.png"], "dpi": dpi })).is_err());
    }
}

#[test]
fn saving_with_flatten_fill_sign_bakes_marks_and_leaves_other_comments() {
    let dir = workdir("fill-sign-flatten");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "fill_sign_add", json!({ "doc": doc, "page": 1, "type": "text", "at": [40.0, 80.0], "text": "Hello" }));
    ok(&mut a, "comment_add", json!({ "doc": doc, "page": 1, "type": "highlight", "find": "Page", "contents": "keep" }));
    ok(&mut a, "form_add_field", json!({ "doc": doc, "page": 1, "type": "text", "rect": [20.0, 120.0, 180.0, 142.0] }));
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "kept.pdf" }));
    let kept = ok(&mut a, "doc_open", json!({ "path": "kept.pdf" }))["doc"].as_u64().unwrap();
    let kept_comments = ok(&mut a, "comment_list", json!({ "doc": kept }))["comments"].as_array().unwrap().clone();
    assert!(kept_comments.iter().any(|c| c["contents"] == "Hello"), "without the flag the typewriter stays: {kept_comments:?}");

    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "flat.pdf", "flatten_fill_sign": true }));
    let flat = ok(&mut a, "doc_open", json!({ "path": "flat.pdf" }))["doc"].as_u64().unwrap();
    let comments = ok(&mut a, "comment_list", json!({ "doc": flat }))["comments"].as_array().unwrap().clone();
    assert!(comments.iter().all(|c| c["contents"] != "Hello"), "the typewriter was baked in: {comments:?}");
    assert!(comments.iter().any(|c| c["contents"] == "keep"), "the highlight stays: {comments:?}");
    let fields = ok(&mut a, "form_fields", json!({ "doc": flat }))["fields"].as_array().unwrap().clone();
    assert_eq!(fields.len(), 1, "the form field stays");
}

#[test]
fn flattening_through_tools() {
    let dir = workdir("flatten");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "comment_add", json!({ "doc": doc, "page": 1, "type": "textbox", "rect": [10, 200, 190, 240], "contents": "Approved" }));
    ok(&mut a, "doc_flatten", json!({ "doc": doc }));
    assert_eq!(ok(&mut a, "comment_list", json!({ "doc": doc }))["count"], 0);
    assert!(page_text(&mut a, doc)[0].contains("Approved"), "the text box is now page text");
    assert!(matches!(a.call("doc_flatten", &json!({ "doc": doc, "comments": false, "fields": false })), Err(ToolError::InvalidArgs(_))));
}

/// Inserting a file's pages brings its bookmarks too, under one named after the file, at the
/// pages' place among the document's own bookmarks.
#[test]
fn inserting_a_file_brings_its_bookmarks() {
    let dir = workdir("insert-bookmarks");
    let mut a = auto(&dir);
    let annex = ok(&mut a, "doc_open", json!({ "path": "b.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "bookmark_add", json!({ "doc": annex, "title": "Annex intro", "page": 1 }));
    ok(&mut a, "bookmark_add", json!({ "doc": annex, "title": "Annex table", "page": 2 }));
    ok(&mut a, "doc_save", json!({ "doc": annex, "path": "annex.pdf" }));
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "bookmark_add", json!({ "doc": doc, "title": "Guide start", "page": 1 }));
    ok(&mut a, "bookmark_add", json!({ "doc": doc, "title": "Guide end", "page": 3 }));
    ok(&mut a, "page_insert_file", json!({ "doc": doc, "path": "annex.pdf", "at": 2 }));
    let list = ok(&mut a, "bookmark_list", json!({ "doc": doc }));
    let top: Vec<(&str, Value)> = list["bookmarks"].as_array().unwrap().iter().map(|b| (b["title"].as_str().unwrap(), b["page"].clone())).collect();
    assert_eq!(top, [("Guide start", json!(1)), ("annex", json!(2)), ("Guide end", json!(5))], "{list}");
    let kids: Vec<(&str, Value)> =
        list["bookmarks"][1]["children"].as_array().unwrap().iter().map(|b| (b["title"].as_str().unwrap(), b["page"].clone())).collect();
    assert_eq!(kids, [("Annex intro", json!(2)), ("Annex table", json!(3))], "{list}");
}

#[test]
fn replacing_pages_through_tools() {
    let dir = workdir("replace");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "page_replace", json!({ "doc": doc, "pages": [2, 3], "path": "b.pdf", "from_pages": [2, 1] }));
    assert_eq!(page_text(&mut a, doc), ["Page 1", "Page 2", "Page 1"]);
    assert!(
        matches!(a.call("page_replace", &json!({ "doc": doc, "pages": [1, 2, 3], "path": "b.pdf" })), Err(ToolError::Failed(_))),
        "b.pdf has only 2 pages"
    );
}

#[test]
fn preparing_a_form_through_tools() {
    let dir = workdir("prepare");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let add = |a: &mut Automation, args: Value| ok(a, "form_add_field", args)["field"].as_str().unwrap().to_owned();
    assert_eq!(add(&mut a, json!({ "doc": doc, "page": 1, "type": "text", "rect": [20, 20, 180, 42] })), "Text1");
    assert_eq!(add(&mut a, json!({ "doc": doc, "page": 1, "type": "combo", "rect": [20, 60, 180, 82], "options": ["Red", "Green"] })), "Dropdown1");
    assert_eq!(add(&mut a, json!({ "doc": doc, "page": 1, "type": "radio", "rect": [20, 100, 34, 114], "group": "size", "export": "S" })), "size");
    assert_eq!(add(&mut a, json!({ "doc": doc, "page": 1, "type": "radio", "rect": [40, 100, 54, 114], "group": "size", "export": "L" })), "size");
    let r = ok(&mut a, "form_set_props", json!({ "doc": doc, "field": "Text1", "name": "full name", "required": true, "tooltip": "Your name" }));
    assert_eq!(r["field"], "full name");
    ok(&mut a, "form_delete_field", json!({ "doc": doc, "field": "Dropdown1" }));
    ok(&mut a, "form_set_props", json!({ "doc": doc, "field": "full name", "rect": [30, 20, 190, 42] }));
    let fields = ok(&mut a, "form_fields", json!({ "doc": doc }));
    let f = fields["fields"].as_array().unwrap();
    assert_eq!(f.iter().map(|f| f["name"].as_str().unwrap()).collect::<Vec<_>>(), ["full name", "size"]);
    assert_eq!((f[0]["required"].as_bool(), f[0]["tooltip"].as_str()), (Some(true), Some("Your name")));
    assert_eq!(f[0]["rect"], json!([30.0, 20.0, 190.0, 42.0]), "moved; the rect round-trips in view coordinates");
    assert_eq!(f[1]["options"], json!(["S", "L"]));
    // #94: the mark a check box or radio button shows.
    assert_eq!(f[1]["check_style"], "circle", "radio buttons default to a circle");
    ok(&mut a, "form_set_props", json!({ "doc": doc, "field": "size", "check_style": "star" }));
    assert_eq!(ok(&mut a, "form_fields", json!({ "doc": doc }))["fields"][1]["check_style"], "star");
    assert!(matches!(a.call("form_set_props", &json!({ "doc": doc, "field": "size", "check_style": "heart" })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(a.call("form_set_props", &json!({ "doc": doc, "field": "full name", "check_style": "star" })), Err(ToolError::Failed(_))));
    ok(&mut a, "form_fill", json!({ "doc": doc, "values": { "full name": "Ada", "size": "L" } }));
    assert!(page_text(&mut a, doc)[0].contains("Ada"));
    // Options tab: alignment, default, flags (comb needs a limit).
    assert!(matches!(a.call("form_set_props", &json!({ "doc": doc, "field": "full name", "flags": { "comb": true } })), Err(ToolError::Failed(_))));
    ok(
        &mut a,
        "form_set_props",
        json!({ "doc": doc, "field": "full name", "align": "center", "default": "Anon", "max_length": 8, "flags": { "comb": true, "spell_check": false } }),
    );
    assert!(matches!(
        a.call("form_set_props", &json!({ "doc": doc, "field": "full name", "flags": { "sparkles": true } })),
        Err(ToolError::InvalidArgs(_))
    ));
    assert!(matches!(
        a.call("form_add_field", &json!({ "doc": doc, "page": 1, "type": "slider", "rect": [0, 0, 9, 9] })),
        Err(ToolError::InvalidArgs(_))
    ));
    assert!(matches!(a.call("form_delete_field", &json!({ "doc": doc, "field": "nope" })), Err(ToolError::Failed(_))));
    // An image field shows the picture it is given.
    assert_eq!(add(&mut a, json!({ "doc": doc, "page": 2, "type": "image", "rect": [20, 20, 120, 120] })), "Image1");
    let png = |a: &mut Automation| match a.call("page_render", &json!({ "doc": doc, "page": 2, "dpi": 36 })).unwrap().remove(0) {
        Content::Png { data, .. } => data,
        other => panic!("{other:?}"),
    };
    let before = png(&mut a);
    ok(&mut a, "doc_export_images", json!({ "doc": doc, "folder": "pics", "dpi": 18, "pages": [1] }));
    ok(&mut a, "form_set_image", json!({ "doc": doc, "field": "Image1", "path": "pics/a_page_1.png" }));
    let after = png(&mut a);
    assert_ne!(before, after, "the picture is drawn");
    assert_eq!(ok(&mut a, "edit_undo", json!({ "doc": doc }))["undone"], "Set the image of Image1");
    assert!(matches!(a.call("form_set_image", &json!({ "doc": doc, "field": "size", "path": "pics/a_page_1.png" })), Err(ToolError::Failed(_))));
}

#[test]
fn rotating_a_field_through_tools() {
    let dir = workdir("rotate-field");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "form_add_field", json!({ "doc": doc, "page": 1, "type": "text", "rect": [20, 20, 180, 42], "name": "City" }));
    let rect_of = |a: &mut Automation| {
        let f = &ok(a, "form_fields", json!({ "doc": doc }))["fields"][0];
        (f["rotation"].as_i64().unwrap(), f["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>())
    };
    let (rot, before) = rect_of(&mut a);
    assert_eq!(rot, 0);
    assert!(matches!(a.call("form_set_props", &json!({ "doc": doc, "field": "City", "rotation": 45 })), Err(ToolError::InvalidArgs(_))));
    assert_eq!(rect_of(&mut a).1, before, "a rejected rotation changes nothing");
    ok(&mut a, "form_set_props", json!({ "doc": doc, "field": "City", "rotation": 90 }));
    let (rot, turned) = rect_of(&mut a);
    assert_eq!(rot, 90);
    let (bw, bh) = (before[2] - before[0], before[3] - before[1]);
    let (tw, th) = (turned[2] - turned[0], turned[3] - turned[1]);
    assert!((tw - bh).abs() < 0.2 && (th - bw).abs() < 0.2, "swapped {before:?} -> {turned:?}");
    let center = |r: &[f64]| ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    let (bc, tc) = (center(&before), center(&turned));
    assert!((bc.0 - tc.0).abs() < 0.2 && (bc.1 - tc.1).abs() < 0.2, "center moved {bc:?} -> {tc:?}");
    assert_eq!(ok(&mut a, "edit_undo", json!({ "doc": doc }))["undone"], "Change field properties");
    let (rot, back) = rect_of(&mut a);
    assert_eq!((rot, back), (0, before));
}

#[test]
fn redacting_through_tools() {
    let dir = workdir("redact");
    std::fs::write(dir.join("memo.txt"), "Call 555-123-4567 today\nSSN 123-45-6789 is private\nPublic line").unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_create", json!({ "from": "text", "path": "memo.txt" }))["doc"].as_u64().unwrap();
    let r = ok(&mut a, "redact_mark", json!({ "doc": doc, "pattern": "phone" }));
    assert_eq!((r["marked"].as_u64(), r["marks_pending"].as_u64()), (Some(1), Some(1)));
    ok(&mut a, "redact_mark", json!({ "doc": doc, "pattern": "ssn", "overlay": "SSN" }));
    ok(&mut a, "redact_mark", json!({ "doc": doc, "find": "private" }));
    assert_eq!(page_text(&mut a, doc)[0].matches("555-123-4567").count(), 1, "marks alone remove nothing");
    let r = ok(&mut a, "redact_apply", json!({ "doc": doc }));
    assert_eq!((r["applied"].as_u64(), r["marks_pending"].as_u64()), (Some(3), Some(0)));
    let text = page_text(&mut a, doc)[0].clone();
    assert!(!text.contains("555") && !text.contains("6789") && !text.contains("private"), "{text}");
    assert!(text.contains("Call") && text.contains("today") && text.contains("Public line"), "{text}");
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "memo.pdf" }));
    let bytes = std::fs::read(dir.join("memo.pdf")).unwrap();
    assert!(!bytes.windows(4).any(|w| w == b"4567"), "the saved file has no trace of the number");
    assert!(matches!(a.call("redact_apply", &json!({ "doc": doc })), Err(ToolError::Failed(_))));
    assert!(matches!(a.call("redact_mark", &json!({ "doc": doc, "find": "nowhere to be found" })), Err(ToolError::Failed(_))));
    assert!(matches!(a.call("redact_mark", &json!({ "doc": doc })), Err(ToolError::InvalidArgs(_))));
}

#[test]
fn redacting_with_codes_through_tools() {
    let dir = workdir("redact-codes");
    std::fs::write(dir.join("memo.txt"), "Informant Jane Roe met the agent\nPublic line").unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_create", json!({ "from": "text", "path": "memo.txt" }))["doc"].as_u64().unwrap();
    let bad = |a: &mut Automation, args: Value| matches!(a.call("redact_mark", &args), Err(ToolError::InvalidArgs(_)));
    assert!(bad(&mut a, json!({ "doc": doc, "find": "Jane Roe", "code_set": "gdpr", "codes": ["(b)(6)"] })));
    assert!(bad(&mut a, json!({ "doc": doc, "find": "Jane Roe", "code_set": "foia", "codes": ["(k)(1)"] })));
    assert!(bad(&mut a, json!({ "doc": doc, "find": "Jane Roe", "code_set": "foia", "codes": [] })));
    assert!(bad(&mut a, json!({ "doc": doc, "find": "Jane Roe", "codes": ["(b)(6)"] })));
    assert!(bad(&mut a, json!({ "doc": doc, "find": "Jane Roe", "overlay": "X", "code_set": "foia", "codes": ["(b)(6)"] })));
    let Err(ToolError::InvalidArgs(msg)) =
        a.call("redact_mark", &json!({ "doc": doc, "find": "Jane Roe", "code_set": "foia", "codes": ["(b)(10)"] }))
    else {
        panic!("an unknown code is rejected")
    };
    assert!(msg.contains("(b)(7)(C)"), "the error lists the allowed codes: {msg}");
    assert_eq!(
        ok(&mut a, "redact_mark", json!({ "doc": doc, "find": "Jane Roe", "code_set": "foia", "codes": ["(b)(7)(C)", "(b)(6)"] }))["marked"],
        1
    );
    ok(&mut a, "redact_apply", json!({ "doc": doc }));
    let text = page_text(&mut a, doc)[0].clone();
    assert!(!text.contains("Jane") && text.contains("(b)(6), (b)(7)(C)"), "{text}");
}

#[test]
fn redacting_word_lists_through_tools() {
    let dir = workdir("redact-words");
    std::fs::write(dir.join("memo.txt"), "Call Ada today\nAda is private\nPublic line").unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_create", json!({ "from": "text", "path": "memo.txt" }))["doc"].as_u64().unwrap();
    let r = ok(&mut a, "redact_mark", json!({ "doc": doc, "words": ["Ada", " private ", "absent", "Ada"] }));
    assert_eq!(r["marked"].as_u64(), Some(3), "{r}");
    assert_eq!(r["matched_words"], json!([{ "word": "Ada", "marked": 2 }, { "word": "private", "marked": 1 }, { "word": "absent", "marked": 0 }]));
    assert_eq!(ok(&mut a, "edit_undo", json!({ "doc": doc }))["undone"], "Mark for redaction");
    assert_eq!(ok(&mut a, "redact_mark", json!({ "doc": doc, "words": ["Ada", "private"] }))["marks_pending"].as_u64(), Some(3));
    ok(&mut a, "redact_apply", json!({ "doc": doc }));
    let text = page_text(&mut a, doc)[0].clone();
    assert!(!text.contains("Ada") && !text.contains("private"), "{text}");
    assert!(text.contains("Call") && text.contains("Public line"), "{text}");
    for bad in [json!({ "doc": doc, "words": [] }), json!({ "doc": doc, "words": ["  "] }), json!({ "doc": doc, "words": ["x"], "find": "x" })] {
        assert!(matches!(a.call("redact_mark", &bad), Err(ToolError::InvalidArgs(_))), "{bad}");
    }
    assert!(matches!(a.call("redact_mark", &json!({ "doc": doc, "words": ["absent", "nowhere"] })), Err(ToolError::Failed(_))));
}

#[test]
fn redaction_search_respects_case_and_word_boundaries() {
    let dir = workdir("redact-find-options");
    std::fs::write(dir.join("memo.txt"), "Ada ada Adaline\nAda beta betamax").unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_create", json!({ "from": "text", "path": "memo.txt" }))["doc"].as_u64().unwrap();
    let mark = |a: &mut Automation, args: Value| ok(a, "redact_mark", args)["marked"].as_u64().unwrap();
    assert_eq!(mark(&mut a, json!({ "doc": doc, "find": "Ada", "case_sensitive": true, "whole_words": true })), 2);
    ok(&mut a, "edit_undo", json!({ "doc": doc }));
    assert_eq!(mark(&mut a, json!({ "doc": doc, "find": "Ada", "whole_words": true })), 3);
    ok(&mut a, "edit_undo", json!({ "doc": doc }));
    let result = ok(&mut a, "redact_mark", json!({ "doc": doc, "words": ["Ada", "beta"], "case_sensitive": true, "whole_words": true }));
    assert_eq!(result["matched_words"], json!([{ "word": "Ada", "marked": 2 }, { "word": "beta", "marked": 1 }]));
    ok(&mut a, "edit_undo", json!({ "doc": doc }));
    assert!(matches!(a.call("redact_mark", &json!({ "doc": doc, "pattern": "email", "whole_words": true })), Err(ToolError::InvalidArgs(_))));
    assert_eq!(mark(&mut a, json!({ "doc": doc, "find": "Ada", "case_sensitive": true, "whole_words": true })), 2);
    ok(&mut a, "redact_apply", json!({ "doc": doc }));
    let text = &page_text(&mut a, doc)[0];
    assert!(text.contains("ada") && text.contains("Adaline") && text.contains("betamax"), "{text}");
}

#[test]
fn removing_hidden_information_through_tools() {
    let dir = workdir("hidden");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "comment_add", json!({ "doc": doc, "page": 1, "type": "note", "at": [20, 20], "contents": "internal note" }));
    ok(&mut a, "doc_set_info", json!({ "doc": doc, "key": "Title", "value": "Secret plan" }));
    let info = ok(&mut a, "doc_hidden_info", json!({ "doc": doc }));
    let count = |v: &Value, c: &str| v["categories"].as_array().unwrap().iter().find(|x| x["category"] == c).unwrap()["count"].as_u64().unwrap();
    assert!(count(&info, "comments") >= 1 && count(&info, "metadata") >= 1, "{info}");
    let r = ok(&mut a, "doc_remove_hidden", json!({ "doc": doc, "categories": ["comments"] }));
    assert_eq!(r["undo"], "Remove hidden information");
    assert_eq!(ok(&mut a, "comment_list", json!({ "doc": doc }))["count"], 0);
    assert!(count(&ok(&mut a, "doc_hidden_info", json!({ "doc": doc })), "metadata") >= 1, "only comments went");
    let r = ok(&mut a, "doc_remove_hidden", json!({ "doc": doc }));
    assert_eq!(r["undo"], "Sanitize document");
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "clean.pdf" }));
    let bytes = std::fs::read(dir.join("clean.pdf")).unwrap();
    assert!(!bytes.windows(11).any(|w| w == b"Secret plan"), "the old revision is gone");
    assert!(matches!(a.call("doc_remove_hidden", &json!({ "doc": doc, "categories": ["nonsense"] })), Err(ToolError::InvalidArgs(_))));
}

/// A one-page PDF whose text "TOP SECRET" sits under an opaque black rectangle.
fn faux_redacted_fixture() -> Vec<u8> {
    helvetica_page("BT /F1 24 Tf 100 700 Td (TOP SECRET) Tj ET\n0 g\n90 688 220 32 re f")
}

/// A one-page US Letter PDF drawing `body` with Helvetica as /F1.
fn helvetica_page(body: &str) -> Vec<u8> {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_string().into_bytes(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

#[test]
fn auditing_faux_redactions_through_tools() {
    let dir = workdir("faux-redaction-audit");
    std::fs::write(dir.join("redacted.pdf"), faux_redacted_fixture()).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "redacted.pdf" }))["doc"].as_u64().unwrap();
    let r = ok(&mut a, "doc_audit_redactions", json!({ "doc": doc }));
    assert_eq!(r["count"], 1, "{r}");
    let f = &r["findings"][0];
    assert_eq!(f["page"], 1);
    assert_eq!(f["kind"], "covered-text");
    assert!(f["covered_text"].as_str().unwrap().contains("TOP SECRET"), "{f}");
    // Points from the top-left of the page, like every other tool (the cover is `90 688 220 32 re`
    // on a 792 pt high page).
    assert_eq!(f["rect"], json!([90.0, 72.0, 310.0, 104.0]), "{f}");
    // A clean document audits clean.
    let clean = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "doc_audit_redactions", json!({ "doc": clean }))["count"], 0);
}

/// An applied redaction at the end of a line leaves the words before it, then a gap the text
/// matrix jumps over and the redaction box: nothing is under the box any more, so the audit must
/// not report the words beside it as covered. In the middle of a line it never did.
#[test]
fn an_applied_redaction_at_the_end_of_a_line_audits_clean() {
    let dir = workdir("audit-applied-end-of-line");
    let body = "BT /F1 11 Tf 72 692 Td (Mail the team: someone@example.com) Tj ET\nBT /F1 11 Tf 72 662 Td (Account 0042-7788 is in the middle of this line.) Tj ET";
    std::fs::write(dir.join("mail.pdf"), helvetica_page(body)).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "mail.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "redact_mark", json!({ "doc": doc, "pattern": "email" }));
    ok(&mut a, "redact_mark", json!({ "doc": doc, "find": "0042-7788" }));
    ok(&mut a, "redact_apply", json!({ "doc": doc }));
    let text = ok(&mut a, "text_extract", json!({ "doc": doc }))["pages"][0]["text"].as_str().unwrap().to_string();
    assert!(text.contains("Mail the team:") && !text.contains("someone") && !text.contains("0042"), "{text}");
    let r = ok(&mut a, "doc_audit_redactions", json!({ "doc": doc }));
    assert_eq!(r["count"], 0, "{r}");
}

/// A long tagged document lists its tags a page at a time, and only the types, pages or depths
/// asked for: a 20-page guide has hundreds of tags, more than one tool result should hold.
#[test]
fn structure_tags_are_listed_a_page_at_a_time() {
    let dir = workdir("tags-paging");
    // A Document holding, on each of 3 pages, one H1 and 149 P: 451 tags.
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 6 0 R /MarkInfo << /Marked true >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 200 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
        String::new(), // 6: the root, below
        String::new(), // 7: the Document, below
    ];
    let mut kids = Vec::new();
    for page in 0..3 {
        for i in 0..150 {
            let n = objs.len() + 1;
            kids.push(format!("{n} 0 R"));
            objs.push(format!("<< /S /{} /P 7 0 R /Pg {} 0 R >>", if i == 0 { "H1" } else { "P" }, 3 + page));
        }
    }
    objs[5] = "<< /Type /StructTreeRoot /K 7 0 R >>".into();
    objs[6] = format!("<< /S /Document /P 6 0 R /K [{}] >>", kids.join(" "));
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    std::fs::write(dir.join("long.pdf"), pdf).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "long.pdf" }))["doc"].as_u64().unwrap();
    let first = ok(&mut a, "accessibility_tags", json!({ "doc": doc }));
    assert_eq!((first["count"].as_u64(), first["tags"].as_array().map(Vec::len), first["next"].as_u64()), (Some(451), Some(200), Some(200)));
    assert_eq!(first["tags"][0]["type"], "Document");
    let last = ok(&mut a, "accessibility_tags", json!({ "doc": doc, "offset": 400, "limit": 100 }));
    assert_eq!((last["tags"].as_array().map(Vec::len), last["next"].clone()), (Some(51), Value::Null));
    let headings = ok(&mut a, "accessibility_tags", json!({ "doc": doc, "types": ["H1"] }));
    let pages: Vec<u64> = headings["tags"].as_array().unwrap().iter().map(|t| t["page"].as_u64().unwrap()).collect();
    assert_eq!((headings["count"].as_u64(), pages), (Some(3), vec![1, 2, 3]));
    let page2 = ok(&mut a, "accessibility_tags", json!({ "doc": doc, "pages": [2], "types": ["H1", "P"], "limit": 1000 }));
    assert_eq!((page2["count"].as_u64(), page2["next"].clone()), (Some(150), Value::Null));
    assert_eq!(ok(&mut a, "accessibility_tags", json!({ "doc": doc, "max_depth": 0 }))["count"], 1, "the Document alone");
    for bad in [
        json!({ "doc": doc, "limit": 0 }),
        json!({ "doc": doc, "limit": 1001 }),
        json!({ "doc": doc, "offset": -1 }),
        json!({ "doc": doc, "pages": [0] }),
    ] {
        assert!(matches!(a.call("accessibility_tags", &bad), Err(ToolError::InvalidArgs(_))), "{bad}");
    }
}

#[test]
fn printing_through_tools() {
    let dir = workdir("print");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    assert!(ok(&mut a, "printers", json!({}))["printers"].is_array());
    let none = ok(&mut a, "printer_options", json!({ "printer": "No_Such_Queue_PdfCraft" }));
    assert_eq!((none["count"].as_u64(), none["options"].as_array().map(Vec::len)), (Some(0), Some(0)));
    assert!(matches!(a.call("printer_options", &json!({})), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(
        a.call("doc_print", &json!({ "doc": doc, "printer": "default", "options": ["InputSlot"] })),
        Err(ToolError::InvalidArgs(m)) if m.contains("printer_options")
    ));
    assert!(matches!(
        a.call("doc_print", &json!({ "doc": doc, "printer": "default", "options": { "InputSlot": 2 } })),
        Err(ToolError::InvalidArgs(m)) if m.contains("options.InputSlot")
    ));
    let n = ok(&mut a, "doc_info", json!({ "doc": doc }))["document"]["pages"].as_u64().unwrap();
    let r = ok(&mut a, "doc_print", json!({ "doc": doc, "layout": "multiple", "per_sheet": 4, "path": "sheets.pdf" }));
    assert_eq!(r["sheets"].as_u64(), Some(n.div_ceil(4)));
    let printed = ok(&mut a, "doc_open", json!({ "path": "sheets.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "doc_info", json!({ "doc": printed }))["document"]["pages"].as_u64(), Some(n.div_ceil(4)));
    let r = ok(&mut a, "doc_print", json!({ "doc": doc, "pages": "1", "layout": "poster", "scale": 400, "path": "poster.pdf" }));
    assert!(r["sheets"].as_u64().unwrap() > 1);
    assert!(matches!(a.call("doc_print", &json!({ "doc": doc })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(a.call("doc_print", &json!({ "doc": doc, "pages": "99", "path": "x.pdf" })), Err(ToolError::InvalidArgs(_))));
}

#[test]
fn adding_content_through_tools() {
    let dir = workdir("content");
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, 40, 20);
        enc.set_color(png::ColorType::Rgb);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[200u8; 2400]).unwrap();
    }
    std::fs::write(dir.join("logo.png"), png).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "page_add_text", json!({ "doc": doc, "page": 1, "text": "CONFIDENTIAL", "at": [20, 20], "size": 18, "bold": true, "color": "red" }));
    let r = ok(&mut a, "page_add_image", json!({ "doc": doc, "page": 1, "path": "logo.png", "rect": [100, 200, 180, 240] }));
    assert_eq!(r["rect"], json!([100.0, 200.0, 180.0, 240.0]));
    assert!(page_text(&mut a, doc)[0].contains("CONFIDENTIAL"));
    let list = ok(&mut a, "content_list", json!({ "doc": doc }));
    assert_eq!(list["count"], 2);
    assert_eq!((list["items"][0]["type"].as_str(), list["items"][0]["bold"].as_bool()), (Some("text"), Some(true)));
    ok(&mut a, "content_update", json!({ "doc": doc, "page": 1, "index": 1, "text": "DRAFT", "font": "times" }));
    let text = page_text(&mut a, doc)[0].clone();
    assert!(text.contains("DRAFT") && !text.contains("CONFIDENTIAL"), "{text}");
    // Image tools: rotate, flip, crop, replace.
    ok(&mut a, "content_update", json!({ "doc": doc, "page": 1, "index": 2, "rotate": 90, "flip_h": true, "crop": [0.1, 0, 0.1, 0] }));
    ok(&mut a, "content_update", json!({ "doc": doc, "page": 1, "index": 2, "image": "logo.png" }));
    assert_eq!(ok(&mut a, "doc_info", json!({ "doc": doc }))["document"]["dirty"], true);
    assert!(matches!(a.call("content_update", &json!({ "doc": doc, "page": 1, "index": 2, "rotate": 45 })), Err(ToolError::InvalidArgs(_))));
    assert!(
        matches!(a.call("content_update", &json!({ "doc": doc, "page": 1, "index": 1, "rotate": 90 })), Err(ToolError::InvalidArgs(_))),
        "text doesn't rotate"
    );
    ok(&mut a, "content_delete", json!({ "doc": doc, "page": 1, "index": 2 }));
    assert_eq!(ok(&mut a, "content_list", json!({ "doc": doc }))["count"], 1);
    assert!(matches!(a.call("content_delete", &json!({ "doc": doc, "page": 1, "index": 5 })), Err(ToolError::Failed(_))));
}

/// Text styling on an added image is refused (#1331), as image transforms are on text, and the
/// refusal leaves the content, the dirty flag and the undo and redo steps as they were.
#[test]
fn text_styling_on_an_image_is_refused_without_an_edit() {
    let dir = workdir("content-style");
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, 1, 1);
        enc.set_color(png::ColorType::Rgb);
        enc.write_header().unwrap().write_image_data(&[80, 160, 200]).unwrap();
    }
    std::fs::write(dir.join("logo.png"), png).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "page_add_text", json!({ "doc": doc, "page": 1, "text": "sample", "at": [20, 20] }));
    ok(&mut a, "page_add_image", json!({ "doc": doc, "page": 1, "path": "logo.png", "rect": [10, 100, 40, 130] }));
    ok(&mut a, "content_update", json!({ "doc": doc, "page": 1, "index": 2, "rotate": 90 }));
    ok(&mut a, "edit_undo", json!({ "doc": doc }));
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "saved.pdf" }));
    let state = |a: &Automation| {
        let d = a.session().docs().iter().find(|d| d.id.0 == doc).unwrap();
        (format!("{:?}", d.added()), d.dirty, d.edit_generation(), d.can_undo().map(str::to_owned), d.can_redo().map(str::to_owned))
    };
    let before = state(&a);
    assert!(!before.1 && before.4.is_some(), "a clean document with a step to redo: {before:?}");
    for (key, value) in [
        ("font", json!("times")),
        ("size", json!(24)),
        ("bold", json!(true)),
        ("italic", json!(true)),
        ("color", json!("red")),
        ("align", json!("center")),
    ] {
        for rect in [None, Some(json!([20, 100, 50, 130]))] {
            let mut args = json!({ "doc": doc, "page": 1, "index": 2 });
            args[key] = value.clone();
            if let Some(r) = rect {
                args["rect"] = r;
            }
            assert!(matches!(a.call("content_update", &args), Err(ToolError::InvalidArgs(_))), "{args}");
            assert_eq!(state(&a), before, "{args}");
        }
    }
    ok(&mut a, "content_update", json!({ "doc": doc, "page": 1, "index": 1, "font": "times", "size": 24, "align": "center" }));
    assert!(state(&a).1, "text still takes its styling");
}

#[test]
fn form_formats_and_calculations_through_tools() {
    let dir = workdir("formcalc");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    for (name, y) in [("Price", 20), ("Qty", 60), ("Total", 100)] {
        ok(&mut a, "form_add_field", json!({ "doc": doc, "page": 1, "type": "text", "rect": [20, y, 180, y + 22], "name": name }));
    }
    ok(
        &mut a,
        "form_set_props",
        json!({ "doc": doc, "field": "Price", "format": { "type": "number", "decimals": 2, "currency": "$" }, "validate": { "min": 0 } }),
    );
    ok(
        &mut a,
        "form_set_props",
        json!({ "doc": doc, "field": "Total", "format": { "type": "number", "decimals": 2, "currency": "$" }, "calculate": { "notation": "Price * Qty" } }),
    );
    ok(&mut a, "form_fill", json!({ "doc": doc, "values": { "Price": "19.99", "Qty": "3" } }));
    let f = ok(&mut a, "form_fields", json!({ "doc": doc }));
    let total = f["fields"].as_array().unwrap().iter().find(|x| x["name"] == "Total").unwrap().clone();
    assert_eq!((total["value"].as_str(), total["display"].as_str()), (Some("59.97"), Some("$59.97")), "{total}");
    assert_eq!(total["calculate"]["notation"], "Price * Qty");
    assert!(page_text(&mut a, doc)[0].contains("$59.97"));
    let err = a.call("form_fill", &json!({ "doc": doc, "values": { "Price": "-5" } })).unwrap_err();
    assert!(err.to_string().contains("greater than or equal to 0"), "{err}");
    assert!(matches!(
        a.call("form_set_props", &json!({ "doc": doc, "field": "Qty", "format": { "type": "roman" } })),
        Err(ToolError::InvalidArgs(_))
    ));
}

#[test]
fn tab_order_and_field_appearance_through_tools() {
    let dir = workdir("taborder");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    // Added out of reading order: B (top right), A (top left), C (below).
    for (name, x, y) in [("B", 110, 20), ("A", 10, 20), ("C", 10, 60)] {
        ok(&mut a, "form_add_field", json!({ "doc": doc, "page": 1, "type": "text", "rect": [x, y, x + 80, y + 20], "name": name }));
    }
    let r = ok(&mut a, "form_tab_order", json!({ "doc": doc, "order": "row" }));
    assert_eq!(r["tab_order"], json!(["A", "B", "C"]));
    let r = ok(&mut a, "form_tab_order", json!({ "doc": doc, "order": "column" }));
    assert_eq!(r["tab_order"], json!(["A", "C", "B"]));
    let r = ok(&mut a, "form_tab_order", json!({ "doc": doc, "field": "B", "move": "earlier" }));
    assert_eq!(r["tab_order"], json!(["A", "B", "C"]), "ordered manually");
    let r = ok(&mut a, "form_tab_order", json!({ "doc": doc, "order": "column" }));
    assert_eq!(r["tab_order"], json!(["A", "C", "B"]));
    ok(
        &mut a,
        "form_set_props",
        json!({ "doc": doc, "field": "A", "appearance": { "border": "red", "fill": "none", "style": "underline", "font": "courier" } }),
    );
    ok(&mut a, "form_fill", json!({ "doc": doc, "values": { "A": "typed" } }));
    assert!(page_text(&mut a, doc)[0].contains("typed"));
    assert!(matches!(
        a.call("form_set_props", &json!({ "doc": doc, "field": "A", "appearance": { "style": "wavy" } })),
        Err(ToolError::InvalidArgs(_))
    ));
}

#[test]
fn comments_and_form_data_travel_as_xfdf_fdf_and_text() {
    let dir = workdir("xfdf");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "comment_add", json!({ "doc": doc, "page": 2, "type": "note", "at": [30, 30], "contents": "Please review", "author": "Ada" }));
    ok(&mut a, "form_add_field", json!({ "doc": doc, "page": 1, "type": "text", "rect": [20, 20, 180, 42], "name": "City" }));
    ok(&mut a, "form_fill", json!({ "doc": doc, "values": { "City": "Paris" } }));
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "reviewed.pdf" }));
    ok(&mut a, "doc_export_data", json!({ "doc": doc, "path": "all.xfdf" }));
    ok(&mut a, "doc_export_data", json!({ "doc": doc, "path": "data.txt", "what": "fields" }));
    assert_eq!(std::fs::read_to_string(dir.join("data.txt")).unwrap(), "City\nParis\n");
    assert!(matches!(a.call("doc_export_data", &json!({ "doc": doc, "path": "c.csv", "what": "comments" })), Err(ToolError::InvalidArgs(_))));
    // A copy without the comment and with the field empty takes both back.
    ok(&mut a, "comment_delete", json!({ "doc": doc, "page": 2, "index": 1 }));
    ok(&mut a, "form_reset", json!({ "doc": doc }));
    let r = ok(&mut a, "doc_import_data", json!({ "doc": doc, "path": "all.xfdf" }));
    assert_eq!(r["filled_fields"], 1);
    let list = ok(&mut a, "comment_list", json!({ "doc": doc }));
    assert_eq!(list["comments"][0]["contents"], "Please review", "{list}");
    assert_eq!(r["undo"], "Import all.xfdf");
}

#[test]
fn natural_image_stamps_through_tools_on_rotated_pages() {
    for degrees in [0, 90, 180, 270] {
        let dir = workdir(&format!("natural-stamps-{degrees}"));
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 80, 40);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let pixels: Vec<u8> = (0..40)
                .flat_map(|y| {
                    (0..80).flat_map(move |x| {
                        [[240, 20, 20], [20, 180, 20], [20, 20, 240], [230, 180, 20]][usize::from(y >= 20) * 2 + usize::from(x >= 40)]
                    })
                })
                .collect();
            encoder.write_header().unwrap().write_image_data(&pixels).unwrap();
        }
        std::fs::write(dir.join("quadrants.png"), bytes).unwrap();
        let mut a = auto(&dir);
        let doc = ok(&mut a, "doc_open", json!({"path":"a.pdf"}))["doc"].as_u64().unwrap();
        ok(&mut a, "page_rotate", json!({"doc":doc,"pages":[1],"degrees":degrees}));
        ok(&mut a, "stamp_custom", json!({"doc":doc,"page":1,"path":"quadrants.png","at":[100,100]}));
        let rendered = a.call("page_render", &json!({"doc":doc,"page":1,"dpi":72})).unwrap();
        let Content::Png { data, .. } = &rendered[0] else { panic!("expected PNG") };
        let mut reader = png::Decoder::new(std::io::Cursor::new(data)).read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        assert_eq!(info.color_type, png::ColorType::Rgba);
        for ((x, y), colour) in [(80, 90), (120, 90), (80, 110), (120, 110)].into_iter().zip([
            [240, 20, 20, 255],
            [20, 180, 20, 255],
            [20, 20, 240, 255],
            [230, 180, 20, 255],
        ]) {
            let offset = ((y * info.width + x) * 4) as usize;
            assert_eq!(&pixels[offset..offset + 4], &colour, "page rotation {degrees}");
        }
    }
}

#[test]
fn stamps_through_tools() {
    let dir = workdir("stamps");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "comment_add", json!({ "doc": doc, "page": 1, "type": "stamp", "stamp": "approved", "at": [100, 100] }));
    ok(
        &mut a,
        "comment_add",
        json!({ "doc": doc, "page": 1, "type": "stamp", "stamp": "reviewed", "dynamic": true, "at": [100, 200], "author": "Ada" }),
    );
    ok(&mut a, "comment_add", json!({ "doc": doc, "page": 1, "type": "stamp", "stamp": "sign here", "at": [100, 250] }));
    let list = ok(&mut a, "comment_list", json!({ "doc": doc }));
    assert_eq!(list["count"], 3, "{list}");
    let text = page_text(&mut a, doc)[0].clone();
    assert!(text.contains("APPROVED") && text.contains("REVIEWED") && text.contains("By Ada at") && text.contains("SIGN HERE"), "{text}");
    assert!(matches!(
        a.call("comment_add", &json!({ "doc": doc, "page": 1, "type": "stamp", "stamp": "nonsense", "at": [1, 1] })),
        Err(ToolError::InvalidArgs(_))
    ));
    // A custom stamp from another PDF's page.
    ok(&mut a, "stamp_custom", json!({ "doc": doc, "page": 2, "path": "b.pdf", "file_page": 2, "at": [100, 150], "name": "Logo" }));
    let list = ok(&mut a, "comment_list", json!({ "doc": doc }));
    assert_eq!(list["count"], 4, "{list}");
    // Its "Page 2" is drawn over the page's own.
    assert_eq!(page_text(&mut a, doc)[1].matches('2').count(), 2);
    assert_eq!(ok(&mut a, "edit_undo", json!({ "doc": doc }))["undone"], "Add stamp");
    assert!(matches!(a.call("stamp_custom", &json!({ "doc": doc, "page": 1, "path": "nope.png", "at": [1, 1] })), Err(ToolError::Failed(_))));
}

#[test]
fn bookmark_split_with_equal_titles_keeps_every_part() {
    let dir = workdir("organize_dup_titles");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "bookmark_add", json!({ "doc": doc, "title": "Same", "page": 1 }));
    ok(&mut a, "bookmark_add", json!({ "doc": doc, "title": "Same", "page": 2 }));
    ok(&mut a, "bookmark_add", json!({ "doc": doc, "title": "same", "page": 3 }));
    let s = ok(&mut a, "doc_split", json!({ "doc": doc, "bookmarks": true, "out_dir": "parts" }));
    let files: Vec<String> = s["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| std::path::Path::new(f["path"].as_str().unwrap()).file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files, ["a-Same.pdf", "a-Same-2.pdf", "a-same-3.pdf"]);
    for f in &files {
        assert!(dir.join("parts").join(f).exists(), "{f} missing");
    }
}

#[test]
fn organizing_with_filters_bookmark_splits_and_extract_options() {
    let dir = workdir("organize2");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    // Rotate the even page numbers only (a.pdf has 3 pages: page 2 is the only even one).
    let r = ok(&mut a, "page_rotate", json!({ "doc": doc, "degrees": 90, "subset": "even" }));
    assert_eq!(r["rotated"], 1);
    let info = ok(&mut a, "doc_info", json!({ "doc": doc }));
    assert_eq!(info["pages"][1]["rotation"], 90);
    assert_eq!(info["pages"][0]["rotation"], 0);
    // Split at top-level bookmarks: parts named after them.
    ok(&mut a, "bookmark_add", json!({ "doc": doc, "title": "Start", "page": 1 }));
    ok(&mut a, "bookmark_add", json!({ "doc": doc, "title": "End/Part", "page": 3 }));
    let s = ok(&mut a, "doc_split", json!({ "doc": doc, "bookmarks": true, "out_dir": "parts" }));
    let files: Vec<String> = s["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| std::path::Path::new(f["path"].as_str().unwrap()).file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files, ["a-Start.pdf", "a-End_Part.pdf"]);
    let s = ok(&mut a, "doc_split", json!({ "doc": doc, "max_mb": 0.0001, "out_dir": "sized" }));
    assert_eq!(s["files"].as_array().unwrap().len(), 3, "tiny limit: a page per file");
    // Extract as separate files and delete them from the original.
    let e = ok(&mut a, "page_extract", json!({ "doc": doc, "pages": [1, 2], "separate": true, "out_dir": "pages", "delete": true }));
    assert_eq!(e["files"].as_array().unwrap().len(), 2);
    assert_eq!(e["original"]["pages"], 1);
    assert!(matches!(a.call("page_extract", &json!({ "doc": doc, "pages": [1], "separate": true })), Err(ToolError::InvalidArgs(_))));
}

#[test]
fn links_through_tools() {
    let dir = workdir("links");
    std::fs::write(dir.join("notes.txt"), "Docs at https://example.org/docs and www.rust-lang.org").unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_create", json!({ "from": "text", "path": "notes.txt" }))["doc"].as_u64().unwrap();
    let r = ok(&mut a, "links_from_urls", json!({ "doc": doc }));
    assert_eq!(r["created"], json!(["https://example.org/docs", "http://www.rust-lang.org"]));
    ok(&mut a, "link_add", json!({ "doc": doc, "page": 1, "rect": [72, 300, 200, 320], "to_page": 1, "visible": true, "color": "red" }));
    let list = ok(&mut a, "link_list", json!({ "doc": doc }));
    assert_eq!(list["count"], 3);
    let added = list["links"].as_array().unwrap().iter().find(|l| l["to_page"] == 1).unwrap().clone();
    assert_eq!(added["rect"], json!([72.0, 300.0, 200.0, 320.0]));
    ok(&mut a, "link_edit", json!({ "doc": doc, "page": 1, "index": added["index"], "url": "https://pdfcraft.dev" }));
    let list = ok(&mut a, "link_list", json!({ "doc": doc }));
    assert!(list["links"].as_array().unwrap().iter().any(|l| l["url"] == "https://pdfcraft.dev"));
    ok(&mut a, "link_delete", json!({ "doc": doc, "page": 1, "index": added["index"] }));
    let r = ok(&mut a, "links_remove", json!({ "doc": doc, "pages": [1] }));
    assert_eq!(r["removed"], 2);
    assert_eq!(ok(&mut a, "link_list", json!({ "doc": doc }))["count"], 0);
    // Add one more link and test links_remove across all pages
    ok(&mut a, "link_add", json!({ "doc": doc, "page": 1, "rect": [72, 300, 200, 320], "url": "https://example.org" }));
    let r2 = ok(&mut a, "links_remove", json!({ "doc": doc }));
    assert_eq!(r2["removed"], 1);
    assert_eq!(ok(&mut a, "link_list", json!({ "doc": doc }))["count"], 0);
    assert!(matches!(a.call("link_add", &json!({ "doc": doc, "page": 1, "rect": [0, 0, 50, 20] })), Err(ToolError::InvalidArgs(_))));
}

#[test]
fn local_gotor_link_open_refuses_directories_devices_and_non_pdf_targets() {
    let (_base, root) = sandbox("gotor-not-a-file");
    std::fs::create_dir_all(root.join("folder.pdf")).unwrap();
    std::fs::write(root.join("notes.txt"), fixture(1)).unwrap();
    std::fs::write(root.join("dir-source.pdf"), remote_link_pdf("folder.pdf", None)).unwrap();
    std::fs::write(root.join("txt-source.pdf"), remote_link_pdf("notes.txt", None)).unwrap();
    // Only Unix adds a case (a FIFO), so elsewhere the list is never changed.
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut cases = vec![("dir-source.pdf", "not a regular file"), ("txt-source.pdf", "not a .pdf file")];
    #[cfg(unix)]
    {
        // A FIFO inside the root would block a plain read forever.
        if std::process::Command::new("mkfifo").arg(root.join("pipe.pdf")).status().is_ok_and(|s| s.success()) {
            std::fs::write(root.join("fifo-source.pdf"), remote_link_pdf("pipe.pdf", None)).unwrap();
            cases.push(("fifo-source.pdf", "not a regular file"));
        }
    }
    let mut a = auto(&root);
    for (source_path, reason) in cases {
        let source = ok(&mut a, "doc_open", json!({ "path": source_path }))["doc"].as_u64().unwrap();
        let documents_before = a.session().docs().len();
        let e = err(&mut a, "link_open", json!({ "doc": source, "page": 1, "index": 1 }));
        assert!(matches!(&e, ToolError::Failed(m) if m.contains(reason)), "{source_path}: {e:?}");
        assert_eq!(a.session().docs().len(), documents_before, "a refused target is not opened: {source_path}");
    }
}

#[test]
fn local_gotor_link_open_applies_destinations_and_honors_root() {
    let (base, root) = sandbox("gotor-open");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("target.pdf"), fixture(3)).unwrap();
    std::fs::write(root.join("a.pdf"), fixture(1)).unwrap();
    std::fs::write(root.join("sub/source.pdf"), remote_link_pdf("../target.pdf", Some("[2 /FitH 180]"))).unwrap();
    std::fs::write(root.join("sub/escape.pdf"), remote_link_pdf("../../outside/secret.pdf", Some("[0 /Fit]"))).unwrap();
    std::fs::write(root.join("named.pdf"), remote_link_pdf("target.pdf", Some("(chapter)"))).unwrap();
    std::fs::write(root.join("range.pdf"), remote_link_pdf("target.pdf", Some("[99 /Fit]"))).unwrap();
    std::fs::write(root.join("url.pdf"), remote_link_pdf("https://example.org/target.pdf", Some("[0 /Fit]"))).unwrap();
    std::fs::write(root.join("missing-source.pdf"), remote_link_pdf("does-not-exist.pdf", Some("[0 /Fit]"))).unwrap();
    std::fs::write(root.join("not-pdf.pdf"), b"not a PDF").unwrap();
    std::fs::write(root.join("non-pdf-source.pdf"), remote_link_pdf("not-pdf.pdf", Some("[0 /Fit]"))).unwrap();
    std::fs::write(root.join("encrypted.pdf"), protected_fixture("secret")).unwrap();
    std::fs::write(root.join("password-source.pdf"), remote_link_pdf("encrypted.pdf", Some("[0 /Fit]"))).unwrap();
    std::fs::write(root.join("absolute.pdf"), remote_link_pdf(root.join("target.pdf").to_str().unwrap(), Some("[1 /Fit]"))).unwrap();
    link_dir(&root, "outside-link", &base.join("outside"));
    std::fs::write(root.join("symlink.pdf"), remote_link_pdf("outside-link/secret.pdf", Some("[0 /Fit]"))).unwrap();

    let mut a = auto(&root);
    let source = ok(&mut a, "doc_open", json!({ "path": "sub/source.pdf" }))["doc"].as_u64().unwrap();
    let listed = ok(&mut a, "link_list", json!({ "doc": source }));
    assert_eq!(listed["links"][0]["other"], "GoToR");
    let opened = ok(&mut a, "link_open", json!({ "doc": source, "page": 1, "index": 1 }));
    assert_eq!(opened["path"], "target.pdf");
    assert_eq!(opened["destination_applied"], true);
    assert_eq!(opened["destination"]["page"], 3);
    assert_eq!(opened["destination"]["view"]["kind"], "fit_h");
    assert_eq!(opened["document"]["pages"], 3);
    let target_id = opened["document"]["doc"].as_u64().unwrap();
    assert_eq!(a.session().get(pdfcraft_engine::DocId(target_id)).unwrap().info.pages.len(), 3);

    let absolute = ok(&mut a, "doc_open", json!({ "path": "absolute.pdf" }))["doc"].as_u64().unwrap();
    let before = a.session().docs().len();
    for args in [json!({ "doc": absolute, "page": 1, "index": 1 }), json!({ "doc": absolute, "page": 1, "index": 1, "allow_absolute": false })] {
        assert!(matches!(err(&mut a, "link_open", args), ToolError::Failed(m) if m.contains("allow_absolute")));
        assert_eq!(a.session().docs().len(), before);
    }
    let opened = ok(&mut a, "link_open", json!({ "doc": absolute, "page": 1, "index": 1, "allow_absolute": true }));
    assert_eq!(opened["path"], "target.pdf");
    assert_eq!(opened["destination"]["page"], 2);

    for (source_path, warning) in [("named.pdf", "named destination"), ("range.pdf", "out of range")] {
        let source = ok(&mut a, "doc_open", json!({ "path": source_path }))["doc"].as_u64().unwrap();
        let opened = ok(&mut a, "link_open", json!({ "doc": source, "page": 1, "index": 1 }));
        assert_eq!(opened["destination_applied"], false);
        assert!(opened["warning"].as_str().unwrap().contains(warning), "{opened}");
    }

    let password_source = ok(&mut a, "doc_open", json!({ "path": "password-source.pdf" }))["doc"].as_u64().unwrap();
    let documents_before = a.session().docs().len();
    assert!(matches!(err(&mut a, "link_open", json!({ "doc": password_source, "page": 1, "index": 1 })), ToolError::Failed(_)));
    assert_eq!(a.session().docs().len(), documents_before);
    assert!(matches!(err(&mut a, "link_open", json!({ "doc": password_source, "page": 1, "index": 1, "password": "wrong" })), ToolError::Failed(_)));
    assert_eq!(a.session().docs().len(), documents_before);
    let opened = ok(&mut a, "link_open", json!({ "doc": password_source, "page": 1, "index": 1, "password": "secret" }));
    assert_eq!(opened["document"]["encrypted"], true);

    for source_path in ["url.pdf", "missing-source.pdf", "non-pdf-source.pdf"] {
        let source = ok(&mut a, "doc_open", json!({ "path": source_path }))["doc"].as_u64().unwrap();
        let documents_before = a.session().docs().len();
        assert!(matches!(err(&mut a, "link_open", json!({ "doc": source, "page": 1, "index": 1 })), ToolError::Failed(_)));
        assert_eq!(a.session().docs().len(), documents_before, "a rejected target is not opened: {source_path}");
    }

    for source_path in ["sub/escape.pdf", "symlink.pdf"] {
        let source = ok(&mut a, "doc_open", json!({ "path": source_path }))["doc"].as_u64().unwrap();
        let documents_before = a.session().docs().len();
        assert!(matches!(err(&mut a, "link_open", json!({ "doc": source, "page": 1, "index": 1 })), ToolError::Failed(m) if m.contains("outside")));
        assert_eq!(a.session().docs().len(), documents_before, "a refused target is not opened");
    }

    let missing_source = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "link_add", json!({ "doc": missing_source, "page": 1, "rect": [20, 30, 80, 50], "url": "https://example.org" }));
    assert!(
        matches!(err(&mut a, "link_open", json!({ "doc": missing_source, "page": 1, "index": 1 })), ToolError::InvalidArgs(m) if m.contains("not a supported local PDF GoToR"))
    );
}

/// doc_info reports annotation and link rectangles in the tools' displayed-page convention,
/// the same values comment_list and link_list give, so they can be fed back to link_edit (#129).
#[test]
fn doc_info_rects_are_displayed_page_coordinates() {
    let dir = workdir("info-rects");
    let mut a = auto(&dir);
    // Page 2 is rotated so an unrotated-only y flip cannot pass.
    let doc = ok(&mut a, "doc_create", json!({ "from": "blank", "width": 612, "height": 792, "pages": 2 }))["doc"].as_u64().unwrap();
    ok(&mut a, "page_rotate", json!({ "doc": doc, "pages": [2], "degrees": 90 }));
    for page in 1..=2 {
        ok(
            &mut a,
            "comment_add",
            json!({ "doc": doc, "page": page, "type": "note", "at": [72, 72], "author": "Example", "contents": "Fixture note" }),
        );
        ok(&mut a, "link_add", json!({ "doc": doc, "page": page, "rect": [72, 100, 200, 120], "url": "https://example.com" }));
    }
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "fixture.pdf" }));
    let re = ok(&mut a, "doc_open", json!({ "path": "fixture.pdf" }))["doc"].as_u64().unwrap();
    let info = ok(&mut a, "doc_info", json!({ "doc": re }));
    let comments = ok(&mut a, "comment_list", json!({ "doc": re }));
    let links = ok(&mut a, "link_list", json!({ "doc": re }));
    let close = |a: &Value, b: &Value| {
        let (a, b) = (a.as_array().unwrap(), b.as_array().unwrap());
        a.len() == 4 && a.iter().zip(b).all(|(x, y)| (x.as_f64().unwrap() - y.as_f64().unwrap()).abs() < 0.01)
    };
    for page in 1..=2 {
        let page = json!(page);
        let find = |items: &Value| items.as_array().unwrap().iter().find(|x| x["page"] == page).unwrap()["rect"].clone();
        let (info_note, note) = (find(&info["annotations"]), find(&comments["comments"]));
        assert!(close(&info_note, &note), "page {page}: doc_info note {info_note} vs comment_list {note}");
        if page == 1 {
            assert!(close(&note, &json!([72.0, 72.0, 92.0, 92.0])), "note {note}");
        }
        let (info_link, link) = (find(&info["links"]), find(&links["links"]));
        assert!(close(&info_link, &link), "page {page}: doc_info link {info_link} vs link_list {link}");
        assert!(close(&link, &json!([72.0, 100.0, 200.0, 120.0])), "page {page}: link {link}");
    }
    // Reusing the doc_info rectangle in a geometry-taking edit leaves the link where it is.
    let rotated = links["links"].as_array().unwrap().iter().find(|l| l["page"] == 2).unwrap().clone();
    let from_info = info["links"].as_array().unwrap().iter().find(|l| l["page"] == 2).unwrap()["rect"].clone();
    ok(&mut a, "link_edit", json!({ "doc": re, "page": 2, "index": rotated["index"], "rect": from_info }));
    let after = ok(&mut a, "link_list", json!({ "doc": re }));
    let moved = after["links"].as_array().unwrap().iter().find(|l| l["page"] == 2).unwrap()["rect"].clone();
    assert!(close(&moved, &rotated["rect"]), "link moved: {moved} vs {}", rotated["rect"]);
}

/// `comment_add` places a note's or an attachment's icon with its displayed top-left corner at
/// `at`, on rotated pages too. The engine anchors the icon at the user-space top-left of its
/// `/Rect`, which after `/Rotate` is another corner of the square as displayed; converting the
/// point alone put the icon one icon-width off.
#[test]
fn note_icons_anchor_at_the_requested_corner_on_rotated_pages() {
    let dir = workdir("note-anchor");
    std::fs::write(dir.join("note.txt"), b"attached").unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_create", json!({ "from": "blank", "width": 612, "height": 792, "pages": 4 }))["doc"].as_u64().unwrap();
    for (page, degrees) in [(2, 90), (3, 180), (4, 270)] {
        ok(&mut a, "page_rotate", json!({ "doc": doc, "pages": [page], "degrees": degrees }));
    }
    for page in 1..=4 {
        ok(&mut a, "comment_add", json!({ "doc": doc, "page": page, "type": "note", "at": [72, 72], "contents": "Fixture note" }));
        ok(&mut a, "comment_add", json!({ "doc": doc, "page": page, "type": "attachment", "at": [200, 300], "path": "note.txt" }));
    }
    let comments = ok(&mut a, "comment_list", json!({ "doc": doc }));
    let comments = comments["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 8);
    for c in comments {
        let want = if c["type"] == "Text" { [72.0, 72.0, 92.0, 92.0] } else { [200.0, 300.0, 220.0, 320.0] };
        let rect: Vec<f64> = c["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        assert!(rect.iter().zip(want).all(|(x, y)| (x - y).abs() < 0.01), "page {} {}: rect {rect:?}, want {want:?}", c["page"], c["type"]);
    }
}

/// An image signature is drawn upright as displayed, and anchored at the displayed point asked
/// for, on every `/Rotate`: its picture is placed in user space, which the page turns, so the
/// appearance is counter-rotated and its box is found in user space.
#[test]
fn image_signatures_stay_upright_on_rotated_pages() {
    let dir = workdir("image-upright");
    // 80 x 40 px, one colour per quadrant: red and green above, blue and yellow below.
    let quadrants = image::RgbaImage::from_fn(80, 40, |x, y| match (x < 40, y < 20) {
        (true, true) => image::Rgba([255, 0, 0, 255]),
        (false, true) => image::Rgba([0, 255, 0, 255]),
        (true, false) => image::Rgba([0, 0, 255, 255]),
        (false, false) => image::Rgba([255, 255, 0, 255]),
    });
    quadrants.save(dir.join("quadrants.png")).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_create", json!({ "from": "blank", "width": 200, "height": 300, "pages": 4 }))["doc"].as_u64().unwrap();
    for (page, degrees) in [(2, 90), (3, 180), (4, 270)] {
        ok(&mut a, "page_rotate", json!({ "doc": doc, "pages": [page], "degrees": degrees }));
    }
    for page in 1..=4 {
        // 64 x 32 pt from x = 60 as displayed, centred on y = 140.
        ok(&mut a, "fill_sign_add", json!({ "doc": doc, "page": page, "type": "signature", "at": [60, 140], "path": "quadrants.png" }));
    }
    let (white, red, green, blue, yellow) = ([255, 255, 255], [255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 0]);
    let check = |a: &mut Automation, doc: u64| {
        for page in 1..=4 {
            let output = a.call("page_render", &json!({ "doc": doc, "page": page, "dpi": 72 })).unwrap();
            let Content::Png { data, .. } = &output[0] else { panic!() };
            let pixels = image::load_from_memory(data).unwrap().to_rgba8();
            for (x, y, want, what) in [
                (76, 132, red, "signature top-left"),
                (108, 132, green, "signature top-right"),
                (76, 148, blue, "signature bottom-left"),
                (108, 148, yellow, "signature bottom-right"),
                (56, 140, white, "left of the signature"),
                (128, 140, white, "right of the signature"),
                (92, 120, white, "above the signature"),
                (92, 160, white, "below the signature"),
            ] {
                let got = pixels.get_pixel(x, y).0;
                assert!(got[..3].iter().zip(want).all(|(g, w)| g.abs_diff(w) < 12), "page {page}, {what} at ({x}, {y}): {got:?}, want {want:?}");
            }
        }
    };
    check(&mut a, doc);
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "placed.pdf" }));
    let reopened = ok(&mut a, "doc_open", json!({ "path": "placed.pdf" }))["doc"].as_u64().unwrap();
    check(&mut a, reopened);
}

/// Issue #299: a typed signature and initials read across, as displayed, on every `/Rotate`,
/// anchored at the displayed point, and look the same as on an unturned page.
#[test]
fn typed_signatures_stay_upright_on_rotated_pages() {
    let dir = workdir("typed-upright");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_create", json!({ "from": "blank", "width": 200, "height": 300, "pages": 4 }))["doc"].as_u64().unwrap();
    for (page, degrees) in [(2, 90), (3, 180), (4, 270)] {
        ok(&mut a, "page_rotate", json!({ "doc": doc, "pages": [page], "degrees": degrees }));
    }
    for page in 1..=4 {
        ok(&mut a, "fill_sign_add", json!({ "doc": doc, "page": page, "type": "signature", "at": [30, 60], "text": "Ada Lovelace" }));
        ok(&mut a, "fill_sign_add", json!({ "doc": doc, "page": page, "type": "initials", "at": [30, 140], "text": "AL" }));
    }
    let check = |a: &mut Automation, doc: u64| {
        let mut upright = Vec::new();
        for page in 1..=4 {
            let output = a.call("page_render", &json!({ "doc": doc, "page": page, "dpi": 72 })).unwrap();
            let Content::Png { data, .. } = &output[0] else { panic!() };
            let pixels = image::load_from_memory(data).unwrap().to_rgba8();
            assert_eq!(pixels.dimensions(), if page % 2 == 1 { (200, 300) } else { (300, 200) }, "page {page}");
            let dark: Vec<bool> = pixels.pixels().map(|p| p.0[..3].iter().all(|v| *v < 128)).collect();
            for y in [60, 140] {
                let mut b = [u32::MAX, u32::MAX, 0, 0];
                for (x, py, p) in pixels.enumerate_pixels() {
                    if py.abs_diff(y) < 40 && p.0[..3].iter().all(|v| *v < 128) {
                        b = [b[0].min(x), b[1].min(py), b[2].max(x), b[3].max(py)];
                    }
                }
                assert!(b[0] <= b[2], "page {page}: no ink near y = {y}");
                assert!(b[2] - b[0] > b[3] - b[1], "page {page}: the name reads across: {b:?}");
                assert!(b[0].abs_diff(30) <= 2 && b[1] < y && b[3] > y, "page {page}: anchored at (30, {y}): {b:?}");
            }
            if page == 1 {
                upright = dark;
                continue;
            }
            // Same pixels as the unturned page where both pages are (pages 2 and 4 are wider).
            let width = pixels.width();
            let (mut differ, mut ink) = (0, 0);
            for y in 0..200 {
                for x in 0..200 {
                    let (want, got) = (upright[(y * 200 + x) as usize], dark[(y * width + x) as usize]);
                    ink += usize::from(want);
                    differ += usize::from(want != got);
                }
            }
            assert!(differ * 10 < ink, "page {page}: {differ} of {ink} ink pixels differ from the unturned page");
        }
    };
    check(&mut a, doc);
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "placed.pdf" }));
    let reopened = ok(&mut a, "doc_open", json!({ "path": "placed.pdf" }))["doc"].as_u64().unwrap();
    check(&mut a, reopened);
}

#[test]
fn comment_checkmarks_locks_hiding_and_summaries_through_tools() {
    let dir = workdir("comment-polish");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let c = ok(&mut a, "comment_add", json!({ "doc": doc, "page": 1, "type": "note", "at": [20, 20], "contents": "Sticky", "author": "Ada" }));
    let id = c["comment"]["id"].as_str().unwrap().to_string();
    ok(&mut a, "comment_set_status", json!({ "doc": doc, "id": id, "status": "accepted" }));
    ok(&mut a, "comment_mark", json!({ "doc": doc, "id": id }));
    ok(&mut a, "comment_lock", json!({ "doc": doc, "id": id }));
    let list = ok(&mut a, "comment_list", json!({ "doc": doc }));
    assert_eq!(list["count"], 1, "{list}");
    let c = &list["comments"][0];
    assert_eq!((c["status"].as_str(), c["marked"].as_bool(), c["locked"].as_bool()), (Some("Accepted"), Some(true), Some(true)));
    assert!(matches!(a.call("comment_delete", &json!({ "doc": doc, "id": id })), Err(ToolError::Failed(_))), "locked");
    ok(&mut a, "comment_mark", json!({ "doc": doc, "id": id, "marked": false }));
    ok(&mut a, "comment_lock", json!({ "doc": doc, "id": id, "locked": false }));
    let list = ok(&mut a, "comment_list", json!({ "doc": doc }));
    assert_eq!((list["comments"][0]["marked"].as_bool(), list["comments"][0]["locked"].as_bool()), (Some(false), Some(false)));

    assert_eq!(ok(&mut a, "comments_hide", json!({ "doc": doc }))["hidden"], true);
    ok(&mut a, "comments_hide", json!({ "doc": doc, "hidden": false }));

    let r = ok(&mut a, "comments_summarize", json!({ "doc": doc, "sort": "author", "out": "summary.pdf" }));
    assert!(r["bytes"].as_u64().unwrap() > 500);
    let text = ok(&mut a, "doc_open", json!({ "path": "summary.pdf" }))["doc"].as_u64().unwrap();
    let found = ok(&mut a, "text_find", json!({ "doc": text, "query": "Sticky" }));
    assert!(found["count"].as_u64().unwrap() >= 1, "{found}");
    assert!(matches!(a.call("comments_summarize", &json!({ "doc": doc, "sort": "colour" })), Err(ToolError::InvalidArgs(_))));
}

/// Text the standard fonts can't draw is never written as `?` silently, through the tools. A form
/// value, a button caption, a text box and Fill & Sign text are kept as typed (as Acrobat keeps
/// them) and the result's `warnings` names the character; a form also asks other viewers to
/// redraw it. Where the `?` would be permanent the tool refuses: a typed signature, a redaction's
/// overlay text, a new document from text, and flattening what PdfCraft drew with `?`. A comment
/// summary that quotes such text writes it as [U+XXXX].
#[test]
fn text_the_standard_fonts_cannot_draw_is_never_silent_through_tools() {
    let dir = workdir("undrawable");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let refused = |a: &mut Automation, tool: &str, args: Value, ch: &str| match a.call(tool, &args) {
        Err(ToolError::Failed(e) | ToolError::InvalidArgs(e)) => assert!(e.contains(ch), "{tool}: {e}"),
        other => panic!("{tool}: expected a refusal, got {other:?}"),
    };
    let warned = |out: &Value, parts: &[&str]| {
        let w = out["warnings"].as_array().unwrap_or_else(|| panic!("no warnings: {out}"));
        assert!(w.iter().filter_map(Value::as_str).any(|m| parts.iter().all(|p| m.contains(p))), "{parts:?} not in {w:?}");
    };
    // Forms: a value is kept and the form asks other viewers to redraw it; a button caption too.
    ok(&mut a, "form_add_field", json!({ "doc": doc, "page": 1, "type": "text", "rect": [20, 20, 180, 42], "name": "who" }));
    let out = ok(&mut a, "form_fill", json!({ "doc": doc, "values": { "who": "Ada" } }));
    assert!(out.get("warnings").is_none(), "{out}");
    let out = ok(&mut a, "form_fill", json!({ "doc": doc, "values": { "who": "张三" } }));
    warned(&out, &["form field \"who\"", "\"张\" (U+5F20)", "other PDF viewers redraw the field"]);
    let fields = ok(&mut a, "form_fields", json!({ "doc": doc }));
    assert_eq!(fields["fields"][0]["value"], "张三", "{fields}");
    let out = ok(&mut a, "form_add_field", json!({ "doc": doc, "page": 1, "type": "button", "rect": [20, 60, 120, 82], "caption": "提交" }));
    warned(&out, &["\"提\" (U+63D0)"]);
    // Comments and Fill & Sign text: kept, and said.
    let out = ok(&mut a, "comment_add", json!({ "doc": doc, "page": 1, "type": "textbox", "rect": [10, 200, 190, 240], "contents": "欢迎" }));
    warned(&out, &["a comment on page 1", "\"欢\" (U+6B22)", "every viewer"]);
    let out = ok(&mut a, "fill_sign_add", json!({ "doc": doc, "page": 1, "type": "text", "at": [20, 300], "text": "签名" }));
    warned(&out, &["U+7B7E"]);
    // A sticky note draws an icon, not its text: nothing to say.
    let out = ok(&mut a, "comment_add", json!({ "doc": doc, "page": 1, "type": "note", "at": [40, 40], "contents": "签名 OK", "author": "Ada" }));
    assert!(out.get("warnings").is_none(), "{out}");
    // Permanent `?`: refused.
    refused(&mut a, "fill_sign_add", json!({ "doc": doc, "page": 1, "type": "signature", "at": [20, 340], "text": "张三" }), "pass `path`");
    // Cyrillic comes from the script font's Inter fallback (#963), so it isn't refused.
    ok(&mut a, "fill_sign_add", json!({ "doc": doc, "page": 1, "type": "signature", "at": [20, 380], "text": "Саша" }));
    refused(&mut a, "redact_mark", json!({ "doc": doc, "page": 1, "rect": [10, 400, 190, 420], "overlay": "机密" }), "\"机\" (U+673A)");
    refused(&mut a, "doc_flatten", json!({ "doc": doc }), "nothing was flattened");
    refused(&mut a, "doc_create", json!({ "from": "text", "text": "Notes\n第二行" }), "(line 2)");
    // A summary quotes the note: escaped, not refused, not `?`.
    ok(&mut a, "comments_summarize", json!({ "doc": doc, "out": "summary.pdf" }));
    let summary = ok(&mut a, "doc_open", json!({ "path": "summary.pdf" }))["doc"].as_u64().unwrap();
    let text = page_text(&mut a, summary).join("\n");
    assert!(text.contains("[U+7B7E][U+540D] OK") && text.contains("[U+XXXX]"), "{text}");
    assert!(!text.contains("?? OK"), "{text}");
}

/// #1908: `comment_edit font_size` resizes the text of a text box, in one undo step, and refuses
/// sizes outside 1-400 pt and comments without text.
#[test]
fn text_box_font_size_through_the_tool() {
    let dir = workdir("fontsize");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "comment_add", json!({ "doc": doc, "page": 1, "type": "textbox", "rect": [20, 40, 200, 80], "contents": "Hello" }));
    ok(&mut a, "comment_add", json!({ "doc": doc, "page": 1, "type": "rectangle", "rect": [20, 120, 80, 160] }));
    ok(&mut a, "comment_edit", json!({ "doc": doc, "page": 1, "index": 1, "font_size": 30 }));
    for bad in [0, 401] {
        assert!(matches!(a.call("comment_edit", &json!({ "doc": doc, "page": 1, "index": 1, "font_size": bad })), Err(ToolError::InvalidArgs(_))));
    }
    assert!(a.call("comment_edit", &json!({ "doc": doc, "page": 1, "index": 2, "font_size": 12 })).is_err());
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "sized.pdf" }));
    let saved = pdfcraft_cos::Document::open(std::sync::Arc::new(std::fs::read(dir.join("sized.pdf")).unwrap())).unwrap();
    let sizes: Vec<String> = saved
        .object_numbers()
        .iter()
        .filter_map(|n| saved.try_get(*n).ok())
        .filter_map(|o| o.as_dict().and_then(|d| d.get(b"DA")?.as_string().map(|s| String::from_utf8_lossy(&s.bytes).into_owned())))
        .collect();
    assert_eq!(sizes.len(), 1, "{sizes:?}");
    assert!(sizes[0].ends_with("/Helv 30 Tf"), "{sizes:?}");
    assert_eq!(ok(&mut a, "edit_undo", json!({ "doc": doc }))["undone"], "Change comment properties");
}

#[test]
fn line_endings_through_the_tool() {
    let dir = workdir("endings");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "comment_add", json!({ "doc": doc, "page": 1, "type": "line", "from": [20, 40], "to": [120, 80], "endings": ["None", "Diamond"] }));
    ok(
        &mut a,
        "comment_add",
        json!({ "doc": doc, "page": 1, "type": "polyline", "points": [[20, 120], [60, 100], [100, 140]], "endings": ["Circle", "Slash"] }),
    );
    ok(
        &mut a,
        "comment_add",
        json!({ "doc": doc, "page": 1, "type": "callout", "rect": [110, 200, 190, 240], "to": [40, 160], "contents": "Look", "endings": ["Square"] }),
    );
    assert!(matches!(
        a.call("comment_add", &json!({ "doc": doc, "page": 1, "type": "line", "from": [20, 40], "to": [120, 80], "endings": ["Sparkle", "None"] })),
        Err(ToolError::InvalidArgs(_))
    ));
    assert!(matches!(
        a.call(
            "comment_add",
            &json!({ "doc": doc, "page": 1, "type": "callout", "rect": [10, 10, 80, 40], "to": [90, 80], "endings": ["None", "None"] })
        ),
        Err(ToolError::InvalidArgs(_))
    ));
    assert!(matches!(
        a.call("comment_add", &json!({ "doc": doc, "page": 1, "type": "rectangle", "rect": [10, 10, 40, 40], "endings": ["Circle"] })),
        Err(ToolError::InvalidArgs(_))
    ));
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "ended.pdf" }));
    let saved = pdfcraft_cos::Document::open(std::sync::Arc::new(std::fs::read(dir.join("ended.pdf")).unwrap())).unwrap();
    for name in [b"Diamond".as_slice(), b"Circle", b"Slash", b"Square"] {
        assert!(
            saved.object_numbers().iter().any(|n| saved.try_get(*n).ok().is_some_and(|o| object_has_name(&o, name))),
            "missing /{}",
            String::from_utf8_lossy(name)
        );
    }
    let drawn: Vec<String> = saved
        .object_numbers()
        .into_iter()
        .filter_map(|n| {
            let obj = saved.try_get(n).ok()?;
            let pdfcraft_cos::Object::Stream(s) = &*obj else { return None };
            if s.dict.name(b"Subtype") != Some(b"Form") {
                return None;
            }
            s.decoded().ok().map(|b| String::from_utf8_lossy(&b).into_owned())
        })
        .collect();
    assert!(drawn.iter().any(|s| s.contains("h B")), "a filled ending was drawn: {drawn:?}");
    assert!(drawn.iter().any(|s| s.contains(" l S")), "an open ending was drawn: {drawn:?}");
}

fn object_has_name(obj: &pdfcraft_cos::Object, name: &[u8]) -> bool {
    match obj {
        pdfcraft_cos::Object::Name(n) => n.as_slice() == name,
        pdfcraft_cos::Object::Array(items) => items.iter().any(|o| object_has_name(o, name)),
        pdfcraft_cos::Object::Dict(d) => d.iter().any(|(_, o)| object_has_name(o, name)),
        pdfcraft_cos::Object::Stream(s) => s.dict.iter().any(|(_, o)| object_has_name(o, name)),
        _ => false,
    }
}

#[test]
fn drawing_comments_through_tools() {
    let dir = workdir("drawing");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    for (ty, extra) in [
        ("polygon", json!({ "points": [[20, 20], [80, 20], [50, 70]] })),
        ("cloud", json!({ "points": [[100, 20], [180, 20], [180, 80], [100, 80]], "color": "red" })),
        ("polyline", json!({ "points": [[20, 120], [60, 100], [100, 120]] })),
        ("callout", json!({ "rect": [110, 200, 190, 240], "to": [40, 160], "contents": "Look" })),
        ("caret", json!({ "at": [60, 150], "contents": "insert this" })),
    ] {
        let mut args = json!({ "doc": doc, "page": 1, "type": ty });
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        ok(&mut a, "comment_add", args);
    }
    let list = ok(&mut a, "comment_list", json!({ "doc": doc }));
    let types: Vec<&str> = list["comments"].as_array().unwrap().iter().map(|c| c["type"].as_str().unwrap()).collect();
    for t in ["Polygon", "PolyLine", "FreeText", "Caret"] {
        assert!(types.contains(&t), "{types:?}");
    }
    assert_eq!(types.iter().filter(|t| **t == "Polygon").count(), 2);
    let callout = list["comments"].as_array().unwrap().iter().find(|c| c["type"] == "FreeText").unwrap();
    let r: Vec<f64> = callout["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert!(r[0] < 40.0 && r[3] > 159.0, "the rect holds the leader line: {r:?}");
    assert!(matches!(
        a.call("comment_add", &json!({ "doc": doc, "page": 1, "type": "polygon", "points": [[1, 1]] })),
        Err(ToolError::Failed(_) | ToolError::InvalidArgs(_))
    ));
    // Replace Text: a strikeout over the found text and a grouped caret.
    ok(&mut a, "comment_add", json!({ "doc": doc, "page": 2, "type": "replace", "find": "page 2", "contents": "second page" }));
    let list = ok(&mut a, "comment_list", json!({ "doc": doc, "page": 2 }));
    let strike = list["comments"].as_array().unwrap().iter().find(|c| c["type"] == "StrikeOut").unwrap().clone();
    assert_eq!(strike["replies"][0]["contents"], "second page", "the caret threads under the strikeout");
    assert!(matches!(a.call("comment_add", &json!({ "doc": doc, "page": 2, "type": "replace", "find": "page 2" })), Err(ToolError::InvalidArgs(_))));
    // A file attached as a comment.
    std::fs::write(dir.join("notes.txt"), b"remember").unwrap();
    ok(&mut a, "comment_add", json!({ "doc": doc, "page": 1, "type": "attachment", "path": "notes.txt", "at": [250, 20], "icon": "Paperclip" }));
    let info = ok(&mut a, "doc_info", json!({ "doc": doc }));
    assert!(info.to_string().contains("notes.txt"), "{info}");
    let png = a.call("page_render", &json!({ "doc": doc, "page": 1, "dpi": 72 })).unwrap();
    assert!(matches!(png[0], Content::Png { .. }));
}

#[test]
fn signing_with_an_image_typed_or_drawn_graphic_through_tools() {
    let dir = workdir("signing-graphic");
    // A solid red 120 x 40 picture.
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, 120, 40);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let rgba: Vec<u8> = (0..120 * 40).flat_map(|_| [220, 0, 0, 255]).collect();
        encoder.write_header().unwrap().write_image_data(&rgba).unwrap();
    }
    std::fs::write(dir.join("logo.png"), png).unwrap();
    std::fs::write(dir.join("broken.png"), b"broken").unwrap();
    let mut a = auto(&dir);
    ok(&mut a, "sign_id_create", json!({ "name": "Ada Lovelace", "key": "p256", "password": "secret12", "path": "ada.p12" }));
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let sign = |a: &mut Automation, rect: [f64; 4], graphic: Value, out: &str| {
        a.call(
            "sign_document",
            &json!({ "doc": doc, "id": "ada.p12", "password": "secret12", "page": 1, "rect": rect, "graphic": graphic, "out": out }),
        )
    };
    for (graphic, why) in [
        (json!({ "image": "logo.png", "text": "Ada" }), "two kinds"),
        (json!({}), "no kind"),
        (json!({ "picture": "logo.png" }), "unknown key"),
        (json!("logo.png"), "not an object"),
        (json!({ "image": "broken.png" }), "not an image"),
        (json!({ "text": "   " }), "nothing to draw"),
        (json!({ "text": "张三" }), "a name the script font and its fallback can't draw"),
        (json!({ "strokes": [[[0, 0], ["x", 1]]] }), "malformed strokes"),
        (json!({ "strokes": [] }), "empty strokes"),
    ] {
        let r = sign(&mut a, [20.0, 200.0, 180.0, 250.0], graphic, "bad.pdf");
        assert!(matches!(r, Err(ToolError::InvalidArgs(_))), "{why}: {r:?}");
    }
    // A missing file fails as it does for every tool that reads one.
    assert!(matches!(sign(&mut a, [20.0, 200.0, 180.0, 250.0], json!({ "image": "missing.png" }), "bad.pdf"), Err(ToolError::Failed(_))));
    // An invisible signature shows nothing, so a graphic for one is a mistake.
    let r = a.call("sign_document", &json!({ "doc": doc, "id": "ada.p12", "password": "secret12", "graphic": { "text": "Ada" }, "out": "bad.pdf" }));
    assert!(matches!(r, Err(ToolError::InvalidArgs(_))), "{r:?}");
    assert!(!dir.join("bad.pdf").exists());

    // Image, typed and drawn graphics, each a signature of its own, all intact.
    sign(&mut a, [20.0, 200.0, 180.0, 250.0], json!({ "image": "logo.png" }), "s1.pdf").unwrap();
    sign(&mut a, [20.0, 130.0, 180.0, 180.0], json!({ "text": "Ada Lovelace" }), "s2.pdf").unwrap();
    sign(&mut a, [20.0, 60.0, 180.0, 110.0], json!({ "strokes": [[[0, 0], [0.3, 0.2], [0.6, 0], [1, 0.2]]] }), "s3.pdf").unwrap();
    let list = ok(&mut a, "sign_list", json!({ "doc": doc }));
    assert_eq!(list["count"], 3);
    for s in list["signatures"].as_array().unwrap() {
        assert_eq!((s["status"].as_str(), s["visible"].as_bool()), (Some("unknown"), Some(true)), "{s}");
    }
    // At 72 dpi a pixel is a point. Each graphic is in the left half of its box (x 23–97), the
    // details on the right.
    let png = a.call("page_render", &json!({ "doc": doc, "page": 1, "dpi": 72 })).unwrap();
    let Content::Png { data, width, .. } = &png[0] else { panic!("expected an image") };
    let mut reader = png::Decoder::new(std::io::Cursor::new(data.as_slice())).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut buf).unwrap();
    let channels = frame.line_size / *width as usize;
    let px = |x: usize, y: usize| buf[y * frame.line_size + x * channels..][..3].to_vec();
    assert_eq!(px(60, 225), [220, 0, 0], "the image fills the left half's middle");
    assert_ne!(px(150, 225), [220, 0, 0], "and not the details");
    let inked = |y0: usize, y1: usize| (y0..y1).any(|y| (23..97).any(|x| px(x, y).iter().all(|c| *c < 100)));
    assert!(inked(130, 180), "the typed name is drawn");
    assert!(inked(60, 110), "the strokes are drawn");
}

#[test]
fn digital_ids_signing_and_validation_through_tools() {
    let dir = workdir("signing");
    let mut a = auto(&dir);
    let id = ok(
        &mut a,
        "sign_id_create",
        json!({ "name": "Ada Lovelace", "organization": "Analytical Engines", "email": "ada@example.com", "country": "GB", "key": "p256", "password": "secret12", "path": "ada.p12" }),
    );
    assert_eq!(id["certificate"]["subject"], "C=GB, O=Analytical Engines, CN=Ada Lovelace, E=ada@example.com");
    assert_eq!(id["certificate"]["self_signed"], true);
    assert!(matches!(a.call("sign_id_create", &json!({ "name": "X", "password": "short", "path": "x.p12" })), Err(ToolError::InvalidArgs(_))));
    // Eight characters at least (#1847): seven is refused.
    assert!(matches!(a.call("sign_id_create", &json!({ "name": "X", "password": "seven77", "path": "x.p12" })), Err(ToolError::InvalidArgs(_))));

    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "sign_list", json!({ "doc": doc }))["count"], 0);
    // A Keychain identity that doesn't exist (macOS) or Keychains at all (elsewhere).
    assert!(matches!(a.call("sign_document", &json!({ "doc": doc, "id": "keychain:No Such Signer", "out": "k.pdf" })), Err(ToolError::Failed(_))));
    assert!(matches!(a.call("sign_document", &json!({ "doc": doc, "id": "windows:No Such Signer", "out": "w.pdf" })), Err(ToolError::Failed(_))));
    let store = ok(&mut a, "sign_windows_ids", json!({}));
    let ids = store["ids"].as_array().unwrap();
    assert_eq!(store["count"].as_u64().unwrap(), ids.len() as u64);
    assert!(ids.iter().all(|id| id["id"].as_str().unwrap().starts_with("windows:")));
    // Store certificates that can't sign are explained, not dropped (issue #179).
    let unusable = store["unusable"].as_array().unwrap();
    assert!(unusable.iter().all(|u| u["subject"].is_string() && !u["reason"].as_str().unwrap().is_empty() && u["no_private_key"].is_boolean()));
    #[cfg(not(windows))]
    assert!(ids.is_empty() && unusable.is_empty());
    assert!(matches!(
        a.call("sign_document", &json!({ "doc": doc, "id": "ada.p12", "password": "wrong!", "out": "signed.pdf" })),
        Err(ToolError::InvalidArgs(_))
    ));
    let r = ok(
        &mut a,
        "sign_document",
        json!({ "doc": doc, "id": "ada.p12", "password": "secret12", "page": 2, "rect": [20, 200, 180, 250], "reason": "Approved", "location": "London", "out": "signed.pdf" }),
    );
    assert_eq!(r["signature"]["status"], "unknown");
    assert_eq!(r["signature"]["signer"], "Ada Lovelace");
    assert_eq!(r["signature"]["page"], 2);
    let rect: Vec<f64> = r["signature"]["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert!((rect[0] - 20.0).abs() < 0.5 && (rect[3] - 250.0).abs() < 0.5, "{rect:?}");
    assert!(dir.join("signed.pdf").exists());

    // Trust the ID: valid; comment afterwards: still valid, change allowed.
    let t = ok(&mut a, "sign_trust", json!({ "paths": ["ada.p12"], "password": "secret12" }));
    assert_eq!(t["trusted"].as_array().unwrap().len(), 1);
    let list = ok(&mut a, "sign_list", json!({ "doc": doc }));
    assert_eq!((list["all_valid"].as_bool(), list["signatures"][0]["status"].as_str()), (Some(true), Some("valid")));
    ok(&mut a, "comment_add", json!({ "doc": doc, "page": 1, "type": "note", "at": [20, 20], "contents": "ok" }));
    let s = &ok(&mut a, "sign_list", json!({ "doc": doc }))["signatures"][0];
    assert_eq!((s["status"].as_str(), s["modification"].as_str()), (Some("valid"), Some("allowed")));
    // A full rewrite is refused for a signed document.
    assert!(matches!(a.call("doc_save", &json!({ "doc": doc, "path": "copy.pdf", "full": true })), Err(ToolError::Failed(_))));
    ok(&mut a, "doc_save", json!({ "doc": doc }));
    // Three revisions: the original, the signature, the comment; the middle one is signed.
    let revs = ok(&mut a, "doc_revisions", json!({ "doc": doc }))["revisions"].as_array().cloned().unwrap();
    assert_eq!(revs.len(), 3, "{revs:?}");
    assert!(revs[0]["signed_by"].as_array().unwrap().is_empty() && revs[1]["signed_by"].as_array().unwrap().len() == 1, "{revs:?}");
    let old = ok(&mut a, "doc_open_revision", json!({ "doc": doc, "revision": 2 }))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "comment_list", json!({ "doc": old }))["comments"].as_array().map(Vec::len), Some(0), "before the comment");
    assert_eq!(ok(&mut a, "sign_list", json!({ "doc": old }))["count"], 1);
    assert!(matches!(a.call("doc_open_revision", &json!({ "doc": doc, "revision": 4 })), Err(ToolError::Failed(_))));
    assert_eq!(ok(&mut a, "sign_trust", json!({ "clear": true }))["trusted"].as_array().unwrap().len(), 0);
    assert_eq!(ok(&mut a, "sign_list", json!({ "doc": doc }))["signatures"][0]["status"], "unknown");
}

/// The decoded text of every visible signature appearance (Helvetica forms) in a signed file.
fn signature_appearances(path: &Path) -> String {
    use pdfcraft_cos::{Document, ObjRef, Object};
    let doc = Document::open(std::sync::Arc::new(std::fs::read(path).unwrap())).unwrap();
    let mut text = String::new();
    for num in doc.object_numbers() {
        if let Object::Stream(s) = &*doc.get(ObjRef { num, generation: 0 })
            && s.dict.name(b"Subtype") == Some(b"Form")
        {
            let content = String::from_utf8_lossy(&s.decoded().unwrap()).into_owned();
            if content.contains("/Helv") {
                text.push_str(&content);
            }
        }
    }
    text
}

/// `sign_document` takes the words of a visible signature's labels (say, in German) and which
/// items it shows, as the interface does. A label the signature's font can't draw is refused,
/// naming the label, the character and what is allowed, before anything is written.
#[test]
fn signing_with_translated_labels_through_tools() {
    let dir = workdir("signing-labels");
    let mut a = auto(&dir);
    ok(&mut a, "sign_id_create", json!({ "name": "Ada Lovelace", "key": "p256", "password": "secret12", "path": "ada.p12" }));
    let open = |a: &mut Automation| ok(a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let doc = open(&mut a);
    let args = |doc: u64, extra: Value, out: &str| {
        let mut args = json!({ "doc": doc, "id": "ada.p12", "password": "secret12", "page": 1, "rect": [10, 150, 190, 290], "reason": "Genehmigt", "location": "Berlin", "out": out });
        for (k, v) in extra.as_object().unwrap() {
            args[k] = v.clone();
        }
        args
    };
    for (extra, why, says) in [
        (
            json!({ "labels": { "signed_by": "数字签名者" } }),
            "a label Helvetica can't draw",
            "labels.signed_by: the standard fonts can't draw \"数\" (U+6570)",
        ),
        (json!({ "labels": { "reason": "Причина: " } }), "even one this signature doesn't show", "labels.reason"),
        (json!({ "labels": { "signer": "Von" } }), "an unknown label", "unknown label \"signer\""),
        (json!({ "labels": { "date": 1 } }), "not a string", "labels.date must be a string"),
        (json!({ "labels": "Datum" }), "not an object", "labels must be an object"),
        (json!({ "labels": { "date": "x".repeat(201) } }), "too long", "longer than 200"),
        (json!({ "appearance": { "reason": "yes" } }), "not true or false", "appearance.reason must be true or false"),
        (json!({ "appearance": { "logo": true } }), "an unknown item", "unknown appearance item"),
        (json!({ "rect": null, "labels": { "date": "Datum: " } }), "labels for an invisible signature", "needs a visible signature"),
        (json!({ "rect": null, "appearance": { "reason": true } }), "an appearance for an invisible one", "needs a visible signature"),
    ] {
        let r = a.call("sign_document", &args(doc, extra, "bad.pdf"));
        assert!(matches!(&r, Err(ToolError::InvalidArgs(m)) if m.contains(says)), "{why}: {r:?}");
    }
    let r = a.call("sign_document", &args(doc, json!({ "labels": { "signed_by": "数字签名者" } }), "bad.pdf"));
    assert!(matches!(&r, Err(ToolError::InvalidArgs(m)) if m.contains("signed_by label") && m.contains("Western European")), "{r:?}");
    assert!(!dir.join("bad.pdf").exists(), "nothing is written for a refused call");

    // German labels, with the reason and the location shown: stamped as given.
    let german = json!({
        "labels": { "signed_by": "Digital signiert von", "reason": "Grund: ", "location": "Ort: ", "date": "Datum: " },
        "appearance": { "reason": true, "location": true },
    });
    let r = ok(&mut a, "sign_document", args(doc, german, "de.pdf"));
    assert_eq!(r["signature"]["reason"], "Genehmigt");
    let stamped = signature_appearances(&dir.join("de.pdf"));
    // Each line starts with its label (a long one wraps after it).
    for word in ["(Digital", "(Grund:", "Genehmigt", "(Ort:", "Berlin", "(Datum:"] {
        assert!(stamped.contains(word), "{word}: {stamped}");
    }
    assert!(!stamped.contains("Reason") && !stamped.contains("Date:"), "{stamped}");

    // Labels omitted keep the English; the labels turned off show the values alone.
    let doc = open(&mut a);
    ok(&mut a, "sign_document", args(doc, json!({ "labels": { "date": "Datum: " }, "appearance": { "reason": true } }), "mixed.pdf"));
    let stamped = signature_appearances(&dir.join("mixed.pdf"));
    assert!(stamped.contains("(Reason:") && stamped.contains("(Datum:") && !stamped.contains("Berlin"), "{stamped}");
    let doc = open(&mut a);
    ok(&mut a, "sign_document", args(doc, json!({ "appearance": { "reason": true, "labels": false } }), "bare.pdf"));
    let stamped = signature_appearances(&dir.join("bare.pdf"));
    assert!(stamped.contains("(Genehmigt)") && !stamped.contains("Reason") && !stamped.contains("Date"), "{stamped}");
}

#[test]
fn optimizing_through_tools() {
    let dir = workdir("optimize");
    // A 2400 × 1600 photo-like JPEG at 600 dpi: a 4 × 2.67 inch page.
    let (w, h) = (2400u32, 1600u32);
    let px: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            [(x * 255 / w) as u8 ^ ((x * y) & 15) as u8, (y * 255 / h) as u8, ((x + y) & 255) as u8]
        })
        .collect();
    let mut jpeg = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95);
    enc.set_pixel_density(image::codecs::jpeg::PixelDensity::dpi(600));
    enc.encode(&px, w, h, image::ExtendedColorType::Rgb8).unwrap();
    std::fs::write(dir.join("photo.jpg"), &jpeg).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_create", json!({ "from": "images", "paths": ["photo.jpg"] }))["doc"].as_u64().unwrap();
    let r = ok(
        &mut a,
        "doc_optimize",
        json!({ "doc": doc, "path": "small.pdf", "color": { "ppi": 100, "above_ppi": 150, "compression": "jpeg", "quality": 45 }, "discard": ["metadata"] }),
    );
    assert_eq!((r["images"].as_u64(), r["images_resampled"].as_u64()), (Some(1), Some(1)), "{r}");
    assert!(r["bytes_after"].as_u64().unwrap() * 5 < r["bytes_before"].as_u64().unwrap(), "{r}");
    assert_eq!(r["discarded"][0]["category"], "metadata");
    // The result opens and shows one page of the same size.
    let small = ok(&mut a, "doc_open", json!({ "path": "small.pdf" }));
    assert_eq!(small["pages"], 1);
    let reduced = ok(&mut a, "doc_reduce", json!({ "doc": doc, "path": "reduced.pdf" }));
    assert!(reduced["bytes_after"].as_u64().unwrap() < reduced["bytes_before"].as_u64().unwrap());
    assert_eq!(reduced["written"], true);
    assert_eq!(std::fs::metadata(dir.join("reduced.pdf")).unwrap().len(), reduced["bytes_after"].as_u64().unwrap());
    assert!(matches!(
        a.call("doc_optimize", &json!({ "doc": doc, "path": "x.pdf", "color": { "compression": "gif" } })),
        Err(ToolError::InvalidArgs(_))
    ));
    assert!(matches!(a.call("doc_optimize", &json!({ "doc": doc, "path": "x.pdf", "discard": ["everything"] })), Err(ToolError::InvalidArgs(_))));
}

/// A one-page PDF whose page picture is drawn as `rows` grey images one pixel row high, 600 px
/// wide on a 72 pt square (600 ppi), as some PDF printers write it.
fn printed_in_strips(rows: usize) -> Vec<u8> {
    let mut objs: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()];
    let names: String = (0..rows).map(|r| format!("/R{r} {} 0 R ", 5 + r)).collect();
    objs.push(b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec());
    objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 72 72] /Resources << /XObject << {names}>> >> /Contents 4 0 R >>").into_bytes());
    let k = 72.0 / rows as f64;
    let content: String = (0..rows)
        .map(|r| {
            format!(
                "q 72 0 0 {k} 0 {} cm /R{r} Do Q
",
                (rows - 1 - r) as f64 * k
            )
        })
        .collect();
    objs.push(
        format!(
            "<< /Length {} >>
stream
{content}
endstream",
            content.len() + 1
        )
        .into_bytes(),
    );
    for r in 0..rows {
        let px: Vec<u8> = (0..600).map(|x| ((x + r * 7) % 256) as u8).collect();
        let mut o = format!(
            "<< /Type /XObject /Subtype /Image /Width 600 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length {} >>
stream
",
            px.len()
        )
        .into_bytes();
        o.extend_from_slice(&px);
        o.extend_from_slice(
            b"
endstream",
        );
        objs.push(o);
    }
    let mut out = b"%PDF-1.5
"
    .to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(
            format!(
                "{} 0 obj
",
                i + 1
            )
            .as_bytes(),
        );
        out.extend_from_slice(o);
        out.extend_from_slice(
            b"
endobj
",
        );
    }
    let x = out.len();
    out.extend_from_slice(
        format!(
            "xref
0 {}
0000000000 65535 f 
",
            objs.len() + 1
        )
        .as_bytes(),
    );
    for o in offs {
        out.extend_from_slice(
            format!(
                "{o:010} 00000 n 
"
            )
            .as_bytes(),
        );
    }
    out.extend_from_slice(
        format!(
            "trailer
<< /Size {} /Root 1 0 R >>
startxref
{x}
%%EOF
",
            objs.len() + 1
        )
        .as_bytes(),
    );
    out
}

#[test]
fn a_page_printed_in_strips_gets_small_through_tools() {
    let dir = workdir("optimize-strips");
    std::fs::write(dir.join("strips.pdf"), printed_in_strips(300)).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "strips.pdf" }))["doc"].as_u64().unwrap();
    let r = ok(&mut a, "doc_optimize", json!({ "doc": doc, "path": "mobile.pdf", "preset": "mobile" }));
    assert_eq!((r["merged_images"].as_u64(), r["fragments_merged"].as_u64(), r["unused_xobjects"].as_u64()), (Some(1), Some(300), Some(300)), "{r}");
    assert!(r["bytes_after"].as_u64().unwrap() * 10 < r["bytes_before"].as_u64().unwrap(), "{r}");
    // Reduce File Size merges too; with the option off, the strips stay.
    let reduced = ok(&mut a, "doc_reduce", json!({ "doc": doc, "path": "reduced.pdf" }));
    assert!(reduced["bytes_after"].as_u64().unwrap() * 10 < reduced["bytes_before"].as_u64().unwrap(), "{reduced}");
    let off = ok(&mut a, "doc_optimize", json!({ "doc": doc, "path": "off.pdf", "merge_fragments": false }));
    assert_eq!(off["merged_images"].as_u64(), Some(0), "{off}");
    // The merged page opens and renders.
    let mobile = ok(&mut a, "doc_open", json!({ "path": "mobile.pdf" }))["doc"].as_u64().unwrap();
    assert!(a.call("page_render", &json!({ "doc": mobile, "page": 1, "dpi": 72 })).is_ok());
    assert!(matches!(a.call("doc_optimize", &json!({ "doc": doc, "path": "x.pdf", "preset": "tiny" })), Err(ToolError::InvalidArgs(_))));
}

#[test]
fn lossless_optimization_through_tools_preserves_image_bytes_and_appearance() {
    use pdfcraft_cos::{Document, ObjRef, Object};
    use std::sync::Arc;
    let dir = workdir("lossless-optimize");
    let mut jpeg = Vec::new();
    let pixels = (0..128 * 128).flat_map(|i| [(i % 128) as u8, (i / 128) as u8, 180]).collect::<Vec<_>>();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95).encode(&pixels, 128, 128, image::ExtendedColorType::Rgb8).unwrap();
    std::fs::write(dir.join("image.jpg"), &jpeg).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_create", json!({"from":"images", "paths":["image.jpg"]}))["doc"].as_u64().unwrap();
    ok(&mut a, "doc_save", json!({"doc":doc, "path":"original.pdf"}));
    let original = std::fs::read(dir.join("original.pdf")).unwrap();
    let before = a.call("page_render", &json!({"doc":doc, "page":1, "dpi":72})).unwrap();
    let report = ok(&mut a, "doc_optimize", json!({"doc":doc, "path":"lossless.pdf", "profile":"lossless"}));
    assert_eq!(report["images_resampled"], 0);
    assert_eq!(report["images_recompressed"], 0);
    assert_eq!(report["discarded"], json!([]));
    let result = std::fs::read(dir.join("lossless.pdf")).unwrap();
    let image_streams = |bytes| {
        let document = Document::open(Arc::new(bytes)).unwrap();
        document
            .object_numbers()
            .into_iter()
            .filter_map(|number| {
                let object = document.get(ObjRef::new(number, document.generation(number)));
                match &*object {
                    Object::Stream(stream) if stream.dict.name(b"Subtype") == Some(b"Image") => {
                        Some((stream.dict.int(b"Width"), stream.dict.int(b"Height"), stream.raw.to_vec()))
                    }
                    _ => None,
                }
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(image_streams(result), image_streams(original.clone()));
    assert_eq!(std::fs::read(dir.join("original.pdf")).unwrap(), original);
    let reopened = ok(&mut a, "doc_open", json!({"path":"lossless.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(a.call("page_render", &json!({"doc":reopened, "page":1, "dpi":72})).unwrap(), before);
    for extra in [
        json!({"profile":"unknown"}),
        json!({"profile":"lossless", "color":{"compression":"jpeg"}}),
        json!({"profile":"lossless", "discard":["metadata"]}),
        json!({"profile":"lossless", "discard_tags":true}),
        json!({"profile":"lossless", "preset":"mobile"}),
        json!({"profile":"lossless", "merge_fragments":true}),
    ] {
        let mut args = json!({"doc":doc, "path":"refused.pdf"});
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        assert!(matches!(a.call("doc_optimize", &args), Err(ToolError::InvalidArgs(_))));
        assert!(!dir.join("refused.pdf").exists());
    }
    let bare = ok(&mut a, "doc_open", json!({"path":"a.pdf"}))["doc"].as_u64().unwrap();
    let report = ok(&mut a, "doc_optimize", json!({"doc":bare, "path":"no-metadata.pdf", "discard":["metadata"]}));
    assert_eq!(report["discarded"], json!([]));
    assert_eq!(ok(&mut a, "doc_open", json!({"path":"no-metadata.pdf"}))["pages"], 3);
}

#[test]
fn initial_view_through_tools() {
    let dir = workdir("initial-view");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let r = ok(&mut a, "doc_initial_view", json!({ "doc": doc }));
    assert_eq!((r["initial_view"]["layout"].as_str(), r["initial_view"]["page"].as_u64()), (Some("Default"), Some(1)));
    let r = ok(
        &mut a,
        "doc_initial_view",
        json!({ "doc": doc, "navigation": "pages", "layout": "two_up", "magnification": 150, "page": 2, "language": "en-GB", "binding": "right", "display_title": true }),
    );
    let v = &r["initial_view"];
    assert_eq!((v["navigation"].as_str(), v["magnification"].as_str(), v["page"].as_u64()), (Some("Pages"), Some("Percent(150.0)"), Some(2)));
    assert_eq!((v["language"].as_str(), v["binding"].as_str()), (Some("en-GB"), Some("right")));
    assert_eq!(ok(&mut a, "edit_undo", json!({ "doc": doc }))["undone"], "Change initial view");
    assert!(matches!(a.call("doc_initial_view", &json!({ "doc": doc, "layout": "spiral" })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(a.call("doc_initial_view", &json!({ "doc": doc, "page": 9 })), Err(ToolError::Failed(_))));
}

#[test]
fn ocr_tools_make_a_scan_searchable() {
    let dir = workdir("ocr");
    let mut a = auto(&dir);
    let status = ok(&mut a, "ocr_status", json!({}));
    assert_eq!(status["languages"][0]["code"], "en");
    for language in ["de", "ja", "fr", "es", "it", "nl", "da", "nb", "sv", "fi", "pt"] {
        assert!(status["languages"].as_array().unwrap().iter().any(|l| l["code"] == language));
    }
    for name in ["ocr_recognize", "ocr_recognize_files"] {
        let tool = tools().into_iter().find(|tool| tool.name == name).unwrap();
        let advertised = &tool.input_schema["properties"]["language"]["enum"];
        let available: Vec<_> = status["languages"].as_array().unwrap().iter().map(|language| language["code"].clone()).collect();
        assert_eq!(advertised, &json!(available), "{name} schema must advertise the same languages as ocr_status");
    }
    if status["available"] != true {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    let text = ok(&mut a, "doc_create", json!({ "from": "text", "text": "Searchable scans with recognised words" }))["doc"].as_u64().unwrap();
    ok(&mut a, "doc_export_images", json!({ "doc": text, "folder": "scan", "dpi": 150, "pages": [1] }));
    let png = std::fs::read_dir(dir.join("scan")).unwrap().next().unwrap().unwrap().path();
    let scan = ok(&mut a, "doc_create", json!({ "from": "images", "paths": [png.to_string_lossy()] }))["doc"].as_u64().unwrap();
    let r = ok(&mut a, "ocr_recognize", json!({ "doc": scan, "language": "it" }));
    assert!(r["pages"][0]["text"].as_str().unwrap().to_lowercase().contains("searchable"), "{r}");
    assert!(r["words"].as_u64().unwrap() >= 4);
    let again = ok(&mut a, "ocr_recognize", json!({ "doc": scan, "pages": [1] }));
    assert!(again["pages"][0]["skipped"].is_string(), "{again}");
    assert!(matches!(a.call("ocr_recognize", &json!({ "doc": scan, "language": "xx" })), Err(ToolError::InvalidArgs(_))));
}

/// #1028: `skip_text_pages: false` reads a picture on a page that has its own text, and the
/// page's own words are left out (counted) instead of being added a second time.
#[test]
fn ocr_recognize_reads_pictures_on_pages_with_text() {
    let dir = workdir("ocr-hybrid");
    let mut a = auto(&dir);
    if ok(&mut a, "ocr_status", json!({}))["available"] != true {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    let text = ok(&mut a, "doc_create", json!({ "from": "text", "text": "Pictures of words become searchable" }))["doc"].as_u64().unwrap();
    ok(&mut a, "doc_export_images", json!({ "doc": text, "folder": "scan", "dpi": 150, "pages": [1] }));
    let png = std::fs::read_dir(dir.join("scan")).unwrap().next().unwrap().unwrap().path();
    let doc = ok(&mut a, "doc_create", json!({ "from": "images", "paths": [png.to_string_lossy()] }))["doc"].as_u64().unwrap();
    ok(&mut a, "page_add_text", json!({ "doc": doc, "page": 1, "text": "Invoice reference 4711", "at": [40, 500], "size": 24 }));

    let skipped = ok(&mut a, "ocr_recognize", json!({ "doc": doc }));
    assert!(skipped["pages"][0]["skipped"].is_string(), "{skipped}");
    let r = ok(&mut a, "ocr_recognize", json!({ "doc": doc, "skip_text_pages": false }));
    let page = &r["pages"][0];
    let added = page["text"].as_str().unwrap().to_lowercase();
    assert!(added.contains("searchable") && !added.contains("invoice"), "{r}");
    assert!(page["already_on_page"].as_u64().unwrap() >= 2, "{r}");
    let found = |a: &mut Automation, q: &str| ok(a, "text_find", json!({ "doc": doc, "query": q }))["count"].as_u64().unwrap();
    assert_eq!((found(&mut a, "invoice"), found(&mut a, "searchable")), (1, 1));
    assert!(matches!(a.call("ocr_recognize", &json!({ "doc": doc, "skip_text_pages": "no" })), Err(ToolError::InvalidArgs(_))));
}

#[test]
fn ocr_enhance_crops_paper_edges_with_margin_and_undo() {
    let dir = workdir("ocr-crop");
    let mut a = auto(&dir);
    if ok(&mut a, "ocr_status", json!({}))["available"] != true {
        return;
    }
    let image = image::RgbImage::from_fn(800, 600, |x, y| image::Rgb(if (180..800).contains(&x) && y < 490 { [235; 3] } else { [25; 3] }));
    image.save(dir.join("scan.png")).unwrap();
    let doc = ok(&mut a, "doc_create", json!({"from":"images","paths":["scan.png"]}))["doc"].as_u64().unwrap();
    let no_crop = ok(&mut a, "ocr_enhance", json!({"doc":doc,"auto_crop":false,"auto_rotate":false,"deskew":false}));
    assert!(no_crop["pages"][0]["crop_box"].is_null());
    let cropped = ok(&mut a, "ocr_enhance", json!({"doc":doc,"crop_margin":0,"auto_rotate":false,"deskew":false}));
    let rect = cropped["pages"][0]["crop_box"].as_array().unwrap();
    assert!(rect[0].as_f64().unwrap() > 100.0);
    assert_eq!(cropped["words"], 0);
    ok(&mut a, "doc_save", json!({"doc":doc,"path":"cropped.pdf"}));
    assert_eq!(ok(&mut a, "edit_undo", json!({"doc":doc}))["undone"], "Correct scans");
    let padded = ok(&mut a, "ocr_enhance", json!({"doc":doc,"crop_margin":10,"auto_rotate":false,"deskew":false}));
    let padded_rect = padded["pages"][0]["crop_box"].as_array().unwrap();
    assert!((rect[0].as_f64().unwrap() - padded_rect[0].as_f64().unwrap() - 10.0).abs() < 1.0);
    assert!((rect[1].as_f64().unwrap() - padded_rect[1].as_f64().unwrap() - 10.0).abs() < 1.0);
    for margin in [-1, 21] {
        assert!(matches!(a.call("ocr_enhance", &json!({"doc":doc,"crop_margin":margin})), Err(ToolError::InvalidArgs(_))));
    }
}

#[test]
fn ocr_enhance_corrects_rotation_without_adding_text() {
    let dir = workdir("ocr-enhance");
    let mut a = auto(&dir);
    if ok(&mut a, "ocr_status", json!({}))["available"] != true {
        return;
    }
    let sentence =
        (0..18).map(|_| "Please read this document with the page upright. The quick brown fox jumps over the lazy dog.\n").collect::<String>();
    let doc = ok(&mut a, "doc_create", json!({"from":"text","text":sentence}))["doc"].as_u64().unwrap();
    ok(&mut a, "page_rotate", json!({"doc":doc,"pages":[1],"degrees":180}));
    ok(&mut a, "doc_export_images", json!({"doc":doc,"folder":"scan","dpi":150,"pages":[1]}));
    let png = std::fs::read_dir(dir.join("scan")).unwrap().next().unwrap().unwrap().path();
    let scan = ok(&mut a, "doc_create", json!({"from":"images","paths":[png.to_string_lossy()]}))["doc"].as_u64().unwrap();
    let corrected = ok(&mut a, "ocr_enhance", json!({"doc":scan,"deskew":false}));
    assert_eq!(corrected["pages"][0]["rotation"], 180, "{corrected}");
    assert_eq!(corrected["words"], 0);
    let read = ok(&mut a, "ocr_recognize", json!({"doc":scan,"auto_rotate":false,"deskew":false}));
    assert!(read["pages"][0]["text"].as_str().unwrap().contains("Please"), "{read}");
}

#[test]
fn ocr_recognize_files_refuses_paths_outside_the_root() {
    let dir = workdir("ocr-files-root");
    let mut a = auto(&dir);
    // Refused before anything else, with or without the OCR models installed.
    for path in ["../outside.pdf", "sub/../../outside.pdf", "/outside.pdf"] {
        let r = a.call("ocr_recognize_files", &json!({ "paths": ["a.pdf", path], "folder": "out" }));
        assert!(matches!(&r, Err(e) if e.to_string().contains("outside the allowed directory")), "{path}: {r:?}");
    }
    assert!(!dir.join("out").exists(), "nothing was started");
}

#[test]
fn ocr_recognize_files_writes_searchable_copies() {
    let dir = workdir("ocr-files");
    let mut a = auto(&dir);
    if ok(&mut a, "ocr_status", json!({}))["available"] != true {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    let text = ok(&mut a, "doc_create", json!({ "from": "text", "text": "Batch recognition works" }))["doc"].as_u64().unwrap();
    ok(&mut a, "doc_export_images", json!({ "doc": text, "folder": "scan", "dpi": 150, "pages": [1] }));
    let png = std::fs::read_dir(dir.join("scan")).unwrap().next().unwrap().unwrap().path();
    let scan = ok(&mut a, "doc_create", json!({ "from": "images", "paths": [png.to_string_lossy()] }))["doc"].as_u64().unwrap();
    ok(&mut a, "doc_save", json!({ "doc": scan, "path": "in/scan.pdf" }));
    let r = ok(&mut a, "ocr_recognize_files", json!({ "paths": ["in/scan.pdf", "missing.pdf"], "folder": "out", "language": "it" }));
    assert!(r["files"][0]["words"].as_u64().unwrap() >= 3, "{r}");
    assert!(r["files"][1]["error"].is_string(), "{r}");
    let out = ok(&mut a, "doc_open", json!({ "path": "out/scan.pdf" }))["doc"].as_u64().unwrap();
    let found = ok(&mut a, "text_find", json!({ "doc": out, "query": "recognition" }));
    assert!(found.to_string().contains("\"page\""), "{found}");
}

#[test]
fn javascript_through_tools() {
    let dir = workdir("js");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    for (name, y) in [("Price", 20), ("Qty", 60), ("Total", 100)] {
        ok(&mut a, "form_add_field", json!({ "doc": doc, "page": 1, "type": "text", "rect": [20, y, 180, y + 22], "name": name }));
    }
    ok(
        &mut a,
        "js_set_document_script",
        json!({ "doc": doc, "name": "lib", "script": "function money(v) { return util.printf('EUR %,2.2f', v); }" }),
    );
    assert_eq!(ok(&mut a, "js_document_scripts", json!({ "doc": doc }))["scripts"][0]["name"], "lib");
    ok(
        &mut a,
        "form_set_script",
        json!({ "doc": doc, "field": "Total", "event": "calculate", "script": "event.value = getField('Price').value * getField('Qty').value;" }),
    );
    ok(&mut a, "form_set_script", json!({ "doc": doc, "field": "Total", "event": "format", "script": "event.value = money(event.value);" }));
    ok(
        &mut a,
        "form_set_script",
        json!({ "doc": doc, "field": "Qty", "event": "validate", "script": "if (event.value < 1) { app.alert('Order at least one'); event.rc = false; }" }),
    );
    ok(&mut a, "form_fill", json!({ "doc": doc, "values": { "Price": "1250", "Qty": "2" } }));
    assert!(page_text(&mut a, doc)[0].contains("EUR 2.500,00"), "{:?}", page_text(&mut a, doc));
    let err = a.call("form_fill", &json!({ "doc": doc, "values": { "Qty": "0" } })).unwrap_err();
    assert!(err.to_string().contains("Order at least one"), "{err}");

    let r = ok(
        &mut a,
        "js_run",
        json!({ "doc": doc, "script": "console.println(getField('Total').value); getField('Qty').value = 3; this.pageNum = 1; app.alert('ok');" }),
    );
    assert_eq!(r["console"][0], "2500");
    assert_eq!(r["alerts"][0], "ok");
    assert_eq!(r["requests"][0]["page"], 2);
    let f = ok(&mut a, "form_fields", json!({ "doc": doc }));
    let total = f["fields"].as_array().unwrap().iter().find(|x| x["name"] == "Total").unwrap().clone();
    assert_eq!(total["value"], "3750");
    let r = ok(&mut a, "js_run", json!({ "doc": doc, "script": "nope()" }));
    assert!(r["error"].as_str().unwrap().contains("nope"));
    assert_eq!(ok(&mut a, "js_enabled", json!({ "enabled": false }))["enabled"], false);
    assert!(a.call("js_run", &json!({ "doc": doc, "script": "1" })).is_err());
}

#[test]
fn merging_form_data_into_a_spreadsheet() {
    let dir = workdir("merge-data");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "form_add_field", json!({ "doc": doc, "page": 1, "type": "text", "rect": [20, 20, 180, 42], "name": "City" }));
    for (city, file) in [("Paris", "one.xfdf"), ("Oslo", "two.fdf")] {
        ok(&mut a, "form_fill", json!({ "doc": doc, "values": { "City": city } }));
        ok(&mut a, "doc_export_data", json!({ "doc": doc, "path": file, "what": "fields" }));
    }
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "filled.pdf" }));
    let r = ok(&mut a, "form_merge_data", json!({ "paths": ["one.xfdf", "two.fdf", "filled.pdf"], "path": "report.csv" }));
    assert_eq!(r["rows"], 3);
    assert_eq!(std::fs::read_to_string(dir.join("report.csv")).unwrap(), "City\nParis\nOslo\nOslo\n");
}

#[test]
fn field_actions_through_tools() {
    let dir = workdir("field-actions");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "form_add_field", json!({ "doc": doc, "page": 1, "type": "button", "rect": [20, 20, 120, 42], "name": "Go" }));
    let r = ok(
        &mut a,
        "form_set_actions",
        json!({ "doc": doc, "field": "Go", "actions": [
            { "trigger": "mouse_up", "javascript": "this.pageNum = 2;" },
            { "trigger": "mouse_enter", "hide": ["Go"] },
            { "trigger": "on_focus", "page": 3 }
        ] }),
    );
    assert_eq!(r["actions"].as_array().unwrap().len(), 3, "{r}");
    assert_eq!(r["actions"][0], json!({ "trigger": "mouse_up", "javascript": "this.pageNum = 2;" }));
    assert_eq!(r["actions"][1], json!({ "trigger": "mouse_enter", "hide": ["Go"] }));
    assert_eq!(r["actions"][2], json!({ "trigger": "on_focus", "page": 3 }));
    let run = ok(&mut a, "js_run", json!({ "doc": doc, "script": "this.pageNum = 2;", "field": "Go" }));
    assert_eq!(run["requests"][0]["page"], 3);
    assert!(a.call("form_set_actions", &json!({ "doc": doc, "field": "Go", "actions": [{ "trigger": "wave" }] })).is_err());
}

#[test]
fn detecting_form_fields_through_tools() {
    let dir = workdir("detect-fields");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_create", json!({ "from": "text", "text": "Name: ____________________\n\nCity: ____________________" }))["doc"]
        .as_u64()
        .unwrap();
    let r = ok(&mut a, "form_detect_fields", json!({ "doc": doc, "add": false }));
    assert_eq!(r["fields"].as_array().unwrap().len(), 2, "{r}");
    assert_eq!(r["fields"][1]["name"], "City");
    assert_eq!(ok(&mut a, "form_fields", json!({ "doc": doc }))["fields"].as_array().unwrap().len(), 0);
    ok(&mut a, "form_detect_fields", json!({ "doc": doc }));
    ok(&mut a, "form_fill", json!({ "doc": doc, "values": { "City": "Lisbon" } }));
    let f = ok(&mut a, "form_fields", json!({ "doc": doc }));
    assert_eq!(f["fields"][1]["value"], "Lisbon", "{f}");
}

#[test]
fn comparing_documents_through_tools() {
    let dir = workdir("compare");
    let mut a = auto(&dir);
    let v1 = ok(&mut a, "doc_create", json!({ "from": "text", "text": "Rent is 900 per month. Pets are not allowed." }))["doc"].as_u64().unwrap();
    let v2 = ok(&mut a, "doc_create", json!({ "from": "text", "text": "Rent is 950 per month. Pets are allowed. Parking included." }))["doc"]
        .as_u64()
        .unwrap();
    let r = ok(&mut a, "doc_compare", json!({ "doc": v2, "other": v1, "visual": true }));
    assert!(!r["visual"].as_array().unwrap().is_empty(), "{r}");
    assert_eq!((r["replaced"].as_u64(), r["inserted"].as_u64(), r["deleted"].as_u64()), (Some(1), Some(1), Some(1)), "{r}");
    assert_eq!(r["changes"][0]["old"]["text"], "900");
    assert_eq!(r["changes"][0]["new"]["text"], "950");
    assert_eq!(r["changes"][0]["new"]["page"], 1);
    ok(&mut a, "doc_compare_report", json!({ "doc": v2, "other": v1, "path": "report.pdf" }));
    assert!(std::fs::read(dir.join("report.pdf")).unwrap().starts_with(b"%PDF"));
    assert_eq!(ok(&mut a, "doc_compare_mark", json!({ "doc": v2, "other": v1 }))["comments"], 3);
    assert!(a.call("doc_compare", &json!({ "doc": v2, "other": 999 })).is_err());
}

#[test]
fn actions_through_tools() {
    let dir = workdir("actions");
    let mut a = auto(&dir);
    let list = ok(&mut a, "action_list", json!({}));
    assert!(list["actions"].as_array().unwrap().iter().any(|x| x["name"] == "Prepare for Distribution"));
    assert!(list["steps"].as_array().unwrap().iter().any(|x| x["step"] == "add_watermark" && x["takes_arg"] == true));
    let r = ok(&mut a, "action_run", json!({ "action": "add page numbers", "paths": ["a.pdf", "b.pdf", "missing.pdf"], "folder": "out" }));
    assert_eq!(r["files"][0]["log"][0], "Add footer");
    assert!(r["files"][2]["error"].is_string());
    let doc = ok(&mut a, "doc_open", json!({ "path": "out/b.pdf" }))["doc"].as_u64().unwrap();
    assert!(page_text(&mut a, doc)[1].contains("Page 2 of 2"), "{:?}", page_text(&mut a, doc));
    let r = ok(&mut a, "action_run", json!({ "steps": [{ "step": "set_title", "arg": "Hello" }], "paths": ["a.pdf"], "folder": "out2" }));
    assert_eq!(r["files"][0]["log"][0], "Set document title");
    assert!(a.call("action_run", &json!({ "steps": [{ "step": "fly" }], "paths": ["a.pdf"], "folder": "x" })).is_err());
}

#[test]
fn action_run_keeps_outputs_with_duplicate_input_basenames() {
    let dir = workdir("action-duplicate-basenames");
    std::fs::create_dir_all(dir.join("left")).unwrap();
    std::fs::create_dir_all(dir.join("right")).unwrap();
    std::fs::write(dir.join("left/report.pdf"), fixture(1)).unwrap();
    std::fs::write(dir.join("right/report.pdf"), fixture(2)).unwrap();
    let mut a = auto(&dir);
    let args = json!({
        "steps": [{ "step": "set_title", "arg": "Processed" }],
        "paths": ["left/report.pdf", "right/report.pdf"],
        "folder": "results"
    });
    let result = ok(&mut a, "action_run", args.clone());
    let files = result["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    let first_path = files[0]["output"].as_str().unwrap();
    let second_path = files[1]["output"].as_str().unwrap();
    assert!(first_path.ends_with("report.pdf"), "{first_path}");
    assert!(second_path.ends_with("report (2).pdf"), "{second_path}");
    let first_bytes = std::fs::read(first_path).unwrap();
    assert_eq!(ok(&mut a, "doc_open", json!({ "path": first_path }))["pages"], 1);
    assert_eq!(ok(&mut a, "doc_open", json!({ "path": second_path }))["pages"], 2);

    let repeated = ok(&mut a, "action_run", args);
    assert!(repeated["files"][0]["output"].as_str().unwrap().ends_with("report (3).pdf"));
    assert!(repeated["files"][1]["output"].as_str().unwrap().ends_with("report (4).pdf"));
    assert_eq!(std::fs::read(first_path).unwrap(), first_bytes, "an earlier output must not be replaced");
}

#[test]
fn pdfa_through_tools() {
    let dir = workdir("pdfa");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_create", json!({ "from": "text", "text": "Archive me" }))["doc"].as_u64().unwrap();
    let r = ok(&mut a, "pdfa_verify", json!({ "doc": doc }));
    assert_eq!(r["compliant"], false);
    assert!(r["issues"].as_array().unwrap().iter().any(|i| i["clause"] == "6.6.2.1"), "{r}");
    let r = ok(&mut a, "pdfa_convert", json!({ "doc": doc, "level": "3b" }));
    assert_eq!(r["declared"]["pdfa"], "PDF/A-3b", "{r}");
    assert!(r["issues"].as_array().unwrap().iter().all(|i| i["fixable"] == false), "{r}");
    assert!(a.call("pdfa_verify", &json!({ "doc": doc, "level": "9z" })).is_err());
}

#[test]
fn print_readiness_through_tools() {
    let dir = workdir("print-readiness");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_create", json!({ "from": "text", "text": "Proof me" }))["doc"].as_u64().unwrap();
    let before = ok(&mut a, "doc_info", json!({ "doc": doc }));
    let r = ok(&mut a, "print_readiness", json!({ "doc": doc }));
    assert!(r["text"].as_str().unwrap().contains("verification only"), "{r}");
    assert_eq!(r["pages"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["pages"][0]["trim_set"], false, "{r}");
    assert_eq!(r["resolution_unknown"], 0, "{r}");
    let findings = r["findings"].as_array().unwrap();
    // A page without a TrimBox is one informational note for the document, not a bleed warning.
    assert!(!findings.iter().any(|f| f["id"] == "page-bleed"), "{r}");
    let trim: Vec<_> = findings.iter().filter(|f| f["id"] == "page-trim").collect();
    assert_eq!(trim.len(), 1, "{r}");
    assert_eq!(trim[0]["severity"], "info");
    assert!(trim[0]["page"].is_null(), "{r}");
    assert_eq!(ok(&mut a, "doc_info", json!({ "doc": doc })), before, "verification doesn't change the document");
    assert!(a.call("print_readiness", &json!({ "doc": 999 })).is_err());
    assert!(pdfcraft_automation::tools().iter().any(|t| t.name == "print_readiness" && t.read_only));
}

#[test]
fn exporting_to_word_html_and_rtf() {
    let dir = workdir("office");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    for ext in ["docx", "pptx", "html", "rtf"] {
        let r = ok(&mut a, "doc_export_office", json!({ "doc": doc, "path": format!("a.{ext}") }));
        assert_eq!(r["format"], ext);
    }
    let html = std::fs::read_to_string(dir.join("a.html")).unwrap();
    assert!(html.contains("Page 1") && html.contains("Page 3") && html.matches("<hr>").count() == 2, "{html}");
    assert!(std::fs::read(dir.join("a.docx")).unwrap().starts_with(b"PK"));
    assert!(std::fs::read(dir.join("a.pptx")).unwrap().starts_with(b"PK"));
    assert!(std::fs::read_to_string(dir.join("a.rtf")).unwrap().contains("Page 2"));
    assert!(a.call("doc_export_office", &json!({ "doc": doc, "path": "a.xyz" })).is_err());
}

/// #773: a diagonal watermark must not enter the body flow or move invoice fields.
#[test]
fn exporting_an_invoice_to_word_keeps_the_watermark_out_of_flow() {
    use pdfcraft_cos::{Dict, Document, ObjRef, SaveOptions, Stream, write_full};
    let dir = workdir("invoice-word");
    let mut pdf = Document::open(std::sync::Arc::new(fixture(1))).unwrap();
    let content = b"q BT /F1 60 Tf 0.8 g 0.70710678 0.70710678 -0.70710678 0.70710678 30 80 Tm (DRAFT) Tj ET Q \
BT /F1 11 Tf 20 250 Td (Description) Tj 110 0 Td (Amount) Tj \
-110 -20 Td (Design) Tj 110 0 Td (100.00) Tj \
-110 -20 Td (Review) Tj 110 0 Td (50.00) Tj ET \
BT /F1 11 Tf 20 20 Td (Total due: 150.00) Tj ET";
    pdf.set(ObjRef::new(5, 0), Stream { dict: Dict::new(), raw: content.to_vec().into() });
    std::fs::write(dir.join("invoice.pdf"), write_full(&pdf, &SaveOptions::default()).unwrap()).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "invoice.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "doc_export_office", json!({ "doc": doc, "path": "invoice.docx" }));
    // Inspect the actual package emitted by the tool, not an intermediate page model.
    let zip = std::fs::read(dir.join("invoice.docx")).unwrap();
    let mut at = 0;
    let mut xml = String::new();
    while zip[at..].starts_with(b"PK\x03\x04") {
        let size = u32::from_le_bytes(zip[at + 18..at + 22].try_into().unwrap()) as usize;
        let len = u16::from_le_bytes(zip[at + 26..at + 28].try_into().unwrap()) as usize;
        let data = at + 30 + len;
        if &zip[at + 30..data] == b"word/document.xml" {
            std::io::Read::read_to_string(&mut flate2::read::DeflateDecoder::new(&zip[data..data + size]), &mut xml).unwrap();
            break;
        }
        at = data + size;
    }
    assert!(xml.contains("rotation:315.000"), "diagonal text survives extraction: {xml}");
    assert!(xml.contains("z-index:-2"), "the large watermark stays behind the invoice");
    assert!(!xml.contains("<wp:inline>") && !xml.contains("<w:tbl>"));
    assert!(!xml.contains("w:val=\"single\""));
    assert_eq!(xml.matches(">DRAFT</w:t>").count(), 1);
    assert_eq!(xml.matches(">Total due: 150.00</w:t>").count(), 1);
    assert_eq!(xml.matches("<w:sectPr>").count(), 1);
    assert!(!xml.contains("w:type=\"page\""));
    assert!(xml.contains("margin-left:20.000pt"), "invoice fields retain their left edge");
    let rendered = a.call("page_render", &json!({ "doc": doc, "page": 1, "dpi": 72 })).unwrap();
    assert!(rendered.iter().any(|c| matches!(c, Content::Png { .. })));
}

#[test]
fn hundreds_of_xfa_calculations_open_and_recalculate_within_their_budgets() {
    let dir = workdir("xfa-many");
    let (sections, rows) = (10, 20);
    std::fs::write(dir.join("many.pdf"), pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::many_calculations_template(sections, rows))).unwrap();
    let mut a = auto(&dir);
    let started = std::time::Instant::now();
    let opened = ok(&mut a, "doc_open", json!({ "path": "many.pdf" }));
    let open_took = started.elapsed();
    let doc = opened["doc"].as_u64().unwrap();
    let warnings = opened["warnings"].to_string();
    assert!(!warnings.contains("were skipped") && !warnings.contains("stopped"), "{warnings}");
    let fields = ok(&mut a, "form_fields", json!({ "doc": doc }));
    let totals: Vec<&serde_json::Value> =
        fields["fields"].as_array().unwrap().iter().filter(|f| f["name"].as_str().is_some_and(|n| n.starts_with("total"))).collect();
    assert_eq!(totals.len(), sections * rows);
    assert!(totals.iter().all(|f| !f["value"].as_str().unwrap_or("").is_empty()), "every total calculated");
    let started = std::time::Instant::now();
    let r = ok(&mut a, "form_fill", json!({ "doc": doc, "values": { "qty": "100" } }));
    let change_took = started.elapsed();
    assert!(!r.to_string().contains("were skipped"), "{r}");
    let fields = ok(&mut a, "form_fields", json!({ "doc": doc }));
    let total = fields["fields"].as_array().unwrap().iter().find(|f| f["name"] == "total").unwrap();
    assert_eq!(total["value"], "200");
    eprintln!("{} calculate scripts: open {open_took:?}, recalculation after a change {change_took:?}", sections * rows + sections);
}

#[test]
fn xfa_forms_flatten_and_convert_to_static_through_tools() {
    let dir = workdir("xfa-flat");
    let data = "<form><page1><country>JP</country></page1></form>";
    std::fs::write(dir.join("fields.pdf"), pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::fields_template(data))).unwrap();
    let mut a = auto(&dir);
    // Convert to static: the fields stay, the XFA goes.
    let doc = ok(&mut a, "doc_open", json!({ "path": "fields.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "doc_info", json!({ "doc": doc }))["xfa"], "dynamic");
    let before = ok(&mut a, "form_fields", json!({ "doc": doc }))["fields"].as_array().unwrap().len();
    ok(&mut a, "xfa_to_static", json!({ "doc": doc }));
    let info = ok(&mut a, "doc_info", json!({ "doc": doc }));
    assert!(info["xfa"].is_null(), "{info}");
    assert_eq!(ok(&mut a, "form_fields", json!({ "doc": doc }))["fields"].as_array().unwrap().len(), before);
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "static.pdf" }));
    let again = ok(&mut a, "doc_open", json!({ "path": "static.pdf" }))["doc"].as_u64().unwrap();
    assert!(ok(&mut a, "doc_info", json!({ "doc": again }))["xfa"].is_null());
    assert!(a.call("xfa_to_static", &json!({ "doc": again })).is_err(), "not an XFA form");
    // Flatten the fields: a plain PDF with the values as page content.
    let doc = ok(&mut a, "doc_open", json!({ "path": "fields.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "doc_flatten", json!({ "doc": doc, "comments": false, "fields": true }));
    assert!(ok(&mut a, "doc_info", json!({ "doc": doc }))["xfa"].is_null());
    assert_eq!(ok(&mut a, "form_fields", json!({ "doc": doc }))["fields"].as_array().unwrap().len(), 0);
    let text = ok(&mut a, "text_extract", json!({ "doc": doc, "pages": [1] }));
    assert!(text.to_string().contains("Japan"), "{text}");
}

#[test]
fn xfa_data_files_export_and_import_through_tools() {
    let dir = workdir("xfa-data");
    let data = "<form><page1><country>JP</country></page1></form>";
    std::fs::write(dir.join("fields.pdf"), pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::fields_template(data))).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "fields.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "doc_export_data", json!({ "doc": doc, "path": "data.xml", "what": "fields" }));
    ok(&mut a, "doc_export_data", json!({ "doc": doc, "path": "data.xdp", "what": "fields" }));
    let xml = std::fs::read_to_string(dir.join("data.xml")).unwrap();
    let xdp = std::fs::read_to_string(dir.join("data.xdp")).unwrap();
    assert!(xml.contains("<form") && xml.contains("<country>JP</country>"), "{xml}");
    assert!(xdp.contains("<xdp:xdp") && xdp.contains("<country>JP</country>"), "{xdp}");
    // Edit the exported data and import it back.
    std::fs::write(dir.join("edited.xdp"), xdp.replace("<country>JP</country>", "<country>CA</country>")).unwrap();
    let r = ok(&mut a, "doc_import_data", json!({ "doc": doc, "path": "edited.xdp" }));
    assert!(r["filled_fields"].as_u64().unwrap() >= 1);
    let fields = ok(&mut a, "form_fields", json!({ "doc": doc }));
    let country = fields["fields"].as_array().unwrap().iter().find(|f| f["name"] == "country").unwrap();
    assert_eq!(country["value"], json!("CA"));
    // A picture for the image field, through form_set_image, lands in the data.
    std::fs::write(dir.join("pic.gif"), pdfcraft_xfa::fixtures::TINY_GIF).unwrap();
    ok(&mut a, "form_set_image", json!({ "doc": doc, "field": "photo", "path": "pic.gif" }));
    ok(&mut a, "doc_export_data", json!({ "doc": doc, "path": "after.xml", "what": "fields" }));
    let after = std::fs::read_to_string(dir.join("after.xml")).unwrap();
    assert!(after.contains("<photo xfa:contentType=\"image/gif\">R0lGODlh"), "{after}");
    // XDP is for XFA forms only.
    std::fs::write(dir.join("plain.pdf"), pdfcraft_xfa::fixtures::static_shell("<form1><page1><name>Ada</name></page1></form1>")).unwrap();
    let plain = ok(&mut a, "doc_open", json!({ "path": "plain.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "doc_export_data", json!({ "doc": plain, "path": "static.xml", "what": "fields" }));
    assert!(std::fs::read_to_string(dir.join("static.xml")).unwrap().contains("<name>Ada</name>"), "a static XFA form exports its data too");
}

#[test]
fn dynamic_xfa_forms_open_render_fill_and_save_through_tools() {
    let dir = workdir("xfa");
    std::fs::write(dir.join("xfa.pdf"), pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::template(2))).unwrap();
    let mut a = auto(&dir);
    let opened = ok(&mut a, "doc_open", json!({ "path": "xfa.pdf" }));
    assert_eq!(opened["pages"], 2, "laid out from the template, not the placeholder page");
    let doc = opened["doc"].as_u64().unwrap();
    let info = ok(&mut a, "doc_info", json!({ "doc": doc }));
    assert_eq!(info["xfa"], "dynamic");
    assert_eq!(info["xfa_layout"]["pages"], 2);
    assert_eq!(info["xfa_layout"]["fields"], 11);
    let fields = ok(&mut a, "form_fields", json!({ "doc": doc }));
    let family = fields["fields"].as_array().unwrap().iter().find(|f| f["name"] == "familyName").expect("familyName");
    assert_eq!((family["type"].as_str(), family["tooltip"].as_str(), family["page"].as_u64()), (Some("text"), Some("Your family name"), Some(1)));
    let answer = fields["fields"].as_array().unwrap().iter().find(|f| f["name"] == "answer").expect("radio group");
    assert_eq!(answer["options"], json!(["Y", "N"]));
    // The page renders with the widgets' own appearances: the check box's border is drawn.
    let png = a.call("page_render", &json!({ "doc": doc, "page": 1, "dpi": 36 })).unwrap();
    let Content::Png { data, .. } = &png[0] else { panic!("expected an image") };
    let decoder = png::Decoder::new(std::io::Cursor::new(data.as_slice()));
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    reader.next_frame(&mut buf).unwrap();
    assert!(buf.iter().filter(|b| **b < 128).count() > 200, "the page is not blank");
    ok(&mut a, "form_fill", json!({ "doc": doc, "values": { "familyName": "Singh", "agree": true, "answer": "N", "born": "2001-02-03" } }));
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "out.pdf" }));
    ok(&mut a, "doc_close", json!({ "doc": doc }));
    let reopened = ok(&mut a, "doc_open", json!({ "path": "out.pdf" }));
    assert_eq!(reopened["pages"], 2, "not laid out twice");
    let doc2 = reopened["doc"].as_u64().unwrap();
    let fields = ok(&mut a, "form_fields", json!({ "doc": doc2 }));
    let by = |n: &str| fields["fields"].as_array().unwrap().iter().find(|f| f["name"] == n).unwrap()["value"].clone();
    assert_eq!((by("familyName"), by("agree"), by("answer"), by("born")), (json!("Singh"), json!(true), json!("N"), json!("2001-02-03")));
    assert_eq!(ok(&mut a, "doc_info", json!({ "doc": doc2 }))["xfa_layout"]["pages"], 2);
}

#[test]
fn cut_stack_printing_through_tools() {
    let dir = workdir("cut-stack");
    std::fs::write(dir.join("numbered.pdf"), fixture(10)).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({"path": "numbered.pdf"}))["doc"].as_u64().unwrap();
    let before = page_text(&mut a, doc);
    for reverse in [false, true] {
        let r = ok(
            &mut a,
            "doc_print",
            json!({
                "doc": doc, "layout": "multiple", "order": "cut-stack", "per_sheet": 4,
                "orientation": "portrait", "auto_rotate": false, "reverse": reverse, "path": "cut.pdf"
            }),
        );
        assert_eq!(r["sheets"], 3);
        let printed = ok(&mut a, "doc_open", json!({"path": "cut.pdf"}))["doc"].as_u64().unwrap();
        let expected =
            if reverse { vec![vec![10, 7, 4, 1], vec![9, 6, 3], vec![8, 5, 2]] } else { vec![vec![1, 4, 7, 10], vec![2, 5, 8], vec![3, 6, 9]] };
        for (text, expected) in page_text(&mut a, printed).iter().zip(&expected) {
            let actual: Vec<usize> = text.split_whitespace().filter_map(|t| t.parse().ok()).collect();
            assert_eq!(&actual, expected, "saved PDF must contain the imposed order: {text}");
        }
        ok(&mut a, "doc_close", json!({"doc": printed}));
    }
    let r = ok(
        &mut a,
        "doc_print",
        json!({
            "doc": doc, "pages": "2-10", "subset": "odd", "reverse": true,
            "layout": "multiple", "order": "cut-stack", "per_sheet": 2, "auto_rotate": false,
            "orientation": "landscape", "path": "range.pdf"
        }),
    );
    assert_eq!(r["pages"], 5);
    let printed = ok(&mut a, "doc_open", json!({"path": "range.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(page_text(&mut a, printed), ["Page 10\nPage 4", "Page 8\nPage 2", "Page 6"]);
    assert_eq!(page_text(&mut a, doc), before);
    assert_eq!(ok(&mut a, "doc_info", json!({"doc": doc}))["document"]["dirty"], false);
    for duplex in ["long-edge", "short-edge"] {
        assert!(matches!(
            a.call(
                "doc_print",
                &json!({
                    "doc": doc, "layout": "multiple", "order": "cut-stack", "duplex": duplex, "path": "refused.pdf"
                })
            ),
            Err(ToolError::InvalidArgs(_))
        ));
    }
    assert!(matches!(a.call("doc_print", &json!({"doc": doc, "order": "cut-stack", "path": "refused.pdf"})), Err(ToolError::InvalidArgs(_))));
    assert!(!dir.join("refused.pdf").exists());
    assert!(a.call("doc_print", &json!({"doc": doc, "layout": "multiple", "order": "cut-stack", "path": "../escaped.pdf"})).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn measurements_calibrate_draw_save_reopen_and_export() {
    let dir = workdir("measurements");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({"path":"a.pdf"}))["doc"].as_u64().unwrap();
    let scale = ok(&mut a, "measure_scale", json!({"doc":doc,"page":1,"points":[[10,10],[70,10]],"distance":6,"unit":"m","precision":3}));
    assert!((scale["scale"]["x"].as_f64().unwrap() - 0.1).abs() < 1e-12);
    for (tool, points) in [
        ("measure_distance", json!([[10, 20], [70, 100]])),
        ("measure_perimeter", json!([[10, 20], [70, 20], [70, 100]])),
        ("measure_area", json!([[10, 20], [70, 20], [70, 100], [10, 100]])),
    ] {
        ok(&mut a, tool, json!({"doc":doc,"page":1,"points":points,"label":"Room, \"A\"","author":"Tester"}));
    }
    let all = ok(&mut a, "measure_list", json!({"doc":doc}));
    assert_eq!(all["count"], 3);
    assert_eq!(all["unsupported"], json!([]));
    assert_eq!(all["truncated"], false);
    for (m, value) in all["measurements"].as_array().unwrap().iter().zip([10.0, 14.0, 48.0]) {
        assert!((m["reading"]["value"].as_f64().unwrap() - value).abs() < 1e-6);
        assert_eq!(m["page"], 1);
        assert_eq!(m["label"], "Room, \"A\"");
    }
    let preview = ok(&mut a, "measure_info", json!({"doc":doc,"page":1,"type":"area","points":[[10,20],[70,20],[70,100],[10,100]]}));
    assert!((preview["reading"]["value"].as_f64().unwrap() - 48.0).abs() < 1e-6);
    let rendered = a.call("page_render", &json!({"doc":doc,"page":1,"dpi":72})).unwrap();
    assert!(matches!(rendered.first(), Some(Content::Png { .. })));
    ok(&mut a, "edit_undo", json!({"doc":doc}));
    assert_eq!(ok(&mut a, "measure_list", json!({"doc":doc}))["count"], 2);
    ok(&mut a, "edit_redo", json!({"doc":doc}));
    ok(&mut a, "doc_save", json!({"doc":doc,"path":"measured.pdf"}));
    let reopened = ok(&mut a, "doc_open", json!({"path":"measured.pdf"}))["doc"].as_u64().unwrap();
    let after = ok(&mut a, "measure_list", json!({"doc":reopened}));
    assert_eq!(after["measurements"], all["measurements"]);
    let exported = ok(&mut a, "measure_export", json!({"doc":reopened,"out":"measurements.csv"}));
    assert_eq!((exported["count"].as_u64(), exported["unsupported"].as_u64()), (Some(3), Some(0)));
    let csv = std::fs::read_to_string(dir.join("measurements.csv")).unwrap();
    assert!(csv.contains("area,48,\"m^2\",\"Room, \"\"A\"\"\""), "{csv}");
    assert!(a.call("measure_export", &json!({"doc":doc,"out":"../outside.csv"})).is_err());
    // A new viewport changes future readings, without recalibrating saved measurements.
    ok(&mut a, "measure_scale", json!({"doc":doc,"page":1,"units_per_point":1,"rect":[0,0,50,50],"unit":"cm"}));
    assert_eq!(ok(&mut a, "measure_scale", json!({"doc":doc,"page":1,"at":[20,20]}))["scale"]["unit"], "cm");
    assert_eq!(ok(&mut a, "measure_scale", json!({"doc":doc,"page":1,"at":[80,80]}))["scale"]["unit"], "m");
    assert_eq!(ok(&mut a, "measure_list", json!({"doc":doc}))["measurements"], all["measurements"]);
    // Rotate the page, then measure in its displayed coordinates.
    ok(&mut a, "page_rotate", json!({"doc":doc,"pages":[1],"degrees":90}));
    ok(&mut a, "measure_distance", json!({"doc":doc,"page":1,"points":[[100,100],[180,160]]}));
    let all = ok(&mut a, "measure_list", json!({"doc":doc}));
    let last = all["measurements"].as_array().unwrap().last().unwrap();
    assert!((last["reading"]["value"].as_f64().unwrap() - 10.0).abs() < 1e-6);
    assert_eq!(last["points"], json!([[100.0, 100.0], [180.0, 160.0]]));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn measurements_bad_arguments_leave_document_and_history_unchanged() {
    let dir = workdir("measurement-errors");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({"path":"a.pdf"}))["doc"].as_u64().unwrap();
    let before = ok(&mut a, "doc_list", json!({}));
    for (tool, args) in [
        ("measure_distance", json!({"doc":doc,"page":1,"points":[[0,0]]})),
        ("measure_distance", json!({"doc":doc,"page":1,"points":[[0,0],[0,0]]})),
        ("measure_area", json!({"doc":doc,"page":1,"points":[[0,0],[20,20],[0,20],[20,0]]})),
        ("measure_scale", json!({"doc":doc,"page":1,"points":[[0,0],[0,0]],"distance":10})),
        ("measure_snap", json!({"doc":doc,"page":1,"at":[1e100,0]})),
        ("measure_scale", json!({"doc":doc,"page":1,"units_per_point":1,"precision":8})),
    ] {
        assert!(a.call(tool, &args).is_err(), "{tool} {args}");
        assert_eq!(ok(&mut a, "doc_list", json!({})), before);
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn measurement_snap_tool_covers_all_targets() {
    let dir = workdir("measurement-snap");
    let mut a = auto(&dir);
    let source = String::from_utf8(fixture(1)).unwrap();
    let old = "BT /F1 24 Tf 20 150 Td (Page 1) Tj ET";
    let drawing = "10 20 m 110 20 l S 60 0 m 60 80 l S";
    assert!(drawing.len() <= old.len());
    let source = source.replace(old, &format!("{drawing:<width$}", width = old.len()));
    std::fs::write(dir.join("drawing.pdf"), source).unwrap();
    let doc = ok(&mut a, "doc_open", json!({"path":"drawing.pdf"}))["doc"].as_u64().unwrap();
    for (at, kind, point, midpoints) in [
        ([11, 280], "endpoint", [10, 280], true),
        ([60, 259], "midpoint", [60, 260], true),
        ([59, 279], "intersection", [60, 280], false),
        ([32, 278], "path", [32, 280], true),
    ] {
        let snap = ok(&mut a, "measure_snap", json!({"doc":doc,"page":1,"at":at,"tolerance":3,"midpoints":midpoints}));
        assert_eq!(snap["snap"]["kind"], kind);
        assert_eq!(snap["snap"]["point"], json!(point.map(f64::from)));
        assert_eq!(snap["truncated"], false);
    }
    assert!(ok(&mut a, "measure_snap", json!({"doc":doc,"page":1,"at":[180,180]}))["snap"].is_null());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn xfa_scripts_run_for_buttons_and_field_changes_through_tools() {
    let dir = workdir("xfa-scripts");
    std::fs::write(dir.join("scripted.pdf"), pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::scripted_template())).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "scripted.pdf" }))["doc"].as_u64().unwrap();
    let field = |a: &mut Automation, n: &str| {
        let f = ok(a, "form_fields", json!({ "doc": doc }));
        f["fields"].as_array().unwrap().iter().find(|f| f["name"] == n).cloned()
    };
    // Opening ran the initialize and calculate scripts.
    assert_eq!(field(&mut a, "qty").unwrap()["value"], "2");
    assert_eq!(field(&mut a, "total").unwrap()["value"], "10");
    // Filling recalculates; a bad value shows its message (the tool reports alerts in js output? no: it is applied, the value stays).
    ok(&mut a, "form_fill", json!({ "doc": doc, "values": { "qty": "4" } }));
    assert_eq!(field(&mut a, "total").unwrap()["value"], "20");
    assert_eq!(ok(&mut a, "doc_info", json!({ "doc": doc }))["xfa_scripts_off"], false);
    // A button's XFA click script runs through js_run, like any button.
    let before = ok(&mut a, "doc_info", json!({ "doc": doc }))["xfa_layout"]["fields"].as_u64().unwrap();
    let r = ok(&mut a, "js_run", json!({ "doc": doc, "script": "", "field": "addRow" }));
    assert!(r["error"].is_null(), "{r}");
    assert!(field(&mut a, "amount_2").is_some());
    assert_eq!(ok(&mut a, "doc_info", json!({ "doc": doc }))["xfa_layout"]["fields"].as_u64().unwrap(), before + 2);
    // Undo takes the row away again.
    ok(&mut a, "edit_undo", json!({ "doc": doc }));
    assert!(field(&mut a, "amount_2").is_none());
    let r = ok(&mut a, "js_run", json!({ "doc": doc, "script": "", "field": "hello" }));
    assert_eq!(r["alerts"], json!(["Hello 4"]));
}

#[test]
fn doc_info_says_when_a_runaway_script_turned_the_forms_scripts_off() {
    let dir = workdir("xfa-scripts-off");
    // An initialize script whose loops are in nested calls runs until it is abandoned.
    let tpl = pdfcraft_xfa::fixtures::scripted_template().replace(
        "if (qty.rawValue === null) qty.rawValue = 2;",
        "function f() { for (var j = 0; j &lt; 99999; j++) {} } for (var k = 0; k &lt; 5000; k++) f();",
    );
    std::fs::write(dir.join("runaway.pdf"), pdfcraft_xfa::fixtures::shell(&tpl)).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "runaway.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "doc_info", json!({ "doc": doc }))["xfa_scripts_off"], true);
    let _ = std::fs::remove_dir_all(dir);
}

/// doc_info describes a link's set-layer-visibility action by layer name.
#[test]
fn doc_info_describes_set_layer_links() {
    let dir = workdir("layer-links");
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [4 0 R 5 0 R] /D << >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [6 0 R] >>",
        "<< /Type /OCG /Name (Red) >>",
        "<< /Type /OCG /Name (Green) >>",
        "<< /Type /Annot /Subtype /Link /Rect [10 10 90 30] /A << /S /SetOCGState /State [/Toggle 5 0 R /OFF 4 0 R 9 0 R] /PreserveRB false >> >>",
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    std::fs::write(dir.join("layers.pdf"), pdf).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "layers.pdf" }))["doc"].as_u64().unwrap();
    let info = ok(&mut a, "doc_info", json!({ "doc": doc }));
    // A group that isn't a layer has no name.
    assert_eq!(
        info["links"][0]["target"],
        json!({
            "layers": [{ "layer": "Green", "state": "toggle" }, { "layer": "Red", "state": "off" }, { "layer": null, "state": "off" }],
            "preserve_rb": false,
        })
    );
}

mod close_argument_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    // Create exclusively and remove only this test's directory, never a pre-existing one.
    struct CloseDir(PathBuf);

    impl CloseDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let parent = std::env::temp_dir();
            for _ in 0..128 {
                let serial = NEXT.fetch_add(1, Ordering::Relaxed);
                let path = parent.join(format!("pdfcraft-close-{}-{serial}", std::process::id()));
                match std::fs::create_dir(&path) {
                    Ok(()) => {
                        let dir = Self(path);
                        std::fs::write(dir.0.join("a.pdf"), fixture(3)).unwrap();
                        std::fs::write(dir.0.join("b.pdf"), fixture(2)).unwrap();
                        return dir;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("creating close test directory: {error}"),
                }
            }
            panic!("no unused close test directory after 128 attempts");
        }
    }

    impl Drop for CloseDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn close_rejects_invalid_discard_types_without_changing_clean_or_dirty_documents() {
        let dir = CloseDir::new();
        let source = std::fs::read(dir.0.join("a.pdf")).unwrap();
        let other_source = std::fs::read(dir.0.join("b.pdf")).unwrap();
        let mut a = auto(&dir.0);
        let other = ok(&mut a, "doc_open", json!({ "path": "b.pdf" }))["doc"].as_u64().unwrap();
        for dirty in [false, true] {
            let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
            if dirty {
                ok(&mut a, "doc_set_info", json!({ "doc": doc, "key": "Title", "value": "Unsaved title" }));
            }
            let list = ok(&mut a, "doc_list", json!({}));
            let info = ok(&mut a, "doc_info", json!({ "doc": doc }));
            let text = ok(&mut a, "text_extract", json!({ "doc": doc }));
            let bytes = a.session().get(pdfcraft_engine::DocId(doc)).unwrap().bytes.clone();
            for invalid in [json!("false"), json!(0), json!([]), json!({})] {
                let error = a.call("doc_close", &json!({ "doc": doc, "discard_changes": invalid })).unwrap_err();
                assert!(matches!(error, ToolError::InvalidArgs(ref message) if message == "discard_changes must be true or false"));
                assert_eq!(ok(&mut a, "doc_list", json!({})), list, "all document identities, order, paths and history stay unchanged");
                assert_eq!(ok(&mut a, "doc_info", json!({ "doc": doc })), info);
                assert_eq!(ok(&mut a, "text_extract", json!({ "doc": doc })), text);
                let current = a.session().get(pdfcraft_engine::DocId(doc)).unwrap();
                assert_eq!(current.bytes, bytes);
                assert_eq!(current.dirty, dirty);
                assert_eq!(std::fs::read(dir.0.join("a.pdf")).unwrap(), source);
                assert_eq!(std::fs::read(dir.0.join("b.pdf")).unwrap(), other_source);
            }
            assert_eq!(ok(&mut a, "doc_close", json!({ "doc": doc, "discard_changes": true }))["closed"], doc);
            assert!(a.session().get(pdfcraft_engine::DocId(doc)).is_none());
            assert!(a.session().get(pdfcraft_engine::DocId(other)).is_some());
        }
    }

    #[test]
    fn close_keeps_default_and_boolean_discard_controls() {
        let dir = CloseDir::new();
        let source = std::fs::read(dir.0.join("a.pdf")).unwrap();
        let mut a = auto(&dir.0);
        let other = ok(&mut a, "doc_open", json!({ "path": "b.pdf" }))["doc"].as_u64().unwrap();
        for discard in [None, Some(Value::Null), Some(json!(false)), Some(json!(true))] {
            let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
            let mut args = json!({ "doc": doc });
            if let Some(discard) = discard {
                args["discard_changes"] = discard;
            }
            assert_eq!(ok(&mut a, "doc_close", args)["closed"], doc);
            assert!(a.session().get(pdfcraft_engine::DocId(doc)).is_none());
            assert!(a.session().get(pdfcraft_engine::DocId(other)).is_some());
        }
        let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
        ok(&mut a, "doc_set_info", json!({ "doc": doc, "key": "Title", "value": "Unsaved title" }));
        let before = ok(&mut a, "doc_list", json!({}));
        let info = ok(&mut a, "doc_info", json!({ "doc": doc }));
        let bytes = a.session().get(pdfcraft_engine::DocId(doc)).unwrap().bytes.clone();
        for args in [json!({ "doc": doc }), json!({ "doc": doc, "discard_changes": null }), json!({ "doc": doc, "discard_changes": false })] {
            let error = a.call("doc_close", &args).unwrap_err();
            assert!(matches!(error, ToolError::Failed(ref message) if message.contains("unsaved changes")));
            assert_eq!(ok(&mut a, "doc_list", json!({})), before);
            assert_eq!(ok(&mut a, "doc_info", json!({ "doc": doc })), info);
            assert_eq!(a.session().get(pdfcraft_engine::DocId(doc)).unwrap().bytes, bytes);
        }
        assert_eq!(ok(&mut a, "doc_close", json!({ "doc": doc, "discard_changes": true }))["closed"], doc);
        assert!(a.session().get(pdfcraft_engine::DocId(doc)).is_none());
        assert!(a.session().get(pdfcraft_engine::DocId(other)).is_some());
        assert_eq!(std::fs::read(dir.0.join("a.pdf")).unwrap(), source);
    }
}

mod combine_argument_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    // Own only a newly created directory; the bounded runner provides project-local TMPDIR.
    struct CombineDir(PathBuf);

    impl CombineDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let parent = std::env::temp_dir();
            for _ in 0..128 {
                let serial = NEXT.fetch_add(1, Ordering::Relaxed);
                let path = parent.join(format!("pdfcraft-combine-{}-{serial}", std::process::id()));
                match std::fs::create_dir(&path) {
                    Ok(()) => {
                        let dir = Self(path);
                        std::fs::write(dir.0.join("a.pdf"), fixture(3)).unwrap();
                        std::fs::write(dir.0.join("b.pdf"), fixture(2)).unwrap();
                        return dir;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("creating combine test directory: {error}"),
                }
            }
            panic!("no unused combine test directory after 128 attempts");
        }
    }

    impl Drop for CombineDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn invalid_combine_selectors_preserve_outputs_inputs_and_open_documents() {
        let dir = CombineDir::new();
        let first = std::fs::read(dir.0.join("a.pdf")).unwrap();
        let second = std::fs::read(dir.0.join("b.pdf")).unwrap();
        let sentinel = b"existing destination";
        std::fs::write(dir.0.join("combined.pdf"), sentinel).unwrap();
        let mut a = auto(&dir.0);
        let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
        ok(&mut a, "doc_set_info", json!({ "doc": doc, "key": "Title", "value": "Unsaved title" }));
        ok(&mut a, "doc_open", json!({ "path": "b.pdf" }));
        let list = ok(&mut a, "doc_list", json!({}));
        let info = ok(&mut a, "doc_info", json!({ "doc": doc }));
        let text = page_text(&mut a, doc);
        let bytes = a.session().get(pdfcraft_engine::DocId(doc)).unwrap().bytes.clone();
        for invalid in [json!(1), json!(true), json!({ "page": 1 }), json!(["1"])] {
            for (index, pages) in [json!([invalid, null]), json!([null, invalid])].into_iter().enumerate() {
                for out in ["combined.pdf", "not-created.pdf"] {
                    let error = a.call("doc_combine", &json!({ "paths": ["a.pdf", "b.pdf"], "pages": pages, "out": out, "open": true })).unwrap_err();
                    assert!(
                        matches!(error, ToolError::InvalidArgs(ref message) if message == &format!("pages[{index}] must be a range string or null"))
                    );
                    assert_eq!(std::fs::read(dir.0.join("combined.pdf")).unwrap(), sentinel);
                    assert!(!dir.0.join("not-created.pdf").exists());
                    assert_eq!(ok(&mut a, "doc_list", json!({})), list);
                    assert_eq!(ok(&mut a, "doc_info", json!({ "doc": doc })), info);
                    assert_eq!(page_text(&mut a, doc), text);
                    assert_eq!(a.session().get(pdfcraft_engine::DocId(doc)).unwrap().bytes, bytes);
                    assert_eq!(std::fs::read(dir.0.join("a.pdf")).unwrap(), first);
                    assert_eq!(std::fs::read(dir.0.join("b.pdf")).unwrap(), second);
                }
            }
        }
        let error = a
            .call("doc_combine", &json!({ "paths": ["missing-a.pdf", "missing-b.pdf"], "pages": [null, false], "out": "not-created.pdf" }))
            .unwrap_err();
        assert!(matches!(error, ToolError::InvalidArgs(ref message) if message == "pages[1] must be a range string or null"));
        assert_eq!(ok(&mut a, "doc_list", json!({})), list);
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 3, "no output or staging files are created");
    }

    #[test]
    fn valid_combine_selectors_keep_all_pages_and_selected_order_after_reopen() {
        let dir = CombineDir::new();
        let first = std::fs::read(dir.0.join("a.pdf")).unwrap();
        let second = std::fs::read(dir.0.join("b.pdf")).unwrap();
        let mut a = auto(&dir.0);
        let all = vec!["Page 1", "Page 2", "Page 3", "Page 1", "Page 2"];
        for (pages, expected) in [
            (None, all.clone()),
            (Some(Value::Null), all.clone()),
            (Some(json!([null, null])), all.clone()),
            (Some(json!(["", null])), all),
            (Some(json!(["3, 1", null])), vec!["Page 3", "Page 1", "Page 1", "Page 2"]),
            (Some(json!([null, "2"])), vec!["Page 1", "Page 2", "Page 3", "Page 2"]),
        ] {
            let mut args = json!({ "paths": ["a.pdf", "b.pdf"], "out": "combined.pdf", "open": false });
            if let Some(pages) = pages {
                args["pages"] = pages;
            }
            let result = ok(&mut a, "doc_combine", args);
            assert!(result["bytes"].as_u64().unwrap() > 0);
            assert!(result.get("document").is_none());
            assert!(a.session().docs().is_empty());
            let mut fresh = auto(&dir.0);
            let opened = ok(&mut fresh, "doc_open", json!({ "path": "combined.pdf" }));
            assert_eq!(opened["pages"].as_u64().unwrap(), expected.len() as u64);
            assert_eq!(page_text(&mut fresh, opened["doc"].as_u64().unwrap()), expected);
            assert_eq!(std::fs::read(dir.0.join("a.pdf")).unwrap(), first);
            assert_eq!(std::fs::read(dir.0.join("b.pdf")).unwrap(), second);
        }
    }
}

#[cfg(feature = "mcp")]
#[test]
fn conventions_core_tools_preserve_root_and_batch_errors() {
    let dir = workdir("conventions-core");
    let mut s = McpServer::new(auto(&dir));
    let run = |s: &mut McpServer, name: &str, args: Value| rpc(s, 1, "tools/call", json!({"name":name,"arguments":args}));
    assert_eq!(run(&mut s, "doc_inspect", json!({}))["result"]["structuredContent"]["documents"], json!([]));
    let opened = run(&mut s, "command_run", json!({"id":"file.open","params":{"path":"a.pdf"}}));
    assert_eq!(opened["result"]["isError"], false, "{opened}");
    let doc = opened["result"]["structuredContent"]["doc"].as_u64().unwrap();
    let filtered = run(&mut s, "command_list", json!({"doc":doc,"filter":"rotate","enabled_only":true}));
    let commands = filtered["result"]["structuredContent"]["commands"].as_array().unwrap();
    assert!(!commands.is_empty());
    assert!(commands.iter().all(|c| c["enabled"] == true));
    assert!(commands.iter().find(|c| c["id"] == "page.rotate").unwrap()["params"].is_object());
    for stop in [true, false] {
        let r = run(
            &mut s,
            "command_batch",
            json!({"steps":[{"id":"page.rotate","params":{"doc":doc,"degrees":90}},{"id":"no.such.command"},{"id":"page.rotate","params":{"doc":doc,"degrees":90}}],"stop_on_error":stop}),
        );
        assert_eq!(r["result"]["isError"], true, "{r}");
        assert_eq!(r["result"]["structuredContent"]["completed"], if stop { 1 } else { 2 });
        assert_eq!(r["result"]["structuredContent"]["failed"], 1);
    }
    let r = run(&mut s, "command_run", json!({"id":"file.save","params":{"doc":doc,"path":"../escaped.pdf"}}));
    assert_eq!(r["result"]["isError"], true);
    assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("outside the allowed directory"));
    let r = run(&mut s, "command_run", json!({"id":"page.rotate","params":{"doc":doc,"degrees":90,"typo":1}}));
    assert_eq!(r["result"]["isError"], false);
    assert!(r["result"]["structuredContent"]["warnings"][0].as_str().unwrap().contains("typo"));
    let before = run(&mut s, "doc_info", json!({"doc":doc}));
    let r = run(&mut s, "render_preview", json!({"doc":doc,"page":1,"max_side":64}));
    assert_eq!(r["result"]["isError"], false, "{r}");
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD.decode(r["result"]["content"][0]["data"].as_str().unwrap()).unwrap();
    let image = image::load_from_memory(&png).unwrap();
    assert!(image.width().max(image.height()) <= 64);
    assert_eq!(before["result"], run(&mut s, "doc_info", json!({"doc":doc}))["result"]);
}

#[cfg(feature = "mcp")]
#[test]
fn conventions_annotations_and_strict_keys_in_both_modes() {
    for compact in [false, true] {
        let mut s = McpServer::new(Automation::new()).with_compact(compact);
        let r = rpc(&mut s, 1, "tools/list", json!({}));
        let tools = r["result"]["tools"].as_array().unwrap();
        for name in ["command_list", "command_run", "command_batch", "doc_inspect", "render_preview"] {
            assert!(tools.iter().any(|t| t["name"] == name), "missing {name}");
        }
        for tool in tools {
            for hint in ["readOnlyHint", "destructiveHint", "idempotentHint", "openWorldHint"] {
                assert!(tool["annotations"][hint].is_boolean(), "{}: {hint}", tool["name"]);
            }
            let r = rpc(&mut s, 2, "tools/call", json!({"name":tool["name"],"arguments":{"bogus_arg":1}}));
            assert_eq!(r["error"]["code"], -32602, "{}", tool["name"]);
            let message = r["error"]["message"].as_str().unwrap();
            assert!(message.contains("bogus_arg") && message.contains("expected:"));
        }
    }
}

#[cfg(feature = "mcp")]
#[test]
fn conventions_resources() {
    let dir = workdir("conventions-resources");
    let mut s = McpServer::new(auto(&dir));
    let read = |s: &mut McpServer, uri: &str| {
        let r = rpc(s, 1, "resources/read", json!({"uri":uri}));
        serde_json::from_str::<Value>(r["result"]["contents"][0]["text"].as_str().expect("JSON resource")).unwrap()
    };
    assert_eq!(read(&mut s, "pdfcraft://document"), json!({"documents":[]}));
    rpc(&mut s, 2, "tools/call", json!({"name":"doc_open","arguments":{"path":"a.pdf"}}));
    let doc = rpc(&mut s, 3, "tools/call", json!({"name":"doc_inspect","arguments":{}}));
    assert_eq!(read(&mut s, "pdfcraft://document"), doc["result"]["structuredContent"]);
    let commands = rpc(&mut s, 4, "tools/call", json!({"name":"command_list","arguments":{}}));
    assert_eq!(read(&mut s, "pdfcraft://commands"), commands["result"]["structuredContent"]);
    // Responses keep the shape every supported protocol revision expects: no cache hints.
    for (method, params) in [("tools/list", json!({})), ("resources/list", json!({})), ("resources/read", json!({"uri":"pdfcraft://document"}))] {
        assert!(rpc(&mut s, 5, method, params)["result"].get("resultType").is_none());
    }
    assert_eq!(rpc(&mut s, 7, "initialize", json!({"protocolVersion":"2026-07-28"}))["result"]["protocolVersion"], "2025-06-18");
    assert!(s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":999}}"#).is_none());
    assert_eq!(
        rpc(&mut s, 9, "tools/call", json!({"name":"doc_inspect","arguments":{},"_meta":{"progressToken":"quick"}}))["result"]["isError"],
        false
    );
}

#[test]
fn command_batch_refuses_more_than_a_thousand_steps() {
    let dir = workdir("batch-cap");
    let mut a = auto(&dir);
    let steps: Vec<Value> = (0..1001).map(|_| json!({"id":"no.such.command"})).collect();
    let err = a.call("command_batch", &json!({"steps": steps})).unwrap_err();
    assert!(err.to_string().contains("at most 1000 steps"), "{err}");
}

#[test]
fn command_batch_tells_a_tool_name_from_an_unknown_or_toolless_command() {
    let dir = workdir("batch-ids");
    let mut a = auto(&dir);
    let steps = json!([{"id":"page_rotate"}, {"id":"doc_inspect"}, {"id":"no.such.command"}, {"id":"help.about"}]);
    let r = ok(&mut a, "command_batch", json!({"steps": steps, "stop_on_error": false}));
    let errors: Vec<&str> = r["results"].as_array().unwrap().iter().map(|s| s["error"].as_str().unwrap()).collect();
    assert_eq!(
        errors,
        [
            r#""page_rotate" is a tool, not a command id; use the command id "page.rotate", or call the tool directly"#,
            r#""doc_inspect" is a tool, not a command id; call it directly (command ids are in command_list)"#,
            r#"unknown command id "no.such.command"; see command_list"#,
            r#"command "help.about" has no headless tool; see command_list"#,
        ]
    );
}

#[test]
fn thai_text_through_tools_saves_reopens_extracts_and_renders() {
    let dir = workdir("thai-content");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({"path": "a.pdf"}))["doc"].as_u64().unwrap();
    let text = "สวัสดี น้ำ ผู้ใช้ ปู่ ๑๒๓ Hello 123";
    let args = json!({"doc": doc, "page": 1, "text": text, "at": [10, 20], "size": 9});
    if pdfcraft_fonts::document_thai_font().is_none() {
        assert!(a.call("page_add_text", &args).unwrap_err().to_string().contains("Thai"));
        assert_eq!(ok(&mut a, "content_list", json!({"doc": doc}))["count"], 0);
        return;
    }
    ok(&mut a, "page_add_text", args);
    assert!(page_text(&mut a, doc)[0].contains(text), "{:?}", page_text(&mut a, doc));
    ok(&mut a, "doc_save", json!({"doc": doc, "path": "thai.pdf"}));
    let reopened = ok(&mut a, "doc_open", json!({"path": "thai.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "content_list", json!({"doc": reopened}))["items"][0]["text"], text);
    assert!(page_text(&mut a, reopened)[0].contains(text));
    let rendered = a.call("page_render", &json!({"doc": reopened, "page": 1, "dpi": 144})).unwrap();
    assert!(rendered.iter().any(|c| matches!(c, Content::Png { data, width, height } if !data.is_empty() && *width > 0 && *height > 0)));
    ok(&mut a, "content_update", json!({"doc": reopened, "page": 1, "index": 1, "text": "แก้ไข น้ำ"}));
    assert!(page_text(&mut a, reopened)[0].contains("แก้ไข น้ำ"));
}

#[test]
fn hebrew_text_through_tools_saves_reopens_extracts_and_renders() {
    let dir = workdir("hebrew-content");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({"path": "a.pdf"}))["doc"].as_u64().unwrap();
    let text = "שם משפחה: שרון, טל׳ 052-1234567 (נייד)";
    let args = json!({"doc": doc, "page": 1, "text": text, "rect": [10, 20, 400, 40], "size": 11, "align": "right"});
    if pdfcraft_fonts::document_hebrew_font(false).is_none() {
        assert!(a.call("page_add_text", &args).unwrap_err().to_string().contains("U+05E9"));
        assert_eq!(ok(&mut a, "content_list", json!({"doc": doc}))["count"], 0);
        return;
    }
    ok(&mut a, "page_add_text", args);
    // Extraction gives back the typing order, not the drawing order.
    assert!(page_text(&mut a, doc)[0].contains(text), "{:?}", page_text(&mut a, doc));
    ok(&mut a, "doc_save", json!({"doc": doc, "path": "hebrew.pdf"}));
    let reopened = ok(&mut a, "doc_open", json!({"path": "hebrew.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "content_list", json!({"doc": reopened}))["items"][0]["text"], text);
    assert!(page_text(&mut a, reopened)[0].contains(text));
    let found = ok(&mut a, "text_find", json!({"doc": reopened, "query": "שרון"}));
    assert_eq!(found["matches"].as_array().map(Vec::len), Some(1), "{found}");
    let rendered = a.call("page_render", &json!({"doc": reopened, "page": 1, "dpi": 144})).unwrap();
    assert!(rendered.iter().any(|c| matches!(c, Content::Png { data, width, height } if !data.is_empty() && *width > 0 && *height > 0)));
    ok(&mut a, "content_update", json!({"doc": reopened, "page": 1, "index": 1, "text": "מיכאל שָׁלוֹם"}));
    assert!(page_text(&mut a, reopened)[0].contains("מיכאל"));
}

#[test]
fn mixed_objects_move_through_tools_with_generation_and_one_undo() {
    let dir = workdir("mixed-object-move");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({"path":"a.pdf"}))["doc"].as_u64().unwrap();
    ok(&mut a, "page_add_text", json!({"doc":doc,"page":1,"text":"Added label","rect":[20,20,130,45]}));
    ok(&mut a, "doc_save", json!({"doc":doc,"path":"before.pdf"}));
    let inventory = ok(&mut a, "object_list", json!({"doc":doc,"page":1}));
    assert_eq!(inventory["count"], 2);
    let objects: Vec<_> = inventory["objects"].as_array().unwrap().iter().map(|o| json!({"kind":o["kind"],"index":o["index"]})).collect();
    let moved = ok(&mut a, "object_move", json!({"doc":doc,"page":1,"generation":inventory["generation"],"objects":objects,"offset":[18,27]}));
    assert_eq!(moved["moved"], 2);
    let after = ok(&mut a, "object_list", json!({"doc":doc,"page":1}));
    for (old, new) in inventory["objects"].as_array().unwrap().iter().zip(after["objects"].as_array().unwrap()) {
        for i in 0..4 {
            assert!((new["rect"][i].as_f64().unwrap() - old["rect"][i].as_f64().unwrap() - if i % 2 == 0 { 18.0 } else { 27.0 }).abs() < 0.02);
        }
        assert_eq!(old["text"], new["text"]);
    }
    assert!(a.call("object_move", &json!({"doc":doc,"page":1,"generation":inventory["generation"],"objects":objects,"offset":[1,1]})).is_err());
    let stable = ok(&mut a, "object_list", json!({"doc":doc,"page":1}));
    assert_eq!(stable, after, "a stale generation cannot edit");
    ok(&mut a, "edit_undo", json!({"doc":doc}));
    assert_eq!(ok(&mut a, "object_list", json!({"doc":doc,"page":1}))["objects"], inventory["objects"]);
    ok(&mut a, "edit_redo", json!({"doc":doc}));
    ok(&mut a, "doc_save", json!({"doc":doc,"path":"moved.pdf"}));
    let reopened = ok(&mut a, "doc_open", json!({"path":"moved.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "object_list", json!({"doc":reopened,"page":1}))["objects"], after["objects"]);
    assert!(page_text(&mut a, reopened)[0].contains("Added label"));
}

#[test]
fn object_move_rejects_invalid_nested_references_without_any_change() {
    let dir = workdir("object-move-refusals");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({"path":"a.pdf"}))["doc"].as_u64().unwrap();
    let before = ok(&mut a, "object_list", json!({"doc":doc,"page":1}));
    for objects in [
        json!([]),
        json!([{"kind":"text","index":0}]),
        json!([{"kind":"path","index":1}]),
        json!([{"kind":"text","index":1,"extra":true}]),
        json!([{"kind":"text","index":1.5}]),
        json!([{"kind":"text","index":1},{"kind":"text","index":1}]),
        json!([{"kind":"text","index":1},{"kind":"image","index":99}]),
        json!(vec![json!({"kind":"text","index":1}); 1001]),
    ] {
        assert!(a.call("object_move", &json!({"doc":doc,"page":1,"objects":objects,"offset":[10,20]})).is_err());
        assert_eq!(ok(&mut a, "object_list", json!({"doc":doc,"page":1})), before);
    }
    for offset in [json!([0, 0]), json!([1]), json!([1, 2, 3]), json!(["1", 2]), json!([1e30, 1])] {
        assert!(a.call("object_move", &json!({"doc":doc,"page":1,"objects":[{"kind":"text","index":1}],"offset":offset})).is_err());
        assert_eq!(ok(&mut a, "object_list", json!({"doc":doc,"page":1})), before);
    }
}

#[test]
fn a_deleted_image_is_not_kept_by_reduce_or_optimize() {
    let dir = workdir("deleted-image");
    let (w, h) = (600u32, 400u32);
    let px: Vec<u8> = (0..w * h).flat_map(|i| [(i % w * 255 / w) as u8 ^ (i & 7) as u8, (i / w * 255 / h) as u8, 128]).collect();
    let mut photo = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut photo, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&px).unwrap();
    }
    std::fs::write(dir.join("photo.png"), photo).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_create", json!({ "from": "images", "paths": ["photo.png"] }))["doc"].as_u64().unwrap();
    ok(&mut a, "image_edit", json!({ "doc": doc, "page": 1, "image": 1, "action": "delete" }));
    assert_eq!(ok(&mut a, "page_images", json!({ "doc": doc, "page": 1 }))["count"], 0);
    // The page is blank, so the copies are tiny: the picture's data is not carried along.
    let r = ok(&mut a, "doc_optimize", json!({ "doc": doc, "path": "optimized.pdf" }));
    assert_eq!(r["unused_xobjects"], 1, "{r}");
    assert!(r["bytes_after"].as_u64().unwrap() < 4_000, "{r}");
    let r = ok(&mut a, "doc_reduce", json!({ "doc": doc, "path": "reduced.pdf" }));
    assert!(r["bytes_after"].as_u64().unwrap() < 4_000, "{r}");
    let reopened = ok(&mut a, "doc_open", json!({ "path": "reduced.pdf" }));
    assert_eq!(reopened["pages"], 1);
}

/// A two-page PDF with the same three-row table ("Name Qty / Apple 12 / Pear 7"); the second
/// page is turned with /Rotate 90.
fn table_pdf() -> Vec<u8> {
    let body = "BT /F1 12 Tf 20 250 Td (Name) Tj 130 0 Td (Qty) Tj -130 -20 Td (Apple) Tj 130 0 Td (12) Tj -130 -20 Td (Pear) Tj 130 0 Td (7) Tj ET";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R 5 0 R] /Count 2 /MediaBox [0 0 300 300] >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Rotate 90 /Contents 6 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// Issue #740: `text_extract` with `rect` takes what Column select takes, in the displayed-page
/// coordinates `text_find` reports, on upright and turned pages alike.
#[test]
fn text_extract_with_a_rect_takes_one_column() {
    let dir = workdir("column");
    std::fs::write(dir.join("table.pdf"), table_pdf()).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "table.pdf" }))["doc"].as_u64().unwrap();
    let reading = ok(&mut a, "text_extract", json!({ "doc": doc, "pages": [1] }));
    assert_eq!(reading["pages"][0]["text"], "Name\nApple\nPear\nQty\n12\n7", "reading order is column by column");

    for page in [1u64, 2] {
        // The box around "Qty" and "7", as text_find reports them on this page.
        let rect_of = |a: &mut Automation, q: &str| -> [f64; 4] {
            let found = ok(a, "text_find", json!({ "doc": doc, "query": q }));
            let m = found["matches"].as_array().unwrap().iter().find(|m| m["page"] == page).unwrap().clone();
            let r: Vec<f64> = m["rects"][0].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
            [r[0], r[1], r[2], r[3]]
        };
        let (qty, seven) = (rect_of(&mut a, "Qty"), rect_of(&mut a, "7"));
        let rect = [qty[0].min(seven[0]) - 2.0, qty[1].min(seven[1]) - 2.0, qty[2].max(seven[2]) + 2.0, qty[3].max(seven[3]) + 2.0];
        let column = ok(&mut a, "text_extract", json!({ "doc": doc, "pages": [page], "rect": rect }));
        assert_eq!(column["pages"][0]["text"], "Qty\n12\n7", "page {page}, rect {rect:?}");
    }

    // The whole table, row by row with tab-separated cells (upright page).
    let all = ok(&mut a, "text_extract", json!({ "doc": doc, "pages": [1], "rect": [0, 0, 300, 300] }));
    assert_eq!(all["pages"][0]["text"], "Name\tQty\nApple\t12\nPear\t7");
    // Nothing inside: empty text, not an error.
    let none = ok(&mut a, "text_extract", json!({ "doc": doc, "pages": [1], "rect": [0, 0, 5, 5] }));
    assert_eq!(none["pages"][0]["text"], "");
    // A rect that is not four numbers is refused.
    for bad in [json!([1, 2, 3]), json!("0 0 1 1"), json!([0, 0, "a", 1])] {
        let e = a.call("text_extract", &json!({ "doc": doc, "rect": bad })).unwrap_err();
        assert!(matches!(e, ToolError::InvalidArgs(_)), "{bad}: {e}");
    }
}

#[test]
fn mixed_visual_region_edit_preview_save_reopen_and_undo_through_tools() {
    use pdfcraft_cos::{Dict, Document, Object, Stream, write_incremental};
    let dir = workdir("mixed-region");
    let mut cos = Document::open(std::sync::Arc::new(fixture(1))).unwrap();
    let page = pdfcraft_model::pages(&cos)[0].clone();
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type1"));
    font.set(b"BaseFont".to_vec(), Object::name("Helvetica-Bold"));
    let bold = cos.add(Object::Dict(font));
    let mut resources = cos.resolve(page.dict.get(b"Resources").unwrap()).as_dict().unwrap().clone();
    let mut fonts = cos.resolve(resources.get(b"Font").unwrap()).as_dict().unwrap().clone();
    fonts.set(b"F2".to_vec(), Object::Ref(bold));
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    let stream = cos.add(Object::Stream(Stream::flate(Dict::new(),
        b"BT /F2 1 Tf 18 0 0 18 20 240 Tm (Heading) Tj ET BT /F1 12 Tf 0 g 20 217 Td (Body ) Tj /F2 12 Tf 0 0.2 0.5 rg (bold) Tj /F1 12 Tf 0 g ( text) Tj ET")));
    cos.update_dict(page.obj, |d| {
        d.set(b"Resources".to_vec(), Object::Dict(resources));
        d.set(b"Contents".to_vec(), Object::Ref(stream));
    })
    .unwrap();
    std::fs::write(dir.join("mixed.pdf"), write_incremental(&cos, &Default::default()).unwrap()).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({"path":"mixed.pdf"}))["doc"].as_u64().unwrap();
    let before = ok(&mut a, "text_paragraphs", json!({"doc":doc,"page":1}));
    assert_eq!(before["count"], 1);
    assert_eq!(before["paragraphs"][0]["editor_text"], "Heading\nBody bold text");
    assert_eq!(before["paragraphs"][0]["rows"].as_array().unwrap().len(), 2);
    assert_eq!(before["paragraphs"][0]["lines"].as_array().unwrap().len(), 4);
    let unchanged = ok(&mut a, "text_edit_layout", json!({"doc":doc,"page":1,"paragraph":1}));
    assert_eq!(unchanged["overflow"], false);
    assert_eq!(unchanged["source_rect"], unchanged["draft_rect"]);
    let growing = ok(&mut a, "text_edit_layout", json!({"doc":doc,"page":1,"paragraph":1,"text":"Heading\nBody bold text\nExtra row\nAnother row"}));
    assert_eq!(growing["overflow"], true);
    assert_eq!(ok(&mut a, "text_paragraphs", json!({"doc":doc,"page":1})), before, "measuring does not apply a draft");
    let normal = a.call("page_render", &json!({"doc":doc,"page":1,"dpi":72})).unwrap();
    let preview = a.call("page_render", &json!({"doc":doc,"page":1,"dpi":72,"hide_text_block":1})).unwrap();
    assert_ne!(normal, preview);
    assert_eq!(ok(&mut a, "text_paragraphs", json!({"doc":doc,"page":1})), before, "preview is read-only");
    ok(&mut a, "text_edit", json!({"doc":doc,"page":1,"paragraph":1,"text":"Heading\nText bold text"}));
    ok(&mut a, "doc_save", json!({"doc":doc,"path":"out/mixed-edited.pdf"}));
    let re = ok(&mut a, "doc_open", json!({"path":"out/mixed-edited.pdf"}))["doc"].as_u64().unwrap();
    let after = ok(&mut a, "text_paragraphs", json!({"doc":re,"page":1}));
    assert_eq!(after["count"], 1);
    assert_eq!(after["paragraphs"][0]["editor_text"], "Heading\nText bold text");
    let saved = Document::open(std::sync::Arc::new(std::fs::read(dir.join("out/mixed-edited.pdf")).unwrap())).unwrap();
    let mut session = pdfcraft_engine::Session::new();
    let id = session.open("saved.pdf", None, std::sync::Arc::new(write_incremental(&saved, &Default::default()).unwrap()), None).unwrap();
    let runs = session.get(id).unwrap().text_lines(0);
    let preserved = runs.iter().find(|l| l.text == "bold").unwrap();
    assert!(preserved.bold);
    assert_eq!(preserved.color, [0.0, 0.2, 0.5]);
    ok(&mut a, "text_edit", json!({"doc":re,"page":1,"paragraph":1,"text":""}));
    assert_eq!(ok(&mut a, "text_paragraphs", json!({"doc":re,"page":1}))["count"], 0);
    ok(&mut a, "edit_undo", json!({"doc":re}));
    assert_eq!(ok(&mut a, "text_paragraphs", json!({"doc":re,"page":1})), after);
}

#[test]
fn clipped_image_bounds_and_text_edit_save_reopen_through_tools() {
    use pdfcraft_cos::{Dict, Document, Object, Stream, write_incremental};
    let dir = workdir("clipped-artwork");
    let mut cos = Document::open(std::sync::Arc::new(fixture(1))).unwrap();
    let page = pdfcraft_model::pages(&cos)[0].clone();
    let mut image = Dict::new();
    for (key, value) in [
        ("Type", Object::name("XObject")),
        ("Subtype", Object::name("Image")),
        ("Width", Object::Int(1)),
        ("Height", Object::Int(1)),
        ("BitsPerComponent", Object::Int(8)),
        ("ColorSpace", Object::name("DeviceRGB")),
    ] {
        image.set(key.as_bytes().to_vec(), value);
    }
    let image = cos.add(Object::Stream(Stream::flate(image, &[220, 235, 245])));
    let mut resources = cos.resolve(page.dict.get(b"Resources").unwrap()).as_dict().unwrap().clone();
    let mut xo = Dict::new();
    xo.set(b"Im0".to_vec(), Object::Ref(image));
    resources.set(b"XObject".to_vec(), Object::Dict(xo));
    let contents = cos.add(Object::Stream(Stream::flate(
        Dict::new(),
        b"q 30 170 130 80 re W* n q 800 0 0 900 -300 -400 cm /Im0 Do Q BT /F1 12 Tf 30 195 Td (Shared caption) Tj ET Q BT /F1 12 Tf 30 130 Td (Editable caption) Tj ET",
    )));
    cos.update_dict(page.obj, |d| {
        d.set(b"Resources".to_vec(), Object::Dict(resources));
        d.set(b"Contents".to_vec(), Object::Ref(contents));
    })
    .unwrap();
    std::fs::write(dir.join("clipped.pdf"), write_incremental(&cos, &Default::default()).unwrap()).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({"path":"clipped.pdf"}))["doc"].as_u64().unwrap();
    let pixels = a.call("page_render", &json!({"doc":doc,"page":1,"dpi":72})).unwrap();
    let images = ok(&mut a, "page_images", json!({"doc":doc,"page":1}));
    assert_eq!(ok(&mut a, "image_hit", json!({"doc":doc,"page":1,"point":[50,60]}))["images"], json!([1]));
    assert_eq!(ok(&mut a, "image_hit", json!({"doc":doc,"page":1,"point":[20,60]}))["images"], json!([]));
    assert_eq!(images["images"][0]["visible_rect"], json!([30.0, 50.0, 160.0, 130.0]));
    assert_eq!(images["images"][0]["rect"], json!([-300.0, -200.0, 500.0, 700.0]), "raw placement remains available for transforms");
    let objects = ok(&mut a, "object_list", json!({"doc":doc,"page":1}));
    let object = objects["objects"].as_array().unwrap().iter().find(|o| o["kind"] == "image").unwrap();
    assert_eq!(object["rect"], images["images"][0]["visible_rect"]);
    assert_eq!(a.call("page_render", &json!({"doc":doc,"page":1,"dpi":72})).unwrap(), pixels, "inventory does not change rendering");
    let labels = ok(&mut a, "text_paragraphs", json!({"doc":doc,"page":1}));
    ok(&mut a, "image_edit", json!({"doc":doc,"page":1,"image":1,"action":"move","rect":[-432.0,-300.0,688.0,960.0]}));
    let resized = ok(&mut a, "page_images", json!({"doc":doc,"page":1}));
    assert_eq!(
        resized["images"][0]["visible_rect"],
        json!([30.0, 50.0, 160.0, 130.0]),
        "a clip shared with the caption stays where it is (#1049 moves only an image's own clip)"
    );
    assert_eq!(ok(&mut a, "text_paragraphs", json!({"doc":doc,"page":1})), labels);
    // The one-colour image still fills the fixed shared clip, so the page looks the same.
    let resized_pixels = a.call("page_render", &json!({"doc":doc,"page":1,"dpi":72})).unwrap();
    ok(&mut a, "doc_save", json!({"doc":doc,"path":"out/resized.pdf"}));
    let resized_re = ok(&mut a, "doc_open", json!({"path":"out/resized.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "page_images", json!({"doc":resized_re,"page":1})), resized);
    assert_eq!(a.call("page_render", &json!({"doc":resized_re,"page":1,"dpi":72})).unwrap(), resized_pixels);
    ok(&mut a, "edit_undo", json!({"doc":doc}));
    assert_eq!(a.call("page_render", &json!({"doc":doc,"page":1,"dpi":72})).unwrap(), pixels);
    ok(&mut a, "text_edit", json!({"doc":doc,"page":1,"paragraph":2,"text":"Edited caption"}));
    ok(&mut a, "doc_save", json!({"doc":doc,"path":"out/edited.pdf"}));
    let re = ok(&mut a, "doc_open", json!({"path":"out/edited.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "text_paragraphs", json!({"doc":re,"page":1}))["paragraphs"][1]["editor_text"], "Edited caption");
    assert_eq!(ok(&mut a, "page_images", json!({"doc":re,"page":1})), images, "image placement and crop survive text editing");
    ok(&mut a, "edit_undo", json!({"doc":doc}));
    assert_eq!(a.call("page_render", &json!({"doc":doc,"page":1,"dpi":72})).unwrap(), pixels);
}

/// Independent inch/mm dimensions, checked in the saved file through the tool path.
#[test]
fn architectural_and_iso_papers_through_tools() {
    let dir = workdir("plotter-papers");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let mm = 72.0 / 25.4;
    for (name, w, h) in [
        ("ARCH A", 9.0 * 72.0, 12.0 * 72.0),
        ("arch b", 12.0 * 72.0, 18.0 * 72.0),
        ("ARCH C", 18.0 * 72.0, 24.0 * 72.0),
        ("ARCH D", 24.0 * 72.0, 36.0 * 72.0),
        ("ARCH E1", 30.0 * 72.0, 42.0 * 72.0),
        ("ARCH E (36 x 48 in)", 36.0 * 72.0, 48.0 * 72.0),
        ("ANSI C", 17.0 * 72.0, 22.0 * 72.0),
        ("ANSI D", 22.0 * 72.0, 34.0 * 72.0),
        ("ANSI E", 34.0 * 72.0, 44.0 * 72.0),
        ("A2", 420.0 * mm, 594.0 * mm),
        ("A1", 594.0 * mm, 841.0 * mm),
        ("A0", 841.0 * mm, 1189.0 * mm),
        ("A6", 105.0 * mm, 148.0 * mm),
        ("Letter", 612.0, 792.0),
        ("US Legal", 612.0, 1008.0),
        ("Tabloid", 792.0, 1224.0),
    ] {
        for orientation in ["portrait", "landscape"] {
            let out = ok(
                &mut a,
                "doc_print",
                json!({"doc": doc, "pages": "1", "paper": name,
                "layout": "actual", "orientation": orientation, "path": "plot.pdf"}),
            );
            assert_eq!(out["sheets"], 1);
            let bytes = std::fs::read(dir.join("plot.pdf")).unwrap();
            let saved = pdfcraft_cos::Document::open(std::sync::Arc::new(bytes)).unwrap();
            let pages = pdfcraft_model::pages(&saved);
            assert_eq!(pages.len(), 1);
            let (ew, eh) = if orientation == "landscape" { (h, w) } else { (w, h) };
            let (aw, ah) = pages[0].display_size(&saved);
            assert!((aw - ew).abs() < 0.02 && (ah - eh).abs() < 0.02, "{name} {orientation}: {aw} x {ah}");
        }
    }
    let before = std::fs::read(dir.join("plot.pdf")).unwrap();
    let bad = a.call("doc_print", &json!({"doc": doc, "paper": "ARCH Z", "path": "plot.pdf"}));
    assert!(matches!(bad, Err(ToolError::InvalidArgs(m)) if m.contains("ARCH E1") && m.contains("A0")));
    assert_eq!(std::fs::read(dir.join("plot.pdf")).unwrap(), before);
    assert!(a.call("doc_print", &json!({"doc": doc, "paper": "ARCH D", "path": "../escape.pdf"})).is_err());
}

/// A 2-page 200×300 pt PDF in the `fixture()` style for the reading-colours test: bold black
/// "Page N" text on a white page; page 2 also draws a 4×4 uncompressed `/DeviceRGB` image,
/// scaled to 40×40 pt at 120, 200.
fn reading_colors_fixture() -> Vec<u8> {
    let body1 = "BT /F1 48 Tf 20 150 Td (Page 1) Tj ET";
    let body2 = "BT /F1 48 Tf 20 150 Td (Page 2) Tj ET\nq 40 0 0 40 120 200 cm /Im1 Do Q";
    // 16 opaque pure-red pixels; images are never remapped by the reading-colours setting.
    let image: Vec<u8> = (0..16).flat_map(|_| [0xff_u8, 0x00, 0x00]).collect();
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 300] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 7 0 R >> >> >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font << /F1 7 0 R >> /XObject << /Im1 8 0 R >> >> >>".to_vec(),
        format!("<< /Length {} >>\nstream\n{body1}\nendstream", body1.len()).into_bytes(),
        format!("<< /Length {} >>\nstream\n{body2}\nendstream", body2.len()).into_bytes(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>".to_vec(),
    ];
    let mut image_obj = format!(
        "<< /Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length {} >>\nstream\n",
        image.len()
    )
    .into_bytes();
    image_obj.extend_from_slice(&image);
    image_obj.extend_from_slice(b"\nendstream");
    objs.push(image_obj);
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// A fresh directory whose `a.pdf` is `reading_colors_fixture()`.
fn reading_colors_workdir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-automation-reading-colors-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.pdf"), reading_colors_fixture()).unwrap();
    dir
}

/// Decodes a `page_render` PNG (pages here are 200×300 pt) into RGBA pixels.
fn png_pixels(content: &Content) -> Vec<u8> {
    let Content::Png { data, width, height } = content else { panic!("expected a PNG") };
    assert_eq!((*width, *height), (200, 300));
    let mut reader = png::Decoder::new(std::io::Cursor::new(data)).read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    assert_eq!((info.width, info.height), (200, 300));
    pixels
}

/// The RGBA pixel at (`x`, `y`), top-left origin, of a 200-wide image.
fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * 200 + x) * 4) as usize;
    pixels[at..at + 4].try_into().unwrap()
}

/// The `view_reading_colors` tool changes what `page_render` draws — text and background remap,
/// images never do — without touching the file, and its errors name what's wrong (#897).
#[test]
fn reading_colors_through_tools() {
    let dir = reading_colors_workdir();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let render = |a: &mut Automation, page: u64| png_pixels(&a.call("page_render", &json!({ "doc": doc, "page": page, "dpi": 72 })).unwrap()[0]);

    // Off: black text on a white page.
    let page1_off = render(&mut a, 1);
    let page2_off = render(&mut a, 2);
    assert_eq!(pixel(&page1_off, 10, 290), [255, 255, 255, 255]);

    let echo = ok(&mut a, "view_reading_colors", json!({ "doc": doc, "mode": "white-on-black" }));
    assert_eq!(echo, json!({ "mode": "white-on-black", "text": "#ffffff", "background": "#000000", "only_black": false, "line_art": true }));

    // The page clears to the setting's background, and the text paints with its colour.
    let page1_on = render(&mut a, 1);
    assert_eq!(pixel(&page1_on, 10, 290), [0, 0, 0, 255], "background clears to black");
    // The brightest pixel inside the text band is a stroke interior: a 48 pt bold stem covers it.
    let (tx, ty) = (20..172u32).flat_map(|x| (116..160u32).map(move |y| (x, y))).max_by_key(|&(x, y)| u32::from(pixel(&page1_on, x, y)[0])).unwrap();
    let on = pixel(&page1_on, tx, ty);
    assert!(on[0] >= 250, "a text-interior pixel renders white: {on:?}");
    let off = pixel(&page1_off, tx, ty);
    assert!(off[0] <= 60, "the same pixel is black ink without the setting: {off:?}");

    // Images are never remapped: page 2's picture draws byte-identically in both modes.
    let page2_on = render(&mut a, 2);
    fn image_row(px: &[u8], y: u32) -> &[u8] {
        let at = ((y * 200 + 122) * 4) as usize;
        &px[at..at + 36 * 4]
    }
    assert_eq!(pixel(&page2_on, 140, 80), [255, 0, 0, 255], "the image keeps its colour");
    for y in 62..98u32 {
        assert_eq!(image_row(&page2_on, y), image_row(&page2_off, y), "image row {y}");
    }

    // Errors name the problem and the allowed values.
    let missing = a.call("view_reading_colors", &json!({ "doc": doc, "mode": "custom", "text": "#00ff00" }));
    assert!(matches!(&missing, Err(ToolError::InvalidArgs(m)) if m.contains("background")), "{missing:?}");
    let unknown = a.call("view_reading_colors", &json!({ "doc": doc, "mode": "nope" }));
    assert!(matches!(&unknown, Err(ToolError::InvalidArgs(m)) if m.contains("off") && m.contains("custom")), "{unknown:?}");
    let no_mode = a.call("view_reading_colors", &json!({ "doc": doc }));
    assert!(matches!(&no_mode, Err(ToolError::InvalidArgs(m)) if m.contains("mode")), "{no_mode:?}");

    // Back off: the render matches the original one again.
    assert_eq!(
        ok(&mut a, "view_reading_colors", json!({ "doc": doc, "mode": "off" })),
        json!({ "mode": "off", "text": null, "background": null, "only_black": false, "line_art": false })
    );
    assert_eq!(render(&mut a, 1), page1_off);
}

#[test]
fn private_crops_with_nested_state_resize_save_reopen_through_tools() {
    use pdfcraft_cos::{Dict, Document, Object, Stream, write_incremental};
    let dir = workdir("private-crop-state");
    let mut cos = Document::open(std::sync::Arc::new(fixture(1))).unwrap();
    let page = pdfcraft_model::pages(&cos)[0].clone();
    let mut image = Dict::new();
    for (key, value) in [
        ("Type", Object::name("XObject")),
        ("Subtype", Object::name("Image")),
        ("Width", Object::Int(1)),
        ("Height", Object::Int(1)),
        ("BitsPerComponent", Object::Int(8)),
        ("ColorSpace", Object::name("DeviceRGB")),
    ] {
        image.set(key.as_bytes().to_vec(), value);
    }
    let image = cos.add(Object::Stream(Stream::flate(image, &[220, 235, 245])));
    let mut resources = cos.resolve(page.dict.get(b"Resources").unwrap()).as_dict().unwrap().clone();
    let mut xo = Dict::new();
    xo.set(b"Im0".to_vec(), Object::Ref(image));
    resources.set(b"XObject".to_vec(), Object::Dict(xo));
    let parts: [&[u8]; 3] =
        [b"q 30 170 130 80 re W*", b"n 0.5 g q 800 0 0 900 -300 -400 cm /Im0 Do", b"Q Q BT /F1 12 Tf 30 130 Td (Neighbour caption) Tj ET"];
    let refs = parts.iter().map(|b| Object::Ref(cos.add(Object::Stream(Stream::flate(Dict::new(), b))))).collect();
    cos.update_dict(page.obj, |d| {
        d.set(b"Resources".to_vec(), Object::Dict(resources));
        d.set(b"Contents".to_vec(), Object::Array(refs));
    })
    .unwrap();
    std::fs::write(dir.join("crop.pdf"), write_incremental(&cos, &Default::default()).unwrap()).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({"path":"crop.pdf"}))["doc"].as_u64().unwrap();
    let original = ok(&mut a, "page_images", json!({"doc":doc,"page":1}));
    let labels = ok(&mut a, "text_paragraphs", json!({"doc":doc,"page":1}));
    let pixels = a.call("page_render", &json!({"doc":doc,"page":1,"dpi":72})).unwrap();
    for factor in [1.2, 0.7] {
        let rect = [30.0 + (-300.0 - 30.0) * factor, 50.0 + (-200.0 - 50.0) * factor, 30.0 + (500.0 - 30.0) * factor, 50.0 + (700.0 - 50.0) * factor];
        ok(&mut a, "image_edit", json!({"doc":doc,"page":1,"image":1,"action":"move","rect":rect}));
        let images = ok(&mut a, "page_images", json!({"doc":doc,"page":1}));
        let expected = [30.0, 50.0, 30.0 + 130.0 * factor, 50.0 + 80.0 * factor];
        for (actual, expected) in images["images"][0]["visible_rect"].as_array().unwrap().iter().zip(expected) {
            assert!((actual.as_f64().unwrap() - expected).abs() < 0.01);
        }
        assert_eq!(images["images"][0]["pixels"], original["images"][0]["pixels"]);
        assert_eq!(ok(&mut a, "text_paragraphs", json!({"doc":doc,"page":1})), labels);
        let resized = a.call("page_render", &json!({"doc":doc,"page":1,"dpi":72})).unwrap();
        assert_ne!(resized, pixels, "the actual crop changes size");
        ok(&mut a, "doc_save", json!({"doc":doc,"path":"out/resized.pdf"}));
        let re = ok(&mut a, "doc_open", json!({"path":"out/resized.pdf"}))["doc"].as_u64().unwrap();
        assert_eq!(ok(&mut a, "page_images", json!({"doc":re,"page":1})), images);
        assert_eq!(a.call("page_render", &json!({"doc":re,"page":1,"dpi":72})).unwrap(), resized);
        ok(&mut a, "edit_undo", json!({"doc":doc}));
        assert_eq!(a.call("page_render", &json!({"doc":doc,"page":1,"dpi":72})).unwrap(), pixels);
        assert_eq!(ok(&mut a, "page_images", json!({"doc":doc,"page":1})), original);
    }
}

#[test]
fn absolute_gotor_consent_does_not_bypass_validation_or_passwords() {
    let (base, root) = sandbox("gotor-absolute-consent");
    std::fs::create_dir_all(root.join("folder.pdf")).unwrap();
    std::fs::write(root.join("notes.txt"), fixture(1)).unwrap();
    std::fs::write(root.join("encrypted.pdf"), protected_fixture("secret")).unwrap();
    link_dir(&root, "outside-link", &base.join("outside"));
    let mut a = auto(&root);
    for (name, target, reason) in [
        ("missing", root.join("missing.pdf"), "missing.pdf"),
        ("folder", root.join("folder.pdf"), "not a regular file"),
        ("text", root.join("notes.txt"), "not a .pdf file"),
        ("outside", base.join("outside/secret.pdf"), "outside"),
        ("symlink", root.join("outside-link/secret.pdf"), "outside"),
    ] {
        let path = format!("{name}-source.pdf");
        std::fs::write(root.join(&path), remote_link_pdf(target.to_str().unwrap(), Some("[0 /Fit]"))).unwrap();
        let doc = ok(&mut a, "doc_open", json!({ "path": path }))["doc"].as_u64().unwrap();
        let before = a.session().docs().len();
        // The absence of a target or a root escape must not be probed before consent.
        assert!(
            matches!(err(&mut a, "link_open", json!({ "doc": doc, "page": 1, "index": 1 })), ToolError::Failed(m) if m.contains("allow_absolute"))
        );
        assert!(
            matches!(err(&mut a, "link_open", json!({ "doc": doc, "page": 1, "index": 1, "allow_absolute": true })), ToolError::Failed(m) if m.contains(reason)),
            "{name}"
        );
        assert_eq!(a.session().docs().len(), before);
    }
    std::fs::write(root.join("password-source.pdf"), remote_link_pdf(root.join("encrypted.pdf").to_str().unwrap(), Some("[0 /Fit]"))).unwrap();
    let doc = ok(&mut a, "doc_open", json!({ "path": "password-source.pdf" }))["doc"].as_u64().unwrap();
    let before = a.session().docs().len();
    for password in ["", "wrong"] {
        assert!(matches!(
            err(&mut a, "link_open", json!({ "doc": doc, "page": 1, "index": 1, "allow_absolute": true, "password": password })),
            ToolError::Failed(_)
        ));
        assert_eq!(a.session().docs().len(), before);
    }
    let opened = ok(&mut a, "link_open", json!({ "doc": doc, "page": 1, "index": 1, "allow_absolute": true, "password": "secret" }));
    assert_eq!(opened["document"]["encrypted"], true);
    assert_eq!(opened["destination_applied"], true);
}

#[test]
fn finding_untagged_content_limits_results_to_content_annotations_and_widgets() {
    let dir = workdir("find-untagged");
    std::fs::write(dir.join("field.pdf"), form_pdf("name")).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "field.pdf" }))["doc"].as_u64().unwrap();
    let found = ok(&mut a, "accessibility_find_untagged", json!({ "doc": doc }));
    let active: Vec<&str> = found["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|result| result["status"] != "skipped")
        .map(|result| result["rule"].as_str().unwrap())
        .collect();
    assert_eq!(active, ["tagged-content", "tagged-annotations", "tagged-form-fields"]);
    let fields = found["results"].as_array().unwrap().iter().find(|result| result["rule"] == "tagged-form-fields").unwrap();
    assert_eq!(fields["status"], "failed");
    assert_eq!(fields["findings"][0]["page"], 1);
}

#[test]
fn a_missing_path_under_the_root_names_where_it_was_looked_for() {
    // #1890: a script written from another working directory repeats the root's folder in its
    // paths. The refusal names the resolved path and the root, so there is something to correct.
    let (base, root) = sandbox("resolved-path");
    let mut a = auto(&root);
    let looked = |rel: &str| root.join(rel).display().to_string();
    for (path, rel) in
        [("root/inside.pdf", "root/inside.pdf"), ("missing/deeper/x.pdf", "missing/deeper/x.pdf"), ("./sub/../root/inside.pdf", "root/inside.pdf")]
    {
        let ToolError::Failed(m) = err(&mut a, "doc_open", json!({ "path": path })) else { panic!("{path}: not a failure") };
        assert_eq!(m, format!("{path} not found (looked for {}, --root {})", looked(rel), root.display()));
    }
    // The same mistake with a file that does exist outside still gets the plain refusal (#136).
    let ToolError::Failed(m) = err(&mut a, "doc_open", json!({ "path": "../outside/secret.pdf" })) else { panic!("not a failure") };
    assert!(!m.contains("looked for"), "{m}");
    ok(&mut a, "doc_open", json!({ "path": "inside.pdf" }));
    let _ = std::fs::remove_dir_all(base);
}

/// `doc_print` prints as image: pictures at `dpi`, grey with `grayscale`; a resolution outside
/// 72–600 is refused.
#[test]
fn doc_print_as_image() {
    let dir = workdir("print-image");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let r = ok(&mut a, "doc_print", json!({ "doc": doc, "pages": "1", "as_image": true, "dpi": 150, "grayscale": true, "path": "image.pdf" }));
    assert_eq!(r["sheets"], 1);
    let bytes = std::fs::read(dir.join("image.pdf")).unwrap();
    let out = pdfcraft_cos::Document::open(std::sync::Arc::new(bytes)).unwrap();
    let pictures =
        out.scan_objects().filter(|(_, o)| matches!(&**o, pdfcraft_cos::Object::Stream(st) if st.dict.name(b"Subtype") == Some(b"Image"))).count();
    assert_eq!(pictures, 1);
    match a.call("doc_print", &json!({ "doc": doc, "as_image": true, "dpi": 20, "path": "x.pdf" })) {
        Err(ToolError::InvalidArgs(m)) => assert!(m.contains("dpi"), "{m}"),
        other => panic!("{other:?}"),
    }
}

/// Without a root, `write_output` still writes through a staging file (#2166): the folder is
/// made, nothing is left beside the file, and a folder at the path is refused.
#[test]
fn a_root_less_write_output_is_atomic_too() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-automation-rootless-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let free = Automation::new();
    let target = dir.join("new").join("page.png");
    let written = free.write_output(&target.to_string_lossy(), &[1, 2, 3]).unwrap();
    assert_eq!(written, target);
    assert_eq!(std::fs::read(&target).unwrap(), [1, 2, 3]);
    let names: Vec<_> = std::fs::read_dir(dir.join("new")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["page.png"], "no staging file left beside it");
    let e = free.write_output(&dir.join("new").to_string_lossy(), &[4]).unwrap_err();
    assert!(matches!(&e, ToolError::Failed(m) if m.contains("is a folder")), "{e:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
