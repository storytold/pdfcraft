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

/// `--out` naming the input replaces it through a staging file (#2166): the result is the
/// edited, reopenable file, and nothing is left beside it.
#[test]
fn editing_in_place_leaves_a_valid_file_and_no_staging_file() {
    let input = tmp("in-place", "doc.pdf");
    std::fs::write(&input, fixture()).unwrap();
    let out = Command::new(BIN).args(["edit", input.to_str().unwrap(), "--out", input.to_str().unwrap(), "--rotate", "1:90"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(first_page_rotation(&input), 90);
    let names: Vec<String> =
        std::fs::read_dir(input.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["doc.pdf"], "no staging file beside the result");
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
                    ("render", "--page", "0", "bad --page: expected a positive page number"),
                    ("edit", "--move", "1:0", "bad --move target: expected a page number of at least 1, got \"0\""),
                    ("edit", "--insert-blank", "0", "bad --insert-blank position: expected a page position of at least 1, got \"0\""),
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

/// A value the option cannot hold names the option and the value, and fails before the input
/// file is opened: a missing input still gets the option's own message.
#[test]
fn a_value_an_option_cannot_hold_is_named_before_the_file_is_opened() {
    let dir = tmp("values", "");
    for (option, value, diagnostic) in [
        ("--rotate", "1:abc", "bad --rotate degrees: expected a whole number of degrees, got \"abc\""),
        ("--rotate", "1:45", "bad --rotate degrees: expected a multiple of 90 (such as 90, -90 or 180), got 45"),
        ("--rotate", "1:-45", "bad --rotate degrees: expected a multiple of 90 (such as 90, -90 or 180), got -45"),
        ("--move", "1:abc", "bad --move target: expected a page number of at least 1, got \"abc\""),
        ("--move", "1:0", "bad --move target: expected a page number of at least 1, got \"0\""),
        ("--insert-blank", "abc", "bad --insert-blank position: expected a page position of at least 1, got \"abc\""),
        ("--insert-blank", "0", "bad --insert-blank position: expected a page position of at least 1, got \"0\""),
    ] {
        let out = Command::new(BIN).current_dir(&dir).args(["edit", "missing.pdf", "--out", "out.pdf", option, value]).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{option} {value}");
        assert_eq!(out.stderr, format!("pdfcraft-cli: {diagnostic}\n").as_bytes(), "{option} {value}");
        assert!(out.stdout.is_empty(), "{option} {value}");
        assert!(!dir.join("out.pdf").exists(), "{option} {value}: no output may be written");
    }
    // A negative multiple of 90 is still a rotation the engine accepts: the refusal above is
    // the multiple-of-90 rule, not a sign problem (the missing input is the next error).
    let out = Command::new(BIN).current_dir(&dir).args(["edit", "missing.pdf", "--out", "out.pdf", "--rotate", "1:-90"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("--rotate degrees"), "{}", String::from_utf8_lossy(&out.stderr));
}

/// A destination past the end must fail, not silently become an append (#1330).
#[test]
fn a_destination_past_the_end_fails_and_writes_nothing() {
    let input = tmp("past-end", "in.pdf");
    std::fs::write(&input, fixture()).unwrap();
    let output = tmp("past-end", "out.pdf");
    for (option, value, diagnostic) in [
        ("--insert-blank", "3", "--insert-blank 3 is past the end: it must be between 1 and 2 (the document has 1 pages)"),
        ("--insert-blank", "99", "--insert-blank 99 is past the end: it must be between 1 and 2 (the document has 1 pages)"),
        ("--move", "1:3", "--move target 3 is past the end: it must be between 1 and 2 (the document has 1 pages)"),
        ("--move", "1:99", "--move target 99 is past the end: it must be between 1 and 2 (the document has 1 pages)"),
    ] {
        let out = Command::new(BIN).args(["edit", input.to_str().unwrap(), "--out", output.to_str().unwrap(), option, value]).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{option} {value}");
        assert_eq!(String::from_utf8_lossy(&out.stderr), format!("pdfcraft-cli: {diagnostic}\n"), "{option} {value}");
        assert!(!output.exists(), "{option} {value}: no output may be written");
    }
}

/// One past the last page appends, and each destination is checked against the page count at that point.
#[test]
fn the_append_destination_and_counts_grown_by_earlier_edits_are_accepted() {
    let input = tmp("append", "in.pdf");
    std::fs::write(&input, fixture()).unwrap();
    for (name, args) in [
        ("insert.pdf", vec!["--insert-blank", "2"]),
        ("move.pdf", vec!["--insert-blank", "1", "--move", "1:3"]),
        ("grown.pdf", vec!["--insert-blank", "2", "--insert-blank", "3"]),
    ] {
        let output = tmp("append", name);
        let mut all = vec!["edit", input.to_str().unwrap(), "--out", output.to_str().unwrap()];
        all.extend(args);
        let out = Command::new(BIN).args(&all).output().unwrap();
        assert!(out.status.success(), "{name}: {}", String::from_utf8_lossy(&out.stderr));
        let info = pdfcraft_render::inspect(std::sync::Arc::new(std::fs::read(&output).unwrap()), None).unwrap();
        let sizes: Vec<_> = info.pages.iter().map(|p| (p.width, p.height)).collect();
        let expected: &[(f32, f32)] = match name {
            "insert.pdf" => &[(200.0, 300.0), (612.0, 792.0)],
            "move.pdf" => &[(200.0, 300.0), (612.0, 792.0)],
            _ => &[(200.0, 300.0), (612.0, 792.0), (612.0, 792.0)],
        };
        assert_eq!(sizes, expected, "{name}");
    }
}

/// Two pages whose catalog points at a page tree object the file does not have (#1656). The
/// reader finds the pages by scanning, in file order, so `edit` must work on the same pages.
/// Object 3 is written before object 2, so file order and object numbers disagree.
fn fixture_without_page_tree() -> Vec<u8> {
    let stream = |content: &str| format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes();
    let objs: Vec<(usize, Vec<u8>)> = vec![
        (1, b"<< /Type /Catalog /Pages 6 0 R >>".to_vec()),
        (3, b"<< /Type /Page /Parent 6 0 R /MediaBox [0 0 200 300] /Contents 4 0 R >>".to_vec()),
        (2, b"<< /Type /Page /Parent 6 0 R /MediaBox [0 0 300 200] /Contents 5 0 R >>".to_vec()),
        (4, stream("0 0 m 10 10 l S")),
        (5, stream("0 0 m 20 20 l S")),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = vec![0; objs.len()];
    for (num, o) in &objs {
        offsets[num - 1] = out.len();
        out.extend_from_slice(format!("{num} 0 obj\n").as_bytes());
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
fn a_document_whose_page_tree_is_missing_edits_the_pages_the_reader_shows() {
    let input = tmp("no-page-tree", "in.pdf");
    std::fs::write(&input, fixture_without_page_tree()).unwrap();
    let before = pdfcraft_render::inspect(std::sync::Arc::new(fixture_without_page_tree()), None).unwrap();
    assert_eq!(before.pages.iter().map(|p| (p.width, p.height)).collect::<Vec<_>>(), [(200.0, 300.0), (300.0, 200.0)]);
    for (name, args, expected) in [
        // Width and height are as displayed: the turned page reads 200 x 300.
        ("rotate.pdf", ["--rotate", "2:90"], &[(200.0, 300.0, 0), (200.0, 300.0, 90)][..]),
        ("delete.pdf", ["--delete", "1"], &[(300.0, 200.0, 0)][..]),
    ] {
        let output = tmp("no-page-tree", name);
        let mut all = vec!["edit", input.to_str().unwrap(), "--out", output.to_str().unwrap()];
        all.extend(args);
        let out = Command::new(BIN).args(&all).output().unwrap();
        assert!(out.status.success(), "{name}: {}", String::from_utf8_lossy(&out.stderr));
        let info = pdfcraft_render::inspect(std::sync::Arc::new(std::fs::read(&output).unwrap()), None).unwrap();
        let pages: Vec<_> = info.pages.iter().map(|p| (p.width, p.height, p.rotation)).collect();
        assert_eq!(pages, expected, "{name}");
    }
}

/// Like [`fixture_without_page_tree`], but the scanning reader shows different pages from the
/// page objects: it skips the blank 200 x 200 page (no `/Contents`) and shows the Text
/// annotation (which has `/Contents`) as a page. No page tree may be rebuilt from such a file.
fn fixture_whose_scan_disagrees() -> Vec<u8> {
    let stream = |content: &str| format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes();
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 9 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 9 0 R /MediaBox [0 0 100 100] /Contents 5 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 9 0 R /MediaBox [0 0 200 200] >>".to_vec(),
        b"<< /Type /Page /Parent 9 0 R /MediaBox [0 0 300 300] /Contents 6 0 R /Annots [7 0 R] >>".to_vec(),
        stream("0 0 m 10 10 l S"),
        stream("0 0 m 20 20 l S"),
        b"<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /Contents (hello) >>".to_vec(),
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
fn a_page_tree_is_not_rebuilt_when_the_reader_shows_other_pages() {
    let input = tmp("scan-disagrees", "in.pdf");
    std::fs::write(&input, fixture_whose_scan_disagrees()).unwrap();
    let shown = pdfcraft_render::inspect(std::sync::Arc::new(fixture_whose_scan_disagrees()), None).unwrap();
    assert_eq!(shown.pages.len(), 3);
    assert!(shown.pages.iter().all(|p| p.width != 200.0), "the reader must not show the blank page");
    let output = tmp("scan-disagrees", "out.pdf");
    let out = Command::new(BIN).args(["edit", input.to_str().unwrap(), "--out", output.to_str().unwrap(), "--delete", "2"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stderr), "pdfcraft-cli: the document's page tree lists no pages: the file is damaged\n");
    assert!(!output.exists(), "no page may be deleted");
}

/// A page past the end names its own option and the page count left by the earlier options (#1550).
#[test]
fn a_page_past_the_end_after_earlier_edits_names_that_option() {
    let input = tmp("chained", "in.pdf");
    std::fs::write(&input, fixture()).unwrap();
    let output = tmp("chained", "out.pdf");
    // 1 page, grown to 3, then --delete leaves 2: the 3 in --move 3:1 is the page that is gone.
    let args = ["--insert-blank", "2", "--insert-blank", "3", "--delete", "3", "--move", "3:1"];
    let out = Command::new(BIN).args(["edit", input.to_str().unwrap(), "--out", output.to_str().unwrap()]).args(args).output().unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--move: page 3 does not exist") && err.contains("has 2 pages"), "{err}");
    assert!(!output.exists());
}
