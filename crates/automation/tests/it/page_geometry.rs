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

/// Three 200 × 300 pt pages: a line, a filled square (its four sides) and two lines.
fn three_pages() -> Vec<u8> {
    let contents = ["10 20 m 110 20 l S", "10 10 50 50 re f", "0 0 m 50 0 l S 0 10 m 50 10 l S"];
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R 5 0 R 7 0 R] /Count 3 /MediaBox [0 0 200 300] >>".to_owned(),
    ];
    for (i, content) in contents.iter().enumerate() {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R >>", 4 + 2 * i));
        objs.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
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

/// A folder of its own per test (they run at the same time) holding the fixture as `a.pdf`.
fn workdir(name: &str) -> PathBuf {
    workdir_with(name, &fixture())
}

fn workdir_with(name: &str, pdf: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-automation-geometry-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.pdf"), pdf).unwrap();
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

fn keys(v: &Value) -> Vec<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}

/// The answer as a whole holds at most `max_total_segments` segments (by default `max_segments`):
/// it stops before the page that would pass that, says where to continue, and lists the pages
/// that fit exactly as a call that fits does. A call that fits is unchanged: no new keys.
#[test]
fn page_geometry_answer_stops_at_its_segment_budget() {
    let dir = workdir_with("budget", &three_pages());
    let (mut a, doc) = open(&dir);
    let full = ok(&mut a, "page_geometry", json!({ "doc": doc }));
    assert_eq!(keys(&full), ["coords", "limits", "pages"], "nothing added when every page fits");
    let pages = full["pages"].as_array().unwrap().clone();
    let counts: Vec<u64> = pages.iter().map(|p| p["count"].as_u64().unwrap()).collect();
    assert_eq!(counts.len(), 3);
    assert!(counts.iter().all(|c| *c > 0) && counts[1] > counts[0], "{counts:?}");
    let total: u64 = counts.iter().sum();

    let exact = ok(&mut a, "page_geometry", json!({ "doc": doc, "max_total_segments": total }));
    assert_eq!(exact.to_string(), full.to_string(), "a budget the pages fit gives the same bytes");

    let short = ok(&mut a, "page_geometry", json!({ "doc": doc, "max_total_segments": total - 1 }));
    assert_eq!(short["pages"], json!(pages[..2]), "the pages that fit, unchanged");
    assert_eq!((short["next_page"].clone(), short["pages_left"].clone()), (json!(3), json!(1)));
    let rest = ok(&mut a, "page_geometry", json!({ "doc": doc, "pages": [3], "max_total_segments": total - 1 }));
    assert_eq!(rest["pages"], json!(pages[2..]), "following next_page lists the rest");
    assert!(rest.get("next_page").is_none());

    // The first page asked for is always listed, so following next_page always progresses.
    let first = ok(&mut a, "page_geometry", json!({ "doc": doc, "pages": [2, 1, 3], "max_total_segments": 1 }));
    assert_eq!(first["pages"], json!([pages[1]]));
    assert_eq!((first["next_page"].clone(), first["pages_left"].clone()), (json!(1), json!(2)));

    // By default the budget is max_segments: one page's worth, here less than all three.
    let most = counts[0] + counts[1];
    let default = ok(&mut a, "page_geometry", json!({ "doc": doc, "max_segments": most }));
    assert_eq!(default["pages"].as_array().unwrap().len(), 2);
    assert_eq!(default["next_page"], 3);

    for bad in [json!(0), json!(20_000_001), json!("many")] {
        assert!(a.call("page_geometry", &json!({ "doc": doc, "max_total_segments": bad })).is_err(), "{bad}");
    }
    let _ = std::fs::remove_dir_all(dir);
}
