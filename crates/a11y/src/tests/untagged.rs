//! Marking a tagged page's untagged content as artifacts.

use pdfcraft_cos::ObjRef;

use super::*;

/// A tagged one-page document: element 5 claims MCID 0 on page 4, the page content is
/// `contents` (objects 8 on), its `/XObject` resources are `xobjects`, and `extra` follows the
/// content streams.
fn tagged(contents: &[&str], xobjects: &str, extra: &[String]) -> Document {
    let refs: Vec<String> = (0..contents.len()).map(|i| format!("{} 0 R", i + 8)).collect();
    let mut bodies: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 3 0 R /MarkInfo << /Marked true >> >>".into(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".into(),
        "<< /Type /StructTreeRoot /K 5 0 R /ParentTree 6 0 R >>".into(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /StructParents 0 /Contents [{}] /Resources << /Font << /F1 7 0 R >> /XObject << {xobjects} >> >> >>",
            refs.join(" ")
        ),
        "<< /Type /StructElem /S /P /P 3 0 R /Pg 4 0 R /K 0 >>".into(),
        "<< /Nums [0 [5 0 R]] >>".into(),
        FONT.into(),
    ];
    bodies.extend(contents.iter().map(|c| stream(c)));
    bodies.extend(extra.iter().cloned());
    pdf(&bodies, "")
}

fn form(data: &str, extra: &str) -> String {
    format!("<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] {extra} /Length {} >>\nstream\n{data}\nendstream", data.len())
}

/// The first page's content streams: their objects and decoded text.
fn contents(doc: &Document) -> Vec<(Option<ObjRef>, String)> {
    let page = pdfcraft_model::pages(doc).remove(0);
    let entries = match page.dict.get(b"Contents").cloned() {
        Some(Object::Array(a)) => a,
        Some(o) => vec![o],
        None => Vec::new(),
    };
    entries
        .iter()
        .map(|e| {
            let s = doc.resolve(e);
            let Object::Stream(s) = &*s else { panic!("a content stream") };
            (e.as_ref(), String::from_utf8(s.decoded_strict().unwrap()).unwrap())
        })
        .collect()
}

fn tagged_content(doc: &Document) -> Status {
    status(&check(doc, &Options { rules: [Rule::TaggedContent].into(), pages: None }), Rule::TaggedContent)
}

/// Whether `q`/`Q`, `BT`/`ET`, `BX`/`EX` and marked content nest properly in `text`.
fn nests(text: &str) -> bool {
    let mut open: Vec<&[u8]> = Vec::new();
    for op in pdfcraft_content::ops(text.as_bytes()) {
        let want: &[u8] = match op.op.as_slice() {
            b"q" => {
                open.push(b"q");
                continue;
            }
            b"BT" => {
                open.push(b"BT");
                continue;
            }
            b"BX" => {
                open.push(b"BX");
                continue;
            }
            b"BMC" | b"BDC" => {
                open.push(b"BDC");
                continue;
            }
            b"Q" => b"q",
            b"ET" => b"BT",
            b"EX" => b"BX",
            b"EMC" => b"BDC",
            _ => continue,
        };
        if open.pop() != Some(want) {
            return false;
        }
    }
    open.is_empty()
}

const CLAIMED: &str = "/P <</MCID 0>> BDC BT /F1 12 Tf 20 150 Td (Tagged) Tj ET EMC";

#[test]
fn untagged_content_on_a_tagged_page_becomes_an_artifact() {
    let mut doc = tagged(&[&format!("{CLAIMED} q 0 0 10 10 re f Q BT /F1 12 Tf 20 100 Td (Loose) Tj ET")], "", &[]);
    assert_eq!(tagged_content(&doc), Status::Failed);
    let planned = plan_untagged_artifacts(&doc, &[]).unwrap();
    let before = contents(&doc);
    let done = mark_untagged_artifacts(&mut doc, &[]).unwrap();
    assert_eq!(planned, done);
    assert_eq!(done.marked.iter().map(|m| m.page).collect::<Vec<_>>(), [0]);
    assert!(done.items >= 2 && done.notes.iter().any(|n| n.contains("assistive technology")), "{done:?}");
    assert_eq!(tagged_content(&doc), Status::Passed);
    let after = contents(&doc);
    assert_ne!(before, after);
    assert!(after.iter().all(|(_, t)| nests(t) && t.contains("/P <</MCID 0>> BDC")), "{after:?}");
}

#[test]
fn a_page_without_tags_is_skipped() {
    let mut doc = tagged(&["BT /F1 12 Tf 20 100 Td (Loose) Tj ET"], "", &[]);
    doc.update_dict(ObjRef::new(4, 0), |p| {
        p.remove(b"StructParents");
    })
    .unwrap();
    doc.update_dict(ObjRef::new(5, 0), |e| e.set(b"K".to_vec(), Object::Array(Vec::new()))).unwrap();
    let before = contents(&doc);
    let done = mark_untagged_artifacts(&mut doc, &[0]).unwrap();
    assert!(done.marked.is_empty() && done.skipped.iter().any(|s| s.page == 0), "{done:?}");
    assert_eq!(contents(&doc), before);
}

#[test]
fn marked_content_nothing_claims_is_left_and_disclosed() {
    let page = format!("{CLAIMED} BT /F1 12 Tf (a) Tj ET /Span <</MCID 5>> BDC BT /F1 12 Tf (b) Tj ET EMC BT /F1 12 Tf (c) Tj ET");
    let mut doc = tagged(&[&page], "", &[]);
    let done = mark_untagged_artifacts(&mut doc, &[]).unwrap();
    // The loose text objects either side are wrapped apart; the orphan sequence is not enclosed.
    assert_eq!((done.sequences, done.items), (2, 2), "{done:?}");
    assert!(done.notes.iter().any(|n| n.contains("doesn't claim")), "{done:?}");
    let (_, text) = &contents(&doc)[0];
    assert!(text.contains("/Span <</MCID 5>> BDC") && nests(text), "{text}");
}

#[test]
fn a_form_holding_tagged_and_untagged_content_is_left_and_disclosed() {
    let mixed = form("/P <</MCID 1>> BDC 0 0 5 5 re f EMC 5 5 5 5 re f", "");
    let mut doc = tagged(&[&format!("{CLAIMED} /Fm Do 0 0 10 10 re f")], "/Fm 9 0 R", &[mixed]);
    let done = mark_untagged_artifacts(&mut doc, &[]).unwrap();
    // Only the page's own rectangle: the form is not enclosed.
    assert_eq!(done.items, 1, "{done:?}");
    assert!(done.notes.iter().any(|n| n.contains("tagged and untagged")), "{done:?}");
    let (_, text) = &contents(&doc)[0];
    assert!(text.find("/Artifact BMC") > text.find("/Fm Do"), "{text}");
}

#[test]
fn a_form_drawn_too_deep_to_read_is_never_enclosed() {
    // Forms 9 to 21 each draw the next; form 22, past the depth read, holds tagged content.
    let mut extra: Vec<String> = (9..22).map(|n| form("/Fn Do", &format!("/Resources << /XObject << /Fn {} 0 R >> >>", n + 1))).collect();
    extra.push(form("/P <</MCID 2>> BDC 0 0 5 5 re f EMC", ""));
    let mut doc = tagged(&[&format!("{CLAIMED} 0 0 1 1 re f /Fm Do 2 2 1 1 re f")], "/Fm 9 0 R", &extra);
    let done = mark_untagged_artifacts(&mut doc, &[]).unwrap();
    // One wrap each side of the form, none around it.
    assert_eq!(done.sequences, 2, "{done:?}");
    assert!(done.notes.iter().any(|n| n.contains("read in full")), "{done:?}");
}

#[test]
fn loose_text_beside_tagged_text_is_wrapped_inside_its_text_object() {
    let mut doc = tagged(&["BT /F1 12 Tf 20 150 Td (Loose) Tj /P <</MCID 0>> BDC (Tagged) Tj EMC (More) Tj ET"], "", &[]);
    mark_untagged_artifacts(&mut doc, &[]).unwrap();
    assert_eq!(tagged_content(&doc), Status::Passed);
    let (_, text) = &contents(&doc)[0];
    assert!(nests(text), "{text}");
}

#[test]
fn only_streams_that_change_are_rewritten_and_no_wrap_crosses_two() {
    let mut doc = tagged(&[CLAIMED, "0 0 1 1 re f", "2 2 1 1 re f"], "", &[]);
    mark_untagged_artifacts(&mut doc, &[]).unwrap();
    let after = contents(&doc);
    assert_eq!(after[0].0, Some(ObjRef::new(8, 0)));
    assert!(after[1..].iter().all(|(r, t)| r.is_some_and(|r| r.num > 10) && t.contains("/Artifact BMC") && nests(t)), "{after:?}");
    assert_eq!(tagged_content(&doc), Status::Passed);
}

#[test]
fn a_page_with_a_damaged_stream_is_skipped() {
    let damaged = "<< /Length 5 /Filter /FlateDecode >>\nstream\nxxxxx\nendstream".to_string();
    let mut doc = tagged(&[CLAIMED, "0 0 1 1 re f"], "", &[damaged]);
    let all = Object::Array((8..=10).map(|n| Object::Ref(ObjRef::new(n, 0))).collect());
    doc.update_dict(ObjRef::new(4, 0), |p| p.set(b"Contents".to_vec(), all)).unwrap();
    let done = mark_untagged_artifacts(&mut doc, &[]).unwrap();
    assert!(done.marked.is_empty() && done.skipped.iter().any(|s| s.reason.contains("damaged")), "{done:?}");
}

#[test]
fn a_graphics_state_that_spans_two_streams_is_wrapped_inside_each() {
    let mut doc = tagged(&[CLAIMED, "q 0 0 1 1 re f", "Q 2 2 1 1 re f"], "", &[]);
    let done = mark_untagged_artifacts(&mut doc, &[]).unwrap();
    assert_eq!((done.sequences, done.items), (2, 2), "{done:?}");
    let after = contents(&doc);
    let balanced = |t: &str| t.matches("/Artifact BMC").count() == 1 && t.matches("EMC").count() == 1;
    assert!(after[1..].iter().all(|(_, t)| balanced(t)), "{after:?}");
    assert_eq!(tagged_content(&doc), Status::Passed);
}

const STRUCT_IMAGE: &str = "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /StructParent 1 /Length 1 >>\nstream\nx\nendstream";

#[test]
fn an_image_with_a_struct_parent_is_never_enclosed() {
    let mut doc = tagged(&[&format!("{CLAIMED} /Im Do 0 0 1 1 re f")], "/Im 9 0 R", &[STRUCT_IMAGE.into()]);
    let done = mark_untagged_artifacts(&mut doc, &[]).unwrap();
    assert_eq!((done.sequences, done.items), (1, 1), "{done:?}");
    let (_, text) = &contents(&doc)[0];
    assert!(text.find("/Artifact BMC") > text.find("/Im Do"), "{text}");
}

#[test]
fn a_form_drawing_an_image_with_a_struct_parent_is_never_enclosed() {
    let wrapper = form("/Im Do", "/Resources << /XObject << /Im 9 0 R >> >>");
    let mut doc = tagged(&[&format!("{CLAIMED} /Fm Do 0 0 1 1 re f")], "/Fm 10 0 R", &[STRUCT_IMAGE.into(), wrapper]);
    let done = mark_untagged_artifacts(&mut doc, &[]).unwrap();
    assert_eq!((done.sequences, done.items), (1, 1), "{done:?}");
    let (_, text) = &contents(&doc)[0];
    assert!(text.find("/Artifact BMC") > text.find("/Fm Do"), "{text}");
}

#[test]
fn a_path_built_in_one_stream_and_ended_in_the_next_is_left_and_disclosed() {
    // A painted path, and a clip ended by `n`; each stream also holds a path of its own.
    for (built, ended) in [("0 0 1 1 re", "f 2 2 1 1 re f"), ("0 0 5 5 re W", "n 2 2 1 1 re f")] {
        let mut doc = tagged(&[CLAIMED, built, ended], "", &[]);
        let done = mark_untagged_artifacts(&mut doc, &[]).unwrap();
        assert_eq!((done.sequences, done.items), (1, 1), "{done:?}");
        assert!(done.notes.iter().any(|n| n.contains("painted in the next")), "{done:?}");
        let after = contents(&doc);
        assert_eq!(after[1].1, built, "{after:?}");
        assert!(after[2].1.trim_start().starts_with(&ended[..1]) && nests(&after[2].1), "{after:?}");
        assert_eq!(after[2].1.matches("/Artifact BMC").count(), 1, "{after:?}");
    }
}

#[test]
fn an_inline_image_split_across_two_streams_is_never_entered() {
    let mut doc = tagged(&[CLAIMED, "0 0 1 1 re f BI /W 1 /H 1 /BPC 8 /CS /G ID", "x EI 2 2 1 1 re f"], "", &[]);
    let done = mark_untagged_artifacts(&mut doc, &[]).unwrap();
    assert_eq!((done.sequences, done.items), (2, 2), "{done:?}");
    assert!(done.notes.iter().any(|n| n.contains("split across two content streams")), "{done:?}");
    let after = contents(&doc);
    let (first, second) = (&after[1].1, &after[2].1);
    // A wrap may close before `BI` and open after `EI`, never between them.
    let marks = |t: &str| t.contains("BMC") || t.contains("EMC");
    assert!(first.split_once("BI").is_some_and(|(_, image)| !marks(image)), "{done:?} {after:?}");
    assert!(second.split_once("EI").is_some_and(|(image, _)| !marks(image)), "{done:?} {after:?}");
}
