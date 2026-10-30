//! Contributor-original shared resources exercise the public extract and split tools.

use pdfcraft_automation::{Automation, Content};
use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_full};
use serde_json::{Value, json};

fn fixture() -> Vec<u8> {
    let mut doc = Document::new_empty();
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let mut images = Dict::new();
    for (name, color) in [("Page", [50, 50, 50]), ("Stamp", [0, 180, 0])] {
        let dict = Dict::from_iter([
            (b"Type".to_vec(), Object::name("XObject")),
            (b"Subtype".to_vec(), Object::name("Image")),
            (b"Width".to_vec(), 1.into()),
            (b"Height".to_vec(), 1.into()),
            (b"BitsPerComponent".to_vec(), 8.into()),
            (b"ColorSpace".to_vec(), Object::name("DeviceRGB")),
        ]);
        let image = doc.add(Object::Stream(Stream::from_raw(dict, color.to_vec())));
        images.set(name.as_bytes().to_vec(), Object::Ref(image));
    }
    let resources = doc.add(Dict::from_iter([(b"XObject".to_vec(), Object::Dict(images))]));
    let appearance = doc.add(Object::Stream(Stream::from_raw(
        Dict::from_iter([
            (b"Type".to_vec(), Object::name("XObject")),
            (b"Subtype".to_vec(), Object::name("Form")),
            (b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 1.into(), 1.into()])),
            (b"Resources".to_vec(), Object::Ref(resources)),
        ]),
        b"/Stamp Do".to_vec(),
    )));
    let annotation = doc.add(Dict::from_iter([
        (b"Type".to_vec(), Object::name("Annot")),
        (b"Subtype".to_vec(), Object::name("Stamp")),
        (b"Rect".to_vec(), Object::Array(vec![50.into(), 50.into(), 110.into(), 110.into()])),
        (b"F".to_vec(), 4.into()),
        (b"AP".to_vec(), Object::Dict(Dict::from_iter([(b"N".to_vec(), Object::Ref(appearance))]))),
    ]));
    let contents = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"q 20 0 0 20 10 10 cm /Page Do Q".to_vec())));
    let mut kids = Vec::new();
    for _ in 0..2 {
        let page = doc.add(Dict::from_iter([
            (b"Type".to_vec(), Object::name("Page")),
            (b"Parent".to_vec(), Object::Ref(pages)),
            (b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 160.into(), 160.into()])),
            (b"Resources".to_vec(), Object::Ref(resources)),
            (b"Contents".to_vec(), Object::Ref(contents)),
            (b"Annots".to_vec(), Object::Array(vec![Object::Ref(annotation)])),
        ]));
        kids.push(Object::Ref(page));
    }
    doc.update_dict(pages, |pages| {
        pages.set(b"Kids".to_vec(), Object::Array(kids));
        pages.set(b"Count".to_vec(), 2);
    })
    .unwrap();
    write_full(&doc, &SaveOptions::default()).unwrap()
}

fn call(a: &mut Automation, tool: &str, args: Value) -> Value {
    a.call(tool, &args).unwrap().into_iter().find_map(|content| if let Content::Json(value) = content { Some(value) } else { None }).unwrap()
}

fn render(a: &mut Automation, doc: u64) -> Vec<u8> {
    let result = a.call("page_render", &json!({"doc":doc,"page":1,"dpi":72})).unwrap();
    let Content::Png { data, width, height } = &result[0] else { panic!("expected PNG") };
    assert_eq!((*width, *height), (160, 160));
    data.clone()
}

#[test]
fn extract_and_split_preserve_shared_stamp_pixels_after_save_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let source = fixture();
    std::fs::write(dir.path().join("source.pdf"), &source).unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let doc = call(&mut a, "doc_open", json!({"path":"source.pdf"}))["doc"].as_u64().unwrap();
    let before = render(&mut a, doc);
    let extracted = call(&mut a, "page_extract", json!({"doc":doc,"pages":[1],"out":"extracted.pdf","open":true}));
    let extracted = extracted["document"]["doc"].as_u64().unwrap();
    assert_eq!(render(&mut a, extracted), before);
    call(&mut a, "doc_save", json!({"doc":extracted,"path":"extracted-saved.pdf","full":true}));
    call(&mut a, "doc_close", json!({"doc":extracted}));
    let reopened = call(&mut a, "doc_open", json!({"path":"extracted-saved.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(render(&mut a, reopened), before);
    let split = call(&mut a, "doc_split", json!({"doc":doc,"every":1,"out_dir":"parts"}));
    for file in split["files"].as_array().unwrap() {
        let part = call(&mut a, "doc_open", json!({"path":file["path"]}))["doc"].as_u64().unwrap();
        assert_eq!(render(&mut a, part), before);
    }
    assert_eq!(std::fs::read(dir.path().join("source.pdf")).unwrap(), source);
    let info = call(&mut a, "doc_info", json!({"doc":doc}));
    assert_eq!(info["document"]["dirty"], false);
}
