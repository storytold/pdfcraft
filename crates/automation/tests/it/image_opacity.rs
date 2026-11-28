//! `image_edit` opacity end to end: what `page_render` draws, `page_images.opacity`, undo, and
//! save and reopen.

use pdfcraft_automation::{Automation, Content, ToolError};
use serde_json::{Value, json};

/// One 100 × 100 pt page covered by a 1 × 1 black picture.
fn fixture() -> Vec<u8> {
    let content = "q 100 0 0 100 0 0 cm /Im0 Do Q";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << /XObject << /Im0 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\n\u{0}\nendstream"
            .into(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
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

/// The grey level at the middle of page 1 rendered at 72 dpi.
fn middle(a: &mut Automation, doc: u64) -> u8 {
    let rendered = a.call("page_render", &json!({ "doc": doc, "page": 1, "dpi": 72 })).unwrap();
    let Content::Png { data, width, .. } = &rendered[0] else { panic!("expected a PNG") };
    let rgba = craft_image_codecs::decode(data).unwrap().to_rgba8();
    rgba[(50 * *width as usize + 50) * 4]
}

fn opacity(a: &mut Automation, doc: u64) -> f64 {
    ok(a, "page_images", json!({ "doc": doc, "page": 1 }))["images"][0]["opacity"].as_f64().unwrap()
}

#[test]
fn image_edit_sets_opacity_that_renders_undoes_and_saves() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-automation-image-opacity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.pdf"), fixture()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let black = middle(&mut a, doc);
    assert!((opacity(&mut a, doc) - 1.0).abs() < 1e-6);

    let r = ok(&mut a, "image_edit", json!({ "doc": doc, "page": 1, "image": 1, "action": "opacity", "opacity": 0.5 }));
    assert_eq!(r["undo"], "Set opacity");
    assert!((opacity(&mut a, doc) - 0.5).abs() < 1e-6);
    let half = middle(&mut a, doc);
    assert!(half > black.saturating_add(80) && half < 200, "half-transparent black over white is mid grey: {black} then {half}");

    for bad in [json!(-0.5), json!(2), json!("half"), json!(null)] {
        let refused = a.call("image_edit", &json!({ "doc": doc, "page": 1, "image": 1, "action": "opacity", "opacity": bad }));
        assert!(matches!(refused, Err(ToolError::InvalidArgs(_))), "opacity {bad} must be refused");
    }

    ok(&mut a, "edit_undo", json!({ "doc": doc }));
    assert_eq!(middle(&mut a, doc), black, "undo makes it opaque again");
    ok(&mut a, "edit_redo", json!({ "doc": doc }));
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "out.pdf" }));
    let saved = ok(&mut a, "doc_open", json!({ "path": "out.pdf" }))["doc"].as_u64().unwrap();
    assert!((opacity(&mut a, saved) - 0.5).abs() < 1e-6, "after save and reopen");
    assert_eq!(middle(&mut a, saved), half);
    let _ = std::fs::remove_dir_all(&dir);
}
