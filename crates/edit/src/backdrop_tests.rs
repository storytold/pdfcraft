use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full, write_incremental};

use super::*;
use crate::{Background, MarkKind, add_background, remove_marks};

const TEXT: &str = "BT /F1 24 Tf 40 90 Td (Hello) Tj ET";

/// A document from its objects (object 1 is the catalog, 2 the page tree).
fn build(objs: &[String]) -> Document {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

fn stream(dict: &str, data: &str) -> String {
    format!("<< {dict} /Length {} >>\nstream\n{data}\nendstream", data.len())
}

/// One 300 × 200 page drawing `pieces` (objects 5…), with a font, a page-sized image `/Im0`
/// (object 4) and two graphics states, `/Half` (ca 0.5) and `/Full` (ca 1).
/// The graphics states [`page`] offers. `INVERT` is a transfer function that maps 1 to 0, so
/// `1 g` through it paints black (#2815 review).
const GRAPHICS_STATES: &str = concat!(
    "/Half << /ca 0.5 >> /Full << /ca 1 /CA 1 >> /LineOnly << /LW 2 >> ",
    "/Masked << /SMask << /S /Luminosity /G 4 0 R >> >> /Unmasked << /SMask /None >> ",
    "/Multiply << /BM /Multiply >> /NormalBlend << /BM /Normal >> ",
    "/Invert << /TR << /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >> >> ",
    "/Invert2 << /TR2 << /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >> >> ",
    "/IdentityTR << /TR /Identity >> /DefaultTR2 << /TR2 /Default >> ",
    "/InvertButTR2Default << /TR << /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >> /TR2 /Default >> ",
    "/IdentityButTR2Invert << /TR /Identity /TR2 << /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >> >>",
);

fn page(pieces: &[&str]) -> Document {
    let refs: Vec<String> = (0..pieces.len()).map(|i| format!("{} 0 R", i + 5)).collect();
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents [{}] /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> /XObject << /Im0 4 0 R >> /ExtGState << {GS} >> >> >>",
            refs.join(" "),
            GS = GRAPHICS_STATES
        ),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", "\u{80}"),
    ];
    objs.extend(pieces.iter().map(|p| stream("", p)));
    build(&objs)
}

/// The decoded content of a page, stream by stream.
fn streams(doc: &Document) -> Vec<String> {
    let p = &pdfcraft_model::pages(doc)[0];
    let list = match p.dict.get(b"Contents").map(|c| doc.resolve(c)).as_deref() {
        Some(Object::Array(a)) => a.clone(),
        _ => vec![],
    };
    list.iter()
        .map(|o| match &*doc.resolve(o) {
            Object::Stream(s) => String::from_utf8_lossy(&s.decoded().unwrap()).into_owned(),
            _ => String::new(),
        })
        .collect()
}

fn reopen(doc: &Document) -> Document {
    let bytes = write_incremental(doc, &SaveOptions::default()).unwrap();
    Document::open(Arc::new(bytes)).unwrap()
}

fn red() -> Background {
    Background { color: [1.0, 0.0, 0.0], ..Background::default() }
}

#[test]
fn a_white_fill_backdrop_is_replaced_and_put_back_when_the_background_goes() {
    let content = format!("q 1 1 1 rg 0 0 300 200 re f Q {TEXT}");
    let mut doc = page(&[&content]);
    assert_eq!(page_backdrop(&doc, 0).unwrap(), Some(Backdrop::Fill));
    let original = streams(&doc);
    add_background(&mut doc, &[0], &red(), false).unwrap();
    let s = streams(&reopen(&doc));
    assert_eq!(s.len(), 2, "{s:?}");
    assert!(s[0].contains("/PCMark /Background"));
    assert_eq!(s[1], format!("q 1 1 1 rg 0 0 300 200 re n Q {TEXT}"), "only the fill operator changed");
    assert_eq!(page_backdrop(&doc, 0).unwrap(), None, "nothing hides the background now");
    // The original stream object is untouched; the page points at a copy.
    let Object::Stream(orig) = &*doc.get(ObjRef { num: 5, generation: 0 }) else { panic!("object 5 is the content") };
    assert_eq!(String::from_utf8_lossy(&orig.decoded().unwrap()), content);
    assert_eq!(remove_marks(&mut doc, &[0], MarkKind::Background).unwrap(), 1);
    assert_eq!(streams(&reopen(&doc)), original, "exactly the original content");
    assert_eq!(page_backdrop(&doc, 0).unwrap(), Some(Backdrop::Fill));
}

#[test]
fn updating_the_background_replaces_the_backdrop_once_and_removing_still_restores() {
    let mut doc = page(&[&format!("1 1 1 rg 0 0 300 200 re f 0 g {TEXT}")]);
    let original = streams(&doc);
    add_background(&mut doc, &[0], &red(), false).unwrap();
    add_background(&mut doc, &[0], &Background { color: [0.0, 0.0, 1.0], ..Background::default() }, true).unwrap();
    let s = streams(&doc);
    assert_eq!(s.len(), 2, "one background, one copy: {s:?}");
    assert!(s[1].contains("re n"));
    remove_marks(&mut doc, &[0], MarkKind::Background).unwrap();
    assert_eq!(streams(&doc), original);
}

#[test]
fn a_backdrop_split_across_content_streams_is_replaced_in_the_piece_holding_the_operator() {
    let mut doc = page(&["1 1 1 rg 0 0 300 200 re", &format!("f* {TEXT}")]);
    let original = streams(&doc);
    assert_eq!(page_backdrop(&doc, 0).unwrap(), Some(Backdrop::Fill));
    add_background(&mut doc, &[0], &red(), false).unwrap();
    let s = streams(&doc);
    assert_eq!(s[1..], ["1 1 1 rg 0 0 300 200 re".to_string(), format!("n {TEXT}")], "{s:?}");
    remove_marks(&mut doc, &[0], MarkKind::Background).unwrap();
    assert_eq!(streams(&doc), original);
}

#[test]
fn backdrops_writers_paint_are_recognised() {
    for content in [
        // Browsers: a flipped, scaled coordinate system and a page clip first.
        "q 0.5 0 0 -0.5 0 200 cm 0 0 600 400 re W n 1 1 1 rg 0 0 600 400 re f Q",
        // Larger than the page, in CMYK, through an opaque graphics state.
        "/Full gs 0 0 0 0 k -10 -10 320 220 re f",
        "/DeviceRGB cs 1 1 1 sc 0 0 300 200 re F",
        "0.99 g 0 0 300 200 re f",
        // Turned a quarter: still a rectangle.
        "0 1 -1 0 300 0 cm 1 g 0 0 200 300 re f",
    ] {
        let doc = page(&[&format!("{content} 0 g {TEXT}")]);
        assert_eq!(page_backdrop(&doc, 0).unwrap(), Some(Backdrop::Fill), "{content}");
    }
}

#[test]
fn content_that_is_not_a_page_backdrop_is_left_alone() {
    for content in [
        // Not the whole page.
        "1 g 0 0 300 150 re f",
        "q 0 0 100 100 re W n 1 g 0 0 300 200 re f Q",
        // Not white, or not opaque.
        "0.5 g 0 0 300 200 re f",
        "1 0 0 rg 0 0 300 200 re f",
        "0 0 0 0.5 k 0 0 300 200 re f",
        "/Half gs 1 g 0 0 300 200 re f",
        "/Missing gs 1 g 0 0 300 200 re f",
        "/Pattern cs /P0 scn 0 0 300 200 re f",
        // The initial colour is black.
        "0 0 300 200 re f",
        // Something else is painted first.
        "BT /F1 9 Tf (x) Tj ET 1 g 0 0 300 200 re f",
        "0 g 0 0 1 1 re f 1 g 0 0 300 200 re f",
        "1 g 0 0 300 200 re S 1 g 0 0 300 200 re f",
        // Not a single rectangle: a hole, a turned square, a fill-and-stroke.
        "1 g 0 0 300 200 re 50 50 10 10 re f*",
        "1 g 0 0 m 300 0 l 300 200 l 0 200 l h f",
        "0.7 0.7 -0.7 0.7 150 -100 cm 1 g 0 0 400 400 re f",
        "1 g 0 0 300 200 re B",
        // Malformed.
        "Q 1 g 0 0 300 200 re f",
        "1 g 0 0 300 re f",
        "1 g 0 0 1e400 200 re f",
    ] {
        let mut doc = page(&[&format!("{content} {TEXT}")]);
        assert_eq!(page_backdrop(&doc, 0).unwrap(), None, "{content}");
        let before = streams(&doc);
        add_background(&mut doc, &[0], &red(), false).unwrap();
        assert_eq!(streams(&doc)[1..], before[..], "{content}: the page's content is unchanged");
    }
}

#[test]
fn a_scanned_page_is_reported_and_its_picture_kept() {
    let mut doc = page(&[&format!("q 300 0 0 200 0 0 cm /Im0 Do Q {TEXT}")]);
    assert_eq!(page_backdrop(&doc, 0).unwrap(), Some(Backdrop::Image));
    let before = streams(&doc);
    add_background(&mut doc, &[0], &red(), false).unwrap();
    assert_eq!(streams(&doc)[1..], before[..]);
    // A small picture is content, not a backdrop.
    let doc = page(&["q 30 0 0 20 0 0 cm /Im0 Do Q"]);
    assert_eq!(page_backdrop(&doc, 0).unwrap(), None);
}

#[test]
fn replacing_can_be_turned_off() {
    let mut doc = page(&[&format!("1 g 0 0 300 200 re f {TEXT}")]);
    let before = streams(&doc);
    add_background(&mut doc, &[0], &Background { replace_backdrop: false, ..red() }, false).unwrap();
    assert_eq!(streams(&doc)[1..], before[..]);
}

/// Rewrite the page's backdrop copy (the stream after the background) as another tool would:
/// Edit text and redaction keep its dictionary and change its bytes.
fn rewrite_copy(doc: &mut Document, bytes: &str) -> ObjRef {
    let p = &pdfcraft_model::pages(doc)[0];
    let Some(Object::Array(list)) = p.dict.get(b"Contents").cloned() else { panic!("contents") };
    let Object::Ref(copy) = list[1] else { panic!("a reference") };
    let Object::Stream(s) = (*doc.get(copy)).clone() else { panic!("a stream") };
    assert!(s.dict.get(b"PCBackdrop").is_some(), "the copy");
    doc.set(copy, Object::Stream(Stream::flate(s.dict.clone(), bytes.as_bytes())));
    copy
}

#[test]
fn an_edit_after_the_fill_is_kept_and_the_backdrop_still_comes_back() {
    let mut doc = page(&[&format!("1 g 0 0 300 200 re f {TEXT}")]);
    add_background(&mut doc, &[0], &red(), false).unwrap();
    rewrite_copy(&mut doc, "1 g 0 0 300 200 re n BT /F1 24 Tf 40 90 Td (Edited) Tj ET");
    remove_marks(&mut doc, &[0], MarkKind::Background).unwrap();
    assert_eq!(streams(&doc), ["1 g 0 0 300 200 re f BT /F1 24 Tf 40 90 Td (Edited) Tj ET"], "the edit survives, under the fill");
    assert_eq!(page_backdrop(&doc, 0).unwrap(), Some(Backdrop::Fill));
    assert_eq!(backdrops_left_off(&doc), Vec::<usize>::new());
}

#[test]
fn a_change_before_the_fill_leaves_it_off_and_names_the_page() {
    for edited in [
        // A different colour before the fill: the hash of what comes before it no longer matches.
        "0.99 g 0 0 300 200 re n BT /F1 24 Tf 40 90 Td (Edited) Tj ET",
        // Something painted before it.
        "BT /F1 9 Tf (x) Tj ET 1 g 0 0 300 200 re n",
        // The fill is gone.
        "BT /F1 24 Tf 40 90 Td (Edited) Tj ET",
    ] {
        let mut doc = page(&[&format!("1 g 0 0 300 200 re f {TEXT}")]);
        add_background(&mut doc, &[0], &red(), false).unwrap();
        assert_eq!(backdrops_left_off(&doc), Vec::<usize>::new(), "a background is over it");
        rewrite_copy(&mut doc, edited);
        remove_marks(&mut doc, &[0], MarkKind::Background).unwrap();
        assert_eq!(streams(&doc), [edited.to_string()], "{edited}: the copy stays as it is");
        assert_eq!(backdrops_left_off(&doc), [0], "{edited}");
    }
}

#[test]
fn a_fill_that_would_cover_content_painted_before_it_stays_off() {
    let mut doc = page(&[&format!("1 g 0 0 300 200 re f {TEXT}")]);
    add_background(&mut doc, &[0], &red(), false).unwrap();
    // A tool puts a stream of its own before the copy, untagged: turning the fill back on would
    // now paint white over that text.
    let over = doc.add(Object::Stream(Stream::flate(pdfcraft_cos::Dict::new(), b"BT /F1 9 Tf 10 10 Td (Over) Tj ET")));
    let p = pdfcraft_model::pages(&doc)[0].clone();
    let Some(Object::Array(mut list)) = p.dict.get(b"Contents").cloned() else { panic!("contents") };
    list.insert(1, Object::Ref(over));
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), Object::Array(list))).unwrap();
    remove_marks(&mut doc, &[0], MarkKind::Background).unwrap();
    let s = streams(&doc);
    assert_eq!(s.len(), 2, "{s:?}");
    assert!(s[1].contains("re n") && !s[1].contains("re f"), "nothing white goes over the text: {s:?}");
    assert_eq!(backdrops_left_off(&doc), [0]);
}

#[test]
fn the_tag_holds_no_reference_and_a_full_save_keeps_no_second_copy_of_the_page() {
    let mut doc = page(&[&format!("1 g 0 0 300 200 re f {TEXT}")]);
    add_background(&mut doc, &[0], &red(), false).unwrap();
    let saved = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
    // Every stream in the file that holds the page's text: only the copy the page draws. A
    // reference from the tag to the original stream kept a second, which a redaction or Remove
    // Hidden Information on the copy left in the file.
    let holding: Vec<(ObjRef, Dict)> = saved
        .scan_objects()
        .filter_map(|(r, o)| match &*o {
            Object::Stream(s) if String::from_utf8_lossy(&s.decoded().unwrap_or_default()).contains("(Hello)") => Some((r, s.dict.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(holding.len(), 1, "{holding:?}");
    let Some(Object::Dict(note)) = holding[0].1.get(b"PCBackdrop").map(|n| (*saved.resolve(n)).clone()) else { panic!("the tag") };
    fn refs(o: &Object) -> bool {
        match o {
            Object::Ref(_) => true,
            Object::Array(a) => a.iter().any(refs),
            Object::Dict(d) => d.iter().any(|(_, v)| refs(v)),
            _ => false,
        }
    }
    assert!(!note.iter().any(|(_, v)| refs(v)), "{note:?}");
    assert_eq!(note.get(b"Op").and_then(Object::as_string).map(|s| s.bytes.clone()), Some(b"f".to_vec()));
    assert_eq!(note.get(b"Before").and_then(Object::as_string).map(|s| s.bytes.len()), Some(32), "a SHA-256");
    // The original is gone, so Remove writes the fill back into a stream of its own.
    let mut saved = saved;
    remove_marks(&mut saved, &[0], MarkKind::Background).unwrap();
    assert_eq!(streams(&saved), [format!("1 g 0 0 300 200 re f {TEXT}")]);
    assert_eq!(page_backdrop(&saved, 0).unwrap(), Some(Backdrop::Fill));
}

#[test]
fn a_copy_written_out_again_with_other_spacing_still_comes_back() {
    // Redaction writes the whole stream out again: one operator a line, numbers its own way.
    let mut doc = page(&[&format!("1 g 0 0 300 200 re f {TEXT}")]);
    add_background(&mut doc, &[0], &red(), false).unwrap();
    rewrite_copy(&mut doc, "1.0 g\n0 0 300.000 200 re\nn\nBT\n/F1 24 Tf\n40 90 Td\n[-6505] TJ\nET");
    remove_marks(&mut doc, &[0], MarkKind::Background).unwrap();
    assert_eq!(streams(&doc), ["1.0 g\n0 0 300.000 200 re\nf\nBT\n/F1 24 Tf\n40 90 Td\n[-6505] TJ\nET"], "the fill is back, the rest as written");
    assert_eq!(backdrops_left_off(&doc), Vec::<usize>::new());
}

#[test]
fn a_tag_that_does_not_add_up_is_left_alone() {
    let content = format!("1 g 0 0 300 200 re f {TEXT}");
    let copied = format!("1 g 0 0 300 200 re n {TEXT}");
    let good = ops_hash(&pdfcraft_content::parse(b"1 g 0 0 300 200 re").ops);
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    for tag in [
        // Not a fill operator, a wrong hash, an index that isn't the `n` or runs past the end.
        format!("<< /Index 2 /Op (B) /Before <{}> >>", hex(&good)),
        format!("<< /Index 2 /Op (f) /Before <{}> >>", hex(&[0; 32])),
        format!("<< /Index 1 /Op (f) /Before <{}> >>", hex(&good)),
        format!("<< /Index 99999 /Op (f) /Before <{}> >>", hex(&good)),
        format!("<< /Index -1 /Op (f) /Before <{}> >>", hex(&good)),
        // Missing entries, wrong types, and the reference earlier test builds wrote.
        "<< /Index 2 /Op (f) >>".to_string(),
        "<< /Index (2) /Op /f /Before 3 >>".to_string(),
        "<< /Original 5 0 R /At 19 /Len 1 >>".to_string(),
        "(not a dictionary)".to_string(),
    ] {
        let mut doc = build(&[
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 200] >>".into(),
            "<< /Type /Page /Parent 2 0 R /Contents [4 0 R] >>".into(),
            stream(&format!("/PCBackdrop {tag}"), &copied),
            stream("", &content),
        ]);
        remove_marks(&mut doc, &[0], MarkKind::Background).unwrap();
        assert_eq!(streams(&doc), std::slice::from_ref(&copied), "{tag}");
        assert_eq!(backdrops_left_off(&doc), [0], "{tag}");
    }
    // The same page with an honest tag comes back.
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents [4 0 R] >>".into(),
        stream(&format!("/PCBackdrop << /Index 2 /Op (f) /Before <{}> >>", hex(&good)), &copied),
    ]);
    remove_marks(&mut doc, &[0], MarkKind::Background).unwrap();
    assert_eq!(streams(&doc), [content]);
}

#[test]
fn operators_hash_by_value_not_by_spelling() {
    let h = |s: &[u8]| ops_hash(&pdfcraft_content::parse(s).ops);
    assert_eq!(h(b"1 g 0 0 300 200 re"), h(b"1.0 g\n0  0 300.00000 200 re"));
    assert_eq!(h(b"0 g"), h(b"-0.0 g"));
    assert_ne!(h(b"1 g 0 0 300 200 re"), h(b"0.99 g 0 0 300 200 re"));
    assert_ne!(h(b"(ab) Tj"), h(b"(a) (b) Tj"), "operand boundaries count");
    assert_ne!(h(b"/a /b BDC"), h(b"/ab BDC"));
    assert_ne!(h(b""), h(b"q"));
    // Nesting is bounded: deep arrays hash without running out of stack.
    let deep = format!("{}{} Tj", "[".repeat(1000), "]".repeat(1000));
    let _ = h(deep.as_bytes());
}

#[test]
fn a_content_stream_shared_by_two_pages_is_copied_for_the_one_page() {
    let content = format!("1 g 0 0 300 200 re f {TEXT}");
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>".into(),
        stream("", &content),
    ]);
    add_background(&mut doc, &[0], &red(), false).unwrap();
    assert_eq!(page_backdrop(&doc, 0).unwrap(), None);
    assert_eq!(page_backdrop(&doc, 1).unwrap(), Some(Backdrop::Fill), "the other page keeps its backdrop");
}

#[test]
fn pages_without_content_or_with_odd_contents_have_no_backdrop() {
    let doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents [5 0 R 99 0 R] >>".into(),
        "<< /Not /AStream >>".into(),
    ]);
    assert_eq!(page_backdrop(&doc, 0).unwrap(), None);
    assert_eq!(page_backdrop(&doc, 1).unwrap(), None);
    assert!(matches!(page_backdrop(&doc, 7), Err(EditError::NoSuchPage(7))));
}

/// `pages` 300 × 200 pages in a flat tree; even pages paint a white backdrop, odd ones don't.
fn many_pages(pages: usize) -> Document {
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", i + 5)).collect();
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{}] /Count {pages} /MediaBox [0 0 300 200] >>", kids.join(" ")),
        stream("", &format!("1 g 0 0 300 200 re f 0 g {TEXT}")),
        stream("", TEXT),
    ];
    objs.extend((0..pages).map(|i| format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R >>", 3 + i % 2)));
    build(&objs)
}

#[test]
fn checking_many_pages_walks_the_page_tree_once() {
    // The Add Background dialog checks every page of a long document. Looking each page up on
    // its own walked the whole tree per page, so checking them all grew with the square of the
    // page count (minutes at 20,000 pages); one walk keeps it in proportion.
    const PAGES: usize = 4000;
    let doc = many_pages(PAGES);
    let all: Vec<usize> = (0..PAGES).collect();
    let start = std::time::Instant::now();
    let found = page_backdrops(&doc, &all);
    let took = start.elapsed();
    assert_eq!(found.len(), PAGES);
    for (i, f) in found.iter().enumerate() {
        assert_eq!(*f, (i % 2 == 0).then_some(Backdrop::Fill), "page {i}");
    }
    assert!(took < std::time::Duration::from_secs(5), "checking {PAGES} pages took {took:?}");
}

#[test]
fn a_batch_answers_in_the_order_asked_and_a_missing_page_hides_nothing() {
    let doc = many_pages(5);
    assert_eq!(page_backdrops(&doc, &[3, 0, 0]), [None, Some(Backdrop::Fill), Some(Backdrop::Fill)]);
    assert_eq!(page_backdrops(&doc, &[]), []);
    // The viewer can count more pages than the page tree holds: the others still answer.
    assert_eq!(page_backdrops(&doc, &[2, 5, usize::MAX]), [Some(Backdrop::Fill), None, None]);
    assert_eq!(page_backdrop(&doc, 4).unwrap(), Some(Backdrop::Fill));
    assert!(matches!(page_backdrop(&doc, 5), Err(EditError::NoSuchPage(5))));
}

#[test]
fn a_background_over_many_pages_replaces_each_backdrop_and_removing_restores_them() {
    // Pages share their content streams, so each replaced page gets its own copy and the rest
    // are checked against the document as it is after the earlier ones.
    let mut doc = many_pages(4);
    let all = [0, 1, 2, 3];
    let before: Vec<Object> = pdfcraft_model::pages(&doc).iter().filter_map(|p| p.dict.get(b"Contents").cloned()).collect();
    add_background(&mut doc, &all, &red(), true).unwrap();
    assert_eq!(page_backdrops(&doc, &all), [None; 4]);
    // Update: the backdrops are gone already, so nothing is copied again.
    add_background(&mut doc, &all, &red(), true).unwrap();
    assert_eq!(page_backdrops(&doc, &all), [None; 4]);
    remove_marks(&mut doc, &all, MarkKind::Background).unwrap();
    let after: Vec<Object> = pdfcraft_model::pages(&doc).iter().filter_map(|p| p.dict.get(b"Contents").cloned()).collect();
    let refs = |list: &[Object]| -> Vec<Vec<Object>> {
        list.iter()
            .map(|c| match &*doc.resolve(c) {
                Object::Array(a) => a.clone(),
                _ => vec![c.clone()],
            })
            .collect()
    };
    assert_eq!(refs(&after), refs(&before), "every page points at its original stream again");
    assert_eq!(page_backdrops(&doc, &all), [Some(Backdrop::Fill), None, Some(Backdrop::Fill), None]);
}

#[test]
fn a_content_stream_whose_bytes_live_in_an_external_file_is_left_alone() {
    let objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents [4 0 R] >>".into(),
        stream("/F (content.bin)", &format!("1 g 0 0 300 200 re f {TEXT}")),
    ];
    let mut doc = build(&objs);
    assert_eq!(page_backdrop(&doc, 0).unwrap(), None, "the inline bytes are not what the page draws");
    let before = streams(&doc);
    add_background(&mut doc, &[0], &red(), false).unwrap();
    assert_eq!(streams(&doc)[1..], before[..]);
}

#[test]
fn the_backdrop_copy_keeps_the_original_dictionary_less_its_old_encoding() {
    let objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents [4 0 R] >>".into(),
        stream("/Vendor (keep me) /DL 9999", &format!("1 g 0 0 300 200 re f {TEXT}")),
    ];
    let mut doc = build(&objs);
    add_background(&mut doc, &[0], &red(), false).unwrap();
    let doc = reopen(&doc);
    let p = &pdfcraft_model::pages(&doc)[0];
    let Some(Object::Array(list)) = p.dict.get(b"Contents").map(|c| (*doc.resolve(c)).clone()) else { panic!("contents") };
    let Object::Stream(copy) = &*doc.resolve(&list[1]) else { panic!("a stream") };
    assert!(copy.dict.get(b"Vendor").is_some(), "unknown entries survive");
    assert!(copy.dict.get(b"DL").is_none(), "the old decoded length no longer holds");
    assert!(copy.dict.get(b"PCBackdrop").is_some());
    assert_eq!(String::from_utf8_lossy(&copy.decoded().unwrap()), format!("1 g 0 0 300 200 re n {TEXT}"));
}

#[test]
fn a_stream_listed_twice_is_left_untouched_as_ambiguous() {
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents [4 0 R 4 0 R] >>".into(),
        stream("", &format!("q 1 g 0 0 300 200 re f Q {TEXT}")),
    ]);
    let original = streams(&doc);
    add_background(&mut doc, &[0], &red(), false).unwrap();
    let s = streams(&doc);
    assert_eq!(s.len(), 3, "{s:?}");
    assert_eq!(s[1..], original[..], "repeated executions are not qualified independently");
    remove_marks(&mut doc, &[0], MarkKind::Background).unwrap();
    assert_eq!(streams(&doc), original);
}

/// #2815 review: a `gs` sets only the keys its dictionary has, so a translucent, masked,
/// blended or transferred fill stays that way through a later `gs` that leaves the key out,
/// and a transfer function that isn't the identity can turn `1 g` into any colour.
#[test]
fn graphics_state_a_later_gs_leaves_out_still_keeps_the_fill() {
    for state in [
        "/Half gs /LineOnly gs",
        "/Masked gs /LineOnly gs",
        "/Multiply gs /LineOnly gs",
        "/Invert gs /LineOnly gs",
        "/Invert2 gs /LineOnly gs",
        "/Invert gs",
        "/Invert2 gs",
        "/IdentityButTR2Invert gs",
        "/Invert gs q /LineOnly gs Q",
    ] {
        let mut doc = page(&[&format!("{state} 1 g 0 0 300 200 re f {TEXT}")]);
        assert_eq!(page_backdrop(&doc, 0).unwrap(), None, "{state}");
        let before = streams(&doc);
        add_background(&mut doc, &[0], &red(), false).unwrap();
        assert_eq!(streams(&doc)[1..], before[..], "{state}: the page's content is unchanged");
    }
}

/// The other side of the same rule: a later `gs` that does set the key back, `TR2` naming the
/// default over a `TR` function, and `Q` restoring what `q` saved still leave a plain white fill.
#[test]
fn graphics_state_set_back_before_the_fill_still_finds_the_backdrop() {
    for state in [
        "/Half gs /Full gs",
        "/Masked gs /Unmasked gs",
        "/Multiply gs /NormalBlend gs",
        "/Invert gs /IdentityTR gs",
        "/Invert2 gs /DefaultTR2 gs",
        "/InvertButTR2Default gs",
        "/LineOnly gs",
        "q /Half gs Q",
        "q /Invert gs Q",
    ] {
        let doc = page(&[&format!("{state} 1 g 0 0 300 200 re f {TEXT}")]);
        assert_eq!(page_backdrop(&doc, 0).unwrap(), Some(Backdrop::Fill), "{state}");
    }
}

#[test]
fn an_undecodable_stream_before_a_white_fill_keeps_the_backdrop_unknown() {
    let objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents [4 0 R 5 0 R] >>".into(),
        stream("/Filter /FlateDecode", "not a deflate stream"),
        stream("", &format!("1 g 0 0 300 200 re f {TEXT}")),
    ];
    let doc = build(&objs);
    assert_eq!(page_backdrop(&doc, 0).unwrap(), None, "unknown earlier paint must not be treated as an empty stream");
}

#[test]
fn shared_fill_used_as_later_artwork_is_not_rewritten() {
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents [4 0 R 5 0 R 6 0 R 5 0 R 7 0 R] >>".into(),
        stream("", "1 g"),
        stream("", "0 0 300 200 re f"),
        stream("", "q 0 g 0.25 0 0 0.25 20 20 cm"),
        stream("", "Q"),
    ]);
    assert_eq!(page_backdrop(&doc, 0).unwrap(), None);
    let original = streams(&doc);
    add_background(&mut doc, &[0], &red(), false).unwrap();
    assert_eq!(streams(&doc)[1..], original[..], "the later black artwork keeps its fill");
    let mut saved = reopen(&doc);
    assert_eq!(streams(&saved)[1..], original[..]);
    remove_marks(&mut saved, &[0], MarkKind::Background).unwrap();
    assert_eq!(streams(&saved), original);
    assert_eq!(streams(&reopen(&saved)), original);
}

#[test]
fn compressed_backdrop_content_is_refused_before_expanding_past_the_scan_budget() {
    let mut doc = page(&["1 g 0 0 300 200 re f"]);
    let mut bytes = b"1 g 0 0 300 200 re f\n".to_vec();
    bytes.resize(2 * 1024 * 1024, b' ');
    let source = doc.add(Object::Stream(Stream::flate(Dict::new(), &bytes)));
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Ref(source))).unwrap();
    assert_eq!(page_backdrop(&doc, 0).unwrap(), None);
    add_background(&mut doc, &[0], &red(), false).unwrap();
    let Object::Stream(unchanged) = &*doc.get(source) else { panic!("stream") };
    assert_eq!(unchanged.decoded_strict().unwrap(), bytes, "refusal preserves the source");
}
