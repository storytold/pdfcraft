//! `preview` is the helper the Windows File Explorer preview handler (`LinkcoPdfPreviewHandler.dll`)
//! runs for every page it shows: it must always print one `STATUS\t…` line and exit successfully,
//! so the handler can show a friendly message instead of a generic failure.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

fn tmp(test: &str, name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pdfcraft-cli-preview-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn fixture(test: &str) -> PathBuf {
    let pdf = tmp(test, "in.pdf");
    let bytes = pdfcraft_engine::Session::new().create_from_text("Preview test", "Hello from the preview pane").unwrap();
    std::fs::write(&pdf, bytes.as_slice()).unwrap();
    pdf
}

/// Runs `preview` and returns the tab-separated fields of its `STATUS` line.
fn preview(pdf: &Path, out: &Path, extra: &[&str]) -> Vec<String> {
    let mut args = vec!["preview", pdf.to_str().unwrap(), "--out", out.to_str().unwrap()];
    args.extend_from_slice(extra);
    let r = Command::new(BIN).args(&args).output().unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let stdout = String::from_utf8_lossy(&r.stdout).into_owned();
    let line = stdout.lines().find(|l| l.starts_with("STATUS\t")).unwrap_or_else(|| panic!("no STATUS line: {stdout:?}"));
    line.split('\t').map(str::to_owned).collect()
}

fn png_size(path: &Path) -> (u32, u32) {
    let b = std::fs::read(path).unwrap();
    assert_eq!(&b[..8], b"\x89PNG\r\n\x1a\n");
    (u32::from_be_bytes([b[16], b[17], b[18], b[19]]), u32::from_be_bytes([b[20], b[21], b[22], b[23]]))
}

#[test]
fn ok_status_reports_pages_and_raster_size() {
    let pdf = fixture("ok");
    let out = tmp("ok", "p1.png");
    let s = preview(&pdf, &out, &["--page", "1", "--dpi", "72"]);
    assert_eq!(s[1], "OK", "{s:?}");
    assert_eq!(s[2], "1");
    assert_eq!(s[3], "1");
    let (w, h) = png_size(&out);
    assert_eq!((s[4].parse::<u32>().unwrap(), s[5].parse::<u32>().unwrap()), (w, h));
    assert!(w > 0 && h > w, "{w}×{h}");
}

#[test]
fn width_sets_the_raster_width_and_max_px_caps_it() {
    let pdf = fixture("width");
    let out = tmp("width", "p1.png");
    let s = preview(&pdf, &out, &["--width", "400"]);
    assert_eq!(s[1], "OK", "{s:?}");
    let (w, _) = png_size(&out);
    assert!((399..=401).contains(&w), "{w}");

    let s = preview(&pdf, &out, &["--width", "5000", "--max-px", "500"]);
    assert_eq!(s[1], "OK", "{s:?}");
    let (w, h) = png_size(&out);
    assert!(w.max(h) <= 501, "{w}×{h}");
}

#[test]
fn a_page_past_the_end_shows_the_last_page() {
    let pdf = fixture("clamp");
    let out = tmp("clamp", "p.png");
    let s = preview(&pdf, &out, &["--page", "99", "--dpi", "36"]);
    assert_eq!(s[1], "OK", "{s:?}");
    assert_eq!(s[3], "1");
}

#[test]
fn damaged_and_missing_files_get_a_status_not_a_failure() {
    let bad = tmp("bad", "bad.pdf");
    std::fs::write(&bad, b"this is not a PDF").unwrap();
    let out = tmp("bad", "p.png");
    let s = preview(&bad, &out, &[]);
    assert!(s[1] == "INVALID_PDF" || s[1] == "EMPTY_PDF", "{s:?}");
    assert!(!out.exists());

    let missing = tmp("missing", "nope.pdf");
    let s = preview(&missing, &out, &[]);
    assert_eq!(s[1], "IO_ERROR", "{s:?}");
}
