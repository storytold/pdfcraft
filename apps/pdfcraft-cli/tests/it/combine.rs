//! `combine --sort`: the order files go in, whatever order the shell listed them in.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, UNIX_EPOCH};

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

/// A PDF of `pages` pages, each saying `label`.
fn pdf(label: &str, pages: usize) -> Vec<u8> {
    let mut objs = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string()];
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {pages} /MediaBox [0 0 200 300] >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string());
    for i in 0..pages {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i));
        let body = format!("BT /F1 12 Tf 20 150 Td ({label}) Tj ET");
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
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

/// An empty folder for `test`, with `files` in it: each a path and how many pages it has.
fn folder(test: &str, files: &[(&str, usize)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-combine-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, pages) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let label = path.file_stem().unwrap().to_string_lossy().into_owned();
        std::fs::write(path, pdf(&label, *pages)).unwrap();
    }
    dir
}

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(BIN).args(args).current_dir(dir).output().unwrap()
}

/// Run `combine` with `args`, and give what each page of the result says.
fn combined(dir: &Path, args: &[&str]) -> Vec<String> {
    let output = run(dir, &[&["combine"], args, &["--out", "out.pdf"]].concat());
    assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
    let text = run(dir, &["text", "out.pdf"]);
    String::from_utf8(text.stdout).unwrap().split('\x0c').map(str::trim).filter(|page| !page.is_empty()).map(str::to_owned).collect()
}

#[test]
fn sort_name_puts_scan2_before_scan10_whatever_the_shell_gave() {
    let dir = folder("name", &[("scan10.pdf", 1), ("scan2.pdf", 1), ("Alpha.pdf", 1)]);
    // The order a shell expands `*.pdf` in.
    let listed = ["Alpha.pdf", "scan10.pdf", "scan2.pdf"];
    assert_eq!(combined(&dir, &listed), ["Alpha", "scan10", "scan2"], "without --sort the arguments are the order");
    assert_eq!(combined(&dir, &[&listed[..], &["--sort", "name"]].concat()), ["Alpha", "scan2", "scan10"]);
    assert_eq!(combined(&dir, &[&["--sort", "name", "--descending"], &listed[..]].concat()), ["scan10", "scan2", "Alpha"], "options can come first");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sort_reads_pages_size_modified_time_and_folder() {
    let dir = folder("facts", &[("a.pdf", 3), ("b.pdf", 1), ("sub/c.pdf", 2), ("sub b/d.pdf", 1), ("sub/inner/e.pdf", 1)]);
    // Padding between the trailer and startxref moves no offset: one page, yet the largest file.
    let padded = String::from_utf8(pdf("b", 1)).unwrap().replace("startxref", &format!("{}\nstartxref", "% padding\n".repeat(500)));
    std::fs::write(dir.join("b.pdf"), padded).unwrap();
    for (path, seconds) in [("sub/c.pdf", 100), ("a.pdf", 200), ("b.pdf", 300)] {
        let file = std::fs::File::options().write(true).open(dir.join(path)).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_secs(seconds)).unwrap();
    }
    let files = ["a.pdf", "b.pdf", "sub/c.pdf"];
    assert_eq!(combined(&dir, &[&files[..], &["--sort", "pages"]].concat()), ["b", "c", "c", "a", "a", "a"]);
    assert_eq!(combined(&dir, &[&files[..], &["--sort", "size"]].concat()), ["c", "c", "a", "a", "a", "b"]);
    assert_eq!(combined(&dir, &[&files[..], &["--sort", "modified"]].concat()), ["c", "c", "a", "a", "a", "b"]);
    let modified = combined(&dir, &[&files[..], &["--sort", "modified", "--descending"]].concat());
    assert_eq!(modified, ["b", "a", "a", "a", "c", "c"]);
    let folders = ["sub b/d.pdf", "sub/inner/e.pdf", "sub/c.pdf", "b.pdf"];
    assert_eq!(combined(&dir, &[&folders[..], &["--sort", "folder"]].concat()), ["b", "c", "c", "e", "d"], "a folder before the folders inside it");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sort_folder_reads_a_leading_dot_as_the_same_folder() {
    let dir = folder("spelling", &[("a.pdf", 1), ("m.pdf", 1), ("z.pdf", 1), ("docs/b.pdf", 1)]);
    assert_eq!(combined(&dir, &["z.pdf", "./a.pdf", "m.pdf", "--sort", "folder"]), ["a", "m", "z"]);
    assert_eq!(combined(&dir, &["./docs/b.pdf", "z.pdf", "./a.pdf", "--sort", "folder"]), ["a", "z", "b"]);
    assert_eq!(combined(&dir, &["docs/b.pdf", "./z.pdf", "a.pdf", "--sort", "folder", "--descending"]), ["b", "a", "z"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sort_folder_keeps_folders_that_read_alike_apart() {
    let dir = folder("alike", &[("v01/a.pdf", 1), ("v1/b.pdf", 1), ("v01/c.pdf", 1), ("v1/d.pdf", 1)]);
    let listed = ["v01/a.pdf", "v1/b.pdf", "v01/c.pdf", "v1/d.pdf"];
    assert_eq!(combined(&dir, &[&listed[..], &["--sort", "folder"]].concat()), ["a", "c", "b", "d"]);
    assert_eq!(combined(&dir, &[&listed[..], &["--sort", "folder", "--descending"]].concat()), ["b", "d", "a", "c"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sort_folder_puts_a_file_before_its_namesake_in_a_subfolder() {
    let dir = folder("namesakes", &[("a.pdf", 1)]);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/a.pdf"), pdf("inner", 1)).unwrap();
    for listed in [["a.pdf", "sub/a.pdf"], ["sub/a.pdf", "a.pdf"]] {
        assert_eq!(combined(&dir, &[&listed[..], &["--sort", "folder"]].concat()), ["a", "inner"], "{listed:?}");
        assert_eq!(combined(&dir, &[&listed[..], &["--sort", "folder", "--descending"]].concat()), ["inner", "a"], "{listed:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sort_folder_goes_by_the_folder_before_the_name_and_sort_name_by_the_name_alone() {
    let dir = folder("subfolder", &[("b.pdf", 1), ("sub/a.pdf", 1)]);
    let listed = ["sub/a.pdf", "b.pdf"];
    assert_eq!(combined(&dir, &[&listed[..], &["--sort", "folder"]].concat()), ["b", "a"]);
    assert_eq!(combined(&dir, &[&listed[..], &["--sort", "folder", "--descending"]].concat()), ["a", "b"]);
    assert_eq!(combined(&dir, &[&["b.pdf", "sub/a.pdf"][..], &["--sort", "name"]].concat()), ["a", "b"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bad_sort_says_what_is_wrong_and_writes_nothing() {
    let dir = folder("refused", &[("a.pdf", 1), ("b.pdf", 1)]);
    for (args, says) in [
        (&["--sort", "colour"][..], "--sort must be"),
        (&["--sort", "warnings"][..], "\"warnings\""),
        (&["--descending"][..], "--descending needs --sort"),
        (&["--sort"][..], "--sort needs a value"),
    ] {
        let output = run(&dir, &[&["combine", "a.pdf", "b.pdf", "--out", "out.pdf"], args].concat());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success() && stderr.contains(says), "{args:?}: {stderr}");
        assert!(!dir.join("out.pdf").exists(), "{args:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
