//! Original overlapping rectangles render, export and survive a full save through public tools.

use pdfcraft_automation::{Automation, Content};
use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_full};
use serde_json::{Value, json};

fn fixture(forward: usize, reverse: usize, even_odd: bool) -> Vec<u8> {
    let mut content = "-10 -10 200 200 re\n".repeat(forward);
    content.push_str(&"-10 190 200 -200 re\n".repeat(reverse));
    content.push_str(if even_odd { "f*" } else { "f" });
    let mut doc = Document::new_empty();
    let tree = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let contents = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content.into_bytes())));
    let page = doc.add(Dict::from_iter([
        (b"Type".to_vec(), Object::name("Page")),
        (b"Parent".to_vec(), Object::Ref(tree)),
        (b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 180.into(), 180.into()])),
        (b"Contents".to_vec(), Object::Ref(contents)),
    ]));
    doc.update_dict(tree, |tree| {
        tree.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        tree.set(b"Count".to_vec(), 1);
    })
    .unwrap();
    write_full(&doc, &SaveOptions::default()).unwrap()
}

fn call(a: &mut Automation, tool: &str, args: Value) -> Value {
    a.call(tool, &args).unwrap().into_iter().find_map(|content| if let Content::Json(value) = content { Some(value) } else { None }).unwrap()
}

fn assert_pixels(bytes: &[u8], filled: bool) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    assert_eq!((info.width, info.height), (240, 240));
    let channels = info.color_type.samples();
    assert!(channels >= 3);
    let expected = if filled { 0 } else { 255 };
    assert!(pixels[..info.buffer_size()].chunks_exact(channels).all(|pixel| pixel[..3] == [expected; 3]));
}

fn render(a: &mut Automation, doc: u64) -> Vec<u8> {
    let content = a.call("page_render", &json!({"doc":doc,"page":1,"dpi":96})).unwrap();
    let Content::Png { data, .. } = &content[0] else { panic!("expected PNG") };
    data.clone()
}

#[test]
fn render_export_and_reopen_preserve_large_signed_path_windings() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    for (index, (forward, reverse, even_odd, filled)) in
        [(129, 0, false, true), (0, 129, false, true), (128, 0, true, false), (32_769, 32_769, false, false)].into_iter().enumerate()
    {
        let name = format!("winding-{index}");
        let source = fixture(forward, reverse, even_odd);
        std::fs::write(dir.path().join(format!("{name}.pdf")), &source).unwrap();
        let doc = call(&mut a, "doc_open", json!({"path":format!("{name}.pdf")}))["doc"].as_u64().unwrap();
        assert_pixels(&render(&mut a, doc), filled);
        let exported = call(&mut a, "doc_export_images", json!({"doc":doc,"folder":format!("{name}-images"),"pages":[1],"dpi":96}));
        assert_eq!(exported["count"], 1);
        let image = std::fs::read(dir.path().join(format!("{name}-images/{name}_page_1.png"))).unwrap();
        assert_pixels(&image, filled);
        call(&mut a, "doc_save", json!({"doc":doc,"path":format!("{name}-saved.pdf"),"full":true}));
        call(&mut a, "doc_close", json!({"doc":doc}));
        let reopened = call(&mut a, "doc_open", json!({"path":format!("{name}-saved.pdf")}))["doc"].as_u64().unwrap();
        assert_pixels(&render(&mut a, reopened), filled);
        assert_eq!(call(&mut a, "doc_info", json!({"doc":reopened}))["document"]["dirty"], false);
        assert_eq!(std::fs::read(dir.path().join(format!("{name}.pdf"))).unwrap(), source);
    }
}
