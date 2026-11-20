//! Flattened appearances are placed as page XObjects. A stream a `Do` operator names must be a
//! form XObject with `/Type /XObject` and `/Subtype /Form` (ISO 32000-2 Table 93); appearance
//! streams written without them are skipped by strict readers once the annotation is gone
//! (issue #7612). Flatten places a conformant copy and never edits the stored stream, which
//! another annotation may share.
use std::sync::Arc;

use pdfcraft_cos::{Document, ObjRef, Object, SaveOptions, write_incremental};

/// One page with four text-field widgets: `field_a` and `field_d` share an appearance stream
/// that lacks `/Type` and `/Subtype`, `field_b`'s lacks them too, and `field_c`'s has the full
/// form XObject header. Objects 5, 7 and 9 are the appearance streams.
fn fixture() -> Document {
    let ink = "q BT /Helv 10 Tf 0 g 2 3 Td (V) Tj ET Q";
    let deficient =
        |data: &str| format!("<< /BBox [0 0 200 16] /Resources << /Font << /Helv 4 0 R >> >> /Length {} >>\nstream\n{data}\nendstream", data.len());
    let conformant = |data: &str| {
        format!(
            "<< /Type /XObject /Subtype /Form /FormType 1 /Matrix [1 0 0 1 0 0] /BBox [0 0 200 16] /Resources << /Font << /Helv 4 0 R >> >> /Length {} >>\nstream\n{data}\nendstream",
            data.len()
        )
    };
    let widget = |name: &str, ap: u32, y: u32| {
        format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name}) /V ({name}) /Rect [10 {y} 210 {}] /AP << /N {ap} 0 R >> /F 4 >>", y + 16)
    };
    let objects: &[(&str, Option<String>)] = &[
        ("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R 8 0 R 10 0 R 11 0 R] >> >>", None),
        ("<< /Type /Pages /Kids [3 0 R] /Count 1 >>", None),
        (
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /Helv 4 0 R >> >> /Contents 12 0 R /Annots [6 0 R 8 0 R 10 0 R 11 0 R] >>",
            None,
        ),
        ("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>", None),
        ("", Some(deficient(ink))),
        ("", Some(widget("field_a", 5, 700))),
        ("", Some(deficient(ink))),
        ("", Some(widget("field_b", 7, 670))),
        ("", Some(conformant(ink))),
        ("", Some(widget("field_c", 9, 640))),
        ("", Some(widget("field_d", 5, 610))),
        ("", Some("<< /Length 0 >>\nstream\n\nendstream".to_string())),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, (dict, data)) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        match data {
            Some(stream) => out.extend_from_slice(stream.as_bytes()),
            None => out.extend_from_slice(dict.as_bytes()),
        }
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

/// The object numbers the page's `/XObject` dictionary places, in placement order.
fn placed(doc: &Document) -> Vec<(Vec<u8>, ObjRef)> {
    let page = pdfcraft_model::pages(doc).remove(0);
    let resources = doc.resolve(page.dict.get(b"Resources").unwrap());
    let xobjects = doc.resolve(resources.as_dict().unwrap().get(b"XObject").unwrap());
    let mut placed: Vec<(Vec<u8>, ObjRef)> = xobjects.as_dict().unwrap().iter().filter_map(|(k, v)| v.as_ref().map(|r| (k.clone(), r))).collect();
    placed.sort();
    placed
}

/// The dictionary of the stream `r` names.
fn stream_dict(doc: &Document, r: ObjRef) -> pdfcraft_cos::Dict {
    doc.get(r).as_dict().cloned().expect("placed XObject is a stream")
}

/// After flattening, every XObject the page draws is a form XObject a strict reader accepts:
/// `/Type /XObject`, `/Subtype /Form` (and `/FormType`, ISO 32000-2 Table 93). Stored streams
/// that lacked the keys are copied, not edited; a conformant one is reused as it is.
#[test]
fn appearances_without_xobject_keys_flatten_as_conformant_copies() {
    let mut doc = fixture();
    let originals: Vec<Object> = [5, 7, 9].map(|n| (*doc.get(ObjRef::new(n, 0))).clone()).to_vec();
    assert_eq!(pdfcraft_edit::flatten(&mut doc, &[0], false, true).unwrap(), 4);
    let doc = Document::open(Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())).unwrap();
    let placed = placed(&doc);
    assert_eq!(placed.len(), 4, "every flattened field drew its appearance");
    for (name, r) in &placed {
        let d = stream_dict(&doc, *r);
        assert_eq!(d.name(b"Type"), Some(b"XObject".as_slice()), "{name:?}: /Type");
        assert_eq!(d.name(b"Subtype"), Some(b"Form".as_slice()), "{name:?}: /Subtype");
        assert!(matches!(d.get(b"FormType"), Some(Object::Int(1))), "{name:?}: /FormType");
        assert!(d.get(b"BBox").is_some(), "{name:?}: /BBox kept");
    }
    let by_name = |n: &[u8]| placed.iter().find(|(k, _)| k == n).map(|(_, r)| *r).unwrap();
    assert_eq!(by_name(b"PCFl2"), ObjRef::new(9, 0), "a conformant stream is placed unchanged");
    assert_eq!(by_name(b"PCFl0"), by_name(b"PCFl3"), "two fields share one conformant copy");
    assert_ne!(by_name(b"PCFl0"), ObjRef::new(5, 0), "the deficient original is not placed");
    assert_ne!(by_name(b"PCFl1"), ObjRef::new(7, 0));
    if let (Object::Stream(copy), Object::Stream(source)) = (&*doc.get(by_name(b"PCFl0")), &*doc.get(ObjRef::new(5, 0))) {
        assert_eq!(copy.raw, source.raw, "the placed copy keeps the source stream's bytes");
    } else {
        panic!("placed copy and source are streams");
    }
    for (n, original) in [5usize, 7, 9].into_iter().zip(originals) {
        assert_eq!(*doc.get(ObjRef::new(n as u32, 0)), original, "stored appearance {n} is never edited");
    }
    let page = pdfcraft_model::pages(&doc).remove(0);
    if let Some(annots) = page.dict.get(b"Annots") {
        assert_eq!(doc.resolve(annots).as_array().map(|l| l.len()), Some(0));
    }
}
