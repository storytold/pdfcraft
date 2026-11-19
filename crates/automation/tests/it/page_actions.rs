//! Page actions and timers through the tool table (#169): `js_page` reads a page as the viewer
//! does, `js_advance` moves the script clock on, and `page_render` draws what they show.

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

/// Two pages; on the first, two frames over one another (red showing, green hidden) and a
/// page-open action flipping them every 100 ms, with a page-close action stopping it.
fn animation_pdf() -> Vec<u8> {
    let fill = |rgb: &str| {
        let body = format!("{rgb} rg 0 0 100 100 re f");
        format!("<< /Type /XObject /Subtype /Form /BBox [0 0 100 100] /Length {} >>\nstream\n{body}\nendstream", body.len())
    };
    let open = "var on = 0; function flip() { getField('f' + on).display = display.hidden; on = 1 - on; getField('f' + on).display = display.visible; } \
                var t = app.setInterval('flip()', 100);";
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R 10 0 R] /Count 2 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] /AA << /O << /S /JavaScript /JS 8 0 R >> /C << /S /JavaScript /JS 9 0 R >> >> >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (f0) /F 4 /Rect [0 0 200 300] /P 3 0 R /AP << /N 6 0 R >> >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (f1) /F 6 /Rect [0 0 200 300] /P 3 0 R /AP << /N 7 0 R >> >>".into(),
        fill("1 0 0"),
        fill("0 1 0"),
        format!("<< /Length {} >>\nstream\n{open}\nendstream", open.len()),
        "<< /Length 20 >>\nstream\napp.clearInterval(t)\nendstream".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
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

#[test]
fn js_page_and_js_advance_run_an_animation_that_page_render_draws() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-page-actions-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("anim.pdf"), animation_pdf()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let id = tool(&mut a, "doc_open", json!({ "path": "anim.pdf" }))["doc"].as_u64().unwrap();
    let centre = |a: &mut Automation| {
        let (w, h, px) = a.render_pixels(id, 1, 1.0).unwrap();
        let i = ((h / 2 * w + w / 2) * 4) as usize;
        px[i..i + 3].to_vec()
    };
    assert_eq!(centre(&mut a), [255, 0, 0]);
    let out = tool(&mut a, "js_page", json!({ "doc": id, "page": 1 }));
    assert_eq!((out["errors"].clone(), out["next_timer_in_ms"].clone()), (json!([]), json!(100.0)), "{out}");
    let out = tool(&mut a, "js_advance", json!({ "doc": id, "ms": 150 }));
    assert_eq!((out["ran"].clone(), out["pages"].clone(), out["edited"].clone()), (json!(1), json!([1]), json!(false)), "{out}");
    assert_eq!(out["next_timer_in_ms"], json!(50.0));
    assert_eq!(centre(&mut a), [0, 255, 0], "page_render draws the frame the timer showed");
    let out = tool(&mut a, "js_advance", json!({ "doc": id, "ms": 1000 }));
    assert_eq!(out["ran"], json!(10), "{out}");
    assert_eq!(centre(&mut a), [0, 255, 0]);
    assert!(a.call("js_advance", &json!({ "doc": id, "ms": 3_600_001 })).is_err());
    let out = tool(&mut a, "js_page", json!({ "doc": id, "page": 2 }));
    assert_eq!(out["next_timer_in_ms"], Value::Null, "the page's close action cleared the interval: {out}");
    let doc = a.session().get(pdfcraft_engine::DocId(id)).unwrap();
    assert!(!doc.dirty && doc.undo_depth() == 0, "the frames turned in the view only");
}

/// A page whose open action starts a 10 ms interval.
const INTERVAL: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /AA << /O << /S /JavaScript /JS (var n = 0; app.setInterval\\('n++', 10\\);) >> >> >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// `js_advance` runs at most 6,000 timers, even with one still due where the clock ends: that
/// one runs at the next call.
#[test]
fn js_advance_runs_no_more_timers_than_its_cap() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-js-advance-cap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("interval.pdf"), INTERVAL).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let id = tool(&mut a, "doc_open", json!({ "path": "interval.pdf" }))["doc"].as_u64().unwrap();
    tool(&mut a, "js_page", json!({ "doc": id, "page": 1 }));
    let out = tool(&mut a, "js_advance", json!({ "doc": id, "ms": 60_010 }));
    assert_eq!((out["ran"].clone(), out["stopped_early"].clone()), (json!(6000), json!(true)), "{out:?}");
    let out = tool(&mut a, "js_advance", json!({ "doc": id, "ms": 0 }));
    assert_eq!(out["ran"], json!(1), "{out:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
