//! A value-taking option's operand is never read as another option (#7516). A password can be any
//! string, `--out` included, and the commands used to look options up by scanning every token:
//! in `extract --password --out in.pdf --pages 1 --out out.pdf` the password's operand was taken
//! for the real `--out`, its neighbour `in.pdf` for the output, and the source file was replaced.

use std::path::PathBuf;
use std::process::Command;

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

/// A fresh directory holding `in.pdf` (3 pages), and that file's path as the command line takes it.
fn workdir(test: &str) -> (PathBuf, String) {
    let d = std::env::temp_dir().join(format!("pdfcraft-cli-operands-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let input = d.join("in.pdf");
    std::fs::write(&input, fixture(3)).unwrap();
    let name = input.to_str().unwrap().to_string();
    (d, name)
}

#[test]
fn extract_does_not_write_over_its_source_when_the_password_is_out() {
    let (dir, input) = workdir("extract");
    let out = dir.join("out.pdf");
    let run = Command::new(BIN).args(["extract", "--password", "--out", &input, "--pages", "1", "--out", out.to_str().unwrap()]).output().unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    assert_eq!(std::fs::read(&input).unwrap(), fixture(3), "the source file was replaced");
    assert!(out.is_file(), "the page went to the real --out");
}

#[test]
fn extract_reads_the_real_pages_when_the_password_is_pages() {
    let (dir, input) = workdir("extract-pages");
    let out = dir.join("out.pdf");
    // `--password --pages` then the file: the old lookup read the file's name as the page list.
    let run = Command::new(BIN).args(["extract", "--password", "--pages", &input, "--pages", "2", "--out", out.to_str().unwrap()]).output().unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    assert!(out.is_file());
}

#[test]
fn split_writes_to_the_real_out_dir_when_the_password_is_out_dir() {
    let (dir, input) = workdir("split");
    let parts = dir.join("parts");
    let run =
        Command::new(BIN).args(["split", "--password", "--out-dir", &input, "--every", "1", "--out-dir", parts.to_str().unwrap()]).output().unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    for n in 1..=3 {
        assert!(parts.join(format!("in-p{n}.pdf")).is_file(), "part {n} missing");
    }
    assert_eq!(std::fs::read(&input).unwrap(), fixture(3));
}

#[test]
fn render_writes_to_the_real_out_when_the_password_is_out() {
    let (dir, input) = workdir("render");
    let png = dir.join("page.png");
    let run = Command::new(BIN).args(["render", "--password", "--out", &input, "--dpi", "36", "--out", png.to_str().unwrap()]).output().unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    assert!(std::fs::read(&png).unwrap().starts_with(b"\x89PNG"), "the image went to the real --out");
    assert_eq!(std::fs::read(&input).unwrap(), fixture(3), "the source file was replaced");
}

/// An option left without its operand at the end of the line is named, instead of being read as
/// absent: `render in.pdf --out page.png --page` used to render page 1 and exit 0.
#[test]
fn an_option_with_no_operand_is_named() {
    let (dir, input) = workdir("dangling");
    let png = dir.join("page.png");
    for args in [
        vec!["render", &input, "--out", png.to_str().unwrap(), "--page"],
        vec!["render", &input, "--out", png.to_str().unwrap(), "--dpi"],
        vec!["split", &input, "--every", "1", "--password"],
    ] {
        let option = *args.last().unwrap();
        let run = Command::new(BIN).current_dir(&dir).args(&args).output().unwrap();
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(!run.status.success(), "{args:?} unexpectedly succeeded");
        assert!(stderr.contains(&format!("{option} needs a value")), "{args:?}: {stderr}");
    }
    assert!(!png.exists(), "nothing was rendered");
}
