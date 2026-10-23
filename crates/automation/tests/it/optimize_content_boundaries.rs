//! Contributor-original PDFs exercise optimization across content stream boundaries.

use std::sync::Arc;

use pdfcraft_automation::{Automation, Content};
use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_full};
use serde_json::{Value, json};

const COLORS: [[u8; 3]; 4] = [[220, 30, 40], [20, 190, 60], [30, 60, 220], [230, 190, 20]];

fn fixture(ending: &str, split_operands: bool) -> Vec<u8> {
    let mut doc = Document::new_empty();
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let font = doc.add(Object::Dict(Dict::from_iter([
        (b"Type".to_vec(), Object::name("Font")),
        (b"Subtype".to_vec(), Object::name("Type1")),
        (b"BaseFont".to_vec(), Object::name("Helvetica")),
    ])));
    let mut xobjects = Dict::new();
    for (i, color) in COLORS.iter().enumerate() {
        let dict = Dict::from_iter([
            (b"Type".to_vec(), Object::name("XObject")),
            (b"Subtype".to_vec(), Object::name("Image")),
            (b"Width".to_vec(), 20.into()),
            (b"Height".to_vec(), 5.into()),
            (b"BitsPerComponent".to_vec(), 8.into()),
            (b"ColorSpace".to_vec(), Object::name("DeviceRGB")),
        ]);
        let image = doc.add(Object::Stream(Stream::flate(dict, &color.repeat(100))));
        xobjects.set(format!("Im{i}").into_bytes(), Object::Ref(image));
    }
    // q and the translation stay active across both pieces. The optional split places
    // the final two cm operands and its operator in the next stream.
    let first = if split_operands {
        "q 1 0 0 1".to_string()
    } else {
        format!("q 1 0 0 1 10 0 cm\nBT /F1 20 Tf 30 730 Td (FIRST) Tj ET\n% trailing comment{ending}")
    };
    let mut second = if split_operands {
        "10 0 cm\nBT /F1 20 Tf 30 730 Td (FIRST) Tj ET\n".to_string()
    } else {
        "BT /F1 20 Tf 30 400 Td (HIDDEN) Tj ET\n".to_string()
    };
    for i in 0..4 {
        second.push_str(&format!("q 20 0 0 5 100 {} cm /Im{i} Do Q\n", 200 + (3 - i) * 5));
    }
    second.push_str("BT /F1 20 Tf 30 100 Td (LAST) Tj ET\nQ\n");
    let contents: Vec<Object> =
        [first, second].into_iter().map(|piece| Object::Ref(doc.add(Object::Stream(Stream::from_raw(Dict::new(), piece.into_bytes()))))).collect();
    let resources = Dict::from_iter([
        (b"Font".to_vec(), Object::Dict(Dict::from_iter([(b"F1".to_vec(), Object::Ref(font))]))),
        (b"XObject".to_vec(), Object::Dict(xobjects)),
    ]);
    let page = doc.add(Object::Dict(Dict::from_iter([
        (b"Type".to_vec(), Object::name("Page")),
        (b"Parent".to_vec(), Object::Ref(pages)),
        (b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 600.into(), 800.into()])),
        (b"Resources".to_vec(), Object::Dict(resources)),
        (b"Contents".to_vec(), Object::Array(contents)),
    ])));
    doc.update_dict(pages, |dict| {
        dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        dict.set(b"Count".to_vec(), Object::Int(1));
    })
    .unwrap();
    write_full(&doc, &SaveOptions::default()).unwrap()
}

fn call(a: &mut Automation, tool: &str, args: Value) -> Value {
    let result = a.call(tool, &args).unwrap();
    let Content::Json(value) = &result[0] else { panic!("expected JSON from {tool}") };
    value.clone()
}

fn check_page(a: &mut Automation, doc: u64, hidden: bool, phase: &str) {
    let text = call(a, "text_extract", json!({"doc": doc}));
    let words: Vec<_> = text["pages"][0]["text"].as_str().unwrap().split_whitespace().collect();
    assert_eq!(words, if hidden { vec!["FIRST", "HIDDEN", "LAST"] } else { vec!["FIRST", "LAST"] }, "{phase} text");
    let rendered = a.call("page_render", &json!({"doc": doc, "page": 1, "dpi": 72})).unwrap();
    let Content::Png { data, width, height } = &rendered[0] else { panic!("expected page PNG") };
    assert_eq!((*width, *height), (600, 800));
    let mut reader = png::Decoder::new(std::io::Cursor::new(data)).read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    let channels = info.color_type.samples();
    assert!(channels >= 3);
    let pixel = |x: usize, y: usize| &pixels[(y * 600 + x) * channels..(y * 600 + x) * channels + 3];
    let dark = |left, top, right, bottom| {
        (top..bottom).flat_map(|y| (left..right).map(move |x| (x, y))).filter(|&(x, y)| pixel(x, y).iter().any(|v| *v < 128)).count()
    };
    assert!(dark(35, 45, 130, 75) > 30, "FIRST must draw at its translated position");
    assert!(dark(35, 675, 130, 705) > 30, "LAST must draw at its translated position");
    assert_eq!(dark(35, 375, 150, 405) > 0, hidden, "HIDDEN region must match the real comment ending");
    for (row, color) in COLORS.iter().enumerate() {
        for y in 581 + row * 5..584 + row * 5 {
            for x in 112..128 {
                assert_eq!(pixel(x, y), color, "strip {row}, pixel {x},{y}");
            }
        }
    }
    assert_eq!(pixel(105, 590), [255, 255, 255], "the cross-piece translation stays active");
}

fn check_saved_images(bytes: Vec<u8>, merged: bool) {
    let doc = Document::open(Arc::new(bytes)).unwrap();
    let root = doc.get(doc.root().unwrap());
    let pages = doc.get(root.as_dict().unwrap().reference(b"Pages").unwrap());
    let kids = pages.as_dict().unwrap().get(b"Kids").unwrap().as_array().unwrap();
    let page = doc.resolve(&kids[0]);
    let resources = doc.resolve(page.as_dict().unwrap().get(b"Resources").unwrap());
    let xobjects = doc.resolve(resources.as_dict().unwrap().get(b"XObject").unwrap());
    let images: Vec<_> = xobjects.as_dict().unwrap().iter().map(|(_, value)| doc.resolve(value)).collect();
    assert_eq!(images.len(), if merged { 1 } else { 4 });
    if merged {
        let Object::Stream(image) = &*images[0] else { panic!("expected saved image") };
        assert_eq!((image.dict.int(b"Width"), image.dict.int(b"Height")), (Some(20), Some(20)));
        let expected: Vec<_> = COLORS.iter().flat_map(|color| color.repeat(100)).collect();
        assert_eq!(image.decoded().unwrap(), expected, "saved RGB rows keep their strip order");
    }
}

fn roundtrip(ending: &str, split_operands: bool, merge: bool) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("input.pdf"), fixture(ending, split_operands)).unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let doc = call(&mut a, "doc_open", json!({"path": "input.pdf"}))["doc"].as_u64().unwrap();
    let hidden = !ending.is_empty() && !split_operands;
    check_page(&mut a, doc, hidden, "original");
    let report = call(
        &mut a,
        "doc_optimize",
        json!({
            "doc": doc, "path": "output.pdf", "merge_fragments": merge,
            "color": {"downsample": false, "compression": "retain"},
            "gray": {"downsample": false, "compression": "retain"}
        }),
    );
    assert_eq!(report["fragments_merged"], if merge { 4 } else { 0 });
    assert_eq!(report["merged_images"], if merge { 1 } else { 0 });
    let reopened = call(&mut a, "doc_open", json!({"path": "output.pdf"}))["doc"].as_u64().unwrap();
    check_page(&mut a, reopened, hidden, "reopened");
    check_saved_images(std::fs::read(dir.path().join("output.pdf")).unwrap(), merge);
}

#[test]
fn fragment_merge_preserves_a_comment_across_content_pieces() {
    roundtrip("", false, true);
}

#[test]
fn fragment_merge_preserves_explicit_comment_line_endings() {
    for ending in ["\n", "\r", "\r\n"] {
        roundtrip(ending, false, true);
    }
}

#[test]
fn fragment_merge_preserves_split_operands_and_graphics_state() {
    roundtrip("", true, true);
}

#[test]
fn disabled_fragment_merge_preserves_content_pieces() {
    roundtrip("", false, false);
}
