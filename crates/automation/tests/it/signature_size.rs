//! The actual tool table honors explicit physical signature sizes without desktop state.

use pdfcraft_automation::{Automation, Content};
use pdfcraft_engine::DocId;
use serde_json::{Value, json};

fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

#[test]
fn fill_sign_explicit_size_roundtrips_through_tools_without_changing_legacy_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let session = pdfcraft_engine::Session::new();
    std::fs::write(dir.path().join("original.pdf"), session.create_blank(300.0, 400.0, 1).unwrap().as_slice()).unwrap();
    {
        let file = std::fs::File::create(dir.path().join("original.png")).unwrap();
        let mut encoder = png::Encoder::new(file, 60, 20);
        encoder.set_color(png::ColorType::Rgba);
        encoder.write_header().unwrap().write_image_data(&[20, 40, 60, 128].repeat(60 * 20)).unwrap();
    }
    let mut a = Automation::new().with_root(dir.path()).unwrap().with_clock(|| 1_700_000_000);
    let id = tool(&mut a, "doc_open", json!({"path":"original.pdf"}))["doc"].as_u64().unwrap();
    for (kind, image, size) in [
        ("signature", false, None),
        ("signature", false, Some([65.0, 17.0])),
        ("initials", false, Some([31.0, 13.0])),
        ("signature", true, Some([65.0, 17.0])),
        ("initials", true, None),
    ] {
        let mut args = json!({"doc":id,"page":1,"type":kind,"at":[30,100]});
        if image {
            args["path"] = json!("original.png");
        } else {
            args["text"] = json!("Synthetic User");
        }
        if let Some(size) = size {
            args["size"] = json!(size);
        }
        tool(&mut a, "fill_sign_add", args);
    }
    let doc = a.session().get(DocId(id)).unwrap();
    assert_eq!(doc.info.annotations.len(), 5);
    for (index, (width, height)) in
        [(None, 32.0), (Some(65.0), 17.0), (Some(31.0), 13.0), (Some(65.0), 17.0), (Some(72.0), 24.0)].into_iter().enumerate()
    {
        let size = doc.signature_placement_size(0, index).unwrap().unwrap();
        assert!((size.height() - height).abs() < 0.001, "{index}: {size:?}");
        if let Some(width) = width {
            assert!((size.width() - width).abs() < 0.001, "{index}: {size:?}");
        }
    }
    let before = a.session().save_bytes(DocId(id)).unwrap();
    for invalid in [
        json!({"type":"signature","text":"Name","size":[0,17]}),
        json!({"type":"signature","text":"Name","size":[65,-1]}),
        json!({"type":"signature","text":"Name","size":[65,14401]}),
        json!({"type":"signature","text":"Name","size":[65]}),
        json!({"type":"signature","text":"Name","size":"large"}),
        json!({"type":"signature","text":"Name","size":[65,"17"]}),
        json!({"type":"text","text":"Name","size":[65,17]}),
        json!({"type":"check","size":[65,17]}),
    ] {
        let mut args = invalid;
        args["doc"] = json!(id);
        args["page"] = json!(1);
        args["at"] = json!([30, 100]);
        assert!(a.call("fill_sign_add", &args).is_err(), "{args}");
        assert_eq!(a.session().save_bytes(DocId(id)).unwrap(), before, "{args}");
    }
    tool(&mut a, "doc_save", json!({"doc":id,"path":"placed.pdf"}));
    let reopened = tool(&mut a, "doc_open", json!({"path":"placed.pdf"}))["doc"].as_u64().unwrap();
    let size = a.session().get(DocId(reopened)).unwrap().signature_placement_size(0, 3).unwrap().unwrap();
    assert!((size.width() - 65.0).abs() < 0.001 && (size.height() - 17.0).abs() < 0.001);
    let (_, _, pixels) = a.render_pixels(reopened, 1, 1.0).unwrap();
    assert!(pixels.as_chunks::<4>().0.iter().any(|p| p[0] < 200 && p[1] < 200 && p[2] < 200), "the saved appearance must actually render");
}
