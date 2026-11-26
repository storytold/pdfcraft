//! `render --out` writes the file format its name says (#248): a `.png` is a PNG, not a netpbm
//! PAM stream with a `.png` extension.

use std::path::PathBuf;
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

fn tmp(test: &str, name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pdfcraft-cli-render-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

/// A one-page PDF with some text, as `--out` would be given it.
fn fixture(test: &str) -> PathBuf {
    let pdf = tmp(test, "in.pdf");
    let bytes = pdfcraft_engine::Session::new().create_from_text("Render test", "Hello from PdfCraft").unwrap();
    std::fs::write(&pdf, bytes.as_slice()).unwrap();
    pdf
}

fn render(pdf: &std::path::Path, out: &std::path::Path) -> Output {
    Command::new(BIN).args(["render", pdf.to_str().unwrap(), "--page", "1", "--dpi", "36", "--out", out.to_str().unwrap()]).output().unwrap()
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

#[test]
fn a_png_name_gets_a_png() {
    let pdf = fixture("png");
    let out = tmp("png", "p1.png");
    let r = render(&pdf, &out);
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "not a PNG: {:?}", &bytes[..bytes.len().min(16)]);
    // IHDR: the page is 612×792 pt, rendered at 36 dpi
    assert_eq!(&bytes[12..16], b"IHDR");
    assert_eq!((be32(&bytes[16..20]), be32(&bytes[20..24])), (306, 396));
    // the reported size matches the file
    let stderr = String::from_utf8_lossy(&r.stderr);
    assert!(stderr.contains("306×396"), "{stderr}");
}

#[test]
fn jpeg_and_tiff_names_get_those_formats() {
    let pdf = fixture("jpg");
    let jpg = tmp("jpg", "p1.JPG");
    assert!(render(&pdf, &jpg).status.success());
    assert_eq!(&std::fs::read(&jpg).unwrap()[..2], b"\xff\xd8");
    let tif = tmp("jpg", "p1.tif");
    assert!(render(&pdf, &tif).status.success());
    let bytes = std::fs::read(&tif).unwrap();
    assert!(bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*"), "not a TIFF: {:?}", &bytes[..4]);
}

#[test]
fn the_file_records_the_dpi_it_was_rendered_at() {
    // The engine's tests check each format; this checks that `--dpi` reaches the file.
    let pdf = fixture("dpi");
    let jpg = tmp("dpi", "p1.jpg");
    assert!(render(&pdf, &jpg).status.success());
    let bytes = std::fs::read(&jpg).unwrap();
    assert_eq!(&bytes[6..11], b"JFIF\0");
    assert_eq!(bytes[13..18], [1, 0, 36, 0, 36], "JFIF density in dots per inch");
}

#[test]
fn a_pam_name_still_gets_the_netpbm_stream() {
    let pdf = fixture("pam");
    let out = tmp("pam", "p1.pam");
    assert!(render(&pdf, &out).status.success());
    let bytes = std::fs::read(&out).unwrap();
    let header = b"P7\nWIDTH 306\nHEIGHT 396\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n";
    assert!(bytes.starts_with(header), "{:?}", String::from_utf8_lossy(&bytes[..bytes.len().min(80)]));
    assert_eq!(bytes.len(), header.len() + 306 * 396 * 4);
}

#[test]
fn an_unknown_extension_fails_and_writes_nothing() {
    let pdf = fixture("bmp");
    let out = tmp("bmp", "p1.bmp");
    let r = render(&pdf, &out);
    assert!(!r.status.success());
    let stderr = String::from_utf8_lossy(&r.stderr);
    assert!(stderr.contains(".png, .jpg, .tif or .pam"), "{stderr}");
    assert!(!out.exists());
}

/// `--page` and `--dpi` are checked before the file is opened. A page that is not a positive
/// whole number names the same expectation `text` names; a dpi that parses as an `f32` but
/// cannot be rendered (`0`, a negative value, `NaN`) names the option and the value, instead
/// of reaching the renderer as a scale it refuses with its own internal message.
#[test]
fn a_page_or_dpi_the_renderer_cannot_use_is_refused_before_reading_the_file() {
    for (option, value, diagnostic) in [
        ("--page", "abc", "bad --page: expected a positive page number"),
        ("--page", "0", "bad --page: expected a positive page number"),
        ("--dpi", "abc", "bad --dpi: expected a positive number of dots per inch, got \"abc\""),
        ("--dpi", "0", "bad --dpi: expected a positive number of dots per inch, got \"0\""),
        ("--dpi", "-3", "bad --dpi: expected a positive number of dots per inch, got \"-3\""),
        ("--dpi", "NaN", "bad --dpi: expected a positive number of dots per inch, got \"NaN\""),
        ("--dpi", "inf", "bad --dpi: expected a positive number of dots per inch, got \"inf\""),
    ] {
        let out = tmp("refuse", "out.png");
        // `missing.pdf` is never read: the option's own message is the error.
        let r = Command::new(BIN).args(["render", "missing.pdf", "--out", out.to_str().unwrap(), option, value]).output().unwrap();
        assert_eq!(r.status.code(), Some(1), "{option} {value}");
        assert_eq!(r.stderr, format!("pdfcraft-cli: {diagnostic}\n").as_bytes(), "{option} {value}");
        assert!(!out.exists(), "{option} {value}: no output may be written");
    }
}
