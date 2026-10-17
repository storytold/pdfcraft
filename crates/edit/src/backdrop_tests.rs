use std::sync::Arc;

use pdfcraft_cos::{Document, ObjRef, Object, SaveOptions, Stream, write_incremental};

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
fn page(pieces: &[&str]) -> Document {
    let refs: Vec<String> = (0..pieces.len()).map(|i| format!("{} 0 R", i + 5)).collect();
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents [{}] /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> /XObject << /Im0 4 0 R >> /ExtGState << /Half << /ca 0.5 >> /Full << /ca 1 /CA 1 >> >> >> >>",
            refs.join(" ")
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

#[test]
fn a_backdrop_copy_edited_since_is_kept_when_the_background_goes() {
    let mut doc = page(&[&format!("1 g 0 0 300 200 re f {TEXT}")]);
    add_background(&mut doc, &[0], &red(), false).unwrap();
    // Something else rewrites the copy (as Edit text does), keeping its dictionary.
    let p = &pdfcraft_model::pages(&doc)[0];
    let Some(Object::Array(list)) = p.dict.get(b"Contents").cloned() else { panic!("contents") };
    let Object::Ref(copy) = list[1] else { panic!("a reference") };
    let Object::Stream(s) = (*doc.get(copy)).clone() else { panic!("a stream") };
    let edited = "1 g 0 0 300 200 re n BT /F1 24 Tf 40 90 Td (Edited) Tj ET";
    doc.set(copy, Object::Stream(Stream::flate(s.dict.clone(), edited.as_bytes())));
    remove_marks(&mut doc, &[0], MarkKind::Background).unwrap();
    assert_eq!(streams(&doc), [edited.to_string()], "the edit survives");
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
