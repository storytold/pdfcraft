//! Decisive regression 8: a tool sees the redaction areas applying removes. `comment_list` and
//! `doc_info` give each Redact mark's exact user-space areas, or its typed refusal; `redact_apply`
//! removes exactly that, or refuses with the reason. The mark here has a null first corner, which
//! the comment list's own quads read as 0: the areas must not.
use pdfcraft_automation::{Automation, Content, ToolError};
use serde_json::{Value, json};

/// `AB1234CD` in 10 pt Courier at (10, 100) (AB is x 10–22, 1234 is 22–46), with `annots` listed
/// as objects 6, 7, …, then `extras` (not listed).
fn pdf(annots: &[&str], extras: &[&str]) -> Vec<u8> {
    let body = "BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET";
    let list: String = (0..annots.len()).map(|i| format!("{} 0 R ", i + 6)).collect();
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>".to_owned(),
        format!("<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 4 0 R >> >> /Annots [{list}] >>"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
    ];
    objects.extend(annots.iter().chain(extras).map(|s| (*s).to_owned()));
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
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

fn setup(test: &str, annots: &[&str]) -> (std::path::PathBuf, Automation, Value) {
    setup_with(test, annots, &[])
}

fn setup_with(test: &str, annots: &[&str], extras: &[&str]) -> (std::path::PathBuf, Automation, Value) {
    let dir = std::env::temp_dir().join(format!("pdfcraft-redaction-areas-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("in.pdf"), pdf(annots, extras)).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = ok(&mut a, "doc_open", json!({ "path": "in.pdf" }))["doc"].clone();
    (dir, a, doc)
}

fn text(a: &mut Automation, doc: &Value) -> String {
    ok(a, "text_extract", json!({ "doc": doc }))["pages"][0]["text"].as_str().unwrap_or_default().to_owned()
}

#[test]
fn tools_see_the_areas_applying_removes() {
    let mark = "<< /Type /Annot /Subtype /Redact /Rect [22 95 46 110] /QuadPoints [null 110 46 110 22 95 46 95] >>";
    let (dir, mut a, doc) = setup("ready", &[mark]);
    let comments = ok(&mut a, "comment_list", json!({ "doc": doc }));
    let r = &comments["comments"][0]["redaction"];
    assert_eq!(r["status"], "ready", "{r}");
    assert_eq!(r["user_areas"], json!([[22.0, 95.0, 46.0, 110.0]]), "exact, in user space, not [0 95 46 110]");
    assert_eq!((r["uses_rect"].clone(), r["quad_issues"]["invalid_groups"].clone()), (json!(true), json!(1)));
    assert_eq!((r["units"].clone(), r["user_unit"].clone()), (json!("PDF default user space units of the page, [x0, y0, x1, y1]"), json!(1.0)));
    let info = ok(&mut a, "doc_info", json!({ "doc": doc }));
    assert_eq!(info["redactions"]["status"], "available");
    assert_eq!(info["redactions"]["marks"][0]["user_areas"], r["user_areas"]);
    assert_eq!((info["redactions"]["marks"][0]["page"].clone(), info["redactions"]["marks"][0]["index"].clone()), (json!(1), json!(1)));
    ok(&mut a, "redact_apply", json!({ "doc": doc }));
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "out.pdf", "full": true }));
    let saved = ok(&mut a, "doc_open", json!({ "path": "out.pdf" }))["doc"].clone();
    let after = text(&mut a, &saved);
    assert!(!after.contains("1234") && after.contains("AB") && after.contains("CD"), "{after:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tools_see_a_refusal_and_why() {
    let mixed = "<< /Type /Annot /Subtype /Redact /QuadPoints [22 110 28 110 22 95 28 95 null 1 2 3 4 5 6 7] >>";
    let (dir, mut a, doc) = setup("refused", &[mixed]);
    let comments = ok(&mut a, "comment_list", json!({ "doc": doc }));
    let r = &comments["comments"][0]["redaction"];
    assert_eq!((r["status"].clone(), r["refusal"].clone()), (json!("refused"), json!("quad_needs_rect")), "{r}");
    assert_eq!((r["user_areas"].clone(), r["locator"].clone()), (json!([]), Value::Null));
    assert!(r["reason"].as_str().unwrap().starts_with("Cannot apply this redaction"), "{r}");
    let info = ok(&mut a, "doc_info", json!({ "doc": doc }));
    assert_eq!(info["redactions"]["refused"], 1);
    let Err(ToolError::Failed(reason)) = a.call("redact_apply", &json!({ "doc": doc })) else {
        panic!("redact_apply must refuse");
    };
    assert!(reason.contains("1 redaction mark(s) have no readable area") && reason.contains("nothing was redacted"), "{reason}");
    assert!(text(&mut a, &doc).contains("AB1234CD"), "the page is unchanged");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Through the tools: a mark whose /Subtype follows 34 references (past the bound), over AB, beside
/// an ordinary mark over 1234. Editing the ordinary mark's comment used to make the view ready
/// (the comment list, read from the graph, lost the first mark) and redact_apply left AB. It now
/// stays refused before and after the edit, and the page keeps its text.
#[test]
fn tools_keep_a_mark_whose_type_cant_be_followed_refused() {
    // Only the two marks are listed; the fill (object 8) and the chain (9…) are other objects.
    let annots =
        ["<< /Type /Annot /Subtype 9 0 R /Rect [8 95 22 110] /AP << /N 8 0 R >> >>", "<< /Type /Annot /Subtype /Redact /Rect [22 95 46 110] >>"];
    let mut extras = vec!["<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 16 >>\nstream\n0 g 0 0 1 1 re f\nendstream".to_owned()];
    for i in 0..34 {
        extras.push(if i == 33 { "/Redact".into() } else { format!("{} 0 R", 10 + i) });
    }
    let extras: Vec<&str> = extras.iter().map(String::as_str).collect();
    let (dir, mut a, doc) = setup_with("unresolved-type", &annots, &extras);
    // The view is available (the page draws), and the first mark is refused as a mark whose type
    // can't be read: not a display failure.
    let refused = |info: &Value| {
        info["redactions"]["status"] == "available"
            && info["redactions"]["marks"]
                .as_array()
                .is_some_and(|m| m.iter().any(|m| m["index"] == 1 && m["status"] == "refused" && m["refusal"] == "unresolved_type"))
    };
    let before = ok(&mut a, "doc_info", json!({ "doc": doc }));
    assert!(refused(&before), "{}", before["redactions"]);
    assert!(a.call("redact_apply", &json!({ "doc": doc })).is_err());
    ok(&mut a, "comment_edit", json!({ "doc": doc, "page": 1, "index": 2, "contents": "refresh" }));
    let after = ok(&mut a, "doc_info", json!({ "doc": doc }));
    assert!(refused(&after), "{}", after["redactions"]);
    assert!(a.call("redact_apply", &json!({ "doc": doc })).is_err());
    assert!(text(&mut a, &doc).contains("AB1234CD"), "the page is unchanged");
    let _ = std::fs::remove_dir_all(&dir);
}
