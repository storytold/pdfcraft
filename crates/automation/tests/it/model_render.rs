//! `model_render` draws a page's 3D model headlessly: a synthetic PDF holding a synthetic PRC.

use std::path::PathBuf;

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// One triangle at z = 0 facing +z, with corners (-1,-1), (1,-1) and (0,1), as a PRC file of one
/// file structure with no product tree, in hexadecimal for the stream's `/ASCIIHexDecode`.
const TRIANGLE_PRC_HEX: &str = "505243c91f0000c91f00000100000002000000030000000400000000000000000000000000000000000000010000000500000006000000070000000800000000000000060000006b0000009a000000a3000000ab000000cd000000d5000000dd000000e500000000000000505243c91f0000c91f0000050000000600000007000000080000000000000000000000000000000000000000000000789c63000000010001789c030000000001789c9b71c087758da2422b43084fe3140e038713464cd759381a162800006ed70828789c030000000001789c030000000001789c030000000001";

/// A one-page PDF whose page holds a 3D annotation over a stream of `/Subtype /{subtype}`.
fn pdf(subtype: &str, hex: &str) -> Vec<u8> {
    let stream = format!("<< /Type /3D /Subtype /{subtype} /Filter /ASCIIHexDecode /Length {} >>\nstream\n{hex}>\nendstream", hex.len() + 1);
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] >> 4 0 R] >>".into(),
        "<< /Type /Annot /Subtype /3D /Rect [20 20 180 180] /3DD 5 0 R >>".into(),
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

#[test]
fn an_orbit_frames_the_model_from_a_named_view_and_its_moves() {
    let (_dir, mut a) = setup("orbit");
    let doc = open(&mut a, "prc.pdf");
    let draw = |a: &mut Automation, orbit: Value| {
        let out = a.call("model_render", &json!({ "doc": doc, "page": 1, "width": 64, "height": 48, "perspective": false, "orbit": orbit })).unwrap();
        let Content::Png { data, .. } = &out[0] else { panic!("expected a PNG") };
        data.clone()
    };
    // The triangle lies in z = 0: the Top view looks down on its face, the Front view along its edge.
    let top = draw(&mut a, json!({ "view": "top" }));
    assert_eq!(rgb(&top, 32, 24), [190, 192, 200], "the top view sees the face head on");
    let front = draw(&mut a, json!({ "view": "front" }));
    assert!((0..48).all(|y| rgb(&front, 32, y) == [255, 255, 255]), "the front view sees only its edge");
    let tipped = draw(&mut a, json!({ "view": "front", "turn": [0, 30] }));
    assert_ne!(rgb(&tipped, 32, 24), [255, 255, 255], "tipped down from the front, the face is in the middle");
    // With y up the Front view looks along -z, onto the face.
    let y_up = draw(&mut a, json!({ "view": "front", "up": "+y" }));
    assert_eq!(rgb(&y_up, 32, 24), [190, 192, 200]);
    // Turning the front view down by 90° stops just short of the top view and sees the face.
    let turned = draw(&mut a, json!({ "view": "front", "turn": [0, 90] }));
    assert_ne!(rgb(&turned, 32, 24), [255, 255, 255]);
    // A pan moves the target right, so the model moves left; three radii take it out of the picture.
    let (left, right) = extent(&top, 24);
    let (moved_left, moved_right) = extent(&draw(&mut a, json!({ "view": "top", "pan": [0.5, 0] })), 24);
    assert!(moved_left < left && moved_right < right, "{left}..{right} moved to {moved_left}..{moved_right}");
    let panned = draw(&mut a, json!({ "view": "top", "pan": [3, 0] }));
    assert!((0..64).all(|x| rgb(&panned, x, 24) == [255, 255, 255]), "the model has left the picture");
    // Zooming in about the centre keeps the face under it and leaves no background at the corner.
    let zoomed = draw(&mut a, json!({ "view": "top", "zoom": 8 }));
    assert_eq!(rgb(&zoomed, 32, 24), [190, 192, 200]);
    assert_ne!(rgb(&zoomed, 0, 0), [255, 255, 255], "zoomed in, the face fills the corner too");
}

/// The first and last columns on row `y` that are not background.
fn extent(data: &[u8], y: usize) -> (usize, usize) {
    let drawn: Vec<usize> = (0..64).filter(|&x| rgb(data, x, y) != [255, 255, 255]).collect();
    (*drawn.first().expect("the model is on the row"), *drawn.last().expect("the model is on the row"))
}

#[test]
fn a_wrong_orbit_is_a_clear_error() {
    let (_dir, mut a) = setup("orbit-bad");
    let doc = open(&mut a, "prc.pdf");
    for bad in [
        json!({ "orbit": { "view": "side" } }),
        json!({ "orbit": { "up": "z" } }),
        json!({ "orbit": { "up": "+z", "front": "-z" } }),
        json!({ "orbit": { "turn": [1] } }),
        json!({ "orbit": { "zoom": 0 } }),
        json!({ "orbit": { "zoom_at": [0.1, 0] } }),
        json!({ "orbit": { "pan": [1, "a"] } }),
        json!({ "orbit": { "spin": 1 } }),
        json!({ "orbit": 1 }),
        json!({ "orbit": {}, "direction": [0, 0, -1] }),
    ] {
        let mut args = json!({ "doc": doc, "page": 1, "width": 16, "height": 16 });
        args.as_object_mut().unwrap().extend(bad.as_object().unwrap().clone());
        let err = a.call("model_render", &args).unwrap_err().to_string();
        assert!(err.contains("orbit"), "{bad}: {err}");
    }
    assert!(
        a.call("model_render", &json!({ "doc": doc, "page": 1, "width": 16, "height": 16, "orbit": {} })).is_ok(),
        "an empty orbit is the isometric view"
    );
}

fn json_of(mut out: Vec<Content>) -> Value {
    match out.remove(0) {
        Content::Json(v) => v,
        other => panic!("expected JSON: {other:?}"),
    }
}

#[test]
fn a_pick_finds_the_model_exactly_where_the_picture_shows_it() {
    let (_dir, mut a) = setup("pick");
    let doc = open(&mut a, "prc.pdf");
    let view = json!({ "doc": doc, "page": 1, "width": 64, "height": 48, "direction": [0, 0, -1], "perspective": false });
    let Content::Png { data, .. } = &a.call("model_render", &view).unwrap()[0] else { panic!("expected a PNG") };
    let mut drawn = 0;
    for y in (0..48).step_by(3) {
        for x in (0..64).step_by(3) {
            let mut args = view.clone();
            args["at"] = json!([x as f64 + 0.5, y as f64 + 0.5]);
            let hit = json_of(a.call("model_pick", &args).unwrap());
            let shown = rgb(data, x, y) != [255, 255, 255];
            assert_eq!(!hit["hit"].is_null(), shown, "pixel {x}, {y}: {hit}");
            drawn += usize::from(shown);
        }
    }
    assert!(drawn > 20, "the triangle fills part of the picture");
    let mut centre = view.clone();
    centre["at"] = json!([32.0, 24.0]);
    let hit = json_of(a.call("model_pick", &centre).unwrap())["hit"].clone();
    assert_eq!((hit["mesh"].as_u64(), hit["triangle"].as_u64()), (Some(1), Some(1)), "{hit}");
    assert!(hit["point"][2].as_f64().unwrap().abs() < 1e-9, "the triangle lies at z = 0: {hit}");
    assert_eq!(hit["normal"], json!([0.0, 0.0, 1.0]), "it faces +z");
    // The default perspective view meets the model too.
    let hit = json_of(a.call("model_pick", &json!({ "doc": doc, "page": 1, "at": [256, 256] })).unwrap());
    assert!(!hit["hit"].is_null(), "{hit}");
}

#[test]
fn a_wrong_pick_position_is_a_clear_error() {
    let (_dir, mut a) = setup("pick-bad");
    let doc = open(&mut a, "prc.pdf");
    for at in [json!(null), json!([1]), json!([1, 2, 3]), json!([-1, 0]), json!(["1", 0]), json!([65, 0]), json!([0, 48.5])] {
        let mut args = json!({ "doc": doc, "page": 1, "width": 64, "height": 48 });
        if !at.is_null() {
            args["at"] = at.clone();
        }
        let err = a.call("model_pick", &args).unwrap_err().to_string();
        assert!(err.contains("at must") || err.ends_with("missing argument at"), "{at}: {err}");
    }
    let err = a.call("model_pick", &json!({ "doc": doc, "page": 1, "at": [0, 0], "model": 2 })).unwrap_err().to_string();
    assert!(err.contains("has 1 3D models, not 2"), "{err}");
}
