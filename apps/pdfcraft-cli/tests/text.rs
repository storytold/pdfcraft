//! `text` must fail loudly when a page it was asked for cannot be read (#131), and when the
//! document itself cannot be opened, e.g. a password-protected file without `--password` (#132).

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

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

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("pdfcraft-cli-text-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

#[test]
fn text_of_a_selected_page_that_cannot_be_read_fails() {
    let path = tmp("three.pdf");
    std::fs::write(&path, fixture(3)).unwrap();
    let out = Command::new(BIN).args(["text", path.to_str().unwrap(), "--page", "9"]).output().unwrap();
    assert!(!out.status.success(), "a page that cannot be read must not exit 0");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("page 9"), "the failing page must be named: {stderr}");
}

#[test]
fn text_of_a_readable_document_succeeds() {
    let path = tmp("ok.pdf");
    std::fs::write(&path, fixture(3)).unwrap();
    let out = Command::new(BIN).args(["text", path.to_str().unwrap()]).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Page 1") && stdout.contains("Page 3"), "{stdout}");
}

/// `three.pdf` encrypted with the open password `openme`, via `run --script`. `tag` keeps each
/// test's files apart: tests run in parallel and would otherwise overwrite each other's.
fn protected_fixture(tag: &str) -> std::path::PathBuf {
    let src = tmp(&format!("to-protect-{tag}.pdf"));
    std::fs::write(&src, fixture(3)).unwrap();
    let out_path = tmp(&format!("protected-{tag}.pdf"));
    let script = tmp(&format!("protect-{tag}.json"));
    let steps = serde_json::json!([
        { "tool": "doc_open", "args": { "path": src } },
        { "tool": "doc_protect", "args": { "doc": 1, "open_password": "openme" } },
        { "tool": "doc_save", "args": { "doc": 1, "path": out_path } },
    ]);
    std::fs::write(&script, steps.to_string()).unwrap();
    let out = Command::new(BIN).args(["run", "--script", script.to_str().unwrap()]).output().unwrap();
    assert!(out.status.success(), "protecting the fixture failed: {}", String::from_utf8_lossy(&out.stderr));
    out_path
}

#[test]
fn text_of_a_protected_document_without_the_password_fails() {
    let path = protected_fixture("no-password");
    let out = Command::new(BIN).args(["text", path.to_str().unwrap()]).output().unwrap();
    assert!(!out.status.success(), "a document that cannot be opened must not exit 0");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("password"), "the reason must be named: {stderr}");
    assert!(out.stdout.is_empty(), "{}", String::from_utf8_lossy(&out.stdout));
}

#[test]
fn text_of_a_protected_document_with_the_password_succeeds() {
    let path = protected_fixture("password");
    let out = Command::new(BIN).args(["text", path.to_str().unwrap(), "--password", "openme"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Page 1") && stdout.contains("Page 3"), "{stdout}");
}
