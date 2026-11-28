//! `doc_info` page boxes (ISO 32000-2 §14.11.2): listed only for the pages asked for, read
//! from malformed boxes the way the page is displayed, and turned with the page's `/Rotate`.

use pdfcraft_automation::{Automation, Content, ToolError};
use serde_json::{Value, json};

/// Call tool `name` and return its JSON result.
fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

/// A PDF whose `/Pages` node holds `inherited` and whose pages are the dictionary bodies `pages`.
fn pdf(inherited: &str, pages: &[&str]) -> Vec<u8> {
    let kids: Vec<String> = (0..pages.len()).map(|n| format!("{} 0 R", n + 3)).collect();
    let mut objs = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string()];
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {} {inherited} >>", kids.join(" "), pages.len()));
    objs.extend(pages.iter().map(|p| format!("<< /Type /Page /Parent 2 0 R {p} >>")));
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (n, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", n + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// Open `bytes` and return `doc_info`'s `pages`, with the boxes of every page.
fn pages_with_boxes(bytes: Vec<u8>) -> Vec<Value> {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("boxes.pdf"), bytes).unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let doc = tool(&mut a, "doc_open", json!({ "path": "boxes.pdf" }))["doc"].as_u64().unwrap();
    let count = tool(&mut a, "doc_info", json!({ "doc": doc }))["pages"].as_array().unwrap().len();
    let all: Vec<usize> = (1..=count).collect();
    tool(&mut a, "doc_info", json!({ "doc": doc, "box_pages": all }))["pages"].as_array().unwrap().clone()
}

fn rect(v: &Value) -> [f64; 4] {
    let r: Vec<f64> = v.as_array().unwrap_or_else(|| panic!("not a rect: {v}")).iter().map(|x| x.as_f64().unwrap()).collect();
    [r[0], r[1], r[2], r[3]]
}

fn near(a: [f64; 4], b: [f64; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() <= 0.02)
}

/// Whether `inner` lies inside `outer`, give or take the tools' rounding.
fn inside(inner: [f64; 4], outer: [f64; 4]) -> bool {
    inner[0] >= outer[0] - 0.02 && inner[1] >= outer[1] - 0.02 && inner[2] <= outer[2] + 0.02 && inner[3] <= outer[3] + 0.02
}

/// Without `box_pages` no page carries boxes; with it, only the pages named do, and a page
/// number outside the document is refused.
#[test]
fn doc_info_lists_page_boxes_only_for_the_pages_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let page = "/MediaBox [0 0 200 300] /TrimBox [10 20 190 280]";
    std::fs::write(dir.path().join("three.pdf"), pdf("", &[page, page, page])).unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let doc = tool(&mut a, "doc_open", json!({ "path": "three.pdf" }))["doc"].as_u64().unwrap();
    let plain = tool(&mut a, "doc_info", json!({ "doc": doc }))["pages"].clone();
    assert!(plain.as_array().unwrap().iter().all(|p| p.get("boxes").is_none() && p.get("boxes_user").is_none()), "{plain}");
    let some = tool(&mut a, "doc_info", json!({ "doc": doc, "box_pages": [2] }))["pages"].clone();
    let with: Vec<bool> = some.as_array().unwrap().iter().map(|p| p["boxes"].is_object() && p["boxes_user"].is_object()).collect();
    assert_eq!(with, [false, true, false], "{some}");
    assert!(some.to_string().len() > plain.to_string().len());
    assert!(matches!(a.call("doc_info", &json!({ "doc": doc, "box_pages": [4] })), Err(ToolError::InvalidArgs(_))));
}

/// Malformed boxes read as the page displays them: the crop box is always the displayed page
/// ([0, 0, width, height]) whatever extra entries, reversed corners or sliver of a crop box
/// the file has, and every box is normalized and inside the media box.
#[test]
fn doc_info_page_boxes_are_normalized_as_the_page_displays() {
    let pages = pages_with_boxes(pdf(
        "/MediaBox [0 0 200 300]",
        &[
            // A crop box with a fifth entry: the renderer reads its first four.
            "/CropBox [10 20 190 280 0] /TrimBox [10 20 170 250]",
            // A crop box a sliver high: no area to the renderer, which shows the media box.
            "/CropBox [0 0 200 0.0001] /TrimBox [170 250 10 20]",
            // Boxes past the media box, or off it altogether.
            "/BleedBox [-50 -50 250 350] /ArtBox [500 500 600 600] /CropBox [300 400 -100 -100]",
        ],
    ));
    for p in &pages {
        let (b, u) = (&p["boxes"], &p["boxes_user"]);
        let size = [0.0, 0.0, p["width"].as_f64().unwrap(), p["height"].as_f64().unwrap()];
        assert!(near(rect(&b["crop"]), size), "the crop box is the displayed page: {p}");
        for k in ["media", "crop", "bleed", "trim", "art"] {
            let (v, r) = (rect(&b[k]), rect(&u[k]));
            assert!(v[0] < v[2] && v[1] < v[3] && r[0] < r[2] && r[1] < r[3], "{k} normalized: {p}");
            assert!(inside(r, rect(&u["media"])) && inside(v, rect(&b["media"])), "{k} inside the media box: {p}");
        }
    }
    // Reversed corners are the same box.
    assert_eq!(pages[0]["boxes_user"]["trim"], json!([10.0, 20.0, 170.0, 250.0]));
    assert_eq!(pages[1]["boxes_user"]["trim"], pages[0]["boxes_user"]["trim"]);
}

/// Margins (left, top, right, bottom) of `inner` within `outer`, in displayed coordinates.
fn margins(inner: [f64; 4], outer: [f64; 4]) -> [f64; 4] {
    [inner[0] - outer[0], inner[1] - outer[1], outer[2] - inner[2], outer[3] - inner[3]]
}

/// The displayed margins (left, top, right, bottom) of a box whose user-space margins are
/// `[left, bottom, right, top]`, on a page turned `rotation` degrees clockwise.
fn turned(m: [f64; 4], rotation: i64) -> [f64; 4] {
    let [l, b, r, t] = m;
    match rotation {
        90 => [b, l, t, r],
        180 => [r, b, l, t],
        270 => [t, r, b, l],
        _ => [l, t, r, b],
    }
}

/// The displayed boxes turn with the page: whatever way `/Rotate` is written (negative, past a
/// full turn, inherited, or not a multiple of 90 and so ignored), the trim box's displayed
/// margins are its user-space margins turned the way the page is shown.
#[test]
fn doc_info_page_boxes_turn_with_the_page() {
    // User-space margins of the trim box inside the media box: left 10, bottom 20, right 30, top 50.
    let boxes = "/MediaBox [0 0 200 300] /TrimBox [10 20 170 250]";
    let cases: [(&str, i64); 7] =
        [("", 90), ("/Rotate 0", 0), ("/Rotate 180", 180), ("/Rotate 270", 270), ("/Rotate -90", 270), ("/Rotate 450", 90), ("/Rotate 45", 0)];
    let page_dicts: Vec<String> = cases.iter().map(|(r, _)| format!("{r} {boxes}")).collect();
    let refs: Vec<&str> = page_dicts.iter().map(String::as_str).collect();
    let pages = pages_with_boxes(pdf("/Rotate 90", &refs));
    for (p, (_, rotation)) in pages.iter().zip(cases) {
        assert_eq!(p["rotation"], rotation, "{p}");
        let b = &p["boxes"];
        let shown = margins(rect(&b["trim"]), rect(&b["crop"]));
        assert!(near(shown, turned([10.0, 20.0, 30.0, 50.0], rotation)), "trim margins {shown:?} on {p}");
        assert!(near(rect(&b["crop"]), [0.0, 0.0, p["width"].as_f64().unwrap(), p["height"].as_f64().unwrap()]), "{p}");
    }
}
