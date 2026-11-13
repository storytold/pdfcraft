//! `model_render` draws a page's 3D model headlessly: a synthetic PDF holding a synthetic PRC.

use std::path::PathBuf;

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// One triangle at z = 0 facing +z, with corners (-1,-1), (1,-1) and (0,1), as a PRC file of one
/// file structure with no product tree, in hexadecimal for the stream's `/ASCIIHexDecode`.
const TRIANGLE_PRC_HEX: &str = "505243c91f0000c91f00000100000002000000030000000400000000000000000000000000000000000000010000000500000006000000070000000800000000000000060000006b0000009a000000a3000000ab000000cd000000d5000000dd000000e500000000000000505243c91f0000c91f0000050000000600000007000000080000000000000000000000000000000000000000000000789c63000000010001789c030000000001789c9b71c087758da2422b43084fe3140e038713464cd759381a162800006ed70828789c030000000001789c030000000001789c030000000001";

/// The same triangle stored as a `TESS_3D_Compressed` whose normals are recalculated, so it is
/// drawn only once its triangles are rebuilt. `prc-render`'s tests build it
/// (`the_automation_fixture_is_the_compressed_triangle` keeps this copy their output).
const COMPRESSED_TRIANGLE_PRC_HEX: &str = "505243c91f0000c91f00000100000002000000030000000400000000000000000000000000000000000000010000000500000006000000070000000800000000000000060000006b0000009a000000a3000000ab000000d2000000da000000e2000000ea00000000000000505243c91f0000c91f0000050000000600000007000000080000000000000000000000000000000000000000000000789c63000000010001789c030000000001789c9b71c08775aded1d0624d0d2c808055a8d0c8c8ccc0c0c0c0a0a223ce2e50c2c00bed0061f789c030000000001789c030000000001789c030000000001";

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
fn a_compressed_prc_model_is_rebuilt_and_drawn() {
    let (dir, mut a) = setup("prc-compressed");
    std::fs::write(dir.join("in/compressed.pdf"), pdf("PRC", COMPRESSED_TRIANGLE_PRC_HEX)).unwrap();
    let doc = open(&mut a, "compressed.pdf");
    let args = json!({ "doc": doc, "page": 1, "width": 64, "height": 48, "direction": [0, 0, -1], "perspective": false });
    let out = a.call("model_render", &args).unwrap();
    let Content::Png { data, .. } = &out[0] else { panic!("expected a PNG") };
    assert_eq!(rgb(data, 0, 0), [255, 255, 255], "the corner is background");
    assert_eq!(rgb(data, 32, 24), [190, 192, 200], "the centre is the triangle, lit head on");
    // Its apex is up: the drawn span narrows from the base to the top, and the centre column
    // is covered over many rows.
    let (top, bottom) = rows(data, 32);
    assert!(bottom - top > 20, "rows {top}..{bottom}");
    let (near_top, near_bottom) = (extent(data, top + 4), extent(data, bottom - 2));
    assert!(near_bottom.1 - near_bottom.0 > 2 * (near_top.1 - near_top.0), "{near_top:?} {near_bottom:?}");
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
    let draw = |a: &mut Automation, orbit: Value| orbit_picture(a, doc, false, &orbit);
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

/// The first and last rows on column `x` that are not background.
fn rows(data: &[u8], x: usize) -> (usize, usize) {
    let drawn: Vec<usize> = (0..48).filter(|&y| rgb(data, x, y) != [255, 255, 255]).collect();
    (*drawn.first().expect("the model is on the column"), *drawn.last().expect("the model is on the column"))
}

/// A 64 by 48 picture of the model through `orbit`.
fn orbit_picture(a: &mut Automation, doc: u64, perspective: bool, orbit: &Value) -> Vec<u8> {
    let out =
        a.call("model_render", &json!({ "doc": doc, "page": 1, "width": 64, "height": 48, "perspective": perspective, "orbit": orbit })).unwrap();
    let Content::Png { data, .. } = &out[0] else { panic!("expected a PNG") };
    data.clone()
}

#[test]
fn an_empty_orbit_frames_the_isometric_view_in_perspective() {
    let (_dir, mut a) = setup("orbit-default");
    let doc = open(&mut a, "prc.pdf");
    let default = orbit_picture(&mut a, doc, false, &json!({}));
    assert_ne!(rgb(&default, 32, 24), [255, 255, 255], "the model is centred");
    let (left, right) = extent(&default, 24);
    let (high, low) = rows(&default, 32);
    assert!(left > 0 && right < 63 && high > 0 && low < 47, "inside the picture: {left}..{right}, {high}..{low}");
    assert!(right - left > 16, "and fills it: {left}..{right}");
    assert_eq!(default, orbit_picture(&mut a, doc, false, &json!({ "view": "iso", "up": "+z", "front": "+y" })), "the defaults");
    let unset = a.call("model_render", &json!({ "doc": doc, "page": 1, "width": 64, "height": 48, "orbit": {} })).unwrap();
    let Content::Png { data: unset, .. } = &unset[0] else { panic!("expected a PNG") };
    assert_eq!(*unset, orbit_picture(&mut a, doc, true, &json!({})), "perspective is the default");
    assert_ne!(*unset, default, "and shows");
}

#[test]
fn zoom_at_keeps_the_point_under_it_still() {
    let (_dir, mut a) = setup("orbit-zoom-at");
    let doc = open(&mut a, "prc.pdf");
    let (left, right) = extent(&orbit_picture(&mut a, doc, false, &json!({ "view": "top", "zoom": 2 })), 24);
    // A point 0.2 picture heights off centre stays there, so what was at the centre moves
    // 0.2 x (2 - 1) x 48 = 9.6 pixels the other way.
    for (at, sign) in [(0.2, -1.0), (-0.2, 1.0)] {
        let (l, r) = extent(&orbit_picture(&mut a, doc, false, &json!({ "view": "top", "zoom": 2, "zoom_at": [at, 0] })), 24);
        for (was, now) in [(left, l), (right, r)] {
            let moved = now as f64 - was as f64;
            assert!((moved - sign * 9.6).abs() <= 1.5, "at {at}: {was} moved to {now}");
        }
    }
}

#[test]
fn the_zoom_is_held_within_its_range() {
    let (_dir, mut a) = setup("orbit-zoom-range");
    let doc = open(&mut a, "prc.pdf");
    let far = orbit_picture(&mut a, doc, false, &json!({ "view": "top", "zoom": 0.05 }));
    assert_eq!(far, orbit_picture(&mut a, doc, false, &json!({ "view": "top", "zoom": 0.001 })), "the far end");
    assert_ne!(far, orbit_picture(&mut a, doc, false, &json!({ "view": "top", "zoom": 0.2 })));
    // Panned to just inside the near edge, so a nearer view would move the edge in the picture.
    let close = |z: f64| json!({ "view": "top", "zoom": z, "pan": [0, -0.7] });
    let near = orbit_picture(&mut a, doc, false, &close(50.0));
    assert_eq!(near, orbit_picture(&mut a, doc, false, &close(1000.0)), "the near end");
    assert_ne!(near, orbit_picture(&mut a, doc, false, &close(25.0)));
}

#[test]
fn an_orbit_draws_in_perspective_at_the_same_scale() {
    let (_dir, mut a) = setup("orbit-perspective");
    let doc = open(&mut a, "prc.pdf");
    let tipped = json!({ "view": "front", "turn": [0, 30] });
    let deep = orbit_picture(&mut a, doc, true, &tipped);
    assert_ne!(rgb(&deep, 32, 24), [255, 255, 255]);
    assert_ne!(deep, orbit_picture(&mut a, doc, false, &tipped), "foreshortened");
    let top = json!({ "view": "top" });
    let (left, right) = extent(&orbit_picture(&mut a, doc, true, &top), 24);
    let (flat_left, flat_right) = extent(&orbit_picture(&mut a, doc, false, &top), 24);
    assert!(left.abs_diff(flat_left) <= 2 && right.abs_diff(flat_right) <= 2, "{left}..{right} against {flat_left}..{flat_right}");
}

#[test]
fn an_orbit_front_yaw_and_upward_pan_each_move_the_picture() {
    let (_dir, mut a) = setup("orbit-moves");
    let doc = open(&mut a, "prc.pdf");
    let mut draw = |orbit: Value| orbit_picture(&mut a, doc, false, &orbit);
    // Yawing the Front view by 90 degrees is the Right view, which looks along -x; +x is its opposite.
    let right_view = draw(json!({ "view": "right", "turn": [0, 30] }));
    assert_eq!(right_view, draw(json!({ "view": "front", "turn": [90, 30] })), "yaw");
    assert_eq!(right_view, draw(json!({ "view": "front", "front": "-x", "turn": [0, 30] })), "front -x");
    assert_ne!(right_view, draw(json!({ "view": "front", "turn": [0, 30] })));
    assert_eq!(draw(json!({ "view": "right", "turn": [180, 30] })), draw(json!({ "view": "front", "front": "+x", "turn": [0, 30] })), "front +x");
    // From above, the triangle's apex is up the picture; a quarter yaw lays it along the row,
    // twice as wide there.
    let top = draw(json!({ "view": "top" }));
    let (left, right) = extent(&top, 24);
    let (yaw_left, yaw_right) = extent(&draw(json!({ "view": "top", "turn": [90, 0] })), 24);
    assert!(yaw_right - yaw_left > (right - left) * 3 / 2, "{left}..{right} yawed to {yaw_left}..{yaw_right}");
    // A pan up moves the target up, so the model moves down.
    let (high, low) = rows(&top, 32);
    let (moved_high, moved_low) = rows(&draw(json!({ "view": "top", "pan": [0, 0.25] })), 32);
    assert!(moved_high > high && moved_low >= low, "{high}..{low} moved to {moved_high}..{moved_low}");
}

#[test]
fn a_wrong_orbit_is_a_clear_error() {
    let (_dir, mut a) = setup("orbit-bad");
    let doc = open(&mut a, "prc.pdf");
    for (bad, reason) in [
        (json!({ "orbit": { "view": "side" } }), "view as one of"),
        (json!({ "orbit": { "view": 1 } }), "view as a string"),
        (json!({ "orbit": { "up": "z" } }), "up as one of"),
        (json!({ "orbit": { "up": 1 } }), "up as a string"),
        (json!({ "orbit": { "front": "y" } }), "front as one of"),
        (json!({ "orbit": { "up": "+z", "front": "-z" } }), "front perpendicular to up"),
        (json!({ "orbit": { "turn": [1] } }), "turn as [yaw, pitch]"),
        (json!({ "orbit": { "turn": [1, 2, 3] } }), "turn as [yaw, pitch]"),
        (json!({ "orbit": { "turn": 1 } }), "turn as [yaw, pitch]"),
        (json!({ "orbit": { "zoom": 0 } }), "zoom as a positive number"),
        (json!({ "orbit": { "zoom": -2 } }), "zoom as a positive number"),
        (json!({ "orbit": { "zoom": "a" } }), "zoom as a positive number"),
        (json!({ "orbit": { "zoom_at": [0.1, 0] } }), "with zoom when it has zoom_at"),
        (json!({ "orbit": { "zoom": 2, "zoom_at": [1] } }), "zoom_at as [right, up]"),
        (json!({ "orbit": { "zoom": 2, "zoom_at": [1, 2, 3] } }), "zoom_at as [right, up]"),
        (json!({ "orbit": { "pan": [1, "a"] } }), "pan as [right, up]"),
        (json!({ "orbit": { "pan": [1] } }), "pan as [right, up]"),
        (json!({ "orbit": { "pan": [1, 2, 3] } }), "pan as [right, up]"),
        (json!({ "orbit": { "spin": 1 } }), "without spin"),
        (json!({ "orbit": 1 }), "an object"),
        (json!({ "orbit": {}, "direction": [0, 0, -1] }), "left out when direction is given"),
    ] {
        let mut args = json!({ "doc": doc, "page": 1, "width": 16, "height": 16 });
        args.as_object_mut().unwrap().extend(bad.as_object().unwrap().clone());
        let err = a.call("model_render", &args).unwrap_err().to_string();
        assert!(err.contains("orbit must be") && err.contains(reason), "{bad}: {err}");
    }
}
