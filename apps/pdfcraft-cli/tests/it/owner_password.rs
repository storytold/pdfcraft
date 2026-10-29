//! `info`, `text` and `render` take the owner password of revision 2–4 files, as `run doc_open`
//! does (#2670, item 8). The renderer authenticates only user passwords of those revisions, so
//! the CLI must hand it the user password that owner authentication recovers.

use std::process::Command;
use std::sync::Arc;

use pdfcraft_cos::{Algorithm, NewEncryption};

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

/// A one-page 200×300 pt PDF reading "Page 1", drawn by `content`.
fn fixture(content: &str) -> Vec<u8> {
    let stream = format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len());
    let objs: [&[u8]; 5] = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>",
        stream.as_bytes(),
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

const PAGE: &str = "BT /F1 24 Tf 20 150 Td (Page 1) Tj ET";

/// `fixture` protected with user password "user" and owner password "owner", allowing nothing,
/// so copying its text needs the owner password.
fn protected(algorithm: Algorithm) -> Vec<u8> {
    protected_with(algorithm, PAGE)
}

fn protected_with(algorithm: Algorithm, content: &str) -> Vec<u8> {
    let mut doc = pdfcraft_cos::Document::open(Arc::new(fixture(content))).unwrap();
    doc.set_encryption(&NewEncryption {
        algorithm,
        user_password: "user",
        owner_password: "owner",
        permissions: 0,
        encrypt_metadata: true,
        seed: [3; 32],
    })
    .unwrap();
    pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("pdfcraft-cli-owner-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

const ALGORITHMS: [Algorithm; 4] = [Algorithm::Rc4_40, Algorithm::Rc4_128, Algorithm::Aes128, Algorithm::Aes256];

#[test]
fn info_accepts_the_owner_password() {
    for alg in ALGORITHMS {
        let path = tmp(&format!("info-{alg:?}.pdf"));
        std::fs::write(&path, protected(alg)).unwrap();
        for pw in ["owner", "user"] {
            let out = Command::new(BIN).args(["info", path.to_str().unwrap(), "--password", pw]).output().unwrap();
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(out.status.success(), "{alg:?} with {pw:?}: {stderr}");
            let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(json["pages"], 1, "{alg:?} with {pw:?}");
            assert_eq!(json["encrypted"], true, "{alg:?} with {pw:?}");
        }
        let out = Command::new(BIN).args(["info", path.to_str().unwrap(), "--password", "wrong"]).output().unwrap();
        assert!(!out.status.success(), "{alg:?}: a wrong password still fails");
        assert!(String::from_utf8_lossy(&out.stderr).contains("incorrect"), "{alg:?}");
    }
}

#[test]
fn text_accepts_the_owner_password() {
    for alg in ALGORITHMS {
        let path = tmp(&format!("text-{alg:?}.pdf"));
        std::fs::write(&path, protected(alg)).unwrap();
        let out = Command::new(BIN).args(["text", path.to_str().unwrap(), "--password", "owner"]).output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{alg:?}: {stderr}");
        assert!(String::from_utf8_lossy(&out.stdout).contains("Page 1"), "{alg:?}");
        // The user password opens it, but the permissions still forbid copying.
        let out = Command::new(BIN).args(["text", path.to_str().unwrap(), "--password", "user"]).output().unwrap();
        assert!(!out.status.success(), "{alg:?}: copying needs the owner password");
        assert!(String::from_utf8_lossy(&out.stderr).contains("don't allow copying"), "{alg:?}");
    }
}

/// Asserts that `data` is a PNG of the fixture's 200×300 pt page at `dpi` with the text drawn:
/// a blank page of the right size fails too.
fn assert_rendered_page(data: &[u8], dpi: u32, what: &str) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(data)).read_info().unwrap_or_else(|e| panic!("{what}: {e}"));
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut buf).unwrap();
    assert_eq!((frame.width, frame.height), (200 * dpi / 72, 300 * dpi / 72), "{what}");
    // A rendered page is opaque, so `render` writes it as 8-bit RGB (`pdfcraft_engine::export::encode_png`).
    assert_eq!((frame.color_type, frame.bit_depth), (png::ColorType::Rgb, png::BitDepth::Eight), "{what}");
    // Pixels with every colour channel below half intensity: ink, not white paper.
    let inked = buf[..frame.buffer_size()].as_chunks::<3>().0.iter().filter(|px| px.iter().all(|c| *c < 128)).count();
    assert!(inked > 100, "{what}: only {inked} inked pixels, the page is blank");
}

#[test]
fn render_accepts_the_owner_password() {
    const DPI: u32 = 144;
    for alg in ALGORITHMS {
        let path = tmp(&format!("render-{alg:?}.pdf"));
        std::fs::write(&path, protected(alg)).unwrap();
        for pw in ["owner", "user"] {
            let png = tmp(&format!("render-{alg:?}-{pw}.png"));
            let _ = std::fs::remove_file(&png);
            let out = Command::new(BIN)
                .args(["render", path.to_str().unwrap(), "--page", "1", "--dpi", &DPI.to_string()])
                .args(["--password", pw, "--out", png.to_str().unwrap()])
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(out.status.success(), "{alg:?} with {pw:?}: {stderr}");
            assert_rendered_page(&std::fs::read(&png).unwrap(), DPI, &format!("{alg:?} with {pw:?}"));
        }
    }
}

#[test]
fn audit_places_findings_with_the_owner_password() {
    // A black box over the text: a faux redaction, so `audit` reads the page geometry too.
    let covered = format!("{PAGE} 0 g 10 140 120 40 re f");
    for alg in ALGORITHMS {
        let path = tmp(&format!("audit-{alg:?}.pdf"));
        std::fs::write(&path, protected_with(alg, &covered)).unwrap();
        let out = Command::new(BIN).args(["audit", path.to_str().unwrap(), "--password", "owner"]).output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("incorrect"), "{alg:?}: {stderr}");
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{alg:?}: {e}: {stderr}"));
        assert_eq!(report["findings"][0]["covered_text"], "Page 1", "{alg:?}");
        // Top-left points on the 300 pt high page: the box spans y 140–180 from the bottom.
        assert_eq!(report["findings"][0]["rect"], serde_json::json!([10.0, 120.0, 130.0, 160.0]), "{alg:?}");
    }
}
