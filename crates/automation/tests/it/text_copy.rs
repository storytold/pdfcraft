//! `text_extract` and `text_find` on a page of the cases text copy has to get right: words drawn out
//! of order, runs of different sizes on one baseline, a ligature glyph named in the encoding and a
//! line turned upwards in a row of text.

use std::path::PathBuf;

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// A fresh directory for one test.
fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pdfcraft-text-copy-{}-{name}", std::process::id()));
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

/// One page of labelled rows (`/F1` Helvetica, `/F2` Helvetica with code 128 named `/fi`):
/// 01 `of<fi>ce`; 06 `exam-` over `ple`; 07 two columns of two lines; 08 "second" drawn before
/// "first", to its right; 11 a 14 pt invisible word and a 9 pt note on one baseline; 12 letter
/// spacing; 13 "up" set upwards; 14 two words placed by `Td`.
fn copy_pdf() -> Vec<u8> {
    let rows = [
        ("01", "BT /F2 14 Tf 90 {Y} Td (of\\200ce) Tj ET"),
        ("06", "BT /F1 14 Tf 90 {Y} Td (exam-) Tj 0 -16 Td (ple) Tj ET"),
        ("07", "BT /F1 12 Tf 90 {Y} Td (left one) Tj 0 -14 Td (left two) Tj ET BT /F1 12 Tf 300 {Y} Td (right one) Tj 0 -14 Td (right two) Tj ET"),
        ("08", "BT /F1 14 Tf 160 {Y} Td (second) Tj ET BT /F1 14 Tf 90 {Y} Td (first) Tj ET"),
        ("11", "BT /F1 14 Tf 3 Tr 90 {Y} Td (hidden) Tj ET BT /F1 9 Tf 0.5 g 160 {Y} Td (<- invisible word here) Tj ET"),
        ("12", "BT /F1 14 Tf 8 Tc 90 {Y} Td (SPACED) Tj ET"),
        ("13", "BT /F1 14 Tf 0 1 -1 0 110 {Y} Tm (up) Tj ET"),
        ("14", "BT /F1 14 Tf 90 {Y} Td (split) Tj 40 0 Td (words) Tj ET"),
    ];
    let mut body = String::new();
    let mut y = 740;
    for (tag, row) in rows {
        body += &format!("BT /F1 9 Tf 0.4 g 40 {y} Td ({tag}) Tj ET 0 g\nq {} Q\n", row.replace("{Y}", &y.to_string()));
        y -= match tag {
            "06" | "13" => 48,
            "07" => 44,
            _ => 38,
        };
    }
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding << /Type /Encoding /BaseEncoding /WinAnsiEncoding /Differences [128 /fi] >> >>"
            .to_string(),
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

/// Copy keeps a baseline on one line in left-to-right order, columns column by column, the
/// ligature as its letters and the turned line in its row; Find matches what Copy reads, and its
/// rectangles cover the glyphs it names.
#[test]
fn text_extract_and_find_read_one_line_per_baseline() {
    let dir = tmp("baseline");
    std::fs::write(dir.join("copy.pdf"), copy_pdf()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = ok(&mut a, "doc_open", json!({ "path": "copy.pdf" }))["doc"].as_u64().unwrap();
    let text = ok(&mut a, "text_extract", json!({ "doc": doc }));
    assert_eq!(
        text["pages"][0]["text"],
        "01 office\n06 exam-\nple\n07 left one\nleft two\nright one\nright two\n08 first second\n11 hidden <- invisible word here\n12 SPACED\n13\nup\n14 split words"
    );
    let find = |a: &mut Automation, q: &str| ok(a, "text_find", json!({ "doc": doc, "query": q }));
    let first_second = find(&mut a, "first second");
    assert_eq!(first_second["count"], 1);
    assert_eq!(first_second["matches"][0]["text"], "first second");
    // One rectangle across both words, from the left edge of "first" (x = 90) past the right edge of "second".
    let rects = first_second["matches"][0]["rects"].as_array().unwrap();
    assert_eq!(rects.len(), 1, "{rects:?}");
    let x = |k: usize| rects[0][k].as_f64().unwrap();
    assert!((x(0) - 90.0).abs() < 0.5 && x(2) > 200.0, "{rects:?}");
    let office = find(&mut a, "office");
    assert_eq!(office["count"], 1);
    assert_eq!(office["matches"][0]["text"], "office");
    assert_eq!(find(&mut a, "hidden <- invisible")["count"], 1);
    assert_eq!(find(&mut a, "left one right one")["count"], 0, "the columns stay apart");
    let _ = std::fs::remove_dir_all(&dir);
}
