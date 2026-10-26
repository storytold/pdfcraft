//! `export`: a PDF to Word, PowerPoint, HTML or RTF, the format chosen by `--out`'s extension.

use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

/// A one-page 200×300 pt PDF reading "Hello export".
fn fixture() -> Vec<u8> {
    let body = "BT /F1 24 Tf 20 150 Td (Hello export) Tj ET";
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_vec(),
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

fn workdir(test: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("pdfcraft-cli-export-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("in.pdf"), fixture()).unwrap();
    d
}

fn export(dir: &std::path::Path, extra: &[&str]) -> Output {
    Command::new(BIN).arg("export").arg(dir.join("in.pdf")).args(extra).output().unwrap()
}

#[test]
fn exports_to_word_as_a_package() {
    let dir = workdir("docx");
    let out_path = dir.join("out.docx");
    let out = export(&dir, &["--out", out_path.to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let bytes = std::fs::read(&out_path).unwrap();
    assert!(bytes.starts_with(b"PK"), "a .docx is a zip package");
    assert!(bytes.windows(17).any(|w| w == b"word/document.xml"), "the package names its main part");
}

#[test]
fn the_format_follows_the_extension() {
    let dir = workdir("formats");
    let html = dir.join("out.html");
    assert!(export(&dir, &["--out", html.to_str().unwrap()]).status.success());
    assert!(std::fs::read_to_string(&html).unwrap().contains("Hello export"));
    let rtf = dir.join("out.RTF");
    assert!(export(&dir, &["--out", rtf.to_str().unwrap()]).status.success());
    assert!(std::fs::read_to_string(&rtf).unwrap().starts_with("{\\rtf"));
    let pptx = dir.join("out.pptx");
    assert!(export(&dir, &["--out", pptx.to_str().unwrap()]).status.success());
    assert!(std::fs::read(&pptx).unwrap().starts_with(b"PK"));
}

#[test]
fn a_missing_or_unsupported_destination_fails_and_writes_nothing() {
    let dir = workdir("bad");
    let out = export(&dir, &[]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("missing --out"));
    let odd = dir.join("out.doc");
    let out = export(&dir, &["--out", odd.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("must end in .docx"));
    assert!(!odd.exists());
    let out = export(&dir, &["--out", dir.join("x.docx").to_str().unwrap(), "--bogus", "1"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown option --bogus"));
}

#[test]
fn an_unreadable_input_is_an_error() {
    let dir = workdir("missing");
    let out = Command::new(BIN).arg("export").arg(dir.join("nope.pdf")).args(["--out", dir.join("x.docx").to_str().unwrap()]).output().unwrap();
    assert!(!out.status.success());
    assert!(!dir.join("x.docx").exists());
}
