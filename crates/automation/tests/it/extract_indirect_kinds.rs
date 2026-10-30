//! Contributor-original colored Type 3 glyphs exercise indirect kind metadata through tools.

use pdfcraft_automation::{Automation, Content};
use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_full};
use serde_json::{Value, json};

fn fixture(indirect: bool) -> Vec<u8> {
    let mut doc = Document::new_empty();
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let mut images = Dict::new();
    for (name, color) in [("Small", [50, 50, 50]), ("Paint", [0, 180, 0])] {
        let attrs = Dict::from_iter([
            (b"Subtype".to_vec(), Object::name("Image")),
            (b"Width".to_vec(), 1.into()),
            (b"Height".to_vec(), 1.into()),
            (b"BitsPerComponent".to_vec(), 8.into()),
            (b"ColorSpace".to_vec(), Object::name("DeviceRGB")),
        ]);
        let image = doc.add(Object::Stream(Stream::from_raw(attrs, color.to_vec())));
        images.set(name.as_bytes().to_vec(), Object::Ref(image));
    }
    let images = doc.add(images);
    let program = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"1000 0 d0 q 1000 0 0 1000 0 0 cm /Paint Do Q".to_vec())));
    let subtype = if indirect { Object::Ref(doc.add(Object::name("Type3"))) } else { Object::name("Type3") };
    let font = doc.add(Dict::from_iter([
        (b"Type".to_vec(), Object::name("Font")),
        (b"Subtype".to_vec(), subtype),
        (b"FontBBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 1000.into(), 1000.into()])),
        (b"FontMatrix".to_vec(), Object::Array(vec![Object::Real(0.001), 0.into(), 0.into(), Object::Real(0.001), 0.into(), 0.into()])),
        (b"CharProcs".to_vec(), Object::Dict(Dict::from_iter([(b"A".to_vec(), Object::Ref(program))]))),
        (b"Encoding".to_vec(), Object::Dict(Dict::from_iter([(b"Differences".to_vec(), Object::Array(vec![65.into(), Object::name("A")]))]))),
        (b"FirstChar".to_vec(), 65.into()),
        (b"LastChar".to_vec(), 65.into()),
        (b"Widths".to_vec(), Object::Array(vec![1000.into()])),
        (b"Resources".to_vec(), Object::Dict(Dict::from_iter([(b"XObject".to_vec(), Object::Ref(images))]))),
    ]));
    let resources = doc.add(Dict::from_iter([
        (b"XObject".to_vec(), Object::Ref(images)),
        (b"Font".to_vec(), Object::Dict(Dict::from_iter([(b"T3".to_vec(), Object::Ref(font))]))),
    ]));
    let contents =
        doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"q 20 0 0 20 10 10 cm /Small Do Q BT /T3 50 Tf 50 80 Td (A) Tj ET".to_vec())));
    let page = doc.add(Dict::from_iter([
        (b"Type".to_vec(), Object::name("Page")),
        (b"Parent".to_vec(), Object::Ref(pages)),
        (b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 160.into(), 160.into()])),
        (b"Resources".to_vec(), Object::Ref(resources)),
        (b"Contents".to_vec(), Object::Ref(contents)),
    ]));
    doc.update_dict(pages, |pages| {
        pages.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        pages.set(b"Count".to_vec(), 1);
    })
    .unwrap();
    write_full(&doc, &SaveOptions::default()).unwrap()
}

fn call(a: &mut Automation, tool: &str, args: Value) -> Value {
    a.call(tool, &args).unwrap().into_iter().find_map(|content| if let Content::Json(value) = content { Some(value) } else { None }).unwrap()
}

fn render(a: &mut Automation, doc: u64) -> Vec<u8> {
    let result = a.call("page_render", &json!({"doc":doc,"page":1,"dpi":72})).unwrap();
    let Content::Png { data, .. } = &result[0] else { panic!("expected PNG") };
    data.clone()
}

#[test]
fn extract_and_split_keep_indirect_type3_artwork_after_save_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    for (name, indirect) in [("direct", false), ("indirect", true)] {
        let source = fixture(indirect);
        std::fs::write(dir.path().join(format!("{name}.pdf")), &source).unwrap();
        let doc = call(&mut a, "doc_open", json!({"path":format!("{name}.pdf")}))["doc"].as_u64().unwrap();
        let before = render(&mut a, doc);
        let mut reader = png::Decoder::new(std::io::Cursor::new(&before)).read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        let channels = info.color_type.samples();
        assert!(channels >= 3);
        let green = pixels[..info.buffer_size()].chunks_exact(channels).filter(|pixel| pixel[1] > 100 && pixel[0] < 50 && pixel[2] < 50).count();
        assert_eq!(green, 2500, "the original contains a visible colored glyph");
        let extracted =
            call(&mut a, "page_extract", json!({"doc":doc,"pages":[1],"out":format!("{name}-extracted.pdf"),"open":true}))["document"]["doc"]
                .as_u64()
                .unwrap();
        assert_eq!(render(&mut a, extracted), before, "{name} extraction");
        call(&mut a, "doc_save", json!({"doc":extracted,"path":format!("{name}-saved.pdf"),"full":true}));
        call(&mut a, "doc_close", json!({"doc":extracted}));
        let saved = call(&mut a, "doc_open", json!({"path":format!("{name}-saved.pdf")}))["doc"].as_u64().unwrap();
        assert_eq!(render(&mut a, saved), before, "{name} save/reopen");
        let parts = call(&mut a, "doc_split", json!({"doc":doc,"every":1,"out_dir":format!("{name}-parts")}));
        let split = call(&mut a, "doc_open", json!({"path":parts["files"][0]["path"]}))["doc"].as_u64().unwrap();
        assert_eq!(render(&mut a, split), before, "{name} split");
        assert_eq!(std::fs::read(dir.path().join(format!("{name}.pdf"))).unwrap(), source);
        assert_eq!(call(&mut a, "doc_info", json!({"doc":doc}))["document"]["dirty"], false);
    }
}
