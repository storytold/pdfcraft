//! `audit` reports the rectangles of faux redactions where `doc_audit_redactions` does: in
//! points from the top-left of the page as displayed, after `/Rotate`.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

/// Two US Letter pages with "TOP SECRET" under an opaque `90 688 220 32 re`; the second page is
/// turned a quarter (`/Rotate 90`).
fn fixture() -> Vec<u8> {
    let body = "BT /F1 24 Tf 100 700 Td (TOP SECRET) Tj ET\n0 g\n90 688 220 32 re f";
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [4 0 R 5 0 R] /Count 2 /MediaBox [0 0 612 792] >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font << /F1 3 0 R >> >> >>".into(),
        "<< /Type /Page /Parent 2 0 R /Rotate 90 /Contents 6 0 R /Resources << /Font << /F1 3 0 R >> >> >>".into(),
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

/// The `rect` of each finding, in page order.
fn rects(report: &serde_json::Value) -> Vec<serde_json::Value> {
    report["findings"].as_array().expect("findings").iter().map(|f| f["rect"].clone()).collect()
}

#[test]
fn audit_rectangles_match_the_tool_in_top_left_points() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-audit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("faux.pdf"), fixture()).unwrap();

    let out = Command::new(BIN).arg("audit").arg(dir.join("faux.pdf")).output().unwrap();
    assert!(!out.status.success(), "findings must fail the audit");
    let cli: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(cli["count"], 2, "{cli}");
    // Page 1 upright: y measured down from the top of the 792 pt page. Page 2 turned a quarter
    // clockwise: 792 pt wide, and the cover's user-space y becomes its x.
    let expected = [serde_json::json!([90.0, 72.0, 310.0, 104.0]), serde_json::json!([688.0, 90.0, 720.0, 310.0])];
    assert_eq!(rects(&cli), expected, "{cli}");

    // The tool reports the same rectangles.
    let steps = serde_json::json!([
        { "tool": "doc_open", "args": { "path": "faux.pdf" } },
        { "tool": "doc_audit_redactions", "args": { "doc": 1 } },
    ]);
    std::fs::write(dir.join("steps.json"), steps.to_string()).unwrap();
    let out = Command::new(BIN).arg("run").arg("--script").arg(dir.join("steps.json")).arg("--root").arg(&dir).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let tool = serde_json::Deserializer::from_str(&stdout).into_iter::<serde_json::Value>().last().unwrap().unwrap();
    assert_eq!(rects(&tool), rects(&cli), "{tool}");
}

/// #4770: black boxes hide grey and coloured text just as they hide black text, but do not
/// remove any of it. Exercise the issue's public script before and after flattening annotations.
#[test]
fn audit_finds_all_text_colours_before_and_after_flattening() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-audit-colours-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let steps = serde_json::json!([
        {"tool": "doc_create", "args": {"from": "blank", "pages": 1, "width": 595, "height": 842}},
        {"tool": "page_add_text", "args": {"doc": 1, "page": 1, "text": "black SECRET", "at": [72, 100], "size": 14}},
        {"tool": "page_add_text", "args": {"doc": 1, "page": 1, "text": "grey SECRET", "at": [72, 200], "size": 14, "color": "#888888"}},
        {"tool": "page_add_text", "args": {"doc": 1, "page": 1, "text": "blue SECRET", "at": [72, 300], "size": 14, "color": "#1a6ee0"}},
        {"tool": "page_add_text", "args": {"doc": 1, "page": 1, "text": "near-black SECRET", "at": [72, 400], "size": 14, "color": "#222222"}},
        {"tool": "comment_add", "args": {"doc": 1, "page": 1, "type": "rectangle", "rect": [60, 92, 300, 124], "fill": "#000000", "color": "#000000"}},
        {"tool": "comment_add", "args": {"doc": 1, "page": 1, "type": "rectangle", "rect": [60, 192, 300, 224], "fill": "#000000", "color": "#000000"}},
        {"tool": "comment_add", "args": {"doc": 1, "page": 1, "type": "rectangle", "rect": [60, 292, 300, 324], "fill": "#000000", "color": "#000000"}},
        {"tool": "comment_add", "args": {"doc": 1, "page": 1, "type": "rectangle", "rect": [60, 392, 300, 424], "fill": "#000000", "color": "#000000"}},
        {"tool": "doc_audit_redactions", "args": {"doc": 1}},
        {"tool": "doc_flatten", "args": {"doc": 1}},
        {"tool": "doc_audit_redactions", "args": {"doc": 1}},
        {"tool": "doc_save", "args": {"doc": 1, "path": "boxes.pdf"}}
    ]);
    std::fs::write(dir.join("steps.json"), steps.to_string()).unwrap();
    let out = Command::new(BIN).arg("run").arg("--script").arg(dir.join("steps.json")).arg("--root").arg(&dir).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let tool_reports: Vec<serde_json::Value> = serde_json::Deserializer::from_slice(&out.stdout)
        .into_iter::<serde_json::Value>()
        .map(Result::unwrap)
        .filter(|value| value.get("findings").is_some())
        .collect();
    assert_eq!(tool_reports.len(), 2, "both annotation and flattened audits must run: {tool_reports:?}");

    let out = Command::new(BIN).arg("audit").arg(dir.join("boxes.pdf")).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "findings must fail the audit: {}", String::from_utf8_lossy(&out.stderr));
    let cli: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let expected = ["black SECRET", "grey SECRET", "blue SECRET", "near-black SECRET"];
    for report in tool_reports.iter().chain(std::iter::once(&cli)) {
        assert_eq!(report["count"], 4, "{report}");
        let texts: Vec<&str> = report["findings"].as_array().unwrap().iter().map(|finding| finding["covered_text"].as_str().unwrap()).collect();
        assert_eq!(texts, expected, "{report}");
    }

    let out = Command::new(BIN).arg("text").arg(dir.join("boxes.pdf")).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let extracted = String::from_utf8(out.stdout).unwrap();
    for text in expected {
        assert!(extracted.contains(text), "covered text must still be extractable: {extracted}");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
