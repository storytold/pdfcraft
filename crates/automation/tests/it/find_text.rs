//! `text_find` on a ligature glyph named in the encoding, on a word divided by a hyphen at a
//! line end, and on halfwidth and fullwidth forms.

use std::path::PathBuf;

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// A fresh directory for one test.
fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pdfcraft-find-text-{}-{name}", std::process::id()));
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

/// One page: `of<fi>ce` in a Helvetica whose code 128 is named `/fi`, and `exam-` at the end of
/// one line with `ple` starting the next.
fn find_pdf() -> Vec<u8> {
    let body = "BT /F2 14 Tf 90 700 Td (of\\200ce) Tj ET\nBT /F1 14 Tf 90 650 Td (exam-) Tj 0 -16 Td (ple) Tj ET\n";
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

/// "office" finds the ligature, and "example" both halves of the divided word, highlighted line by
/// line; the hyphenated form is still found as written.
#[test]
fn text_find_matches_ligatures_and_words_divided_at_a_line_end() {
    let dir = tmp("forms");
    std::fs::write(dir.join("find.pdf"), find_pdf()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = ok(&mut a, "doc_open", json!({ "path": "find.pdf" }))["doc"].as_u64().unwrap();
    let find = |a: &mut Automation, q: &str| ok(a, "text_find", json!({ "doc": doc, "query": q }));

    let office = find(&mut a, "office");
    assert_eq!(office["count"], 1, "{office}");
    let rects = office["matches"][0]["rects"].as_array().unwrap();
    assert_eq!(rects.len(), 1);
    assert!((rects[0][0].as_f64().unwrap() - 90.0).abs() < 0.5, "from the left edge of the o: {rects:?}");

    let example = find(&mut a, "example");
    assert_eq!(example["count"], 1, "{example}");
    assert_eq!(example["matches"][0]["text"], "exam-\nple");
    let rects = example["matches"][0]["rects"].as_array().unwrap();
    assert_eq!(rects.len(), 2, "one highlight on each line: {rects:?}");
    let top = |k: usize| rects[k][1].as_f64().unwrap();
    assert!(top(1) > top(0) + 10.0, "the second on the next line down: {rects:?}");

    for q in ["exam-ple", "Exam- ple", "exam-"] {
        assert_eq!(find(&mut a, q)["count"], 1, "{q}");
    }
    assert_eq!(find(&mut a, "examples")["count"], 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// One page in a Helvetica whose `/ToUnicode` maps its codes to halfwidth and fullwidth forms:
/// `ＡＢＣ１２３` on one line, and below it `ｶﾞｲﾄﾞ` with each halfwidth sound mark a glyph of its own.
fn width_pdf() -> Vec<u8> {
    let cmap = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapType 2 def \
                1 begincodespacerange <00> <FF> endcodespacerange 10 beginbfchar \
                <41> <FF21> <42> <FF22> <43> <FF23> <31> <FF11> <32> <FF12> <33> <FF13> \
                <44> <FF76> <45> <FF9E> <46> <FF72> <47> <FF84> \
                endbfchar endcmap CMapName currentdict /CMap defineresource pop end end";
    let body = "BT /F1 14 Tf 90 700 Td (ABC123) Tj 0 -30 Td (DEFGE) Tj ET\n";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /ToUnicode 6 0 R >>".to_string(),
        format!("<< /Length {} >>\nstream\n{cmap}\nendstream", cmap.len()),
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

/// "abc123" finds the fullwidth letters and digits and "ガイド" the halfwidth katakana with their
/// separate sound marks, returning the page's own characters; the halfwidth query highlights the
/// same glyphs.
#[test]
fn text_find_matches_halfwidth_and_fullwidth_forms_from_their_ordinary_width() {
    let dir = tmp("width");
    std::fs::write(dir.join("width.pdf"), width_pdf()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = ok(&mut a, "doc_open", json!({ "path": "width.pdf" }))["doc"].as_u64().unwrap();
    let find = |a: &mut Automation, q: &str| ok(a, "text_find", json!({ "doc": doc, "query": q }));

    let latin = find(&mut a, "abc123");
    assert_eq!(latin["count"], 1, "{latin}");
    assert_eq!(latin["matches"][0]["text"], "ＡＢＣ１２３");
    let kana = find(&mut a, "ガイド");
    assert_eq!(kana["count"], 1, "{kana}");
    assert_eq!(kana["matches"][0]["text"], "ｶﾞｲﾄﾞ");
    let half = find(&mut a, "ｶﾞｲﾄﾞ");
    assert_eq!(half["matches"][0]["rects"], kana["matches"][0]["rects"]);
    assert_eq!(find(&mut a, "カ")["count"], 0, "the ｶ takes a sound mark: it is ガ");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `ignore_width: false` makes width significant in `text_find`, `text_replace` and `redact_mark`:
/// "abc123" no longer finds "ＡＢＣ１２３", nor "ガイド" "ｶﾞｲﾄﾞ", while the page's own forms still
/// find themselves; `true` is the default.
#[test]
fn ignore_width_false_makes_width_significant() {
    let dir = tmp("width-exact");
    std::fs::write(dir.join("width.pdf"), width_pdf()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = ok(&mut a, "doc_open", json!({ "path": "width.pdf" }))["doc"].as_u64().unwrap();
    let find =
        |a: &mut Automation, q: &str, ignore: bool| ok(a, "text_find", json!({ "doc": doc, "query": q, "ignore_width": ignore }))["count"].clone();

    assert_eq!(find(&mut a, "abc123", true), 1);
    assert_eq!(find(&mut a, "abc123", false), 0);
    assert_eq!(find(&mut a, "ガイド", false), 0);
    assert_eq!(find(&mut a, "ｶﾞｲﾄﾞ", false), 1);
    assert_eq!(find(&mut a, "ＡＢＣ１２３", false), 1);
    assert_eq!(find(&mut a, "ｶ", false), 1, "as written, the base before its sound mark");
    assert_eq!(find(&mut a, "ｶ", true), 0);

    let replaced = |a: &mut Automation, ignore: bool| {
        ok(a, "text_replace", json!({ "doc": doc, "find": "abc123", "replace": "x", "ignore_width": ignore }))["replaced"].clone()
    };
    assert_eq!(replaced(&mut a, false), 0, "no match where width counts");
    assert_eq!(replaced(&mut a, true), 1);

    let mark = |a: &mut Automation, ignore: bool| a.call("redact_mark", &json!({ "doc": doc, "find": "ガイド", "ignore_width": ignore })).is_ok();
    assert!(!mark(&mut a, false), "nothing matched, so nothing was marked");
    assert!(mark(&mut a, true));
    assert!(
        a.call("redact_mark", &json!({ "doc": doc, "pattern": "email", "ignore_width": false })).is_err(),
        "ignore_width applies only to find or words"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
