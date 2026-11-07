//! `model_render` draws a page's 3D model headlessly: a synthetic PDF holding a synthetic PRC.

use std::path::PathBuf;

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// One triangle at z = 0 facing +z, with corners (-1,-1), (1,-1) and (0,1), as a PRC file of one
/// file structure with no product tree, in hexadecimal for the stream's `/ASCIIHexDecode`.
const TRIANGLE_PRC_HEX: &str = "505243c91f0000c91f00000100000002000000030000000400000000000000000000000000000000000000010000000500000006000000070000000800000000000000060000006b0000009a000000a3000000ab000000cd000000d5000000dd000000e500000000000000505243c91f0000c91f0000050000000600000007000000080000000000000000000000000000000000000000000000789c63000000010001789c030000000001789c9b71c087758da2422b43084fe3140e038713464cd759381a162800006ed70828789c030000000001789c030000000001789c030000000001";

/// A one-page PDF whose page holds a 3D annotation over a stream of `/Subtype /{subtype}`.
fn pdf(subtype: &str, hex: &str) -> Vec<u8> {
    pdf_with(subtype, hex, "")
}

/// As [`pdf`], with `extra` entries in the 3D annotation (a `/3DV`, say).
fn pdf_with(subtype: &str, hex: &str, extra: &str) -> Vec<u8> {
    let stream = format!("<< /Type /3D /Subtype /{subtype} /Filter /ASCIIHexDecode /Length {} >>\nstream\n{hex}>\nendstream", hex.len() + 1);
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] >> 4 0 R] >>".into(),
        format!("<< /Type /Annot /Subtype /3D /Rect [20 20 180 180] /3DD 5 0 R {extra} >>"),
        stream,
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

/// A fresh folder with `prc.pdf` and `u3d.pdf` in `in`, `outside.pdf` beside it, and an
/// automation rooted at `in`.
fn setup(test: &str) -> (PathBuf, Automation) {
    let dir = std::env::temp_dir().join(format!("pdfcraft-model3d-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("in")).unwrap();
    std::fs::write(dir.join("in/prc.pdf"), pdf("PRC", TRIANGLE_PRC_HEX)).unwrap();
    std::fs::write(dir.join("in/u3d.pdf"), pdf("U3D", "553344")).unwrap();
    std::fs::write(dir.join("outside.pdf"), pdf("PRC", TRIANGLE_PRC_HEX)).unwrap();
    // Looking down -z from 10 above, y up, 1 / 0.25 = 4 units across the annotation, on blue.
    let view = "/3DV << /Type /3DView /XN (Top) /C2W [1 0 0 0 1 0 0 0 -1 0 0 10] /CO 10 \
                /P << /Subtype /O /OS 0.25 /OB /Min >> /BG << /C [0 0 1] >> >>";
    std::fs::write(dir.join("in/view.pdf"), pdf_with("PRC", TRIANGLE_PRC_HEX, view)).unwrap();
    let a = Automation::new().with_root(dir.join("in")).unwrap();
    (dir, a)
}

fn open(a: &mut Automation, path: &str) -> u64 {
    match a.call("doc_open", &json!({ "path": path })).unwrap().remove(0) {
        Content::Json(v) => v["doc"].as_u64().unwrap(),
        other => panic!("expected JSON: {other:?}"),
    }
}

/// The RGB of pixel (x, y) of a PNG.
fn rgb(data: &[u8], x: usize, y: usize) -> [u8; 3] {
    let mut reader = png::Decoder::new(std::io::Cursor::new(data)).read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    let channels = info.line_size / info.width as usize;
    let at = y * info.line_size + x * channels;
    [pixels[at], pixels[at + 1], pixels[at + 2]]
}

#[test]
fn a_prc_model_is_drawn_over_the_background() {
    let (_dir, mut a) = setup("prc");
    let doc = open(&mut a, "prc.pdf");
    let args = json!({ "doc": doc, "page": 1, "width": 64, "height": 48, "direction": [0, 0, -1], "perspective": false });
    let out = a.call("model_render", &args).unwrap();
    let Content::Png { data, width, height } = &out[0] else { panic!("expected a PNG") };
    assert_eq!((*width, *height), (64, 48));
    assert_eq!(rgb(data, 0, 0), [255, 255, 255], "the corner is background");
    assert_eq!(rgb(data, 32, 24), [190, 192, 200], "the centre is the triangle, lit head on");
    // In perspective the headlight meets the face at a slight angle, so only not-background holds.
    let out = a.call("model_render", &json!({ "doc": doc, "page": 1, "width": 64, "height": 48, "direction": [0, 0, -1] })).unwrap();
    let Content::Png { data, .. } = &out[0] else { panic!("expected a PNG") };
    assert_ne!(rgb(data, 32, 24), [255, 255, 255], "the centre is drawn in perspective too");
    // The same model named explicitly, and the default isometric view, both draw.
    assert!(a.call("model_render", &json!({ "doc": doc, "page": 1, "model": 1 })).is_ok());
}

#[test]
fn a_model_is_drawn_as_its_saved_view_shows_it_unless_asked_to_fit() {
    let (_dir, mut a) = setup("view");
    let doc = open(&mut a, "view.pdf");
    let png = |a: &mut Automation, args: Value| match a.call("model_render", &args).unwrap().remove(0) {
        Content::Png { data, .. } => data,
        other => panic!("expected a PNG: {other:?}"),
    };
    // By default, the saved view: its blue background, and the 2-unit triangle across half of
    // the 4 units the view spans, centred.
    let saved = png(&mut a, json!({ "doc": doc, "page": 1, "width": 64, "height": 64 }));
    assert_eq!(rgb(&saved, 1, 1), [0, 0, 255], "the view's background");
    assert_eq!(rgb(&saved, 32, 33), [190, 192, 200], "the triangle, head on");
    assert_eq!(rgb(&saved, 32, 6), [0, 0, 255], "the triangle's apex is a quarter of the way down");
    assert_eq!(png(&mut a, json!({ "doc": doc, "page": 1, "width": 64, "height": 64, "view": "saved" })), saved);
    // "fit", or a direction, frames it to fill the image on white instead.
    let fit = png(&mut a, json!({ "doc": doc, "page": 1, "width": 64, "height": 64, "view": "fit" }));
    assert_eq!(rgb(&fit, 1, 1), [255, 255, 255]);
    let along = png(&mut a, json!({ "doc": doc, "page": 1, "width": 64, "height": 64, "direction": [0, 0, -1] }));
    assert_eq!(rgb(&along, 1, 1), [255, 255, 255]);
    // A model without a saved view can't be asked for one; a wrong view name is refused.
    let plain = open(&mut a, "prc.pdf");
    let err = a.call("model_render", &json!({ "doc": plain, "page": 1, "view": "saved" })).unwrap_err().to_string();
    assert!(err.contains("no saved view"), "{err}");
    assert!(a.call("model_render", &json!({ "doc": doc, "page": 1, "view": "front" })).is_err());
}

#[test]
fn a_u3d_model_or_a_missing_one_is_a_clear_error() {
    let (_dir, mut a) = setup("u3d");
    let doc = open(&mut a, "u3d.pdf");
    let err = a.call("model_render", &json!({ "doc": doc, "page": 1 })).unwrap_err().to_string();
    assert!(err.contains("U3D"), "{err}");
    let doc = open(&mut a, "prc.pdf");
    let err = a.call("model_render", &json!({ "doc": doc, "page": 1, "model": 2 })).unwrap_err().to_string();
    assert!(err.contains("has 1 3D models, not 2"), "{err}");
    for bad in [json!({ "direction": [0, 0, 0] }), json!({ "direction": [1, 2] }), json!({ "width": 0 }), json!({ "height": 5000 })] {
        let mut args = json!({ "doc": doc, "page": 1 });
        args.as_object_mut().unwrap().extend(bad.as_object().unwrap().clone());
        assert!(a.call("model_render", &args).is_err(), "{bad}");
    }
}

#[test]
fn a_model_outside_the_root_is_never_opened_to_be_drawn() {
    let (dir, mut a) = setup("root");
    let outside = dir.join("outside.pdf");
    for path in [Value::from("../outside.pdf"), Value::from(outside.to_string_lossy().into_owned())] {
        assert!(a.call("doc_open", &json!({ "path": path })).is_err(), "{path} lies outside the root");
    }
    // The tool takes no path of its own: only a document opened inside the root reaches it.
    assert!(a.call("model_render", &json!({ "doc": 1, "page": 1 })).is_err());
    let doc = open(&mut a, "prc.pdf");
    assert!(a.call("model_render", &json!({ "doc": doc, "page": 1 })).is_ok());
}
