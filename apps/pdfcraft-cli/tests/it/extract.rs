//! `extract --pages`: single pages and ranges (#1923), and an out-of-range page named as the
//! user typed it (1-based; it was off by one in 0.7.1).

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
    let mut out = b"%PDF-1.7
"
    .to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(
            format!(
                "{} 0 obj
",
                i + 1
            )
            .as_bytes(),
        );
        out.extend_from_slice(o);
        out.extend_from_slice(
            b"
endobj
",
        );
    }
    let xref = out.len();
    out.extend_from_slice(
        format!(
            "xref
0 {}
0000000000 65535 f 
",
            objs.len() + 1
        )
        .as_bytes(),
    );
    for o in offsets {
        out.extend_from_slice(
            format!(
                "{o:010} 00000 n 
"
            )
            .as_bytes(),
        );
    }
    out.extend_from_slice(
        format!(
            "trailer
<< /Size {} /Root 1 0 R >>
startxref
{xref}
%%EOF
",
            objs.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// A fresh directory holding `five.pdf` (5 pages).
fn workdir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pdfcraft-cli-extract-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("five.pdf"), fixture(5)).unwrap();
    d
}

fn extract(dir: &std::path::Path, pages: &str) -> Output {
    let out = dir.join(format!("out-{}.pdf", pages.replace(['-', ','], "_")));
    Command::new(BIN).args(["extract", dir.join("five.pdf").to_str().unwrap(), "--pages", pages, "--out", out.to_str().unwrap()]).output().unwrap()
}

/// The page count `info` reports for the extracted file.
fn pages_of(dir: &std::path::Path, pages: &str) -> usize {
    let out = dir.join(format!("out-{}.pdf", pages.replace(['-', ','], "_")));
    let info = Command::new(BIN).args(["info", out.to_str().unwrap()]).output().unwrap();
    let text = String::from_utf8_lossy(&info.stdout);
    let n = text.split("\"pages\":").nth(1).and_then(|s| s.trim().split(|c: char| !c.is_ascii_digit()).next()).unwrap_or("");
    n.parse().unwrap_or_else(|_| panic!("no page count in {text}"))
}

#[test]
fn pages_and_ranges_are_accepted_in_the_order_given() {
    let dir = workdir("ranges");
    for (pages, count) in [("1-3", 3), ("5,1-2", 3), ("2-2", 1), ("1,3,5", 3), (" 4 - 5 , 1 ", 3)] {
        let out = extract(&dir, pages);
        assert!(out.status.success(), "{pages:?}: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(pages_of(&dir, pages), count, "{pages:?}");
    }
}

#[test]
fn bad_pages_and_ranges_are_named_as_typed() {
    let dir = workdir("bad");
    for (pages, what) in [
        ("3-1", "bad page range \"3-1\""),
        ("0", "bad page number \"0\""),
        ("1-x", "bad page number \"1-x\""),
        ("2-", "bad page number \"2-\""),
        ("1-9999999", "bad page range \"1-9999999\""),
    ] {
        let out = extract(&dir, pages);
        assert!(!out.status.success(), "{pages:?} succeeded");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(what), "{pages:?}: {stderr}");
    }
    // A page past the end is named as the user counts it (1-based), as a single page and as the
    // first page a range reaches past the end.
    for (pages, missing) in [("6", "page 6 does not exist"), ("9", "page 9 does not exist"), ("4-7", "page 6 does not exist")] {
        let out = extract(&dir, pages);
        assert!(!out.status.success(), "{pages:?} succeeded");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(missing), "{pages:?}: {stderr}");
    }
}
