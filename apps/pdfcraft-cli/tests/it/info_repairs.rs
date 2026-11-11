//! `info` lists the notes about damage repaired while opening, as the app shows them in Document
//! Properties and as `doc_info` reports them in `repairs`: a rebuilt cross-reference index, and a
//! page tree rebuilt from the pages found by scanning (a note the app adds after reading).

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

/// A PDF with `pages` pages, each drawing a rectangle (a page found by scanning has content), and
/// `catalog` as object 1; `startxref` points at its cross-reference table, or at `bad_startxref`.
fn fixture(catalog: &str, pages: usize, bad_startxref: Option<usize>) -> Vec<u8> {
    let content = "0 0 0 rg 10 10 40 20 re f";
    let kids: String = (0..pages).map(|i| format!("{} 0 R ", 4 + i)).collect();
    let mut objs = vec![
        catalog.to_string(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>"),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
    objs.extend((0..pages).map(|_| "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 120 60] /Contents 3 0 R >>".to_string()));
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in &offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    let start = bad_startxref.unwrap_or(xref);
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

const CATALOG: &str = "<< /Type /Catalog /Pages 2 0 R >>";

fn info_of(bytes: &[u8], name: &str) -> serde_json::Value {
    let dir = std::env::temp_dir().join(format!("pdfcraft-info-repairs-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("f.pdf");
    std::fs::write(&path, bytes).unwrap();
    let out = Command::new(BIN).args(["info", path.to_str().unwrap()]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json = serde_json::from_slice(&out.stdout).expect("info prints JSON");
    let _ = std::fs::remove_dir_all(&dir);
    json
}

fn repairs(json: &serde_json::Value) -> Vec<String> {
    json["repairs"].as_array().unwrap_or_else(|| panic!("no repairs array: {json}")).iter().map(|l| l.as_str().unwrap().to_string()).collect()
}

#[test]
fn info_lists_a_rebuilt_cross_reference_index() {
    // `startxref` points into the middle of an object, so the index is rebuilt from the headers.
    let json = info_of(&fixture(CATALOG, 1, Some(20)), "xref");
    assert_eq!(json["pages"], serde_json::json!(1), "{json}");
    let lines = repairs(&json);
    assert!(lines.iter().any(|l| l.contains("rebuilt cross-reference index")), "{lines:?}");
}

#[test]
fn info_lists_a_page_tree_the_app_rebuilt_by_scanning() {
    // The catalog names a page tree object the file does not have (#1656).
    let json = info_of(&fixture("<< /Type /Catalog /Pages 9 0 R >>", 2, None), "pagetree");
    let lines = repairs(&json);
    assert!(lines.iter().any(|l| l.contains("page tree is missing")), "{lines:?}");
}

#[test]
fn info_lists_no_repairs_for_a_sound_file() {
    let json = info_of(&fixture(CATALOG, 1, None), "sound");
    assert_eq!(json["repairs"], serde_json::json!([]), "{json}");
}
