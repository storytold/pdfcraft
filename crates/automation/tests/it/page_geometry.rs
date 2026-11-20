//! `page_geometry` end to end: the segments snapping reads, in page or display points, from the
//! document as it stands.

use std::path::{Path, PathBuf};

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// A 200 × 300 pt page drawing a line, a filled square and, through a form's `/Matrix`, a
/// second line.
fn fixture() -> Vec<u8> {
    let content = "10 20 m 110 20 l S 20 40 30 30 re f /Fm Do";
    let form = "0 0 m 40 0 l S";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Fm 5 0 R >> >> >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        format!("<< /Type /XObject /Subtype /Form /BBox [0 0 50 50] /Matrix [1 0 0 1 100 200] /Length {} >>\nstream\n{form}\nendstream", form.len()),
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

/// A folder of its own per test (they run at the same time) holding the fixture as `a.pdf`.
fn workdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-automation-geometry-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.pdf"), fixture()).unwrap();
    dir
}

fn ok(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args) {
        Ok(mut c) => match c.remove(0) {
            Content::Json(v) => v,
            other => panic!("{tool}: expected JSON, got {other:?}"),
        },
        Err(e) => panic!("{tool} {args}: {e}"),
    }
}

fn open(dir: &Path) -> (Automation, u64) {
    let mut a = Automation::new().with_root(dir).unwrap();
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    (a, doc)
}

fn segments(page: &Value) -> Vec<[f64; 4]> {
    page["segments"].as_array().unwrap().iter().map(|s| std::array::from_fn(|i| s[i].as_f64().unwrap())).collect()
}

/// The listing is what snapping reads: as many segments as `measure_snap` counts, including the
/// line a form draws through its `/Matrix`.
#[test]
fn page_geometry_lists_what_snapping_reads() {
    let dir = workdir("plain");
    let (mut a, doc) = open(&dir);
    let page = ok(&mut a, "page_geometry", json!({ "doc": doc }))["pages"][0].clone();
    let snap = ok(&mut a, "measure_snap", json!({ "doc": doc, "page": 1, "at": [0, 0] }));
    assert_eq!(page["count"], snap["segments"]);
    assert_eq!((page["truncated"].clone(), page["unreadable"].clone()), (json!(false), json!(0)));
    let listed = segments(&page);
    assert_eq!(listed.len() as u64, page["count"].as_u64().unwrap());
    assert!(listed.contains(&[10.0, 20.0, 110.0, 20.0]), "{listed:?}");
    assert!(listed.contains(&[100.0, 200.0, 140.0, 200.0]), "the form's line, through its /Matrix: {listed:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// Display points are the ones `measure_snap` takes and returns: every listed end snaps to
/// itself, on the page as drawn and after an unsaved quarter turn.
#[test]
fn page_geometry_display_points_are_where_snapping_finds_them() {
    let dir = workdir("display");
    let (mut a, doc) = open(&dir);
    let page_points = segments(&ok(&mut a, "page_geometry", json!({ "doc": doc }))["pages"][0]);
    let shown = ok(&mut a, "page_geometry", json!({ "doc": doc, "pages": [1], "coords": "display" }));
    assert_eq!(shown["coords"], "display");
    for (p, d) in page_points.iter().zip(segments(&shown["pages"][0])) {
        assert_eq!([d[0], d[2]], [p[0], p[2]], "x unchanged on an unrotated page");
        assert_eq!([d[1], d[3]], [300.0 - p[1], 300.0 - p[3]], "y down from the top");
    }
    ok(&mut a, "page_rotate", json!({ "doc": doc, "pages": [1], "degrees": 90 }));
    let turned = segments(&ok(&mut a, "page_geometry", json!({ "doc": doc, "coords": "display" }))["pages"][0]);
    assert_ne!(turned, segments(&shown["pages"][0]), "the turn moves the display points");
    for s in &turned {
        for end in [[s[0], s[1]], [s[2], s[3]]] {
            let snap = ok(&mut a, "measure_snap", json!({ "doc": doc, "page": 1, "at": end, "tolerance": 0.5 }));
            let at: Vec<f64> = snap["snap"]["point"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
            assert!((at[0] - end[0]).abs() < 1e-3 && (at[1] - end[1]).abs() < 1e-3, "{end:?} snapped to {at:?}");
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// A limit stops a page and says so; arguments out of range are refused.
#[test]
fn page_geometry_limits_and_bad_arguments() {
    let dir = workdir("limits");
    let (mut a, doc) = open(&dir);
    let all = ok(&mut a, "page_geometry", json!({ "doc": doc }))["pages"][0]["count"].as_u64().unwrap();
    let page = ok(&mut a, "page_geometry", json!({ "doc": doc, "max_segments": 2 }))["pages"][0].clone();
    assert_eq!(page["truncated"], true);
    assert!(page["count"].as_u64().unwrap() < all);
    for args in [
        json!({ "doc": doc, "coords": "screen" }),
        json!({ "doc": doc, "max_segments": 0 }),
        json!({ "doc": doc, "max_targets": "many" }),
        json!({ "doc": doc, "max_bytes": 1_u64 << 40 }),
        json!({ "doc": doc, "pages": [2] }),
        json!({ "doc": doc, "pages": [] }),
    ] {
        assert!(a.call("page_geometry", &args).is_err(), "{args}");
    }
    let _ = std::fs::remove_dir_all(dir);
}
