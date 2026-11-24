//! SyncTeX search through the tool table: inverse (page position to source line) and forward
//! (source line to page rectangles), against a synthetic `.synctex` file beside a synthetic PDF.

use std::io::Write;
use std::path::PathBuf;

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// Points are 65781.76 units; the first box is x 100-400, y 190-200 from the top of page 1.
const FIXTURE: &str = "SyncTeX Version:1
Input:1:/work/main.tex
Input:2:./chap/one.tex
Output:pdf
Magnification:1000
Unit:1
X Offset:0
Y Offset:0
Content:
!0
{1
[1,1:0,0:0,0,0
(1,12:6578176,13156352:19734528,657818,0
x1,12:6578176,13156352
x1,13:16445440,13156352
)
(2,5:6578176,19734528:19734528,657818,0
x2,5:6578176,19734528
)
]
}1
{2
[1,1:0,0:0,0,0
(1,40:6578176,13156352:19734528,657818,0
x1,40:6578176,13156352
)
]
}2
Postamble:
Count:9
";

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pdfcraft-synctex-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn two_page_pdf() -> Vec<u8> {
    pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 612 792] >>")
        .obj("<< /Type /Page /Parent 2 0 R >>")
        .obj("<< /Type /Page /Parent 2 0 R >>")
        .build()
}

fn fixture(tmp: &std::path::Path, line12: u32) -> String {
    FIXTURE.replace("/work", &tmp.to_string_lossy()).replace(",12:", &format!(",{line12}:")).replace("x1,12:", &format!("x1,{line12}:"))
}

fn gz(text: &str) -> Vec<u8> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(text.as_bytes()).unwrap();
    enc.finish().unwrap()
}

/// A folder with `out.pdf` (two Letter pages) and, when `with_file`, `out.synctex`; the PDF open.
fn open(tag: &str, with_file: bool) -> (Automation, u64, PathBuf) {
    let tmp = dir(tag);
    std::fs::write(tmp.join("out.pdf"), two_page_pdf()).unwrap();
    if with_file {
        std::fs::write(tmp.join("out.synctex"), fixture(&tmp, 12)).unwrap();
    }
    let mut a = Automation::new().with_root(&tmp).unwrap();
    let doc = call(&mut a, "doc_open", json!({"path": "out.pdf"})).unwrap()["doc"].as_u64().unwrap();
    (a, doc, tmp)
}

fn call(a: &mut Automation, name: &str, args: Value) -> Result<Value, String> {
    match a.call(name, &args) {
        Ok(mut out) => match out.remove(0) {
            Content::Json(v) => Ok(v),
            other => Err(format!("expected JSON: {other:?}")),
        },
        Err(e) => Err(e.to_string()),
    }
}

fn rect(v: &Value) -> [f64; 4] {
    let r = v["rects"][0].as_array().unwrap();
    [r[0].as_f64().unwrap(), r[1].as_f64().unwrap(), r[2].as_f64().unwrap(), r[3].as_f64().unwrap()]
}

#[test]
fn inverse_names_the_source_line() {
    let (mut a, doc, _tmp) = open("inverse", true);
    let got = call(&mut a, "synctex_inverse", json!({"doc": doc, "page": 1, "at": [120, 195]})).unwrap();
    assert_eq!(got["line"], 12, "{got}");
    assert!(got["file"].as_str().unwrap().ends_with("main.tex"), "{got}");
}

#[test]
fn forward_names_the_page_and_rect() {
    let (mut a, doc, tmp) = open("forward", true);
    let file = tmp.join("main.tex").to_string_lossy().into_owned();
    let got = call(&mut a, "synctex_forward", json!({"doc": doc, "file": file, "line": 40, "column": 3})).unwrap();
    assert_eq!(got["page"], 2, "{got}");
    assert_eq!(got["rects"].as_array().unwrap().len(), 1, "{got}");
    for (g, w) in rect(&got).iter().zip([100.0, 190.0, 400.0, 200.0]) {
        assert!((g - w).abs() < 0.5, "{got}");
    }
}

#[test]
fn a_rotated_page_answers_in_display_points() {
    let (mut a, doc, tmp) = open("rotated", true);
    call(&mut a, "page_rotate", json!({"doc": doc, "pages": [1], "degrees": 90})).unwrap();
    let file = tmp.join("main.tex").to_string_lossy().into_owned();
    let got = call(&mut a, "synctex_forward", json!({"doc": doc, "file": file, "line": 12})).unwrap();
    let [x0, y0, x1, y1] = rect(&got);
    assert!(y1 - y0 > x1 - x0, "taller than wide: {got}");
    let at = [(x0 + x1) / 2.0, (y0 + y1) / 2.0];
    let back = call(&mut a, "synctex_inverse", json!({"doc": doc, "page": 1, "at": at})).unwrap();
    assert!(matches!(back["line"].as_u64(), Some(12 | 13)), "{back}");
}

#[test]
fn no_synctex_file_says_where_it_looked() {
    let (mut a, doc, _tmp) = open("nofile", false);
    let err = call(&mut a, "synctex_inverse", json!({"doc": doc, "page": 1, "at": [120, 195]})).unwrap_err();
    assert!(err.contains("out.synctex.gz") && err.contains("out.synctex"), "{err}");
}

#[test]
fn an_unknown_source_lists_inputs() {
    let (mut a, doc, _tmp) = open("unknown", true);
    let err = call(&mut a, "synctex_forward", json!({"doc": doc, "file": "/nope.tex", "line": 1})).unwrap_err();
    assert!(err.contains("main.tex"), "{err}");
}

#[test]
fn the_gz_file_wins_and_a_rebuild_is_seen() {
    let (mut a, doc, tmp) = open("gz", true);
    let line = |a: &mut Automation| call(a, "synctex_inverse", json!({"doc": doc, "page": 1, "at": [120, 195]})).unwrap()["line"].clone();
    assert_eq!(line(&mut a), 12);
    std::fs::write(tmp.join("out.synctex.gz"), gz(&fixture(&tmp, 99))).unwrap();
    assert_eq!(line(&mut a), 99);
    std::fs::write(tmp.join("out.synctex.gz"), gz(&fixture(&tmp, 7))).unwrap();
    assert_eq!(line(&mut a), 7);
}

#[test]
fn synctex_outside_root_is_refused() {
    let (mut a, doc, _tmp) = open("root", true);
    let err = call(&mut a, "synctex_inverse", json!({"doc": doc, "page": 1, "at": [120, 195], "synctex": "/etc/passwd"})).unwrap_err();
    assert!(err.contains("outside"), "{err}");
}

/// The file found beside the PDF is held to the root as a named one is: a link there that leads
/// outside is not read.
#[cfg(unix)]
#[test]
fn a_link_beside_the_pdf_that_leaves_the_root_is_refused() {
    let (mut a, doc, tmp) = open("link", false);
    let outside = dir("link-outside");
    std::fs::write(outside.join("secret.synctex"), fixture(&outside, 12)).unwrap();
    std::os::unix::fs::symlink(outside.join("secret.synctex"), tmp.join("out.synctex")).unwrap();
    let err = call(&mut a, "synctex_inverse", json!({"doc": doc, "page": 1, "at": [120, 195]})).unwrap_err();
    assert!(err.contains("outside"), "{err}");
    assert!(!err.contains("main.tex"), "{err}");
}

/// The same refusal when the link's target is not there: the answer does not tell whether a file
/// outside the root exists.
#[cfg(unix)]
#[test]
fn a_link_beside_the_pdf_to_a_missing_file_outside_is_refused_alike() {
    let (mut a, doc, tmp) = open("link-missing", false);
    let outside = dir("link-missing-outside");
    std::os::unix::fs::symlink(outside.join("absent.synctex"), tmp.join("out.synctex.gz")).unwrap();
    let err = call(&mut a, "synctex_inverse", json!({"doc": doc, "page": 1, "at": [120, 195]})).unwrap_err();
    assert!(err.contains("outside"), "{err}");
}
