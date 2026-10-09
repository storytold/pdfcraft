//! `split --out-dir DIR` creates DIR when it does not exist yet (#249), and says so when it can't.

use std::path::PathBuf;
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

/// A PDF with `n` empty 200×300 pt pages.
fn fixture(n: usize) -> Vec<u8> {
    let mut objs: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")).into_bytes());
    for _ in 0..n {
        objs.push(b"<< /Type /Page /Parent 2 0 R >>".to_vec());
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

/// A fresh directory holding `report.pdf` (3 pages).
fn workdir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pdfcraft-cli-split-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("report.pdf"), fixture(3)).unwrap();
    d
}

fn split(dir: &std::path::Path, out_dir: &std::path::Path) -> Output {
    Command::new(BIN)
        .args(["split", dir.join("report.pdf").to_str().unwrap(), "--every", "1", "--out-dir", out_dir.to_str().unwrap()])
        .output()
        .unwrap()
}

#[test]
fn a_missing_out_dir_is_created() {
    let dir = workdir("create");
    let parts = dir.join("parts").join("nested");
    let out = split(&dir, &parts);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    for n in 1..=3 {
        let part = parts.join(format!("report-p{n}.pdf"));
        assert!(part.is_file(), "{} missing; stdout: {stdout}", part.display());
        assert!(stdout.contains(&format!("report-p{n}.pdf")), "{stdout}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_out_dir_that_cannot_be_created_is_reported_as_such() {
    let dir = workdir("file");
    let not_a_dir = dir.join("parts");
    std::fs::write(&not_a_dir, b"in the way").unwrap();
    let out = split(&dir, &not_a_dir);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--out-dir") && stderr.contains("parts"), "{stderr}");
    assert!(!stderr.contains("report-p1.pdf"), "the folder is the problem, not the first part: {stderr}");
    assert_eq!(std::fs::read(&not_a_dir).unwrap(), b"in the way");
    let _ = std::fs::remove_dir_all(&dir);
}
