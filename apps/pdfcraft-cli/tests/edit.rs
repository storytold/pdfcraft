//! `edit` must reject options it does not know instead of writing an unchanged copy (#133).

use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

/// A one-page 200×300 pt PDF.
fn fixture() -> Vec<u8> {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
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

fn tmp(test: &str, name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pdfcraft-cli-edit-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

/// The page rotation `pdfcraft-cli info` reports for the first page, via the inspector.
fn first_page_rotation(path: &std::path::Path) -> u16 {
    let bytes = std::sync::Arc::new(std::fs::read(path).unwrap());
    pdfcraft_render::inspect(bytes, None).unwrap().pages[0].rotation
}

#[test]
fn a_misspelled_option_fails_and_writes_nothing() {
    let input = tmp("typo", "in.pdf");
    std::fs::write(&input, fixture()).unwrap();
    let output = tmp("typo", "typo.pdf");
    let out = Command::new(BIN).args(["edit", input.to_str().unwrap(), "--out", output.to_str().unwrap(), "--rotat", "1:90"]).output().unwrap();
    assert!(!out.status.success(), "an unknown option must not exit 0");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown option") && stderr.contains("--rotat"), "the option must be named: {stderr}");
    assert!(stderr.contains("--rotate"), "the accepted options must be listed: {stderr}");
    assert!(!output.exists(), "no output may be written for a rejected command line");
}

#[test]
fn an_option_without_its_value_fails() {
    let input = tmp("novalue", "in.pdf");
    std::fs::write(&input, fixture()).unwrap();
    let output = tmp("novalue", "out.pdf");
    let out = Command::new(BIN).args(["edit", input.to_str().unwrap(), "--out", output.to_str().unwrap(), "--title"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--title needs a value"));
    assert!(!output.exists());
}

#[test]
fn a_second_input_file_fails() {
    let input = tmp("stray", "in.pdf");
    std::fs::write(&input, fixture()).unwrap();
    let output = tmp("stray", "out.pdf");
    let out = Command::new(BIN).args(["edit", input.to_str().unwrap(), "other.pdf", "--out", output.to_str().unwrap()]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unexpected argument \"other.pdf\""));
    assert!(!output.exists());
}

#[test]
fn the_documented_options_still_edit_and_save() {
    let input = tmp("ok", "in.pdf");
    std::fs::write(&input, fixture()).unwrap();
    for (name, extra) in [("incremental.pdf", Vec::new()), ("full.pdf", vec!["--full"])] {
        let output = tmp("ok", name);
        let mut args = vec!["edit", input.to_str().unwrap(), "--out", output.to_str().unwrap(), "--rotate", "1:90", "--title", "Rotated"];
        args.extend(extra);
        let out = Command::new(BIN).args(&args).output().unwrap();
        assert!(out.status.success(), "{name}: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(first_page_rotation(&output), 90, "{name}");
        let info = pdfcraft_render::inspect(std::sync::Arc::new(std::fs::read(&output).unwrap()), None).unwrap();
        assert_eq!(info.title.as_deref(), Some("Rotated"), "{name}");
    }
}
