//! Comment formatting and appearances through XFDF (ISO 19444-1, XFDF 3.0) and FDF
//! (ISO 32000-2 §12.7.8): each property both ways, appearance streams with their resources, and
//! files that are malformed or too large.

use std::sync::Arc;

use base64::Engine as _;
use pdfcraft_cos::{SaveOptions, Stream, write_full};
use proptest::prelude::*;

use super::*;

/// A document with `n` empty pages.
fn pages(n: usize) -> Document {
    let mut doc = Document::new_empty();
    let root = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let mut kids = Vec::new();
    for _ in 0..n {
        let mut p = Dict::new();
        p.set(b"Type".to_vec(), Object::name("Page"));
        p.set(b"Parent".to_vec(), Object::Ref(root));
        p.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 600.into(), 800.into()]));
        kids.push(Object::Ref(doc.add(p)));
    }
    doc.update_dict(root, |d| {
        d.set(b"Count".to_vec(), Object::Int(kids.len() as i64));
        d.set(b"Kids".to_vec(), Object::Array(kids));
    })
    .unwrap();
    doc
}

fn page_ref(doc: &Document, i: usize) -> ObjRef {
    pdfcraft_model::pages(doc)[i].obj
}

/// Add an annotation dictionary to page 0 as an object of its own.
fn put(doc: &mut Document, mut d: Dict) -> ObjRef {
    let page = page_ref(doc, 0);
    d.set(b"Type".to_vec(), Object::name("Annot"));
    d.set(b"P".to_vec(), Object::Ref(page));
    let r = doc.add(Object::Dict(d));
    doc.update_dict(page, |p| match p.get_mut(b"Annots") {
        Some(Object::Array(a)) => a.push(Object::Ref(r)),
        _ => p.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(r)])),
    })
    .unwrap();
    r
}

fn nums(v: &[f64]) -> Object {
    Object::Array(v.iter().map(|x| Object::Real(*x)).collect())
}

fn annot(sub: &str, nm: &str, rect: [f64; 4]) -> Dict {
    let mut d = Dict::new();
    d.set(b"Subtype".to_vec(), Object::name(sub));
    d.set(b"NM".to_vec(), PdfString::text(nm));
    d.set(b"Rect".to_vec(), nums(&rect));
    d.set(b"F".to_vec(), Object::Int(4));
    d
}

/// The imported annotation named `nm`.
fn named(doc: &Document, nm: &str) -> Dict {
    let r = xfdf::find_by_name(doc, nm).unwrap_or_else(|| panic!("no comment {nm}"));
    doc.get(r).as_dict().unwrap().clone()
}

fn f64s(doc: &Document, o: Option<&Object>) -> Vec<f64> {
    o.map(|o| doc.resolve(o)).and_then(|o| o.as_array().map(|a| a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect())).unwrap_or_default()
}

fn text(doc: &Document, d: &Dict, k: &[u8]) -> Option<String> {
    d.get(k).and_then(|o| doc.resolve(o).as_string().map(|s| s.to_text()))
}

fn sub_dict(doc: &Document, d: &Dict, k: &[u8]) -> Dict {
    d.get(k).map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default()
}

const RICH: &str = r#"<?xml version="1.0"?><body xmlns="http://www.w3.org/1999/xhtml" xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/" xfa:APIVersion="Acrobat:11.0.0" xfa:spec="2.0.2"><p dir="ltr"><span style="font-weight:bold">Bold</span> &amp; <i>plain</i></p><p>two</p></body>"#;

/// A free text comment with every formatting entry XFDF 3.0 gives an element or attribute for.
fn formatted_freetext() -> Dict {
    let mut d = annot("FreeText", "ft", [100.0, 100.0, 300.0, 200.0]);
    d.set(b"Contents".to_vec(), PdfString::text("Bold & plain\rtwo"));
    d.set(b"RC".to_vec(), PdfString::text(RICH));
    d.set(b"DA".to_vec(), PdfString::literal(b"0 0 1 rg /Helv 14 Tf".to_vec()));
    d.set(b"DS".to_vec(), PdfString::text("font: Helvetica,sans-serif 14.0pt; text-align:right; color:#0000FF"));
    d.set(b"Q".to_vec(), Object::Int(2));
    d.set(b"Rotate".to_vec(), Object::Int(90));
    d.set(b"RD".to_vec(), nums(&[1.5, 2.0, 3.0, 4.0]));
    let mut bs = Dict::new();
    bs.set(b"W".to_vec(), Object::Real(2.0));
    bs.set(b"S".to_vec(), Object::name("D"));
    bs.set(b"D".to_vec(), nums(&[3.0, 2.0]));
    d.set(b"BS".to_vec(), Object::Dict(bs));
    d
}

fn formatted_line() -> Dict {
    let mut d = annot("Line", "ln", [50.0, 50.0, 250.0, 120.0]);
    d.set(b"L".to_vec(), nums(&[60.0, 60.0, 240.0, 60.0]));
    d.set(b"LL".to_vec(), Object::Real(-12.0));
    d.set(b"LLE".to_vec(), Object::Real(3.0));
    d.set(b"LLO".to_vec(), Object::Real(1.5));
    d.set(b"Cap".to_vec(), Object::Bool(true));
    d.set(b"CP".to_vec(), Object::name("Top"));
    d.set(b"CO".to_vec(), nums(&[4.0, -2.0]));
    d.set(b"IT".to_vec(), Object::name("LineDimension"));
    let mut bs = Dict::new();
    bs.set(b"W".to_vec(), Object::Real(1.0));
    bs.set(b"S".to_vec(), Object::name("D"));
    bs.set(b"D".to_vec(), nums(&[4.0, 3.0, 2.0, 3.0]));
    d.set(b"BS".to_vec(), Object::Dict(bs));
    d
}

fn round_trip_xfdf(src: &Document) -> (String, Document) {
    let xml = export_xfdf(src, true, false, "src.pdf");
    let mut dst = pages(2);
    import(&mut dst, xml.as_bytes()).unwrap();
    (xml, dst)
}

#[test]
fn free_text_formatting_is_exported_as_xfdf_elements_and_attributes() {
    let mut src = pages(1);
    put(&mut src, formatted_freetext());
    let xml = export_xfdf(&src, true, false, "src.pdf");
    for want in [
        r#"justification="right""#,
        r#"rotation="90""#,
        r#"width="2""#,
        r#"style="dash""#,
        r#"dashes="3,2""#,
        r#"fringe="1.5,2,3,4""#,
        "<defaultstyle>font: Helvetica,sans-serif 14.0pt; text-align:right; color:#0000FF</defaultstyle>",
        // The rich text's element, as markup and without its XML declaration.
        r#"<contents-richtext><body xmlns="http://www.w3.org/1999/xhtml""#,
        r#"<span style="font-weight:bold">Bold</span> &amp; <i>plain</i></p><p>two</p></body></contents-richtext>"#,
    ] {
        assert!(xml.contains(want), "missing {want} in\n{xml}");
    }
    // What it writes is XML the reader accepts.
    assert!(roxmltree::Document::parse(&xml).is_ok());
}

#[test]
fn free_text_formatting_comes_back_from_xfdf() {
    let mut src = pages(1);
    put(&mut src, formatted_freetext());
    let (_, dst) = round_trip_xfdf(&src);
    let d = named(&dst, "ft");
    assert_eq!(text(&dst, &d, b"RC").as_deref(), Some(RICH));
    assert_eq!(text(&dst, &d, b"DS").as_deref(), Some("font: Helvetica,sans-serif 14.0pt; text-align:right; color:#0000FF"));
    assert_eq!(text(&dst, &d, b"Contents").as_deref(), Some("Bold & plain\rtwo"));
    assert_eq!(d.int(b"Q"), Some(2));
    assert_eq!(d.int(b"Rotate"), Some(90));
    assert_eq!(f64s(&dst, d.get(b"RD")), vec![1.5, 2.0, 3.0, 4.0]);
    let bs = sub_dict(&dst, &d, b"BS");
    assert_eq!(bs.name(b"S"), Some(b"D".as_slice()), "a dashed border comes back dashed, not solid");
    assert_eq!(f64s(&dst, bs.get(b"D")), vec![3.0, 2.0]);
    assert_eq!(bs.get(b"W").and_then(Object::as_f64), Some(2.0));
}

#[test]
fn line_leaders_and_caption_round_trip() {
    let mut src = pages(1);
    put(&mut src, formatted_line());
    let (xml, dst) = round_trip_xfdf(&src);
    for want in [
        r#"leaderLength="-12""#,
        r#"leaderExtend="3""#,
        r#"leader-offset="1.5""#,
        r#"caption="yes""#,
        r#"caption-style="Top""#,
        r#"caption-offset-h="4" caption-offset-v="-2""#,
        r#"dashes="4,3,2,3""#,
    ] {
        assert!(xml.contains(want), "missing {want} in\n{xml}");
    }
    let d = named(&dst, "ln");
    assert_eq!(d.get(b"LL").and_then(Object::as_f64), Some(-12.0));
    assert_eq!(d.get(b"LLE").and_then(Object::as_f64), Some(3.0));
    assert_eq!(d.get(b"LLO").and_then(Object::as_f64), Some(1.5));
    assert_eq!(d.get(b"Cap"), Some(&Object::Bool(true)));
    assert_eq!(d.name(b"CP"), Some(b"Top".as_slice()));
    assert_eq!(f64s(&dst, d.get(b"CO")), vec![4.0, -2.0]);
    assert_eq!(f64s(&dst, sub_dict(&dst, &d, b"BS").get(b"D")), vec![4.0, 3.0, 2.0, 3.0]);
}

#[test]
fn square_circle_caret_and_redaction_attributes_round_trip() {
    let mut src = pages(1);
    let mut sq = annot("Square", "sq", [10.0, 10.0, 110.0, 110.0]);
    sq.set(b"RD".to_vec(), nums(&[5.0, 5.0, 5.0, 5.0]));
    let mut be = Dict::new();
    be.set(b"S".to_vec(), Object::name("C"));
    be.set(b"I".to_vec(), Object::Real(2.0));
    sq.set(b"BE".to_vec(), Object::Dict(be));
    put(&mut src, sq);
    let mut ci = annot("Circle", "ci", [120.0, 10.0, 220.0, 110.0]);
    let mut bs = Dict::new();
    bs.set(b"W".to_vec(), Object::Real(3.0));
    bs.set(b"S".to_vec(), Object::name("B"));
    ci.set(b"BS".to_vec(), Object::Dict(bs));
    put(&mut src, ci);
    let mut ca = annot("Caret", "ca", [230.0, 10.0, 250.0, 30.0]);
    ca.set(b"Sy".to_vec(), Object::name("P"));
    ca.set(b"RD".to_vec(), nums(&[1.0, 1.0, 1.0, 1.0]));
    put(&mut src, ca);
    let mut rd = annot("Redact", "rd", [300.0, 10.0, 400.0, 30.0]);
    rd.set(b"OverlayText".to_vec(), PdfString::text("WITHHELD"));
    rd.set(b"Repeat".to_vec(), Object::Bool(true));
    rd.set(b"Q".to_vec(), Object::Int(1));
    put(&mut src, rd);
    // A solid border that still holds a dash array keeps it, and stays solid.
    let mut solid = annot("Square", "solid", [120.0, 200.0, 220.0, 300.0]);
    let mut bs = Dict::new();
    bs.set(b"W".to_vec(), Object::Real(1.0));
    bs.set(b"S".to_vec(), Object::name("S"));
    bs.set(b"D".to_vec(), nums(&[2.0]));
    solid.set(b"BS".to_vec(), Object::Dict(bs));
    put(&mut src, solid);
    // An older /Border array: width 2, dashes 4 on 1.
    let mut old = annot("Square", "old", [10.0, 200.0, 110.0, 300.0]);
    old.set(b"Border".to_vec(), Object::Array(vec![0.into(), 0.into(), 2.into(), nums(&[4.0, 1.0])]));
    put(&mut src, old);
    let (xml, dst) = round_trip_xfdf(&src);
    for want in [
        r#"style="cloudy" intensity="2""#,
        r#"fringe="5,5,5,5""#,
        r#"style="bevelled""#,
        r#"symbol="paragraph""#,
        r#"overlay-text="WITHHELD""#,
        r#"overlay-text-repeat="true""#,
        r#"justification="1""#,
    ] {
        assert!(xml.contains(want), "missing {want} in\n{xml}");
    }
    let sq = named(&dst, "sq");
    assert_eq!(f64s(&dst, sq.get(b"RD")), vec![5.0; 4]);
    assert_eq!(sub_dict(&dst, &sq, b"BE").get(b"I").and_then(Object::as_f64), Some(2.0));
    assert_eq!(sub_dict(&dst, &named(&dst, "ci"), b"BS").name(b"S"), Some(b"B".as_slice()));
    let ca = named(&dst, "ca");
    assert_eq!(ca.name(b"Sy"), Some(b"P".as_slice()));
    assert_eq!(f64s(&dst, ca.get(b"RD")), vec![1.0; 4]);
    let rd = named(&dst, "rd");
    assert_eq!(text(&dst, &rd, b"OverlayText").as_deref(), Some("WITHHELD"));
    assert_eq!(rd.get(b"Repeat"), Some(&Object::Bool(true)));
    assert_eq!(rd.int(b"Q"), Some(1));
    let solid = sub_dict(&dst, &named(&dst, "solid"), b"BS");
    assert_eq!((solid.name(b"S"), f64s(&dst, solid.get(b"D"))), (Some(b"S".as_slice()), vec![2.0]));
    let old = sub_dict(&dst, &named(&dst, "old"), b"BS");
    assert_eq!((old.get(b"W").and_then(Object::as_f64), old.name(b"S")), (Some(2.0), Some(b"D".as_slice())));
    assert_eq!(f64s(&dst, old.get(b"D")), vec![4.0, 1.0]);
}

/// The XFDF the audit imported: what another tool writes for formatted comments.
#[test]
fn formatting_written_by_another_tool_is_imported() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<xfdf xmlns="http://ns.adobe.com/xfdf/" xml:space="preserve"><annots>
<freetext page="0" rect="100,100,300,200" name="a" justification="2" rotation="90" width="2" style="dash" dashes="3,2" fringe="2,2,2,2">
<contents-richtext><body xmlns="http://www.w3.org/1999/xhtml"><p>Hello <b>there</b></p></body></contents-richtext>
<defaultstyle>font: 12pt Helvetica</defaultstyle><defaultappearance>0 g /Helv 12 Tf</defaultappearance></freetext>
<line page="0" rect="50,50,250,120" name="b" start="60,60" end="240,60" leaderLength="10" leaderExtend="2" leader-offset="1"/>
</annots></xfdf>"#;
    let mut dst = pages(1);
    import(&mut dst, xml.as_bytes()).unwrap();
    let a = named(&dst, "a");
    assert_eq!(a.int(b"Q"), Some(2));
    assert_eq!(a.int(b"Rotate"), Some(90));
    assert_eq!(f64s(&dst, a.get(b"RD")), vec![2.0; 4]);
    assert_eq!(text(&dst, &a, b"DS").as_deref(), Some("font: 12pt Helvetica"));
    assert_eq!(
        text(&dst, &a, b"RC").as_deref(),
        Some(r#"<?xml version="1.0"?><body xmlns="http://www.w3.org/1999/xhtml"><p>Hello <b>there</b></p></body>"#)
    );
    // Without <contents>, the comment's text is the rich text's.
    assert_eq!(text(&dst, &a, b"Contents").as_deref(), Some("Hello there"));
    let bs = sub_dict(&dst, &a, b"BS");
    assert_eq!(bs.name(b"S"), Some(b"D".as_slice()));
    assert_eq!(f64s(&dst, bs.get(b"D")), vec![3.0, 2.0]);
    let b = named(&dst, "b");
    assert_eq!(b.get(b"LL").and_then(Object::as_f64), Some(10.0));
    assert_eq!(b.get(b"LLE").and_then(Object::as_f64), Some(2.0));
    assert_eq!(b.get(b"LLO").and_then(Object::as_f64), Some(1.0));
}

#[test]
fn plain_rich_text_is_the_comments_text_and_paragraphs_become_lines() {
    let xml = r#"<?xml version="1.0"?><xfdf xmlns="http://ns.adobe.com/xfdf/"><annots>
<text page="0" rect="10,10,30,30" name="p"><contents-richtext>just text</contents-richtext></text>
<text page="0" rect="40,10,60,30" name="q"><contents-richtext>
  <body xmlns="http://www.w3.org/1999/xhtml">
    <p>one</p>
    <p>two</p>
  </body>
</contents-richtext></text>
</annots></xfdf>"#;
    let mut dst = pages(1);
    import(&mut dst, xml.as_bytes()).unwrap();
    let p = named(&dst, "p");
    assert_eq!(text(&dst, &p, b"Contents").as_deref(), Some("just text"));
    assert!(!p.contains(b"RC"));
    assert_eq!(text(&dst, &named(&dst, "q"), b"Contents").as_deref(), Some("one\rtwo"));
}

#[test]
fn rich_text_that_is_not_xml_survives_as_text() {
    let mut src = pages(1);
    let mut d = annot("Text", "bad", [10.0, 10.0, 30.0, 30.0]);
    d.set(b"Contents".to_vec(), PdfString::text("x"));
    d.set(b"RC".to_vec(), PdfString::text("<p>unclosed & raw"));
    put(&mut src, d);
    let (xml, dst) = round_trip_xfdf(&src);
    assert!(xml.contains("<contents-richtext>&lt;p&gt;unclosed &amp; raw</contents-richtext>"), "{xml}");
    let d = named(&dst, "bad");
    assert_eq!(text(&dst, &d, b"RC").as_deref(), Some("<p>unclosed & raw"));
    assert_eq!(text(&dst, &d, b"Contents").as_deref(), Some("x"));
}

/// An image stamp: its appearance draws an image XObject from its resources.
fn image_stamp(doc: &mut Document, nm: &str, sub: &str) -> (Vec<u8>, Vec<u8>) {
    let pixels: Vec<u8> = (0..=255u8).cycle().take(3 * 16 * 16).collect();
    let mut im = Dict::new();
    im.set(b"Type".to_vec(), Object::name("XObject"));
    im.set(b"Subtype".to_vec(), Object::name("Image"));
    im.set(b"Width".to_vec(), Object::Int(16));
    im.set(b"Height".to_vec(), Object::Int(16));
    im.set(b"ColorSpace".to_vec(), Object::name("DeviceRGB"));
    im.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    let im = doc.add(Object::Stream(Stream { dict: im, raw: pixels.clone().into() }));
    let content = b"q 200 0 0 100 0 0 cm /Im0 Do Q".to_vec();
    let mut xo = Dict::new();
    xo.set(b"Im0".to_vec(), Object::Ref(im));
    let mut res = Dict::new();
    res.set(b"XObject".to_vec(), Object::Dict(xo));
    let mut form = Dict::new();
    form.set(b"Type".to_vec(), Object::name("XObject"));
    form.set(b"Subtype".to_vec(), Object::name("Form"));
    form.set(b"BBox".to_vec(), nums(&[0.0, 0.0, 200.0, 100.0]));
    form.set(b"Resources".to_vec(), Object::Dict(res));
    form.set(b"Name".to_vec(), PdfString::text("logo"));
    let n = doc.add(Object::Stream(Stream { dict: form, raw: content.clone().into() }));
    let mut ap = Dict::new();
    ap.set(b"N".to_vec(), Object::Ref(n));
    let mut d = annot(sub, nm, [100.0, 500.0, 300.0, 600.0]);
    d.set(b"Name".to_vec(), Object::name("Draft"));
    d.set(b"AP".to_vec(), Object::Dict(ap));
    put(doc, d);
    (content, pixels)
}

/// The normal appearance of `d`, and its `/Im0` image's data.
fn appearance_of(doc: &Document, d: &Dict) -> (Vec<u8>, Vec<u8>) {
    let n = doc.resolve(sub_dict(doc, d, b"AP").get(b"N").unwrap());
    let Object::Stream(n) = n.as_ref() else { panic!("no appearance stream") };
    let im = sub_dict(doc, &sub_dict(doc, &n.dict, b"Resources"), b"XObject").get(b"Im0").map(|o| doc.resolve(o));
    let pixels = match im.as_deref() {
        Some(Object::Stream(s)) => s.decoded().unwrap(),
        _ => Vec::new(),
    };
    (n.decoded().unwrap(), pixels)
}

#[test]
fn an_image_stamp_keeps_its_appearance_through_xfdf() {
    let mut src = pages(1);
    let (content, pixels) = image_stamp(&mut src, "logo", "Stamp");
    let (xml, dst) = round_trip_xfdf(&src);
    assert!(xml.contains("<appearance>"), "{xml}");
    let d = named(&dst, "logo");
    assert_eq!(appearance_of(&dst, &d), (content, pixels), "the stamp comes back with its own picture, not a drawn 'DRAFT'");
    // And it survives a save.
    let saved = Document::open(Arc::new(write_full(&dst, &SaveOptions::default()).unwrap())).unwrap();
    let d = named(&saved, "logo");
    assert_eq!(appearance_of(&saved, &d).1.len(), 3 * 16 * 16);
}

#[test]
fn file_export_carries_only_stamp_appearances_and_copies_carry_every_one() {
    let mut src = pages(1);
    let (content, _) = image_stamp(&mut src, "sq", "Square");
    let xml = export_xfdf(&src, true, false, "src.pdf");
    assert!(!xml.contains("<appearance>"), "XFDF 3.0 gives only stamps an appearance element");
    let copy = export_xfdf_comments(&src, &[(0, 0)], "src.pdf");
    assert!(copy.contains("<appearance>"));
    let mut dst = pages(1);
    import_comments(&mut dst, &copy, Some(0), Some("paste"), Some([10.0, -10.0])).unwrap();
    let d = named(&dst, "paste-1");
    assert_eq!(appearance_of(&dst, &d).0, content, "a pasted copy looks the same");
    assert_eq!(f64s(&dst, d.get(b"Rect")), vec![110.0, 490.0, 310.0, 590.0]);
}

#[test]
fn fdf_carries_only_stamp_appearances() {
    let mut src = pages(1);
    image_stamp(&mut src, "sq", "Square");
    let mut ft = formatted_freetext();
    ft.set(b"NM".to_vec(), PdfString::text("box"));
    let ft = put(&mut src, ft);
    let n = sub_dict(&src, &named(&src, "sq"), b"AP");
    src.update_dict(ft, |d| d.set(b"AP".to_vec(), Object::Dict(n.clone()))).unwrap();
    let fdf = export_fdf(&src, true, false, "src.pdf").unwrap();
    assert!(!fdf.windows(3).any(|w| w == b"/AP"), "a text box and a square carry their formatting, not their appearance");
    let mut dst = pages(1);
    import(&mut dst, &fdf).unwrap();
    let ft = named(&dst, "box");
    assert_eq!((ft.int(b"Q"), ft.int(b"Rotate")), (Some(2), Some(90)));
    assert_eq!(text(&dst, &ft, b"DS").as_deref(), text(&src, &formatted_freetext(), b"DS").as_deref());
    let mut stamp = pages(1);
    image_stamp(&mut stamp, "st", "Stamp");
    assert!(export_fdf(&stamp, true, false, "src.pdf").unwrap().windows(3).any(|w| w == b"/AP"));
}

/// The intent (/IT) goes out under both spellings XFDF files use, and either comes back.
#[test]
fn the_intent_is_written_and_read_as_intent_or_it() {
    let mut src = pages(1);
    let mut d = annot("FreeText", "tw", [10.0, 10.0, 90.0, 30.0]);
    d.set(b"IT".to_vec(), Object::name("FreeTextTypewriter"));
    put(&mut src, d);
    let xml = export_xfdf(&src, true, false, "src.pdf");
    assert!(xml.contains(r#"intent="FreeTextTypewriter" IT="FreeTextTypewriter""#), "{xml}");
    let only_it = r#"<?xml version="1.0" encoding="UTF-8"?>
<xfdf xmlns="http://ns.adobe.com/xfdf/" xml:space="preserve"><annots>
<freetext opacity="1" flags="print" IT="FreeTextTypewriter" name="a" page="0" justification="left" rect="71.000400,737.255000,138.977000,747.299000" rotation="0" subject="Typewriter"><defaultappearance> /Helvetica 9 Tf</defaultappearance><defaultstyle>font: Helvetica ,sans-serif 9.00pt;color:#FF0000</defaultstyle></freetext>
<freetext IT="FreeTextCallout" name="b" page="0" rect="10,10,90,30" callout="5,5,20,20,10,10"/>
</annots></xfdf>"#;
    let mut dst = pages(1);
    import(&mut dst, only_it.as_bytes()).unwrap();
    let a = named(&dst, "a");
    assert_eq!(a.name(b"IT"), Some(b"FreeTextTypewriter".as_slice()));
    assert_eq!((a.int(b"Q"), a.int(b"Rotate"), text(&dst, &a, b"Subj").as_deref()), (Some(0), Some(0), Some("Typewriter")));
    assert_eq!(f64s(&dst, a.get(b"Rect")), vec![71.0004, 737.255, 138.977, 747.299]);
    assert_eq!(named(&dst, "b").name(b"IT"), Some(b"FreeTextCallout".as_slice()));
}

#[test]
fn a_stamp_without_area_exports_no_appearance() {
    let mut src = pages(1);
    image_stamp(&mut src, "flat", "Stamp");
    let r = xfdf::find_by_name(&src, "flat").unwrap();
    src.update_dict(r, |d| d.set(b"Rect".to_vec(), nums(&[100.0, 500.0, 100.0, 600.0]))).unwrap();
    assert!(!export_xfdf(&src, true, false, "src.pdf").contains("<appearance>"));
}

/// The appearance encoding other tools write (the stamps attached to PDFBOX-4437 and
/// PDFBOX-4478 hold this shape): data written `filtered`/`ascii` under a dictionary that still
/// names its filter, and `raw`/`hex`.
#[test]
fn an_appearance_written_by_another_tool_is_imported() {
    let ap = r#"<DICT KEY="AP">
<STREAM KEY="N" DEFINE="">
<ARRAY KEY="BBox">
<FIXED VAL="0.000000"/>
<FIXED VAL="0.000000"/>
<FIXED VAL="100.000000"/>
<FIXED VAL="50.000000"/>
</ARRAY>
<ARRAY KEY="Filter">
<NAME VAL="FlateDecode"/>
</ARRAY>
<INT KEY="FormType" VAL="1"/>
<INT KEY="Length" VAL="999"/>
<DICT KEY="Resources">
<DICT KEY="XObject">
<STREAM KEY="Fm0">
<ARRAY KEY="BBox"><INT VAL="0"/><INT VAL="0"/><INT VAL="10"/><INT VAL="10"/></ARRAY>
<NAME KEY="Subtype" VAL="Form"/>
<BOOL KEY="Interpolate" VAL="true"/>
<DATA MODE="RAW" ENCODING="HEX">3020302031302031302072650A66
</DATA>
</STREAM>
</DICT>
</DICT>
<NAME KEY="Subtype" VAL="Form"/>
<NAME KEY="Type" VAL="XObject"/>
<DATA MODE="FILTERED" ENCODING="ASCII">q
/Fm0 Do
0 0 1 rg 0 0 100 50 re f
Q
</DATA>
</STREAM>
</DICT>"#;
    let b64 = base64::engine::general_purpose::STANDARD.encode(ap);
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><xfdf xmlns="http://ns.adobe.com/xfdf/" xml:space="preserve"><annots><stamp page="0" rect="200,600,300,650" name="s" icon="SBApproved" subject="Approved"><appearance>{b64}</appearance></stamp></annots></xfdf>"#
    );
    let mut dst = pages(1);
    let report = import(&mut dst, xml.as_bytes()).unwrap();
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    let d = named(&dst, "s");
    let n = dst.resolve(sub_dict(&dst, &d, b"AP").get(b"N").unwrap());
    let Object::Stream(n) = n.as_ref() else { panic!("no appearance stream") };
    assert!(!n.dict.contains(b"Filter"), "filtered data is already decoded");
    assert_eq!(n.decoded().unwrap(), b"q\n/Fm0 Do\n0 0 1 rg 0 0 100 50 re f\nQ\n");
    let fm = sub_dict(&dst, &sub_dict(&dst, &n.dict, b"Resources"), b"XObject").get(b"Fm0").map(|o| dst.resolve(o)).unwrap();
    let Object::Stream(fm) = fm.as_ref() else { panic!("no form") };
    assert_eq!(fm.decoded().unwrap(), b"0 0 10 10 re\nf");
    assert_eq!(fm.dict.get(b"Interpolate"), Some(&Object::Bool(true)));
}

/// XFDF with one comment of type `sub` whose `<appearance>` text is `inner`.
fn appearance_xfdf(inner: &str, sub: &str) -> String {
    format!(
        r##"<?xml version="1.0"?><xfdf xmlns="http://ns.adobe.com/xfdf/"><annots><{sub} page="0" rect="10,10,110,60" name="x" color="#FF0000"><appearance>{inner}</appearance></{sub}></annots></xfdf>"##
    )
}

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s)
}

#[test]
fn unreadable_appearances_are_skipped_recorded_and_drawn_again() {
    let deep = format!(
        r#"<DICT KEY="AP"><STREAM KEY="N">{}<INT KEY="X" VAL="1"/>{}<DATA MODE="RAW" ENCODING="HEX">00</DATA></STREAM></DICT>"#,
        r#"<DICT KEY="D">"#.repeat(70),
        "</DICT>".repeat(70)
    );
    for (why, inner) in [
        ("not base 64", "@@@not base64@@@".to_string()),
        ("not XML", b64("<DICT KEY=\"AP\"><STREAM")),
        ("not a dictionary", b64("<ARRAY/>")),
        ("no normal appearance", b64(r#"<DICT KEY="AP"><NAME KEY="N" VAL="x"/></DICT>"#)),
        ("a stream without data", b64(r#"<DICT KEY="AP"><STREAM KEY="N"><INT KEY="Length" VAL="1"/></STREAM></DICT>"#)),
        ("odd hexadecimal", b64(r#"<DICT KEY="AP"><STREAM KEY="N"><DATA MODE="RAW" ENCODING="HEX">ABC</DATA></STREAM></DICT>"#)),
        ("an unknown element", b64(r#"<DICT KEY="AP"><STREAM KEY="N"><WHAT/><DATA ENCODING="ASCII">q Q</DATA></STREAM></DICT>"#)),
        ("an unknown encoding", b64(r#"<DICT KEY="AP"><STREAM KEY="N"><DATA ENCODING="ZIP">q Q</DATA></STREAM></DICT>"#)),
        ("a bad number", b64(r#"<DICT KEY="AP"><STREAM KEY="N"><FIXED KEY="X" VAL="nan"/><DATA ENCODING="ASCII">q Q</DATA></STREAM></DICT>"#)),
        ("a DTD", b64(r#"<!DOCTYPE d [<!ENTITY a "aaaa">]><DICT KEY="AP"><STREAM KEY="N"><DATA ENCODING="ASCII">&a;</DATA></STREAM></DICT>"#)),
        ("too deep", b64(&deep)),
        (
            "states with none selected",
            b64(r#"<DICT KEY="AP"><DICT KEY="N"><STREAM KEY="On"><DATA ENCODING="ASCII">q Q</DATA></STREAM></DICT></DICT>"#),
        ),
    ] {
        let mut dst = pages(1);
        let report = import(&mut dst, appearance_xfdf(&inner, "square").as_bytes()).unwrap_or_else(|e| panic!("{why}: {e}"));
        assert_eq!(report.comments, 1, "{why}: the comment itself is imported");
        assert_eq!(report.skipped.len(), 1, "{why}: {:?}", report.skipped);
        assert!(report.skipped[0].starts_with("x: "), "{why}: {:?}", report.skipped);
        // PdfCraft draws the square from its own entries instead.
        let d = named(&dst, "x");
        let n = sub_dict(&dst, &d, b"AP").get(b"N").map(|o| dst.resolve(o));
        assert!(matches!(n.as_deref(), Some(Object::Stream(_))), "{why}: drawn again");
    }
}

#[test]
fn an_oversized_appearance_is_refused_before_it_is_decoded() {
    let mut budget = 1000;
    let inner = b64(&format!(r#"<DICT KEY="AP"><STREAM KEY="N"><DATA ENCODING="ASCII">{}</DATA></STREAM></DICT>"#, "q ".repeat(2000)));
    assert!(appearance::decode(&inner, &mut budget).is_err());
    assert_eq!(budget, 1000, "nothing was charged for a refused appearance");
    let mut budget = appearance::MAX_IMPORT_APPEARANCE_BYTES;
    let big = "A".repeat(appearance::MAX_APPEARANCE_BYTES / 3 * 4 + 8);
    let err = appearance::decode(&big, &mut budget).unwrap_err();
    assert!(err.contains("larger than"), "{err}");
}

#[test]
fn many_appearances_share_one_import_budget() {
    let mut budget = 100;
    let one = b64(r#"<DICT KEY="AP"><STREAM KEY="N"><DATA ENCODING="ASCII">0 0 m 1 1 l S</DATA></STREAM></DICT>"#);
    let mut read = 0;
    while appearance::decode(&one, &mut budget).is_ok() {
        read += 1;
        assert!(read < 10);
    }
    assert!(read >= 1 && budget < 100);
}

#[test]
fn an_imported_redaction_mark_is_always_drawn_by_pdfcraft() {
    let inner = b64(
        r#"<DICT KEY="AP"><STREAM KEY="N"><ARRAY KEY="BBox"><INT VAL="0"/><INT VAL="0"/><INT VAL="1"/><INT VAL="1"/></ARRAY><DATA ENCODING="ASCII">%nothing</DATA></STREAM></DICT>"#,
    );
    let mut dst = pages(1);
    import(&mut dst, appearance_xfdf(&inner, "redact").as_bytes()).unwrap();
    let d = named(&dst, "x");
    let n = dst.resolve(sub_dict(&dst, &d, b"AP").get(b"N").unwrap());
    let Object::Stream(n) = n.as_ref() else { panic!("no appearance") };
    assert_ne!(n.decoded().unwrap(), b"%nothing", "the file's appearance could hide what the mark removes");
}

#[test]
fn appearance_cycles_and_strings_survive_export() {
    let mut src = pages(1);
    image_stamp(&mut src, "c", "Stamp");
    let d = named(&src, "c");
    let n = sub_dict(&src, &d, b"AP").get(b"N").and_then(Object::as_ref).unwrap();
    // The form's resources name the form itself, and hold a binary string.
    let Object::Stream(mut form) = (*src.get(n)).clone() else { panic!("no form") };
    let mut xo = Dict::new();
    xo.set(b"Self".to_vec(), Object::Ref(n));
    let mut res = sub_dict(&src, &form.dict, b"Resources");
    res.set(b"Pattern".to_vec(), Object::Dict(xo));
    res.set(b"Bin".to_vec(), Object::String(PdfString::literal(vec![0u8, 1, 0xFE, 0xFF])));
    form.dict.set(b"Resources".to_vec(), Object::Dict(res));
    src.set(n, Object::Stream(form));
    let (xml, dst) = round_trip_xfdf(&src);
    assert!(xml.contains("<appearance>"));
    let d = named(&dst, "c");
    let n = dst.resolve(sub_dict(&dst, &d, b"AP").get(b"N").unwrap());
    let Object::Stream(n) = n.as_ref() else { panic!("no form") };
    let res = sub_dict(&dst, &n.dict, b"Resources");
    assert_eq!(res.get(b"Bin"), Some(&Object::String(PdfString::literal(vec![0u8, 1, 0xFE, 0xFF]))));
    // The reference back to the form ends there (as null, so the entry is left out).
    assert!(sub_dict(&dst, &res, b"Pattern").is_empty());
}

#[test]
fn an_appearance_that_names_its_page_does_not_carry_the_page_along() {
    let mut src = pages(1);
    image_stamp(&mut src, "pg", "Stamp");
    let d = named(&src, "pg");
    let n = sub_dict(&src, &d, b"AP").get(b"N").and_then(Object::as_ref).unwrap();
    let page = page_ref(&src, 0);
    let Object::Stream(mut form) = (*src.get(n)).clone() else { panic!("no form") };
    form.dict.set(b"PieceInfo".to_vec(), Object::Ref(page));
    src.set(n, Object::Stream(form));
    let xml = export_xfdf(&src, true, false, "src.pdf");
    let b64: String = xml.split("<appearance>").nth(1).unwrap().split("</appearance>").next().unwrap().split_whitespace().collect();
    let inner = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(b64).unwrap()).unwrap();
    assert!(inner.contains(r#"<NULL KEY="PieceInfo"/>"#), "{inner}");
    assert!(!inner.contains("Kids") && !inner.contains("MediaBox"));
}

#[test]
fn fdf_carries_appearances_with_their_resources() {
    let mut src = pages(1);
    let (content, pixels) = image_stamp(&mut src, "logo", "Stamp");
    put(&mut src, formatted_freetext());
    let fdf = export_fdf(&src, true, false, "src.pdf").unwrap();
    assert!(fdf.windows(4).any(|w| w == b"/AP "), "the FDF holds the appearance");
    let mut dst = pages(1);
    import(&mut dst, &fdf).unwrap();
    let d = named(&dst, "logo");
    assert_eq!(appearance_of(&dst, &d), (content, pixels));
    // Formatting already travelled in FDF and still does.
    let ft = named(&dst, "ft");
    assert_eq!((ft.int(b"Q"), ft.int(b"Rotate")), (Some(2), Some(90)));
    assert_eq!(text(&dst, &ft, b"RC").as_deref(), Some(RICH));
    let saved = write_full(&dst, &SaveOptions::default()).unwrap();
    assert!(Document::open(Arc::new(saved)).is_ok());
}

/// A shading's function sits a dozen levels below the comment (appearance, form, resources,
/// pattern, shading, function, functions, …); an FDF import nulled everything past 16 levels, so
/// a gradient stamp lost its colours.
#[test]
fn deeply_nested_appearance_resources_survive_fdf_and_xfdf() {
    let mut src = pages(1);
    image_stamp(&mut src, "deep", "Stamp");
    let d = named(&src, "deep");
    let n = sub_dict(&src, &d, b"AP").get(b"N").and_then(Object::as_ref).unwrap();
    let Object::Stream(mut form) = (*src.get(n)).clone() else { panic!("no form") };
    let mut inner = Object::Array(vec![Object::Real(0.25), Object::Real(0.5), Object::Real(0.75)]);
    for _ in 0..20 {
        let mut level = Dict::new();
        level.set(b"Next".to_vec(), inner);
        inner = Object::Dict(level);
    }
    let mut res = sub_dict(&src, &form.dict, b"Resources");
    res.set(b"Pattern".to_vec(), inner);
    form.dict.set(b"Resources".to_vec(), Object::Dict(res));
    src.set(n, Object::Stream(form));
    let innermost = |doc: &Document| {
        let d = named(doc, "deep");
        let n = doc.resolve(sub_dict(doc, &d, b"AP").get(b"N").unwrap());
        let Object::Stream(n) = n.as_ref() else { panic!("no form") };
        let mut at = sub_dict(doc, &n.dict, b"Resources").get(b"Pattern").cloned().unwrap();
        for _ in 0..20 {
            at = at.as_dict().and_then(|d| d.get(b"Next")).cloned().unwrap_or(Object::Null);
        }
        f64s(doc, Some(&at))
    };
    let mut dst = pages(1);
    import(&mut dst, &export_fdf(&src, true, false, "src.pdf").unwrap()).unwrap();
    assert_eq!(innermost(&dst), vec![0.25, 0.5, 0.75], "FDF");
    let (_, dst) = round_trip_xfdf(&src);
    assert_eq!(innermost(&dst), vec![0.25, 0.5, 0.75], "XFDF");
}

#[test]
fn fdf_comments_off_the_document_copy_no_streams_in() {
    let mut src = pages(3);
    // A stamp on the third page, imported into a one-page document: nothing to place it on.
    image_stamp(&mut src, "far", "Stamp");
    let r = xfdf::find_by_name(&src, "far").unwrap();
    let p2 = page_ref(&src, 2);
    let p0 = page_ref(&src, 0);
    src.update_dict(p0, |p| {
        p.remove(b"Annots");
    })
    .unwrap();
    src.update_dict(p2, |p| p.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(r)]))).unwrap();
    let fdf = export_fdf(&src, true, false, "src.pdf").unwrap();
    let mut dst = pages(1);
    let before = dst.add(Object::Null);
    assert!(import(&mut dst, &fdf).is_err(), "nothing in the file is for this document");
    let after = dst.add(Object::Null);
    assert_eq!(after.num, before.num + 1, "the stamp's appearance and image were not copied in");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// Whatever formatting a free text and a line comment carry comes back the same.
    #[test]
    fn formatting_round_trips(
        q in 0i64..3,
        rotate in prop::sample::select(vec![0i64, 90, 180, 270]),
        width in 0.0f64..20.0,
        dashes in prop::collection::vec(0.5f64..30.0, 1..6),
        fringe in prop::collection::vec(0.0f64..50.0, 4),
        ll in -100.0f64..100.0,
        lle in 0.0f64..50.0,
        llo in 0.0f64..50.0,
        ds in "[a-zA-Z0-9:;.# ]{0,40}",
    ) {
        // XFDF writes numbers to four decimals.
        let r = |v: f64| (v * 10_000.0).round() / 10_000.0;
        let mut src = pages(1);
        let mut ft = formatted_freetext();
        ft.set(b"Q".to_vec(), Object::Int(q));
        ft.set(b"Rotate".to_vec(), Object::Int(rotate));
        ft.set(b"RD".to_vec(), nums(&fringe));
        ft.set(b"DS".to_vec(), PdfString::text(&ds));
        let mut bs = Dict::new();
        bs.set(b"W".to_vec(), Object::Real(width));
        bs.set(b"S".to_vec(), Object::name("D"));
        bs.set(b"D".to_vec(), nums(&dashes));
        ft.set(b"BS".to_vec(), Object::Dict(bs));
        put(&mut src, ft);
        let mut ln = formatted_line();
        ln.set(b"LL".to_vec(), Object::Real(ll));
        ln.set(b"LLE".to_vec(), Object::Real(lle));
        ln.set(b"LLO".to_vec(), Object::Real(llo));
        put(&mut src, ln);
        let (_, dst) = round_trip_xfdf(&src);
        let ft = named(&dst, "ft");
        prop_assert_eq!(ft.int(b"Q"), Some(q));
        prop_assert_eq!(ft.int(b"Rotate"), Some(rotate));
        prop_assert_eq!(f64s(&dst, ft.get(b"RD")), fringe.iter().map(|v| r(*v)).collect::<Vec<_>>());
        prop_assert_eq!(text(&dst, &ft, b"DS"), Some(ds.clone()));
        let bs = sub_dict(&dst, &ft, b"BS");
        prop_assert_eq!(bs.get(b"W").and_then(Object::as_f64), Some(r(width)));
        prop_assert_eq!(f64s(&dst, bs.get(b"D")), dashes.iter().map(|v| r(*v)).collect::<Vec<_>>());
        let ln = named(&dst, "ln");
        prop_assert_eq!(ln.get(b"LL").and_then(Object::as_f64), Some(r(ll)));
        prop_assert_eq!(ln.get(b"LLE").and_then(Object::as_f64), Some(r(lle)));
        prop_assert_eq!(ln.get(b"LLO").and_then(Object::as_f64), Some(r(llo)));
    }

    /// Any appearance dictionary of numbers, names, strings and nested data comes back the same.
    #[test]
    fn appearance_values_round_trip(
        ints in prop::collection::vec(any::<i64>(), 0..8),
        reals in prop::collection::vec(-1.0e6f64..1.0e6, 0..8),
        names in prop::collection::vec("[A-Za-z#/ ]{1,12}", 0..5),
        strings in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..20), 0..5),
        data in prop::collection::vec(any::<u8>(), 0..300),
    ) {
        let mut src = pages(1);
        let mut extra = Dict::new();
        extra.set(b"Ints".to_vec(), Object::Array(ints.iter().map(|i| Object::Int(*i)).collect()));
        extra.set(b"Reals".to_vec(), Object::Array(reals.iter().map(|v| Object::Real(*v)).collect()));
        extra.set(b"Names".to_vec(), Object::Array(names.iter().map(|n| Object::Name(n.as_bytes().to_vec())).collect()));
        extra.set(b"Strings".to_vec(), Object::Array(strings.iter().map(|s| Object::String(PdfString::literal(s.clone()))).collect()));
        let mut form = Dict::new();
        form.set(b"Subtype".to_vec(), Object::name("Form"));
        form.set(b"BBox".to_vec(), nums(&[0.0, 0.0, 10.0, 10.0]));
        form.set(b"Extra".to_vec(), Object::Dict(extra.clone()));
        let n = src.add(Object::Stream(Stream { dict: form, raw: data.clone().into() }));
        let mut ap = Dict::new();
        ap.set(b"N".to_vec(), Object::Ref(n));
        let mut d = annot("Stamp", "p", [0.0, 0.0, 10.0, 10.0]);
        d.set(b"AP".to_vec(), Object::Dict(ap));
        put(&mut src, d);
        let (_, dst) = round_trip_xfdf(&src);
        let d = named(&dst, "p");
        let n = dst.resolve(sub_dict(&dst, &d, b"AP").get(b"N").unwrap());
        let Object::Stream(n) = n.as_ref() else { panic!("no appearance") };
        prop_assert_eq!(n.raw.as_ref(), data.as_slice());
        prop_assert_eq!(sub_dict(&dst, &n.dict, b"Extra"), extra);
    }
}

/// A stamp on page 0 whose normal appearance is one form stream holding `content`.
fn content_stamp(doc: &mut Document, nm: &str, content: Vec<u8>) -> ObjRef {
    let mut form = Dict::new();
    form.set(b"Type".to_vec(), Object::name("XObject"));
    form.set(b"Subtype".to_vec(), Object::name("Form"));
    form.set(b"BBox".to_vec(), nums(&[0.0, 0.0, 100.0, 50.0]));
    let n = doc.add(Object::Stream(Stream { dict: form, raw: content.into() }));
    let mut ap = Dict::new();
    ap.set(b"N".to_vec(), Object::Ref(n));
    let mut d = annot("Stamp", nm, [100.0, 500.0, 200.0, 550.0]);
    d.set(b"AP".to_vec(), Object::Dict(ap));
    put(doc, d);
    n
}

/// The `<appearance>` XML export wrote for the first comment of `xml`.
fn exported_appearance(xml: &str) -> String {
    let b64: String = xml.split("<appearance>").nth(1).unwrap().split("</appearance>").next().unwrap().split_whitespace().collect();
    String::from_utf8(base64::engine::general_purpose::STANDARD.decode(b64).unwrap()).unwrap()
}

/// The write-time checks leave out the closing tags and the hexadecimal line breaks: an appearance
/// just under the limit there came out over it, and import then refused what export wrote.
#[test]
fn an_exported_appearance_always_fits_what_import_reads() {
    let mut src = pages(1);
    content_stamp(&mut src, "big", vec![0u8; (appearance::MAX_APPEARANCE_BYTES / 2) - 4096]);
    let d = named(&src, "big");
    if let Some(text) = appearance::encode(&src, &d, &mut appearance::MAX_IMPORT_APPEARANCE_BYTES.clone()) {
        let mut budget = appearance::MAX_IMPORT_APPEARANCE_BYTES;
        appearance::decode(&text, &mut budget).expect("what export writes, import reads");
    }
}

/// A page whose `/Type` is a reference is still the source page: written in place it would carry
/// the page's contents into a file meant to hold one comment.
#[test]
fn a_page_with_an_indirect_type_stays_out_of_an_exported_appearance() {
    let mut src = pages(1);
    let n = content_stamp(&mut src, "pg", b"q Q".to_vec());
    let page = page_ref(&src, 0);
    let page_type = src.add(Object::name("Page"));
    src.update_dict(page, |p| p.set(b"Type".to_vec(), Object::Ref(page_type))).unwrap();
    let Object::Stream(mut form) = (*src.get(n)).clone() else { panic!("no form") };
    form.dict.set(b"PieceInfo".to_vec(), Object::Ref(page));
    src.set(n, Object::Stream(form));
    let inner = exported_appearance(&export_xfdf(&src, true, false, "src.pdf"));
    assert!(inner.contains(r#"<NULL KEY="PieceInfo"/>"#), "{inner}");
    assert!(!inner.contains("MediaBox"), "{inner}");
}

/// XML reads a carriage return as a line feed, so data holding one goes out as hexadecimal.
#[test]
fn stream_data_with_carriage_returns_comes_back_byte_for_byte() {
    let mut src = pages(1);
    let data = b"q\r/Im0\rDo\r\nQ\r".to_vec();
    content_stamp(&mut src, "cr", data.clone());
    let (_, dst) = round_trip_xfdf(&src);
    let d = named(&dst, "cr");
    let n = dst.resolve(sub_dict(&dst, &d, b"AP").get(b"N").unwrap());
    let Object::Stream(n) = n.as_ref() else { panic!("no appearance") };
    assert_eq!(n.raw.as_ref(), data.as_slice());
}

/// XFDF has no appearance state: the stream `/AS` selects is what goes out.
#[test]
fn the_selected_appearance_state_is_what_xfdf_carries() {
    let mut src = pages(1);
    let on = content_stamp(&mut src, "st", b"0 0 1 rg 0 0 100 50 re f".to_vec());
    let off = src.add(Object::Stream(Stream { dict: Dict::new(), raw: b"%off".to_vec().into() }));
    let mut states = Dict::new();
    states.set(b"On".to_vec(), Object::Ref(on));
    states.set(b"Off".to_vec(), Object::Ref(off));
    let mut ap = Dict::new();
    ap.set(b"N".to_vec(), Object::Dict(states.clone()));
    ap.set(b"D".to_vec(), Object::Dict(states));
    let r = xfdf::find_by_name(&src, "st").unwrap();
    src.update_dict(r, |d| {
        d.set(b"AP".to_vec(), Object::Dict(ap.clone()));
        d.set(b"AS".to_vec(), Object::name("On"));
    })
    .unwrap();
    let (_, dst) = round_trip_xfdf(&src);
    let d = named(&dst, "st");
    let ap = sub_dict(&dst, &d, b"AP");
    for key in [b"N".as_slice(), b"D"] {
        let s = dst.resolve(ap.get(key).unwrap());
        let Object::Stream(s) = s.as_ref() else { panic!("{key:?} is not the selected stream: {s:?}") };
        assert_eq!(s.raw.as_ref(), b"0 0 1 rg 0 0 100 50 re f");
    }
    // A state no stream answers to has nothing to draw: no appearance goes out.
    src.update_dict(r, |d| d.set(b"AS".to_vec(), Object::name("Gone"))).unwrap();
    assert!(!export_xfdf(&src, true, false, "src.pdf").contains("<appearance>"));
}

/// Forms that draw forms: FDF export writes the chain however long, and import copied only the
/// first dozen and nulled the rest without a word. A chain sixty forms deep comes back whole; one
/// deeper than import follows is refused rather than imported in part.
#[test]
fn a_chain_of_forms_comes_back_whole_from_fdf_or_is_refused() {
    let chain = |forms: usize| {
        let mut src = pages(1);
        put(&mut src, annot("Text", "first", [10.0, 10.0, 30.0, 30.0]));
        let mut inner = src.add(Object::Stream(Stream { dict: Dict::new(), raw: b"%innermost".to_vec().into() }));
        for _ in 0..forms {
            let mut xo = Dict::new();
            xo.set(b"Fm".to_vec(), Object::Ref(inner));
            let mut res = Dict::new();
            res.set(b"XObject".to_vec(), Object::Dict(xo));
            let mut form = Dict::new();
            form.set(b"Subtype".to_vec(), Object::name("Form"));
            form.set(b"Resources".to_vec(), Object::Dict(res));
            inner = src.add(Object::Stream(Stream { dict: form, raw: b"/Fm Do".to_vec().into() }));
        }
        let mut ap = Dict::new();
        ap.set(b"N".to_vec(), Object::Ref(inner));
        let mut d = annot("Stamp", "chain", [100.0, 500.0, 200.0, 550.0]);
        d.set(b"AP".to_vec(), Object::Dict(ap));
        put(&mut src, d);
        export_fdf(&src, true, false, "src.pdf").unwrap()
    };
    let mut dst = pages(1);
    import(&mut dst, &chain(60)).unwrap();
    let d = named(&dst, "chain");
    let mut at = dst.resolve(sub_dict(&dst, &d, b"AP").get(b"N").unwrap());
    for level in 0..60 {
        let Object::Stream(s) = at.as_ref() else { panic!("level {level}: {at:?}") };
        let fm = sub_dict(&dst, &sub_dict(&dst, &s.dict, b"Resources"), b"XObject").get(b"Fm").cloned().unwrap_or(Object::Null);
        at = dst.resolve(&fm);
    }
    let Object::Stream(innermost) = at.as_ref() else { panic!("the innermost form was lost: {at:?}") };
    assert_eq!(innermost.raw.as_ref(), b"%innermost");
    let mut dst = pages(1);
    let before = dst.add(Object::Null);
    let err = import(&mut dst, &chain(80)).unwrap_err();
    assert!(err.to_string().contains("nest more than"), "{err}");
    // All or nothing: the note before the stamp, and the forms copied so far, are not left behind.
    assert_eq!(dst.add(Object::Null).num, before.num + 1);
    assert!(xfdf::find_by_name(&dst, "first").is_none());
}

/// A `/P` an FDF comment holds points into the FDF; it is replaced, and nothing it reaches is
/// copied into the document.
#[test]
fn an_fdf_comments_page_link_copies_nothing_in() {
    let fdf = |p: &str| {
        format!(
            "%FDF-1.2\n1 0 obj\n<< /FDF << /Annots [2 0 R] >> >>\nendobj\n2 0 obj\n<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /NM (t) /Page 0{p} >>\nendobj\n3 0 obj\n<< /Type /Page /Junk 4 0 R >>\nendobj\n4 0 obj\n<< /Length 5 >>\nstream\nhello\nendstream\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n"
        )
        .into_bytes()
    };
    let added = |bytes: &[u8]| {
        let mut dst = pages(1);
        let before = dst.add(Object::Null);
        import(&mut dst, bytes).unwrap();
        let p = named(&dst, "t").reference(b"P");
        assert_eq!(p, Some(page_ref(&dst, 0)));
        dst.add(Object::Null).num - before.num
    };
    assert_eq!(added(&fdf(" /P 3 0 R")), added(&fdf("")), "the FDF's page and its stream were not copied in");
}

/// Two line breaks in a row leave a blank line in the comment's plain text.
#[test]
fn consecutive_rich_text_line_breaks_are_kept() {
    let xml = r#"<?xml version="1.0"?><xfdf xmlns="http://ns.adobe.com/xfdf/"><annots>
<text page="0" rect="10,10,30,30" name="b"><contents-richtext><body xmlns="http://www.w3.org/1999/xhtml"><p>one<br/><br/>two</p><p><br/></p><p>three<br/></p><p>four</p></body></contents-richtext></text>
</annots></xfdf>"#;
    let mut dst = pages(1);
    import(&mut dst, xml.as_bytes()).unwrap();
    assert_eq!(text(&dst, &named(&dst, "b"), b"Contents").as_deref(), Some("one\r\rtwo\r\rthree\rfour"));
}

/// Rich text that is not XML, on a comment with no plain text, comes back as rich text, not as
/// the comment's text (a line would otherwise show the markup as its caption).
#[test]
fn rich_text_alone_that_is_not_xml_comes_back_as_rich_text() {
    let mut src = pages(1);
    let mut d = annot("Line", "ln", [10.0, 10.0, 90.0, 30.0]);
    d.set(b"L".to_vec(), nums(&[10.0, 20.0, 90.0, 20.0]));
    d.set(b"RC".to_vec(), PdfString::text("<p>unclosed"));
    put(&mut src, d);
    let (_, dst) = round_trip_xfdf(&src);
    let d = named(&dst, "ln");
    assert_eq!(text(&dst, &d, b"RC").as_deref(), Some("<p>unclosed"));
    assert_ne!(text(&dst, &d, b"Contents").as_deref(), Some("<p>unclosed"));
}

/// A prefix the rich text inherits from the XFDF around it is declared on it, so the rich text
/// is XML of its own and goes out again as markup.
#[test]
fn rich_text_keeps_the_namespaces_it_inherits() {
    let xml = r#"<?xml version="1.0"?><xfdf xmlns="http://ns.adobe.com/xfdf/" xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"><annots>
<text page="0" rect="10,10,30,30" name="ns"><contents>hi</contents><contents-richtext><body xmlns="http://www.w3.org/1999/xhtml" xfa:spec="2.0.2"><p>hi</p></body></contents-richtext></text>
</annots></xfdf>"#;
    let mut dst = pages(1);
    import(&mut dst, xml.as_bytes()).unwrap();
    let rc = text(&dst, &named(&dst, "ns"), b"RC").unwrap();
    assert!(common::parse_form_xml(&rc).is_ok(), "{rc}");
    let out = export_xfdf(&dst, true, false, "dst.pdf");
    assert!(
        out.contains(r#"<contents-richtext><body xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/" xmlns="http://www.w3.org/1999/xhtml" xfa:spec"#),
        "{out}"
    );
}

/// A redaction mark is drawn by PdfCraft whatever the file says: a cloudy border (which a
/// redaction has none of) or text the standard fonts can't draw left the file's blank appearance
/// in place, and the mark removed content nobody saw marked.
#[test]
fn an_imported_redaction_mark_is_drawn_even_when_cloudy_or_undrawable() {
    let inner = b64(
        r#"<DICT KEY="AP"><STREAM KEY="N"><ARRAY KEY="BBox"><INT VAL="0"/><INT VAL="0"/><INT VAL="1"/><INT VAL="1"/></ARRAY><DATA ENCODING="ASCII">%nothing</DATA></STREAM></DICT>"#,
    );
    for extra in [r#" style="cloudy""#, r#" overlay-text="日本語""#] {
        let xml = appearance_xfdf(&inner, "redact").replace(r#"name="x""#, &format!(r#"name="x"{extra}"#));
        let mut dst = pages(1);
        let before = dst.add(Object::Null);
        import(&mut dst, xml.as_bytes()).unwrap();
        let d = named(&dst, "x");
        let n = dst.resolve(sub_dict(&dst, &d, b"AP").get(b"N").unwrap());
        let Object::Stream(n) = n.as_ref() else { panic!("{extra}: no appearance") };
        let drawn = n.decoded().unwrap();
        assert_ne!(drawn, b"%nothing", "{extra}");
        assert!(String::from_utf8_lossy(&drawn).contains("h S"), "{extra}: the outline is stroked: {}", String::from_utf8_lossy(&drawn));
        assert!(!d.contains(b"BE"), "{extra}");
        // The file's appearance was not read in at all.
        for num in before.num..dst.add(Object::Null).num {
            if let Object::Stream(s) = dst.get(ObjRef::new(num, 0)).as_ref() {
                assert_ne!(s.raw.as_ref(), b"%nothing", "{extra}");
            }
        }
    }
}

/// Appearances share one budget over the whole import: past it, the rest are skipped.
#[test]
fn the_import_budget_is_shared_by_every_comment() {
    let one = b64(r#"<DICT KEY="AP"><STREAM KEY="N"><DATA ENCODING="ASCII">0 0 m 1 1 l S</DATA></STREAM></DICT>"#);
    let size = base64::engine::general_purpose::STANDARD.decode(&one).unwrap().len();
    let stamps: String = ["a", "b", "c"]
        .iter()
        .map(|nm| format!(r#"<stamp page="0" rect="10,10,110,60" name="{nm}"><appearance>{one}</appearance></stamp>"#))
        .collect();
    let xml = format!(r#"<?xml version="1.0"?><xfdf xmlns="http://ns.adobe.com/xfdf/"><annots>{stamps}</annots></xfdf>"#);
    let mut dst = pages(1);
    let report = xfdf::import_xfdf_with(&mut dst, &xml, None, None, None, size * 5 / 2).unwrap();
    assert_eq!(report.comments, 3);
    assert_eq!(report.skipped.len(), 1, "{:?}", report.skipped);
    assert!(report.skipped[0].starts_with("c: "), "{:?}", report.skipped);
}

/// A dash array of any length export writes comes back (65 numbers were written and dropped).
#[test]
fn a_long_dash_array_comes_back() {
    let mut src = pages(1);
    let dashes: Vec<f64> = (1..=65).map(f64::from).collect();
    let mut bs = Dict::new();
    bs.set(b"W".to_vec(), Object::Real(1.0));
    bs.set(b"S".to_vec(), Object::name("D"));
    bs.set(b"D".to_vec(), nums(&dashes));
    let mut d = annot("Square", "sq", [10.0, 10.0, 90.0, 60.0]);
    d.set(b"BS".to_vec(), Object::Dict(bs));
    put(&mut src, d);
    let (_, dst) = round_trip_xfdf(&src);
    assert_eq!(f64s(&dst, sub_dict(&dst, &named(&dst, "sq"), b"BS").get(b"D")), dashes);
}

/// Export stops writing appearances where import would stop reading them, so every appearance
/// written comes back.
#[test]
fn export_writes_no_more_appearance_than_import_reads() {
    let mut src = pages(1);
    content_stamp(&mut src, "a", b"0 0 m 1 1 l S".to_vec());
    let d = named(&src, "a");
    let mut probe = usize::MAX;
    let one = appearance::encode(&src, &d, &mut probe).unwrap();
    let size = usize::MAX - probe;
    let mut budget = size * 3 / 2;
    assert_eq!(appearance::encode(&src, &d, &mut budget), Some(one));
    assert_eq!(appearance::encode(&src, &d, &mut budget), None, "past what import reads");
}

/// A page an FDF stamp's appearance reaches (its /Type a reference here) is not copied in with
/// its contents.
#[test]
fn an_fdf_appearance_that_reaches_a_page_copies_no_page_in() {
    let fdf = b"%FDF-1.2\n1 0 obj\n<< /FDF << /Annots [2 0 R] >> >>\nendobj\n\
2 0 obj\n<< /Type /Annot /Subtype /Stamp /Rect [10 10 60 60] /NM (s) /Page 0 /AP << /N 3 0 R >> >>\nendobj\n\
3 0 obj\n<< /Subtype /Form /BBox [0 0 1 1] /PieceInfo 4 0 R /Length 3 >>\nstream\nq Q\nendstream\nendobj\n\
4 0 obj\n<< /Type 6 0 R /Contents 5 0 R >>\nendobj\n\
5 0 obj\n<< /Length 11 >>\nstream\npagecontent\nendstream\nendobj\n\
6 0 obj\n/Page\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n";
    let mut dst = pages(1);
    let before = dst.add(Object::Null);
    import(&mut dst, fdf).unwrap();
    for num in before.num..dst.add(Object::Null).num {
        if let Object::Stream(s) = dst.get(ObjRef::new(num, 0)).as_ref() {
            assert_ne!(s.raw.as_ref(), b"pagecontent", "the page's contents were copied in");
        }
    }
    let n = dst.resolve(sub_dict(&dst, &named(&dst, "s"), b"AP").get(b"N").unwrap());
    let Object::Stream(n) = n.as_ref() else { panic!("no appearance") };
    assert!(matches!(n.dict.get(b"PieceInfo"), None | Some(Object::Null)), "{:?}", n.dict.get(b"PieceInfo"));
}

/// Rich text keeps the default (XHTML) namespace it inherits from `<contents-richtext>`, and a
/// prefix it redeclares with spaces around `=` is not declared twice.
#[test]
fn rich_text_keeps_an_inherited_default_namespace() {
    let xml = r#"<?xml version="1.0"?><xfdf xmlns="http://ns.adobe.com/xfdf/" xmlns:a="urn:a" xmlns:b="urn:b"><annots>
<text page="0" rect="10,10,30,30" name="d"><contents>hi</contents><contents-richtext xmlns="http://www.w3.org/1999/xhtml"><body><p>hi</p></body></contents-richtext></text>
<text page="0" rect="40,10,60,30" name="w"><contents>hi</contents><contents-richtext><body xmlns="http://www.w3.org/1999/xhtml" xmlns:a = "urn:a2" a:x="1" b:y="2"><p>hi</p></body></contents-richtext></text>
</annots></xfdf>"#;
    let mut dst = pages(1);
    import(&mut dst, xml.as_bytes()).unwrap();
    let rc = text(&dst, &named(&dst, "d"), b"RC").unwrap();
    let parsed = common::parse_form_xml(&rc).unwrap();
    assert_eq!(parsed.root_element().tag_name().namespace(), Some("http://www.w3.org/1999/xhtml"), "{rc}");
    let rc = text(&dst, &named(&dst, "w"), b"RC").unwrap();
    assert!(common::parse_form_xml(&rc).is_ok(), "{rc}");
    assert!(!rc.contains(r#"xmlns:a="urn:a""#), "{rc}");
}

/// A redaction mark whose /Rect has no area shows its quads all the same: the /Rect grows to hold
/// them.
#[test]
fn a_redaction_marks_rect_holds_its_quads() {
    let xml = r#"<?xml version="1.0"?><xfdf xmlns="http://ns.adobe.com/xfdf/"><annots><redact page="0" name="r" rect="50,50,50,50" coords="10,40,90,40,10,20,90,20"/></annots></xfdf>"#;
    let mut dst = pages(1);
    import(&mut dst, xml.as_bytes()).unwrap();
    assert_eq!(f64s(&dst, named(&dst, "r").get(b"Rect")), vec![10.0, 20.0, 90.0, 50.0]);
}
