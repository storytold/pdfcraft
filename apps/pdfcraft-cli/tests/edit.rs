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

mod one_based_arguments {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            for _ in 0..128 {
                let path =
                    std::env::temp_dir().join(format!("pdfcraft-cli-one-based-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
                match std::fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(e) => panic!("cannot create test directory: {e}"),
                }
            }
            panic!("cannot reserve test directory");
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn zero_render_page_and_edit_positions_fail_before_io() {
        let dir = TestDir::new();
        let source = fixture();
        std::fs::write(dir.0.join("input.pdf"), &source).unwrap();
        for input in ["input.pdf", "missing.pdf"] {
            for destination in ["output.pam", "new.pam", "input.pdf"] {
                std::fs::write(dir.0.join("output.pam"), b"keep destination").unwrap();
                for (command, option, value, diagnostic) in [
                    ("render", "--page", "0", "--page must be at least 1"),
                    ("edit", "--move", "1:0", "--move target must be at least 1"),
                    ("edit", "--insert-blank", "0", "--insert-blank must be at least 1"),
                ] {
                    let out = Command::new(BIN).current_dir(&dir.0).args([command, input, "--out", destination, option, value]).output().unwrap();
                    assert_eq!(out.status.code(), Some(1), "{command} {input} {option} {value}");
                    assert_eq!(out.stderr, format!("pdfcraft-cli: {diagnostic}\n").as_bytes());
                    assert!(out.stdout.is_empty());
                    assert_eq!(std::fs::read(dir.0.join("input.pdf")).unwrap(), source);
                    assert_eq!(std::fs::read(dir.0.join("output.pam")).unwrap(), b"keep destination");
                    assert!(!dir.0.join("new.pam").exists());
                    assert!(!dir.0.join("missing.pdf").exists());
                }
            }
        }
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 2);
    }

    #[test]
    fn first_render_page_move_target_and_insertion_position_remain_usable() {
        let dir = TestDir::new();
        let source = fixture();
        std::fs::write(dir.0.join("input.pdf"), &source).unwrap();
        for output in ["page.pam", "page.png"] {
            let out =
                Command::new(BIN).current_dir(&dir.0).args(["render", "input.pdf", "--page", "1", "--dpi", "18", "--out", output]).output().unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            let bytes = std::fs::read(dir.0.join(output)).unwrap();
            if output.ends_with(".pam") {
                let header = b"P7\nWIDTH 50\nHEIGHT 75\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n";
                assert!(bytes.starts_with(header));
                assert_eq!(bytes.len(), header.len() + 50 * 75 * 4);
            } else {
                assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
                assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 50);
                assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 75);
            }
        }
        let out = Command::new(BIN)
            .current_dir(&dir.0)
            .args(["edit", "input.pdf", "--out", "edited.pdf", "--move", "1:1", "--rotate", "1:90", "--insert-blank", "1"])
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(out.stdout.is_empty() && out.stderr.is_empty());
        let info = pdfcraft_render::inspect(std::sync::Arc::new(std::fs::read(dir.0.join("edited.pdf")).unwrap()), None).unwrap();
        assert_eq!(info.pages.len(), 2);
        assert_eq!((info.pages[0].width, info.pages[0].height, info.pages[0].rotation), (612.0, 792.0, 0));
        assert_eq!((info.pages[1].width, info.pages[1].height, info.pages[1].rotation), (300.0, 200.0, 90));
        assert_eq!(std::fs::read(dir.0.join("input.pdf")).unwrap(), source);
    }
}
