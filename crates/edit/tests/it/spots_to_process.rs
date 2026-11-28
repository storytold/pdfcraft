//! Spots to process: a chosen spot ink's colours become the CMYK its tint transform gives, in
//! page content, forms, appearances, images, indexed spaces and shadings, and what can't be
//! converted or loses its overprint is reported.

use pdfcraft_content::parse;
use pdfcraft_cos::{Document, ObjRef, Object};
use pdfcraft_edit::convert_spots_to_process;
use std::sync::Arc;

/// A document of `objs` (object k is `k + 1 0 obj`; object 1 is the catalog). A `>>stream:`
/// entry is written as a stream holding the text after the colon, with its `/Length`; a
/// `>>hex:` entry holds the bytes the hex digits after the colon give.
fn pdf(objs: &[&str]) -> Document {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        if let Some((dict, data)) = o.split_once(">>stream:") {
            out.extend_from_slice(format!("{dict} /Length {} >>\nstream\n{data}\nendstream", data.len()).as_bytes());
        } else if let Some((dict, hex)) = o.split_once(">>hex:") {
            let bytes: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
            out.extend_from_slice(format!("{dict} /Length {} >>\nstream\n", bytes.len()).as_bytes());
            out.extend_from_slice(&bytes);
            out.extend_from_slice(b"\nendstream");
        } else {
            out.extend_from_slice(o.as_bytes());
        }
        out.extend_from_slice(b"\nendobj\n");
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

const CATALOG: &str = "<< /Type /Catalog /Pages 2 0 R >>";
const PAGES: &str = "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>";
/// Gold: a Type 4 (PostScript) tint transform, tint t to C 0, M 0.2t, Y t, K 0.
const GOLD_TINT: &str = "<< /FunctionType 4 /Domain [0 1] /Range [0 1 0 1 0 1 0 1] >>stream:{ dup 0 exch 0.2 mul 3 -1 roll 0 }";

fn gold(t: f64) -> [f64; 4] {
    [0.0, 0.2 * t, t, 0.0]
}

fn stream(doc: &Document, r: ObjRef) -> pdfcraft_cos::Stream {
    match &*doc.get(r) {
        Object::Stream(s) => s.clone(),
        _ => panic!("{r} is not a stream"),
    }
}

/// The operands of every `op` operator in stream `r`.
fn operands(doc: &Document, r: ObjRef, op: &str) -> Vec<Vec<f64>> {
    let data = stream(doc, r).decoded().unwrap();
    parse(&data).ops.iter().filter(|o| o.is(op)).map(|o| o.operands.iter().filter_map(Object::as_f64).collect()).collect()
}

fn close(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01)
}

fn r(n: u32) -> ObjRef {
    ObjRef::new(n, 0)
}

/// One page whose resources are `resources` and whose content (object 4) is `content`; object 5
/// is the Gold tint transform, then `more`.
fn page(resources: &str, content: &str, more: &[&str]) -> Document {
    let page = format!("<< /Type /Page /Parent 2 0 R /Resources << {resources} >> /Contents 4 0 R >>");
    let content = format!("<< >>stream:{content}");
    let mut objs = vec![CATALOG, PAGES, &page, &content, GOLD_TINT];
    objs.extend_from_slice(more);
    pdf(&objs)
}

/// The page's `/Resources /<category> /<name>` entry, an array.
fn space(doc: &Document, category: &[u8], name: &[u8]) -> Vec<Object> {
    let page = doc.get(r(3));
    let res = page.as_dict().and_then(|d| d.get(b"Resources")).and_then(Object::as_dict).and_then(|d| d.get(category)).and_then(Object::as_dict);
    res.and_then(|d| d.get(name)).and_then(Object::as_array).unwrap().clone()
}

const GOLD_SPACE: &str = "/ColorSpace << /G [/Separation /Gold /DeviceCMYK 5 0 R] >>";

#[test]
fn a_spot_colour_becomes_the_cmyk_its_tint_transform_gives() {
    let mut doc = page(GOLD_SPACE, "/G cs 0.5 scn 0 0 10 10 re f /G CS 0 0 m 9 9 l S", &[]);
    let done = convert_spots_to_process(&mut doc, &["Gold".into()]).unwrap();
    assert_eq!((done.spaces, done.refused.len()), (1, 0), "{done:?}");
    assert!(close(operands(&doc, r(4), "k").last().unwrap(), &gold(0.5)), "the fill: {:?}", operands(&doc, r(4), "k"));
    assert!(close(&operands(&doc, r(4), "K")[0], &gold(1.0)), "CS alone gives the initial colour, every tint 1");
    assert!(operands(&doc, r(4), "scn").is_empty() && operands(&doc, r(4), "cs").is_empty(), "no spot operator is left");
    assert_eq!(space(&doc, b"ColorSpace", b"G")[0].as_name(), Some(&b"Separation"[..]), "the space itself stays, for anything not reached");
}

#[test]
fn a_devicen_that_also_names_a_spot_not_chosen_is_left_and_reported() {
    let more = ["<< /FunctionType 4 /Domain [0 1 0 1] /Range [0 1 0 1 0 1 0 1] >>stream:{ 0 0 }"];
    let mut doc = page("/ColorSpace << /N [/DeviceN [/Gold /Silver] /DeviceCMYK 6 0 R] >>", "/N cs 0.5 0.5 scn 0 0 10 10 re f", &more);
    let before = stream(&doc, r(4)).decoded().unwrap();
    let done = convert_spots_to_process(&mut doc, &["Gold".into()]).unwrap();
    assert_eq!(done.converted(), 0, "{done:?}");
    assert!(done.refused.iter().any(|x| x.ink == "Gold" && x.reason.contains("Silver")), "{:?}", done.refused);
    assert_eq!(stream(&doc, r(4)).decoded().unwrap(), before, "the content is unchanged");
    let all = convert_spots_to_process(&mut doc, &[]).unwrap();
    assert!(all.paints >= 2 && all.refused.is_empty(), "choosing every spot converts it: {all:?}");
}

#[test]
fn a_spot_whose_alternate_is_not_cmyk_is_refused() {
    let more = ["<< /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [1 0.8 0] /N 1 >>"];
    let mut doc = page("/ColorSpace << /G [/Separation /Gold /DeviceRGB 6 0 R] >>", "/G cs 1 scn 0 0 10 10 re f", &more);
    let done = convert_spots_to_process(&mut doc, &[]).unwrap();
    assert_eq!(done.converted(), 0);
    assert!(done.refused.iter().any(|x| x.reason.contains("DeviceCMYK")), "{:?}", done.refused);
    assert!(operands(&doc, r(4), "k").is_empty());
}

#[test]
fn a_form_is_converted_under_the_colour_its_caller_set() {
    let more = ["<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] >>stream:0.25 sc 0 0 10 10 re f"];
    let mut doc = page(&format!("{GOLD_SPACE} /XObject << /F 6 0 R >>"), "/G cs /F Do", &more);
    convert_spots_to_process(&mut doc, &["Gold".into()]).unwrap();
    let form = operands(&doc, r(6), "k");
    assert!(form.len() == 1 && close(&form[0], &gold(0.25)), "the form's sc is read in the caller's space: {form:?}");
}

#[test]
fn a_comment_appearance_is_converted() {
    let page = "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Annots [6 0 R] >>";
    let mut doc = pdf(&[
        CATALOG,
        PAGES,
        page,
        "<< >>stream:",
        GOLD_TINT,
        "<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /AP << /N 7 0 R >> >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /ColorSpace << /G [/Separation /Gold /DeviceCMYK 5 0 R] >> >> >>stream:/G cs 0.5 scn 0 0 10 10 re f",
    ]);
    let done = convert_spots_to_process(&mut doc, &["Gold".into()]).unwrap();
    assert!(done.paints >= 2, "{done:?}");
    assert!(close(operands(&doc, r(7), "k").last().unwrap(), &gold(0.5)));
}

#[test]
fn overprint_on_a_converted_ink_is_reported_in_either_mode() {
    let gs = |opm: u8| format!("{GOLD_SPACE} /ExtGState << /O << /OP true /OPM {opm} >> >>");
    for opm in [0, 1] {
        let mut fill = page(&gs(opm), "/O gs /G cs 1 scn 0 0 10 10 re f", &[]);
        let done = convert_spots_to_process(&mut fill, &[]).unwrap();
        assert_eq!(done.overprint_changed, vec![r(4)], "mode {opm}: the CMYK fill now replaces process plates");
        let mut stroke = page(&gs(opm), "/O gs /G CS 1 SCN 0 0 10 10 re S", &[]);
        let done = convert_spots_to_process(&mut stroke, &[]).unwrap();
        assert_eq!(done.overprint_changed, vec![r(4)], "mode {opm}: and so does the stroke");
    }
    let mut plain = page(GOLD_SPACE, "/G cs 1 scn 0 0 10 10 re f", &[]);
    let done = convert_spots_to_process(&mut plain, &[]).unwrap();
    assert!(done.overprint_changed.is_empty() && done.paints > 0, "without overprint nothing is reported: {done:?}");
}

#[test]
fn a_form_drawn_twice_counts_its_paints_once() {
    let more = ["<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] >>stream:0.25 sc 0 0 10 10 re f"];
    let mut once = page(&format!("{GOLD_SPACE} /XObject << /F 6 0 R >>"), "/G cs /F Do", &more);
    let one = convert_spots_to_process(&mut once, &["Gold".into()]).unwrap();
    let mut twice = page(&format!("{GOLD_SPACE} /XObject << /F 6 0 R >>"), "/G cs /F Do /F Do", &more);
    let two = convert_spots_to_process(&mut twice, &["Gold".into()]).unwrap();
    assert_eq!(one.paints, two.paints, "the form is one stream, rewritten once: {one:?} {two:?}");
}

#[test]
fn an_image_decode_array_is_applied_before_the_tint_transform() {
    let more = [
        "<< /Type /XObject /Subtype /Image /Width 2 /Height 1 /BitsPerComponent 8 /Decode [1 0] /ColorSpace [/Separation /Gold /DeviceCMYK 5 0 R] >>hex:00FF",
    ];
    let mut doc = page("/XObject << /I 6 0 R >>", "/I Do", &more);
    let done = convert_spots_to_process(&mut doc, &["Gold".into()]).unwrap();
    assert_eq!(done.images, 1, "{done:?}");
    let s = stream(&doc, r(6));
    assert!(s.dict.get(b"Decode").is_none(), "the written samples need no Decode");
    let px: Vec<f64> = s.decoded().unwrap().iter().map(|b| f64::from(*b) / 255.0).collect();
    assert!(close(&px[..4], &gold(1.0)) && close(&px[4..], &gold(0.0)), "Decode [1 0] inverts the tints: {px:?}");
}

#[test]
fn a_shading_with_an_empty_domain_is_refused() {
    let sh = "/Shading << /S << /ShadingType 2 /Coords [0 0 10 0] /Domain [1 1] /ColorSpace [/Separation /Gold /DeviceCMYK 5 0 R] /Function << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> >> >>";
    let mut doc = page(sh, "/S sh", &[]);
    let done = convert_spots_to_process(&mut doc, &[]).unwrap();
    assert_eq!(done.shadings, 0, "{done:?}");
    assert!(done.refused.iter().any(|f| f.reason.contains("domain")), "{done:?}");
}

#[test]
fn an_image_in_the_spot_becomes_cmyk_samples() {
    let more =
        ["<< /Type /XObject /Subtype /Image /Width 2 /Height 1 /BitsPerComponent 8 /ColorSpace [/Separation /Gold /DeviceCMYK 5 0 R] >>hex:00FF"];
    let mut doc = page("/XObject << /I 6 0 R >>", "/I Do", &more);
    let done = convert_spots_to_process(&mut doc, &["Gold".into()]).unwrap();
    assert_eq!(done.images, 1, "{done:?}");
    let s = stream(&doc, r(6));
    assert_eq!(s.dict.name(b"ColorSpace"), Some(&b"DeviceCMYK"[..]));
    let px: Vec<f64> = s.decoded().unwrap().iter().map(|b| f64::from(*b) / 255.0).collect();
    assert!(close(&px[..4], &gold(0.0)) && close(&px[4..], &gold(1.0)), "each sample through the tint transform: {px:?}");
}

#[test]
fn an_indexed_space_over_the_spot_gets_a_cmyk_lookup_table() {
    let mut doc = page("/ColorSpace << /X [/Indexed [/Separation /Gold /DeviceCMYK 5 0 R] 1 <80FF>] >>", "/X cs 1 sc 0 0 10 10 re f", &[]);
    let done = convert_spots_to_process(&mut doc, &[]).unwrap();
    assert_eq!(done.lookups, 1, "{done:?}");
    let space = space(&doc, b"ColorSpace", b"X");
    assert_eq!(space[1].as_name(), Some(&b"DeviceCMYK"[..]));
    let table: Vec<f64> = space[3].as_string().unwrap().bytes.iter().map(|b| f64::from(*b) / 255.0).collect();
    assert!(close(&table[..4], &gold(128.0 / 255.0)) && close(&table[4..], &gold(1.0)), "{table:?}");
    assert_eq!(operands(&doc, r(4), "sc"), vec![vec![1.0]], "an index stays an index");
}

#[test]
fn an_axial_shading_in_the_spot_is_resampled_in_cmyk() {
    let sh = "/Shading << /S << /ShadingType 2 /Coords [0 0 10 0] /ColorSpace [/Separation /Gold /DeviceCMYK 5 0 R] /Function << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> >> >>";
    let mut doc = page(sh, "/S sh", &[]);
    let done = convert_spots_to_process(&mut doc, &[]).unwrap();
    assert_eq!(done.shadings, 1, "{done:?}");
    let page = doc.get(r(3));
    let shading = page
        .as_dict()
        .and_then(|d| d.get(b"Resources"))
        .and_then(Object::as_dict)
        .and_then(|d| d.get(b"Shading"))
        .and_then(Object::as_dict)
        .and_then(|d| d.get(b"S"))
        .and_then(Object::as_dict)
        .unwrap()
        .clone();
    assert_eq!(shading.name(b"ColorSpace"), Some(&b"DeviceCMYK"[..]));
    let pieces = shading.get(b"Function").and_then(Object::as_dict).and_then(|f| f.get(b"Functions")).and_then(Object::as_array).unwrap().clone();
    let end = |piece: &Object, key: &[u8]| -> Vec<f64> {
        piece.as_dict().unwrap().get(key).unwrap().as_array().unwrap().iter().filter_map(Object::as_f64).collect()
    };
    assert!(
        close(&end(&pieces[0], b"C0"), &gold(0.0)) && close(&end(pieces.last().unwrap(), b"C1"), &gold(1.0)),
        "the ends are the spot's ends in CMYK"
    );
}

#[test]
fn an_ink_named_with_hex_escapes_is_the_one_with_those_bytes() {
    let res = "/ColorSpace << /G [/Separation /PANTONE#20871#20C /DeviceCMYK 5 0 R] >>";
    let mut doc = page(res, "/G cs 1 scn 0 0 10 10 re f", &[]);
    let missed = convert_spots_to_process(&mut doc, &["PANTONE#20872#20C".into()]).unwrap();
    assert!(missed.converted() == 0 && missed.notes.iter().any(|n| n.contains("872")), "an ink no space names is reported: {missed:?}");
    let done = convert_spots_to_process(&mut doc, &["PANTONE#20871#20C".into()]).unwrap();
    assert!(done.paints > 0, "{done:?}");
    assert!(convert_spots_to_process(&mut doc, &["Black".into()]).is_err(), "a process ink is not a spot");
}
