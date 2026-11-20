//! `geometry` prints `page_geometry`'s answer for a file as one line of JSON, and refuses bad
//! arguments on stderr with nothing on stdout.

use std::process::Command;

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

/// Two 200 × 300 pt pages: the first draws a line, the second a square.
fn fixture() -> Vec<u8> {
    let first = "10 20 m 110 20 l S";
    let second = "10 10 50 50 re f";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 /MediaBox [0 0 200 300] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{first}\nendstream", first.len()),
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{second}\nendstream", second.len()),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn file(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-geometry-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, fixture()).unwrap();
    path.to_string_lossy().into_owned()
}

fn geometry(args: &[&str]) -> Value {
    let out = Command::new(BIN).arg("geometry").args(args).output().unwrap();
    assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1, "one line of JSON");
    serde_json::from_str(&stdout).unwrap()
}

#[test]
fn geometry_lists_every_page_or_the_pages_asked_for() {
    let path = file("plain.pdf");
    let all = geometry(&[&path]);
    assert_eq!(all["pages"].as_array().unwrap().len(), 2, "every page by default");
    assert_eq!(all["pages"][0]["segments"], json!([[10.0, 20.0, 110.0, 20.0]]));
    assert!(all["pages"][1]["count"].as_u64().unwrap() >= 4, "the square's sides");

    let second = geometry(&[&path, "--pages", "2", "--coords", "display", "--max-segments", "2"]);
    assert_eq!(second["coords"], "display");
    let pages = second["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0]["page"], 2);
    assert_eq!(pages[0]["truncated"], true, "--max-segments reaches the tool");
}

#[test]
fn geometry_refuses_bad_arguments() {
    let path = file("bad.pdf");
    for args in [
        vec!["geometry"],
        vec!["geometry", &path, "--pages", "3"],
        vec!["geometry", &path, "--coords", "screen"],
        vec!["geometry", &path, "--max-segments", "lots"],
        vec!["geometry", &path, "--style"],
        vec!["geometry", "missing.pdf"],
    ] {
        let out = Command::new(BIN).args(&args).output().unwrap();
        assert!(!out.status.success(), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("geometry"), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
}

/// The refusal of a command that must not run: a failure, nothing on stdout, and stderr naming
/// what was wrong.
fn refused(args: &[&str], why: &str) {
    let out = Command::new(BIN).arg("geometry").args(args).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{args:?}");
    assert!(out.stdout.is_empty(), "{args:?}: {}", String::from_utf8_lossy(&out.stdout));
    assert!(stderr.contains(why), "{args:?}: {stderr}");
}

/// A value-taking option given with no value is refused before the document is read, rather
/// than running with the option's default; one left out is still fine.
#[test]
fn geometry_refuses_an_option_with_no_value() {
    let path = file("novalue.pdf");
    for option in ["--pages", "--coords", "--password", "--max-segments", "--max-targets", "--max-bytes", "--max-total-segments"] {
        refused(&[&path, option], &format!("geometry: {option} needs a value"));
        refused(&[&path, "--pages", "1", option], &format!("geometry: {option} needs a value"));
        // Before the file is opened: the missing value is the complaint, not the missing file.
        refused(&["missing.pdf", option], &format!("geometry: {option} needs a value"));
    }
    // A password may be anything, another option's name included, as for the other commands.
    assert_eq!(geometry(&[&path, "--password", "--coords"])["pages"].as_array().unwrap().len(), 2);
}

/// One input file: a second is refused, wherever it stands, rather than ignored.
#[test]
fn geometry_refuses_a_second_input_file() {
    let (first, second) = (file("first.pdf"), file("second.pdf"));
    refused(&[&first, &second], "geometry: unexpected argument");
    refused(&[&first, "--pages", "1", &second], &format!("unexpected argument {second:?}"));
    refused(&[], "geometry: missing file");
}

/// `--max-total-segments` bounds the whole answer, which then says where to continue.
#[test]
fn geometry_max_total_segments_reaches_the_tool() {
    let path = file("total.pdf");
    let all = geometry(&[&path]);
    assert!(all.get("next_page").is_none(), "every page fits by default");
    let first = geometry(&[&path, "--max-total-segments", "1"]);
    assert_eq!(first["pages"], json!([all["pages"][0]]));
    assert_eq!((first["next_page"].clone(), first["pages_left"].clone()), (json!(2), json!(1)));
    refused(&[&path, "--max-total-segments", "lots"], "--max-total-segments");
}

/// A password that looks like an option is the password, not that option: one walk reads the
/// options, so `--password --coords --max-total-segments 1` sets only the password and the budget.
#[test]
fn geometry_reads_a_password_that_looks_like_an_option_as_the_password() {
    let path = file("password-like-option.pdf");
    std::fs::write(&path, fixture()).unwrap();
    let answer = geometry(&[&path, "--password", "--coords", "--max-total-segments", "1"]);
    assert_eq!(answer["coords"], json!("page"));
    assert_eq!(answer["pages"].as_array().map(Vec::len), Some(1));
    assert_eq!(answer["next_page"], json!(2));
}
