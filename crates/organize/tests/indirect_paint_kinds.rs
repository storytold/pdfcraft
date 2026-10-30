//! Paint kind entries can name their primitive values through indirect objects.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};
use pdfcraft_organize::{SplitBy, extract_pages, pages, split};

#[derive(Clone, Copy, Debug)]
enum Kind {
    Type3,
    Form,
    Pattern,
    Mask,
}

fn primitive(doc: &mut Document, value: Object, indirect: bool) -> Object {
    if indirect { Object::Ref(doc.add(value)) } else { value }
}

fn fixture(kind: Kind, indirect: bool) -> Document {
    let mut doc = Document::new_empty();
    let tree = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
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
    let mut resources = Dict::from_iter([(b"XObject".to_vec(), Object::Ref(images))]);
    let mut content = "q 20 0 0 20 10 10 cm /Small Do Q ".to_string();
    match kind {
        Kind::Type3 => {
            let program = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"1000 0 d0 q 1000 0 0 1000 0 0 cm /Paint Do Q".to_vec())));
            let subtype = primitive(&mut doc, Object::name("Type3"), indirect);
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
            resources.set(b"Font".to_vec(), Object::Dict(Dict::from_iter([(b"T3".to_vec(), Object::Ref(font))])));
            content.push_str("BT /T3 50 Tf 50 80 Td (A) Tj ET");
        }
        Kind::Form => {
            let subtype = primitive(&mut doc, Object::name("Form"), indirect);
            let form = doc.add(Object::Stream(Stream::from_raw(
                Dict::from_iter([
                    (b"Subtype".to_vec(), subtype),
                    (b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 1.into(), 1.into()])),
                    (b"Resources".to_vec(), Object::Dict(Dict::from_iter([(b"XObject".to_vec(), Object::Ref(images))]))),
                ]),
                b"/Paint Do".to_vec(),
            )));
            doc.update_dict(images, |images| images.set(b"Fm".to_vec(), Object::Ref(form))).unwrap();
            content.push_str("q 50 0 0 50 50 80 cm /Fm Do Q");
        }
        Kind::Pattern => {
            // Malformed resource-less paint is conservatively treated as possibly inherited,
            // whether its kind primitive is direct or indirect.
            let pattern_type = primitive(&mut doc, 1.into(), indirect);
            let pattern =
                doc.add(Object::Stream(Stream::from_raw(Dict::from_iter([(b"PatternType".to_vec(), pattern_type)]), b"/Paint Do".to_vec())));
            resources.set(b"Pattern".to_vec(), Object::Dict(Dict::from_iter([(b"Tile".to_vec(), Object::Ref(pattern))])));
        }
        Kind::Mask => {
            let group = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"/Paint Do".to_vec())));
            let mode = primitive(&mut doc, Object::name("Luminosity"), indirect);
            let mask = Object::Dict(Dict::from_iter([(b"S".to_vec(), mode), (b"G".to_vec(), Object::Ref(group))]));
            resources.set(
                b"ExtGState".to_vec(),
                Object::Dict(Dict::from_iter([(b"GS".to_vec(), Object::Dict(Dict::from_iter([(b"SMask".to_vec(), mask)])))])),
            );
        }
    }
    let resources = doc.add(resources);
    let contents = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content.into_bytes())));
    let page = doc.add(Dict::from_iter([
        (b"Type".to_vec(), Object::name("Page")),
        (b"Parent".to_vec(), Object::Ref(tree)),
        (b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 160.into(), 160.into()])),
        (b"Resources".to_vec(), Object::Ref(resources)),
        (b"Contents".to_vec(), Object::Ref(contents)),
    ]));
    doc.update_dict(tree, |tree| {
        tree.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        tree.set(b"Count".to_vec(), 1);
    })
    .unwrap();
    doc
}

fn image_names(doc: &Document) -> Vec<Vec<u8>> {
    let page = doc.get(pages(doc).unwrap()[0].obj);
    let resources = doc.resolve(page.as_dict().unwrap().get(b"Resources").unwrap());
    let images = doc.resolve(resources.as_dict().unwrap().get(b"XObject").unwrap());
    images.as_dict().unwrap().iter().map(|(name, _)| name.clone()).collect()
}

fn check(kind: Kind) {
    for indirect in [false, true] {
        let source = fixture(kind, indirect);
        let before = write_full(&source, &SaveOptions::default()).unwrap();
        for output in [extract_pages(&source, &[0]).unwrap(), split(&source, &SplitBy::PageCount(1)).unwrap().remove(0)] {
            let saved = Document::open(Arc::new(write_full(&output, &SaveOptions::default()).unwrap())).unwrap();
            assert!(image_names(&saved).iter().any(|name| name == b"Paint"), "{kind:?}, indirect={indirect}");
        }
        assert_eq!(write_full(&source, &SaveOptions::default()).unwrap(), before);
    }
}

#[test]
fn extract_keeps_images_used_by_indirect_type3_subtypes() {
    check(Kind::Type3);
}

#[test]
fn extract_keeps_images_used_by_indirect_form_subtypes() {
    check(Kind::Form);
}

#[test]
fn extract_protects_inherited_paint_with_indirect_kind_metadata() {
    check(Kind::Pattern);
    check(Kind::Mask);
}

#[test]
fn indirect_kind_extraction_matches_poppler_pixels() {
    let pdftoppm = pdfcraft_oracle::or_skip!(pdfcraft_oracle::pdftoppm());
    let dir = std::env::temp_dir().join(format!("pdfcraft-indirect-kinds-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (index, kind) in [Kind::Type3, Kind::Form].into_iter().enumerate() {
        let source = fixture(kind, true);
        let render = |doc: &Document, stage: &str| {
            let path = dir.join(format!("{index}-{stage}.pdf"));
            std::fs::write(&path, write_full(doc, &SaveOptions::default()).unwrap()).unwrap();
            let result = pdftoppm.command().args(["-singlefile", "-r", "72", "-gray"]).arg(path).output().unwrap();
            assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
            assert!(result.stderr.is_empty(), "{}", String::from_utf8_lossy(&result.stderr));
            result.stdout
        };
        let before = render(&source, "source");
        assert!(before.iter().filter(|byte| **byte < 200).count() > 2500, "the source has substantial artwork");
        assert_eq!(render(&extract_pages(&source, &[0]).unwrap(), "extracted"), before);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn malformed_indirect_subtypes_terminate_without_panicking() {
    for value in [Some(Object::Null), Some(Object::Int(1)), Some(Object::Ref(ObjRef::new(999_999, 0))), None] {
        let mut doc = fixture(Kind::Form, true);
        let page = doc.get(pages(&doc).unwrap()[0].obj);
        let resources = doc.resolve(page.as_dict().unwrap().get(b"Resources").unwrap());
        let images = doc.resolve(resources.as_dict().unwrap().get(b"XObject").unwrap());
        let form = images.as_dict().unwrap().reference(b"Fm").unwrap();
        let subtype = doc.get(form).as_dict().unwrap().reference(b"Subtype").unwrap();
        // The final case is a self-referencing metadata object, not a name.
        doc.set(subtype, value.unwrap_or(Object::Ref(subtype)));
        let output = extract_pages(&doc, &[0]).unwrap();
        assert!(image_names(&output).iter().any(|name| name == b"Small"));
    }
}
