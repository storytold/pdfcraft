//! Convert colours on some pages leaves the others as they were: a form or picture they share
//! is converted as a copy for the pages being converted, and the resource dictionary they
//! share is not changed.

use pdfcraft_cos::{Document, ObjRef, Object, Stream};
use pdfcraft_edit::{ColourTarget, ConvertColours, convert_colours};
use std::sync::Arc;

/// A document of `objs` (object k is `k + 1 0 obj`; object 1 is the catalog). A `{stream:...}`
/// entry is written as a stream holding the text after the colon, with its `/Length`.
fn pdf(objs: &[&str]) -> Document {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        let body = match o.split_once(">>stream:") {
            Some((dict, data)) => format!("{dict} /Length {} >>\nstream\n{data}\nendstream", data.len()),
            None => (*o).to_string(),
        };
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

fn flat(b: &[u8]) -> String {
    String::from_utf8_lossy(b).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn stream_text(doc: &Document, r: ObjRef) -> String {
    match &*doc.get(r) {
        Object::Stream(s) => flat(&s.decoded().unwrap()),
        _ => panic!("not a stream"),
    }
}

fn no_images(_: &Document, _: &Stream, _: ColourTarget) -> Option<Stream> {
    None
}

/// Two pages sharing one resource dictionary (inherited from the page tree) whose form draws
/// red, and a form inside it that draws blue.
fn two_pages() -> Document {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 600 800] /Resources 5 0 R >>",
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>",
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>",
        "<< /XObject << /Fm1 7 0 R >> >>",
        "<< >>stream:/Fm1 Do",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 100 100] /Resources << /XObject << /Fm2 8 0 R >> >> >>stream:1 0 0 rg 0 0 10 10 re f /Fm2 Do",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 100 100] >>stream:0 0 1 rg 0 0 5 5 re f",
    ])
}

fn form_of(doc: &Document, page: usize, name: &[u8]) -> ObjRef {
    let p = pdfcraft_model::pages(doc).swap_remove(page);
    let res = doc.resolve(p.dict.get(b"Resources").unwrap());
    let xo = doc.resolve(res.as_dict().unwrap().get(b"XObject").unwrap());
    xo.as_dict().unwrap().get(name).and_then(Object::as_ref).unwrap()
}

fn inner_form(doc: &Document, outer: ObjRef) -> ObjRef {
    match &*doc.get(outer) {
        Object::Stream(s) => {
            let res = doc.resolve(s.dict.get(b"Resources").unwrap());
            let xo = doc.resolve(res.as_dict().unwrap().get(b"XObject").unwrap());
            xo.as_dict().unwrap().get(b"Fm2").and_then(Object::as_ref).unwrap()
        }
        _ => panic!("not a stream"),
    }
}

#[test]
fn a_shared_form_is_converted_as_a_copy() {
    let mut doc = two_pages();
    let done = convert_colours(&mut doc, &[0], &ConvertColours::default(), &mut no_images).unwrap();
    assert_eq!(done.colours, 2);
    // The other page still draws the original forms, in their colours.
    let kept = form_of(&doc, 1, b"Fm1");
    assert!(stream_text(&doc, kept).contains("1 0 0 rg 0 0 10 10 re f"), "{}", stream_text(&doc, kept));
    assert!(stream_text(&doc, inner_form(&doc, kept)).contains("0 0 1 rg 0 0 5 5 re f"));
    // The converted page draws copies, the inner one too.
    let copy = form_of(&doc, 0, b"Fm1");
    assert_ne!(copy, kept);
    assert!(stream_text(&doc, copy).contains("0.299 g 0 0 10 10 re f"), "{}", stream_text(&doc, copy));
    assert!(stream_text(&doc, inner_form(&doc, copy)).contains("0.114 g 0 0 5 5 re f"));
}

#[test]
fn converting_every_page_changes_the_forms_in_place() {
    let mut doc = two_pages();
    let before = form_of(&doc, 0, b"Fm1");
    convert_colours(&mut doc, &[0, 1], &ConvertColours::default(), &mut no_images).unwrap();
    assert_eq!(form_of(&doc, 0, b"Fm1"), before);
    assert_eq!(form_of(&doc, 1, b"Fm1"), before);
    assert!(stream_text(&doc, before).contains("0.299 g 0 0 10 10 re f"));
}

/// One page of `content` with `res` as its resources, followed by `more` objects from 5 up.
fn page_of(res: &str, content: &str, more: &[&str]) -> Document {
    let page = format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources {res} >>");
    let content = format!("<< >>stream:{content}");
    let mut objs = vec!["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 >>", page.as_str(), content.as_str()];
    objs.extend_from_slice(more);
    pdf(&objs)
}

/// The `/None` colorant paints nothing; a colour of its own would make it appear.
#[test]
fn the_none_colorant_stays() {
    let tint = "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 0 0 1] /N 1 >>";
    let res = format!(
        "<< /ColorSpace << /S [/Separation /None /DeviceCMYK {tint}] /D [/DeviceN [/None /None] /DeviceCMYK {tint}] /Real [/Separation /Red /DeviceCMYK {tint}] >> >>"
    );
    let content = "/S cs 1 scn 0 0 9 9 re f /D cs 1 1 scn 0 0 9 9 re f";
    let mut doc = page_of(&res, content, &[]);
    assert_eq!(convert_colours(&mut doc, &[0], &ConvertColours::default(), &mut no_images).unwrap().colours, 0);
    assert_eq!(stream_text(&doc, ObjRef { num: 4, generation: 0 }), content);
    let mut doc = page_of(&res, "/Real cs 1 scn 0 0 9 9 re f", &[]);
    assert_eq!(convert_colours(&mut doc, &[0], &ConvertColours::default(), &mut no_images).unwrap().colours, 2, "a real spot is converted");
}

/// Pictures the helper cannot re-encode are counted, unless they are in the target already or
/// have no colours of their own.
#[test]
fn pictures_left_alone_are_counted() {
    const IMAGE: &str = "/Type /XObject /Subtype /Image /Width 1 /Height 1";
    let indexed = format!("<< {IMAGE} /BitsPerComponent 8 /ColorSpace [/Indexed /DeviceRGB 0 <FF0000>] >>stream:x");
    let gray_table = format!("<< {IMAGE} /BitsPerComponent 8 /ColorSpace [/Indexed /DeviceGray 0 <80>] >>stream:x");
    let gray = format!("<< {IMAGE} /BitsPerComponent 8 /ColorSpace /DeviceGray >>stream:x");
    let stencil = format!("<< {IMAGE} /BitsPerComponent 1 /ImageMask true >>stream:x");
    let res = "<< /XObject << /A 5 0 R /B 6 0 R /C 7 0 R /D 8 0 R >> >>";
    let mut doc = page_of(res, "/A Do /B Do /C Do /D Do", &[&indexed, &gray_table, &gray, &stencil]);
    let mut none = doc.clone();
    assert_eq!(convert_colours(&mut doc, &[0], &ConvertColours::default(), &mut no_images).unwrap().images_left, 1);
    let s = ConvertColours { images: false, ..ConvertColours::default() };
    assert_eq!(convert_colours(&mut none, &[0], &s, &mut no_images).unwrap().images_left, 0, "pictures were not asked for");
}
