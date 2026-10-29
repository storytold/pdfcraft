//! Extract/Split must filter the XObject map inside a Form's direct Resources dictionary.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, SaveOptions, Stream, write_full};
use pdfcraft_organize::{SplitBy, extract_pages, pages, split};

#[derive(Clone, Copy)]
enum Storage {
    Inline,
    IndirectResources,
    IndirectXObjects,
}

fn fixture(storage: Storage) -> Document {
    let mut doc = Document::new_empty();
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let mut images = Dict::new();
    for (name, size, color) in [("Used", 1, [220, 40, 30]), ("Unused", 128, [17, 69, 251])] {
        let attributes = Dict::from_iter([
            (b"Type".to_vec(), Object::name("XObject")),
            (b"Subtype".to_vec(), Object::name("Image")),
            (b"Width".to_vec(), Object::Int(size)),
            (b"Height".to_vec(), Object::Int(size)),
            (b"BitsPerComponent".to_vec(), 8.into()),
            (b"ColorSpace".to_vec(), Object::name("DeviceRGB")),
        ]);
        let image = doc.add(Object::Stream(Stream::from_raw(attributes, color.repeat((size * size) as usize))));
        images.set(name.as_bytes().to_vec(), Object::Ref(image));
    }
    let images = if matches!(storage, Storage::IndirectXObjects) { Object::Ref(doc.add(images)) } else { Object::Dict(images) };
    let resources = Dict::from_iter([(b"XObject".to_vec(), images), (b"PrivateMarker".to_vec(), Object::String(PdfString::text("keep me")))]);
    let resources = if matches!(storage, Storage::IndirectResources) { Object::Ref(doc.add(resources)) } else { Object::Dict(resources) };
    let attributes = Dict::from_iter([
        (b"Type".to_vec(), Object::name("XObject")),
        (b"Subtype".to_vec(), Object::name("Form")),
        (b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 100.into(), 100.into()])),
        (b"Resources".to_vec(), resources),
    ]);
    let form = doc.add(Object::Stream(Stream::from_raw(attributes, b"q 30 0 0 30 10 10 cm /Used Do Q".to_vec())));
    let contents = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"/Fm Do".to_vec())));
    let page = doc.add(Dict::from_iter([
        (b"Type".to_vec(), Object::name("Page")),
        (b"Parent".to_vec(), Object::Ref(pages)),
        (b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 100.into(), 100.into()])),
        (b"Contents".to_vec(), Object::Ref(contents)),
        (
            b"Resources".to_vec(),
            Object::Dict(Dict::from_iter([(b"XObject".to_vec(), Object::Dict(Dict::from_iter([(b"Fm".to_vec(), Object::Ref(form))])))])),
        ),
    ]));
    doc.update_dict(pages, |pages| {
        pages.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        pages.set(b"Count".to_vec(), 1);
    })
    .unwrap();
    doc
}

fn form(doc: &Document) -> ObjRef {
    let page = doc.get(pages(doc).unwrap()[0].obj);
    let resources = doc.resolve(page.as_dict().unwrap().get(b"Resources").unwrap());
    let xobjects = doc.resolve(resources.as_dict().unwrap().get(b"XObject").unwrap());
    xobjects.as_dict().unwrap().reference(b"Fm").unwrap()
}

fn resources(doc: &Document) -> Dict {
    let form = doc.get(form(doc));
    doc.resolve(form.as_dict().unwrap().get(b"Resources").unwrap()).as_dict().unwrap().clone()
}

fn names(doc: &Document) -> Vec<Vec<u8>> {
    let resources = resources(doc);
    let images = doc.resolve(resources.get(b"XObject").unwrap());
    let mut names: Vec<_> = images.as_dict().unwrap().iter().map(|(name, _)| name.clone()).collect();
    names.sort_unstable();
    names
}

fn roundtrip(doc: &Document) -> Document {
    Document::open(Arc::new(write_full(doc, &SaveOptions::default()).unwrap())).unwrap()
}

#[test]
fn extract_prunes_unused_images_inside_inline_form_resources() {
    let source = fixture(Storage::Inline);
    let before = write_full(&source, &SaveOptions::default()).unwrap();
    for output in [extract_pages(&source, &[0]).unwrap(), split(&source, &SplitBy::PageCount(1)).unwrap().remove(0)] {
        let bytes = write_full(&output, &SaveOptions::default()).unwrap();
        let output = roundtrip(&output);
        assert_eq!(names(&output), [b"Used".to_vec()]);
        assert!(bytes.len() + 40_000 < before.len(), "the unused image bytes must leave the saved output");
        assert_eq!(resources(&output).get(b"PrivateMarker"), resources(&source).get(b"PrivateMarker"), "unrelated resource data stays");
    }
    assert_eq!(write_full(&source, &SaveOptions::default()).unwrap(), before);
    assert!(names(&source).iter().any(|name| name == b"Unused"));
}

#[test]
fn indirect_form_resource_storage_still_prunes_unused_images() {
    for storage in [Storage::IndirectResources, Storage::IndirectXObjects] {
        let output = roundtrip(&extract_pages(&fixture(storage), &[0]).unwrap());
        assert_eq!(names(&output), [b"Used".to_vec()]);
    }
}

#[test]
fn nested_resource_less_forms_share_the_correct_inline_owner() {
    let mut source = fixture(Storage::Inline);
    let outer = form(&source);
    let inner = source.add(Object::Stream(Stream::from_raw(
        Dict::from_iter([
            (b"Subtype".to_vec(), Object::name("Form")),
            (b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 100.into(), 100.into()])),
        ]),
        b"q 30 0 0 30 10 10 cm /Used Do Q".to_vec(),
    )));
    let mut resources = resources(&source);
    let mut images = resources.get(b"XObject").unwrap().as_dict().unwrap().clone();
    images.set(b"Nested".to_vec(), Object::Ref(inner));
    resources.set(b"XObject".to_vec(), Object::Dict(images));
    let mut attributes = source.get(outer).as_dict().unwrap().clone();
    attributes.set(b"Resources".to_vec(), Object::Dict(resources));
    source.set(outer, Object::Stream(Stream::from_raw(attributes, b"/Nested Do".to_vec())));
    let output = roundtrip(&extract_pages(&source, &[0]).unwrap());
    assert_eq!(names(&output), [b"Nested".to_vec(), b"Used".to_vec()]);
}

#[test]
fn unparsed_paint_still_protects_inline_form_resources() {
    let mut source = fixture(Storage::Inline);
    let outer = form(&source);
    let group = source.add(Object::Stream(Stream::from_raw(
        Dict::from_iter([
            (b"Subtype".to_vec(), Object::name("Form")),
            (b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 1.into(), 1.into()])),
        ]),
        b"/Unused Do".to_vec(),
    )));
    let mut resources = resources(&source);
    let mask = Dict::from_iter([(b"S".to_vec(), Object::name("Luminosity")), (b"G".to_vec(), Object::Ref(group))]);
    let state = Dict::from_iter([(b"SMask".to_vec(), Object::Dict(mask))]);
    resources.set(b"ExtGState".to_vec(), Object::Dict(Dict::from_iter([(b"GS".to_vec(), Object::Dict(state))])));
    source.update_dict(outer, |attributes| attributes.set(b"Resources".to_vec(), Object::Dict(resources))).unwrap();
    let output = roundtrip(&extract_pages(&source, &[0]).unwrap());
    assert_eq!(names(&output), names(&source), "the resource-less soft-mask group can draw the second image");
}

#[test]
fn inline_form_pruning_preserves_poppler_pixels() {
    let pdftoppm = pdfcraft_oracle::or_skip!(pdfcraft_oracle::pdftoppm());
    let source = fixture(Storage::Inline);
    let output = extract_pages(&source, &[0]).unwrap();
    let dir = std::env::temp_dir().join(format!("pdfcraft-inline-form-resources-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let render = |doc: &Document, name: &str| {
        let path = dir.join(format!("{name}.pdf"));
        std::fs::write(&path, write_full(doc, &SaveOptions::default()).unwrap()).unwrap();
        let result = pdftoppm.command().args(["-singlefile", "-r", "72"]).arg(path).output().unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        assert!(result.stderr.is_empty(), "{}", String::from_utf8_lossy(&result.stderr));
        result.stdout
    };
    let before = render(&source, "source");
    assert_eq!(render(&output, "extracted"), before);
    assert!(before.windows(3).filter(|pixel| *pixel == [220, 40, 30]).count() >= 900, "the fixture contains visible artwork");
    std::fs::remove_dir_all(dir).unwrap();
}
