use std::sync::Arc;

use pdfcraft_cos::{Document, ObjRef, Object, SaveOptions, write_incremental};

use super::modify::text_box_of;
use super::*;

/// Two pages. Page 1 has an inline Square annotation in a direct /Annots array; page 2 has an
/// indirect /Annots array holding a Stamp (a type we cannot redraw).
fn fixture() -> Document {
    let objs: Vec<&[u8]> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>",                                       // 1
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 612 792] >>", // 2
        b"<< /Type /Page /Parent 2 0 R /Annots [<< /Type /Annot /Subtype /Square /Rect [10 10 50 50] /C [1 0 0] >>] >>", // 3
        b"<< /Type /Page /Parent 2 0 R /Annots 5 0 R >>",                           // 4
        b"[6 0 R]",                                                                 // 5
        b"<< /Type /Annot /Subtype /Stamp /Rect [100 100 200 150] /Name /Approved /Foo (kept) >>", // 6
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).expect("opens")
}

fn meta(id: &str) -> Meta {
    Meta { date: Some("D:20261001120000Z".into()), id: id.into() }
}

fn new(page: usize, shape: Shape) -> NewAnnotation {
    NewAnnotation {
        page,
        shape,
        style: Style { color: [0.0, 0.4, 1.0], opacity: 1.0, width: 2.0, fill: None, ..Style::default() },
        contents: "hello".into(),
        author: "Ada".into(),
    }
}

/// Save incrementally and reopen, so every check runs against what a reader would see.
fn reopen(doc: &Document) -> Document {
    let bytes = write_incremental(doc, &SaveOptions::default()).expect("writes");
    // A second, independent parser must accept the result too.
    hayro_syntax::Pdf::new(bytes.clone()).expect("hayro-syntax parses the output");
    Document::open(Arc::new(bytes)).expect("reopens")
}

fn list(doc: &Document, page: usize) -> Vec<Dict> {
    let p = page_refs(doc).unwrap()[page];
    annots(doc, p).iter().map(|o| doc.resolve(o).as_dict().cloned().unwrap()).collect()
}

fn ap_content(doc: &Document, d: &Dict) -> String {
    let n = d.get(b"AP").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().and_then(|a| a.reference(b"N"))).expect("has /AP /N");
    let obj = doc.get(n);
    let Object::Stream(s) = &*obj else { panic!("AP is not a stream") };
    String::from_utf8_lossy(&s.decoded().unwrap()).into_owned()
}

fn text(d: &Dict, k: &[u8]) -> String {
    d.get(k).and_then(|o| o.as_string()).map(|s| s.to_text()).unwrap_or_default()
}

fn rect(d: &Dict) -> Vec<f64> {
    d.get(b"Rect").unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect()
}

#[test]
fn every_tool_creates_a_drawable_comment() {
    let shapes = [
        Shape::Note { at: [100.0, 700.0], icon: NoteIcon::Comment },
        Shape::TextMarkup { kind: Markup::Highlight, quads: vec![[72.0, 712.0, 200.0, 712.0, 72.0, 700.0, 200.0, 700.0]] },
        Shape::TextMarkup { kind: Markup::Underline, quads: vec![[72.0, 612.0, 200.0, 612.0, 72.0, 600.0, 200.0, 600.0]] },
        Shape::TextMarkup { kind: Markup::StrikeOut, quads: vec![[72.0, 512.0, 200.0, 512.0, 72.0, 500.0, 200.0, 500.0]] },
        Shape::TextMarkup { kind: Markup::Squiggly, quads: vec![[72.0, 412.0, 200.0, 412.0, 72.0, 400.0, 200.0, 400.0]] },
        Shape::Rectangle { rect: [300.0, 300.0, 200.0, 200.0] },
        Shape::Oval { rect: [300.0, 300.0, 400.0, 350.0] },
        Shape::Line { from: [50.0, 50.0], to: [150.0, 100.0], start: LineEnding::None, end: LineEnding::OpenArrow },
        Shape::Ink { strokes: vec![vec![[10.0, 10.0], [20.0, 30.0], [30.0, 10.0]], vec![[40.0, 40.0]]] },
        Shape::TextBox { rect: [300.0, 600.0, 500.0, 650.0], font_size: 14.0 },
    ];
    let mut doc = fixture();
    for (i, s) in shapes.iter().enumerate() {
        let idx = add_annotation(&mut doc, &new(0, s.clone()), &meta(&format!("id-{i}"))).expect("adds");
        assert!(idx >= 1, "appended after the existing inline annotation");
    }
    let doc = reopen(&doc);
    let all = list(&doc, 0);
    // The inline square, 10 comments and the sticky note's pop-up.
    assert_eq!(all.len(), 12);
    let subtypes: Vec<&[u8]> = all.iter().map(|d| d.name(b"Subtype").unwrap()).collect();
    assert_eq!(
        subtypes,
        [
            &b"Square"[..],
            b"Text",
            b"Popup",
            b"Highlight",
            b"Underline",
            b"StrikeOut",
            b"Squiggly",
            b"Square",
            b"Circle",
            b"Line",
            b"Ink",
            b"FreeText"
        ]
    );
    for d in all.iter().filter(|d| d.contains(b"NM")) {
        assert_eq!(text(d, b"T"), "Ada");
        assert_eq!(text(d, b"Contents"), "hello");
        assert!(d.reference(b"P").is_some(), "/P points at the page");
        assert!(!ap_content(&doc, d).is_empty(), "{:?} has an appearance", d.name(b"Subtype"));
    }
    // Highlight multiplies; the rectangle was normalized.
    let hl = &all[3];
    let ap = hl.get(b"AP").unwrap().as_dict().unwrap().reference(b"N").unwrap();
    let res = doc.get(ap).as_dict().unwrap().get(b"Resources").cloned().unwrap();
    let gs = res.as_dict().unwrap().get(b"ExtGState").unwrap().as_dict().unwrap().get(b"GS0").unwrap().as_dict().unwrap().clone();
    assert_eq!(gs.name(b"BM"), Some(&b"Multiply"[..]));
    assert_eq!(rect(&all[7]), [200.0, 200.0, 300.0, 300.0]);
    // The arrow's rectangle includes the arrowhead and the stroke.
    let r = rect(&all[9]);
    assert!(r[0] < 50.0 && r[2] > 150.0 + 1.0);
    // The text box draws its contents in Helvetica.
    assert!(ap_content(&doc, &all[11]).contains("(hello) Tj"));
    assert!(ap_content(&doc, &all[11]).contains("/Helv 14 Tf"));
    // The note is a fixed-size icon with a closed pop-up.
    assert_eq!(all[1].int(b"F"), Some(28));
    let entries = annots(&doc, page_refs(&doc).unwrap()[0]);
    assert_eq!(all[2].reference(b"Parent"), entries[1].as_ref());
    assert_eq!(all[1].reference(b"Popup"), entries[2].as_ref());
}

/// #260 (page 7): Acrobat draws an Ink annotation as a smooth curve through its `/InkList`
/// points, where PdfCraft drew straight segments between them. Three or more points now make a
/// Catmull-Rom spline through every point (as cubic Béziers); two points stay a line, one a dot.
#[test]
fn ink_strokes_are_smooth_curves_through_their_points() {
    let mut doc = fixture();
    let strokes = vec![vec![[0.0, 0.0], [10.0, 10.0], [20.0, 0.0]], vec![[30.0, 0.0], [40.0, 10.0]], vec![[50.0, 50.0]]];
    let i = add_annotation(&mut doc, &new(0, Shape::Ink { strokes }), &meta("ink")).unwrap();
    let ap = ap_content(&doc, &list(&doc, 0)[i]);
    // Tangents at each point run parallel to the chord between its neighbours (the ends use
    // their own point): (0,0)→(10,10) leaves along (10,10) and arrives along (20,0).
    assert!(ap.contains("0 0 m\n1.667 1.667 6.667 10 10 10 c\n13.333 10 18.333 1.667 20 0 c\nS\n"), "{ap}");
    assert!(ap.contains("30 0 m\n40 10 l\nS\n"), "{ap}");
    assert!(ap.contains("50 50 m\n50.01 50 l\nS\n"), "{ap}");
    // /Rect holds the whole curve: rising to (20, 20) and dropping to (21, 0), it swings above
    // y 20 (its control point is at y 21.667), beyond the points' own bounds plus the margin.
    let rise = Shape::Ink { strokes: vec![vec![[0.0, 0.0], [10.0, 10.0], [20.0, 20.0], [21.0, 0.0]]] };
    let j = add_annotation(&mut doc, &new(0, rise), &meta("rise")).unwrap();
    let top = rect(&list(&doc, 0)[j])[3];
    assert!(top >= 21.667 + 2.0 - 0.01, "/Rect top {top}");
}

#[test]
fn indirect_annots_array_is_updated_in_place() {
    let mut doc = fixture();
    add_annotation(&mut doc, &new(1, Shape::Rectangle { rect: [0.0, 0.0, 10.0, 10.0] }), &meta("x")).unwrap();
    let doc = reopen(&doc);
    let page = doc.get(page_refs(&doc).unwrap()[1]);
    assert_eq!(page.as_dict().unwrap().reference(b"Annots"), Some(ObjRef::new(5, 0)));
    assert_eq!(doc.get(ObjRef::new(5, 0)).as_array().unwrap().len(), 2);
}

#[test]
fn invalid_geometry_is_rejected() {
    let mut doc = fixture();
    let bad = [
        Shape::TextMarkup { kind: Markup::Highlight, quads: vec![] },
        Shape::Rectangle { rect: [0.0, 0.0, 0.5, 10.0] },
        Shape::Line { from: [1.0, 1.0], to: [1.0, 1.0], start: LineEnding::None, end: LineEnding::None },
        Shape::Ink { strokes: vec![vec![]] },
        Shape::Note { at: [f64::NAN, 0.0], icon: NoteIcon::Note },
    ];
    for s in bad {
        assert!(matches!(add_annotation(&mut doc, &new(0, s.clone()), &meta("x")), Err(AnnotError::Invalid(_))), "{s:?}");
    }
    assert_eq!(add_annotation(&mut doc, &new(5, Shape::Rectangle { rect: [0.0, 0.0, 10.0, 10.0] }), &meta("x")), Err(AnnotError::NoSuchPage(5)));
    assert_eq!(AnnotError::NoSuchPage(5).to_string(), "page 6 does not exist", "the message counts pages from 1 (#1550)");
    assert!(!doc.is_modified(), "failed adds change nothing");
}

#[test]
fn replies_and_status_thread_onto_the_parent() {
    let mut doc = fixture();
    // Replying to the inline square promotes it to an indirect object.
    let r = add_reply(&mut doc, 0, 0, "Agreed", "Bob", &meta("r1")).unwrap();
    let s = set_review_state(&mut doc, 0, 0, ReviewState::Accepted, "Bob", &meta("r2")).unwrap();
    assert_eq!((r, s), (1, 2));
    let doc = reopen(&doc);
    let p = page_refs(&doc).unwrap()[0];
    let entries = annots(&doc, p);
    let parent = entries[0].as_ref().expect("promoted to an indirect object");
    let all = list(&doc, 0);
    assert_eq!(all[1].reference(b"IRT"), Some(parent));
    assert_eq!(text(&all[1], b"Contents"), "Agreed");
    assert_eq!(rect(&all[1]), [10.0, 10.0, 50.0, 50.0]);
    assert_eq!(ap_content(&doc, &all[1]), "", "replies draw nothing");
    assert_eq!(text(&all[2], b"State"), "Accepted");
    assert_eq!(text(&all[2], b"StateModel"), "Review");
    assert_eq!(text(&all[2], b"Contents"), "Accepted set by Bob");
}

/// A state reply records a state, not a note (ISO 32000-2 §12.5.6.3: the state is kept "in a
/// separate text annotation"), so it is written Hidden; a reviewer's next change replies to their
/// previous state reply ("Additional state changes shall be made by adding text annotations in
/// reply to the previous reply for a given user"). Another reviewer's state, and another state
/// model, start their own chain on the comment.
#[test]
fn status_and_checkmark_replies_are_hidden_and_chain_per_reviewer() {
    let mut doc = fixture();
    let first = set_review_state(&mut doc, 0, 0, ReviewState::Accepted, "Bob", &meta("s1")).unwrap();
    let carol = set_review_state(&mut doc, 0, 0, ReviewState::Completed, "Carol", &meta("s2")).unwrap();
    let second = set_review_state(&mut doc, 0, 0, ReviewState::Rejected, "Bob", &meta("s3")).unwrap();
    let mark = set_marked(&mut doc, 0, 0, true, "Bob", &meta("m1")).unwrap();
    let unmark = set_marked(&mut doc, 0, 0, false, "Bob", &meta("m2")).unwrap();
    let doc = reopen(&doc);
    let p = page_refs(&doc).unwrap()[0];
    let entries = annots(&doc, p);
    let at = |i: usize| entries[i].as_ref().unwrap();
    let all = list(&doc, 0);
    for i in [first, carol, second, mark, unmark] {
        assert_eq!(all[i].int(b"F"), Some(30), "hidden, printable, no zoom, no rotate: {i}");
    }
    assert_eq!(all[first].reference(b"IRT"), Some(at(0)));
    assert_eq!(all[carol].reference(b"IRT"), Some(at(0)), "another reviewer starts on the comment");
    assert_eq!(all[second].reference(b"IRT"), Some(at(first)), "Bob's change replies to Bob's last status");
    assert_eq!(all[mark].reference(b"IRT"), Some(at(0)), "a checkmark is its own chain");
    assert_eq!(all[unmark].reference(b"IRT"), Some(at(mark)));
    // A reply that isn't a state stays visible.
    let mut doc = doc;
    let r = add_reply(&mut doc, 0, 0, "Agreed", "Bob", &meta("r")).unwrap();
    assert_eq!(list(&doc, 0)[r].int(b"F"), Some(28));
}

/// Review on #4126: a history has no length limit. The 70th change still replies to the 69th,
/// and a reviewer whose name was typed with spaces continues their own chain.
#[test]
fn a_long_status_history_keeps_chaining_to_the_latest() {
    let mut doc = fixture();
    let mut prev: Option<usize> = None;
    for i in 0..70 {
        let state = if i % 2 == 0 { ReviewState::Accepted } else { ReviewState::Rejected };
        let at = set_review_state(&mut doc, 0, 0, state, "Bob", &meta(&format!("s{i}"))).unwrap();
        if let Some(p) = prev {
            let entries = annots(&doc, page_refs(&doc).unwrap()[0]);
            assert_eq!(list(&doc, 0)[at].reference(b"IRT"), entries[p].as_ref(), "change {i}");
        }
        prev = Some(at);
    }
    let padded = set_review_state(&mut doc, 0, 0, ReviewState::Completed, " Bob ", &meta("pad")).unwrap();
    let entries = annots(&doc, page_refs(&doc).unwrap()[0]);
    assert_eq!(list(&doc, 0)[padded].reference(b"IRT"), prev.and_then(|p| entries[p].as_ref()));
    // Listed newest first, the first reply read walks the whole chain at once.
    let latest = entries[padded].as_ref();
    let p = page_refs(&doc).unwrap()[0];
    let mut reversed = entries.clone();
    reversed.reverse();
    set_annots(&mut doc, p, reversed).unwrap();
    let next = set_review_state(&mut doc, 0, 71, ReviewState::Accepted, "Bob", &meta("next")).unwrap();
    assert_eq!(list(&doc, 0)[next].reference(b"IRT"), latest);
}

/// Review on #4126: a reply with `/StateModel` but no `/State` has the model's default state
/// (ISO 32000-2 Table 175), for the reader as for the writer: it is Bob's latest status, None,
/// and his next change replies to it.
#[test]
fn a_state_model_without_a_state_is_the_default_state() {
    let mut doc = fixture();
    let bare = add_reply(&mut doc, 0, 0, "", "Bob", &meta("bare")).unwrap();
    let p = page_refs(&doc).unwrap()[0];
    let r = annots(&doc, p)[bare].as_ref().unwrap();
    doc.update_dict(r, |d| {
        d.set(b"StateModel".to_vec(), PdfString::text("Review"));
        d.set(b"F".to_vec(), Object::Int(30));
    })
    .unwrap();
    let summary = summaries(&doc).into_iter().find(|s| s.index == bare).unwrap();
    assert_eq!(summary.state.as_deref(), Some("None"));
    let next = set_review_state(&mut doc, 0, 0, ReviewState::Accepted, "Bob", &meta("next")).unwrap();
    assert_eq!(list(&doc, 0)[next].reference(b"IRT"), Some(r));

    // A model PdfCraft doesn't know isn't a state: a status replying to such a reply is about
    // that reply, not about the comment, so Bob's next change on the comment starts afresh.
    let mut doc = fixture();
    let other = add_reply(&mut doc, 0, 0, "", "Ann", &meta("other")).unwrap();
    let p = page_refs(&doc).unwrap()[0];
    let o = annots(&doc, p)[other].as_ref().unwrap();
    // Nor is a `/State` the readers can't read, such as a number (review on #4126).
    doc.update_dict(o, |d| {
        d.set(b"StateModel".to_vec(), PdfString::text("Other"));
        d.set(b"State".to_vec(), Object::Int(5));
    })
    .unwrap();
    assert_eq!(summaries(&doc).into_iter().find(|s| s.index == other).unwrap().state, None);
    set_review_state(&mut doc, 0, other, ReviewState::Rejected, "Bob", &meta("on-other")).unwrap();
    let fresh = set_review_state(&mut doc, 0, 0, ReviewState::Completed, "Bob", &meta("fresh")).unwrap();
    assert_eq!(list(&doc, 0)[fresh].reference(b"IRT"), annots(&doc, p)[0].as_ref());
}

/// A third change replies to the second, not the first; and Bob's status on another comment
/// is never the one his next change on this comment replies to.
#[test]
fn a_reviewers_chain_continues_from_their_latest_status_on_that_comment() {
    let mut doc = fixture();
    let note = add_annotation(&mut doc, &new(0, Shape::Note { at: [100.0, 100.0], icon: NoteIcon::Comment }), &meta("n")).unwrap();
    let first = set_review_state(&mut doc, 0, 0, ReviewState::Accepted, "Bob", &meta("s1")).unwrap();
    let elsewhere = set_review_state(&mut doc, 0, note, ReviewState::Cancelled, "Bob", &meta("s2")).unwrap();
    let second = set_review_state(&mut doc, 0, 0, ReviewState::Rejected, "Bob", &meta("s3")).unwrap();
    let third = set_review_state(&mut doc, 0, 0, ReviewState::Completed, "Bob", &meta("s4")).unwrap();
    let doc = reopen(&doc);
    let entries = annots(&doc, page_refs(&doc).unwrap()[0]);
    let at = |i: usize| entries[i].as_ref().unwrap();
    let all = list(&doc, 0);
    assert_eq!(all[elsewhere].reference(b"IRT"), Some(at(note)));
    assert_eq!(all[second].reference(b"IRT"), Some(at(first)), "not Bob's status on the note");
    assert_eq!(all[third].reference(b"IRT"), Some(at(second)), "the latest, not the first");
}

#[test]
fn delete_takes_popup_and_reply_chain() {
    let mut doc = fixture();
    let note = add_annotation(&mut doc, &new(0, Shape::Note { at: [100.0, 100.0], icon: NoteIcon::Comment }), &meta("n")).unwrap();
    let reply = add_reply(&mut doc, 0, note, "first", "Bob", &meta("r1")).unwrap();
    add_reply(&mut doc, 0, reply, "reply to reply", "Ada", &meta("r2")).unwrap();
    add_reply(&mut doc, 0, 0, "on the square", "Ada", &meta("r3")).unwrap();
    assert_eq!(list(&doc, 0).len(), 6);
    delete_annotation(&mut doc, 0, note).unwrap();
    let doc = reopen(&doc);
    let left: Vec<String> =
        list(&doc, 0).iter().map(|d| format!("{}:{}", String::from_utf8_lossy(d.name(b"Subtype").unwrap()), text(d, b"Contents"))).collect();
    assert_eq!(left, ["Square:", "Text:on the square"]);
    let mut doc = doc;
    assert_eq!(delete_annotation(&mut doc, 0, 9), Err(AnnotError::NoSuchAnnotation { page: 0, index: 9 }));
}

/// A locked reply can't be deleted, so it doesn't go with what it answers either: deleting the
/// reply above it, or the comment, is refused and changes nothing; once it is unlocked they go.
#[test]
fn a_locked_reply_keeps_what_it_answers() {
    let mut doc = fixture();
    let note = add_annotation(&mut doc, &new(0, Shape::Note { at: [100.0, 100.0], icon: NoteIcon::Comment }), &meta("n")).unwrap();
    let reply = add_reply(&mut doc, 0, note, "first", "Bob", &meta("r1")).unwrap();
    let answer = add_reply(&mut doc, 0, reply, "locked answer", "Ada", &meta("r2")).unwrap();
    set_locked(&mut doc, 0, answer, true).unwrap();
    let before = list(&doc, 0).len();
    assert_eq!(delete_annotation(&mut doc, 0, reply), Err(AnnotError::Invalid("a reply to it is locked".into())));
    assert_eq!(delete_annotation(&mut doc, 0, note), Err(AnnotError::Invalid("a reply to it is locked".into())));
    assert_eq!(list(&doc, 0).len(), before, "nothing went");
    // Asked first, read-only, as a Delete command does: the same answer, and the answer's own
    // lock is its own.
    assert_eq!(deletable(&doc, 0, note), Err(AnnotError::Invalid("a reply to it is locked".into())));
    assert_eq!(deletable(&doc, 0, answer), Err(AnnotError::Invalid("the comment is locked".into())));
    // Lifted on a copy, to draw the page beneath, the locks don't count.
    let mut copy = doc.clone();
    lift_annotation(&mut copy, 0, note).unwrap();
    assert!(list(&copy, 0).len() < before, "the thread is lifted off the copy");
    set_locked(&mut doc, 0, answer, false).unwrap();
    assert_eq!(deletable(&doc, 0, reply), Ok(()));
    delete_annotation(&mut doc, 0, reply).unwrap();
    let left: Vec<String> = list(&doc, 0).iter().map(|d| text(d, b"Contents")).collect();
    assert!(!left.iter().any(|t| t == "first" || t == "locked answer"), "the reply and its answer went: {left:?}");
}

/// A reply or pop-up written inline in `/Annots` (a file can, though the standard asks for
/// references) goes with the comment it points at, and so does a pop-up an inline reply or an
/// inline comment names. An unrelated inline comment stays until it is deleted itself.
#[test]
fn delete_takes_inline_replies_and_popups() {
    let mut doc = fixture();
    let mut popup = |note: &str| {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("Annot"));
        d.set(b"Subtype".to_vec(), Object::name("Popup"));
        d.set(b"Contents".to_vec(), PdfString::text(note));
        doc.add(Object::Dict(d))
    };
    let (reply_popup, square_popup) = (popup("of the reply"), popup("of the square"));
    let inline = |subtype: &str, links: &[(&[u8], ObjRef)]| {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("Annot"));
        d.set(b"Subtype".to_vec(), Object::name(subtype));
        links.iter().for_each(|(key, r)| d.set(key.to_vec(), Object::Ref(*r)));
        Object::Dict(d)
    };
    let stamp = ObjRef::new(6, 0);
    let entries = vec![
        Object::Ref(stamp),
        inline("Text", &[(b"IRT", stamp), (b"Popup", reply_popup)]),
        Object::Ref(reply_popup),
        inline("Popup", &[(b"Parent", stamp)]),
        inline("Square", &[(b"Popup", square_popup)]),
        Object::Ref(square_popup),
    ];
    doc.set(ObjRef::new(5, 0), Object::Array(entries));
    let subtypes = |doc: &Document| -> Vec<String> {
        list(doc, 1).iter().map(|d| String::from_utf8_lossy(d.name(b"Subtype").unwrap()).into_owned() + ":" + &text(d, b"Contents")).collect()
    };
    delete_annotation(&mut doc, 1, 0).unwrap();
    let mut doc = reopen(&doc);
    assert_eq!(subtypes(&doc), ["Square:", "Popup:of the square"]);
    delete_annotation(&mut doc, 1, 0).unwrap();
    assert!(subtypes(&reopen(&doc)).is_empty(), "the inline comment's pop-up went with it");
}

#[test]
fn deleting_the_last_comment_removes_annots() {
    let mut doc = fixture();
    delete_annotation(&mut doc, 0, 0).unwrap();
    let doc = reopen(&doc);
    assert!(!doc.get(page_refs(&doc).unwrap()[0]).as_dict().unwrap().contains(b"Annots"));
}

#[test]
fn move_shifts_all_geometry_and_the_popup() {
    let mut doc = fixture();
    let q = [72.0, 712.0, 200.0, 712.0, 72.0, 700.0, 200.0, 700.0];
    let h = add_annotation(&mut doc, &new(0, Shape::TextMarkup { kind: Markup::Highlight, quads: vec![q] }), &meta("h")).unwrap();
    let i = add_annotation(&mut doc, &new(0, Shape::Ink { strokes: vec![vec![[10.0, 10.0], [20.0, 20.0]]] }), &meta("i")).unwrap();
    let n = add_annotation(&mut doc, &new(0, Shape::Note { at: [100.0, 100.0], icon: NoteIcon::Note }), &meta("n")).unwrap();
    for idx in [h, i, n] {
        move_annotation(&mut doc, 0, idx, 10.0, -5.0, &meta("")).unwrap();
    }
    let doc = reopen(&doc);
    let all = list(&doc, 0);
    let qp: Vec<f64> = all[h].get(b"QuadPoints").unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect();
    assert_eq!(qp, [82.0, 707.0, 210.0, 707.0, 82.0, 695.0, 210.0, 695.0]);
    let ink = all[i].get(b"InkList").unwrap().as_array().unwrap()[0].as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect::<Vec<_>>();
    assert_eq!(ink, [20.0, 5.0, 30.0, 15.0]);
    assert_eq!(rect(&all[n]), [110.0, 75.0, 130.0, 95.0]);
    let popup = &all[n + 1];
    assert_eq!(rect(popup)[0], 140.0);
}

#[test]
fn restyle_and_resize_redraw_the_appearance() {
    let mut doc = fixture();
    let r = add_annotation(&mut doc, &new(0, Shape::Rectangle { rect: [10.0, 10.0, 110.0, 60.0] }), &meta("r")).unwrap();
    set_style(&mut doc, 0, r, StyleChange::new(Some([1.0, 0.0, 0.0]), Some(0.5), Some(4.0), None), &meta("")).unwrap();
    set_rect(&mut doc, 0, r, [0.0, 0.0, 200.0, 100.0], &meta("")).unwrap();
    let doc2 = reopen(&doc);
    let d = &list(&doc2, 0)[r];
    let ap = ap_content(&doc2, d);
    assert!(ap.contains("1 0 0 RG") && ap.contains("4 w") && ap.contains("/GS0 gs"), "{ap}");
    assert!(ap.contains("2 2 196 96 re"), "inset by half the border: {ap}");
    assert_eq!(d.get(b"CA").and_then(|o| o.as_f64()), Some(0.5));
    // A stamp's appearance can't be regenerated, so restyling it changes nothing.
    let before = list(&doc, 1)[0].clone();
    assert_eq!(
        set_style(&mut doc, 1, 0, StyleChange::new(Some([0.0; 3]), None, None, None), &meta("")),
        Err(AnnotError::Unsupported("Stamp".into()))
    );
    assert_eq!(list(&doc, 1)[0], before);
    // Lines can't be resized as rectangles.
    let l = add_annotation(
        &mut doc,
        &new(0, Shape::Line { from: [0.0, 0.0], to: [10.0, 10.0], start: LineEnding::None, end: LineEnding::None }),
        &meta("l"),
    )
    .unwrap();
    assert!(matches!(set_rect(&mut doc, 0, l, [0.0, 0.0, 5.0, 5.0], &meta("")), Err(AnnotError::Invalid(_))));
}

/// Typed text or a text box takes an oval or a circle for its border: the text box keeps its
/// place and the shape goes around it (`/Rect` grows, `/RD` holds the text box, `/PCBorderShape`
/// the shape); back to a rectangle, it is the text box it was. A resized frame takes its text box
/// along, a thicker border leaves it where it is.
#[test]
fn text_boxes_take_an_oval_or_a_circle_around_their_text() {
    let close = |a: &[f64], b: &[f64]| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01);
    let nums = |d: &Dict, k: &[u8]| -> Vec<f64> {
        d.get(k).and_then(|o| o.as_array()).map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default()
    };
    // Where the text starts: its first line's text matrix.
    let text_at = |doc: &Document, d: &Dict| ap_content(doc, d).lines().find(|l| l.ends_with(" Tm") || l.contains(" Tm ")).map(str::to_owned);
    let mut doc = fixture();
    let mut tw = new(0, Shape::Typewriter { rect: [100.0, 100.0, 200.0, 120.0], font_size: 10.0 });
    tw.style.width = 1.0;
    let t = add_annotation(&mut doc, &tw, &meta("t")).unwrap();
    // In an oval or a circle the text is centred in its text box (left-aligned in a box): one
    // line of "hello" at 10 pt in the 100 × 20 text box centred on (150, 110), 3 pt from its top.
    let centred = format!("1 0 0 1 {} {} Tm (hello) Tj", n(150.0 - appearance::text_width("hello", 10.0) / 2.0), n(107.0));
    assert!(text_at(&doc, &list(&doc, 0)[t]).is_some_and(|l| l.starts_with("1 0 0 1 103 ")), "a box: left-aligned");
    set_border_shape(&mut doc, 0, t, BorderShape::Oval, &meta("")).unwrap();
    let doc2 = reopen(&doc);
    let d = &list(&doc2, 0)[t];
    // Around the 100 × 20 text box, centred: √2 its size, two points more for the 1 pt border.
    let (ax, ay) = (50.0 * std::f64::consts::SQRT_2 + 2.0, 10.0 * std::f64::consts::SQRT_2 + 2.0);
    assert!(close(&rect(d), &[150.0 - ax, 110.0 - ay, 150.0 + ax, 110.0 + ay]), "{:?}", rect(d));
    assert!(close(&nums(d, b"RD"), &[ax - 50.0, ay - 10.0, ax - 50.0, ay - 10.0]), "{:?}", nums(d, b"RD"));
    assert_eq!(d.name(b"PCBorderShape"), Some(&b"Oval"[..]));
    let ap = ap_content(&doc2, d);
    assert!(ap.contains(" c\n") && ap.contains("S\n") && !ap.contains("re S"), "an oval, not a box: {ap}");
    assert_eq!(text_at(&doc2, d).as_deref(), Some(centred.as_str()), "the text keeps its place");
    assert_eq!(props(&doc2, 0, t).unwrap().border_shape, Some(BorderShape::Oval));
    // A circle around the text: the text box shrinks to "hello" with its padding, centred where it
    // was, and the circle touches its sides (the text a point clear of the border inside).
    set_border_shape(&mut doc, 0, t, BorderShape::Circle, &meta("")).unwrap();
    let d = &list(&doc, 0)[t];
    let hw = appearance::text_width("hello", 10.0) / 2.0;
    let r = (hw + 5.0).max(hw.hypot(6.0) + 2.0);
    assert!(close(&rect(d), &[150.0 - r, 110.0 - r, 150.0 + r, 110.0 + r]), "{:?}", rect(d));
    assert!(close(&nums(d, b"RD"), &[r - hw - 5.0, r - 10.0, r - hw - 5.0, r - 10.0]), "{:?}", nums(d, b"RD"));
    assert_eq!(text_at(&doc, d).as_deref(), Some(centred.as_str()));
    // A thicker border: the typed text's box grows by the change on every side, so its text keeps
    // its place inside the thicker line (as with a rectangle), and the circle goes around that.
    set_style(&mut doc, 0, t, StyleChange::new(None, None, Some(3.0), None), &meta("")).unwrap();
    let d = &list(&doc, 0)[t];
    assert_eq!(text_at(&doc, d).as_deref(), Some(centred.as_str()));
    let r3 = (hw + 7.0).max(hw.hypot(6.0) + 4.0);
    assert!(close(&rect(d), &[150.0 - r3, 110.0 - r3, 150.0 + r3, 110.0 + r3]), "{:?}", rect(d));
    // Its frame resized: the text box follows inside, keeping its proportions, its longer sides
    // reaching the circle.
    set_rect(&mut doc, 0, t, [150.0 - 2.0 * r3, 110.0 - 2.0 * r3, 150.0 + 2.0 * r3, 110.0 + 2.0 * r3], &meta("")).unwrap();
    let d = &list(&doc, 0)[t];
    let rd = nums(d, b"RD");
    let tb = [rect(d)[0] + rd[0], rect(d)[1] + rd[1], rect(d)[2] - rd[2], rect(d)[3] - rd[3]];
    assert!(((tb[2] - tb[0]) / (tb[3] - tb[1]) - (hw + 7.0) / 12.0).abs() < 0.01, "the text box keeps its proportions: {tb:?}");
    assert!(close(&rect(d), &[150.0 - 2.0 * r3, 110.0 - 2.0 * r3, 150.0 + 2.0 * r3, 110.0 + 2.0 * r3]), "{:?}", rect(d));
    // A rectangle again: the text box it has, nothing around it.
    set_border_shape(&mut doc, 0, t, BorderShape::Rectangle, &meta("")).unwrap();
    let d = &list(&doc, 0)[t];
    assert!(close(&rect(d), &tb), "{:?} vs {tb:?}", rect(d));
    assert!(!d.contains(b"RD") && !d.contains(b"PCBorderShape"));
    assert!(ap_content(&doc, d).contains("re S"));

    // A filled text box made with an oval: the background takes the shape too.
    let mut tb = new(0, Shape::TextBox { rect: [300.0, 500.0, 400.0, 540.0], font_size: 12.0 });
    (tb.style.fill, tb.style.width, tb.style.border_shape) = (Some([1.0, 1.0, 0.6]), 1.0, BorderShape::Oval);
    let b = add_annotation(&mut doc, &tb, &meta("b")).unwrap();
    let d = &list(&doc, 0)[b];
    assert_eq!(d.name(b"PCBorderShape"), Some(&b"Oval"[..]));
    let ap = ap_content(&doc, d);
    assert!(ap.contains("1 1 0.6 rg") && ap.contains(" c\nh\nf\n") && !ap.contains(" re f"), "{ap}");
    // A callout's border stays a rectangle.
    let c = add_annotation(
        &mut doc,
        &new(
            0,
            Shape::Callout {
                rect: [300.0, 300.0, 400.0, 340.0],
                knee: [280.0, 320.0],
                point: [250.0, 280.0],
                ending: LineEnding::OpenArrow,
                font_size: 10.0,
            },
        ),
        &meta("c"),
    )
    .unwrap();
    assert!(matches!(set_border_shape(&mut doc, 0, c, BorderShape::Oval, &meta("")), Err(AnnotError::Invalid(_))));
    assert_eq!(props(&doc, 0, c).unwrap().border_shape, None);
    assert_eq!(BorderShape::parse("CIRCLE"), Some(BorderShape::Circle));
}

/// A circle goes snug around short text, as a number on a drawing is circled: the text box is no
/// wider than its text needs and the circle touches its sides, where one through the box's corners
/// stood well clear. It follows the text and its size as they change; one made larger by hand
/// keeps its size.
#[test]
fn a_circle_goes_snug_around_its_text_and_follows_it() {
    let close = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01;
    let mut doc = fixture();
    // "3" at 10 pt with a 1 pt border, in the box Fill & Sign sizes to it: the digit with 2 pt of
    // padding and 4 pt to spare across, a line and 6 pt down, the border around that.
    let w3 = appearance::text_width("3", 10.0);
    let mut typed = new(0, Shape::Typewriter { rect: [100.0, 680.0, 110.0 + w3, 700.0], font_size: 10.0 });
    (typed.contents, typed.style.width, typed.style.border_shape) = ("3".into(), 1.0, BorderShape::Circle);
    let t = add_annotation(&mut doc, &typed, &meta("t")).unwrap();
    let frame = |doc: &Document| {
        let r = rect(&list(doc, 0)[t]);
        ((r[2] - r[0], r[3] - r[1]), ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0))
    };
    let centre = (105.0 + w3 / 2.0, 690.0);
    // As tall as the box, 20 pt across: through the corners it was 29.
    assert!(close(frame(&doc).0, (20.0, 20.0)) && close(frame(&doc).1, centre), "{:?}", frame(&doc));
    // The text box Fill & Sign gives a box to type into, 40 pt wide: a circle goes snug around its
    // text there too.
    let mut wide = typed.clone();
    wide.shape = Shape::Typewriter { rect: [300.0, 680.0, 342.0, 700.0], font_size: 10.0 };
    wide.style.border_shape = BorderShape::Rectangle;
    let b = add_annotation(&mut doc, &wide, &meta("b")).unwrap();
    set_border_shape(&mut doc, 0, b, BorderShape::Circle, &meta("")).unwrap();
    let r = rect(&list(&doc, 0)[b]);
    assert!(close((r[2] - r[0], r[3] - r[1]), (20.0, 20.0)), "{r:?}");
    // More text: the circle grows with it, about the same centre; less, and it shrinks back.
    set_contents(&mut doc, 0, t, "123", &meta("")).unwrap();
    let w = appearance::text_width("123", 10.0) / 2.0;
    let d = 2.0 * (w + 5.0).max(10.0).max(w.hypot(6.0) + 2.0);
    assert!(close(frame(&doc).0, (d, d)) && close(frame(&doc).1, centre), "{:?}", frame(&doc));
    set_contents(&mut doc, 0, t, "3", &meta("")).unwrap();
    assert!(close(frame(&doc).0, (20.0, 20.0)) && close(frame(&doc).1, centre), "{:?}", frame(&doc));
    // A larger font: a larger circle, still snug.
    set_font_size(&mut doc, 0, t, 20.0, &meta("")).unwrap();
    let w = appearance::text_width("3", 20.0) / 2.0;
    let d = 2.0 * (w + 5.0).max(16.0).max(w.hypot(12.0) + 2.0);
    assert!(close(frame(&doc).0, (d, d)) && close(frame(&doc).1, centre), "{:?}", frame(&doc));
    set_font_size(&mut doc, 0, t, 10.0, &meta("")).unwrap();
    assert!(close(frame(&doc).0, (20.0, 20.0)), "{:?}", frame(&doc));
    // Made larger by hand, it keeps its size as its text changes.
    set_rect(&mut doc, 0, t, [centre.0 - 20.0, centre.1 - 20.0, centre.0 + 20.0, centre.1 + 20.0], &meta("")).unwrap();
    assert!(close(frame(&doc).0, (40.0, 40.0)), "{:?}", frame(&doc));
    set_contents(&mut doc, 0, t, "4", &meta("")).unwrap();
    assert!(close(frame(&doc).0, (40.0, 40.0)) && close(frame(&doc).1, centre), "{:?}", frame(&doc));
}

/// A circle around a line wider than it is tall touches its text box's sides: `/RD` is 0 there,
/// not a rounding error below it, and a hair below 0 still reads. (A text box that couldn't be
/// read stopped following its text, and its text wrapped in a narrower one.) The circle keeps its
/// centre as its text changes.
#[test]
fn a_circle_touching_its_text_box_keeps_following_its_text() {
    let mut doc = fixture();
    // Sized to its text: 2 pt of padding and half a point to spare across, a line down.
    let (w, h) = (appearance::text_width("Testtext", 12.0) + 4.5, 12.0 * 1.11 + 4.0);
    let mut b = new(0, Shape::TextBox { rect: [210.79, 700.0 - h, 210.79 + w, 700.0], font_size: 12.0 });
    (b.contents, b.style.width) = ("Testtext".into(), 0.0);
    let t = add_annotation(&mut doc, &b, &meta("t")).unwrap();
    // Circle, then a 1 pt border, which the circle grows by.
    set_border_shape(&mut doc, 0, t, BorderShape::Circle, &meta("")).unwrap();
    set_style(&mut doc, 0, t, StyleChange::new(None, None, Some(1.0), None), &meta("")).unwrap();
    let lines = |doc: &Document| ap_content(doc, &list(doc, 0)[t]).matches(" Tj").count();
    let frame = |doc: &Document| {
        let r = rect(&list(doc, 0)[t]);
        ((r[2] - r[0], r[3] - r[1]), ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0))
    };
    let rd = |doc: &Document| -> Vec<f64> { list(doc, 0)[t].get(b"RD").unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect() };
    assert!(rd(&doc).iter().all(|x| *x >= 0.0) && rd(&doc)[0] < 0.01, "touching the sides: {:?}", rd(&doc));
    assert_eq!(lines(&doc), 1, "Testtext on one line");
    let (wide, centre) = frame(&doc);
    set_contents(&mut doc, 0, t, "KRS", &meta("")).unwrap();
    let (small, at) = frame(&doc);
    assert!(small.0 < wide.0 - 10.0 && (small.0 - small.1).abs() < 0.01, "a smaller circle: {small:?} from {wide:?}");
    assert!((at.0 - centre.0).abs() < 0.01 && (at.1 - centre.1).abs() < 0.01, "about its centre: {at:?} {centre:?}");
    assert_eq!(lines(&doc), 1);
    // A hair below 0, as rounding left it: still read, so the circle follows its text again.
    let (_, r) = annot_ref(&mut doc, 0, t).unwrap();
    let mut was = rd(&doc);
    was[0] = -2.8e-14;
    doc.update_dict(r, |d| d.set(b"RD".to_vec(), num_array(&was))).unwrap();
    assert!(text_box_of(&list(&doc, 0)[t]).is_some());
    set_contents(&mut doc, 0, t, "Testtext", &meta("")).unwrap();
    assert!((frame(&doc).0.0 - wide.0).abs() < 0.01 && lines(&doc) == 1, "{:?}", frame(&doc));
    assert!(rd(&doc).iter().all(|x| *x >= 0.0), "{:?}", rd(&doc));
}

/// On a page turned a quarter, typed text is laid out as the page is shown (`/PCTextRotate`), and a
/// circle made with it goes around the text as shown: as large as around the same text on an
/// upright page. (Made before the turn was set, its text wrapped at the box's displayed height and
/// the circle went around several lines.)
#[test]
fn a_circle_made_on_a_turned_page_is_sized_as_on_an_upright_one() {
    let w = appearance::text_width("12345", 10.0);
    let typed = |rect: [f64; 4]| {
        let mut t = new(0, Shape::Typewriter { rect, font_size: 10.0 });
        (t.contents, t.style.width, t.style.border_shape) = ("12345".into(), 1.0, BorderShape::Circle);
        t
    };
    let size = |doc: &Document, i: usize| {
        let r = rect(&list(doc, 0)[i]);
        (r[2] - r[0], r[3] - r[1])
    };
    let mut upright = fixture();
    let u = add_annotation(&mut upright, &typed([100.0, 680.0, 110.0 + w, 700.0]), &meta("u")).unwrap();
    for turn in [90, 270] {
        let mut doc = fixture();
        let page = page_refs(&doc).unwrap()[0];
        doc.update_dict(page, |d| d.set(b"Rotate".to_vec(), Object::Int(turn))).unwrap();
        // The same box as shown: its sides swapped in user space.
        let t = add_annotation(&mut doc, &typed([100.0, 680.0, 120.0, 690.0 + w]), &meta("t")).unwrap();
        assert_eq!(list(&doc, 0)[t].get(b"PCTextRotate").and_then(Object::as_int), Some(turn));
        let (turned, straight) = (size(&doc, t), size(&upright, u));
        assert!((turned.0 - straight.0).abs() < 0.01 && (turned.1 - straight.1).abs() < 0.01, "{turn}°: {turned:?}, upright {straight:?}");
    }
}

/// A rectangle's fill, and a text box's border, background and text, each drawn at an opacity of
/// its own: the line's in `/CA`, the fill's in `/ca` (PDF 2.0), the text's in `/PCTextOpacity`, and
/// in the appearance `/GS0` (strokes at the line's, fills at the fill's) and `/GS1` for the text.
#[test]
fn parts_are_drawn_at_opacities_of_their_own() {
    let num = |d: &Dict, k: &[u8]| d.get(k).and_then(|o| o.as_f64());
    let gstate = |doc: &Document, d: &Dict, name: &[u8]| -> Option<(f64, f64)> {
        let n = d.get(b"AP").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().and_then(|a| a.reference(b"N")))?;
        let obj = doc.get(n);
        let Object::Stream(s) = &*obj else { return None };
        let res = s.dict.get(b"Resources").map(|r| doc.resolve(r))?;
        let gs = res.as_dict()?.get(b"ExtGState").map(|g| doc.resolve(g))?;
        let gs = gs.as_dict()?.get(name).map(|g| doc.resolve(g))?;
        let gs = gs.as_dict()?;
        Some((gs.get(b"CA")?.as_f64()?, gs.get(b"ca")?.as_f64()?))
    };
    let mut doc = fixture();
    let r = add_annotation(&mut doc, &new(0, Shape::Rectangle { rect: [10.0, 10.0, 110.0, 60.0] }), &meta("r")).unwrap();
    set_fill(&mut doc, 0, r, Some([1.0, 1.0, 0.0]), &meta("")).unwrap();
    set_part_opacity(&mut doc, 0, r, Some(0.3), None, &meta("")).unwrap();
    let doc2 = reopen(&doc);
    let d = &list(&doc2, 0)[r];
    assert_eq!((num(d, b"CA"), num(d, b"ca")), (None, Some(0.3)), "an opaque line, a fill of its own");
    assert_eq!(gstate(&doc2, d, b"GS0"), Some((1.0, 0.3)));
    let ap = ap_content(&doc2, d);
    assert!(ap.starts_with("/GS0 gs") && ap.contains("\nB\n"), "filled and stroked in one go: {ap}");
    let p = props(&doc2, 0, r).unwrap();
    assert_eq!((p.opacity, p.fill_opacity, p.text_opacity), (1.0, Some(0.3), None));
    // The whole comment's opacity (StyleAnnotation, the quick bar): the fill goes along again.
    set_style(&mut doc, 0, r, StyleChange::new(None, Some(0.6), None, None), &meta("")).unwrap();
    let d = &list(&doc, 0)[r];
    assert_eq!((num(d, b"CA"), num(d, b"ca")), (Some(0.6), None));
    assert_eq!(props(&doc, 0, r).unwrap().fill_opacity, Some(0.6));

    // A text box with its border opaque, its background at 20 % and its text opaque, as made.
    let mut tb = new(0, Shape::TextBox { rect: [200.0, 600.0, 400.0, 650.0], font_size: 12.0 });
    tb.style.fill = Some([1.0, 1.0, 0.6]);
    tb.style.width = 1.0;
    (tb.style.fill_opacity, tb.style.text_opacity) = (Some(0.2), Some(1.0));
    let t = add_annotation(&mut doc, &tb, &meta("t")).unwrap();
    let doc2 = reopen(&doc);
    let d = &list(&doc2, 0)[t];
    assert_eq!((num(d, b"CA"), num(d, b"ca"), num(d, b"PCTextOpacity")), (None, Some(0.2), Some(1.0)));
    assert_eq!((gstate(&doc2, d, b"GS0"), gstate(&doc2, d, b"GS1")), (Some((1.0, 0.2)), Some((1.0, 1.0))));
    let ap = ap_content(&doc2, d);
    let (gs1, text) = (ap.find("/GS1 gs\n").expect("the text's own state, an operator of its own"), ap.find("BT").expect("text"));
    assert!(ap.find(" re f").is_some_and(|fill| fill < gs1) && gs1 < text, "background at 20 %, then the text at its own: {ap}");
    // A new fill opacity leaves the text as it is drawn.
    set_part_opacity(&mut doc, 0, t, Some(0.5), None, &meta("")).unwrap();
    let d = &list(&doc, 0)[t];
    assert_eq!((num(d, b"ca"), num(d, b"PCTextOpacity")), (Some(0.5), Some(1.0)));
    let p = props(&doc, 0, t).unwrap();
    assert_eq!((p.opacity, p.fill_opacity, p.text_opacity), (1.0, Some(0.5), Some(1.0)));
    // All at one opacity again: only /CA stays.
    set_part_opacity(&mut doc, 0, t, Some(1.0), Some(1.0), &meta("")).unwrap();
    let d = &list(&doc, 0)[t];
    assert_eq!((num(d, b"CA"), num(d, b"ca"), num(d, b"PCTextOpacity")), (None, None, None));

    // Only the parts a comment has, and finite values; nothing changes when refused.
    let l = add_annotation(
        &mut doc,
        &new(0, Shape::Line { from: [0.0, 0.0], to: [10.0, 10.0], start: LineEnding::None, end: LineEnding::None }),
        &meta("l"),
    )
    .unwrap();
    assert!(matches!(set_part_opacity(&mut doc, 0, l, Some(0.5), None, &meta("")), Err(AnnotError::Invalid(_))), "a line has no fill");
    assert!(matches!(set_part_opacity(&mut doc, 0, r, None, Some(0.5), &meta("")), Err(AnnotError::Invalid(_))), "a rectangle has no text");
    let before = list(&doc, 0)[r].clone();
    assert!(matches!(set_part_opacity(&mut doc, 0, r, Some(f64::NAN), None, &meta("")), Err(AnnotError::Invalid(_))));
    assert_eq!(list(&doc, 0)[r], before);
    // Another app's comment with /CA alone is drawn at it throughout, as before.
    assert_eq!(
        appearance::part_opacity(&Dict::from_iter([(b"CA".to_vec(), Object::Real(0.4))])),
        appearance::PartOpacity { line: 0.4, fill: 0.4, text: 0.4 }
    );
}

#[test]
fn shapes_can_be_filled_and_unfilled() {
    let mut doc = fixture();
    let r = add_annotation(&mut doc, &new(0, Shape::Rectangle { rect: [10.0, 10.0, 110.0, 60.0] }), &meta("r")).unwrap();
    let p = props(&doc, 0, r).unwrap();
    assert!(p.fillable);
    assert_eq!(p.fill, None);
    set_fill(&mut doc, 0, r, Some([0.0, 0.5, 1.0]), &meta("")).unwrap();
    let doc2 = reopen(&doc);
    let d = &list(&doc2, 0)[r];
    let ap = ap_content(&doc2, d);
    assert!(ap.contains("0 0.5 1 rg") && ap.contains("\nB\n"), "filled and stroked: {ap}");
    assert_eq!(props(&doc2, 0, r).unwrap().fill, Some([0.0, 0.5, 1.0]));
    // No fill again: /IC goes and the appearance only strokes.
    set_fill(&mut doc, 0, r, None, &meta("")).unwrap();
    let doc2 = reopen(&doc);
    let d = &list(&doc2, 0)[r];
    assert!(d.get(b"IC").is_none());
    let ap = ap_content(&doc2, d);
    assert!(!ap.contains(" rg") && ap.contains("\nS\n"), "stroked only: {ap}");
    assert_eq!(props(&doc2, 0, r).unwrap().fill, None);
    // Ovals and polygons fill too.
    let o = add_annotation(&mut doc, &new(0, Shape::Oval { rect: [10.0, 10.0, 60.0, 60.0] }), &meta("o")).unwrap();
    set_fill(&mut doc, 0, o, Some([1.0, 1.0, 0.0]), &meta("")).unwrap();
    assert!(ap_content(&doc, &list(&doc, 0)[o]).contains("1 1 0 rg"));
    let vertices = vec![[0.0, 0.0], [50.0, 0.0], [25.0, 40.0]];
    let g = add_annotation(&mut doc, &new(0, Shape::Polygon { vertices, cloud: false }), &meta("g")).unwrap();
    set_fill(&mut doc, 0, g, Some([1.0, 0.0, 0.0]), &meta("")).unwrap();
    assert!(ap_content(&doc, &list(&doc, 0)[g]).contains("1 0 0 rg"));
    // Lines have no interior, and bad colours change nothing.
    let l = add_annotation(
        &mut doc,
        &new(0, Shape::Line { from: [0.0, 0.0], to: [10.0, 10.0], start: LineEnding::None, end: LineEnding::None }),
        &meta("l"),
    )
    .unwrap();
    assert!(!props(&doc, 0, l).unwrap().fillable);
    assert!(matches!(set_fill(&mut doc, 0, l, Some([1.0; 3]), &meta("")), Err(AnnotError::Invalid(_))));
    let before = list(&doc, 0)[r].clone();
    assert!(matches!(set_fill(&mut doc, 0, r, Some([f64::NAN, 0.0, 0.0]), &meta("")), Err(AnnotError::Invalid(_))));
    assert_eq!(list(&doc, 0)[r], before);
    assert!(set_fill(&mut doc, 0, 99, None, &meta("")).is_err());
}

#[test]
fn text_box_text_and_colour_changes_are_drawn() {
    let mut doc = fixture();
    let t = add_annotation(&mut doc, &new(0, Shape::TextBox { rect: [0.0, 0.0, 300.0, 100.0], font_size: 12.0 }), &meta("t")).unwrap();
    set_contents(&mut doc, 0, t, "Caf\u{e9} (draft) \u{2014} 100%", &meta("")).unwrap();
    set_style(&mut doc, 0, t, StyleChange::new(Some([1.0, 0.0, 0.0]), None, None, None), &meta("")).unwrap();
    let doc = reopen(&doc);
    let d = &list(&doc, 0)[t];
    assert_eq!(text(d, b"Contents"), "Caf\u{e9} (draft) \u{2014} 100%");
    assert_eq!(text(d, b"DA"), "1 0 0 rg /Helv 12 Tf");
    let n = d.get(b"AP").unwrap().as_dict().unwrap().reference(b"N").unwrap();
    let Object::Stream(s) = &*doc.get(n) else { panic!() };
    let raw = s.decoded().unwrap();
    // WinAnsi bytes, with the parentheses escaped.
    let needle = b"(Caf\xe9 \\(draft\\) \x97 100%) Tj";
    assert!(raw.windows(needle.len()).any(|w| w == needle), "{}", String::from_utf8_lossy(&raw));
    assert!(String::from_utf8_lossy(&raw).contains("1 0 0 rg"));
    // Rich text follows the plain text and the style.
    assert_eq!(text(d, b"DS"), "font: Helvetica 12pt; text-align:left; color:#FF0000");
    let rc = text(d, b"RC");
    assert!(rc.starts_with("<?xml") && rc.contains("<p dir=\"ltr\">Caf\u{e9} (draft) \u{2014} 100%</p>") && rc.contains("color:#FF0000"), "{rc}");
}

/// The rectangle a text box needs for `text` (the appearance's padding plus the 2 pt slack):
/// the wrap width and top edge stay, the height fits the wrapped lines.
fn expected_box(top: f64, w: f64, text: &str, size: f64, border: f64) -> [f64; 4] {
    let pad = 2.0 + border;
    let lines = appearance::wrap(text, size, w - 2.0 * pad).len().max(1) as f64;
    [0.0, top - lines * size * 1.2 - 2.0 * pad - 2.0, w, 0.0]
}

#[test]
fn editing_a_text_box_refits_its_rectangle() {
    let mut doc = fixture();
    let t = add_annotation(&mut doc, &new(0, Shape::TextBox { rect: [100.0, 680.0, 304.0, 692.0], font_size: 12.0 }), &meta("t")).unwrap();
    // The style's border is 2 pt wide, so the text is padded by 2 + 2.
    let long = "the quick brown fox jumps over the lazy dog and keeps going";
    set_contents(&mut doc, 0, t, long, &meta("")).unwrap();
    let d = &list(&doc, 0)[t];
    let r = rect(d);
    assert_eq!((r[0], r[2], r[3]), (100.0, 304.0, 692.0), "width and top edge stay");
    let want = expected_box(692.0, 204.0, long, 12.0, 2.0);
    assert_eq!(r[1], want[1], "the height fits the wrapped lines");
    // The appearance covers the new rectangle, and every wrapped line is drawn.
    let n = d.get(b"AP").unwrap().as_dict().unwrap().reference(b"N").unwrap();
    let bbox: Vec<f64> = doc.get(n).as_dict().unwrap().get(b"BBox").unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect();
    assert_eq!(bbox, r);
    assert_eq!(ap_content(&doc, d).matches(" Tj").count(), appearance::wrap(long, 12.0, 204.0 - 2.0 * 4.0).len(), "nothing is clipped");
    // Shorter text shrinks the box back to one line.
    set_contents(&mut doc, 0, t, "Hi", &meta("")).unwrap();
    let r = rect(&list(&doc, 0)[t]);
    assert_eq!(r, [100.0, expected_box(692.0, 204.0, "Hi", 12.0, 2.0)[1], 304.0, 692.0]);
}

#[test]
fn editing_a_callout_refits_the_box_and_keeps_the_leader() {
    let mut doc = fixture();
    let c = add_annotation(
        &mut doc,
        &new(
            0,
            Shape::Callout {
                rect: [300.0, 600.0, 500.0, 612.0],
                knee: [260.0, 560.0],
                point: [200.0, 520.0],
                font_size: 12.0,
                ending: LineEnding::OpenArrow,
            },
        ),
        &meta("c"),
    )
    .unwrap();
    let long = "the quick brown fox jumps over the lazy dog and keeps going";
    set_contents(&mut doc, 0, c, long, &meta("")).unwrap();
    let d = &list(&doc, 0)[c];
    let outer = rect(d);
    let f = |k: &[u8]| -> Vec<f64> { d.get(k).unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect() };
    let cl = f(b"CL");
    // The box grows down from its top; the side the line meets stays where it was, so the line
    // doesn't change (as Acrobat keeps it while a box is resized from its other corner).
    let want = expected_box(612.0, 200.0, long, 12.0, 2.0);
    assert_eq!(cl, [200.0, 520.0, 260.0, 560.0, 300.0, 606.0]);
    // The text box (recovered from /Rect and /RD) keeps its width and top edge.
    let tb = [outer[0] + f(b"RD")[0], outer[1] + f(b"RD")[1], outer[2] - f(b"RD")[2], outer[3] - f(b"RD")[3]];
    assert_eq!((tb[0], tb[2], tb[3]), (300.0, 500.0, 612.0));
    assert_eq!(tb[1], want[1]);
    // /Rect still grows to hold the leader line: its padded bounds are [186, 506, 274, 574].
    assert_eq!((outer[0], outer[2], outer[3]), (186.0, 500.0, 612.0));
    assert_eq!(ap_content(&doc, d).matches(" Tj").count(), appearance::wrap(long, 12.0, 200.0 - 2.0 * 4.0).len(), "nothing is clipped");
}

#[test]
fn real_annotation_flags_do_not_lock_comments() {
    let mut doc = fixture();
    let page = page_refs(&doc).unwrap()[0];
    let mut entries = annots(&doc, page);
    let Object::Dict(annotation) = &mut entries[0] else { panic!("fixture annotation is inline") };
    annotation.set(b"F".to_vec(), Object::Real(128.5));
    set_annots(&mut doc, page, entries).unwrap();

    assert!(!summaries(&doc)[0].locked, "a non-integer /F is outside the annotation flag domain");
    assert!(!props(&doc, 0, 0).unwrap().locked);
    assert!(set_info(&mut doc, 0, 0, Some("Ada"), None, None, &meta("")).is_ok());
}

#[test]
fn carriage_returns_in_comment_text_become_line_breaks() {
    let mut doc = fixture();
    let page = page_refs(&doc).unwrap()[0];
    let mut entries = annots(&doc, page);
    let Object::Dict(annotation) = &mut entries[0] else { panic!("fixture annotation is inline") };
    // Acrobat ends lines in /Contents with CR; the interface only breaks lines at LF.
    annotation.set(b"Contents".to_vec(), PdfString::text("one\r\rtwo\r\nthree"));
    set_annots(&mut doc, page, entries).unwrap();

    assert_eq!(summaries(&doc)[0].contents.as_deref(), Some("one\n\ntwo\nthree"));
}

#[test]
fn a_locked_text_box_still_refits_its_text() {
    let mut doc = fixture();
    let t = add_annotation(&mut doc, &new(0, Shape::TextBox { rect: [100.0, 680.0, 304.0, 692.0], font_size: 12.0 }), &meta("t")).unwrap();
    set_locked(&mut doc, 0, t, true).unwrap();
    // Resizing is refused, but the text (and its re-fit) stays editable, as in Acrobat.
    assert!(matches!(set_rect(&mut doc, 0, t, [0.0, 0.0, 50.0, 50.0], &meta("")), Err(AnnotError::Invalid(_))));
    let long = "the quick brown fox jumps over the lazy dog";
    set_contents(&mut doc, 0, t, long, &meta("")).unwrap();
    let r = rect(&list(&doc, 0)[t]);
    assert_eq!(r[3], 692.0);
    let want = expected_box(692.0, 204.0, long, 12.0, 2.0);
    assert_eq!(r[1], want[1], "the box grew to fit the wrapped lines");
}

#[test]
fn unknown_keys_survive_edits() {
    let mut doc = fixture();
    move_annotation(&mut doc, 1, 0, 1.0, 1.0, &meta("")).unwrap();
    set_contents(&mut doc, 1, 0, "note", &meta("")).unwrap();
    let doc = reopen(&doc);
    let d = &list(&doc, 1)[0];
    assert_eq!(text(d, b"Foo"), "kept");
    assert_eq!(d.name(b"Name"), Some(&b"Approved"[..]));
    assert_eq!(rect(d), [101.0, 101.0, 201.0, 151.0]);
}

#[test]
fn wrapping_and_widths() {
    assert!(appearance::text_width("MMMM", 10.0) > appearance::text_width("iiii", 10.0) * 2.0);
    let lines = appearance::wrap("the quick brown fox jumps over the lazy dog", 12.0, 80.0);
    assert!(lines.len() > 2);
    assert!(lines.iter().all(|l| appearance::text_width(l, 12.0) <= 80.0));
    assert_eq!(appearance::wrap("a\nb", 12.0, 100.0), ["a", "b"]);
    // A word longer than the line is broken.
    let long = appearance::wrap("Supercalifragilisticexpialidocious", 12.0, 40.0);
    assert!(long.len() > 3 && long.concat() == "Supercalifragilisticexpialidocious");
    assert_eq!(n(1.0), "1");
    assert_eq!(n(-0.0001), "0");
    assert_eq!(n(2.50), "2.5");
}

#[test]
fn fill_and_sign_items_are_drawn() {
    let mut doc = fixture();
    let add = |doc: &mut Document, shape: Shape, contents: &str| {
        let style = Style::default_for(&shape);
        add_annotation(doc, &NewAnnotation { page: 1, shape, style, contents: contents.into(), author: "Ada".into() }, &meta("x")).unwrap()
    };
    let t = add(&mut doc, Shape::Typewriter { rect: [100.0, 700.0, 200.0, 716.0], font_size: 11.0 }, "Ada Lovelace");
    let marks: Vec<usize> = [FillMark::Check, FillMark::Cross, FillMark::Dot, FillMark::Line]
        .into_iter()
        .enumerate()
        .map(|(k, m)| add(&mut doc, Shape::Mark { rect: [100.0 + 20.0 * k as f64, 600.0, 114.0 + 20.0 * k as f64, 614.0], mark: m }, ""))
        .collect();
    let s = add(&mut doc, Shape::Signature { strokes: vec![vec![[10.0, 10.0], [30.0, 20.0], [50.0, 10.0]]] }, "");
    let doc = reopen(&doc);
    let all = list(&doc, 1);
    assert_eq!(all[t].name(b"IT"), Some(&b"FreeTextTypeWriter"[..]));
    assert!(ap_content(&doc, &all[t]).contains("(Ada Lovelace) Tj"));
    for (k, m) in marks.iter().enumerate() {
        assert_eq!(all[*m].name(b"Subtype"), Some(&b"Stamp"[..]));
        let ap = ap_content(&doc, &all[*m]);
        assert!(ap.contains(if k == 2 { " c" } else { " l" }), "mark {k}: {ap}");
    }
    assert_eq!(text(&all[marks[0]], b"Subj"), "Checkmark");
    assert_eq!(text(&all[s], b"Subj"), "Signature");
    // Other stamps still can't be restyled; ours can.
    let mut doc = doc;
    set_style(&mut doc, 1, marks[0], StyleChange::new(Some([0.0, 0.0, 1.0]), None, None, None), &meta("")).unwrap();
    assert!(set_style(&mut doc, 1, 0, StyleChange::new(Some([0.0; 3]), None, None, None), &meta("")).is_err());
}

/// A border chosen for typewriter text goes around it in the text's colour. Changing it grows or
/// shrinks the box on every side, so the text stays where it was typed; a text box keeps its box.
#[test]
fn a_typewriter_border_goes_around_its_text_which_stays_put() {
    let mut doc = fixture();
    let shape = Shape::Typewriter { rect: [100.0, 700.0, 200.0, 716.0], font_size: 11.0 };
    let style = Style { width: 1.0, ..Style::default_for(&shape) };
    let t = add_annotation(&mut doc, &NewAnnotation { page: 1, shape, style, contents: "Ada".into(), author: "Ada".into() }, &meta("x")).unwrap();
    // The text starts 2 pt inside the border: 100 + 1 + 2 across, 716 - 1 - 2 - 0.9 em down.
    let at = "1 0 0 1 103 703.1 Tm";
    let d = &list(&doc, 1)[t];
    let ap = ap_content(&doc, d);
    assert!(ap.contains("1 w\n") && ap.contains(" re S"), "a 1 pt border: {ap}");
    assert!(ap.contains(at), "{ap}");
    for (width, expected) in [(2.0, [99.0, 699.0, 201.0, 717.0]), (0.0, [101.0, 701.0, 199.0, 715.0])] {
        set_style(&mut doc, 1, t, StyleChange::new(None, None, Some(width), None), &meta("")).unwrap();
        let d = &list(&doc, 1)[t];
        assert_eq!(rect(d), expected, "{width} pt border");
        let ap = ap_content(&doc, d);
        assert!(ap.contains(at), "{width} pt: the text keeps its place: {ap}");
        assert_eq!(ap.contains(" re S"), width > 0.0, "{width} pt: {ap}");
    }
    let shape = Shape::TextBox { rect: [100.0, 600.0, 200.0, 650.0], font_size: 11.0 };
    let b = add_annotation(&mut doc, &new(1, shape), &meta("b")).unwrap();
    set_style(&mut doc, 1, b, StyleChange::new(None, None, Some(3.0), None), &meta("")).unwrap();
    assert_eq!(rect(&list(&doc, 1)[b]), [100.0, 600.0, 200.0, 650.0], "a text box keeps the box it was drawn with");
    // Another app's typed text with its width in /Border (no /BS): 4 pt, so 2 pt shrinks the box
    // by 2 on every side, as it is drawn.
    let shape = Shape::Typewriter { rect: [100.0, 500.0, 200.0, 530.0], font_size: 11.0 };
    let o = add_annotation(
        &mut doc,
        &NewAnnotation { page: 1, style: Style::default_for(&shape), shape: shape.clone(), contents: "Bo".into(), author: "Ada".into() },
        &meta("o"),
    )
    .unwrap();
    let (_, r) = annot_ref(&mut doc, 1, o).unwrap();
    doc.update_dict(r, |d| {
        d.remove(b"BS");
        d.set(b"Border".to_vec(), num_array(&[0.0, 0.0, 4.0]));
    })
    .unwrap();
    set_style(&mut doc, 1, o, StyleChange::new(None, None, Some(2.0), None), &meta("")).unwrap();
    assert_eq!(rect(&list(&doc, 1)[o]), [102.0, 502.0, 198.0, 528.0], "the imported 4 pt border is read from /Border");
    // A border colour that isn't a colour is refused before anything is written.
    let mut bad = NewAnnotation { page: 1, style: Style::default_for(&shape), shape, contents: "x".into(), author: "Ada".into() };
    bad.style.border_color = Some([f64::NAN, 0.0, 0.0]);
    let before = list(&doc, 1).len();
    assert!(matches!(add_annotation(&mut doc, &bad, &meta("n")), Err(AnnotError::Invalid(_))));
    assert_eq!(list(&doc, 1).len(), before);
}

/// Text keeps its colour and the border gets its own: the stroking colour in `/DA`, which a new
/// text colour or font size keeps, and which `None` gives back to the text's.
#[test]
fn a_text_border_can_have_its_own_colour() {
    let mut doc = fixture();
    let shape = Shape::Typewriter { rect: [100.0, 700.0, 200.0, 716.0], font_size: 11.0 };
    let style = Style { color: [0.0, 0.5, 0.0], width: 1.0, border_color: Some([1.0, 0.0, 0.0]), ..Style::default_for(&shape) };
    let t = add_annotation(&mut doc, &NewAnnotation { page: 1, shape, style, contents: "Ada".into(), author: "Ada".into() }, &meta("x")).unwrap();
    let da = |doc: &Document| text(&list(doc, 1)[t], b"DA");
    assert_eq!(da(&doc), "0 0.5 0 rg 1 0 0 RG /Helv 11 Tf");
    let ap = ap_content(&doc, &list(&doc, 1)[t]);
    assert!(ap.contains("1 0 0 RG\n1 w"), "the border in red: {ap}");
    assert!(ap.contains("0 0.5 0 rg"), "the text in green: {ap}");
    assert_eq!(props(&doc, 1, t).unwrap().border_color, Some([1.0, 0.0, 0.0]));
    set_style(&mut doc, 1, t, StyleChange::new(Some([0.0, 0.0, 1.0]), None, None, None), &meta("")).unwrap();
    assert_eq!(da(&doc), "0 0 1 rg 1 0 0 RG /Helv 11 Tf", "a new text colour keeps the border's");
    set_font_size(&mut doc, 1, t, 14.0, &meta("")).unwrap();
    assert_eq!(da(&doc), "0 0 1 rg 1 0 0 RG /Helv 14 Tf", "so does a new size");
    set_border_color(&mut doc, 1, t, Some([0.5, 0.5, 0.5]), &meta("")).unwrap();
    assert!(ap_content(&doc, &list(&doc, 1)[t]).contains("0.5 0.5 0.5 RG\n1 w"));
    set_border_color(&mut doc, 1, t, None, &meta("")).unwrap();
    assert_eq!(da(&doc), "0 0 1 rg /Helv 14 Tf");
    assert!(ap_content(&doc, &list(&doc, 1)[t]).contains("0 0 1 RG\n1 w"), "the text's colour again");
    assert_eq!(props(&doc, 1, t).unwrap().border_color, None);
    let m = add_annotation(&mut doc, &new(0, Shape::Rectangle { rect: [10.0, 10.0, 50.0, 50.0] }), &meta("m")).unwrap();
    assert!(set_border_color(&mut doc, 0, m, Some([1.0, 0.0, 0.0]), &meta("")).is_err(), "a rectangle's colour is its border");
}

#[test]
fn author_subject_and_note_icon_change() {
    let mut doc = fixture();
    let n = add_annotation(&mut doc, &new(0, Shape::Note { at: [10.0, 100.0], icon: NoteIcon::Comment }), &meta("n")).unwrap();
    let before = ap_content(&doc, &list(&doc, 0)[n]);
    set_info(&mut doc, 0, n, Some("Grace"), Some("Question"), Some(NoteIcon::Help), &meta("")).unwrap();
    let d = &list(&doc, 0)[n];
    assert_eq!((text(d, b"T").as_str(), text(d, b"Subj").as_str()), ("Grace", "Question"));
    assert_eq!(d.name(b"Name"), Some(&b"Help"[..]));
    assert_ne!(ap_content(&doc, d), before, "the icon was redrawn");
    assert!(matches!(set_info(&mut doc, 0, 0, None, None, Some(NoteIcon::Key), &meta("")), Err(AnnotError::Invalid(_))));
}

#[test]
fn stamps_draw_their_label_and_by_line() {
    let mut doc = fixture();
    let shape =
        Shape::Stamp { rect: [100.0, 100.0, 260.0, 142.0], stamp: StampKind::DynApproved, by: Some("By Ada at 2:14 pm, Oct 02, 2026".into()) };
    let i = add_annotation(
        &mut doc,
        &NewAnnotation { page: 0, style: Style::default_for(&shape), shape, contents: String::new(), author: "Ada".into() },
        &Meta::default(),
    )
    .unwrap();
    let s = &summaries(&doc)[0];
    assert_eq!((s.subtype.as_str(), s.index), ("Stamp", i));
    let ap = ap_content(&doc, &list(&doc, 0)[i]);
    assert!(ap.contains("(APPROVED) Tj") && ap.contains("(By Ada at 2:14 pm, Oct 02, 2026) Tj") && ap.contains("/HelvB"), "{ap}");
    let sign = Shape::Stamp { rect: [100.0, 300.0, 220.0, 330.0], stamp: StampKind::SignHere, by: None };
    let j = add_annotation(
        &mut doc,
        &NewAnnotation { page: 0, style: Style::default_for(&sign), shape: sign, contents: String::new(), author: "Ada".into() },
        &Meta::default(),
    )
    .unwrap();
    assert!(ap_content(&doc, &list(&doc, 0)[j]).contains("(SIGN HERE) Tj"));
    assert_eq!(StampKind::from_name(b"Approved"), Some(StampKind::Approved), "standard names");
    assert!(StampKind::ALL.iter().all(|k| k.size().0 >= 80.0));
}

#[test]
fn links_are_added_edited_listed_and_removed() {
    use crate::links::{self, Highlight, LinkAction, LinkStyle, LinkZoom};
    let mut doc = fixture();
    let before = links::list(&doc).len();
    let i = links::add(&mut doc, 0, [10.0, 10.0, 110.0, 30.0], &LinkAction::Uri("https://example.org".into()), &LinkStyle::default()).unwrap();
    let style = LinkStyle { visible: true, color: [1.0, 0.0, 0.0], width: 2.0, dashed: true, underline: false, highlight: Highlight::Outline };
    links::add(&mut doc, 1, [10.0, 10.0, 60.0, 30.0], &LinkAction::Page { page: 0, zoom: LinkZoom::FitPage }, &style).unwrap();
    let doc = reopen(&doc);
    let all = links::list(&doc);
    assert_eq!(all.len(), before + 2);
    let web = all.iter().find(|l| l.page == 0 && l.index == i).unwrap();
    assert_eq!(
        (web.action.clone(), web.style.visible, web.style.highlight),
        (LinkAction::Uri("https://example.org".into()), false, Highlight::Invert)
    );
    let page = all.iter().find(|l| l.page == 1 && l.action == LinkAction::Page { page: 0, zoom: LinkZoom::FitPage }).unwrap().clone();
    assert_eq!(page.style, style);
    let mut doc = doc;
    links::set(&mut doc, 1, page.index, Some([0.0, 0.0, 50.0, 20.0]), Some(&LinkAction::Uri("mailto:ada@example.org".into())), None).unwrap();
    let changed = links::list(&doc).into_iter().find(|l| l.page == 1 && l.index == page.index).unwrap();
    assert_eq!((changed.rect, changed.action), ([0.0, 0.0, 50.0, 20.0], LinkAction::Uri("mailto:ada@example.org".into())));
    links::delete(&mut doc, 1, page.index).unwrap();
    assert!(links::delete(&mut doc, 0, 999).is_err(), "no such annotation");
    let n = links::remove_all(&mut doc, None).unwrap();
    assert!(n >= 1 && links::list(&doc).is_empty());
    assert!(
        links::add(&mut doc, 0, [0.0, 0.0, 1.0, 1.0], &LinkAction::Page { page: 0, zoom: LinkZoom::FitPage }, &LinkStyle::default()).is_err(),
        "too small"
    );
}

#[test]
fn page_links_keep_their_zoom() {
    use crate::links::{self, LinkAction, LinkStyle, LinkZoom};
    let page1 = page_refs(&fixture()).unwrap()[1];
    let dest_of = |doc: &Document, obj: ObjRef| doc.get(obj).as_dict().and_then(|d| d.get(b"Dest").cloned());
    // Each zoom is written as its destination type (Table 149) and reads back after a save.
    let null = Object::Null;
    for (zoom, written) in [
        (LinkZoom::FitPage, vec![Object::name("Fit")]),
        (LinkZoom::FitWidth, vec![Object::name("FitH"), null.clone()]),
        (LinkZoom::FitHeight, vec![Object::name("FitV"), null.clone()]),
        (LinkZoom::FitVisible, vec![Object::name("FitB")]),
        (LinkZoom::Inherit, vec![Object::name("XYZ"), null.clone(), null.clone(), null.clone()]),
        (LinkZoom::Percent(100.0), vec![Object::name("XYZ"), null.clone(), null.clone(), Object::Real(1.0)]),
        (LinkZoom::Percent(250.0), vec![Object::name("XYZ"), null.clone(), null.clone(), Object::Real(2.5)]),
    ] {
        let mut doc = fixture();
        let i = links::add(&mut doc, 0, [10.0, 10.0, 60.0, 30.0], &LinkAction::Page { page: 1, zoom }, &LinkStyle::default()).unwrap();
        let link = links::list(&doc).into_iter().find(|l| l.page == 0 && l.index == i).unwrap();
        let mut expected = vec![Object::Ref(page1)];
        expected.extend(written);
        assert_eq!(dest_of(&doc, link.obj.unwrap()), Some(Object::Array(expected)), "{zoom:?}");
        let doc = reopen(&doc);
        let link = links::list(&doc).into_iter().find(|l| l.page == 0 && l.index == i).unwrap();
        assert_eq!(link.action, LinkAction::Page { page: 1, zoom }, "{zoom:?} after a save");
    }
    // What other writers use reads as the nearest zoom: a zero zoom keeps the reader's, the boxed
    // fits as the plain ones, a rectangle as Fit Page, and a GoTo action like a /Dest.
    let mut doc = fixture();
    let i = links::add(&mut doc, 0, [10.0, 10.0, 60.0, 30.0], &LinkAction::Page { page: 1, zoom: LinkZoom::FitPage }, &LinkStyle::default()).unwrap();
    let obj = links::list(&doc).into_iter().find(|l| l.page == 0 && l.index == i).unwrap().obj.unwrap();
    let dest = |kind: &str, operands: Vec<Object>| {
        let mut d = vec![Object::Ref(page1), Object::name(kind)];
        d.extend(operands);
        Object::Array(d)
    };
    for (key, value, zoom) in [
        ("Dest", dest("XYZ", vec![Object::Int(100), Object::Int(700), Object::Int(0)]), LinkZoom::Inherit),
        ("Dest", dest("FitBH", vec![Object::Int(500)]), LinkZoom::FitWidth),
        ("Dest", dest("FitBV", vec![Object::Int(20)]), LinkZoom::FitHeight),
        ("Dest", dest("FitR", vec![Object::Int(0), Object::Int(0), Object::Int(10), Object::Int(10)]), LinkZoom::FitPage),
        ("A", dest("XYZ", vec![null.clone(), null.clone(), Object::Real(1.5)]), LinkZoom::Percent(150.0)),
        // A factor outside what PdfCraft writes reads as the destination has it, not capped, so
        // it agrees with the viewer's `destination`; one whose percentage overflows stays finite.
        ("Dest", dest("XYZ", vec![null.clone(), null.clone(), Object::Real(100.0)]), LinkZoom::Percent(10000.0)),
        ("Dest", dest("XYZ", vec![null.clone(), null.clone(), Object::Real(0.005)]), LinkZoom::Percent(0.5)),
        ("Dest", dest("XYZ", vec![null.clone(), null.clone(), Object::Real(1e307)]), LinkZoom::Percent(f64::MAX)),
    ] {
        let mut d = doc.get(obj).as_dict().cloned().unwrap();
        d.remove(b"Dest");
        d.remove(b"A");
        if key == "A" {
            let mut go_to = pdfcraft_cos::Dict::new();
            go_to.set(b"S".to_vec(), Object::name("GoTo"));
            go_to.set(b"D".to_vec(), value);
            d.set(b"A".to_vec(), Object::Dict(go_to));
        } else {
            d.set(b"Dest".to_vec(), value);
        }
        doc.set(obj, Object::Dict(d));
        let link = links::list(&doc).into_iter().find(|l| l.page == 0 && l.index == i).unwrap();
        assert_eq!(link.action, LinkAction::Page { page: 1, zoom }, "{key}");
    }
    // A zoom that isn't a positive percentage is refused before anything is written.
    for p in [0.0, -50.0, f64::NAN, f64::INFINITY] {
        let before = links::list(&doc).len();
        let refused =
            links::add(&mut doc, 0, [10.0, 10.0, 60.0, 30.0], &LinkAction::Page { page: 1, zoom: LinkZoom::Percent(p) }, &LinkStyle::default());
        assert!(refused.is_err(), "{p}");
        assert_eq!(links::list(&doc).len(), before, "{p}: no link added");
    }
}

/// A link rewritten with the zoom it has keeps its own `/XYZ` factor, however far outside the
/// 1–6400% a new zoom is written within: giving it a new page (Link Properties' OK, `link_edit`
/// without `zoom`) must not change its zoom. A zoom chosen anew is still written within 1–6400%.
#[test]
fn a_rewritten_link_keeps_its_own_zoom_factor() {
    use crate::links::{self, LinkAction, LinkStyle, LinkZoom};
    let pages = page_refs(&fixture()).unwrap();
    let dest_of = |doc: &Document, obj: ObjRef| doc.get(obj).as_dict().and_then(|d| d.get(b"Dest").cloned());
    let xyz = |page: ObjRef, factor: Object| Object::Array(vec![Object::Ref(page), Object::name("XYZ"), Object::Null, Object::Null, factor]);
    for (key, factor) in [
        ("Dest", Object::Real(0.005)),
        ("Dest", Object::Real(100.0)),
        ("Dest", Object::Int(100)),
        ("Dest", Object::Real(1e307)),
        ("A", Object::Real(0.005)),
    ] {
        let mut doc = fixture();
        let i =
            links::add(&mut doc, 0, [10.0, 10.0, 60.0, 30.0], &LinkAction::Page { page: 1, zoom: LinkZoom::FitPage }, &LinkStyle::default()).unwrap();
        let obj = links::list(&doc).into_iter().find(|l| l.page == 0 && l.index == i).unwrap().obj.unwrap();
        let mut d = doc.get(obj).as_dict().cloned().unwrap();
        d.remove(b"Dest");
        if key == "A" {
            let mut go_to = pdfcraft_cos::Dict::new();
            go_to.set(b"S".to_vec(), Object::name("GoTo"));
            go_to.set(b"D".to_vec(), xyz(pages[1], factor.clone()));
            d.set(b"A".to_vec(), Object::Dict(go_to));
        } else {
            d.set(b"Dest".to_vec(), xyz(pages[1], factor.clone()));
        }
        doc.set(obj, Object::Dict(d));
        let LinkAction::Page { zoom, .. } = links::list(&doc).into_iter().find(|l| l.page == 0 && l.index == i).unwrap().action else {
            panic!("{key} {factor:?}")
        };
        links::set(&mut doc, 0, i, None, Some(&LinkAction::Page { page: 0, zoom }), None).unwrap();
        assert_eq!(dest_of(&doc, obj), Some(xyz(pages[0], factor.clone())), "{key} {factor:?}: the factor is the link's own");
        let doc = reopen(&doc);
        assert_eq!(
            links::list(&doc).into_iter().find(|l| l.page == 0 && l.index == i).unwrap().action,
            LinkAction::Page { page: 0, zoom },
            "{key} {factor:?}"
        );
    }
    // A zoom other than the link's is written within 1–6400%.
    let mut doc = fixture();
    let i = links::add(&mut doc, 0, [10.0, 10.0, 60.0, 30.0], &LinkAction::Page { page: 1, zoom: LinkZoom::Percent(50.0) }, &LinkStyle::default())
        .unwrap();
    let obj = links::list(&doc).into_iter().find(|l| l.page == 0 && l.index == i).unwrap().obj.unwrap();
    links::set(&mut doc, 0, i, None, Some(&LinkAction::Page { page: 1, zoom: LinkZoom::Percent(10000.0) }), None).unwrap();
    assert_eq!(dest_of(&doc, obj), Some(xyz(pages[1], Object::Real(64.0))));
}

/// A go-to edited without a change (its own page and zoom, as `link_edit` naming its page does)
/// is left as it is: the `/XYZ` position, a fit's position and the GoTo form stay. Given a new
/// page with its zoom, a link keeps how that zoom is written, a boxed fit (`/FitBH`, `/FitBV`)
/// included, but not its position, which belongs to the old page.
#[test]
fn an_unchanged_go_to_keeps_its_destination_and_a_new_page_keeps_its_fit() {
    use crate::links::{self, LinkAction, LinkStyle, LinkZoom};
    let pages = page_refs(&fixture()).unwrap();
    let dest = |page: ObjRef, kind: &str, operands: Vec<Object>| {
        let mut d = vec![Object::Ref(page), Object::name(kind)];
        d.extend(operands);
        Object::Array(d)
    };
    let link_with = |key: &str, value: Object| {
        let mut doc = fixture();
        let i =
            links::add(&mut doc, 0, [10.0, 10.0, 60.0, 30.0], &LinkAction::Page { page: 1, zoom: LinkZoom::FitPage }, &LinkStyle::default()).unwrap();
        let obj = links::list(&doc).into_iter().find(|l| l.page == 0 && l.index == i).unwrap().obj.unwrap();
        let mut d = doc.get(obj).as_dict().cloned().unwrap();
        d.remove(b"Dest");
        if key == "A" {
            let mut go_to = pdfcraft_cos::Dict::new();
            go_to.set(b"S".to_vec(), Object::name("GoTo"));
            go_to.set(b"D".to_vec(), value);
            d.set(b"A".to_vec(), Object::Dict(go_to));
        } else {
            d.set(b"Dest".to_vec(), value);
        }
        doc.set(obj, Object::Dict(d));
        (doc, i, obj)
    };
    let (num, null) = (|n: i64| Object::Int(n), Object::Null);
    // The same page and zoom: nothing is rewritten.
    for (key, value) in [
        ("Dest", dest(pages[1], "XYZ", vec![num(72), num(500), Object::Real(2.0)])),
        ("Dest", dest(pages[1], "FitBH", vec![num(250)])),
        ("Dest", dest(pages[1], "FitV", vec![num(20)])),
        ("A", dest(pages[1], "XYZ", vec![num(72), num(500), Object::Real(0.005)])),
    ] {
        let (mut doc, i, obj) = link_with(key, value);
        let before = doc.get(obj).as_dict().cloned();
        let LinkAction::Page { page, zoom } = links::list(&doc).into_iter().find(|l| l.page == 0 && l.index == i).unwrap().action else {
            panic!("{key}")
        };
        links::set(&mut doc, 0, i, None, Some(&LinkAction::Page { page, zoom }), None).unwrap();
        assert_eq!(doc.get(obj).as_dict().cloned(), before, "{key} {zoom:?}: left as it is");
    }
    // A new page: the fit's kind and the factor stay, positions go.
    for (value, written) in [
        (dest(pages[1], "FitBH", vec![num(250)]), dest(pages[0], "FitBH", vec![null.clone()])),
        (dest(pages[1], "FitBV", vec![num(20)]), dest(pages[0], "FitBV", vec![null.clone()])),
        (dest(pages[1], "FitH", vec![num(250)]), dest(pages[0], "FitH", vec![null.clone()])),
        (
            dest(pages[1], "XYZ", vec![num(72), num(500), Object::Real(2.0)]),
            dest(pages[0], "XYZ", vec![null.clone(), null.clone(), Object::Real(2.0)]),
        ),
    ] {
        let (mut doc, i, obj) = link_with("Dest", value.clone());
        let LinkAction::Page { zoom, .. } = links::list(&doc).into_iter().find(|l| l.page == 0 && l.index == i).unwrap().action else {
            panic!("{value:?}")
        };
        links::set(&mut doc, 0, i, None, Some(&LinkAction::Page { page: 0, zoom }), None).unwrap();
        assert_eq!(doc.get(obj).as_dict().and_then(|d| d.get(b"Dest").cloned()), Some(written), "{value:?}");
    }
}

#[test]
fn link_uri_is_percent_encoded_to_7_bit_ascii() {
    use crate::links::{self, LinkAction, LinkStyle};
    let mut doc = fixture();
    for url in ["https://example.com/ação", "https://example.com/a%20b"] {
        links::add(&mut doc, 0, [10.0, 10.0, 110.0, 30.0], &LinkAction::Uri(url.into()), &LinkStyle::default()).unwrap();
    }
    let doc = reopen(&doc);
    let urls: Vec<String> = links::list(&doc)
        .into_iter()
        .filter_map(|link| match link.action {
            LinkAction::Uri(url) => Some(url),
            _ => None,
        })
        .collect();
    assert_eq!(urls, ["https://example.com/a%C3%A7%C3%A3o", "https://example.com/a%20b"]);
    assert!(urls.iter().all(|url| url.is_ascii()));
}

#[test]
fn link_edit_rejects_non_finite_and_overflowed_rectangles_without_mutating_the_pdf() {
    use crate::links::{self, LinkAction, LinkStyle};
    let mut doc = fixture();
    let original_rect = [10.0, 20.0, 110.0, 70.0];
    let index = links::add(&mut doc, 0, original_rect, &LinkAction::Uri("https://example.org".into()), &LinkStyle::default()).unwrap();
    let before = links::list(&doc);
    let changed_action = LinkAction::Uri("https://other.example".into());
    for rect in [
        [f64::NAN, 0.0, 100.0, 30.0],
        [0.0, 0.0, f64::INFINITY, 30.0],
        [f64::NEG_INFINITY, 0.0, 100.0, 30.0],
        [-1e308, 0.0, 1e308, 30.0],
        [0.0, 0.0, 1.5, 30.0],
    ] {
        assert!(links::set(&mut doc, 0, index, Some(rect), Some(&changed_action), None).is_err(), "edit accepted {rect:?}");
        assert_eq!(links::list(&doc), before, "invalid edit changed an existing link");
        assert!(links::add(&mut doc, 0, rect, &changed_action, &LinkStyle::default()).is_err(), "add accepted {rect:?}");
    }
    // A reversed but otherwise valid rectangle is normalized on edit.
    links::set(&mut doc, 0, index, Some([110.0, 70.0, 10.0, 20.0]), None, None).unwrap();
    let edited = links::list(&doc).into_iter().find(|l| l.page == 0 && l.index == index).map(|l| l.rect);
    assert_eq!(edited, Some(original_rect));
}

#[test]
fn urls_are_found_in_text() {
    let t: Vec<char> = "See https://example.org/a?b=1, or www.rust-lang.org. Not a.b or http:/x.".chars().collect();
    let found: Vec<String> = crate::links::find_urls(&t).into_iter().map(|(_, u)| u).collect();
    assert_eq!(found, ["https://example.org/a?b=1", "http://www.rust-lang.org"]);
}

#[test]
fn common_email_addresses_become_mailto_links_with_source_character_ranges() {
    let source = "İ Contact ada.lovelace+forms@example.co.uk, browse HTTPS://EXAMPLE.ORG; or ask admin@example.org.";
    let chars: Vec<char> = source.chars().collect();
    let links = crate::links::find_urls(&chars);
    let found: Vec<(String, String)> = links.into_iter().map(|(range, url)| (chars.get(range).unwrap().iter().collect(), url)).collect();
    assert_eq!(
        found,
        [
            ("ada.lovelace+forms@example.co.uk".into(), "mailto:ada.lovelace+forms@example.co.uk".into()),
            ("HTTPS://EXAMPLE.ORG".into(), "HTTPS://EXAMPLE.ORG".into()),
            ("admin@example.org".into(), "mailto:admin@example.org".into()),
        ]
    );
}

#[test]
fn malformed_email_addresses_are_not_autolinked() {
    for invalid in ["a@b", "broken@localhost", "a..b@example.org", "foo@.example.org", "foo@example..org", "foo@example.com@x"] {
        let input: Vec<char> = invalid.chars().collect();
        assert!(crate::links::find_urls(&input).is_empty(), "{invalid:?} produced a link");
    }
}

#[test]
fn urls_are_found_after_length_changing_lowercase() {
    // U+0130 (İ) lowercases to "i" plus a combining dot above, so a case-folded copy is longer
    // than the source. The old matcher took the address offset from that copy and sliced the
    // source with it, panicking at `chars[i..j]` (#816).
    let mut text = "\u{130}".repeat(20);
    text.push_str(" http://example.com");
    let chars: Vec<char> = text.chars().collect();
    let found = crate::links::find_urls(&chars);
    assert_eq!(found.len(), 1, "{found:?}");
    let (range, uri) = found.into_iter().next().unwrap();
    assert_eq!(uri, "http://example.com");
    assert_eq!(range, 21..39, "range is in source-character indices");
}

/// Review on #4472: an address inside one written with another scheme isn't taken out of it
/// (`ftp://www.example.com` opened as `http://www.example.com`), and brackets that pair up
/// inside an address belong to it.
#[test]
fn urls_keep_their_written_scheme_and_paired_brackets() {
    let found = |s: &str| -> Vec<String> { crate::links::find_urls(&s.chars().collect::<Vec<_>>()).into_iter().map(|(_, u)| u).collect() };
    for s in
        ["ftp://www.example.com/pub", "file://www.example.com/x", "javascript:www.example.com", "ftp://user@example.com/x", "smb://a@example.org/s"]
    {
        assert_eq!(found(s), Vec::<String>::new(), "{s}");
    }
    assert_eq!(found("mailto:someone@example.com and (www.example.com)"), ["mailto:someone@example.com", "http://www.example.com"]);
    assert_eq!(found("See https://en.wikipedia.org/wiki/Mercury_(planet)."), ["https://en.wikipedia.org/wiki/Mercury_(planet)"]);
    assert_eq!(found("(https://example.com/a_(b)_c)"), ["https://example.com/a_(b)_c"]);
    assert_eq!(found("https://example.com/a(b c"), ["https://example.com/a"]);
    // Deeply nested or unbalanced brackets end the address without trouble.
    let deep = format!("https://example.com/{}x{}", "(".repeat(10_000), ")".repeat(9_999));
    assert_eq!(found(&deep), ["https://example.com/"]);
}

/// Review on #4472: brackets that never close in text without spaces (`https://a/(` again and
/// again) made each address in it read the rest of the text, then start again one character on,
/// a pass over the page per address. Each is read 2,048 characters at most now, and the
/// addresses around them are still found. A run of address characters longer than that is no
/// address.
#[test]
fn unclosed_brackets_in_text_without_spaces_are_read_once_per_address() {
    let found = |s: &str| -> Vec<String> { crate::links::find_urls(&s.chars().collect::<Vec<_>>()).into_iter().map(|(_, u)| u).collect() };
    let text = format!("www.before.org {} www.after.org", "https://a/(".repeat(60_000));
    let started = std::time::Instant::now();
    assert_eq!(found(&text), ["http://www.before.org", "http://www.after.org"]);
    assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
    let long = format!("https://example.com/{} www.after.org", "a".repeat(3000));
    assert_eq!(found(&long), ["http://www.after.org"]);
    let fits = format!("https://example.com/{}", "a".repeat(2000));
    assert_eq!(found(&fits), std::slice::from_ref(&fits));
}

#[test]
fn url_scheme_and_www_match_case_insensitively() {
    let t: Vec<char> = "HTTP://Example.COM".chars().collect();
    let found: Vec<String> = crate::links::find_urls(&t).into_iter().map(|(_, u)| u).collect();
    assert_eq!(found, ["HTTP://Example.COM"]);
    let t: Vec<char> = "\u{130} www.Example.org".chars().collect();
    let found: Vec<String> = crate::links::find_urls(&t).into_iter().map(|(_, u)| u).collect();
    assert_eq!(found, ["http://www.Example.org"]);
}

#[test]
fn polygons_clouds_connected_lines_callouts_and_carets_are_drawn() {
    let mut doc = fixture();
    let tri = vec![[100.0, 100.0], [200.0, 100.0], [150.0, 180.0]];
    let i = add_annotation(&mut doc, &new(0, Shape::Polygon { vertices: tri.clone(), cloud: false }), &meta("p")).unwrap();
    let c = add_annotation(&mut doc, &new(0, Shape::Polygon { vertices: tri.clone(), cloud: true }), &meta("c")).unwrap();
    let l = add_annotation(
        &mut doc,
        &new(0, Shape::PolyLine { vertices: vec![[300.0, 300.0], [350.0, 320.0], [400.0, 300.0]], start: LineEnding::None, end: LineEnding::None }),
        &meta("l"),
    )
    .unwrap();
    let co = add_annotation(
        &mut doc,
        &new(
            0,
            Shape::Callout {
                rect: [300.0, 500.0, 420.0, 540.0],
                knee: [260.0, 520.0],
                point: [220.0, 460.0],
                font_size: 10.0,
                ending: LineEnding::OpenArrow,
            },
        ),
        &meta("co"),
    )
    .unwrap();
    let k = add_annotation(&mut doc, &new(0, Shape::Caret { rect: [50.0, 600.0, 60.0, 612.0] }), &meta("k")).unwrap();
    let doc = reopen(&doc);
    let a = list(&doc, 0);

    let p = &a[i];
    assert_eq!(p.name(b"Subtype"), Some(&b"Polygon"[..]));
    assert_eq!(text(p, b"Subj"), "Polygon");
    let ap = ap_content(&doc, p);
    assert!(ap.contains("100 100 m") && ap.contains("150 180 l") && ap.contains("h\nS"), "{ap}");
    let r = rect(p);
    assert!(r[0] <= 99.0 && r[3] >= 181.0, "{r:?}");

    let cl = &a[c];
    assert_eq!((text(cl, b"Subj").as_str(), cl.name(b"IT")), ("Cloud", Some(&b"PolygonCloud"[..])));
    let ap = ap_content(&doc, cl);
    assert!(ap.matches(" c\n").count() >= 6, "bumps along every edge: {ap}");
    // Bumps bulge outwards: the outline reaches below the bottom edge.
    let r = rect(cl);
    assert!(r[1] < 100.0 - appearance::cloud_radius(2.0), "{r:?}");

    let pl = &a[l];
    assert_eq!((pl.name(b"Subtype"), text(pl, b"Subj").as_str()), (Some(&b"PolyLine"[..]), "Polygonal Line"));
    let ap = ap_content(&doc, pl);
    assert!(ap.contains("400 300 l") && !ap.contains("h\n"), "open path: {ap}");

    let callout = &a[co];
    assert_eq!(callout.name(b"IT"), Some(&b"FreeTextCallout"[..]));
    let line: Vec<f64> = callout.get(b"CL").unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect();
    assert_eq!(line, vec![220.0, 460.0, 260.0, 520.0, 300.0, 520.0], "attached to the box's left side");
    let r = rect(callout);
    assert!(r[0] < 220.0 && r[1] < 460.0 && r[2] >= 420.0 && r[3] >= 540.0, "{r:?}");
    let ap = ap_content(&doc, callout);
    assert!(ap.contains("220 460 m") && ap.contains("(hello) Tj"), "{ap}");

    let caret = &a[k];
    assert_eq!((caret.name(b"Subtype"), text(caret, b"Subj").as_str()), (Some(&b"Caret"[..]), "Inserted Text"));
    assert!(ap_content(&doc, caret).contains(" c h f"));

    // Resizing a callout moves its text box; the leader re-attaches.
    let mut doc = doc;
    set_rect(&mut doc, 0, co, [100.0, 500.0, 200.0, 540.0], &meta("x")).unwrap();
    let callout = &list(&doc, 0)[co];
    let line: Vec<f64> = callout.get(b"CL").unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect();
    assert_eq!(&line[4..], &[200.0, 520.0], "now the right side");
    assert!(ap_content(&doc, callout).contains("(hello) Tj"));

    // Moving shifts vertices.
    move_annotation(&mut doc, 0, i, 10.0, 0.0, &meta("m")).unwrap();
    let v: Vec<f64> = list(&doc, 0)[i].get(b"Vertices").unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect();
    assert_eq!(v[0], 110.0);

    for bad in [
        Shape::Polygon { vertices: vec![[0.0, 0.0], [10.0, 10.0]], cloud: false },
        Shape::PolyLine { vertices: vec![[0.0, 0.0]], start: LineEnding::None, end: LineEnding::None },
        Shape::Caret { rect: [0.0, 0.0, 0.0, 10.0] },
    ] {
        assert!(add_annotation(&mut doc, &new(0, bad), &meta("b")).is_err());
    }
}

#[test]
fn cloud_paths_bulge_outwards_either_way_round() {
    let ccw = [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)];
    let cw: Vec<(f64, f64)> = ccw.iter().rev().copied().collect();
    for pts in [ccw.to_vec(), cw] {
        let p = cloud_ys(&appearance::cloud_path(&pts, 5.0));
        assert!(p.iter().any(|y| *y < -3.0) && p.iter().any(|y| *y > 103.0), "{p:?}");
        assert!(p.iter().all(|y| *y > -10.0 && *y < 110.0));
    }
}

/// Every y coordinate of the curve control points in a cloud path.
fn cloud_ys(path: &str) -> Vec<f64> {
    path.lines()
        .filter(|l| l.ends_with(" c"))
        .flat_map(|l| {
            let v: Vec<f64> = l.split_whitespace().filter_map(|t| t.parse().ok()).collect();
            [v[1], v[3]]
        })
        .collect()
}

#[test]
fn replace_text_groups_a_strikeout_and_a_caret() {
    let mut doc = fixture();
    let quads = [[100.0, 720.0, 200.0, 720.0, 100.0, 706.0, 200.0, 706.0]];
    let red = Style { color: [0.89, 0.13, 0.13], opacity: 1.0, width: 1.0, fill: None, ..Style::default() };
    let blue = Style { color: [0.0, 0.47, 0.84], opacity: 1.0, width: 1.0, fill: None, ..Style::default() };
    let i = add_text_replacement(
        &mut doc,
        TextReplacement {
            page: 0,
            quads: &quads,
            replacement: "new words",
            author: "Ada",
            strike: &red,
            caret: &blue,
            strike_meta: &meta("r"),
            caret_meta: &meta("r-caret"),
        },
    )
    .unwrap();
    let doc = reopen(&doc);
    let a = list(&doc, 0);
    // The caret is the group's primary (ISO 32000-2 §12.5.6.2): it holds the text, has no
    // `/IRT`, and is what the call returns.
    let caret = &a[i];
    assert_eq!((caret.name(b"Subtype"), caret.name(b"IT")), (Some(&b"Caret"[..]), Some(&b"Replace"[..])));
    assert_eq!(text(caret, b"Contents").as_str(), "new words");
    assert!(caret.get(b"IRT").is_none() && caret.get(b"RT").is_none());
    // The strikeout is its subordinate, with no text of its own.
    let strike = a.iter().find(|d| d.name(b"Subtype") == Some(b"StrikeOut")).unwrap();
    assert_eq!((strike.name(b"IT"), strike.name(b"RT")), (Some(&b"StrikeOutTextEdit"[..]), Some(&b"Group"[..])));
    assert!(strike.get(b"Contents").is_none());
    assert!(is_group_subordinate(strike));
    // #2666: the pair needs its own `/NM` each. XFDF names a reply by the `/NM` of what it
    // replies to, so sharing one id writes `inreplyto` equal to the subordinate's own `name`
    // and the pair does not survive an export and import.
    assert_eq!(text(caret, b"NM").as_str(), "r-caret");
    assert_eq!(text(strike, b"NM").as_str(), "r");
    let irt = strike.reference(b"IRT").unwrap();
    assert_eq!(text(doc.get(irt).as_dict().unwrap(), b"NM").as_str(), "r-caret", "the strikeout is grouped with the caret");
    let r = rect(caret);
    assert!(r[0] < 200.0 && r[2] > 200.0 && r[1] < 706.0, "at the end of the line, below its baseline: {r:?}");
    let summary = summaries(&doc);
    assert_eq!(summary.iter().filter(|s| s.page == 0 && !s.is_group_member()).count(), 2, "the fixture's Square and the caret: {summary:?}");
    // Replace Text is one comment: deleting either the caret or the strikeout deletes both, as
    // Acrobat does.
    for member in [i, a.iter().position(|d| d.name(b"Subtype") == Some(b"StrikeOut")).unwrap()] {
        let mut doc = doc.clone();
        delete_annotation(&mut doc, 0, member).unwrap();
        assert!(!list(&doc, 0).iter().any(|d| matches!(d.name(b"Subtype"), Some(b"Caret" | b"StrikeOut"))), "{member}");
    }
    let marks = |doc: &Document| list(doc, 0).iter().filter(|d| matches!(d.name(b"Subtype"), Some(b"Caret" | b"StrikeOut"))).count();
    let si = a.iter().position(|d| d.name(b"Subtype") == Some(b"StrikeOut")).unwrap();
    let mut doc = doc;
    let c = list(&doc, 0).iter().position(|d| d.name(b"Subtype") == Some(b"Caret")).unwrap();
    // A locked caret keeps the strikeout too: the pair can't be broken by deleting the other.
    let mut locked = doc.clone();
    set_locked(&mut locked, 0, c, true).unwrap();
    assert!(deletable(&locked, 0, si).is_err() && delete_annotation(&mut locked, 0, si).is_err(), "a locked caret refuses its strikeout's deletion");
    assert_eq!(marks(&locked), 2);
    delete_annotation(&mut doc, 0, c).unwrap();
    assert_eq!(marks(&doc), 0, "deleting the caret deletes the strikeout");
    assert!(
        add_text_replacement(
            &mut doc,
            TextReplacement {
                page: 0,
                quads: &[],
                replacement: "x",
                author: "Ada",
                strike: &red,
                caret: &blue,
                strike_meta: &meta("r"),
                caret_meta: &meta("r-caret")
            }
        )
        .is_err()
    );
}

/// ISO 32000-2 §12.5.6.2: `/RT /Group` makes an annotation a subordinate of the group its
/// `/IRT` annotation is primary of. Another `/RT` reads as a reply, and so does a bare `/IRT`.
/// Deleting a member deletes just it, as Acrobat does, although §12.5.6.2 lists deletion among the
/// operations that act on the entire group: the subordinate goes alone, and the primary goes with
/// its replies and leaves its subordinate, which reads as a comment of its own.
#[test]
fn a_group_reads_as_one_and_deletes_by_member() {
    let mut doc = fixture();
    let square = add_annotation(&mut doc, &new(0, Shape::Rectangle { rect: [100.0, 100.0, 200.0, 200.0] }), &meta("sq")).unwrap();
    let (_, sr) = annot_ref(&mut doc, 0, square).unwrap();
    let irt = |doc: &mut Document, id: &str, rt: Option<&str>| {
        let i = add_annotation(doc, &new(0, Shape::Oval { rect: [300.0, 300.0, 350.0, 350.0] }), &meta(id)).unwrap();
        let (_, r) = annot_ref(doc, 0, i).unwrap();
        doc.update_dict(r, |d| {
            d.set(b"IRT".to_vec(), Object::Ref(sr));
            if let Some(rt) = rt {
                d.set(b"RT".to_vec(), Object::name(rt));
            }
        })
        .unwrap();
        i
    };
    let member = irt(&mut doc, "member", Some("Group"));
    irt(&mut doc, "reply", None);
    irt(&mut doc, "explicit", Some("R"));
    irt(&mut doc, "odd", Some("Bogus"));
    let doc = reopen(&doc);
    let grouped: Vec<(String, bool)> =
        summaries(&doc).into_iter().filter(|s| s.in_reply_to.is_some()).map(|s| (s.name.clone().unwrap_or_default(), s.is_group_member())).collect();
    assert_eq!(
        grouped,
        [("member".into(), true), ("reply".into(), false), ("explicit".into(), false), ("odd".into(), false)],
        "only /RT /Group groups"
    );
    let mut alone = doc.clone();
    delete_annotation(&mut alone, 0, member).unwrap();
    assert_eq!(list(&alone, 0).len(), list(&doc, 0).len() - 1, "the subordinate goes alone");
    let mut doc = doc;
    delete_annotation(&mut doc, 0, square).unwrap();
    let left: Vec<Option<String>> = list(&doc, 0).iter().map(|d| d.get(b"NM").map(|_| text(d, b"NM"))).collect();
    assert_eq!(left, [None, Some("member".into())], "the primary and its replies are gone; the fixture's Square and the subordinate stay");
    let summary = summaries(&doc);
    let member = summary.iter().find(|s| s.name.as_deref() == Some("member")).unwrap();
    assert!(member.parent.is_none() && !member.is_group_member(), "left without its primary, it is a comment of its own: {member:?}");
}

/// A primary with one subordinate and one reply, all on page 0 of a reopened file. Returns the
/// primary's and the subordinate's `/Annots` positions and references.
fn group_of_two(text: &str) -> (Document, (usize, ObjRef), (usize, ObjRef)) {
    let mut doc = fixture();
    let primary = add_annotation(&mut doc, &new(0, Shape::Rectangle { rect: [100.0, 100.0, 200.0, 200.0] }), &meta("primary")).unwrap();
    let (_, pr) = annot_ref(&mut doc, 0, primary).unwrap();
    doc.update_dict(pr, |d| d.set(b"Contents".to_vec(), pdfcraft_cos::PdfString::text(text))).unwrap();
    let member = add_annotation(&mut doc, &new(0, Shape::Oval { rect: [300.0, 300.0, 350.0, 350.0] }), &meta("member")).unwrap();
    let (_, mr) = annot_ref(&mut doc, 0, member).unwrap();
    doc.update_dict(mr, |d| {
        d.set(b"IRT".to_vec(), Object::Ref(pr));
        d.set(b"RT".to_vec(), Object::name("Group"));
    })
    .unwrap();
    add_reply(&mut doc, 0, primary, "a reply", "Ada", &meta("reply")).unwrap();
    (reopen(&doc), (primary, pr), (member, mr))
}

/// Deleting a group's primary leaves its subordinate a comment of its own, as Acrobat does: the
/// subordinate loses its `/IRT` and keeps `/RT /Group`, so nothing refers to the deleted primary
/// and a full save leaves its text out.
#[test]
fn deleting_a_primary_releases_its_subordinate() {
    let (mut doc, (primary, _), (_, mr)) = group_of_two("primary secret text");
    delete_annotation(&mut doc, 0, primary).unwrap();
    let member = doc.get(mr).as_dict().cloned().unwrap();
    assert!(member.get(b"IRT").is_none(), "the subordinate no longer names the deleted primary: {member:?}");
    assert_eq!(member.name(b"RT"), Some(&b"Group"[..]), "its /RT stays, as Acrobat leaves it");
    let s = summaries(&doc);
    let listed = s.iter().find(|s| s.name.as_deref() == Some("member")).unwrap();
    assert!(listed.parent.is_none() && listed.in_reply_to.is_none() && !listed.is_group_member(), "a comment of its own: {listed:?}");
    assert!(!s.iter().any(|s| s.name.as_deref() == Some("reply")), "the primary's reply went with it");
    let full = pdfcraft_cos::write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    assert!(!full.windows(19).any(|w| w == b"primary secret text"), "the deleted primary's text is not in a full save");
}

/// Deleting a subordinate leaves its primary as it was.
#[test]
fn deleting_a_subordinate_leaves_its_primary_untouched() {
    let (mut doc, (_, pr), (member, _)) = group_of_two("primary text");
    let before = doc.get(pr).as_dict().cloned();
    delete_annotation(&mut doc, 0, member).unwrap();
    assert_eq!(doc.get(pr).as_dict().cloned(), before);
    assert!(summaries(&doc).iter().any(|s| s.name.as_deref() == Some("reply")), "the primary keeps its reply");
}

/// An inline subordinate (`/Annots` may hold a dictionary, though the standard asks for a
/// reference) whose primary is deleted loses its `/IRT` in the page's list.
#[test]
fn an_inline_subordinate_is_released_too() {
    let mut doc = fixture();
    let primary = add_annotation(&mut doc, &new(0, Shape::Rectangle { rect: [100.0, 100.0, 200.0, 200.0] }), &meta("primary")).unwrap();
    let (_, pr) = annot_ref(&mut doc, 0, primary).unwrap();
    let p = page_refs(&doc).unwrap()[0];
    let mut entries = annots(&doc, p);
    let mut inline = Dict::new();
    inline.set(b"Type".to_vec(), Object::name("Annot"));
    inline.set(b"Subtype".to_vec(), Object::name("Square"));
    inline.set(b"Rect".to_vec(), Object::Array([300, 300, 350, 350].map(Object::Int).to_vec()));
    inline.set(b"IRT".to_vec(), Object::Ref(pr));
    inline.set(b"RT".to_vec(), Object::name("Group"));
    entries.push(Object::Dict(inline));
    set_annots(&mut doc, p, entries).unwrap();
    delete_annotation(&mut doc, 0, primary).unwrap();
    let left = list(&doc, 0);
    let inline = left.iter().find(|d| d.name(b"RT") == Some(b"Group")).expect("the inline subordinate stays");
    assert!(inline.get(b"IRT").is_none());
}

/// Deleting follows `/IRT` replies by visiting each annotation once, with no step limit to cut a
/// thread short: a thread of 200 replies (past the 64 steps the listings follow) goes whole, and
/// replies or group members in a loop (`/IRT` back to each other) end the walk. A loop of group
/// members deletes only the one picked; the other is released.
#[test]
fn deleting_follows_a_long_or_looping_irt_chain_once_each() {
    let mut doc = fixture();
    let top = add_annotation(&mut doc, &new(0, Shape::Rectangle { rect: [100.0, 100.0, 200.0, 200.0] }), &meta("top")).unwrap();
    let mut on = top;
    for k in 0..200 {
        on = add_reply(&mut doc, 0, on, "reply", "Ada", &meta(&format!("r{k}"))).unwrap();
    }
    let before = list(&doc, 0).len();
    delete_annotation(&mut doc, 0, top).unwrap();
    assert_eq!(list(&doc, 0).len(), before - 201, "the comment and its 200-deep thread go together");
    for rt in [None, Some("Group")] {
        let mut doc = fixture();
        let a = add_annotation(&mut doc, &new(0, Shape::Rectangle { rect: [100.0, 100.0, 200.0, 200.0] }), &meta("a")).unwrap();
        let b = add_annotation(&mut doc, &new(0, Shape::Oval { rect: [300.0, 300.0, 350.0, 350.0] }), &meta("b")).unwrap();
        let (_, ar) = annot_ref(&mut doc, 0, a).unwrap();
        let (_, br) = annot_ref(&mut doc, 0, b).unwrap();
        for (r, to) in [(ar, br), (br, ar)] {
            doc.update_dict(r, |d| {
                d.set(b"IRT".to_vec(), Object::Ref(to));
                if let Some(rt) = rt {
                    d.set(b"RT".to_vec(), Object::name(rt));
                }
            })
            .unwrap();
        }
        let before = list(&doc, 0).len();
        delete_annotation(&mut doc, 0, a).unwrap();
        let left = list(&doc, 0);
        match rt {
            None => assert_eq!(left.len(), before - 2, "a reply loop goes whole"),
            Some(_) => {
                assert_eq!(left.len(), before - 1, "a group member goes alone");
                assert!(doc.get(br).as_dict().is_some_and(|d| d.get(b"IRT").is_none()), "the other member is released");
            }
        }
    }
}

/// A subordinate deep in a chain of subordinates, within the 64 annotations the listing follows
/// up a chain or past them, is deleted alone: the chain above it stays.
#[test]
fn a_subordinate_deep_in_a_chain_is_deleted_alone() {
    for subordinates in [63, 64, 65] {
        let mut doc = fixture();
        let top = add_annotation(&mut doc, &new(0, Shape::Rectangle { rect: [100.0, 100.0, 200.0, 200.0] }), &meta("top")).unwrap();
        let (_, mut up) = annot_ref(&mut doc, 0, top).unwrap();
        let mut last = top;
        for k in 0..subordinates {
            last = add_annotation(&mut doc, &new(0, Shape::Oval { rect: [300.0, 300.0, 350.0, 350.0] }), &meta(&format!("s{k}"))).unwrap();
            let (_, r) = annot_ref(&mut doc, 0, last).unwrap();
            doc.update_dict(r, |d| {
                d.set(b"IRT".to_vec(), Object::Ref(up));
                d.set(b"RT".to_vec(), Object::name("Group"));
            })
            .unwrap();
            up = r;
        }
        let before = list(&doc, 0).len();
        delete_annotation(&mut doc, 0, last).unwrap();
        assert_eq!(list(&doc, 0).len(), before - 1, "{subordinates}: the subordinate is gone, the rest of its group stays");
    }
}

/// A locked member doesn't hold back the deletion of another, the primary included: each goes
/// alone (ISO 32000-2 §12.5.3, Table 167: Locked).
#[test]
fn a_locked_group_member_holds_back_no_other_deletion() {
    let mut doc = fixture();
    let top = add_annotation(&mut doc, &new(0, Shape::Rectangle { rect: [100.0, 100.0, 200.0, 200.0] }), &meta("top")).unwrap();
    let (_, tr) = annot_ref(&mut doc, 0, top).unwrap();
    let mut members = Vec::new();
    for id in ["a", "b"] {
        let i = add_annotation(&mut doc, &new(0, Shape::Oval { rect: [300.0, 300.0, 350.0, 350.0] }), &meta(id)).unwrap();
        let (_, r) = annot_ref(&mut doc, 0, i).unwrap();
        doc.update_dict(r, |d| {
            d.set(b"IRT".to_vec(), Object::Ref(tr));
            d.set(b"RT".to_vec(), Object::name("Group"));
        })
        .unwrap();
        members.push(i);
    }
    set_locked(&mut doc, 0, members[1], true).unwrap();
    let before = list(&doc, 0).len();
    let mut alone = doc.clone();
    delete_annotation(&mut alone, 0, members[0]).unwrap();
    assert_eq!(list(&alone, 0).len(), before - 1, "the unlocked subordinate goes alone");
    delete_annotation(&mut doc, 0, top).unwrap();
    assert_eq!(list(&doc, 0).len(), before - 1, "the primary goes alone too");
    let locked = list(&doc, 0).iter().filter(|d| d.get(b"F").and_then(|f| f.as_int()).is_some_and(|f| f & 128 != 0)).count();
    assert_eq!(locked, 1, "the locked member stays");
}

/// Review on #5689 (CodeRabbit): clicking a Replace Text strikeout selects its caret, the
/// group's primary, so a restyle from Comment Properties lands on the caret. Colour and opacity
/// are the proposal's, so the strikeout is restyled and redrawn in the same edit, whichever
/// member is restyled; it takes no border width. A locked member refuses the whole change.
#[test]
fn restyling_a_replace_text_pair_restyles_both_members() {
    let mut doc = fixture();
    let quads = [[100.0, 720.0, 200.0, 720.0, 100.0, 706.0, 200.0, 706.0]];
    let red = Style { color: [0.89, 0.13, 0.13], opacity: 1.0, width: 1.0, fill: None, ..Style::default() };
    let tr = TextReplacement {
        page: 0,
        quads: &quads,
        replacement: "new words",
        author: "Ada",
        strike: &red,
        caret: &red,
        strike_meta: &meta("r"),
        caret_meta: &meta("r-caret"),
    };
    // The call returns the caret, the group's primary.
    let c = add_text_replacement(&mut doc, tr).unwrap();
    let s = list(&doc, 0).iter().position(|d| d.name(b"Subtype") == Some(b"StrikeOut")).unwrap();
    let color = |doc: &Document, i: usize| {
        list(doc, 0)[i].get(b"C").and_then(|o| o.as_array().cloned()).unwrap().iter().filter_map(Object::as_f64).collect::<Vec<_>>()
    };
    let green = [0.0, 0.5, 0.0];

    // Restyled through the caret (what a click on either part selects).
    let mut by_caret = doc.clone();
    set_style(&mut by_caret, 0, c, StyleChange::new(Some(green), Some(0.5), Some(3.0), None), &meta("")).unwrap();
    let by_caret = reopen(&by_caret);
    assert_eq!(color(&by_caret, c), green.to_vec());
    assert_eq!(color(&by_caret, s), green.to_vec(), "the strikeout takes the proposal's colour");
    let strike = &list(&by_caret, 0)[s];
    assert_eq!(strike.get(b"CA").and_then(Object::as_f64), Some(0.5), "and its opacity");
    assert!(strike.get(b"BS").is_none(), "a strikeout has no border to widen: {strike:?}");
    assert!(ap_content(&by_caret, strike).contains("0 0.5 0 RG"), "its appearance is redrawn: {}", ap_content(&by_caret, strike));

    // Restyled through the strikeout: the caret follows.
    let mut by_strike = doc.clone();
    set_style(&mut by_strike, 0, s, StyleChange::new(Some(green), None, None, None), &meta("")).unwrap();
    assert_eq!(color(&by_strike, c), green.to_vec(), "the caret takes the proposal's colour");

    // A locked strikeout refuses the change, and nothing moves.
    let mut locked = doc.clone();
    set_locked(&mut locked, 0, s, true).unwrap();
    assert!(set_style(&mut locked, 0, c, StyleChange::new(Some(green), None, None, None), &meta("")).is_err());
    assert_eq!(color(&locked, c), red.color.to_vec(), "the caret keeps its colour");
}

#[test]
fn files_attach_as_comments() {
    let mut doc = fixture();
    let data = b"col1,col2\n1,2\n".to_vec();
    let shape = Shape::Attachment { at: [300.0, 500.0], icon: AttachIcon::Paperclip, file: "data.csv".into(), data: data.clone() };
    let i = add_annotation(
        &mut doc,
        &NewAnnotation { page: 0, shape, style: Style::default(), contents: String::new(), author: "Ada".into() },
        &meta("f"),
    )
    .unwrap();
    let doc = reopen(&doc);
    let a = &list(&doc, 0)[i];
    assert_eq!((a.name(b"Subtype"), a.name(b"Name")), (Some(&b"FileAttachment"[..]), Some(&b"Paperclip"[..])));
    assert_eq!(text(a, b"Contents"), "data.csv", "the file name describes it");
    let fs = doc.resolve(a.get(b"FS").unwrap()).as_dict().cloned().unwrap();
    assert_eq!(text(&fs, b"UF"), "data.csv");
    let ef = fs.get(b"EF").and_then(|e| e.as_dict()).and_then(|e| e.reference(b"F")).unwrap();
    let Object::Stream(s) = &*doc.get(ef) else { panic!() };
    assert_eq!(s.decoded().unwrap(), data);
    assert!(ap_content(&doc, a).contains(" c\n") || ap_content(&doc, a).contains(" c "), "a drawn paperclip");
    let mut doc = doc;
    let bad = Shape::Attachment { at: [0.0, 0.0], icon: AttachIcon::Tag, file: " ".into(), data: Vec::new() };
    assert!(
        add_annotation(
            &mut doc,
            &NewAnnotation { page: 0, shape: bad, style: Style::default(), contents: String::new(), author: String::new() },
            &meta("x")
        )
        .is_err()
    );
}

#[test]
fn the_eraser_cuts_strokes_and_removes_empty_drawings() {
    let mut doc = fixture();
    let line = |y: f64| (0..=10).map(|k| [100.0 + k as f64 * 20.0, y]).collect::<Vec<_>>();
    let shape = Shape::Ink { strokes: vec![line(500.0), line(400.0)] };
    let i = add_annotation(&mut doc, &new(0, shape), &meta("i")).unwrap();
    // A vertical swipe through the middle of both lines.
    assert!(erase_ink(&mut doc, 0, i, &[[200.0, 550.0], [200.0, 350.0]], 5.0, &meta("e")).unwrap());
    let d = &list(&doc, 0)[i];
    let strokes = d.get(b"InkList").unwrap().as_array().unwrap().len();
    assert_eq!(strokes, 4, "each line cut in two");
    assert!(!erase_ink(&mut doc, 0, i, &[[600.0, 100.0]], 5.0, &meta("e")).unwrap(), "nothing under the eraser");
    // Rubbing out everything deletes the drawing.
    let all: Vec<[f64; 2]> = (0..=20).map(|k| [100.0 + k as f64 * 10.0, 500.0]).chain((0..=20).map(|k| [100.0 + k as f64 * 10.0, 400.0])).collect();
    erase_ink(&mut doc, 0, i, &all[..21], 8.0, &meta("e")).unwrap();
    erase_ink(&mut doc, 0, i, &all[21..], 8.0, &meta("e")).unwrap();
    assert!(!list(&doc, 0).iter().any(|d| d.name(b"Subtype") == Some(b"Ink")));
}

/// A document holding only a catalog, for appearance builds from a bare dictionary whose
/// values are all direct.
fn bare_doc() -> Document {
    document_of(&[b"<< /Type /Catalog /Pages << /Type /Pages /Kids [] /Count 0 >> >>"])
}

/// A one-page document from object bodies (object 1 is the catalog, 3 the page).
fn document_of(objs: &[&[u8]]) -> Document {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).expect("opens")
}

#[test]
fn comments_without_appearances_get_one_for_display_only() {
    let doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R 7 0 R 8 0 R 9 0 R 10 0 R] >>",
        b"<< /Type /Annot /Subtype /FreeText /Rect [48 582 285 632] /Contents (FreeText annotation with valid /DA string.) /DA (/Helv 10 Tf 0 0 1 rg) /Border [0 0 1] /C [.85 .47 .02] >>",
        b"<< /Type /Annot /Subtype /Ink /Rect [70 392 275 472] /InkList [[80 402 120 452 160 412 210 462 260 417]] /Border [0 0 2] /C [.49 .18 .07] >>",
        b"<< /Type /Annot /Subtype /Text /Rect [400 717 420 737] /C [1 1 0] /Contents (A note) >>",
        b"<< /Type /Annot /Subtype /Square /Rect [10 10 50 50] /C [1 0 0] /AP << /N 11 0 R >> >>",
        b"<< /Type /Annot /Subtype /Link /Rect [48 717 308 741] /Border [0 0 1] /C [0 0 1] >>",
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (f) /Rect [10 100 100 120] >>",
        b"<< /Type /Annot /Subtype /Sound /Rect [10 200 30 220] >>",
        b"<< /Type /XObject /Subtype /Form /BBox [10 10 50 50] /Length 0 >>\nstream\n\nendstream",
    ]);
    let display = with_missing_appearances(&doc).expect("three comments need an appearance");
    let has_ap = |doc: &Document| list(doc, 0).iter().map(|d| d.get(b"AP").is_some()).collect::<Vec<_>>();
    assert_eq!(has_ap(&doc), [false, false, false, true, false, false, false], "the document itself is untouched");
    assert_eq!(has_ap(&display), [true, true, true, true, false, false, false], "FreeText, Ink and Text get one; the rest don't");
    let shown = list(&display, 0);
    let free_text = ap_content(&display, &shown[0]);
    assert!(free_text.contains("re f") && free_text.contains("Tj"), "background box and text: {free_text}");
    assert_eq!(shown[3].get(b"AP"), list(&doc, 0)[3].get(b"AP"), "an existing appearance is kept");
    // The display copy survives a save and reopen (what the renderer reads).
    assert_eq!(has_ap(&reopen(&display)), [true, true, true, true, false, false, false]);
    // Nothing to add: no copy.
    let complete = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
        b"<< /Type /Annot /Subtype /Link /Rect [48 717 308 741] >>",
        b"<< /Type /Annot /Subtype /Square /Rect [10 10 50 50] /C [1 0 0] /AP << /N 6 0 R >> >>",
        b"<< /Type /XObject /Subtype /Form /BBox [10 10 50 50] /Length 0 >>\nstream\n\nendstream",
    ]);
    assert!(with_missing_appearances(&complete).is_none());
}

fn line_dict(coords: [f64; 4], ending: &str) -> pdfcraft_cos::Dict {
    let mut d = pdfcraft_cos::Dict::new();
    d.set(b"Subtype".to_vec(), Object::name("Line"));
    d.set(b"Rect".to_vec(), Object::Array([0.0, 0.0, 300.0, 300.0].into_iter().map(Object::Real).collect()));
    d.set(b"C".to_vec(), Object::Array([1.0, 0.0, 0.0].into_iter().map(Object::Real).collect()));
    d.set(b"L".to_vec(), Object::Array(coords.into_iter().map(Object::Real).collect()));
    d.set(b"LE".to_vec(), Object::Array(vec![Object::name("None"), Object::name(ending)]));
    d
}

#[test]
fn malformed_optional_interior_color_does_not_hide_shape_strokes() {
    let mut square = pdfcraft_cos::Dict::new();
    square.set(b"Subtype".to_vec(), Object::name("Square"));
    square.set(b"Rect".to_vec(), Object::Array([0.0, 0.0, 40.0, 30.0].into_iter().map(Object::Real).collect()));
    square.set(b"C".to_vec(), Object::Array([1.0, 0.0, 0.0].into_iter().map(Object::Real).collect()));
    let malformed = Object::Array(vec![Object::Real(1.0), Object::Real(0.0)]);
    square.set(b"IC".to_vec(), malformed.clone());
    let mut circle = square.clone();
    circle.set(b"Subtype".to_vec(), Object::name("Circle"));

    let mut polygon = pdfcraft_cos::Dict::new();
    polygon.set(b"Subtype".to_vec(), Object::name("Polygon"));
    polygon.set(b"Rect".to_vec(), Object::Array([0.0, 0.0, 40.0, 30.0].into_iter().map(Object::Real).collect()));
    polygon.set(b"C".to_vec(), Object::Array([1.0, 0.0, 0.0].into_iter().map(Object::Real).collect()));
    polygon.set(b"Vertices".to_vec(), Object::Array([0.0, 0.0, 40.0, 0.0, 20.0, 30.0].into_iter().map(Object::Real).collect()));
    polygon.set(b"IC".to_vec(), malformed.clone());

    let mut line = line_dict([0.0, 0.0, 40.0, 30.0], "None");
    line.set(b"IC".to_vec(), malformed.clone());
    let mut polyline = pdfcraft_cos::Dict::new();
    polyline.set(b"Subtype".to_vec(), Object::name("PolyLine"));
    polyline.set(b"Rect".to_vec(), Object::Array([0.0, 0.0, 40.0, 30.0].into_iter().map(Object::Real).collect()));
    polyline.set(b"C".to_vec(), Object::Array([1.0, 0.0, 0.0].into_iter().map(Object::Real).collect()));
    polyline.set(b"Vertices".to_vec(), Object::Array([0.0, 0.0, 40.0, 0.0, 20.0, 30.0].into_iter().map(Object::Real).collect()));
    polyline.set(b"LE".to_vec(), Object::Array(vec![Object::name("None"), Object::name("None")]));
    polyline.set(b"IC".to_vec(), malformed);

    for (name, annotation) in [("Square", square), ("Circle", circle), ("Polygon", polygon), ("Line", line), ("PolyLine", polyline)] {
        assert!(appearance::build(&bare_doc(), &annotation).is_some(), "malformed optional /IC must not hide a valid {name} stroke");
    }

    let doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 50 40] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        b"<< /Type /Annot /Subtype /Square /Rect [5 5 45 35] /C [1 0 0] /IC [1 0] >>",
    ]);
    let display = with_missing_appearances(&doc).expect("the valid square gets a fallback appearance");
    let rendered = list(&display, 0).remove(0);
    let content = ap_content(&display, &rendered);
    assert!(content.contains("1 0 0 RG") && content.contains(" re\nS\n"), "the stroke remains drawable without a fill: {content}");
    assert!(list(&doc, 0)[0].get(b"AP").is_none(), "display generation leaves the source document unchanged");
}

/// Any value may be given by reference (ISO 32000-2 §7.3.10). A Square whose border style
/// dictionary (Table 168), colour and dash array are indirect objects is drawn with them, not
/// with the 1 pt solid default; a reference cycle inside `/BS` is not followed for ever.
#[test]
fn indirect_border_style_and_colour_are_drawn() {
    let doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        b"<< /Type /Annot /Subtype /Square /P 3 0 R /Rect [20 20 180 180] /BS 5 0 R /C 6 0 R >>",
        b"<< /Type /Border /W 7 0 R /S /D /D 8 0 R /Self 5 0 R >>",
        b"[0 0 1]",
        b"6",
        b"[4 2]",
    ]);
    let display = with_missing_appearances(&doc).expect("the square gets a display appearance");
    let shown = list(&display, 0).remove(0);
    let content = ap_content(&display, &shown);
    assert!(content.contains("0 0 1 RG\n6 w\n[4 2] 0 d\n"), "the indirect /BS, /W, /D and /C are used: {content}");
    assert!(content.contains("23 23 154 154 re"), "the 6 pt border is inset by half its width: {content}");
    assert_eq!(shown.get(b"BS"), Some(&Object::Ref(ObjRef::new(5, 0))), "the display copy keeps the reference");
    assert_eq!(shown.get(b"P"), Some(&Object::Ref(ObjRef::new(3, 0))));
    let props = props(&doc, 0, 0).expect("props");
    assert_eq!(props.width, Some(6.0), "the properties show the width drawn");
}

/// A followed reference the builder can't draw (a cloudy border effect on a square, which it
/// leaves alone) keeps the drawing made without it instead of none.
#[test]
fn an_undrawable_indirect_value_keeps_the_plain_drawing() {
    let doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        b"<< /Type /Annot /Subtype /Square /Rect [20 20 180 180] /BE 5 0 R /C [1 0 0] >>",
        b"<< /S /C /I 1 >>",
    ]);
    let display = with_missing_appearances(&doc).expect("the square is still drawn");
    let shown = list(&display, 0).remove(0);
    assert!(ap_content(&display, &shown).contains("1 0 0 RG\n1 w\n"));
}

/// Following references copies a bounded number of values: an indirect array past the budget
/// stays a reference (and the comment is not drawn from it), a chain that never ends is not
/// followed, and links to other objects are never copied.
#[test]
fn direct_values_are_bounded() {
    let mut doc = fixture();
    let huge = doc.add(Object::Array(vec![Object::Int(0); (1 << 16) + 1]));
    let chain = doc.add(Object::Null);
    doc.set(chain, Object::Ref(chain));
    let mut d = Dict::new();
    d.set(b"Subtype".to_vec(), Object::name("Square"));
    d.set(b"Rect".to_vec(), Object::Ref(doc.add(num_array(&[0.0, 0.0, 10.0, 10.0]))));
    d.set(b"Vertices".to_vec(), Object::Ref(huge));
    d.set(b"IC".to_vec(), Object::Ref(chain));
    d.set(b"Popup".to_vec(), Object::Ref(huge));
    let direct = crate::draw::with_direct_values(&doc, &d);
    assert_eq!(direct.get(b"Rect").and_then(Object::as_array).map(Vec::len), Some(4), "a small array is followed");
    assert_eq!(direct.get(b"Vertices"), Some(&Object::Ref(huge)), "over the budget: left as a reference");
    assert_eq!(direct.get(b"IC"), Some(&Object::Ref(chain)), "a chain of references is left alone");
    assert_eq!(direct.get(b"Popup"), Some(&Object::Ref(huge)), "a link is not copied");
    let mut plain = Dict::new();
    plain.set(b"W".to_vec(), Object::Int(2));
    assert!(matches!(crate::draw::with_direct_values(&doc, &plain), std::borrow::Cow::Borrowed(_)), "nothing to follow: no copy");
}

/// A new width for a comment whose border style is an indirect object (§7.3.10) keeps that
/// dictionary's other entries (a dashed style stays dashed) and leaves the shared object alone.
#[test]
fn new_width_keeps_an_indirect_border_style() {
    let mut doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        b"<< /Type /Annot /Subtype /Square /Rect [20 20 180 180] /BS 5 0 R /C [1 0 0] >>",
        b"<< /W 6 /S /D /D [4 2] >>",
    ]);
    set_style(&mut doc, 0, 0, StyleChange::new(None, None, Some(3.0), None), &meta("")).unwrap();
    let doc = reopen(&doc);
    let d = list(&doc, 0).remove(0);
    let bs = d.get(b"BS").and_then(Object::as_dict).expect("a direct /BS");
    assert_eq!(bs.get(b"W").and_then(Object::as_f64), Some(3.0));
    assert_eq!(bs.name(b"S"), Some(b"D".as_slice()), "the dashed style is kept");
    assert!(ap_content(&doc, &d).contains("3 w\n[4 2] 0 d\n"));
    let shared = doc.get(ObjRef::new(5, 0));
    assert_eq!(shared.as_dict().and_then(|b| b.get(b"W")).and_then(Object::as_f64), Some(6.0), "the shared object is unchanged");
}

/// A colour given by reference (`/C 6 0 R`, §7.3.10) is read by the properties and the edits as
/// the display draws it: the properties show it, the appearance can be restyled and filled.
#[test]
fn an_indirect_colour_reads_and_restyles_as_drawn() {
    let mut doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        b"<< /Type /Annot /Subtype /Square /Rect [20 20 180 180] /C 5 0 R /IC 6 0 R >>",
        b"[0 0 1]",
        b"[1 1 0]",
    ]);
    let p = props(&doc, 0, 0).expect("props");
    assert_eq!((p.color, p.fill, p.restylable), (Some([0.0, 0.0, 1.0]), Some([1.0, 1.0, 0.0]), true));
    set_style(&mut doc, 0, 0, StyleChange::new(None, None, Some(3.0), None), &meta("")).expect("restylable");
    set_fill(&mut doc, 0, 0, Some([0.0, 1.0, 0.0]), &meta("")).expect("fillable");
    let d = list(&doc, 0).remove(0);
    let content = ap_content(&doc, &d);
    assert!(content.contains("0 0 1 RG") && content.contains("0 1 0 rg") && content.contains("3 w"), "{content}");
}

/// References are followed into everything they point at, and all of it is counted: a referenced
/// array of two arrays of 40,000 numbers is over the per-annotation budget. One display pass
/// copies at most [`crate::draw::PASS_BUDGET`] values in all, so an array many annotations point
/// at is not copied again for each of them.
#[test]
fn following_references_is_bounded_per_value_and_per_pass() {
    let mut doc = fixture();
    let inner = Object::Array(vec![Object::Int(0); 40_000]);
    let nested = doc.add(Object::Array(vec![inner.clone(), inner]));
    let mut d = Dict::new();
    d.set(b"Subtype".to_vec(), Object::name("Square"));
    d.set(b"Junk".to_vec(), Object::Ref(nested));
    let direct = crate::draw::with_direct_values(&doc, &d);
    assert!(direct.get(b"Junk") == Some(&Object::Ref(nested)), "80,003 values: left as a reference");

    let shared = doc.add(Object::Array(vec![Object::Int(0); 60_000]));
    d.set(b"Junk".to_vec(), Object::Ref(shared));
    let mut pass = crate::draw::PASS_BUDGET;
    let copies = (0..100).filter(|_| matches!(crate::draw::with_direct_values_within(&doc, &d, &mut pass), std::borrow::Cow::Owned(_))).count();
    assert_eq!(copies, crate::draw::PASS_BUDGET / 60_001, "copies stop when the pass budget is spent");
}

/// Finding an object too large to copy costs the walk that found it out: an array of two arrays
/// of 40,000 numbers that many annotations point at is walked about 40,000 values deep before it
/// is rejected, and that is charged to the annotation and to the pass, so after a bounded number
/// of such walks the pass has no room left to walk it again. A reference to it repeated in one annotation
/// is not walked again in full, and a small referenced colour after it is still read.
#[test]
fn a_rejected_large_object_is_charged_to_the_budgets() {
    let mut doc = fixture();
    let inner = Object::Array(vec![Object::Int(0); 40_000]);
    let nested = doc.add(Object::Array(vec![inner.clone(), inner]));
    let red = doc.add(Object::Array(vec![Object::Int(1), Object::Int(0), Object::Int(0)]));
    let mut d = Dict::new();
    d.set(b"Subtype".to_vec(), Object::name("Square"));
    d.set(b"Junk".to_vec(), Object::Array(vec![Object::Ref(nested); 1_000]));
    d.set(b"C".to_vec(), Object::Ref(red));
    let mut pass = crate::draw::PASS_BUDGET;
    let direct = crate::draw::with_direct_values_within(&doc, &d, &mut pass);
    assert_eq!(direct.get(b"C"), Some(&Object::Array(vec![Object::Int(1), Object::Int(0), Object::Int(0)])));
    let charged = crate::draw::PASS_BUDGET - pass;
    assert!((40_000..=crate::draw::INLINE_BUDGET).contains(&charged), "the rejected walk is charged once: {charged}");
    d.set(b"Junk".to_vec(), Object::Ref(nested));
    for _ in 0..crate::draw::PASS_BUDGET / 40_000 {
        crate::draw::with_direct_values_within(&doc, &d, &mut pass);
    }
    assert!(pass < 40_000, "after {} annotations the pass has no room for another full walk: {pass}", crate::draw::PASS_BUDGET / 40_000 + 1);
}

/// `/RD` is optional (ISO 32000-2 Table 177): a callout without it has its text box at `/Rect`
/// and is drawn, box, text and leader line. A callout line and `/RD` given by reference are read too.
#[test]
fn callout_without_rd_is_drawn() {
    let doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 400 400] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R] >>",
        b"<< /Type /Annot /Subtype /FreeText /IT /FreeTextCallout /Rect [100 100 300 160] /CL [20 20 60 60 100 130] /LE /OpenArrow /C [1 1 0] /DA (0 g /Helv 12 Tf) /Contents (callout) >>",
        b"<< /Type /Annot /Subtype /FreeText /IT /FreeTextCallout /Rect [10 200 300 300] /CL 7 0 R /RD 8 0 R /C [1 1 0] /DA (0 g /Helv 12 Tf) /Contents (indirect) >>",
        b"<< /Type /Annot /Subtype /FreeText /IT /FreeTextCallout /Rect [10 300 300 390] /CL [5 305 10 350] /RD [1 2] /C [1 1 0] /DA (0 g /Helv 12 Tf) /Contents (bad) >>",
        b"[15 205 40 250 100 250]",
        b"[90 0 0 0]",
    ]);
    let display = with_missing_appearances(&doc).expect("the callouts get a display appearance");
    let shown = list(&display, 0);
    let plain = ap_content(&display, &shown[0]);
    assert!(plain.contains("1 1 0 rg\n100 100 200 60 re f"), "the yellow box fills /Rect: {plain}");
    assert!(plain.contains("20 20 m\n60 60 l\n100 130 l\nS"), "the leader line: {plain}");
    assert!(plain.contains("(callout) Tj"), "the text: {plain}");
    let indirect = ap_content(&display, &shown[1]);
    assert!(indirect.contains("100 200 200 100 re f") && indirect.contains("15 205 m"), "indirect /CL and /RD: {indirect}");
    assert!(shown[2].get(b"AP").is_none(), "a malformed /RD is still not guessed at");
}

/// #3175: viewers draw a note without an appearance as a fixed-size icon at the upper-left corner
/// of its `/Rect`. The display copy must not stretch the icon over a large `/Rect` or shrink it
/// to nothing in one with no width.
#[test]
fn a_note_without_an_appearance_is_shown_at_icon_size() {
    let doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 600] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R] >>",
        b"<< /Type /Annot /Subtype /Text /Rect [-23 240 480 468] /C [1 0 0] /Open true >>",
        b"<< /Type /Annot /Subtype /Text /Rect [310 380 310 392] /Name /Comment /F 4 >>",
        b"<< /Type /Annot /Subtype /FileAttachment /Rect [100 50 300 10] >>",
    ]);
    let display = with_missing_appearances(&doc).expect("the notes get a display appearance");
    let rects = |doc: &Document| -> Vec<Vec<f64>> {
        let rect = |d: &Dict| d.get(b"Rect").and_then(|r| r.as_array()).map(|a| a.iter().filter_map(Object::as_f64).collect::<Vec<f64>>());
        list(doc, 0).iter().map(|d| rect(d).unwrap_or_default()).collect()
    };
    assert_eq!(rects(&display), [vec![-23.0, 448.0, -3.0, 468.0], vec![310.0, 372.0, 330.0, 392.0], vec![100.0, 30.0, 120.0, 50.0]]);
    let original = [vec![-23.0, 240.0, 480.0, 468.0], vec![310.0, 380.0, 310.0, 392.0], vec![100.0, 50.0, 300.0, 10.0]];
    assert_eq!(rects(&doc), original, "the source document is unchanged");
}

/// The fourteen rubber stamp names viewers draw without an appearance (ISO 32000-2 Table 184).
const TABLE_184_STAMPS: [(&str, &str); 14] = [
    ("Approved", "APPROVED"),
    ("Experimental", "EXPERIMENTAL"),
    ("NotApproved", "NOT APPROVED"),
    ("AsIs", "AS IS"),
    ("Expired", "EXPIRED"),
    ("NotForPublicRelease", "NOT FOR PUBLIC RELEASE"),
    ("Confidential", "CONFIDENTIAL"),
    ("Final", "FINAL"),
    ("Sold", "SOLD"),
    ("Departmental", "DEPARTMENTAL"),
    ("ForComment", "FOR COMMENT"),
    ("TopSecret", "TOP SECRET"),
    ("Draft", "DRAFT"),
    ("ForPublicRelease", "FOR PUBLIC RELEASE"),
];

fn stamp_dict(extra: &str) -> Dict {
    let doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        format!("<< /Type /Annot /Subtype /Stamp /Rect [20 40 140 100] {extra} >>").as_bytes(),
    ]);
    list(&doc, 0).remove(0)
}

fn drawn(d: &Dict) -> String {
    String::from_utf8_lossy(&appearance::display_stamp(d).expect("a stamp is drawn").decoded().unwrap()).into_owned()
}

/// A stamp another program wrote without an appearance draws the stamp its `/Name` names, Draft
/// when it has none, and a frame with both diagonals for any other name, never the name itself.
#[test]
fn a_stamp_without_an_appearance_draws_the_stamp_it_names() {
    for (name, label) in TABLE_184_STAMPS {
        let content = drawn(&stamp_dict(&format!("/Name /{name}")));
        assert!(content.contains(&format!("({label}) Tj")), "{name}: {content}");
    }
    assert!(drawn(&stamp_dict("")).contains("(DRAFT) Tj"), "Table 184's default");
    assert!(drawn(&stamp_dict("/IT /Stamp")).contains("(DRAFT) Tj"), "a stamp intent keeps the default");
    // Any other name, a name in another case (names are compared byte for byte, §7.3.5), and an
    // image stamp (`/IT`, which has no `/Name` to default): the frame and its diagonals.
    for extra in ["/Name /MyCustomStamp", "/Name /draft", "/Name /APPROVED", "/IT /StampImage", "/Name /PCVoid"] {
        let content = drawn(&stamp_dict(extra));
        assert!(!content.contains("Tj") && content.contains(" re\n") && content.matches(" l\n").count() == 2, "{extra}: {content}");
    }
    // `/C` colours the stamp, as it does PdfCraft's own.
    assert!(drawn(&stamp_dict("/Name /Final /C [1 0 1]")).contains("1 0 1 RG"));
    // The placeholder stays grey whatever the `/C`: it is not the stamp, so it takes none of its colour.
    let placeholder = drawn(&stamp_dict("/Name /MyCustomStamp /C [1 0 0]"));
    assert!(placeholder.contains("0.45 0.45 0.45 RG") && !placeholder.contains("1 0 0 RG"), "{placeholder}");
    // A stamp's own artwork is never replaced, and the drawing is for display only: edits
    // (restyling, flattening) still find nothing they may write.
    assert!(appearance::display_stamp(&stamp_dict("/Name /Draft /AP << /N 9 0 R >>")).is_none());
    assert!(appearance::build(&bare_doc(), &stamp_dict("/Name /Draft")).is_none());
    assert!(appearance::display_stamp(&stamp_dict("/Name /Draft /PCStamp true")).is_none(), "PdfCraft's own stamps are drawn by `build`");
    assert!(appearance::display_stamp(&stamp_dict("/Name /Draft /Rect [5 5 5 50]")).is_none(), "no width");
}

/// The display copy draws a stamp that has no normal appearance, and keeps one that has.
#[test]
fn the_display_copy_draws_stamps_without_a_normal_appearance_only() {
    let doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R 7 0 R] >>",
        b"<< /Type /Annot /Subtype /Stamp /Rect [20 40 140 100] /Name /TopSecret >>",
        b"<< /Type /Annot /Subtype /Stamp /Rect [20 140 140 200] /Name /Draft /AP << /N 8 0 R >> >>",
        b"<< /Type /Annot /Subtype /Stamp /Rect [20 240 140 300] /Name /Sold /AP << /D 8 0 R >> >>",
        b"<< /Type /Annot /Subtype /Stamp /Rect [20 340 140 400] /Name /Unheard >>",
        b"<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Length 8 >>\nstream\n1 0 0 rg\nendstream",
    ]);
    let display = with_missing_appearances(&doc).expect("three stamps need an appearance");
    let shown = list(&display, 0);
    assert!(ap_content(&display, &shown[0]).contains("(TOP SECRET) Tj"));
    assert_eq!(shown[1].get(b"AP"), list(&doc, 0)[1].get(b"AP"), "an existing appearance is kept");
    assert!(ap_content(&display, &shown[2]).contains("(SOLD) Tj"), "an /AP without /N has nothing to keep");
    assert!(!ap_content(&display, &shown[3]).contains("Tj"), "an unknown name gets the placeholder");
    assert!(list(&doc, 0)[0].get(b"AP").is_none(), "the document itself is untouched");
}

#[test]
fn every_line_ending_is_drawn() {
    let filled = [LineEnding::Square, LineEnding::Circle, LineEnding::Diamond, LineEnding::ClosedArrow, LineEnding::RClosedArrow];
    let (mut open, mut reverse) = (String::new(), String::new());
    for ending in LineEnding::ALL {
        let mut doc = fixture();
        add_annotation(
            &mut doc,
            &new(0, Shape::Line { from: [80.0, 100.0], to: [200.0, 160.0], start: LineEnding::None, end: ending }),
            &meta("ending"),
        )
        .unwrap();
        let doc = reopen(&doc);
        let line = list(&doc, 0).into_iter().find(|d| d.name(b"Subtype") == Some(b"Line")).expect("line");
        let content = ap_content(&doc, &line);
        assert!(!content.is_empty(), "{ending:?}");
        let paints = content.contains("h B") || content.contains("B\n");
        assert_eq!(paints, filled.contains(&ending), "{ending:?} fill: {content}");
        if ending != LineEnding::None {
            let r = rect(&line);
            assert!(r[0] < 70.0 && r[2] > 210.0, "{ending:?} rectangle includes the ending: {r:?}");
        }
        if ending == LineEnding::OpenArrow {
            open = content.clone();
        }
        if ending == LineEnding::ROpenArrow {
            reverse = content;
        }
    }
    assert_ne!(open, reverse, "a reverse arrow points the other way");

    let mut doc = fixture();
    let index = add_annotation(
        &mut doc,
        &new(0, Shape::Line { from: [40.0, 40.0], to: [140.0, 40.0], start: LineEnding::None, end: LineEnding::None }),
        &meta("plain"),
    )
    .unwrap();
    let before = rect(&list(&doc, 0).into_iter().find(|d| d.name(b"Subtype") == Some(b"Line")).expect("line"));
    set_style(&mut doc, 0, index, StyleChange::new(None, None, None, Some(vec![LineEnding::None, LineEnding::Square])), &meta("style")).unwrap();
    let mut doc = reopen(&doc);
    let line = list(&doc, 0).into_iter().find(|d| d.name(b"Subtype") == Some(b"Line")).expect("line");
    let after = rect(&line);
    assert!(after[1] < before[1] - 5.0 && after[3] > before[3] + 5.0, "the square is not clipped: {before:?} → {after:?}");
    assert!(ap_content(&doc, &line).contains("re h B"), "square ending");

    add_annotation(
        &mut doc,
        &new(0, Shape::PolyLine { vertices: vec![[40.0, 200.0], [90.0, 260.0], [150.0, 200.0]], start: LineEnding::Circle, end: LineEnding::Slash }),
        &meta("poly"),
    )
    .unwrap();
    add_annotation(
        &mut doc,
        &new(
            0,
            Shape::Callout {
                rect: [300.0, 500.0, 420.0, 540.0],
                knee: [260.0, 520.0],
                point: [220.0, 460.0],
                font_size: 10.0,
                ending: LineEnding::Diamond,
            },
        ),
        &meta("call"),
    )
    .unwrap();
    let doc = reopen(&doc);
    let all = list(&doc, 0);
    let poly = all.iter().find(|d| d.name(b"Subtype") == Some(b"PolyLine")).expect("polyline");
    let poly_ap = ap_content(&doc, poly);
    assert!(poly_ap.contains("B\n") && poly_ap.contains(" l S"), "circle fills and the slash only strokes: {poly_ap}");
    let call = all.iter().find(|d| d.name(b"IT") == Some(b"FreeTextCallout")).expect("callout");
    assert_eq!(call.name(b"LE"), Some(b"Diamond".as_slice()));
    assert!(ap_content(&doc, call).contains("h B"), "diamond callout");

    assert!(appearance::build(&bare_doc(), &line_dict([f64::NAN, 0.0, 10.0, 0.0], "OpenArrow")).is_none(), "a non-finite endpoint is rejected");
    assert!(appearance::build(&bare_doc(), &line_dict([0.0, 0.0, 10.0, 0.0], "Foo")).is_none(), "an unknown ending is not replaced");
}

#[test]
fn redraw_keeps_shared_annotation_appearances_and_drops_stale_alternates() {
    let alternates: [(&[u8], &[u8]); 2] = [(b"R", b"0 0 20 10 re f\n"), (b"D", b"1 1 18 8 re S\n")];
    for indirect in [false, true] {
        let mut doc = fixture();
        let first = add_annotation(&mut doc, &new(0, Shape::Rectangle { rect: [10.0, 10.0, 110.0, 60.0] }), &meta("first")).unwrap();
        let other = add_annotation(&mut doc, &new(0, Shape::Rectangle { rect: [120.0, 10.0, 220.0, 60.0] }), &meta("other")).unwrap();
        let (_, first_ref) = annot_ref(&mut doc, 0, first).unwrap();
        let (_, other_ref) = annot_ref(&mut doc, 0, other).unwrap();
        let mut entries = annot_dict(&doc, first_ref).get(b"AP").unwrap().as_dict().unwrap().clone();
        for (key, bytes) in alternates {
            let mut d = Dict::new();
            d.set(b"Type".to_vec(), Object::name("XObject"));
            d.set(b"Subtype".to_vec(), Object::name("Form"));
            d.set(b"BBox".to_vec(), num_array(&[0.0, 0.0, 20.0, 10.0]));
            let r = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(d, bytes.to_vec())));
            entries.set(key.to_vec(), Object::Ref(r));
        }
        entries.set(b"VendorState".to_vec(), Object::name("Retained"));
        let shared = if indirect { Object::Ref(doc.add(Object::Dict(entries))) } else { Object::Dict(entries) };
        for r in [first_ref, other_ref] {
            doc.update_dict(r, |d| d.set(b"AP".to_vec(), shared.clone())).unwrap();
        }
        let mut doc = reopen(&doc);
        let other_dict = list(&doc, 0)[other].clone();
        let source = other_dict.get(b"AP").unwrap();
        let before = doc.resolve(source);
        set_style(&mut doc, 0, first, StyleChange::new(Some([1.0, 0.0, 0.0]), None, None, None), &meta("")).unwrap();
        assert_eq!(doc.resolve(source), before, "a shared AP must not change");
        for full in [false, true] {
            let bytes = if full {
                pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap()
            } else {
                assert!(!doc.revisions().is_empty(), "incremental save needs an existing revision");
                assert!(!doc.encryption_changed() && !doc.full_save_required(), "this edit must not require a full rewrite");
                let original = doc.bytes();
                let bytes = write_incremental(&doc, &SaveOptions::default()).unwrap();
                assert!(bytes.len() > original.len(), "incremental save appends the changed appearance");
                assert!(bytes.starts_with(original.as_slice()), "incremental save preserves the complete original prefix");
                bytes
            };
            hayro_syntax::Pdf::new(bytes.clone()).unwrap();
            let doc = Document::open(Arc::new(bytes)).unwrap();
            let all = list(&doc, 0);
            let first_ap = doc.resolve(all[first].get(b"AP").unwrap());
            let other_ap = doc.resolve(all[other].get(b"AP").unwrap());
            let first_ap = first_ap.as_dict().unwrap();
            let other_ap = other_ap.as_dict().unwrap();
            assert_ne!(first_ap.get(b"N"), other_ap.get(b"N"));
            // The restyled annotation keeps unknown entries, but not down/rollover looks in its
            // old colour.
            assert_eq!(first_ap.len(), 2);
            assert_eq!(other_ap.len(), 4);
            for (key, bytes) in alternates {
                assert!(first_ap.get(key).is_none(), "stale /{}", String::from_utf8_lossy(key));
                let object = doc.resolve(other_ap.get(key).unwrap());
                let Object::Stream(stream) = &*object else { panic!("alternate appearance") };
                assert_eq!(stream.decoded().unwrap(), bytes);
            }
            assert_eq!(first_ap.name(b"VendorState"), Some(&b"Retained"[..]));
            assert_eq!(other_ap.name(b"VendorState"), Some(&b"Retained"[..]));
            assert!(ap_content(&doc, &all[first]).contains("1 0 0 RG"));
            assert!(ap_content(&doc, &all[other]).contains("0 0.4 1 RG"));
        }
    }
}

#[test]
fn non_markup_annotations_are_not_comments() {
    // #169: a LaTeX `animate` player's Screen annotation showed up in the Comments list.
    for s in ["Link", "Widget", "Popup", "Screen", "Movie", "RichMedia", "3D", "PrinterMark", "TrapNet", "Watermark"] {
        assert!(!is_comment_subtype(s), "{s}");
    }
    for s in ["Text", "FreeText", "Highlight", "Ink", "Stamp", "FileAttachment", "Redact", "Sound"] {
        assert!(is_comment_subtype(s), "{s}");
    }
    // #3916: comment edits must refuse non-comment annotations (form widgets, links, pop-ups)
    // when a stale `/Annots` index points at one.
    for subtype in ["Widget", "Link", "Popup", "Watermark"] {
        let mut doc = fixture();
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("Annot"));
        d.set(b"Subtype".to_vec(), Object::name(subtype));
        d.set(b"Rect".to_vec(), num_array(&[10.0, 10.0, 50.0, 30.0]));
        d.set(b"Contents".to_vec(), PdfString::text("keep me"));
        let w = doc.add(Object::Dict(d));
        let p = page_refs(&doc).unwrap()[0];
        let mut a = annots(&doc, p);
        a.push(Object::Ref(w));
        let idx = a.len() - 1;
        set_annots(&mut doc, p, a).unwrap();

        assert_eq!(delete_annotation(&mut doc, 0, idx), Err(AnnotError::NoSuchAnnotation { page: 0, index: idx }), "{subtype}");
        assert_eq!(set_contents(&mut doc, 0, idx, "overwritten", &meta("m")), Err(AnnotError::NoSuchAnnotation { page: 0, index: idx }), "{subtype}");
        assert_eq!(move_annotation(&mut doc, 0, idx, 5.0, 5.0, &meta("m")), Err(AnnotError::NoSuchAnnotation { page: 0, index: idx }), "{subtype}");
        assert_eq!(props(&doc, 0, idx), None, "{subtype}");
        assert_eq!(text(&list(&doc, 0)[idx], b"Contents"), "keep me", "{subtype} contents must be untouched");
    }
}

/// A drawing (a drawn Fill & Sign signature is one) resizes by scaling its strokes into the new
/// rectangle, keeping each side's stroke margin, and gets a fresh appearance.
#[test]
fn drawings_resize_by_scaling_their_strokes() {
    let mut doc = fixture();
    let strokes = vec![vec![[100.0, 400.0], [150.0, 460.0], [200.0, 410.0]], vec![[120.0, 420.0], [180.0, 430.0]]];
    let i = add_annotation(&mut doc, &new(0, Shape::Ink { strokes }), &meta("i")).unwrap();
    let nums = |o: &Object| -> Vec<f64> { o.as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect() };
    let points = |doc: &Document| -> Vec<[f64; 2]> {
        let d = &list(doc, 0)[i];
        d.get(b"InkList").unwrap().as_array().unwrap().iter().flat_map(|s| nums(s).as_chunks::<2>().0.to_vec()).collect()
    };
    let before = points(&doc);
    let r0 = nums(list(&doc, 0)[i].get(b"Rect").unwrap());
    let lo = |k: usize| before.iter().map(|p| p[k]).fold(f64::MAX, f64::min);
    let hi = |k: usize| before.iter().map(|p| p[k]).fold(f64::MIN, f64::max);
    let margin = [lo(0) - r0[0], lo(1) - r0[1], r0[2] - hi(0), r0[3] - hi(1)];
    assert!(margin.iter().all(|m| *m > 0.0), "{margin:?}");
    // Twice the size, bottom-left corner fixed.
    let rect = [r0[0], r0[1], r0[0] + 2.0 * (r0[2] - r0[0]), r0[1] + 2.0 * (r0[3] - r0[1])];
    set_rect(&mut doc, 0, i, rect, &meta("r")).unwrap();
    let d = &list(&doc, 0)[i];
    let r1 = nums(d.get(b"Rect").unwrap());
    assert!(r1.iter().zip(rect).all(|(a, b)| (a - b).abs() < 1e-6), "{r1:?} != {rect:?}");
    // Each point keeps its place relative to the drawing's bounds, inside the same margin.
    let inner = |r: &[f64]| [r[0] + margin[0], r[1] + margin[1], r[2] - margin[2], r[3] - margin[3]];
    let (a, b) = (inner(&r0), inner(&r1));
    for (p, q) in before.iter().zip(points(&doc)) {
        let expected = [b[0] + (p[0] - a[0]) / (a[2] - a[0]) * (b[2] - b[0]), b[1] + (p[1] - a[1]) / (a[3] - a[1]) * (b[3] - b[1])];
        assert!((q[0] - expected[0]).abs() < 1e-6 && (q[1] - expected[1]).abs() < 1e-6, "{q:?} != {expected:?}");
    }
    let ap = ap_content(&doc, d);
    let first = points(&doc)[0];
    assert!(ap.contains(&format!("{} {} m", n(first[0]), n(first[1]))), "appearance redrawn at the new size: {ap}");
    // Too small to hold the stroke margin.
    assert!(matches!(set_rect(&mut doc, 0, i, [100.0, 400.0, 101.0, 401.0], &meta("")), Err(AnnotError::Invalid(_))));
}

/// Comment text the standard fonts can't draw is kept, as Acrobat keeps it: a text box, Fill & Sign
/// text or a dynamic stamp's by-line is added with its text as typed, its appearance draws `?` for
/// those characters, and the comment is marked with the first one (cleared once all of it can be
/// drawn). A comment another program drew with a font that has those characters keeps that
/// appearance: a restyle is refused rather than replace it with `?`, while new text is taken.
#[test]
fn comment_text_the_standard_fonts_cannot_draw_is_kept_and_marked() {
    let mut doc = fixture();
    let mark = |doc: &Document, i: usize| pdfcraft_fonts::undrawn_mark(&list(doc, 0)[i]);
    let mut boxed = new(0, Shape::TextBox { rect: [100.0, 600.0, 300.0, 650.0], font_size: 12.0 });
    boxed.contents = "签名 Ada".into();
    let t = add_annotation(&mut doc, &boxed, &meta("t")).unwrap();
    assert_eq!(text(&list(&doc, 0)[t], b"Contents"), "签名 Ada");
    assert!(ap_content(&doc, &list(&doc, 0)[t]).contains("(?? Ada)"), "{}", ap_content(&doc, &list(&doc, 0)[t]));
    assert_eq!(mark(&doc, t), Some('签'));
    let typed = Shape::Typewriter { rect: [100.0, 700.0, 200.0, 716.0], font_size: 11.0 };
    let f = add_annotation(
        &mut doc,
        &NewAnnotation { page: 0, style: Style::default_for(&typed), shape: typed, contents: "张三".into(), author: "Ada".into() },
        &meta("x"),
    )
    .unwrap();
    assert_eq!(mark(&doc, f), Some('张'));
    let stamp =
        Shape::Stamp { rect: [100.0, 100.0, 260.0, 142.0], stamp: StampKind::DynApproved, by: Some("By 张三 at 2:14 pm, Oct 02, 2026".into()) };
    let s = add_annotation(
        &mut doc,
        &NewAnnotation { page: 0, style: Style::default_for(&stamp), shape: stamp, contents: String::new(), author: "张三".into() },
        &meta("s"),
    )
    .unwrap();
    assert_eq!(mark(&doc, s), Some('张'));
    // Our own `?` appearance is redrawn on a restyle; Western European text clears the mark.
    set_style(&mut doc, 0, t, StyleChange::new(Some([1.0, 0.0, 0.0]), None, None, None), &meta("")).unwrap();
    assert_eq!(mark(&doc, t), Some('签'));
    set_contents(&mut doc, 0, t, "Zürich —\nGenève", &meta("")).unwrap();
    assert_eq!(mark(&doc, t), None);
    set_contents(&mut doc, 0, t, "签名", &meta("")).unwrap();
    assert_eq!((text(&list(&doc, 0)[t], b"Contents").as_str(), mark(&doc, t)), ("签名", Some('签')));
    // A text box another program drew (with a font that has 欢): a restyle keeps that appearance.
    let mut foreign = Dict::new();
    foreign.set(b"Type".to_vec(), Object::name("Annot"));
    foreign.set(b"Subtype".to_vec(), Object::name("FreeText"));
    foreign.set(b"Rect".to_vec(), Object::Array([100.0, 400.0, 300.0, 430.0].iter().map(|v| Object::Real(*v)).collect()));
    foreign.set(b"DA".to_vec(), PdfString::literal(b"0 g /Helv 12 Tf".to_vec()));
    foreign.set(b"Contents".to_vec(), PdfString::text("欢迎"));
    let mut drawn = Dict::new();
    drawn.set(b"Type".to_vec(), Object::name("XObject"));
    drawn.set(b"Subtype".to_vec(), Object::name("Form"));
    drawn.set(b"BBox".to_vec(), Object::Array([0.0, 0.0, 200.0, 30.0].iter().map(|v| Object::Real(*v)).collect()));
    let drawn = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(drawn, b"BT /F1 12 Tf (\x01\x02) Tj ET".to_vec())));
    let mut ap = Dict::new();
    ap.set(b"N".to_vec(), Object::Ref(drawn));
    foreign.set(b"AP".to_vec(), Object::Dict(ap));
    let r = doc.add(Object::Dict(foreign));
    let page = page_ref(&doc, 0).unwrap();
    let mut on_page = annots(&doc, page);
    on_page.push(Object::Ref(r));
    set_annots(&mut doc, page, on_page).unwrap();
    let k = list(&doc, 0).len() - 1;
    let before = list(&doc, 0)[k].clone();
    let err = set_style(&mut doc, 0, k, StyleChange::new(Some([1.0, 0.0, 0.0]), Some(0.4), Some(6.0), None), &meta("")).unwrap_err();
    assert!(matches!(&err, AnnotError::Undrawable(m) if m.contains("can't redraw") && m.contains("\"欢\" (U+6B22)")), "{err:?}");
    assert_eq!(list(&doc, 0)[k], before, "a refused restyle keeps the colour, opacity, width and appearance");
    let err = set_rect(&mut doc, 0, k, [40.0, 40.0, 180.0, 90.0], &meta("")).unwrap_err();
    assert!(matches!(&err, AnnotError::Undrawable(_)), "{err:?}");
    assert_eq!(list(&doc, 0)[k], before, "a refused resize keeps the rectangle");
    // New text from the user is taken (and marked), as for any comment.
    set_contents(&mut doc, 0, k, "欢迎光临", &meta("")).unwrap();
    assert_eq!(mark(&doc, k), Some('欢'));
    // One without an appearance is shown in the display copy only; drawing it marks it.
    let mut bare = Dict::new();
    bare.set(b"Type".to_vec(), Object::name("Annot"));
    bare.set(b"Subtype".to_vec(), Object::name("FreeText"));
    bare.set(b"Rect".to_vec(), Object::Array([100.0, 300.0, 300.0, 330.0].iter().map(|v| Object::Real(*v)).collect()));
    bare.set(b"DA".to_vec(), PdfString::literal(b"0 g /Helv 12 Tf".to_vec()));
    bare.set(b"Contents".to_vec(), PdfString::text("欢迎"));
    let b = doc.add(Object::Dict(bare));
    let mut on_page = annots(&doc, page);
    on_page.push(Object::Ref(b));
    set_annots(&mut doc, page, on_page).unwrap();
    let shown = with_missing_appearances(&doc).expect("a display copy");
    assert!(shown.get(b).as_dict().unwrap().contains(b"AP"), "displayed anyway");
    assert!(!doc.get(b).as_dict().unwrap().contains(b"AP"), "the document itself is unchanged");
    set_appearance(&mut doc, b).unwrap();
    assert_eq!(doc.get(b).as_dict().and_then(pdfcraft_fonts::undrawn_mark), Some('欢'));
    // Kept through a save.
    let doc = reopen(&doc);
    assert_eq!(text(&list(&doc, 0)[t], b"Contents"), "签名");
}

/// Content edits rebuild the display copy on every commit but leave annotations alone: the
/// cached appearance is drawn once and reused after that, byte-identical.
#[test]
fn cached_appearances_are_reused_while_annotations_are_unchanged() {
    let mut doc = fixture();
    let idx = add_annotation(&mut doc, &new(0, Shape::Note { at: [100.0, 700.0], icon: NoteIcon::Comment }), &meta("cache")).expect("adds");
    // Files carry comments without appearances; strip the created one to simulate that.
    let page = page_refs(&doc).unwrap()[0];
    let r = annots(&doc, page)[idx].as_ref().expect("indirect");
    doc.update_dict(r, |d| {
        d.remove(b"AP");
    })
    .expect("strips");
    let mut cache = AppearanceCache::default();
    let first = with_missing_appearances_cached(&doc, &mut cache).expect("builds");
    assert_eq!((cache.builds(), cache.hits()), (2, 0), "the note and the file's stamp need an appearance");
    let second = with_missing_appearances_cached(&doc, &mut cache).expect("reuses");
    assert_eq!((cache.builds(), cache.hits()), (2, 2), "nothing is redrawn");
    let [a, b] = [&first, &second].map(|copy| ap_content(copy, &list(copy, 0)[idx]));
    assert!(!a.is_empty());
    assert_eq!(a, b, "the reused appearance is byte-identical");
}

/// A comment that `/Annots` lists many times, and that a second page lists too, is drawn once in
/// the display copy. Every listing used to add another appearance stream, so a short array of
/// repeated references multiplied the display bytes the engine writes on each refresh.
#[test]
fn a_comment_listed_many_times_is_drawn_once_for_display() {
    let first = format!("<< /Type /Page /Parent 2 0 R /Annots [{}] >>", vec!["5 0 R"; 1_000].join(" "));
    let doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 612 792] >>",
        first.as_bytes(),
        b"<< /Type /Page /Parent 2 0 R /Annots [5 0 R] >>",
        b"<< /Type /Annot /Subtype /Square /Rect [10 10 50 50] /C [1 0 0] >>",
    ]);
    let mut cache = AppearanceCache::default();
    let display = with_missing_appearances_cached(&doc, &mut cache).expect("the square needs an appearance");
    assert_eq!(display.object_numbers().len(), doc.object_numbers().len() + 1, "one appearance stream, not one per listing");
    assert_eq!((cache.builds(), cache.hits()), (1, 0));
    assert!(list(&display, 0).iter().chain(&list(&display, 1)).all(|d| d.contains(b"AP")), "every listing shows it");
    with_missing_appearances_cached(&doc, &mut cache).expect("a refresh");
    assert_eq!((cache.builds(), cache.hits()), (1, 1), "a refresh reuses the cached stream once");
}

/// #1908: a text box's font size changes in `/DA` (colour kept), the rich text follows, the box
/// grows to hold the bigger text but never shrinks, and bad sizes and other subtypes are refused.
#[test]
fn text_box_font_size_changes_the_da_and_grows_the_box() {
    let mut doc = fixture();
    let t = add_annotation(&mut doc, &new(0, Shape::TextBox { rect: [100.0, 680.0, 220.0, 692.0], font_size: 12.0 }), &meta("t")).unwrap();
    set_style(&mut doc, 0, t, StyleChange::new(Some([1.0, 0.0, 0.0]), None, None, None), &meta("")).unwrap();
    set_contents(&mut doc, 0, t, "one two three four five six seven eight nine ten eleven twelve", &meta("")).unwrap();
    let height = |doc: &Document| {
        let rect: Vec<f64> = list(doc, 0)[t].get(b"Rect").unwrap().as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
        rect[3] - rect[1]
    };
    let before = height(&doc);
    set_font_size(&mut doc, 0, t, 24.0, &meta("")).unwrap();
    let doc = reopen(&doc);
    let d = &list(&doc, 0)[t];
    assert_eq!(text(d, b"DA"), "1 0 0 rg /Helv 24 Tf");
    assert!(text(d, b"DS").contains("24pt") && text(d, b"RC").contains("font-size:24pt"));
    assert_eq!(props(&doc, 0, t).unwrap().font_size, Some(24.0));
    assert!(height(&doc) > before, "the box grows");
    let mut doc = doc;
    let grown = height(&doc);
    set_font_size(&mut doc, 0, t, 6.0, &meta("")).unwrap();
    assert!(height(&doc) >= grown, "the box never shrinks");
    for bad in [0.0, 0.5, 401.0, f64::NAN, f64::INFINITY] {
        assert!(matches!(set_font_size(&mut doc, 0, t, bad, &meta("")), Err(AnnotError::Invalid(_))), "{bad}");
    }
    assert_eq!(props(&doc, 0, t).unwrap().font_size, Some(6.0));
    let note = add_annotation(&mut doc, &new(0, Shape::Note { at: [50.0, 50.0], icon: NoteIcon::Note }), &meta("t"));
    if let Ok(i) = note {
        assert!(set_font_size(&mut doc, 0, i, 12.0, &meta("")).is_err());
        assert_eq!(props(&doc, 0, i).unwrap().font_size, None);
    }
}

#[test]
fn text_boxes_and_callouts_can_change_fill_and_border() {
    let mut doc = fixture();
    let shapes = [
        Shape::TextBox { rect: [300.0, 600.0, 500.0, 660.0], font_size: 12.0 },
        Shape::Callout {
            rect: [300.0, 600.0, 500.0, 660.0],
            knee: [260.0, 560.0],
            point: [200.0, 520.0],
            font_size: 12.0,
            ending: LineEnding::OpenArrow,
        },
    ];
    for shape in shapes {
        let index = add_annotation(&mut doc, &new(0, shape), &meta("test")).unwrap();
        assert!(props(&doc, 0, index).unwrap().fillable);
        set_fill(&mut doc, 0, index, Some([1.0, 1.0, 1.0]), &meta("test")).unwrap();
        set_style(&mut doc, 0, index, StyleChange::new(None, None, Some(3.0), None), &meta("test")).unwrap();
        let saved = reopen(&doc);
        let properties = props(&saved, 0, index).unwrap();
        assert_eq!(properties.fill, Some([1.0; 3]));
        assert_eq!(properties.width, Some(3.0));
        let ap = ap_content(&saved, &list(&saved, 0)[index]);
        assert!(ap.contains("1 1 1 rg") && ap.contains(" re f") && ap.contains("3 w"), "{ap}");
        set_fill(&mut doc, 0, index, None, &meta("test")).unwrap();
        assert_eq!(props(&doc, 0, index).unwrap().fill, None);
    }
}

#[test]
fn free_text_properties_show_legacy_or_absent_border_width() {
    let mut doc = fixture();
    let index = add_annotation(&mut doc, &new(0, Shape::TextBox { rect: [100.0, 100.0, 200.0, 140.0], font_size: 12.0 }), &meta("test")).unwrap();
    let (_, reference) = annot_ref(&mut doc, 0, index).unwrap();
    doc.update_dict(reference, |d| {
        d.remove(b"BS");
        d.remove(b"Border");
    })
    .unwrap();
    // Neither /BS nor /Border: the /Border default [0 0 1] (ISO 32000-2 Table 166), as drawn.
    assert_eq!(props(&doc, 0, index).unwrap().width, Some(1.0));
    doc.update_dict(reference, |d| {
        d.set(b"Border".to_vec(), Object::Array([0.0, 0.0, 4.0].into_iter().map(Object::Real).collect()));
    })
    .unwrap();
    assert_eq!(props(&doc, 0, index).unwrap().width, Some(4.0));
}

/// A line's ends and the corners of connected lines and polygons can be moved after drawing:
/// the points change, `/Rect` follows them (with room for an arrowhead), and the appearance is
/// drawn anew. A line needs two ends far enough apart; a locked comment keeps its shape.
#[test]
fn lines_and_polygons_are_reshaped_by_their_points() {
    let mut doc = fixture();
    let l = add_annotation(
        &mut doc,
        &new(0, Shape::Line { from: [100.0, 100.0], to: [200.0, 100.0], start: LineEnding::None, end: LineEnding::OpenArrow }),
        &meta("l"),
    )
    .unwrap();
    let before = ap_content(&doc, &list(&doc, 0)[l]);
    set_points(&mut doc, 0, l, &[[100.0, 100.0], [100.0, 300.0]], &meta("")).unwrap();
    let doc2 = reopen(&doc);
    let d = &list(&doc2, 0)[l];
    let ends: Vec<f64> = d.get(b"L").unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect();
    assert_eq!(ends, [100.0, 100.0, 100.0, 300.0]);
    let r = rect(d);
    assert!(r[0] < 100.0 && r[2] > 100.0 && r[1] < 100.0 && r[3] > 300.0, "the rectangle holds the line and its arrowhead: {r:?}");
    assert!(r[2] - r[0] < 40.0, "and no more: {r:?}");
    assert_ne!(ap_content(&doc2, d), before, "drawn anew");
    assert_eq!(summaries(&doc2).into_iter().find(|s| s.index == l).unwrap().points, vec![[100.0, 100.0], [100.0, 300.0]]);
    // Two ends exactly, far enough apart.
    assert!(matches!(set_points(&mut doc, 0, l, &[[1.0, 1.0]], &meta("")), Err(AnnotError::Invalid(_))));
    assert!(matches!(set_points(&mut doc, 0, l, &[[1.0, 1.0], [1.2, 1.0]], &meta("")), Err(AnnotError::Invalid(_))));
    assert!(matches!(set_points(&mut doc, 0, l, &[[f64::NAN, 1.0], [10.0, 1.0]], &meta("")), Err(AnnotError::Invalid(_))));
    // A polygon's corner.
    let g = add_annotation(
        &mut doc,
        &new(0, Shape::Polygon { vertices: vec![[300.0, 300.0], [400.0, 300.0], [350.0, 380.0]], cloud: false }),
        &meta("g"),
    )
    .unwrap();
    set_points(&mut doc, 0, g, &[[300.0, 300.0], [450.0, 300.0], [350.0, 380.0], [300.0, 360.0]], &meta("")).unwrap();
    let d = &list(&doc, 0)[g];
    let v: Vec<f64> = d.get(b"Vertices").unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect();
    assert_eq!(v.len(), 8);
    assert!(rect(d)[2] > 450.0);
    // Locked: refused, and nothing changes.
    set_locked(&mut doc, 0, l, true).unwrap();
    assert!(set_points(&mut doc, 0, l, &[[0.0, 0.0], [50.0, 50.0]], &meta("")).is_err());
    // Other comments have no points to move.
    let s = add_annotation(&mut doc, &new(0, Shape::Rectangle { rect: [10.0, 10.0, 50.0, 50.0] }), &meta("s")).unwrap();
    assert!(matches!(set_points(&mut doc, 0, s, &[[0.0, 0.0], [50.0, 50.0]], &meta("")), Err(AnnotError::Invalid(_))));
}

#[test]
fn page_walk_skips_repeated_and_cyclic_kids_without_queueing_them() {
    let mut doc = fixture();
    let root = ObjRef { num: 2, generation: 0 };
    let (first, second) = (ObjRef { num: 3, generation: 0 }, ObjRef { num: 4, generation: 0 });
    // A cycle back to the root, then both pages 128 times over.
    let mut kids = vec![Object::Ref(root)];
    kids.extend([Object::Ref(first), Object::Ref(second)].into_iter().cycle().take(256));
    doc.update_dict(root, |d| d.set(b"Kids".to_vec(), Object::Array(kids))).unwrap();
    let mut walk = PageWalk::new(&doc, root);
    assert_eq!(walk.next(), Some(first));
    assert!(walk.pending.len() <= 2, "repeated kids retained {} pending entries", walk.pending.len());
    assert_eq!(walk.next(), Some(second));
    assert_eq!(walk.next(), None);
    assert_eq!(page_refs(&doc).unwrap(), [first, second]);
}

/// #1786: TrueType glyph contours (such as Inter's composite Ђ and ћ) overlap with the same
/// winding direction where strokes cross, so typed signature appearances must fill with the
/// non-zero winding operator (`f`), not even-odd (`f*`).
#[test]
fn typed_signature_uses_nonzero_winding_fill() {
    let mut doc = fixture();
    let contours = vec![vec![[0.4, 0.0], [0.6, 0.0], [0.6, 1.0], [0.4, 1.0]], vec![[0.0, 0.7], [1.0, 0.7], [1.0, 0.85], [0.0, 0.85]]];
    let idx = add_annotation(&mut doc, &new(0, Shape::TypedSignature { rect: [100.0, 100.0, 200.0, 140.0], contours }), &meta("sig")).unwrap();
    let ap = ap_content(&doc, &list(&doc, 0)[idx]);
    assert!(ap.ends_with("h\nf\n"), "expected non-zero winding fill (`f`), got: {ap}");
    assert!(!ap.contains("f*"), "must not use even-odd fill (`f*`): {ap}");
}

/// Many page tree nodes sharing one `/Kids` array read it once, not once per node.
#[test]
fn page_walk_reads_a_shared_kids_array_once() {
    let mut doc = fixture();
    let root = ObjRef { num: 2, generation: 0 };
    let (first, second) = (ObjRef { num: 3, generation: 0 }, ObjRef { num: 4, generation: 0 });
    let shared = doc.add(Object::Array([Object::Ref(first), Object::Ref(second)].into_iter().cycle().take(2_000).collect()));
    let nodes: Vec<Object> = (0..1_000)
        .map(|_| {
            let mut d = Dict::new();
            d.set(b"Type".to_vec(), Object::name("Pages"));
            d.set(b"Kids".to_vec(), Object::Ref(shared));
            Object::Ref(doc.add(d))
        })
        .collect();
    doc.update_dict(root, |d| d.set(b"Kids".to_vec(), Object::Array(nodes))).unwrap();
    let mut walk = PageWalk::new(&doc, root);
    let pages: Vec<ObjRef> = walk.by_ref().collect();
    assert_eq!(pages, [first, second]);
    assert!(walk.examined <= 10_000, "{} kids entries read", walk.examined);
}

/// Adding many links to a page extends its `/Annots` once, not once per link.
#[test]
fn many_links_extend_a_page_annotation_list_once() {
    let mut doc = fixture();
    let before = annots(&doc, page_refs(&doc).unwrap()[0]).len();
    let n = 2_000;
    let rects: Vec<[f64; 4]> = (0..n).map(|i| [10.0, i as f64 * 0.1, 50.0, i as f64 * 0.1 + 5.0]).collect();
    ANNOTS_ENTRIES_COPIED.with(|c| c.set(0));
    let added = links::add_many(&mut doc, &[(0, rects, "https://example.com".into())], &links::LinkStyle::default()).unwrap();
    let copied = ANNOTS_ENTRIES_COPIED.with(std::cell::Cell::get);
    assert_eq!(added, n);
    assert_eq!(annots(&doc, page_refs(&doc).unwrap()[0]).len(), before + n);
    assert!(copied <= 4 * n, "{copied} annotation entries copied for {n} links");
}

/// A batch with one bad entry adds nothing: no link objects are left behind unattached.
#[test]
fn a_link_batch_with_a_bad_entry_creates_no_objects() {
    let mut doc = fixture();
    let page = page_refs(&doc).unwrap()[0];
    let before = (annots(&doc, page).len(), doc.scan_objects().count());
    let good = [10.0, 10.0, 50.0, 20.0];
    let style = links::LinkStyle::default();
    for bad in [
        vec![(0, vec![good], "https://example.com".to_string()), (0, vec![good, [f64::NAN, 0.0, 10.0, 10.0]], "https://example.com".into())],
        vec![(0, vec![good], "https://example.com".to_string()), (0, vec![good], "  ".into())],
        vec![(0, vec![good], "https://example.com".to_string()), (99, vec![good], "https://example.com".into())],
    ] {
        assert!(links::add_many(&mut doc, &bad, &style).is_err());
        assert_eq!((annots(&doc, page).len(), doc.scan_objects().count()), before);
    }
}

fn doc_of(objs: &[String]) -> Document {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).expect("opens")
}

#[test]
fn link_listing_skips_repeated_refs_and_caps_web_address_text() {
    // One link referenced 1000 times, and 300 small links sharing one 64 KiB address.
    let annots = format!("{}{}", " 4 0 R".repeat(1000), "<< /Subtype /Link /Rect [0 0 1 1] /A 5 0 R >>".repeat(300));
    let doc = doc_of(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>".into(),
        format!("<< /Type /Page /Parent 2 0 R /Annots [{annots}] >>"),
        "<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /A 5 0 R >>".into(),
        "<< /S /URI /URI 6 0 R >>".into(),
        format!("({})", "u".repeat(64 << 10)),
    ]);
    let all = links::list(&doc);
    assert_eq!(all.iter().filter(|l| l.obj == Some(ObjRef::new(4, 0))).count(), 1);
    assert_eq!(all.len(), 301, "every distinct link is still listed");
    let kept: usize = all.iter().map(|l| if let links::LinkAction::Uri(u) = &l.action { u.len() } else { 0 }).sum();
    assert!(kept <= links::MAX_URI_BYTES, "{kept} bytes of web addresses kept");
    assert!(matches!(&all[0].action, links::LinkAction::Uri(u) if u.len() == 64 << 10));
}

#[test]
fn web_address_cap_counts_the_text_kept_after_conversion() {
    // 200 links sharing a 64 KiB address of invalid UTF-8: 12.5 MiB of bytes, each byte kept as a
    // three-byte replacement character.
    let annots = "<< /Subtype /Link /Rect [0 0 1 1] /A 4 0 R >>".repeat(200);
    let doc = doc_of(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>".into(),
        format!("<< /Type /Page /Parent 2 0 R /Annots [{annots}] >>"),
        "<< /S /URI /URI 5 0 R >>".into(),
        format!("<{}>", "FF".repeat(64 << 10)),
    ]);
    let all = links::list(&doc);
    assert_eq!(all.len(), 200);
    let kept: usize = all.iter().map(|l| if let links::LinkAction::Uri(u) = &l.action { u.len() } else { 0 }).sum();
    assert!(kept <= links::MAX_URI_BYTES, "{kept} bytes of web addresses kept");
    assert!(kept > 0);
}

#[test]
fn cloud_outline_has_a_bump_budget() {
    // 50 long edges would each ask for 10,000 bumps of radius 1.
    let pts: Vec<(f64, f64)> = (0..50).map(|i| (f64::from(i) * std::f64::consts::TAU / 50.0).sin_cos()).map(|(s, c)| (1e7 * c, 1e7 * s)).collect();
    let bumps = appearance::cloud_path(&pts, 1.0).lines().filter(|l| l.ends_with(" c")).count();
    assert!((pts.len()..=appearance::MAX_CLOUD_BUMPS + 2 * pts.len()).contains(&bumps), "{bumps} bumps");
}

/// A note's drawn icon (its built appearance's content) for `/Name` (none when `None`) and `/C`.
fn note_drawing(name: Option<&str>, color: [f64; 3]) -> Vec<u8> {
    let mut d = Dict::new();
    d.set(b"Subtype".to_vec(), Object::name("Text"));
    d.set(b"Rect".to_vec(), Object::Array([0.0, 0.0, 20.0, 20.0].map(Object::Real).to_vec()));
    d.set(b"C".to_vec(), Object::Array(color.map(Object::Real).to_vec()));
    if let Some(n) = name {
        d.set(b"Name".to_vec(), Object::name(n));
    }
    crate::appearance::build(&bare_doc(), &d).expect("a note is drawn").decoded().unwrap()
}

/// ISO 32000-2 §12.5.6.4, Table 175: a viewer shall provide icons for the seven standard names,
/// and may support more. Each of the fourteen names PdfCraft offers draws a shape of its own, so
/// a note shows the icon it names; a name it doesn't know, and a note without one, draw `Note`
/// (the table's default).
#[test]
fn every_note_icon_draws_its_own_shape() {
    let yellow = [1.0, 0.82, 0.0];
    let drawn: Vec<Vec<u8>> = NoteIcon::ALL.iter().map(|i| note_drawing(Some(i.name()), yellow)).collect();
    for (i, a) in drawn.iter().enumerate() {
        for (j, b) in drawn.iter().enumerate().skip(i + 1) {
            assert_ne!(a, b, "{} and {} look alike", NoteIcon::ALL[i].name(), NoteIcon::ALL[j].name());
        }
    }
    let note = note_drawing(Some("Note"), yellow);
    assert_eq!(note_drawing(Some("Sparkle"), yellow), note, "an unknown name");
    assert_eq!(note_drawing(None, yellow), note, "no name");
    for icon in NoteIcon::ALL {
        assert_eq!(NoteIcon::from_name(icon.name()), Some(icon));
    }
}

/// A note named with one of the further icons reads as that icon, and a colour change redraws it
/// as itself, not as a page.
#[test]
fn a_star_note_keeps_its_star_when_recoloured() {
    let mut doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 600] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        b"<< /Type /Annot /Subtype /Text /Rect [100 100 120 120] /Name /Star /C [1 0.82 0] /F 4 >>",
    ]);
    assert_eq!(props(&doc, 0, 0).and_then(|p| p.icon), Some(NoteIcon::Star));
    set_style(&mut doc, 0, 0, StyleChange::new(Some([0.0, 0.0, 1.0]), None, None, None), &meta("")).unwrap();
    let doc = reopen(&doc);
    let d = &list(&doc, 0)[0];
    assert_eq!(d.name(b"Name"), Some(&b"Star"[..]));
    let n = d.get(b"AP").unwrap().as_dict().unwrap().reference(b"N").unwrap();
    let Object::Stream(s) = &*doc.get(n) else { panic!() };
    assert_eq!(s.decoded().unwrap(), note_drawing(Some("Star"), [0.0, 0.0, 1.0]));
}

/// The built appearance's content for an annotation dictionary written in PDF syntax.
fn content_of(src: &str) -> String {
    let doc = document_of(&[src.as_bytes()]);
    let d = doc.get(ObjRef::new(1, 0)).as_dict().cloned().expect("a dictionary");
    let stream = appearance::build(&bare_doc(), &d).expect("drawn");
    String::from_utf8_lossy(&stream.decoded().unwrap()).into_owned()
}

/// ISO 32000-2 Table 166: `/Border [hr vr w [dash]]`; the optional fourth element, a dash array,
/// used to hide the width (drawn 1 pt solid instead of 3 pt dashed).
#[test]
fn legacy_border_width_and_dash_are_drawn() {
    let square = |border: &str| content_of(&format!("<< /Subtype /Square /Rect [0 0 100 100] /C [1 0 0] {border} >>"));
    let c = square("/Border [0 0 3 [6 3]]");
    assert!(c.contains("3 w") && c.contains("[6 3] 0 d"), "{c}");
    assert!(c.contains("1.5 1.5 97 97 re"), "the stroke is inset by half its width: {c}");
    let c = square("/Border [0 0 3]");
    assert!(c.contains("3 w") && !c.contains(" d\n"), "no dash array: solid: {c}");
    // `/BS` overrides `/Border` (Table 166), for the width and the dashes alike.
    let c = square("/Border [0 0 3 [6 3]] /BS << /W 2 >>");
    assert!(c.contains("2 w") && !c.contains(" d\n"), "{c}");
    let c = square("/Border [0 0 3 [6 3]] /BS << /W 2 /S /D /D [4 1] >>");
    assert!(c.contains("2 w") && c.contains("[4 1] 0 d"), "{c}");
    // A /BS without /W means its default width 1, not the ignored /Border's.
    let c = square("/Border [0 0 3] /BS << /S /S >>");
    assert!(c.contains("1 w") && !c.contains("3 w"), "{c}");
    // A FreeText border and a line take the legacy dash array too.
    let c = content_of("<< /Subtype /FreeText /Rect [0 0 100 40] /Contents (x) /Border [0 0 2 [5 2]] >>");
    assert!(c.contains("2 w") && c.contains("[5 2] 0 d"), "{c}");
    let c = content_of("<< /Subtype /Line /Rect [0 0 100 100] /L [10 10 90 90] /C [0 0 1] /Border [0 0 2 [5 2]] >>");
    assert!(c.contains("[5 2] 0 d"), "{c}");
}

/// Dash arrays that can't be drawn (§8.4.3.6: all zeros is invalid; negative or non-numeric
/// lengths, and absurdly long arrays) draw solid, without a panic. A non-finite or negative
/// width falls back to 1.
#[test]
fn bad_dash_arrays_draw_solid() {
    let square = |border: &str| content_of(&format!("<< /Subtype /Square /Rect [0 0 100 100] /C [1 0 0] {border} >>"));
    let long = format!("[{}]", vec!["1"; 65].join(" "));
    for bad in ["[0 0]", "[-3 2]", "[]", "[(a) 2]", "5", long.as_str()] {
        let c = square(&format!("/Border [0 0 3 {bad}]"));
        assert!(c.contains("3 w") && !c.contains(" d\n"), "{bad}: {c}");
        let c = square(&format!("/BS << /W 3 /S /D /D {bad} >>"));
        assert!(c.contains("3 w") && !c.contains(" d\n"), "{bad}: {c}");
    }
    assert!(square(&format!("/Border [0 0 3 [{}]]", vec!["1"; 64].join(" "))).contains(" 0 d\n"), "64 elements are drawn");
    // `/S /D` without `/D` uses the default `[3]` (Table 168).
    assert!(square("/BS << /W 3 /S /D >>").contains("[3] 0 d"));
    let mut d = Dict::new();
    d.set(b"Subtype".to_vec(), Object::name("Square"));
    d.set(b"Rect".to_vec(), Object::Array([0.0, 0.0, 100.0, 100.0].map(Object::Real).to_vec()));
    d.set(b"C".to_vec(), Object::Array([1.0, 0.0, 0.0].map(Object::Real).to_vec()));
    for w in [f64::NAN, f64::INFINITY, -2.0] {
        let dash = Object::Array([f64::NAN, 2.0].map(Object::Real).to_vec());
        d.set(b"Border".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Real(w), dash]));
        let c = String::from_utf8_lossy(&appearance::build(&bare_doc(), &d).unwrap().decoded().unwrap()).into_owned();
        assert!(c.contains("1 w") && !c.contains(" d\n") && !c.contains("NaN") && !c.contains("inf"), "{w}: {c}");
    }
}

/// Table 182: an ink annotation's `/BS` gives the width and dash pattern its paths are drawn with.
#[test]
fn ink_draws_its_border_style_dashes() {
    let ink = |style: &str| content_of(&format!("<< /Subtype /Ink /Rect [0 0 100 100] /C [1 0 0] /InkList [[5 5 50 95 95 5]] {style} >>"));
    let c = ink("/BS << /W 3 /S /D /D [6 3] >>");
    assert!(c.contains("3 w 1 J 1 j\n[6 3] 0 d"), "{c}");
    assert!(!ink("/BS << /W 3 >>").contains(" d\n"));
    assert!(ink("/Border [0 0 2 [4 4]]").contains("[4 4] 0 d"));
}

/// Table 183: `/Sy /P` associates a new paragraph symbol (¶) with the caret. It is drawn to the
/// caret's right in the caret's colour, inside `/Rect`; `/Sy /None` (the default) draws none.
#[test]
fn a_caret_with_a_paragraph_symbol_draws_the_pilcrow() {
    let caret = |extra: &str| {
        let doc = document_of(&[format!("<< /Subtype /Caret /Rect [30 30 60 60] /C [0.9 0.1 0.1] {extra} >>").as_bytes()]);
        let d = doc.get(ObjRef::new(1, 0)).as_dict().cloned().unwrap();
        let s = appearance::build(&bare_doc(), &d).unwrap();
        (s.decoded().unwrap(), s.dict.get(b"Resources").and_then(Object::as_dict).is_some_and(|r| r.contains(b"Font")))
    };
    let has_pilcrow = |c: &[u8]| c.windows(5).any(|w| w == b"(\xb6) T");
    let (c, font) = caret("/Sy /P");
    assert!(has_pilcrow(&c) && font, "{}", String::from_utf8_lossy(&c));
    let text = String::from_utf8_lossy(&c).into_owned();
    assert!(text.starts_with("0.9 0.1 0.1 rg"), "{text}");
    // The caret gives up room on the right for the symbol, which stays within /Rect.
    let td: Vec<f64> = text.split("Tf ").nth(1).unwrap().split_whitespace().take(2).map(|v| v.parse().unwrap()).collect();
    let size: f64 = text.split("/Helv ").nth(1).unwrap().split_whitespace().next().unwrap().parse().unwrap();
    assert!(td[0] > 45.0 && td[0] + appearance::text_width("\u{b6}", size) <= 60.0 + 1e-6 && td[1] >= 30.0, "{text}");
    assert!(!text.contains("60 30 c h f"), "the caret no longer reaches the right edge: {text}");
    for none in ["/Sy /None", ""] {
        let (c, font) = caret(none);
        assert!(!has_pilcrow(&c) && !font, "{none}");
        assert!(String::from_utf8_lossy(&c).contains("60 30 c h f"), "{none}: the caret fills /Rect as before");
    }
    // With /RD leaving room on the right, the caret keeps its box and the symbol fits beside it.
    let (c, _) = caret("/Sy /P /RD [0 0 15 0]");
    let text = String::from_utf8_lossy(&c).into_owned();
    assert!(text.contains("45 30 c h f") && has_pilcrow(&c), "{text}");
    // Malformed /RD and a degenerate /Rect don't panic.
    for rd in ["/RD [40 0 40 0]", "/RD [-1 0 0 0]", "/RD [1 2]", "/RD (x)"] {
        assert!(has_pilcrow(&caret(&format!("/Sy /P {rd}")).0), "{rd}");
    }
    let doc = document_of(&[b"<< /Subtype /Caret /Rect [30 30 30 30] /Sy /P >>"]);
    let d = doc.get(ObjRef::new(1, 0)).as_dict().cloned().unwrap();
    assert!(appearance::build(&bare_doc(), &d).is_some());
}

// ── Line annotations: leader lines and captions (ISO 32000-2 §12.5.6.7, Table 178) ──────────

/// A red Line from `l` with a 1 pt border, a generous `/Rect`, and `extra` entries.
fn table_178_line(l: [f64; 4], extra: &[(&str, Object)]) -> Dict {
    let mut d = Dict::new();
    d.set(b"Subtype".to_vec(), Object::name("Line"));
    d.set(b"Rect".to_vec(), Object::Array([0.0, 0.0, 600.0, 800.0].map(Object::Real).to_vec()));
    d.set(b"C".to_vec(), Object::Array([1.0, 0.0, 0.0].map(Object::Real).to_vec()));
    d.set(b"L".to_vec(), Object::Array(l.map(Object::Real).to_vec()));
    for (k, v) in extra {
        d.set(k.as_bytes().to_vec(), v.clone());
    }
    d
}

/// The built appearance's content.
fn line_drawing(d: &Dict) -> String {
    let s = crate::appearance::build(&bare_doc(), d).expect("the line is drawn");
    String::from_utf8_lossy(&s.decoded().unwrap()).into_owned()
}

/// Every straight two-point stroke (`x0 y0 m x1 y1 l S`) in a drawing.
fn segments(content: &str) -> Vec<[f64; 4]> {
    content
        .lines()
        .filter_map(|line| {
            let t: Vec<&str> = line.split_whitespace().collect();
            if t.len() != 7 || t[2] != "m" || t[5] != "l" || t[6] != "S" {
                return None;
            }
            let v: Vec<f64> = [t[0], t[1], t[3], t[4]].iter().map(|x| x.parse().unwrap()).collect();
            Some([v[0], v[1], v[2], v[3]])
        })
        .collect()
}

/// The caption's text matrix (`a b c d e f Tm`), if any.
fn caption_matrix(content: &str) -> Option<[f64; 6]> {
    let at = content.find(" Tm")?;
    let before: Vec<f64> = content[..at].split_whitespace().rev().take(6).map(|x| x.parse().unwrap()).collect();
    Some([before[5], before[4], before[3], before[2], before[1], before[0]])
}

fn has(segs: &[[f64; 4]], want: [f64; 4]) -> bool {
    segs.iter().any(|s| s.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.01))
}

#[test]
fn a_positive_leader_length_puts_the_line_above_a_left_to_right_line_and_a_negative_one_below() {
    let up = segments(&line_drawing(&table_178_line([72.0, 620.0, 272.0, 620.0], &[("LL", Object::Real(20.0))])));
    assert!(has(&up, [72.0, 640.0, 272.0, 640.0]), "the line is 20 above /L: {up:?}");
    assert!(has(&up, [72.0, 620.0, 72.0, 640.0]) && has(&up, [272.0, 620.0, 272.0, 640.0]), "leader lines from /L up to it: {up:?}");
    assert!(!has(&up, [72.0, 620.0, 272.0, 620.0]), "nothing along /L itself: {up:?}");

    let down = segments(&line_drawing(&table_178_line([72.0, 540.0, 272.0, 540.0], &[("LL", Object::Real(-20.0)), ("LLE", Object::Real(10.0))])));
    assert!(has(&down, [72.0, 520.0, 272.0, 520.0]), "the line is 20 below /L: {down:?}");
    assert!(has(&down, [72.0, 540.0, 72.0, 510.0]) && has(&down, [272.0, 540.0, 272.0, 510.0]), "leaders run 10 past it: {down:?}");
}

#[test]
fn a_leader_offset_leaves_a_gap_and_the_endings_sit_on_the_line_itself() {
    let d = table_178_line(
        [72.0, 460.0, 272.0, 460.0],
        &[("LL", Object::Real(20.0)), ("LLO", Object::Real(10.0)), ("LE", Object::Array(vec![Object::name("OpenArrow"), Object::name("OpenArrow")]))],
    );
    let content = line_drawing(&d);
    let segs = segments(&content);
    assert!(has(&segs, [72.0, 490.0, 272.0, 490.0]), "the line sits |LL| + LLO above /L: {segs:?}");
    assert!(has(&segs, [72.0, 470.0, 72.0, 490.0]) && has(&segs, [272.0, 470.0, 272.0, 490.0]), "leaders start LLO from /L: {segs:?}");
    // The open arrows' tips (`tip l`) are at the ends of the line, not of /L.
    assert!(content.contains(" 72 490 l ") && content.contains(" 272 490 l "), "arrows at the moved line: {content}");
    assert!(!content.contains(" 72 460 l "), "no arrow on /L: {content}");
}

#[test]
fn an_inline_caption_breaks_the_line_around_its_text() {
    let d =
        table_178_line([72.0, 380.0, 272.0, 380.0], &[("Cap", Object::Bool(true)), ("Contents", Object::String(PdfString::text("Inline 50 mm")))]);
    let content = line_drawing(&d);
    assert!(content.contains("(Inline 50 mm) Tj"), "the caption is drawn: {content}");
    assert!(content.contains("/Helv 10 Tf"), "in Helvetica 10: {content}");
    let m = caption_matrix(&content).expect("text matrix");
    let width = crate::appearance::text_width("Inline 50 mm", 10.0);
    assert!((m[4] - (172.0 - width / 2.0)).abs() < 0.01 && m[5] < 380.0 && m[5] > 380.0 - 10.0, "centred on the line: {m:?}");
    let segs = segments(&content);
    assert_eq!(segs.len(), 2, "two pieces: {segs:?}");
    assert!(segs.iter().all(|s| s[2] <= 172.0 - width / 2.0 || s[0] >= 172.0 + width / 2.0), "no stroke under the text: {segs:?}");
    assert!(has(&segs, [72.0, 380.0, 172.0 - width / 2.0 - 2.0, 380.0]), "the left piece starts at the line's start: {segs:?}");
}

#[test]
fn a_top_caption_sits_above_the_unbroken_line() {
    let d = table_178_line(
        [72.0, 300.0, 272.0, 300.0],
        &[("Cap", Object::Bool(true)), ("CP", Object::name("Top")), ("Contents", Object::String(PdfString::text("Top 50 mm")))],
    );
    let content = line_drawing(&d);
    let m = caption_matrix(&content).expect("text matrix");
    assert!(m[5] > 300.0 && m[5] < 305.0, "baseline just above the line: {m:?}");
    assert_eq!(segments(&content), vec![[72.0, 300.0, 272.0, 300.0]], "the line is whole");
}

#[test]
fn a_caption_offset_moves_it_along_and_off_the_line_with_a_tick() {
    let co = |h: f64, v: f64| Object::Array(vec![Object::Real(h), Object::Real(v)]);
    let text = ("Contents", Object::String(PdfString::text("Offset 15")));
    let base = caption_matrix(&line_drawing(&table_178_line([330.0, 700.0, 530.0, 700.0], &[("Cap", Object::Bool(true)), text.clone()]))).unwrap();
    let content = line_drawing(&table_178_line([330.0, 700.0, 530.0, 700.0], &[("Cap", Object::Bool(true)), ("CO", co(0.0, 15.0)), text.clone()]));
    let up = caption_matrix(&content).unwrap();
    assert!((up[5] - base[5] - 15.0).abs() < 0.01 && (up[4] - base[4]).abs() < 0.01, "15 up: {base:?} → {up:?}");
    let segs = segments(&content);
    assert!(has(&segs, [330.0, 700.0, 530.0, 700.0]), "a caption off the line leaves it whole: {segs:?}");
    assert!(segs.iter().any(|s| s[0] == 430.0 && s[2] == 430.0 && s[1] == 700.0 && s[3] > 705.0), "a tick from the midpoint up: {segs:?}");
    let along =
        caption_matrix(&line_drawing(&table_178_line([330.0, 700.0, 530.0, 700.0], &[("Cap", Object::Bool(true)), ("CO", co(-30.0, 0.0)), text])))
            .unwrap();
    assert!((along[4] - base[4] + 30.0).abs() < 0.01 && (along[5] - base[5]).abs() < 0.01, "30 left: {base:?} → {along:?}");
}

#[test]
fn without_cap_true_no_caption_is_drawn() {
    for cap in [None, Some(Object::Bool(false)), Some(Object::Int(1))] {
        let mut extra = vec![("Contents", Object::String(PdfString::text("Hidden caption")))];
        if let Some(c) = cap.clone() {
            extra.push(("Cap", c));
        }
        let d = table_178_line([330.0, 620.0, 530.0, 620.0], &extra);
        let content = line_drawing(&d);
        assert!(!content.contains("Tj"), "{cap:?}: {content}");
        assert_eq!(segments(&content), vec![[330.0, 620.0, 530.0, 620.0]]);
        assert!(crate::appearance::undrawable_text(&d).is_none());
    }
}

#[test]
fn a_diagonal_lines_leaders_are_perpendicular_and_its_caption_turns_with_it() {
    let d = table_178_line(
        [330.0, 460.0, 530.0, 560.0],
        &[
            ("LL", Object::Real(20.0)),
            ("LLE", Object::Real(10.0)),
            ("Cap", Object::Bool(true)),
            ("Contents", Object::String(PdfString::text("Diagonal"))),
        ],
    );
    let content = line_drawing(&d);
    let segs = segments(&content);
    let (ux, uy) = (200.0 / 100f64.hypot(200.0), 100.0 / 100f64.hypot(200.0));
    let leaders: Vec<&[f64; 4]> = segs.iter().filter(|s| (s[2] - s[0]) * ux + (s[3] - s[1]) * uy < 1e-2).collect();
    assert_eq!(leaders.len(), 2, "two leader lines across the line: {segs:?}");
    for l in leaders {
        let (dx, dy) = (l[2] - l[0], l[3] - l[1]);
        assert!((dx.hypot(dy) - 30.0).abs() < 0.01, "|LL| + LLE long: {l:?}");
        assert!(dx < 0.0 && dy > 0.0, "up and to the left (left of travel): {l:?}");
    }
    let m = caption_matrix(&content).expect("caption");
    assert!((m[0] - ux).abs() < 1e-3 && (m[1] - uy).abs() < 1e-3, "the text runs along the line: {m:?}");
    // Drawn right to left, it still reads left to right.
    let back = caption_matrix(&line_drawing(&table_178_line(
        [530.0, 560.0, 330.0, 460.0],
        &[("Cap", Object::Bool(true)), ("Contents", Object::String(PdfString::text("x")))],
    )))
    .unwrap();
    assert!(back[0] > 0.0, "{back:?}");
}

#[test]
fn a_caption_from_rich_text_and_one_the_font_cant_draw() {
    let rc = "<?xml version=\"1.0\"?><body xmlns=\"http://www.w3.org/1999/xhtml\"><p>Dim &amp; <b>5</b>0&#x20;mm &lt;x&gt;</p></body>";
    let d = table_178_line([0.0, 0.0, 100.0, 0.0], &[("Cap", Object::Bool(true)), ("RC", Object::String(PdfString::text(rc)))]);
    let s = crate::appearance::build(&bare_doc(), &d).unwrap().decoded().unwrap();
    assert!(s.windows(17).any(|w| w == b"(Dim & 50 mm <x>)"), "{}", String::from_utf8_lossy(&s));
    let cjk = table_178_line([0.0, 0.0, 100.0, 0.0], &[("Cap", Object::Bool(true)), ("Contents", Object::String(PdfString::text("長さ")))]);
    assert_eq!(crate::appearance::undrawable_text(&cjk), Some(("the line's caption", '長')));
    // A PdfCraft measurement's label is its caption: the comment text isn't drawn a second time.
    let mut measured = cjk.clone();
    measured.set(b"PCMeasureValue".to_vec(), Object::String(PdfString::text("5 mm")));
    assert_eq!(crate::appearance::undrawable_text(&measured), None);
    let content = line_drawing(&measured);
    assert_eq!(content.matches(" Tj").count(), 1, "{content}");
}

#[test]
fn degenerate_and_absurd_leader_and_caption_values_do_not_panic() {
    let cap = ("Cap", Object::Bool(true));
    let text = ("Contents", Object::String(PdfString::text(&"w".repeat(10_000))));
    // A zero-length line has no direction: no leader lines, no caption, no NaN.
    let point =
        line_drawing(&table_178_line([50.0, 50.0, 50.0, 50.0], &[("LL", Object::Real(20.0)), ("LLE", Object::Real(5.0)), cap.clone(), text.clone()]));
    assert!(!point.contains("NaN") && !point.contains("inf") && !point.contains("Tj"), "{point}");
    assert_eq!(segments(&point).len(), 1);
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300, -1e300, f64::MAX] {
        let d = table_178_line(
            [0.0, 0.0, 100.0, 0.0],
            &[
                ("LL", Object::Real(bad)),
                ("LLE", Object::Real(bad)),
                ("LLO", Object::Real(bad)),
                ("CO", Object::Array(vec![Object::Real(bad), Object::Real(bad)])),
                cap.clone(),
                text.clone(),
            ],
        );
        let content = line_drawing(&d);
        assert!(!content.contains("NaN") && !content.contains("inf"), "{bad}: {content}");
        for s in segments(&content) {
            assert!(s.iter().all(|x| x.abs() <= 50_000.0), "{bad}: {s:?}");
        }
    }
    // Negative LLE and LLO count as 0.
    let neg = segments(&line_drawing(&table_178_line(
        [0.0, 0.0, 100.0, 0.0],
        &[("LL", Object::Real(10.0)), ("LLE", Object::Real(-5.0)), ("LLO", Object::Real(-5.0))],
    )));
    assert!(has(&neg, [0.0, 0.0, 0.0, 10.0]) && has(&neg, [0.0, 10.0, 100.0, 10.0]), "{neg:?}");
    // Huge endpoints whose midpoint would overflow fall back to the plain line.
    let huge = line_drawing(&table_178_line([f64::MAX, f64::MAX, f64::MAX / 2.0, f64::MAX], &[("LL", Object::Real(20.0)), cap, text]));
    assert!(!huge.contains("NaN") && !huge.contains("inf"), "{huge}");
    // The caption is cut to 256 characters.
    let long = line_drawing(&table_178_line(
        [0.0, 0.0, 100.0, 0.0],
        &[("Cap", Object::Bool(true)), ("Contents", Object::String(PdfString::text(&"w".repeat(10_000))))],
    ));
    assert!(long.contains(&format!("({}) Tj", "w".repeat(256))), "cut to 256");
    assert!(!long.contains(&"w".repeat(257)));
}

/// A comment's built appearance for the dictionary `body` (no `/AP`), as text, or `None` when
/// PdfCraft can't draw it.
fn built(body: &str) -> Option<String> {
    let doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 600] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        body.as_bytes(),
    ]);
    let d = list(&doc, 0).into_iter().next().unwrap();
    crate::appearance::build(&doc, &d).map(|s| String::from_utf8_lossy(&s.decoded().unwrap()).into_owned())
}

/// The x and y range a drawing's path covers: its points, and each curve's middle (a curve's
/// control points lie outside it, so they aren't counted).
fn extent(ops: &str) -> [f64; 4] {
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    let mut at = (0.0, 0.0);
    let add = |p: (f64, f64), b: &mut [f64; 4]| *b = [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)];
    for line in ops.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        let Some((op, nums)) = t.split_last() else { continue };
        let v: Vec<f64> = nums.iter().filter_map(|n| n.parse().ok()).collect();
        match (*op, v.as_slice()) {
            ("m" | "l", [x, y]) => {
                at = (*x, *y);
                add(at, &mut b);
            }
            ("c", [x1, y1, x2, y2, x3, y3]) => {
                let mid = ((at.0 + 3.0 * x1 + 3.0 * x2 + x3) / 8.0, (at.1 + 3.0 * y1 + 3.0 * y2 + y3) / 8.0);
                add(mid, &mut b);
                at = (*x3, *y3);
                add(at, &mut b);
            }
            _ => {}
        }
    }
    b
}

/// ISO 32000-2 §12.5.4, Table 169: a cloudy border (`/BE /S /C`) applies to squares, circles,
/// polygons and (PDF 1.6) text boxes, honours the `/BS` width and dashes, and its intensity
/// `/I` defaults to 0. Squares, circles and text boxes without an appearance were not drawn at
/// all, so a reviewer's marks vanished; intensity 0 (or none) drew a cloud on polygons.
#[test]
fn cloudy_borders_are_drawn_on_squares_circles_polygons_and_text_boxes() {
    let base = "/Type /Annot /Rect [100 100 300 250] /C [1 0 0] /BS << /W 2 >>";
    for kind in ["Square", "Circle"] {
        let ops = built(&format!("<< {base} /Subtype /{kind} /BE << /S /C /I 1 >> >>")).unwrap_or_else(|| panic!("{kind} is drawn"));
        assert!(ops.matches(" c\n").count() > 8 && !ops.contains(" re\n"), "{kind} as bumps: {ops}");
        let e = extent(&ops);
        assert!(e[0] >= 100.0 && e[1] >= 100.0 && e[2] <= 300.0 && e[3] <= 250.0, "{kind} stays in /Rect: {e:?}");
    }
    let dashed = built(&format!("<< {base} /Subtype /Square /BS << /W 2 /S /D /D [6 3] >> /BE << /S /C /I 1 >> >>")).unwrap();
    assert!(dashed.contains("[6 3] 0 d") && dashed.contains(" c\n"), "dashes honoured: {dashed}");
    let text = built(&format!("<< {base} /Subtype /FreeText /DA (0 0 1 rg /Helv 10 Tf) /Contents (Look here) /BE << /S /C /I 1 >> >>")).unwrap();
    assert!(text.contains(" c\n") && text.contains("(Look here) Tj"), "a cloud around the text: {text}");
    let dashed_text = built(
        "<< /Type /Annot /Rect [100 100 300 250] /C [1 0 0] /BS << /W 2 /S /D /D [4 2] >> /Subtype /FreeText /DA (0 g /Helv 10 Tf) /Contents (x) /BE << /S /C /I 1 >> >>",
    )
    .unwrap();
    assert!(dashed_text.contains("[4 2] 0 d"), "a text box's cloud is dashed too: {dashed_text}");
    // Intensity 0, or none: a plain border (the default is 0).
    for be in ["<< /S /C /I 0 >>", "<< /S /C >>", "<< /S /S /I 2 >>"] {
        let square = built(&format!("<< {base} /Subtype /Square /BE {be} >>")).unwrap();
        assert!(square.contains(" re\n") && !square.contains(" c\n"), "{be}: {square}");
        let polygon =
            built(&format!("<< /Type /Annot /Subtype /Polygon /Rect [0 0 100 100] /C [1 0 0] /Vertices [10 10 90 10 90 90 10 90] /BE {be} >>"))
                .unwrap();
        assert!(!polygon.contains(" c\n"), "{be}: polygon edges stay straight: {polygon}");
    }
    let cloudy_polygon =
        built("<< /Type /Annot /Subtype /Polygon /Rect [0 0 100 100] /C [1 0 0] /Vertices [10 10 90 10 90 90 10 90] /BE << /S /C /I 1 >> >>")
            .unwrap();
    assert!(cloudy_polygon.contains(" c\n"));
    // A cloudy callout is still left to its own appearance.
    assert!(
        built(&format!("<< {base} /Subtype /FreeText /DA (0 g /Helv 10 Tf) /CL [10 10 100 100] /RD [5 5 5 5] /BE << /S /C /I 1 >> >>")).is_none()
    );
}

/// Table 180: `/RD` gives the differences between "the left, top, right, and bottom coordinates
/// of Rect and those of the square or circle, respectively"; a cloudy square's bumps sit on that
/// inner square.
#[test]
fn a_cloudy_square_follows_rd_left_top_right_bottom() {
    let ops = built("<< /Type /Annot /Subtype /Square /Rect [100 100 300 300] /C [1 0 0] /BS << /W 1 >> /BE << /S /C /I 1 >> /RD [20 40 30 25] >>")
        .unwrap();
    // The corners of the inner square are path points: left 120, bottom 125, right 270, top 260.
    for corner in ["120 125", "270 125", "270 260", "120 260"] {
        assert!(ops.contains(corner), "corner {corner} in {ops}");
    }
}

/// Review on #4195: without `/RD`, a small cloudy square still keeps its bumps inside `/Rect`.
/// Its edges are short enough that each rounds to a single bump about three radii long, which
/// bulges further than one radius. An `/RD` leaving less than that margin is widened to it, so
/// the cloud stays inside `/Rect` too.
#[test]
fn a_small_cloudy_square_without_rd_stays_inside_its_rect() {
    for side in [40.0, 48.8, 60.0, 82.0] {
        let ops = built(&format!(
            "<< /Type /Annot /Subtype /Square /Rect [100 100 {x} {x}] /C [1 0 0] /BS << /W 2 >> /BE << /S /C /I 1 >> >>",
            x = 100.0 + side
        ))
        .unwrap_or_else(|| panic!("side {side} is drawn"));
        let e = extent(&ops);
        assert!(e[0] >= 99.0 && e[1] >= 99.0 && e[2] <= 101.0 + side && e[3] <= 101.0 + side, "side {side}: {e:?}");
    }
    let small_rd =
        built("<< /Type /Annot /Subtype /Square /Rect [100 100 200 200] /C [1 0 0] /BS << /W 2 >> /BE << /S /C /I 1 >> /RD [1 1 1 1] >>").unwrap();
    let e = extent(&small_rd);
    assert!(e[0] >= 99.0 && e[1] >= 99.0 && e[2] <= 201.0 && e[3] <= 201.0, "/RD [1 1 1 1] keeps the cloud inside /Rect: {e:?}");
}

/// ISO 32000-2 Table 180: `/RD` gives the differences between `/Rect` and "the actual boundaries
/// of the underlying square or circle", so a plain square or circle (no cloudy border) is drawn
/// inside `/Rect` less `/RD` (left, top, right, bottom) too, its stroke centred half the width
/// inside that box. An `/RD` that isn't four finite non-negative numbers, or leaves no box, is
/// ignored and the shape fills `/Rect` as before.
#[test]
fn plain_squares_and_circles_are_drawn_inside_rect_less_rd() {
    let square = |rd: &str| built(&format!("<< /Type /Annot /Subtype /Square /Rect [100 100 200 200] /C [1 0 0] /BS << /W 3 >> {rd} >>")).unwrap();
    // Rect + 15 + 1.5 on every side: a 67 × 67 square from 116.5.
    assert!(square("/RD [15 15 15 15]").contains("116.5 116.5 67 67 re"), "{}", square("/RD [15 15 15 15]"));
    // Left 10, top 40, right 20, bottom 30: the box is 110..180 × 130..160.
    assert!(square("/RD [10 40 20 30]").contains("111.5 131.5 67 27 re"), "{}", square("/RD [10 40 20 30]"));
    for ignored in ["", "/RD [-1 0 0 0]", "/RD [1 2 3]", "/RD [60 0 60 0]", "/RD [0 50 0 49.5]", "/RD (x)"] {
        assert!(square(ignored).contains("101.5 101.5 97 97 re"), "{ignored}: {}", square(ignored));
    }
    let circle = built("<< /Type /Annot /Subtype /Circle /Rect [100 100 200 200] /C [1 0 0] /BS << /W 3 >> /RD [15 15 15 15] >>").unwrap();
    // The ellipse starts at its rightmost point: 200 - 15 - 1.5, at mid-height.
    assert!(circle.starts_with("1 0 0 RG\n3 w\n183.5 150 m"), "{circle}");
}

/// The circles and rectangles PdfCraft draws write no `/RD`, so their appearance is unchanged by
/// it: the stroke still sits half its width inside `/Rect`.
#[test]
fn pdfcraft_rectangles_and_ellipses_still_fill_their_rect() {
    let mut doc = fixture();
    let rect = [10.0, 10.0, 110.0, 60.0];
    for shape in [Shape::Rectangle { rect }, Shape::Oval { rect }] {
        let oval = matches!(shape, Shape::Oval { .. });
        let i = add_annotation(&mut doc, &new(0, shape), &meta("r")).unwrap();
        let d = list(&doc, 0).remove(i);
        assert!(!d.contains(b"RD"));
        let w = appearance::border_width(&d);
        let ap = ap_content(&doc, &d);
        let r: Vec<f64> = d.get(b"Rect").and_then(Object::as_array).unwrap().iter().filter_map(Object::as_f64).collect();
        let expected = if oval {
            format!("{} {} m", n(r[2] - w / 2.0), n((r[1] + r[3]) / 2.0))
        } else {
            format!("{} {} {} {} re", n(r[0] + w / 2.0), n(r[1] + w / 2.0), n(r[2] - r[0] - w), n(r[3] - r[1] - w))
        };
        assert!(ap.contains(&expected), "{expected} in {ap}");
    }
}

/// Review on #4195: a polygon reshaped with `/BE /S /C` and no `/I` (or `/I 0`) is a plain one,
/// as its appearance is drawn (Table 169: intensity 0 is no cloud), so its `/Rect` gets no room
/// for bumps; with an intensity above 0 it does.
#[test]
fn a_reshaped_polygon_pads_its_rect_only_for_a_drawn_cloud() {
    // `/BE << /S /C >>` with `/I` when given; no `/BE` at all for `None`.
    let reshaped = |be: Option<Option<i64>>| {
        let mut doc = fixture();
        let shape = Shape::Polygon { vertices: vec![[100.0, 100.0], [200.0, 100.0], [150.0, 200.0]], cloud: false };
        let i = add_annotation(&mut doc, &new(0, shape), &meta("p")).unwrap();
        let (_, r) = annot_ref(&mut doc, 0, i).unwrap();
        if let Some(intensity) = be {
            let mut b = pdfcraft_cos::Dict::new();
            b.set(b"S".to_vec(), Object::name("C"));
            if let Some(i) = intensity {
                b.set(b"I".to_vec(), Object::Int(i));
            }
            doc.update_dict(r, |d| d.set(b"BE".to_vec(), Object::Dict(b))).unwrap();
        }
        set_points(&mut doc, 0, i, &[[100.0, 100.0], [300.0, 100.0], [200.0, 250.0]], &meta("")).unwrap();
        rect(&list(&doc, 0)[i])
    };
    let plain = reshaped(None);
    for be in [None, Some(0)] {
        assert_eq!(reshaped(Some(be)), plain, "/I {be:?}: no cloud drawn, no room for one");
    }
    let cloudy = reshaped(Some(Some(1)));
    assert!(cloudy[0] < plain[0] && cloudy[2] > plain[2], "room for the bumps: {cloudy:?} around {plain:?}");
}

/// Review on #4195: a reshaped polygon whose cloud has intensity 2 gets room for its bumps at
/// that intensity. With room only for intensity 1, the bumps on a 200 pt edge reached past
/// `/Rect` and the appearance's `/BBox` clipped them.
#[test]
fn a_reshaped_intensity_two_cloud_stays_inside_its_rect() {
    let mut doc = fixture();
    let shape = Shape::Polygon { vertices: vec![[100.0, 100.0], [200.0, 100.0], [150.0, 200.0]], cloud: true };
    let i = add_annotation(&mut doc, &new(0, shape), &meta("p")).unwrap();
    let (_, r) = annot_ref(&mut doc, 0, i).unwrap();
    let mut be = pdfcraft_cos::Dict::new();
    be.set(b"S".to_vec(), Object::name("C"));
    be.set(b"I".to_vec(), Object::Int(2));
    doc.update_dict(r, |d| d.set(b"BE".to_vec(), Object::Dict(be))).unwrap();
    set_points(&mut doc, 0, i, &[[100.0, 100.0], [300.0, 100.0], [200.0, 250.0]], &meta("")).unwrap();
    let d = &list(&doc, 0)[i];
    let bbox = rect(d);
    // Every point along the drawn bumps, with half the 2 pt line, is inside `/Rect`.
    let content = ap_content(&doc, d);
    let (mut at, mut lowest, mut leftmost) = ((0.0, 0.0), f64::MAX, f64::MAX);
    for line in content.lines() {
        let v: Vec<f64> = line.split_whitespace().filter_map(|t| t.parse().ok()).collect();
        if line.ends_with(" m") && v.len() == 2 {
            at = (v[0], v[1]);
        } else if line.ends_with(" c") && v.len() == 6 {
            for step in 0..=20 {
                let t = f64::from(step) / 20.0;
                let u = 1.0 - t;
                let (a, b, c, e) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                lowest = lowest.min(a * at.1 + b * v[1] + c * v[3] + e * v[5]);
                leftmost = leftmost.min(a * at.0 + b * v[0] + c * v[2] + e * v[4]);
            }
            at = (v[4], v[5]);
        }
    }
    assert!(lowest < 100.0 - 8.0, "the bumps bulge below the bottom edge: {lowest}");
    assert!(lowest - 1.0 >= bbox[1], "the bottom bumps ({lowest}) and the line fit in /Rect {bbox:?}");
    assert!(leftmost - 1.0 >= bbox[0], "the left bumps ({leftmost}) and the line fit in /Rect {bbox:?}");
}

/// Review on #4195: a reshaped polygon gets room for the border it is drawn with. With no `/BS`,
/// a `/Border [0 0 5]` is drawn 5 pt wide (`appearance::border_width`), so the new `/Rect` is the
/// one `/BS /W 5` gets; it was sized for a 1-pt border, and the cloud's outer bumps were clipped.
#[test]
fn a_reshaped_polygon_has_room_for_the_border_it_is_drawn_with() {
    let reshaped = |border: &[u8], width: Object| {
        let mut doc = fixture();
        let shape = Shape::Polygon { vertices: vec![[100.0, 100.0], [200.0, 100.0], [150.0, 200.0]], cloud: true };
        let i = add_annotation(&mut doc, &new(0, shape), &meta("p")).unwrap();
        let (_, r) = annot_ref(&mut doc, 0, i).unwrap();
        doc.update_dict(r, |d| {
            d.remove(b"BS");
            if border == b"BS" {
                let mut bs = pdfcraft_cos::Dict::new();
                bs.set(b"W".to_vec(), width);
                d.set(b"BS".to_vec(), Object::Dict(bs));
            } else {
                d.set(b"Border".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), width]));
            }
        })
        .unwrap();
        set_points(&mut doc, 0, i, &[[100.0, 100.0], [300.0, 100.0], [200.0, 250.0]], &meta("")).unwrap();
        rect(&list(&doc, 0)[i])
    };
    let bs = reshaped(b"BS", Object::Int(5));
    assert_eq!(reshaped(b"Border", Object::Int(5)), bs, "the same 5-pt border, the same room");
    assert_ne!(reshaped(b"BS", Object::Int(1)), bs, "a 1-pt border needs less");
}

/// Every number a drawing writes, which must all be finite.
fn drawn_numbers(ops: &str) -> Vec<f64> {
    ops.split_whitespace().filter(|t| t.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.')).filter_map(|t| t.parse().ok()).collect()
}

/// Review on #4195: a cloud around a zero-size, inverted, sliver or huge `/Rect`, at any border
/// width and any `/BE /I` (tiny, negative, far above Table 169's 2), is either drawn inside
/// `/Rect` with finite numbers and a bounded number of bumps, or not built at all, never a
/// panic, a division by zero or a NaN.
#[test]
fn hostile_cloud_sizes_widths_and_intensities_draw_inside_rect_or_not_at_all() {
    let rects = [
        "0 0 0 0",
        "100 100 100 100",
        "100 100 100.5 300",
        "100 100 101 101",
        "300 250 100 100",
        "100 100 300 250",
        "100 100 110 110",
        "-1000000000000000 -1000000000000000 1000000000000000 1000000000000000",
        "0 0 99999999999999999999999 1",
        &format!("0 0 {} 100", "9".repeat(400)),
        &format!("-{n} -{n} {n} {n}", n = "9".repeat(308)),
    ];
    let (mut drawn, mut refused) = (0, 0);
    for rect in rects {
        let r: Vec<f64> = rect.split_whitespace().filter_map(|t| t.parse().ok()).collect();
        let bounds = [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])];
        for width in ["0", "0.001", "2", "1000000", "-3"] {
            for intensity in ["0.0000001", "2", "1000000000000000000000000000000", "-1"] {
                for kind in ["Square", "Circle", "FreeText"] {
                    let body = format!(
                        "<< /Type /Annot /Subtype /{kind} /Rect [{rect}] /C [1 0 0] /BS << /W {width} >> /BE << /S /C /I {intensity} >> /DA (0 g /Helv 10 Tf) /Contents (x) >>"
                    );
                    let Some(ops) = built(&body) else {
                        refused += 1;
                        continue;
                    };
                    drawn += 1;
                    assert!(!ops.contains("NaN") && !ops.contains("inf"), "{body}: {ops}");
                    assert!(drawn_numbers(&ops).iter().all(|v| v.is_finite()), "{body}");
                    assert!(ops.matches(" c\n").count() <= crate::appearance::MAX_CLOUD_BUMPS + 8, "{body}: bumps are capped");
                    // A cloud (intensity above 0); a negative one is a plain border (Table 169).
                    if !intensity.starts_with('-') && ops.contains(" c\n") {
                        let e = extent(&ops);
                        let slack = 0.5 + 1e-9 * (bounds[2] - bounds[0]).abs().max((bounds[3] - bounds[1]).abs());
                        assert!(
                            e[0] >= bounds[0] - slack && e[1] >= bounds[1] - slack && e[2] <= bounds[2] + slack && e[3] <= bounds[3] + slack,
                            "{body}: the cloud {e:?} stays in /Rect"
                        );
                    }
                }
            }
        }
    }
    assert!(drawn > 0 && refused > 0, "both outcomes are exercised: {drawn} drawn, {refused} refused");
}

/// Review on #4195: a cloud around a rotated rectangle (a polygon turned 45°, its corners in
/// either order) bulges outwards the same way and stays inside its `/Rect`, and `cloud_path`
/// itself takes a zero, negative, infinite or NaN bump size and repeated corners without
/// dividing by zero.
#[test]
fn a_cloud_around_a_rotated_rectangle_bulges_outwards_either_way_round() {
    let ccw = "150 100 200 150 150 200 100 150";
    let cw = "150 100 100 150 150 200 200 150";
    let extents: Vec<[f64; 4]> = [ccw, cw]
        .iter()
        .map(|v| {
            let ops = built(&format!("<< /Type /Annot /Subtype /Polygon /Rect [80 80 220 220] /C [1 0 0] /Vertices [{v}] /BE << /S /C /I 1 >> >>"))
                .unwrap();
            assert!(ops.contains(" c\n"), "{ops}");
            let e = extent(&ops);
            assert!(e[0] >= 80.0 && e[1] >= 80.0 && e[2] <= 220.0 && e[3] <= 220.0, "inside /Rect: {e:?}");
            // Each bump's middle lies outside the turned square |x − 150| + |y − 150| = 50.
            let mut at = (0.0, 0.0);
            for line in ops.lines() {
                let v: Vec<f64> = line.split_whitespace().filter_map(|t| t.parse().ok()).collect();
                if line.ends_with(" m") && v.len() == 2 {
                    at = (v[0], v[1]);
                } else if line.ends_with(" c") && v.len() == 6 {
                    let mid = ((at.0 + 3.0 * v[0] + 3.0 * v[2] + v[4]) / 8.0, (at.1 + 3.0 * v[1] + 3.0 * v[3] + v[5]) / 8.0);
                    assert!((mid.0 - 150.0).abs() + (mid.1 - 150.0).abs() > 50.0 + 1.0, "the bump at {mid:?} bulges outwards");
                    at = (v[4], v[5]);
                }
            }
            e
        })
        .collect();
    for k in 0..4 {
        assert!((extents[0][k] - extents[1][k]).abs() < 0.5, "the same cloud either way round: {extents:?}");
    }
    let square = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
    for r in [0.0, -1.0, f64::INFINITY, f64::NAN, 1e-300, 1e300] {
        let path = crate::appearance::cloud_path(&square, r);
        assert!(!path.contains("NaN") && !path.contains("inf"), "r {r}: {path}");
        assert!(path.ends_with("h\n"));
    }
    for pts in [vec![], vec![(5.0, 5.0)], vec![(5.0, 5.0); 4], vec![(0.0, 0.0), (10.0, 0.0), (10.0, 0.0), (0.0, 0.0)]] {
        let path = crate::appearance::cloud_path(&pts, 2.0);
        assert!(!path.contains("NaN") && !path.contains("inf"), "{pts:?}: {path}");
    }
}

/// PdfCraft's own text boxes, typewriter text and redaction marks write `/BS` (width 0 for text
/// boxes and typewriter text), `/DA /Helv` and a `/DS` that agrees with it, and quadrilaterals:
/// the default border, `/DA` fonts, `/DS` and the `/Rect` outline leave them exactly as they were.
#[test]
fn pdfcraft_text_boxes_and_redaction_marks_look_as_before() {
    let mut doc = fixture();
    let mut tb = new(0, Shape::TextBox { rect: [100.0, 100.0, 220.0, 160.0], font_size: 12.5 });
    tb.style = Style { color: [0.1, 0.2, 0.3], fill: Some([1.0, 1.0, 0.8]), ..Style::default_for(&tb.shape) };
    tb.contents = "PdfCraft text box that wraps onto a second line".into();
    let t = add_annotation(&mut doc, &tb, &meta("t")).unwrap();
    let mut tw = new(0, Shape::Typewriter { rect: [100.0, 300.0, 300.0, 320.0], font_size: 10.0 });
    tw.style = Style::default_for(&tw.shape);
    let w = add_annotation(&mut doc, &tw, &meta("w")).unwrap();
    let quads = vec![[100.0, 520.0, 200.0, 520.0, 100.0, 500.0, 200.0, 500.0]];
    let mut rd = new(0, Shape::Redact { quads, overlay: String::new(), look: OverlayLook::default() });
    rd.style = Style::default_for(&rd.shape);
    let r = add_annotation(&mut doc, &rd, &meta("r")).unwrap();
    let all = list(&doc, 0);
    assert_eq!(
        ap_content(&doc, &all[t]),
        "1 1 0.8 rg\n100 100 120 60 re f\n100 100 120 60 re W n\nBT\n/Helv 12.5 Tf\n0.1 0.2 0.3 rg\n1 0 0 1 102 146.75 Tm (PdfCraft text box) Tj\n1 0 0 1 102 131.75 Tm (that wraps onto a) Tj\n1 0 0 1 102 116.75 Tm (second line) Tj\nET\n"
    );
    assert_eq!(text(&all[t], b"DS"), "font: Helvetica 12.5pt; text-align:left; color:#1A334D");
    assert_eq!(ap_content(&doc, &all[w]), "100 300 200 20 re W n\nBT\n/Helv 10 Tf\n0 0 0 rg\n1 0 0 1 102 309 Tm (hello) Tj\nET\n");
    assert_eq!(ap_content(&doc, &all[r]), "0.89 0.13 0.13 RG\n1 w\n100 520 m 200 520 l 200 500 l 100 500 l h S\n");
    // New text keeps the box's look: Helvetica, /DA's exact colour (not /DS's 8-bit one).
    set_contents(&mut doc, 0, t, "Edited", &meta("")).unwrap();
    let d = &list(&doc, 0)[t];
    assert_eq!(text(d, b"DS"), "font: Helvetica 12.5pt; text-align:left; color:#1A334D");
    let ap = ap_content(&doc, d);
    assert!(ap.contains("/Helv 12.5 Tf\n0.1 0.2 0.3 rg\n") && !ap.contains(" re S"), "{ap}");
}

/// The display appearance of the annotation dictionary `annot` on a page of a document whose
/// form `/DR` maps `Helv`, `Cour`, `TiRo`, `F1` (Courier) and `F2` (a font that isn't standard),
/// and the font resources it draws with.
fn display_of(annot: &str) -> (String, Dict) {
    let doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /DR << /Font << /Helv 5 0 R /Cour 6 0 R /TiRo 7 0 R /F1 6 0 R /F2 9 0 R >> >> >> >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [8 0 R] >>",
        b"<< /Length 0 >>\nstream\n\nendstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman /Encoding /WinAnsiEncoding >>",
        annot.as_bytes(),
        b"<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+HouseGothic >>",
    ]);
    let shown = with_missing_appearances(&doc).expect("drawn for display");
    let d = list(&shown, 0).remove(0);
    let n = d.get(b"AP").and_then(|a| a.as_dict()).and_then(|a| a.reference(b"N")).expect("/AP /N");
    let Object::Stream(s) = &*shown.get(n) else { panic!("not a stream") };
    let fonts = s.dict.get(b"Resources").and_then(|r| r.as_dict()).and_then(|r| r.get(b"Font")).and_then(|f| f.as_dict()).cloned();
    (String::from_utf8_lossy(&s.decoded().unwrap()).into_owned(), fonts.unwrap_or_default())
}

fn base_fonts(fonts: &Dict) -> Vec<String> {
    fonts.iter().filter_map(|(_, f)| f.as_dict()?.name(b"BaseFont").map(|b| String::from_utf8_lossy(b).into_owned())).collect()
}

/// A text box without `/BS` or `/Border` has the `/Border` default [0 0 1] (ISO 32000-2
/// Table 166): a 1-pt border in the text colour. `/BS /W 0` or `/Border [0 0 0]` turn it off.
#[test]
fn free_text_without_a_border_entry_has_the_default_one() {
    let (ap, _) = display_of("<< /Type /Annot /Subtype /FreeText /Rect [45 630 165 700] /DA (0 0 1 rg /Helv 12 Tf) /Contents (Hi) >>");
    assert!(ap.contains("0 0 1 RG\n1 w\n45.5 630.5 119 69 re S\n"), "{ap}");
    assert!(ap.contains("1 0 0 1 48 "), "text inset by the border: {ap}");
    for off in ["/BS << /W 0 >>", "/Border [0 0 0]"] {
        let (ap, _) = display_of(&format!("<< /Type /Annot /Subtype /FreeText /Rect [45 630 165 700] /DA (0 g /Helv 12 Tf) {off} /Contents (Hi) >>"));
        assert!(!ap.contains(" re S"), "{off}: {ap}");
    }
}

/// Review on #4365: a text box's new text is fitted with the border it is drawn with. A 4-pt border
/// written as `/Border [0 0 4]` (no `/BS`) is drawn 4 pt wide, so the box grows as for `/BS /W 4`;
/// before, the fit read only `/BS` and measured it with a 1-pt border, so the drawn text, wrapped
/// 6 pt narrower, could run past the box.
#[test]
fn new_text_is_fitted_with_the_border_the_box_is_drawn_with() {
    let fitted = |border: &str| {
        let annot = format!("<< /Type /Annot /Subtype /FreeText /Rect [45 600 165 700] /DA (0 g /Helv 12 Tf) {border} /Contents (Hi) >>");
        let mut doc = document_of(&[
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
            b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
            annot.as_bytes(),
        ]);
        set_contents(&mut doc, 0, 0, "Several words that wrap over a few lines of this box", &meta("")).unwrap();
        list(&doc, 0).remove(0).get(b"Rect").and_then(|r| r.as_array()).map(|r| r.iter().filter_map(|x| x.as_f64()).collect::<Vec<f64>>()).unwrap()
    };
    let bs = fitted("/BS << /W 4 >>");
    assert_eq!(fitted("/Border [0 0 4]"), bs, "the same 4-pt border, the same box");
    assert_ne!(fitted("/BS << /W 1 >>"), bs, "a 1-pt border fits another box");
}

/// `/DA` names its font in the form's `/DR` (§12.7.4.3): a standard face is drawn in that face,
/// with its widths for wrapping; any other font stays Helvetica.
#[test]
fn free_text_da_fonts_draw_in_their_standard_face() {
    use pdfcraft_fonts::Std14;
    let width = 120.0 - 2.0 * 2.0;
    let words = "iiiiiiiiii iiiiiiiiii";
    let lines = |face: Std14| pdfcraft_fonts::wrap_with(words, 12.0, width, |s, z| face.text_width(s, z)).len();
    assert_eq!((lines(Std14::Helvetica), lines(Std14::Courier)), (1, 2), "the case tells the faces apart");
    for (da, base, key, n) in [
        ("0 g /Cour 12 Tf", "Courier", "/Cour 12 Tf", 2),
        ("1 0 0 rg /TiRo 12 Tf", "Times-Roman", "/TiRo 12 Tf", 1),
        ("0 g /F1 12 Tf", "Courier", "/Cour 12 Tf", 2),
        ("0 g /F2 12 Tf", "Helvetica", "/Helv 12 Tf", 1),
        ("0 g /Nowhere 12 Tf", "Helvetica", "/Helv 12 Tf", 1),
        ("0 g /CoBo 12 Tf", "Courier-Bold", "/CoBo 12 Tf", 2),
    ] {
        let annot = format!("<< /Type /Annot /Subtype /FreeText /Rect [45 630 165 700] /DA ({da}) /BS << /W 0 >> /Contents ({words}) >>");
        let (ap, fonts) = display_of(&annot);
        assert_eq!(base_fonts(&fonts), [base], "{da}");
        assert!(ap.contains(key), "{da}: {ap}");
        assert_eq!(ap.matches(" Tj").count(), n, "{da}: wrapped at {base}'s widths: {ap}");
    }
}

/// Thai in a text box whose `/DA` names Times is drawn from the Thai face and the rest of its
/// text in Times: after a Thai run the text goes back to `/TiRo`, never to `/Helv`, and both
/// kinds of font are in the appearance's resources.
#[test]
fn thai_in_a_times_text_box_keeps_times_for_the_rest() {
    // "สวัสดี Hi": Thai first, then Latin.
    let annot = "<< /Type /Annot /Subtype /FreeText /Rect [45 630 245 700] /DA (0 g /TiRo 12 Tf) /BS << /W 0 >> /Contents <FEFF0E2A0E270E310E2A0E140E35002000480069> >>";
    let (ap, fonts) = display_of(annot);
    assert!(!ap.contains("/Helv"), "{ap}");
    assert!(base_fonts(&fonts).contains(&"Times-Roman".to_string()), "{fonts:?}");
    if pdfcraft_fonts::document_thai_font().is_none() {
        return;
    }
    assert!(ap.contains("/PCTh1 ") && !ap.contains('?'), "the Thai is shaped: {ap}");
    assert_eq!(ap.matches("/TiRo 12 Tf").count(), 2, "Times is selected again after the Thai: {ap}");
    assert!(fonts.get(b"PCTh1").and_then(|f| f.as_dict()).is_some_and(|f| f.name(b"Subtype") == Some(&b"Type3"[..])), "{fonts:?}");
}

/// `/DS` (Table 177) sets the face, size and colour over `/DA`'s; a malformed one is ignored.
#[test]
fn free_text_ds_style_is_drawn() {
    let annot =
        "<< /Type /Annot /Subtype /FreeText /Rect [45 630 165 700] /DA (0 g /Helv 12 Tf) /DS (font: Times 14pt; color: #008000) /Contents (Hi) >>";
    let (ap, fonts) = display_of(annot);
    assert_eq!(base_fonts(&fonts), ["Times-Roman"]);
    assert!(ap.contains("/TiRo 14 Tf\n0 0.502 0 rg\n"), "{ap}");
    let junk = format!("font: NaNpt; font-size: inf; color: rgb(1e999, x); {}", "\u{FFFD}".repeat(20_000));
    let annot = format!("<< /Type /Annot /Subtype /FreeText /Rect [45 630 165 700] /DA (0 0 1 rg /Cour 9 Tf) /DS ({junk}) /Contents (Hi) >>");
    let (ap, fonts) = display_of(&annot);
    assert_eq!(base_fonts(&fonts), ["Courier"]);
    assert!(ap.contains("/Cour 9 Tf\n0 0 1 rg\n"), "{ap}");
}

/// New text in a box drawn in Courier stays Courier: its rich text says so.
#[test]
fn new_text_keeps_a_foreign_box_face() {
    let mut doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        b"<< /Type /Annot /Subtype /FreeText /Rect [45 630 165 700] /DA (0 g /Cour 12 Tf) /Contents (Hi) >>",
    ]);
    set_contents(&mut doc, 0, 0, "Hello", &meta("")).unwrap();
    let d = &list(&doc, 0)[0];
    assert_eq!(text(d, b"DS"), "font: Courier 12pt; text-align:left; color:#000000");
    assert!(ap_content(&doc, d).contains("/Cour 12 Tf"));
}

/// A text box whose `/DS` overrides `/DA` reports the colour and size it is drawn in, and a colour
/// or size change keeps the other drawn value.
#[test]
fn text_box_properties_report_the_drawn_style() {
    let doc = || {
        document_of(&[
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
            b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
            b"<< /Type /Annot /Subtype /FreeText /Rect [45 500 300 700] /Contents (Hi) /DA (1 0 0 rg /Helv 12 Tf) /DS (font: Helvetica 24pt; color:#0000FF) >>",
        ])
    };
    let mut d = doc();
    let p = props(&d, 0, 0).unwrap();
    assert_eq!((p.color, p.font_size), (Some([0.0, 0.0, 1.0]), Some(24.0)));
    set_style(&mut d, 0, 0, StyleChange::new(Some([0.0, 1.0, 0.0]), None, None, None), &meta("")).unwrap();
    let p = props(&d, 0, 0).unwrap();
    assert_eq!((p.color, p.font_size), (Some([0.0, 1.0, 0.0]), Some(24.0)), "a colour change keeps the drawn size");
    let mut d = doc();
    set_font_size(&mut d, 0, 0, 18.0, &meta("")).unwrap();
    let p = props(&d, 0, 0).unwrap();
    assert_eq!((p.color, p.font_size), (Some([0.0, 0.0, 1.0]), Some(18.0)), "a size change keeps the drawn colour");
}

/// A redaction mark without `/QuadPoints` marks `/Rect` (Table 195). While marked it is outlined
/// only when it names an outline colour (an array `/OC` of three numbers); without one it draws
/// nothing. Marks with quadrilaterals and no `/OC` colour are outlined in `/C` as before.
#[test]
fn redaction_marks_without_quadpoints_outline_their_rect_in_their_oc_colour() {
    let (ap, _) = display_of("<< /Type /Annot /Subtype /Redact /Rect [45 470 165 540] /IC [0 0 0] /OC [1 0 0] /C [0 0 1] >>");
    assert_eq!(ap, "1 0 0 RG\n1 w\n45.5 539.5 m 164.5 539.5 l 164.5 470.5 l 45.5 470.5 l h S\n");
    // No /OC colour: nothing to draw while marked (no /C outline, no /IC fill, no overlay text).
    let blank = |annot: &str| {
        let annot = format!("<< /Type /Annot /Subtype /Redact /Rect [45 470 165 540] {annot} >>");
        let doc = document_of(&[
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
            b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
            annot.as_bytes(),
        ]);
        with_missing_appearances(&doc).is_none()
    };
    // An /OC that isn't three finite numbers is no colour; an optional-content dictionary keeps its meaning.
    for annot in [
        "/C [0.9 0.1 0.1]",
        "/IC [0 0 0] /OverlayText (REDACTED) /DA (1 1 1 rg /Helv 10 Tf) /C [0.9 0.1 0.1]",
        "/OC [1 0]",
        "/OC [(a) 0 0]",
        "/OC << /Type /OCG /Name (L) >>",
    ] {
        assert!(blank(annot), "{annot}");
    }
    // With /OC, the interior colour and overlay text still wait for the redaction to be applied.
    let (ap, _) =
        display_of("<< /Type /Annot /Subtype /Redact /Rect [45 470 165 540] /OC [1 0 0] /IC [0 0 0] /OverlayText (X) /DA (1 1 1 rg /Helv 10 Tf) >>");
    assert!(!ap.contains(" f\n") && !ap.contains("Tj"), "{ap}");
    // With quadrilaterals and no /OC: the /C outline, as before.
    let (ap, _) = display_of("<< /Type /Annot /Subtype /Redact /Rect [45 470 165 540] /QuadPoints [45 540 165 540 45 470 165 470] /C [0 0 1] >>");
    assert_eq!(ap, "0 0 1 RG\n1 w\n45 540 m 165 540 l 165 470 l 45 470 l h S\n");
}

/// A redaction mark with quadrilaterals is outlined in its array `/OC` colour when it has one,
/// over `/C`; without `/OC` in `/C`, and without either in the default red.
#[test]
fn redaction_marks_with_quadpoints_take_their_outline_from_oc_then_c() {
    let quad = |extra: &str| {
        display_of(&format!("<< /Type /Annot /Subtype /Redact /Rect [45 470 165 540] /QuadPoints [45 540 165 540 45 470 165 470] {extra} >>")).0
    };
    let outline = "1 w\n45 540 m 165 540 l 165 470 l 45 470 l h S\n";
    assert_eq!(quad("/OC [0 0 1]"), format!("0 0 1 RG\n{outline}"), "/OC blue");
    assert_eq!(quad("/OC [0 0 1] /C [0 1 0]"), format!("0 0 1 RG\n{outline}"), "/OC over /C");
    assert_eq!(quad("/C [0 1 0]"), format!("0 1 0 RG\n{outline}"), "/C only");
    assert_eq!(quad(""), format!("0.89 0.13 0.13 RG\n{outline}"), "neither: red");
    // An optional-content /OC is not a colour.
    assert_eq!(quad("/OC << /Type /OCG /Name (L) >> /C [0 1 0]"), format!("0 1 0 RG\n{outline}"), "OCG /OC");
}

/// The six unapplied marks of the synthetic fixture the parity note's observation used: an array
/// `/OC` outlines the mark in its colour with or without `/QuadPoints`, beside `/C` or `/IC`; a
/// mark without `/QuadPoints` or `/OC` draws nothing while marked.
#[test]
fn unapplied_redaction_marks_follow_their_oc_outline_colour() {
    let quad = "/QuadPoints [60 520 260 520 60 420 260 420]";
    let cases = [
        ("/OC [0 0 1]", Some("0 0 1 RG")),
        ("/C [0 1 0]", None),
        ("/OC [0 0 1] /C [0 1 0]", Some("0 0 1 RG")),
        ("/OC [0 0 1] /IC [0 0 0]", Some("0 0 1 RG")),
        ("", None),
        (&format!("/OC [0 0 1] {quad}"), Some("0 0 1 RG")),
    ];
    for (extra, colour) in cases {
        let annot = format!("<< /Type /Annot /Subtype /Redact /Rect [60 420 260 520] {extra} >>");
        match colour {
            Some(colour) => {
                let (ap, _) = display_of(&annot);
                assert!(ap.starts_with(&format!("{colour}\n1 w\n")) && ap.ends_with(" h S\n") && !ap.contains(" f\n"), "{extra}: {ap}");
            }
            None => {
                let doc = document_of(&[
                    b"<< /Type /Catalog /Pages 2 0 R >>",
                    b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
                    b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
                    annot.as_bytes(),
                ]);
                assert!(with_missing_appearances(&doc).is_none(), "{extra}: drawn");
            }
        }
    }
}

/// New text re-fits the box in the face and size it is drawn in (`/DA`, or `/DS` where it sets
/// one), so the rectangle holds every drawn line.
#[test]
fn new_text_refits_the_box_in_its_drawn_face_and_size() {
    let bottom = |annot: &str, new_text: &str| {
        let mut doc = document_of(&[
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
            b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
            annot.as_bytes(),
        ]);
        set_contents(&mut doc, 0, 0, new_text, &meta("")).unwrap();
        let d = &list(&doc, 0)[0];
        let rect: Vec<f64> = d.get(b"Rect").and_then(|r| r.as_array()).unwrap().iter().filter_map(|x| x.as_f64()).collect();
        assert_eq!(rect[3], 700.0, "the top edge stays");
        (rect[1], ap_content(&doc, d).matches(" Tj").count())
    };
    // The default 1-pt border: a 3-pt pad, so 114 pt for the text; the box is lines × 1.2 × size + 8 tall.
    let words = "iiiiiiiiii iiiiiiiiii";
    let fits = |b: f64, want: f64| (b - want).abs() < 1e-9;
    let (b, n) = bottom("<< /Type /Annot /Subtype /FreeText /Rect [45 630 165 700] /DA (0 g /Helv 12 Tf) /Contents (Hi) >>", words);
    assert!(n == 1 && fits(b, 700.0 - 14.4 - 8.0), "Helvetica: one line, bottom {b}");
    let (b, n) = bottom("<< /Type /Annot /Subtype /FreeText /Rect [45 630 165 700] /DA (0 g /Cour 12 Tf) /Contents (Hi) >>", words);
    assert!(n == 2 && fits(b, 700.0 - 2.0 * 14.4 - 8.0), "Courier: two lines, bottom {b}");
    let (b, n) =
        bottom("<< /Type /Annot /Subtype /FreeText /Rect [45 630 165 700] /DA (0 g /Helv 12 Tf) /DS (font: Helvetica 24pt) /Contents (Hi) >>", "Hi");
    assert!(n == 1 && fits(b, 700.0 - 28.8 - 8.0), "/DS 24 pt: bottom {b}");
}

/// A colour or size change keeps the face a text box is drawn in: `/DA` keeps naming its font
/// resource (here Courier, and a `/DR` font that is Times Bold), and `/DS` says the same face
/// (ISO 32000-2 Table 177).
#[test]
fn a_colour_or_size_change_keeps_the_drawn_face() {
    use pdfcraft_fonts::Std14;
    let doc = |da: &str| {
        let annot = format!("<< /Type /Annot /Subtype /FreeText /Rect [45 500 300 700] /Contents (Hi) /DA ({da}) >>");
        document_of(&[
            b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /DR << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Times-Bold >> >> >> >> >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
            b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
            annot.as_bytes(),
        ])
    };
    let face = |doc: &Document| {
        let d = &list(doc, 0)[0];
        text_style::text_style(d, text_style::dr_face(doc, d)).face
    };
    for (da, font, want) in [("0 g /Cour 12 Tf", "/Cour", Std14::Courier), ("0 g /F1 12 Tf", "/F1", Std14::TimesBold)] {
        let mut d = doc(da);
        assert_eq!(face(&d), want, "{da}");
        set_style(&mut d, 0, 0, StyleChange::new(Some([1.0, 0.0, 0.0]), None, None, None), &meta("")).unwrap();
        assert_eq!(face(&d), want, "colour change, {da}");
        assert!(text(&list(&d, 0)[0], b"DA").contains(&format!("{font} 12 Tf")), "{da}");
        set_font_size(&mut d, 0, 0, 18.0, &meta("")).unwrap();
        assert_eq!(face(&d), want, "size change, {da}");
        let p = props(&d, 0, 0).unwrap();
        assert_eq!((p.color, p.font_size), (Some([1.0, 0.0, 0.0]), Some(18.0)), "{da}");
    }
    // The Courier box's appearance draws Courier.
    let mut d = doc("0 g /Cour 12 Tf");
    set_style(&mut d, 0, 0, StyleChange::new(Some([1.0, 0.0, 0.0]), None, None, None), &meta("")).unwrap();
    assert!(ap_content(&d, &list(&d, 0)[0]).contains("/Cour 12 Tf"));
}

/// A text box without an appearance is drawn from its `/DA` however the file wrote it: a `Tf`
/// without a size, a zero size, a font name the form's `/DR` doesn't list, and hostile operands.
/// The stream names the face the box is drawn in at a usable size, and no number in it is `NaN`
/// or infinite.
#[test]
fn text_boxes_draw_from_any_da_string() {
    let doc = |da: &str| {
        let annot = format!("<< /Type /Annot /Subtype /FreeText /Rect [45 500 300 700] /Contents (Hi) /DA ({da}) >>");
        document_of(&[
            b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /DR << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Times-Bold >> >> >> >> >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
            b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
            annot.as_bytes(),
        ])
    };
    for (da, tf, colour) in [
        ("0 g /Cour Tf", "/Cour 12 Tf", "0 0 0 rg"),
        ("0 g /Cour 0 Tf", "/Cour 12 Tf", "0 0 0 rg"),
        ("0 g /Cour -5 Tf", "/Cour 12 Tf", "0 0 0 rg"),
        ("NaN NaN NaN rg /Cour NaN Tf", "/Cour 12 Tf", "0 0 0 rg"),
        ("inf g /Cour inf Tf", "/Cour 12 Tf", "0 0 0 rg"),
        ("1 0 0 rg /Cour 1e300 Tf", "/Cour 400 Tf", "1 0 0 rg"),
        // In `/DR`: Times Bold. Not in `/DR` and no standard name: Helvetica.
        ("0 g /F1 Tf", "/TiBo 12 Tf", "0 0 0 rg"),
        ("0 g /F9 11 Tf", "/Helv 11 Tf", "0 0 0 rg"),
        // No Tf at all, or garbage.
        ("0 0 1 rg", "/Helv 12 Tf", "0 0 1 rg"),
        ("] [ << >> Tf", "/Helv 12 Tf", "0 0 0 rg"),
    ] {
        let mut d = doc(da);
        set_appearance(&mut d, ObjRef::new(4, 0)).unwrap();
        let content = ap_content(&d, &list(&d, 0)[0]);
        assert!(content.contains(tf), "{da:?}: {content}");
        assert!(content.contains(colour), "{da:?}: {content}");
        let lower = content.to_ascii_lowercase();
        assert!(!lower.contains("nan") && !lower.contains("inf"), "{da:?}: {content}");
    }
}

/// A callout with its text box at [300 600 500 640], knee [260 620] and arrow point [200 520]:
/// the knee line runs level from the box's left side, as Acrobat draws it.
fn callout_fixture() -> (Document, usize) {
    let mut doc = fixture();
    let c = add_annotation(
        &mut doc,
        &new(
            0,
            Shape::Callout {
                rect: [300.0, 600.0, 500.0, 640.0],
                knee: [260.0, 620.0],
                point: [200.0, 520.0],
                font_size: 12.0,
                ending: LineEnding::OpenArrow,
            },
        ),
        &meta("c"),
    )
    .unwrap();
    (doc, c)
}

fn nums(d: &Dict, k: &[u8]) -> Vec<f64> {
    d.get(k).unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect()
}

/// The text box a callout's `/Rect` and `/RD` describe.
fn callout_box_of(d: &Dict) -> [f64; 4] {
    let (r, rd) = (rect(d), nums(d, b"RD"));
    [r[0] + rd[0], r[1] + rd[1], r[2] - rd[2], r[3] - rd[3]]
}

/// #1152: a callout's arrow point and knee move on their own, as its handles drag them (Acrobat:
/// "Callout text boxes have three parts: a text box, a knee line, and an end-point line. You can
/// resize each part by dragging a handle", Acrobat DC Help, 11/28/2019, p. 357). The text box
/// stays and the line re-attaches to it.
#[test]
fn a_callouts_arrow_point_and_knee_move_on_their_own() {
    let (mut doc, c) = callout_fixture();
    // The arrow point alone: the knee and the box stay.
    set_points(&mut doc, 0, c, &[[150.0, 400.0], [260.0, 620.0]], &meta("p")).unwrap();
    let doc = reopen(&doc);
    let d = &list(&doc, 0)[c];
    assert_eq!(nums(d, b"CL"), [150.0, 400.0, 260.0, 620.0, 300.0, 620.0]);
    assert_eq!(callout_box_of(d), [300.0, 600.0, 500.0, 640.0]);
    // /Rect holds the line (padded) and the box.
    let r = rect(d);
    assert!(r[0] < 150.0 && r[1] < 400.0 && r[2] >= 500.0 && r[3] >= 640.0, "{r:?}");
    assert!(ap_content(&doc, d).contains("150 400 m"), "the line starts at the new arrow point");
    // The knee alone, to the right of the box: the line now meets the right side.
    let mut doc = doc;
    set_points(&mut doc, 0, c, &[[150.0, 400.0], [560.0, 620.0]], &meta("k")).unwrap();
    let d = &list(&doc, 0)[c];
    assert_eq!(nums(d, b"CL"), [150.0, 400.0, 560.0, 620.0, 500.0, 620.0]);
    assert_eq!(callout_box_of(d), [300.0, 600.0, 500.0, 640.0]);
    assert_eq!(summaries(&doc).iter().find(|s| s.index == c).map(|s| s.points.clone()), Some(vec![[150.0, 400.0], [560.0, 620.0]]));
}

/// A callout's leader line as its box moves or is resized, as Acrobat Pro DC (a 2021 Continuous
/// release) was seen to keep it (#1152): the arrow point never moves ("The text box moves around
/// a stationary anchor point", Acrobat DC Help, 11/28/2019, p. 357); moved, the knee and the end
/// come along;
/// resized with the side the line meets left in place, nothing changes; that side moved, the line
/// is laid out again from the side facing the arrow point with a 12 pt knee line, and the box stays
/// where it was put (Acrobat moves it next to the arrow point).
#[test]
fn a_callouts_box_moves_around_its_arrow_and_keeps_the_knee_beside_it() {
    let (mut doc, c) = callout_fixture();
    let cl = |doc: &Document| nums(&list(doc, 0)[c], b"CL");
    // Wider and taller, its left side where it was: the line stays.
    set_rect(&mut doc, 0, c, [300.0, 580.0, 540.0, 640.0], &meta("r")).unwrap();
    assert_eq!(cl(&doc), [200.0, 520.0, 260.0, 620.0, 300.0, 620.0]);
    // Moved 40 right and 10 up, same size: the knee and the end come along.
    set_rect(&mut doc, 0, c, [340.0, 590.0, 580.0, 650.0], &meta("m")).unwrap();
    assert_eq!(cl(&doc), [200.0, 520.0, 300.0, 630.0, 340.0, 630.0]);
    // The left side moved: the line meets its middle, the knee 12 pt out.
    set_rect(&mut doc, 0, c, [380.0, 590.0, 580.0, 650.0], &meta("s")).unwrap();
    assert_eq!(cl(&doc), [200.0, 520.0, 368.0, 620.0, 380.0, 620.0]);
    assert_eq!(callout_box_of(&list(&doc, 0)[c]), [380.0, 590.0, 580.0, 650.0], "the box stays where it was put");
    // Past the arrow point: the line leaves from the side facing it.
    set_rect(&mut doc, 0, c, [100.0, 600.0, 180.0, 640.0], &meta("p")).unwrap();
    assert_eq!(cl(&doc), [200.0, 520.0, 192.0, 620.0, 180.0, 620.0]);
}

/// The line is laid out again when the end no longer lies on the side it met (the box shrank past
/// it), and from the side facing the arrow point, not the old knee, which can end up over the box.
#[test]
fn a_callout_is_laid_out_again_when_its_end_leaves_the_box() {
    let (mut doc, c) = callout_fixture();
    // Left side unmoved, but the box now starts above the end (300, 620).
    set_rect(&mut doc, 0, c, [300.0, 625.0, 500.0, 640.0], &meta("a")).unwrap();
    assert_eq!(nums(&list(&doc, 0)[c], b"CL"), [200.0, 520.0, 288.0, 632.5, 300.0, 632.5]);
    let (mut doc, c) = callout_fixture();
    // Over the old knee (260, 620), which would face the bottom; the arrow faces the left side.
    set_rect(&mut doc, 0, c, [250.0, 590.0, 450.0, 640.0], &meta("b")).unwrap();
    assert_eq!(nums(&list(&doc, 0)[c], b"CL"), [200.0, 520.0, 238.0, 615.0, 250.0, 615.0]);
}

/// Review on #4094: the page sends a callout's text box back as `f32` values, so a box moved or
/// resized on screen comes back a few hundred-thousandths of a point off. That still counts as a
/// move (the knee comes along) and as the same side (the line stays), not as a new layout.
#[test]
fn a_callout_box_sent_back_as_f32_still_moves_and_keeps_its_line() {
    let mut doc = fixture();
    let c = add_annotation(
        &mut doc,
        &new(
            0,
            Shape::Callout {
                rect: [323.05, 292.14, 506.98, 374.91],
                knee: [311.05, 333.53],
                point: [299.05, 274.83],
                font_size: 12.0,
                ending: LineEnding::OpenArrow,
            },
        ),
        &meta("c"),
    )
    .unwrap();
    let f32s = |r: [f64; 4]| r.map(|v| f64::from(v as f32));
    let cl = |doc: &Document| nums(&list(doc, 0)[c], b"CL");
    let before = cl(&doc);
    // Resized from the bottom right, its left side where it was.
    set_rect(&mut doc, 0, c, f32s([323.05, 250.14, 540.98, 374.91]), &meta("r")).unwrap();
    assert_eq!(cl(&doc), before, "the line stays");
    // Moved by (47.62, 26.13), same size.
    let tb = callout_box_of(&list(&doc, 0)[c]);
    set_rect(&mut doc, 0, c, f32s([tb[0] + 47.62, tb[1] + 26.13, tb[2] + 47.62, tb[3] + 26.13]), &meta("m")).unwrap();
    let after = cl(&doc);
    assert!((after[2] - (before[2] + 47.62)).abs() < 0.01 && (after[3] - (before[3] + 26.13)).abs() < 0.01, "the knee came along: {after:?}");
    assert_eq!((after[0], after[1]), (before[0], before[1]), "the arrow point stayed");
}

/// Review on #4094: a `/CL` with something other than a number in it is not read at all, rather
/// than read with its numbers shifted into the wrong points.
#[test]
fn a_callout_line_with_a_non_number_is_not_read() {
    let (mut doc, c) = callout_fixture();
    let (_, r) = annot_ref(&mut doc, 0, c).unwrap();
    doc.update_dict(r, |d| {
        d.set(
            b"CL".to_vec(),
            Object::Array(vec![Object::name("x"), Object::Real(200.0), Object::Real(520.0), Object::Real(260.0), Object::Real(620.0)]),
        )
    })
    .unwrap();
    assert!(matches!(set_points(&mut doc, 0, c, &[[1.0, 1.0]], &meta("x")), Err(AnnotError::Invalid(_))), "not a callout line it can reshape");
    assert_eq!(summaries(&doc).into_iter().find(|s| s.index == c).map(|s| (s.points.len(), s.text_box.is_some())), Some((0, false)));
}

/// A new callout's knee is Acrobat's: 12 pt out from the middle of the side facing the arrow.
#[test]
fn a_new_callouts_knee_line_is_twelve_points() {
    let tb = [300.0, 600.0, 500.0, 640.0];
    assert_eq!(callout_knee(tb, [200.0, 520.0]), [288.0, 620.0]);
    assert_eq!(callout_knee(tb, [600.0, 700.0]), [512.0, 620.0]);
    assert_eq!(callout_knee(tb, [400.0, 700.0]), [400.0, 652.0]);
    assert_eq!(callout_knee(tb, [400.0, 500.0]), [400.0, 588.0]);
}

/// ISO 32000-2 Table 177: `/CL` may have four numbers (no knee). Resizing such a callout kept its
/// line only with six, so the box lost `/RD` and the line was left outside `/Rect`.
#[test]
fn a_callout_without_a_knee_keeps_its_line() {
    let (mut doc, c) = callout_fixture();
    let (_, r) = annot_ref(&mut doc, 0, c).unwrap();
    doc.update_dict(r, |d| d.set(b"CL".to_vec(), num_array(&[200.0, 520.0, 300.0, 620.0]))).unwrap();
    set_rect(&mut doc, 0, c, [300.0, 580.0, 500.0, 640.0], &meta("r")).unwrap();
    let d = &list(&doc, 0)[c];
    assert_eq!(nums(d, b"CL"), [200.0, 520.0, 300.0, 620.0], "still four numbers, the left side unmoved");
    assert_eq!(callout_box_of(d), [300.0, 580.0, 500.0, 640.0]);
    assert!(rect(d)[1] < 520.0, "/Rect holds the line");
    // Its one point is the arrow; a second adds a knee.
    assert_eq!(summaries(&doc).iter().find(|s| s.index == c).map(|s| s.points.clone()), Some(vec![[200.0, 520.0]]));
    set_points(&mut doc, 0, c, &[[210.0, 500.0]], &meta("p")).unwrap();
    assert_eq!(nums(&list(&doc, 0)[c], b"CL"), [210.0, 500.0, 300.0, 620.0]);
    set_points(&mut doc, 0, c, &[[210.0, 500.0], [250.0, 610.0]], &meta("k")).unwrap();
    assert_eq!(nums(&list(&doc, 0)[c], b"CL"), [210.0, 500.0, 250.0, 610.0, 300.0, 620.0]);
    // Back to no knee, then the left side moved: the end meets its middle.
    let (_, r) = annot_ref(&mut doc, 0, c).unwrap();
    doc.update_dict(r, |d| d.set(b"CL".to_vec(), num_array(&[210.0, 500.0, 300.0, 620.0]))).unwrap();
    set_rect(&mut doc, 0, c, [280.0, 590.0, 500.0, 640.0], &meta("s")).unwrap();
    assert_eq!(nums(&list(&doc, 0)[c], b"CL"), [210.0, 500.0, 280.0, 615.0]);
}

#[test]
fn a_callout_refuses_points_it_cant_have() {
    let (mut doc, c) = callout_fixture();
    let before = list(&doc, 0)[c].clone();
    for bad in [&[][..], &[[1.0, 2.0], [3.0, 4.0], [5.0, 6.0]][..], &[[f64::NAN, 1.0]][..], &[[1e9, 1.0]][..]] {
        assert!(matches!(set_points(&mut doc, 0, c, bad, &meta("x")), Err(AnnotError::Invalid(_))), "{bad:?}");
    }
    assert_eq!(list(&doc, 0)[c], before, "left as it was");
    // Without a readable /RD there is no box to keep, so nothing moves.
    let (_, r) = annot_ref(&mut doc, 0, c).unwrap();
    doc.update_dict(r, |d| {
        d.remove(b"RD");
    })
    .unwrap();
    assert!(matches!(set_points(&mut doc, 0, c, &[[1.0, 1.0]], &meta("x")), Err(AnnotError::Invalid(_))));
    // Nor with an /RD that puts the box outside /Rect (negative) or leaves no box (under 1 pt):
    // set_points refuses, and summaries offer no text box to drag.
    let outer = rect(&list(&doc, 0)[c]);
    for rd in [[-5.0, 50.0, 0.0, 0.0], [0.0, 0.0, outer[2] - outer[0] - 0.5, 0.0]] {
        doc.update_dict(r, |d| d.set(b"RD".to_vec(), num_array(&rd))).unwrap();
        assert!(matches!(set_points(&mut doc, 0, c, &[[1.0, 1.0]], &meta("x")), Err(AnnotError::Invalid(_))), "{rd:?}");
        assert_eq!(summaries(&doc).into_iter().find(|s| s.index == c).and_then(|s| s.text_box), None, "{rd:?}");
    }
}

/// The side a knee faces: left or right of the box first, then above or below. A knee on the line
/// of the left or right edge past the box is above or below it; the ties are in
/// `a_callouts_knee_has_one_side_for_every_tie`.
#[test]
fn a_callouts_knee_faces_the_side_it_is_beyond() {
    let tb = [100.0, 100.0, 200.0, 150.0];
    assert_eq!(callout_side(tb, [99.0, 125.0]), Side::Left);
    assert_eq!(callout_side(tb, [201.0, 125.0]), Side::Right);
    assert_eq!(callout_side(tb, [100.0, 160.0]), Side::Top, "on the left edge's line");
    assert_eq!(callout_side(tb, [200.0, 90.0]), Side::Bottom, "on the right edge's line");
    assert_eq!(callout_side(tb, [150.0, 125.0]), Side::Bottom, "inside");
    assert!(Side::Left.horizontal() && Side::Right.horizontal() && !Side::Top.horizontal() && !Side::Bottom.horizontal());
}

#[test]
fn summaries_give_a_callouts_points_and_text_box() {
    let (doc, c) = callout_fixture();
    let s = summaries(&doc).into_iter().find(|s| s.index == c).unwrap();
    assert_eq!(s.points, vec![[200.0, 520.0], [260.0, 620.0]]);
    assert_eq!(s.text_box, Some([300.0, 600.0, 500.0, 640.0]));
    let square = summaries(&doc).into_iter().find(|s| s.subtype == "Square").unwrap();
    assert_eq!((square.points.len(), square.text_box), (0, None));
}

/// A callout whose text box can't be read (no `/RD`, a negative inset, or no box left inside
/// `/Rect`) lists no points either, as `pdfcraft_render::inspect` does: `set_points` refuses it.
#[test]
fn summaries_give_no_callout_points_without_a_text_box() {
    for rd in [None, Some([-1.0, 0.0, 0.0, 0.0]), Some([0.0, 0.0, 0.0, 1000.0])] {
        let (mut doc, c) = callout_fixture();
        let (_, r) = annot_ref(&mut doc, 0, c).unwrap();
        doc.update_dict(r, |d| match rd {
            Some(rd) => d.set(b"RD".to_vec(), num_array(&rd)),
            None => {
                d.remove(b"RD");
            }
        })
        .unwrap();
        let s = summaries(&doc).into_iter().find(|s| s.index == c).unwrap();
        assert_eq!((s.points.len(), s.text_box), (0, None), "/RD {rd:?}");
    }
}

/// Review on #4094: a callout given its arrow point alone keeps its knee. The new line was built
/// from the one point and the end, so a six-number `/CL` silently became four.
#[test]
fn a_callouts_arrow_point_alone_keeps_its_knee() {
    let (mut doc, c) = callout_fixture();
    let before = nums(&list(&doc, 0)[c], b"CL");
    assert_eq!(before.len(), 6, "{before:?}");
    set_points(&mut doc, 0, c, &[[190.0, 500.0]], &meta("p")).unwrap();
    let after = nums(&list(&doc, 0)[c], b"CL");
    assert_eq!(after.len(), 6, "the knee stays: {after:?}");
    assert_eq!(after[..2], [190.0, 500.0], "the arrow point moved");
    assert_eq!(after[2..], before[2..], "the knee and the end are where they were");
}

/// Review on #4094: every tie has one defined side. Beside the box (left or right of it) wins over
/// above or below it, so a knee off a corner diagonally, or on the line of the top or bottom edge
/// past the box, takes the left or right side. On the box's outline a knee takes the side whose
/// edge it lies on (within 0.01 pt), and a corner the left or right side, as beside the box.
/// Inside the box, or with a coordinate that isn't a number, it takes the bottom side.
#[test]
fn a_callouts_knee_has_one_side_for_every_tie() {
    let tb = [100.0, 100.0, 200.0, 150.0];
    let cases: [([f64; 2], Side, &str); 30] = [
        ([100.004, 125.0], Side::Left, "on the left edge, give or take an f32"),
        ([199.996, 125.0], Side::Right, "on the right edge, give or take an f32"),
        ([150.0, 149.996], Side::Top, "on the top edge, give or take an f32"),
        ([100.02, 125.0], Side::Bottom, "inside, past the slack"),
        ([150.0, 125.0], Side::Bottom, "inside, at the middle"),
        ([110.0, 145.0], Side::Bottom, "inside, near the top-left corner"),
        ([100.0, 125.0], Side::Left, "on the left edge"),
        ([200.0, 125.0], Side::Right, "on the right edge"),
        ([150.0, 150.0], Side::Top, "on the top edge"),
        ([150.0, 100.0], Side::Bottom, "on the bottom edge"),
        ([100.0, 150.0], Side::Left, "the top-left corner"),
        ([200.0, 150.0], Side::Right, "the top-right corner"),
        ([100.0, 100.0], Side::Left, "the bottom-left corner"),
        ([200.0, 100.0], Side::Right, "the bottom-right corner"),
        ([90.0, 150.0], Side::Left, "the top edge's line, left of the box"),
        ([210.0, 150.0], Side::Right, "the top edge's line, right of the box"),
        ([90.0, 100.0], Side::Left, "the bottom edge's line, left of the box"),
        ([100.0, 160.0], Side::Top, "the left edge's line, above the box"),
        ([100.0, 90.0], Side::Bottom, "the left edge's line, below the box"),
        ([200.0, 160.0], Side::Top, "the right edge's line, above the box"),
        ([90.0, 160.0], Side::Left, "diagonally off the top-left corner"),
        ([210.0, 90.0], Side::Right, "diagonally off the bottom-right corner"),
        ([f64::NAN, 125.0], Side::Bottom, "x not a number"),
        ([150.0, f64::NAN], Side::Bottom, "y not a number"),
        ([f64::NAN, 160.0], Side::Bottom, "x not a number, y above the box"),
        ([f64::NAN, 90.0], Side::Bottom, "x not a number, y below the box"),
        ([90.0, f64::NAN], Side::Bottom, "y not a number, x left of the box"),
        ([210.0, f64::NAN], Side::Bottom, "y not a number, x right of the box"),
        ([f64::NEG_INFINITY, 125.0], Side::Left, "infinitely far left"),
        ([150.0, f64::INFINITY], Side::Top, "infinitely far up"),
    ];
    for (knee, side, what) in cases {
        assert_eq!(callout_side(tb, knee), side, "{what}: {knee:?}");
        // Written corners first, the box is the same box.
        assert_eq!(callout_side([200.0, 150.0, 100.0, 100.0], knee), side, "{what}, corners first");
        // The line meets the middle of that side.
        let want = match side {
            Side::Left => [100.0, 125.0],
            Side::Right => [200.0, 125.0],
            Side::Top => [150.0, 150.0],
            Side::Bottom => [150.0, 100.0],
        };
        assert_eq!(callout_attach(tb, knee), want, "{what}");
    }
}

/// A knee dragged exactly onto the box's left edge keeps the line on the left side, where its end
/// already is, instead of sending it across the box to the bottom; a knee dragged inside the box
/// meets the bottom side.
#[test]
fn a_callouts_knee_on_or_inside_the_box_meets_a_defined_side() {
    let (mut doc, c) = callout_fixture();
    set_points(&mut doc, 0, c, &[[200.0, 520.0], [300.0, 630.0]], &meta("edge")).unwrap();
    assert_eq!(nums(&list(&doc, 0)[c], b"CL"), [200.0, 520.0, 300.0, 630.0, 300.0, 620.0]);
    set_points(&mut doc, 0, c, &[[200.0, 520.0], [350.0, 620.0]], &meta("inside")).unwrap();
    assert_eq!(nums(&list(&doc, 0)[c], b"CL"), [200.0, 520.0, 350.0, 620.0, 400.0, 600.0]);
    assert_eq!(callout_box_of(&list(&doc, 0)[c]), [300.0, 600.0, 500.0, 640.0], "the box stays");
}

/// A callout's `/CL` read as numbers, and the check every edit must pass: four numbers (arrow
/// point, end) or six (arrow point, knee, end), all finite (ISO 32000-2 Table 177), every point
/// inside `/Rect`, and a text box (`/Rect` inset by `/RD`) at least 1 pt a side.
fn assert_callout_line(d: &Dict, len: usize, what: &str) -> Vec<f64> {
    let cl = nums(d, b"CL");
    assert_eq!(cl.len(), len, "{what}: {cl:?}");
    assert!(cl.iter().all(|v| v.is_finite()), "{what}: {cl:?}");
    let r = rect(d);
    for p in cl.chunks(2) {
        assert!(p[0] >= r[0] - 1e-6 && p[0] <= r[2] + 1e-6 && p[1] >= r[1] - 1e-6 && p[1] <= r[3] + 1e-6, "{what}: {p:?} outside {r:?}");
    }
    let tb = callout_box_of(d);
    assert!(tb[2] - tb[0] >= 1.0 && tb[3] - tb[1] >= 1.0, "{what}: text box {tb:?}");
    cl
}

/// Review on #4094: every edit of a callout keeps `/CL` four numbers when it had four and six when
/// it had six: the arrow point dragged, the knee dragged, the box moved, the box resized, the whole
/// callout moved, and the file saved and opened again.
#[test]
fn every_callout_edit_keeps_four_or_six_numbers() {
    for len in [4usize, 6] {
        let (mut doc, c) = callout_fixture();
        if len == 4 {
            let (_, r) = annot_ref(&mut doc, 0, c).unwrap();
            doc.update_dict(r, |d| d.set(b"CL".to_vec(), num_array(&[200.0, 520.0, 300.0, 620.0]))).unwrap();
        }
        let cl = |doc: &Document, what: &str| assert_callout_line(&list(doc, 0)[c], len, &format!("{len}: {what}"));
        set_points(&mut doc, 0, c, &[[150.0, 400.0]], &meta("tip")).unwrap();
        assert_eq!(cl(&doc, "the arrow point")[..2], [150.0, 400.0]);
        if len == 6 {
            set_points(&mut doc, 0, c, &[[150.0, 400.0], [260.0, 630.0]], &meta("knee")).unwrap();
            assert_eq!(cl(&doc, "the knee")[2..4], [260.0, 630.0]);
        }
        let tb = callout_box_of(&list(&doc, 0)[c]);
        set_rect(&mut doc, 0, c, [tb[0] + 40.0, tb[1] + 10.0, tb[2] + 40.0, tb[3] + 10.0], &meta("box")).unwrap();
        assert_eq!(cl(&doc, "the box moved")[..2], [150.0, 400.0], "the arrow point stays");
        let tb = callout_box_of(&list(&doc, 0)[c]);
        set_rect(&mut doc, 0, c, [tb[0] + 30.0, tb[1] - 20.0, tb[2] + 10.0, tb[3]], &meta("size")).unwrap();
        let before = cl(&doc, "the box resized");
        move_annotation(&mut doc, 0, c, 20.0, -15.0, &meta("move")).unwrap();
        let after = cl(&doc, "the callout moved");
        let shifted: Vec<f64> = before.iter().enumerate().map(|(i, v)| v + if i % 2 == 0 { 20.0 } else { -15.0 }).collect();
        assert_eq!(after, shifted, "{len}: every point moves with it");
        let doc = reopen(&doc);
        cl(&doc, "saved and opened again");
    }
}

/// Review on #4094: a `/CL` that is neither four nor six finite numbers (two, three, five or eight
/// numbers, an infinite or not-a-number value, an empty array, a nested array, a name or a string)
/// is not a callout line PdfCraft can reshape. No edit panics, the arrow point and knee are
/// refused, and none writes a `/CL` of another length: resizing leaves it as it was and moving
/// shifts its numbers in place. A `/CL` held as an indirect object is read as the line it is.
#[test]
fn a_hostile_callout_line_is_never_rewritten_into_another_shape() {
    let reals = |v: &[f64]| Object::Array(v.iter().map(|x| Object::Real(*x)).collect());
    let hostile: Vec<(&str, Object)> = vec![
        ("two numbers", reals(&[200.0, 520.0])),
        ("three numbers", reals(&[200.0, 520.0, 260.0])),
        ("five numbers", reals(&[200.0, 520.0, 260.0, 620.0, 300.0])),
        ("eight numbers", reals(&[200.0, 520.0, 260.0, 620.0, 280.0, 620.0, 300.0, 620.0])),
        ("not a number", reals(&[200.0, f64::NAN, 260.0, 620.0, 300.0, 620.0])),
        ("infinite", reals(&[f64::INFINITY, 520.0, 260.0, 620.0])),
        ("empty", Object::Array(Vec::new())),
        ("nested", Object::Array(vec![reals(&[200.0, 520.0]), reals(&[300.0, 620.0])])),
        ("a name", Object::name("CL")),
        ("a string", PdfString::text("200 520 300 620").into()),
    ];
    for (what, cl) in hostile {
        let (mut doc, c) = callout_fixture();
        let (_, r) = annot_ref(&mut doc, 0, c).unwrap();
        doc.update_dict(r, |d| d.set(b"CL".to_vec(), cl.clone())).unwrap();
        let len = cl.as_array().map(Vec::len);
        for points in [&[[150.0, 400.0]][..], &[[150.0, 400.0], [260.0, 630.0]][..]] {
            assert!(set_points(&mut doc, 0, c, points, &meta("p")).is_err(), "{what}: {points:?}");
        }
        // Compared as written: a not-a-number value is never equal to itself.
        let same = |doc: &Document| format!("{:?}", list(doc, 0)[c].get(b"CL")) == format!("{:?}", Some(&cl));
        assert!(same(&doc), "{what}: refused, so unchanged");
        let s = summaries(&doc).into_iter().find(|s| s.index == c).unwrap();
        assert_eq!((s.points.len(), s.text_box), (0, None), "{what}: nothing to drag");
        let _ = set_rect(&mut doc, 0, c, [300.0, 580.0, 520.0, 640.0], &meta("r"));
        assert!(same(&doc), "{what}: resizing leaves it as it was");
        let _ = move_annotation(&mut doc, 0, c, 20.0, -15.0, &meta("m"));
        assert_eq!(list(&doc, 0)[c].get(b"CL").and_then(Object::as_array).map(Vec::len), len, "{what}: moved in place");
        reopen(&doc);
    }
    // Held as an indirect object, a six-number line is a callout line like any other.
    let (mut doc, c) = callout_fixture();
    let line = doc.add(reals(&[200.0, 520.0, 260.0, 620.0, 300.0, 620.0]));
    let (_, r) = annot_ref(&mut doc, 0, c).unwrap();
    doc.update_dict(r, |d| d.set(b"CL".to_vec(), Object::Ref(line))).unwrap();
    set_points(&mut doc, 0, c, &[[150.0, 400.0]], &meta("p")).unwrap();
    assert_eq!(assert_callout_line(&list(&doc, 0)[c], 6, "indirect"), [150.0, 400.0, 260.0, 620.0, 300.0, 620.0]);
}

/// A pop-up is opened, closed and moved through `set_popup` (#comment.popups): a note's own
/// pop-up follows, a comment without one gets one beside it, and the comment itself (its text,
/// its date) is left alone. Summaries report the pop-up. Text boxes, Fill & Sign marks and
/// replies have none of their own; a bad rectangle is refused.
#[test]
fn pop_ups_open_close_and_move_and_are_reported() {
    let mut doc = fixture();
    let note = add_annotation(&mut doc, &new(0, Shape::Note { at: [20.0, 280.0], icon: NoteIcon::Comment }), &meta("n1")).unwrap();
    let before = summaries(&doc).into_iter().find(|s| s.index == note).unwrap();
    assert_eq!(before.popup.map(|p| p.open), Some(false), "new notes have a closed pop-up");
    set_popup(&mut doc, 0, note, None, Some(true), &meta("n1")).unwrap();
    let after = summaries(&doc).into_iter().find(|s| s.index == note).unwrap();
    assert_eq!(after.popup.map(|p| p.open), Some(true));
    assert_eq!(after.popup.map(|p| p.rect), before.popup.map(|p| p.rect), "opening does not move it");
    assert_eq!((after.modified, after.contents.as_deref()), (before.modified, Some("hello")), "the comment is untouched");
    let (_, r) = annot_ref(&mut doc, 0, note).unwrap();
    assert_eq!(annot_dict(&doc, r).get(b"Open"), Some(&Object::Bool(true)), "the note's own /Open follows");
    set_popup(&mut doc, 0, note, Some([150.0, 100.0, 290.0, 200.0]), Some(false), &meta("n1")).unwrap();
    let moved = summaries(&doc).into_iter().find(|s| s.index == note).unwrap();
    assert_eq!(moved.popup, Some(PopupInfo { rect: [150.0, 100.0, 290.0, 200.0], open: false }));
    assert_eq!(annot_dict(&doc, r).get(b"Open"), Some(&Object::Bool(false)));

    // A highlight is created without a pop-up: the first open gives it one, beside it.
    let mark = add_annotation(
        &mut doc,
        &new(0, Shape::TextMarkup { kind: Markup::Highlight, quads: vec![[20.0, 200.0, 120.0, 200.0, 20.0, 190.0, 120.0, 190.0]] }),
        &meta("h1"),
    )
    .unwrap();
    assert_eq!(summaries(&doc).into_iter().find(|s| s.index == mark).unwrap().popup, None);
    set_popup(&mut doc, 0, mark, None, Some(true), &meta("h1")).unwrap();
    let given = summaries(&doc).into_iter().find(|s| s.index == mark).unwrap().popup.expect("a pop-up now");
    assert!(given.open);
    assert!((given.rect[0] - 130.0).abs() < 0.01 && (given.rect[2] - 330.0).abs() < 0.01, "beside the mark: {:?}", given.rect);

    // #2137 review: a /Popup naming another annotation (here the note) isn't this comment's: the
    // note is left alone and the mark gets a pop-up of its own.
    let (_, mr) = annot_ref(&mut doc, 0, mark).unwrap();
    doc.update_dict(mr, |m| m.set(b"Popup".to_vec(), Object::Ref(r))).unwrap();
    let note_before = annot_dict(&doc, r);
    set_popup(&mut doc, 0, mark, Some([10.0, 10.0, 110.0, 60.0]), Some(true), &meta("h1")).unwrap();
    assert_eq!(annot_dict(&doc, r), note_before, "the note isn't moved or opened");
    let own = annot_dict(&doc, mr).reference(b"Popup").expect("a pop-up of its own");
    assert_ne!(own, r);
    assert_eq!(doc.get(own).as_dict().and_then(|p| p.reference(b"Parent")), Some(mr));
    // Its own pop-up, with /Parent naming it, is the one moved next time.
    set_popup(&mut doc, 0, mark, Some([12.0, 10.0, 112.0, 60.0]), None, &meta("h1")).unwrap();
    assert_eq!(annot_dict(&doc, mr).reference(b"Popup"), Some(own), "the same pop-up");

    // #2137 review: nor is it reported as the comment's (see the helper).
    foreign_pop_ups_are_not_the_marks(&mut doc, r, mark, mr);

    // None of their own.
    let text_box = add_annotation(&mut doc, &new(0, Shape::TextBox { rect: [10.0, 10.0, 110.0, 40.0], font_size: 12.0 }), &meta("t1")).unwrap();
    assert!(matches!(set_popup(&mut doc, 0, text_box, None, Some(true), &meta("t1")), Err(AnnotError::Invalid(_))));
    let reply = add_reply(&mut doc, 0, note, "Agreed", "Ada", &meta("r1")).unwrap();
    assert!(matches!(set_popup(&mut doc, 0, reply, None, Some(true), &meta("r1")), Err(AnnotError::Invalid(_))));
    assert!(matches!(set_popup(&mut doc, 0, note, Some([0.0, 0.0, 0.5, 0.5]), None, &meta("n1")), Err(AnnotError::Invalid(_))));
    assert!(matches!(set_popup(&mut doc, 0, note, Some([f64::NAN, 0.0, 10.0, 10.0]), None, &meta("n1")), Err(AnnotError::Invalid(_))));
    assert!(matches!(set_popup(&mut doc, 0, 99, None, Some(true), &meta("n1")), Err(AnnotError::NoSuchAnnotation { .. })));

    // #2137 review: /Annots may list the note through an object that is only a reference to it.
    // Its pop-up's /Parent names the note itself, so the pop-up is still the note's.
    let p = page_ref(&doc, 0).unwrap();
    let via = doc.add(Object::Ref(r));
    let list: Vec<Object> = annots(&doc, p).into_iter().map(|o| if o.as_ref() == Some(r) { Object::Ref(via) } else { o }).collect();
    set_annots(&mut doc, p, list).unwrap();
    let listed = summaries(&doc).into_iter().find(|s| s.index == note).unwrap();
    assert_eq!(listed.popup, Some(PopupInfo { rect: [150.0, 100.0, 290.0, 200.0], open: false }), "listed through {via:?}");
}

/// #2137 review: a pop-up that is not the mark's own is neither reported as its pop-up nor
/// changed through it (part of `pop_ups_open_close_and_move_and_are_reported`).
fn foreign_pop_ups_are_not_the_marks(doc: &mut Document, note: ObjRef, mark: usize, mr: ObjRef) {
    // #2137 review: nor is it reported as the comment's. A /Popup naming the note, or the note's
    // own pop-up (its /Parent is the note), gives the mark no pop-up in the summaries.
    let popup_of_mark = |doc: &Document| summaries(doc).into_iter().find(|s| s.index == mark).unwrap().popup;
    let notes_popup = annot_dict(doc, note).reference(b"Popup").expect("the note's pop-up");
    for foreign in [note, notes_popup] {
        doc.update_dict(mr, |m| m.set(b"Popup".to_vec(), Object::Ref(foreign))).unwrap();
        assert_eq!(popup_of_mark(doc), None, "{foreign:?} isn't the mark's");
    }
    // A pop-up whose /Parent isn't a reference (it must be one) isn't the mark's either: it is
    // neither reported nor changed, and the mark gets one of its own.
    let mut bad = Dict::new();
    bad.set(b"Type".to_vec(), Object::name("Annot"));
    bad.set(b"Subtype".to_vec(), Object::name("Popup"));
    bad.set(b"Rect".to_vec(), Object::Array([0.0, 0.0, 50.0, 50.0].iter().map(|v| Object::Real(*v)).collect()));
    bad.set(b"Parent".to_vec(), Object::Dict(Dict::new()));
    let bad = doc.add(Object::Dict(bad));
    doc.update_dict(mr, |m| m.set(b"Popup".to_vec(), Object::Ref(bad))).unwrap();
    assert_eq!(popup_of_mark(doc), None, "a direct /Parent");
    let bad_before = doc.get(bad).as_dict().cloned();
    set_popup(doc, 0, mark, None, Some(true), &meta("h1")).unwrap();
    assert_eq!(doc.get(bad).as_dict().cloned(), bad_before, "left alone");
    assert_ne!(annot_dict(doc, mr).reference(b"Popup"), Some(bad));
    // A pop-up with no /Parent at all is taken as the comment's.
    let mut orphan = Dict::new();
    orphan.set(b"Type".to_vec(), Object::name("Annot"));
    orphan.set(b"Subtype".to_vec(), Object::name("Popup"));
    orphan.set(b"Rect".to_vec(), Object::Array([0.0, 0.0, 50.0, 50.0].iter().map(|v| Object::Real(*v)).collect()));
    let orphan = doc.add(Object::Dict(orphan));
    doc.update_dict(mr, |m| m.set(b"Popup".to_vec(), Object::Ref(orphan))).unwrap();
    assert_eq!(popup_of_mark(doc), Some(PopupInfo { rect: [0.0, 0.0, 50.0, 50.0], open: false }), "no /Parent");
}

/// #2137 review: a comment's own pop-up without a `/Rect` of four numbers reads as none; opening
/// it gives it the place a new pop-up gets, so the window shows again.
#[test]
fn opening_a_pop_up_without_a_rect_gives_it_one() {
    let mut doc = fixture();
    let note = add_annotation(&mut doc, &new(0, Shape::Note { at: [20.0, 280.0], icon: NoteIcon::Comment }), &meta("n1")).unwrap();
    let (_, r) = annot_ref(&mut doc, 0, note).unwrap();
    let pr = annot_dict(&doc, r).reference(b"Popup").expect("new notes have a pop-up");
    doc.update_dict(pr, |p| {
        p.remove(b"Rect");
    })
    .unwrap();
    assert_eq!(summaries(&doc).into_iter().find(|s| s.index == note).unwrap().popup, None, "no /Rect reads as no pop-up");
    set_popup(&mut doc, 0, note, None, Some(true), &meta("n1")).unwrap();
    let popup = summaries(&doc).into_iter().find(|s| s.index == note).unwrap().popup.expect("the pop-up reads again");
    assert!(popup.open);
    assert!(popup.rect[2] - popup.rect[0] >= 100.0 && popup.rect[3] - popup.rect[1] >= 100.0, "a window's size: {:?}", popup.rect);
    assert_eq!(annot_dict(&doc, r).reference(b"Popup"), Some(pr), "the same pop-up, not a new one");
}

/// #2137 review: a pop-up rectangle with its corners reversed is read, and saved, with them in
/// order (ISO 32000-2 §7.9.5: a reader normalises a rectangle). One whose numbers aren't finite
/// as `f32` (a file's huge number) reads as no pop-up, and opening it gives it a new place.
#[test]
fn a_pop_up_rect_with_reversed_corners_is_put_in_order_and_a_huge_one_is_none() {
    let mut doc = fixture();
    let note = add_annotation(&mut doc, &new(0, Shape::Note { at: [20.0, 280.0], icon: NoteIcon::Comment }), &meta("n1")).unwrap();
    let (_, r) = annot_ref(&mut doc, 0, note).unwrap();
    let pr = annot_dict(&doc, r).reference(b"Popup").expect("new notes have a pop-up");
    let popup = |doc: &Document| summaries(doc).into_iter().find(|s| s.index == note).unwrap().popup;
    let set_rect = |doc: &mut Document, rect: [f64; 4]| doc.update_dict(pr, |p| p.set(b"Rect".to_vec(), num_array(&rect))).unwrap();

    // Reversed in the file: read in order.
    set_rect(&mut doc, [290.0, 200.0, 150.0, 100.0]);
    assert_eq!(popup(&doc), Some(PopupInfo { rect: [150.0, 100.0, 290.0, 200.0], open: false }));
    // Reversed through `set_popup`: saved in order.
    set_popup(&mut doc, 0, note, Some([300.0, 220.0, 160.0, 120.0]), None, &meta("n1")).unwrap();
    let saved: Vec<f64> =
        doc.get(pr).as_dict().and_then(|p| p.get(b"Rect")).and_then(Object::as_array).unwrap().iter().filter_map(Object::as_f64).collect();
    assert_eq!(saved, [160.0, 120.0, 300.0, 220.0]);

    // A number too large for `f32` (or infinite): no pop-up, and opening it gives it a place.
    for huge in [1e300, f64::INFINITY] {
        set_rect(&mut doc, [0.0, 0.0, huge, 100.0]);
        assert_eq!(popup(&doc), None, "{huge}");
    }
    set_popup(&mut doc, 0, note, None, Some(true), &meta("n1")).unwrap();
    let given = popup(&doc).expect("the pop-up reads again");
    assert!(given.open && given.rect.iter().all(|v| v.is_finite()), "{given:?}");
}

#[test]
fn freehand_highlight_is_compatible_ink_with_marker_blending() {
    let shape = Shape::FreehandHighlight { strokes: vec![vec![[20.0, 100.0], [120.0, 100.0]]] };
    let mut annotation = new(0, shape.clone());
    annotation.style = Style::default_for(&shape);
    let mut doc = fixture();
    let index = add_annotation(&mut doc, &annotation, &meta("marker")).unwrap();
    let doc = reopen(&doc);
    let ink = &list(&doc, 0)[index];

    assert_eq!(ink.name(b"Subtype"), Some(&b"Ink"[..]));
    assert_eq!(ink.name(b"IT"), Some(&b"FreehandHighlight"[..]));
    assert_eq!(ink.get(b"CA").and_then(Object::as_f64), Some(0.35));
    let ap = ink.get(b"AP").unwrap().as_dict().unwrap().reference(b"N").unwrap();
    let resources = doc.get(ap).as_dict().unwrap().get(b"Resources").cloned().unwrap();
    let blend = resources.as_dict().unwrap().get(b"ExtGState").unwrap().as_dict().unwrap().get(b"GS0").unwrap().as_dict().unwrap();
    assert_eq!(blend.name(b"BM"), Some(&b"Multiply"[..]));

    let mut doc = doc;
    let rect_of = |doc: &Document, idx: usize| -> Vec<f64> {
        list(doc, 0)[idx].get(b"Rect").unwrap().as_array().unwrap().iter().filter_map(Object::as_f64).collect()
    };
    let rect_before = rect_of(&doc, index);
    set_style(&mut doc, 0, index, StyleChange { color: None, opacity: None, width: Some(34.0), endings: None }, &meta("wider")).unwrap();
    let rect_after = rect_of(&doc, index);
    assert!((rect_after[0] - (rect_before[0] - 10.0)).abs() < 1e-4);
    assert!((rect_after[2] - (rect_before[2] + 10.0)).abs() < 1e-4);

    // Repeated increase/decrease cycles must keep bounds stable and never accumulate padding.
    set_style(&mut doc, 0, index, StyleChange { color: None, opacity: None, width: Some(14.0), endings: None }, &meta("narrower")).unwrap();
    let rect_restored = rect_of(&doc, index);
    for (a, b) in rect_restored.iter().zip(rect_before.iter()) {
        assert!((a - b).abs() < 1e-4, "expected {rect_before:?}, got {rect_restored:?}");
    }
    set_style(&mut doc, 0, index, StyleChange { color: None, opacity: None, width: Some(34.0), endings: None }, &meta("wider-again")).unwrap();
    let rect_second_wide = rect_of(&doc, index);
    for (a, b) in rect_second_wide.iter().zip(rect_after.iter()) {
        assert!((a - b).abs() < 1e-4, "expected {rect_after:?}, got {rect_second_wide:?}");
    }

    // Imported ink whose width is specified only by `/Border [0 0 20]` must use 20 pt as its
    // previous width (shrinking by 5 pt on a decrease to 10 pt rather than expanding as if 1 pt).
    let (_, r) = lookup::annot_ref(&mut doc, 0, index).unwrap();
    doc.update_dict(r, |d| {
        d.remove(b"BS");
        d.set(b"Border".to_vec(), Object::Array(vec![Object::Real(0.0), Object::Real(0.0), Object::Real(20.0)]));
        d.set(b"Rect".to_vec(), create::num_array(&[5.0, 85.0, 135.0, 115.0]));
    })
    .unwrap();
    set_style(&mut doc, 0, index, StyleChange { color: None, opacity: None, width: Some(10.0), endings: None }, &meta("border-shrink")).unwrap();
    let rect_from_border = rect_of(&doc, index);
    for (a, b) in rect_from_border.iter().zip([10.0, 90.0, 130.0, 110.0]) {
        assert!((a - b).abs() < 1e-4, "expected [10, 90, 130, 110], got {rect_from_border:?}");
    }
}

/// A 3D annotation is PDF 1.6 and a PRC model 1.7 (ISO 32000-2 §13.6): an older document's
/// catalog says so, and a newer one is left as it is.
#[test]
fn a_3d_model_raises_an_older_document_to_its_version() {
    let catalog_version = |doc: &Document| {
        let root = doc.root().unwrap();
        doc.get(root).as_dict().unwrap().name(b"Version").map(<[u8]>::to_vec)
    };
    let add = |doc: &mut Document, model: &[u8]| {
        let new = NewModel3d { page: 0, rect: [100.0, 100.0, 300.0, 300.0], model, activation: Activation3d::default(), poster: None, contents: "" };
        add_3d_model(doc, &new, &meta("m")).unwrap();
    };
    let mut doc = fixture();
    add(&mut doc, b"U3D\0rest");
    assert_eq!(catalog_version(&reopen(&doc)), None, "a 1.7 file needs nothing for U3D");
    for (model, version) in [(&b"U3D\0rest"[..], &b"1.6"[..]), (b"PRC rest", b"1.7")] {
        let mut doc = fixture();
        doc.set_version("1.4");
        add(&mut doc, model);
        assert_eq!(catalog_version(&reopen(&doc)).as_deref(), Some(version));
    }
    let mut doc = fixture();
    doc.set_version("2.0");
    add(&mut doc, b"PRC rest");
    assert_eq!(catalog_version(&reopen(&doc)), None, "a 2.0 file takes PRC as it is");
}

/// Before 2.0, PRC is an Adobe extension to PDF 1.7 (§7.12): a PRC model declares it, keeping
/// other extensions and a later ADBE level; U3D and a 2.0 file need nothing.
#[test]
fn a_prc_model_declares_the_adobe_extension_before_pdf_2() {
    let adbe = |doc: &Document| {
        let root = doc.root().unwrap();
        let ext = doc.get(root).as_dict().unwrap().get(b"Extensions").map(|e| doc.resolve(e).as_dict().unwrap().clone());
        ext.map(|e| {
            let adbe = e.get(b"ADBE").map(|a| a.as_dict().unwrap().clone());
            (e.get(b"XYZ").is_some(), adbe.map(|a| (a.name(b"BaseVersion").map(<[u8]>::to_vec), a.get(b"ExtensionLevel").and_then(Object::as_int))))
        })
    };
    let add = |doc: &mut Document, model: &[u8]| {
        let new = NewModel3d { page: 0, rect: [100.0, 100.0, 300.0, 300.0], model, activation: Activation3d::default(), poster: None, contents: "" };
        add_3d_model(doc, &new, &meta("m")).unwrap();
    };
    let mut doc = fixture();
    add(&mut doc, b"PRC rest");
    assert_eq!(adbe(&reopen(&doc)), Some((false, Some((Some(b"1.7".to_vec()), Some(1))))));
    let mut doc = fixture();
    add(&mut doc, b"U3D rest");
    assert_eq!(adbe(&reopen(&doc)), None, "U3D is in ISO 32000-1");
    let mut doc = fixture();
    doc.set_version("2.0");
    add(&mut doc, b"PRC rest");
    assert_eq!(adbe(&reopen(&doc)), None, "PRC is in ISO 32000-2");

    let mut doc = fixture();
    let root = doc.root().unwrap();
    let mut level3 = Dict::new();
    level3.set(b"BaseVersion".to_vec(), Object::name("1.7"));
    level3.set(b"ExtensionLevel".to_vec(), Object::Int(3));
    let mut ext = Dict::new();
    ext.set(b"ADBE".to_vec(), Object::Dict(level3));
    ext.set(b"XYZ".to_vec(), Object::Dict(Dict::new()));
    doc.update_dict(root, |c| c.set(b"Extensions".to_vec(), Object::Dict(ext.clone()))).unwrap();
    add(&mut doc, b"PRC rest");
    assert_eq!(adbe(&reopen(&doc)), Some((true, Some((Some(b"1.7".to_vec()), Some(3))))), "a later level is kept");
    ext.remove(b"ADBE");
    let mut doc = fixture();
    doc.update_dict(root, |c| c.set(b"Extensions".to_vec(), Object::Dict(ext))).unwrap();
    add(&mut doc, b"PRC rest");
    assert_eq!(adbe(&reopen(&doc)), Some((true, Some((Some(b"1.7".to_vec()), Some(1))))), "other extensions are kept");

    let mut doc = fixture();
    let mut ext = Dict::new();
    ext.set(b"ADBE".to_vec(), Object::Array(vec![Object::Dict(Dict::new())]));
    let ext = doc.add(Object::Dict(ext));
    doc.update_dict(root, |c| c.set(b"Extensions".to_vec(), Object::Ref(ext))).unwrap();
    add(&mut doc, b"PRC rest");
    let doc = reopen(&doc);
    let ext = doc.resolve(doc.get(doc.root().unwrap()).as_dict().unwrap().get(b"Extensions").unwrap());
    assert!(ext.as_dict().unwrap().get(b"ADBE").unwrap().as_array().is_some(), "an array of ADBE extensions is left alone");
}

#[test]
fn a_picture_is_placed_by_its_box_and_a_form_needs_a_positive_size() {
    let at = [10.0, 20.0, 110.0, 70.0];
    assert_eq!(appearance::picture_place(at, true, None).as_deref(), Some("100 0 0 50 10 20 cm"), "an image fills its unit square");
    assert_eq!(appearance::picture_place(at, false, Some((200.0, 25.0))).as_deref(), Some("0.5 0 0 2 10 20 cm"));
    for size in [None, Some((0.0, 25.0)), Some((200.0, -1.0))] {
        assert_eq!(appearance::picture_place(at, false, size), None, "{size:?}");
    }
}

/// The poster's box must lie in the rectangle, and a form poster needs a size to scale by.
#[test]
fn a_3d_poster_outside_its_rectangle_or_without_a_size_is_refused() {
    let picture = |at: [f64; 4], size: (f64, f64)| PosterPicture { xobject: ObjRef { num: 6, generation: 0 }, image: false, size, at };
    for (poster, why) in [
        (picture([50.0, 100.0, 250.0, 300.0], (10.0, 10.0)), "starts left of the rectangle"),
        (picture([100.0, 100.0, 300.0, 301.0], (10.0, 10.0)), "ends above it"),
        (picture([100.0, 100.0, 300.0, 300.0], (0.0, 10.0)), "has no width"),
        (picture([100.0 - 2e-7, 150.0, 100.0 - 1e-7, 200.0], (10.0, 10.0)), "lies outside within the rounding slack"),
    ] {
        let mut doc = fixture();
        let new = NewModel3d {
            page: 0,
            rect: [100.0, 100.0, 300.0, 300.0],
            model: b"PRC rest",
            activation: Activation3d::default(),
            poster: Some(poster),
            contents: "",
        };
        assert!(matches!(add_3d_model(&mut doc, &new, &meta("m")), Err(AnnotError::Invalid(_))), "{why}");
        assert_eq!(list(&doc, 0).len(), 1, "{why}: nothing added");
    }
}

/// A poster box fitted to the rectangle in floating point ends an ulp outside it: a 7×3 picture
/// centred in [0 100 3.6 110.8] spans -2.2e-16 to 3.6000000000000005. It is taken, inside the
/// rectangle.
#[test]
fn a_3d_poster_fitted_to_its_rectangle_with_rounding_is_taken() {
    let mut doc = fixture();
    let at = [-2.220446049250313e-16, 104.62857142857143, 3.6000000000000005, 106.17142857142858];
    let poster = PosterPicture { xobject: ObjRef { num: 6, generation: 0 }, image: false, size: (7.0, 3.0), at };
    let new = NewModel3d {
        page: 0,
        rect: [0.0, 100.0, 3.6, 110.8],
        model: b"PRC rest",
        activation: Activation3d::default(),
        poster: Some(poster),
        contents: "",
    };
    assert_eq!(add_3d_model(&mut doc, &new, &meta("m")), Ok(1));
}

#[test]
fn indirect_typewriter_names_keep_outer_whitespace_in_summaries() {
    let mut doc = fixture();
    let intent = Object::Ref(doc.add(Object::name("FreeTextTypeWriter")));
    let page = page_refs(&doc).unwrap()[0];
    let mut entries = annots(&doc, page);
    let Object::Dict(annotation) = &mut entries[0] else { panic!("fixture annotation is inline") };
    annotation.set(b"Subtype".to_vec(), Object::name("FreeText"));
    annotation.set(b"IT".to_vec(), intent);
    let contents = " \nAda\n\nLovelace\n\n ";
    annotation.set(b"Contents".to_vec(), PdfString::text(contents));
    set_annots(&mut doc, page, entries).unwrap();
    let check = |doc: &Document| {
        let list = summaries(doc);
        let summary = list.iter().find(|a| a.page == 0 && a.index == 0).expect("direct Subtype is listed");
        assert_eq!(summary.contents.as_deref(), Some(contents), "indirect typewriter intent preserves outer whitespace");
        assert_eq!(summary.subtype, "FreeText");
        assert_eq!(summary.intent.as_deref(), Some("FreeTextTypeWriter"));
        assert!(summary.fill_sign, "indirect intent still identifies a Fill & Sign mark");
    };
    check(&doc);
    check(&reopen(&doc));
}

/// PDFBOX-2699: a `/C` written `[1 0 0 null]` (Table 166 asks for numbers) is still a four-entry
/// colour, CMYK with the null as 0 as PDFium, MuPDF and Poppler read it, so a shape without an
/// appearance is drawn instead of dropped. `/IC` is read the same way, and three numbers stay RGB.
/// The properties and the comment list report the colours as drawn.
#[test]
fn a_four_entry_colour_with_a_null_is_cmyk_with_zero() {
    let doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 40] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
        b"<< /Type /Annot /Subtype /Square /Rect [5 5 45 35] /C [1 0 0 null] /IC [0 0 1 null] >>",
        b"<< /Type /Annot /Subtype /Square /Rect [55 5 95 35] /C [1 0 0] >>",
    ]);
    let display = with_missing_appearances(&doc).expect("both squares get a fallback appearance");
    let squares = list(&display, 0);
    let cmyk = ap_content(&display, &squares[0]);
    assert!(cmyk.contains("0 1 1 RG") && cmyk.contains("1 1 0 rg"), "CMYK (1,0,0,0) stroke and (0,0,1,0) fill: {cmyk}");
    let rgb = ap_content(&display, &squares[1]);
    assert!(rgb.contains("1 0 0 RG"), "three numbers stay RGB: {rgb}");
    // The properties read the stroke and the fill as they are drawn.
    let p = props(&doc, 0, 0).expect("the square has properties");
    assert_eq!((p.color, p.fill), (Some([0.0, 1.0, 1.0]), Some([1.0, 1.0, 0.0])));
    assert_eq!(props(&doc, 0, 1).and_then(|p| p.color), Some([1.0, 0.0, 0.0]));
    // The comment list reports the same colours.
    let colors: Vec<Option<[f32; 3]>> = summaries(&doc).iter().map(|s| s.color).collect();
    assert_eq!(colors, [Some([0.0, 1.0, 1.0]), Some([1.0, 0.0, 0.0])]);
}

/// A `/C` given by reference is followed for the comment list, as three numbers were, and read
/// like a direct one: `[0 0 1 null]` is CMYK yellow, and a referenced grey level is grey.
#[test]
fn a_referenced_colour_is_listed_like_a_direct_one() {
    let doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 40] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
        b"<< /Type /Annot /Subtype /Square /Rect [5 5 45 35] /C 6 0 R >>",
        b"<< /Type /Annot /Subtype /Circle /Rect [55 5 95 35] /C 7 0 R >>",
        b"[0 0 1 null]",
        b"[0.5]",
    ]);
    let colors: Vec<Option<[f32; 3]>> = summaries(&doc).iter().map(|s| s.color).collect();
    assert_eq!(colors, [Some([1.0, 1.0, 0.0]), Some([0.5, 0.5, 0.5])]);
}

/// A reference among the four entries is not a non-number read as 0: `/C [1 0 0 6 0 R]` with
/// object 6 the number 1 is CMYK black (§7.3.10), so drawing it cyan would be wrong. The display
/// and the properties follow the reference (`draw::with_direct_values`) and draw and
/// read it black; the comment list, which follows only a `/C` given whole by reference, reads no
/// colour rather than cyan.
#[test]
fn a_four_entry_colour_with_a_reference_is_not_read_as_zero() {
    let doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 40] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
        b"<< /Type /Annot /Subtype /Square /Rect [5 5 45 35] /C [1 0 0 6 0 R] >>",
        b"<< /Type /Annot /Subtype /Square /Rect [55 5 95 35] /C [1 0 0] >>",
        b"1",
    ]);
    let display = with_missing_appearances(&doc).expect("both squares get a fallback appearance");
    let squares = list(&display, 0);
    let black = ap_content(&display, &squares[0]);
    assert!(black.contains("0 0 0 RG") && !black.contains("0 1 1 RG"), "CMYK black, no cyan stand-in: {black}");
    assert_eq!(props(&doc, 0, 0).and_then(|p| p.color), Some([0.0, 0.0, 0.0]));
    assert_ne!(summaries(&doc)[0].color, Some([0.0, 1.0, 1.0]), "nor cyan in the comment list");
}

/// A text markup annotation is drawn over its `/QuadPoints` (ISO 32000-2 Table 182), and an
/// appearance is mapped onto `/Rect` (§12.5.5). A highlight whose `/Rect` lies beside its text
/// (here written with its corners swapped, as a real file has it) got an appearance cut to that
/// `/Rect`, so nothing showed. The display copy's appearance and `/Rect` now take in the
/// quadrilaterals; a markup whose `/Rect` already holds them keeps it.
#[test]
fn text_markup_whose_quads_reach_past_its_rect_is_drawn_over_them() {
    let doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R 7 0 R 8 0 R 9 0 R 10 0 R] >>",
        b"<< /Type /Annot /Subtype /Highlight /Rect [335.1 370.83 324.6 471.78] /C [1 1 0] /QuadPoints [370 287 471 287 370 277 471 277] >>",
        b"<< /Type /Annot /Subtype /Underline /Rect [10 10 20 20] /C [0 0 1] /QuadPoints [100 120 200 120 100 100 200 100] >>",
        b"<< /Type /Annot /Subtype /Highlight /Rect [50 50 150 70] /C [1 1 0] /QuadPoints [60 68 140 68 60 52 140 52] >>",
        b"<< /Type /Annot /Subtype /StrikeOut /Rect [10 10 20 20] /C [1 0 0] /QuadPoints [100 220 200 220 100 200 200 200] >>",
        b"<< /Type /Annot /Subtype /Squiggly /Rect [10 10 20 20] /C [0 1 0] /QuadPoints [100 320 200 320 100 300 200 300] >>",
        b"<< /Type /Annot /Subtype /Highlight /Rect [10 10 20 20] /C [1 1 0] /QuadPoints [-1e30 20 1e30 20 -1e30 10 1e30 10] >>",
        b"<< /Type /Annot /Subtype /Highlight /Rect [-1e308 10 1e308 20] /C [1 1 0] /QuadPoints [100 120 200 120 100 100 200 100] >>",
    ]);
    let display = with_missing_appearances(&doc).expect("the markups need an appearance");
    let shown = list(&display, 0);
    let rect = |d: &Dict| nums_of(d, b"Rect");
    let bbox = |d: &Dict| {
        let n = d.get(b"AP").map(|a| display.resolve(a)).and_then(|a| a.as_dict().and_then(|a| a.reference(b"N"))).unwrap();
        let obj = display.get(n);
        let Object::Stream(s) = &*obj else { panic!("AP is not a stream") };
        nums_of(&s.dict, b"BBox")
    };
    assert_eq!(rect(&shown[0]), [324.6, 277.0, 471.0, 471.78], "the highlight's /Rect takes in its text");
    assert_eq!(bbox(&shown[0]), rect(&shown[0]));
    assert!(ap_content(&display, &shown[0]).contains("370 287 m 471 287 l 471 277 l 370 277 l h f"), "the quad is filled");
    // Half the underline's width (0.07 × 20 / 2) past the quad.
    assert_eq!(rect(&shown[1]), [10.0, 10.0, 200.7, 120.7], "an underline's too");
    assert_eq!(bbox(&shown[1]), rect(&shown[1]));
    assert_eq!(rect(&shown[2]), [50.0, 50.0, 150.0, 70.0], "a /Rect that holds the quads stays as it is");
    assert_eq!(rect(&shown[3]), [10.0, 10.0, 200.7, 220.7], "a strikeout's too");
    assert_eq!(bbox(&shown[3]), rect(&shown[3]));
    assert_eq!(rect(&shown[4]), [10.0, 10.0, 200.5, 320.5], "a squiggly's too");
    assert_eq!(bbox(&shown[4]), rect(&shown[4]));
    assert_eq!(rect(&shown[5]), [0.0, 10.0, 612.0, 20.0], "quads far past the page reach only as far as it shows");
    // A /Rect too wide for its width to be a number keeps its size: the on-page quad above it
    // doesn't make it taller.
    assert_eq!(rect(&shown[6]), [-1e308, 10.0, 1e308, 20.0], "an overflowing /Rect is not widened");
    assert_eq!(rect(&list(&doc, 0)[0]), [335.1, 370.83, 324.6, 471.78], "the source document is unchanged");
}

/// The box widens over the quads the markup is drawn over, read as every `/QuadPoints` reader
/// reads them (`pdfcraft_geom::quad_points::usable_quads`): an indirect array or entry is
/// followed, and a malformed quad or trailing partial one is skipped while the others still widen
/// the box, so what is drawn is not cut to `/Rect`.
#[test]
fn text_markup_reach_reads_quads_like_the_drawing() {
    let doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R] >>",
        b"<< /Type /Annot /Subtype /Highlight /Rect [10 10 20 20] /C [1 1 0] /QuadPoints 7 0 R >>",
        b"<< /Type /Annot /Subtype /Highlight /Rect [10 10 20 20] /C [1 1 0] /QuadPoints [100 120 /oops 120 100 100 200 100 100 220 200 220 100 200 200 200 1 2 3] >>",
        b"<< /Type /Annot /Subtype /Highlight /Rect [10 10 20 20] /C [1 1 0] /QuadPoints [100 120 200 120 100 100 8 0 R 100] >>",
        b"[100 120 200 120 100 100 200 100]",
        b"200",
    ]);
    let display = with_missing_appearances(&doc).expect("the markups need an appearance");
    let shown = list(&display, 0);
    let rect = |d: &Dict| nums_of(d, b"Rect");
    assert_eq!(rect(&shown[0]), [10.0, 10.0, 200.0, 120.0], "an indirect array widens the box");
    assert_eq!(ap_bbox(&display, &shown[0]), rect(&shown[0]));
    assert_eq!(rect(&shown[1]), [10.0, 10.0, 200.0, 220.0], "the usable quad still widens it");
    assert_eq!(ap_bbox(&display, &shown[1]), rect(&shown[1]));
    assert_eq!(rect(&shown[2]), [10.0, 10.0, 200.0, 120.0], "an indirect entry is followed");
}

/// `n` copies of the quadrilateral `[100 120 200 120 100 100 200 100]`, as `/QuadPoints`.
fn repeated_quads(n: usize) -> String {
    format!("[{}]", vec!["100 120 200 120 100 100 200 100"; n].join(" "))
}

/// The `/BBox` of `d`'s normal appearance in `doc`.
fn ap_bbox(doc: &Document, d: &Dict) -> Vec<f64> {
    let n = d.get(b"AP").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().and_then(|a| a.reference(b"N"))).expect("has /AP /N");
    let obj = doc.get(n);
    let Object::Stream(s) = &*obj else { panic!("AP is not a stream") };
    nums_of(&s.dict, b"BBox")
}

/// The widened box stops at the page's visible region (its crop box, ISO 32000-2 §14.11.2), and
/// quadrilaterals wholly outside it widen nothing. A markup with more than
/// `appearance::MAX_MARKUP_QUADS` quadrilaterals keeps its `/Rect`. An annotation with an
/// appearance of its own, and one that isn't text markup, keep theirs whatever `/QuadPoints` say.
#[test]
fn text_markup_reach_stops_at_the_page_and_the_quad_cap() {
    let cap = appearance::MAX_MARKUP_QUADS;
    let at_cap = format!("<< /Type /Annot /Subtype /Highlight /Rect [10 10 20 20] /C [1 1 0] /QuadPoints {} >>", repeated_quads(cap));
    let past_cap = format!("<< /Type /Annot /Subtype /Highlight /Rect [10 10 20 20] /C [1 1 0] /QuadPoints {} >>", repeated_quads(cap + 1));
    let doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
        b"<< /Type /Page /Parent 2 0 R /CropBox [0 0 300 400] /Annots [4 0 R 5 0 R 6 0 R 7 0 R 8 0 R 9 0 R] >>",
        b"<< /Type /Annot /Subtype /Highlight /Rect [10 10 20 20] /C [1 1 0] /QuadPoints [100 380 500 380 100 360 500 360] >>",
        b"<< /Type /Annot /Subtype /Highlight /Rect [10 10 20 20] /C [1 1 0] /QuadPoints [400 500 450 500 400 480 450 480] >>",
        at_cap.as_bytes(),
        past_cap.as_bytes(),
        b"<< /Type /Annot /Subtype /Highlight /Rect [10 10 20 20] /C [1 1 0] /QuadPoints [100 120 200 120 100 100 200 100] /AP << /N 10 0 R >> >>",
        b"<< /Type /Annot /Subtype /Square /Rect [10 10 20 20] /C [0 0 1] /QuadPoints [100 120 200 120 100 100 200 100] >>",
        b"<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Length 0 >>\nstream\n\nendstream",
    ]);
    let display = with_missing_appearances(&doc).expect("the markups need an appearance");
    let shown = list(&display, 0);
    let rect = |d: &Dict| nums_of(d, b"Rect");
    assert_eq!(rect(&shown[0]), [10.0, 10.0, 300.0, 380.0], "the box stops at the crop box");
    assert_eq!(ap_bbox(&display, &shown[0]), rect(&shown[0]));
    assert_eq!(rect(&shown[1]), [10.0, 10.0, 20.0, 20.0], "quads off the page widen nothing");
    assert_eq!(rect(&shown[2]), [10.0, 10.0, 200.0, 120.0], "as many quads as the cap widen the box");
    assert_eq!(rect(&shown[3]), [10.0, 10.0, 20.0, 20.0], "more quads than the cap keep the /Rect");
    assert_eq!(ap_bbox(&display, &shown[3]), rect(&shown[3]));
    assert_eq!(rect(&shown[4]), [10.0, 10.0, 20.0, 20.0], "a markup with an appearance of its own keeps its /Rect");
    assert_eq!(
        shown[4].get(b"AP").map(|a| display.resolve(a)).and_then(|a| a.as_dict().and_then(|a| a.reference(b"N"))),
        Some(ObjRef::new(10, 0)),
        "and its appearance"
    );
    assert_eq!(rect(&shown[5]), [10.0, 10.0, 20.0, 20.0], "a square is not text markup");
    assert_eq!(ap_bbox(&display, &shown[5]), rect(&shown[5]));
}

/// A restyle (the edit path, `set_appearance`) redraws a text markup over its quadrilaterals and
/// saves the `/Rect` its appearance covers, as far as the page shows, since the appearance is
/// mapped onto `/Rect` (ISO 32000-2 §12.5.5). A markup whose `/Rect` already holds its quads keeps
/// that `/Rect` as written, swapped corners and all.
#[test]
fn restyled_text_markup_saves_the_rect_its_appearance_covers() {
    let mut doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R] >>",
        b"<< /Type /Annot /Subtype /Highlight /Rect [335.1 370.83 324.6 471.78] /C [1 1 0] /QuadPoints [370 287 471 287 370 277 471 277] >>",
        b"<< /Type /Annot /Subtype /Highlight /Rect [150 70 50 50] /C [1 1 0] /QuadPoints [60 68 140 68 60 52 140 52] >>",
        b"<< /Type /Annot /Subtype /Underline /Rect [10 10 20 20] /C [0 0 1] /QuadPoints [500 120 700 120 500 100 700 100] >>",
    ]);
    for index in 0..3 {
        set_style(&mut doc, 0, index, StyleChange::new(Some([1.0, 0.0, 0.0]), None, None, None), &meta("")).unwrap();
    }
    let saved = reopen(&doc);
    let shown = list(&saved, 0);
    let rect = |d: &Dict| nums_of(d, b"Rect");
    assert_eq!(rect(&shown[0]), [324.6, 277.0, 471.0, 471.78], "the saved /Rect takes in the highlighted text");
    assert_eq!(ap_bbox(&saved, &shown[0]), rect(&shown[0]));
    assert!(ap_content(&saved, &shown[0]).contains("370 287 m 471 287 l 471 277 l 370 277 l h f"), "the quad is filled");
    assert_eq!(rect(&shown[1]), [150.0, 70.0, 50.0, 50.0], "a /Rect that holds the quads is kept as written");
    assert_eq!(rect(&shown[2]), [10.0, 10.0, 612.0, 120.7], "the saved /Rect stops at the page");
    assert_eq!(ap_bbox(&saved, &shown[2]), rect(&shown[2]));
}

/// A one-page document whose page lists `annots` (objects 4 on) and the display copy
/// `with_missing_appearances` draws for it.
fn displayed(annots: &[&[u8]]) -> (Document, Document) {
    let kids: Vec<String> = (0..annots.len()).map(|i| format!("{} 0 R", i + 4)).collect();
    let page = format!("<< /Type /Page /Parent 2 0 R /Annots [{}] >>", kids.join(" "));
    let mut objs: Vec<&[u8]> =
        vec![b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>", page.as_bytes()];
    objs.extend_from_slice(annots);
    let doc = document_of(&objs);
    let display = with_missing_appearances(&doc).expect("the annotations need an appearance");
    (doc, display)
}

/// Ink strokes (`/InkList`, ISO 32000-2 Table 185) outside an ink annotation's `/Rect` are drawn:
/// the display copy's `/Rect` and `/BBox` take in the strokes and half their width. Shaped like
/// the pencil marks of `issue13447.pdf`, whose `/Rect` has x and y swapped. An ink annotation
/// whose `/Rect` holds its points keeps it, even where its wide stroke reaches past it.
#[test]
fn ink_whose_strokes_reach_past_its_rect_is_drawn_through_them() {
    let (doc, display) = displayed(&[
        b"<< /Type /Annot /Subtype /Ink /Rect [109.25 260.01 21.25 313.51] /BS << /W 3 >> /C [1 0 0] /InkList [[260 502.75 300 537 330 520] [280 510]] >>",
        b"<< /Type /Annot /Subtype /Ink /Rect [0 0 100 100] /BS << /W 30 >> /C [1 0 0] /InkList [[10 10 90 90]] >>",
    ]);
    let shown = list(&display, 0);
    let rect = |d: &Dict| nums_of(d, b"Rect");
    // The smooth curve rises past its highest point (537) to 537.33; half the width (1.5) more.
    assert_rect_near(&rect(&shown[0]), [21.25, 260.01, 331.5, 538.825], "the ink's /Rect takes in its strokes");
    assert_eq!(ap_bbox(&display, &shown[0]), rect(&shown[0]));
    assert!(ap_content(&display, &shown[0]).contains("260 502.75 m"), "the stroke is drawn");
    assert_eq!(rect(&shown[1]), [0.0, 0.0, 100.0, 100.0], "a /Rect that holds the points stays as it is");
    assert_eq!(rect(&list(&doc, 0)[0]), [109.25, 260.01, 21.25, 313.51], "the source document is unchanged");
}

/// A polyline's `/Vertices` (Table 181) outside its `/Rect` are drawn, with its line endings
/// (`/LE`): the box takes in the arrow at the last vertex.
#[test]
fn polyline_whose_vertices_reach_past_its_rect_is_drawn_through_them() {
    let (_, display) = displayed(&[
        b"<< /Type /Annot /Subtype /PolyLine /Rect [667.5 250.69 567.75 344.44] /BS << /W 2 >> /C [0 0 1] /Vertices [250 44 350 94 285 138] >>",
        b"<< /Type /Annot /Subtype /PolyLine /Rect [10 10 20 20] /BS << /W 2 >> /C [0 0 1] /LE [/None /OpenArrow] /Vertices [100 100 200 100 200 200] >>",
    ]);
    let shown = list(&display, 0);
    let rect = |d: &Dict| nums_of(d, b"Rect");
    assert_eq!(rect(&shown[0]), [249.0, 43.0, 667.5, 344.44], "the polyline's /Rect takes in its vertices");
    assert_eq!(ap_bbox(&display, &shown[0]), rect(&shown[0]));
    assert!(ap_content(&display, &shown[0]).contains("250 44 m"), "the polyline is drawn");
    // Half the width (1) and the arrow (6 + 3 × 2) past the vertices.
    assert_eq!(rect(&shown[1]), [10.0, 10.0, 213.0, 213.0], "and its arrow");
}

/// A polygon's `/Vertices` outside its `/Rect` are drawn, its cloudy border (§12.5.4) included.
#[test]
fn polygon_whose_vertices_reach_past_its_rect_is_drawn_through_them() {
    let (_, display) = displayed(&[
        b"<< /Type /Annot /Subtype /Polygon /Rect [10 10 20 20] /BS << /W 1 >> /C [0 0 1] /IC [1 1 0] /Vertices [100 100 200 100 150 180] >>",
        b"<< /Type /Annot /Subtype /Polygon /Rect [10 10 20 20] /BS << /W 2 >> /C [0 0 1] /BE << /S /C /I 1 >> /Vertices [100 100 200 100 150 180] >>",
    ]);
    let shown = list(&display, 0);
    let rect = |d: &Dict| nums_of(d, b"Rect");
    assert_eq!(rect(&shown[0]), [10.0, 10.0, 200.5, 180.5], "the polygon's /Rect takes in its vertices");
    assert_eq!(ap_bbox(&display, &shown[0]), rect(&shown[0]));
    assert!(ap_content(&display, &shown[0]).contains("100 100 m"), "the polygon is drawn");
    // Half the width (1) and the cloud's reach (`cloud_reach`: 1.5 bumps of 6 + 1.5 × 2 at
    // intensity 1) past the vertices.
    assert_eq!(rect(&shown[1]), [10.0, 10.0, 214.5, 194.5], "and its cloudy border");
}

/// A line's `/L` (Table 178) outside its `/Rect` is drawn, with its line ending and its leader
/// lines. Shaped like the arrows of `issue13447.pdf`.
#[test]
fn line_whose_points_reach_past_its_rect_is_drawn_through_them() {
    let (_, display) = displayed(&[
        b"<< /Type /Annot /Subtype /Line /Rect [598.31 146.63 537 316.13] /BS << /W 6 >> /C [0 0 1] /LE [/None /ClosedArrow] /L [176 45 178 154] >>",
        b"<< /Type /Annot /Subtype /Line /Rect [10 10 20 20] /BS << /W 2 >> /C [0 0 1] /L [100 100 200 100] /LL 20 /LLE 5 >>",
        b"<< /Type /Annot /Subtype /Line /Rect [90 90 210 110] /BS << /W 2 >> /C [0 0 1] /LE [/OpenArrow /OpenArrow] /L [100 100 200 100] >>",
    ]);
    let shown = list(&display, 0);
    let rect = |d: &Dict| nums_of(d, b"Rect");
    // Half the width (3) and the arrow (6 + 3 × 6) past `/L`.
    assert_eq!(rect(&shown[0]), [149.0, 18.0, 598.31, 316.13], "the line's /Rect takes in /L and its arrow");
    assert_eq!(ap_bbox(&display, &shown[0]), rect(&shown[0]));
    assert!(ap_content(&display, &shown[0]).contains("176 45 m"), "the line is drawn");
    // The leaders rise 20 + 5 above `/L`; half the width (1) past them.
    assert_eq!(rect(&shown[1]), [10.0, 10.0, 201.0, 126.0], "and its leader lines");
    assert_eq!(rect(&shown[2]), [90.0, 90.0, 210.0, 110.0], "a /Rect that holds /L stays as it is");
}

/// Points past the caps (`appearance::MAX_REACH_POINTS`) leave `/Rect` as it is, as do points
/// that aren't numbers; a restyle (the edit path) saves the widened `/Rect` of an ink annotation.
#[test]
fn path_reach_stops_at_the_point_cap_and_is_saved_on_restyle() {
    let cap = appearance::MAX_REACH_POINTS;
    let stroke = |n: usize| format!("[{}]", vec!["100 120"; n].join(" "));
    let at_cap = format!("<< /Type /Annot /Subtype /Ink /Rect [10 10 20 20] /BS << /W 0 >> /C [1 0 0] /InkList [{} [200 200]] >>", stroke(cap - 1));
    let past_cap = format!("<< /Type /Annot /Subtype /Ink /Rect [10 10 20 20] /BS << /W 0 >> /C [1 0 0] /InkList [{} [200 200]] >>", stroke(cap));
    let (_, display) = displayed(&[at_cap.as_bytes(), past_cap.as_bytes()]);
    let shown = list(&display, 0);
    let rect = |d: &Dict| nums_of(d, b"Rect");
    assert_eq!(rect(&shown[0]), [10.0, 10.0, 200.0, 200.0], "as many points as the cap widen the box");
    assert_eq!(rect(&shown[1]), [10.0, 10.0, 20.0, 20.0], "more points than the cap keep the /Rect");

    let mut doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        b"<< /Type /Annot /Subtype /Ink /Rect [109.25 260.01 21.25 313.51] /BS << /W 3 >> /C [1 0 0] /InkList [[260 502.75 300 537 330 520]] >>",
    ]);
    set_style(&mut doc, 0, 0, StyleChange::new(Some([0.0, 0.0, 1.0]), None, None, None), &meta("")).unwrap();
    let saved = reopen(&doc);
    let shown = list(&saved, 0);
    assert_rect_near(&rect(&shown[0]), [21.25, 260.01, 331.5, 538.825], "the saved /Rect takes in the strokes");
    assert_eq!(ap_bbox(&saved, &shown[0]), rect(&shown[0]));
}

/// A squiggle's peaks stand 1.5 above a quad's bottom edge however low the quad is, so over a quad
/// 1 high they reach past it: the widened box takes them in, with half the line's width.
#[test]
fn squiggly_peaks_over_a_low_quad_are_inside_the_widened_box() {
    let (_, display) =
        displayed(&[b"<< /Type /Annot /Subtype /Squiggly /Rect [10 10 20 20] /C [0 1 0] /QuadPoints [100 301 200 301 100 300 200 300] >>"]);
    let shown = list(&display, 0);
    // Peak 300 + 1.5, half the 0.5 line width: 301.75; the sides reach 0.75 past the quad.
    assert_eq!(nums_of(&shown[0], b"Rect"), [10.0, 10.0, 200.75, 301.75]);
    assert_eq!(ap_bbox(&display, &shown[0]), nums_of(&shown[0], b"Rect"));
}

/// A line's caption (`/Cap`, ISO 32000-2 Table 178) is drawn beside `/L`: the widened box takes
/// in the caption's text as well as the line.
#[test]
fn a_line_caption_is_inside_the_widened_box() {
    let (_, display) = displayed(&[
        b"<< /Type /Annot /Subtype /Line /Rect [10 10 20 20] /BS << /W 2 >> /C [0 0 1] /L [100 100 200 100] /Cap true /CP /Top /Contents (Caption) >>",
    ]);
    let shown = list(&display, 0);
    let rect = nums_of(&shown[0], b"Rect");
    // The caption sits on top of the line: its 10-point text rises well past the stroke's 101.
    assert!(rect[3] > 108.0, "{rect:?}");
    assert_eq!(ap_bbox(&display, &shown[0]), rect);
}

/// A line just above the crop box whose wide stroke reaches down onto the page is drawn as far as
/// the page shows, not cut to its `/Rect`.
#[test]
fn a_stroke_that_reaches_onto_the_page_from_points_off_it_widens_the_box() {
    let doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
        b"<< /Type /Page /Parent 2 0 R /CropBox [0 0 300 400] /Annots [4 0 R] >>",
        b"<< /Type /Annot /Subtype /Line /Rect [10 10 20 20] /BS << /W 6 >> /C [0 0 1] /L [100 401 200 401] >>",
    ]);
    let display = with_missing_appearances(&doc).expect("the line needs an appearance");
    let shown = list(&display, 0);
    // The stroke spans 398 to 404 across and 97 to 203 along; the page shows up to 400.
    assert_eq!(nums_of(&shown[0], b"Rect"), [10.0, 10.0, 203.0, 400.0]);
}

/// A cached display appearance was drawn for its page's crop box: once the crop box grows, the
/// next refresh draws the annotation again for the larger region.
#[test]
fn a_cached_appearance_is_redrawn_for_a_changed_crop_box() {
    let mut doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
        b"<< /Type /Page /Parent 2 0 R /CropBox [0 0 300 400] /Annots [4 0 R] >>",
        b"<< /Type /Annot /Subtype /Highlight /Rect [10 10 20 20] /C [1 1 0] /QuadPoints [100 380 500 380 100 360 500 360] >>",
    ]);
    let mut cache = AppearanceCache::default();
    let display = with_missing_appearances_cached(&doc, &mut cache).expect("drawn");
    assert_eq!(nums_of(&list(&display, 0)[0], b"Rect"), [10.0, 10.0, 300.0, 380.0]);
    doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"CropBox".to_vec(), Object::Array([0, 0, 612, 792].map(Object::Int).to_vec()))).unwrap();
    let display = with_missing_appearances_cached(&doc, &mut cache).expect("drawn");
    let shown = list(&display, 0);
    assert_eq!(nums_of(&shown[0], b"Rect"), [10.0, 10.0, 500.0, 380.0], "drawn for the larger crop box");
    assert_eq!(ap_bbox(&display, &shown[0]), nums_of(&shown[0], b"Rect"));
}

/// Redrawing a comment drawn through its points finds its page through `/P` when that page
/// lists it, reading that one page's `/Annots` rather than every page's: here the last of 300.
#[test]
fn restyling_a_comment_drawn_through_its_points_reads_only_its_page() {
    let pages = 300;
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 3 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {pages} /MediaBox [0 0 612 792] >>", kids.join(" ")));
    for i in 0..pages {
        let (page, annot) = (3 + 2 * i, 4 + 2 * i);
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Annots [{annot} 0 R] >>"));
        objs.push(format!(
            "<< /Type /Annot /Subtype /Highlight /P {page} 0 R /Rect [10 10 20 20] /C [1 1 0] /QuadPoints [100 120 200 120 100 100 200 100] >>"
        ));
    }
    let bytes: Vec<&[u8]> = objs.iter().map(|o| o.as_bytes()).collect();
    let mut doc = document_of(&bytes);
    let last = pages - 1;
    ANNOTS_ENTRIES_COPIED.with(|c| c.set(0));
    set_style(&mut doc, last, 0, StyleChange::new(Some([1.0, 0.0, 0.0]), None, None, None), &meta("")).unwrap();
    let copied = ANNOTS_ENTRIES_COPIED.with(std::cell::Cell::get);
    assert!(copied < 20, "{copied} annotation entries copied for one restyle");
    assert_eq!(nums_of(&list(&doc, last)[0], b"Rect"), [10.0, 10.0, 200.0, 120.0], "drawn for its page");
}

/// `got` is `want` to within 0.001 in every coordinate.
fn assert_rect_near(got: &[f64], want: [f64; 4], what: &str) {
    assert!(got.len() == 4 && got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-3), "{what}: {got:?}");
}

fn nums_of(d: &Dict, key: &[u8]) -> Vec<f64> {
    d.get(key).and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|o| o.as_f64()).collect()).unwrap_or_default()
}

/// The `y` of every `1 0 0 1 x y Tm` in `content`: where each piece of text sits.
fn baselines(content: &str) -> Vec<f64> {
    content.split(" Tm").filter_map(|s| s.rsplit(' ').next()?.parse().ok()).collect()
}

/// Thai typed into Fill & Sign text or a text box is shaped and drawn from the Thai face, as Add
/// text draws it: Type 3 glyphs (their streams objects of their own) with a ToUnicode map, never
/// `?`. The box grows to hold the ink of its stacked marks and descenders, which reach past the
/// 1.2 em lines boxes are sized by. A build without a Thai face refuses the text, saying why.
#[test]
fn thai_in_fill_sign_text_and_text_boxes_is_drawn_from_the_thai_face() {
    let mut doc = fixture();
    // Each line has the face's highest stack (น้ำ) and deepest descender (ฎู).
    let thai = "น้ำฎู บุญ\nปั้นน้ำฎู";
    let item = |shape: Shape| NewAnnotation { page: 0, style: Style::default_for(&shape), shape, contents: thai.into(), author: "Ada".into() };
    let shapes = [
        Shape::Typewriter { rect: [100.0, 700.0, 300.0, 716.0], font_size: 24.0 },
        Shape::TextBox { rect: [100.0, 400.0, 400.0, 430.0], font_size: 30.0 },
    ];
    if pdfcraft_fonts::document_thai_font().is_none() {
        for shape in shapes {
            let err = add_annotation(&mut doc, &item(shape), &meta("t")).unwrap_err();
            assert!(matches!(&err, AnnotError::Undrawable(m) if m.contains("Thai") && m.contains("CRAFT_FONTS_DIR")), "{err:?}");
        }
        assert_eq!(list(&doc, 0).len(), 1, "nothing was added");
        let t = add_annotation(&mut doc, &new(0, Shape::TextBox { rect: [100.0, 600.0, 300.0, 650.0], font_size: 12.0 }), &meta("b")).unwrap();
        assert!(matches!(set_contents(&mut doc, 0, t, thai, &meta("")), Err(AnnotError::Undrawable(_))));
        assert_eq!(text(&list(&doc, 0)[t], b"Contents"), "hello");
        return;
    }
    let reach = pdfcraft_fonts::shaped_text::thai_reach;
    let (lo, hi) = (reach("ฎู").unwrap()[0], reach("น้ำ").unwrap()[1]);
    for (shape, size) in shapes.into_iter().zip([24.0, 30.0]) {
        let t = add_annotation(&mut doc, &item(shape), &meta("t")).unwrap();
        let doc = reopen(&doc);
        let d = &list(&doc, 0)[t];
        assert_eq!((text(d, b"Contents").as_str(), pdfcraft_fonts::undrawn_mark(d)), (thai, None));
        let content = ap_content(&doc, d);
        assert!(!content.contains('?') && content.contains("/PCTh1 "), "{content}");
        // The glyphs and the text they stand for are objects of the saved file.
        let n = d.get(b"AP").and_then(|a| a.as_dict()).and_then(|a| a.reference(b"N")).unwrap();
        let ap = doc.get(n);
        let fonts = ap.as_dict().and_then(|s| s.get(b"Resources")).and_then(|r| r.as_dict()).and_then(|r| r.get(b"Font")).and_then(|f| f.as_dict());
        let font = fonts.and_then(|f| f.get(b"PCTh1")).and_then(|f| f.as_dict()).unwrap().clone();
        assert_eq!(font.name(b"Subtype"), Some(&b"Type3"[..]));
        let cmap = doc.get(font.reference(b"ToUnicode").unwrap());
        let Object::Stream(cmap) = &*cmap else { panic!("ToUnicode is a stream object") };
        let cmap = String::from_utf8_lossy(&cmap.decoded().unwrap()).into_owned();
        assert!(cmap.contains("<0E190E490E33>"), "น้ำ copies as typed: {cmap}");
        let procs = font.get(b"CharProcs").and_then(|p| p.as_dict()).unwrap();
        assert!(procs.iter().all(|(_, g)| matches!(g, Object::Ref(_))), "glyph programs are stream objects");
        // Every line's ink lies inside the box: the appearance's BBox and its clip.
        let (r, ys) = (rect(d), baselines(&content));
        let (top, bottom) = (ys.iter().copied().fold(f64::MIN, f64::max), ys.iter().copied().fold(f64::MAX, f64::min));
        assert!(top + hi * size <= r[3] + 1e-3 && bottom + lo * size >= r[1] - 1e-3, "{ys:?} at {size} in {r:?}");
    }
    // New text refits the box to its ink too: the stack on its first line, the descender on its last.
    set_contents(&mut doc, 0, 1, "น้ำ\nฎู", &meta("")).unwrap();
    let d = &list(&doc, 0)[1];
    let (r, ys) = (rect(d), baselines(&ap_content(&doc, d)));
    assert!(ys[0] + hi * 24.0 <= r[3] + 1e-3 && ys[ys.len() - 1] + lo * 24.0 >= r[1] - 1e-3, "{ys:?} in {r:?}");
}

const QUAD: [f64; 8] = [20.0, 110.0, 40.0, 110.0, 20.0, 95.0, 40.0, 95.0];

fn reals(v: &[f64]) -> Vec<Object> {
    v.iter().map(|x| Object::Real(*x)).collect()
}

/// The appearance a `subtype` with `/QuadPoints` `quads` draws, as text.
fn quad_appearance(subtype: &str, quads: Vec<Object>) -> Option<String> {
    quad_appearance_with(&bare_doc(), subtype, Object::Array(quads))
}

fn quad_appearance_with(doc: &Document, subtype: &str, quad_points: Object) -> Option<String> {
    let mut d = Dict::new();
    d.set(b"Subtype".to_vec(), Object::name(subtype));
    d.set(b"Rect".to_vec(), Object::Array([20, 95, 40, 110].map(Object::Int).to_vec()));
    d.set(b"C".to_vec(), Object::Array(vec![Object::Int(1), Object::Int(0), Object::Int(0)]));
    d.set(b"QuadPoints".to_vec(), quad_points);
    appearance::build(doc, &d).map(|s| String::from_utf8_lossy(&s.decoded().unwrap()).into_owned())
}

/// Appearance drawing reads `/QuadPoints` by the rule the other readers do, indirect objects
/// included: entries by reference, and the array itself by reference, draw the same quads as
/// the direct values.
#[test]
fn appearances_draw_indirect_quads_like_direct_ones() {
    for subtype in ["Redact", "Highlight"] {
        let clean = quad_appearance(subtype, reals(&QUAD)).unwrap();
        let mut doc = bare_doc();
        let by_ref: Vec<Object> = QUAD.iter().map(|v| Object::Ref(doc.add(Object::Real(*v)))).collect();
        assert_eq!(quad_appearance_with(&doc, subtype, Object::Array(by_ref)).as_deref(), Some(clean.as_str()), "{subtype}: entries by reference");
        let array_ref = Object::Ref(doc.add(Object::Array(reals(&QUAD))));
        assert_eq!(quad_appearance_with(&doc, subtype, array_ref).as_deref(), Some(clean.as_str()), "{subtype}: the array by reference");
    }
}

/// Redaction outlines and text markup draw the quads redaction apply would use, no more: a NaN
/// quad wrote `NaN` into the stream, and one non-number or a trailing partial quad left the other,
/// usable quads undrawn.
#[test]
fn appearances_draw_only_usable_quads() {
    let mut one_nan = QUAD;
    one_nan[3] = f64::NAN;
    for subtype in ["Redact", "Highlight"] {
        let clean = quad_appearance(subtype, reals(&QUAD)).unwrap();
        assert!(!clean.contains("NaN"), "{clean}");
        let mut name_first = reals(&[QUAD, QUAD].concat());
        name_first[2] = Object::name("oops");
        let same =
            [reals(&[one_nan, QUAD].concat()), name_first, reals(&[&QUAD[..], &QUAD[..4]].concat()), reals(&[QUAD, [f64::INFINITY; 8]].concat())];
        for (i, quads) in same.into_iter().enumerate() {
            assert_eq!(quad_appearance(subtype, quads).as_deref(), Some(clean.as_str()), "{subtype} case {i}");
        }
        // No usable quad draws nothing, as with no /QuadPoints.
        assert_eq!(quad_appearance(subtype, reals(&[f64::NAN; 8])), None, "{subtype}");
    }
}

/// The comment list reads `/QuadPoints` by the rule redaction apply does: unusable quads are left
/// out, not read as zeros, and an indirect number is followed.
#[test]
fn summaries_list_only_usable_quads() {
    let mut doc = fixture();
    let forty = Object::Ref(doc.add(Object::Int(40)));
    let page = page_refs(&doc).unwrap()[0];
    let mut entries = annots(&doc, page);
    let Object::Dict(annotation) = &mut entries[0] else { panic!("fixture annotation is inline") };
    // A non-number in the first quad, an indirect 40 in the second, NaN in the third, and four
    // entries of a fourth.
    let mut quads = reals(&[QUAD, QUAD, QUAD, QUAD].concat());
    quads[1] = Object::name("oops");
    quads[10] = forty;
    quads[17] = Object::Real(f64::NAN);
    quads.truncate(28);
    annotation.set(b"Subtype".to_vec(), Object::name("Redact"));
    annotation.set(b"QuadPoints".to_vec(), Object::Array(quads));
    set_annots(&mut doc, page, entries).unwrap();
    let summary = summaries(&doc).into_iter().find(|a| a.page == 0 && a.index == 0).unwrap();
    assert_eq!(summary.quads, vec![QUAD.map(|v| v as f32)]);
}

/// A page as iOS Markup saves a signature: its private copy on the page and on the stamp, which
/// also carries a normal appearance. A plain note follows the stamp, at index 1.
fn apple_markup_page() -> Document {
    document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R 7 0 R] /AAPL:PPK 5 0 R /AAPL:PPKHash 5 0 R >>",
        b"<< /Type /Annot /Subtype /Stamp /Rect [100 100 200 150] /F 644 /AP << /N 6 0 R >> /AAPL:AKExtras << /PPKType (draw) >> >>",
        b"<< /Length 0 >>
stream

endstream",
        b"<< /Type /XObject /Subtype /Form /BBox [0 0 100 50] /Length 22 >>
stream
0 0 1 RG 1 1 m 9 9 l S
endstream",
        b"<< /Type /Annot /Subtype /Text /Rect [400 400 420 420] /Contents (note) >>",
    ])
}

/// Apple's copy is gone from the page and from every annotation left on it, and the drop is
/// in the repair log (fidelity: never silent).
fn assert_apple_copy_dropped(doc: &Document) {
    assert!(doc.repair_log().iter().any(|l| l.contains("Apple Markup")), "{:?}", doc.repair_log());
    let doc = reopen(doc);
    let page = doc.get(page_refs(&doc).unwrap()[0]);
    let page = page.as_dict().unwrap();
    assert!(!page.contains(b"AAPL:PPK") && !page.contains(b"AAPL:PPKHash"));
    let all = list(&doc, 0);
    assert_eq!(all[0].name(b"Subtype"), Some(&b"Stamp"[..]));
    assert!(all.iter().all(|d| !d.contains(b"AAPL:AKExtras")));
}

#[test]
fn deleting_from_an_apple_markup_page_drops_apples_stale_copy() {
    let mut doc = apple_markup_page();
    delete_annotation(&mut doc, 0, 1).unwrap();
    assert_apple_copy_dropped(&doc);
}

#[test]
fn replying_on_an_apple_markup_page_drops_apples_stale_copy() {
    let mut doc = apple_markup_page();
    add_reply(&mut doc, 0, 1, "Agreed", "Bob", &meta("r")).unwrap();
    assert_apple_copy_dropped(&doc);
}

#[test]
fn marking_on_an_apple_markup_page_drops_apples_stale_copy() {
    let mut doc = apple_markup_page();
    set_marked(&mut doc, 0, 1, true, "Bob", &meta("m")).unwrap();
    assert_apple_copy_dropped(&doc);
}

#[test]
fn adding_to_an_apple_markup_page_drops_apples_stale_copy() {
    let mut doc = apple_markup_page();
    add_annotation(&mut doc, &new(0, Shape::Note { at: [300.0, 300.0], icon: NoteIcon::Comment }), &meta("n")).unwrap();
    assert_apple_copy_dropped(&doc);
    // The Markup stamp itself stays, with its flags and its appearance, byte for byte.
    let doc = reopen(&doc);
    let all = list(&doc, 0);
    assert_eq!(all[0].get(b"F").and_then(Object::as_int), Some(644));
    assert_eq!(ap_content(&doc, &all[0]), "0 0 1 RG 1 1 m 9 9 l S");
}

#[test]
fn a_refused_edit_on_an_apple_markup_page_drops_nothing() {
    // An inline, locked Markup stamp: restyling it is refused, so Apple's copy must stay put
    // (promoting the inline dictionary to an object is not a change to the markup).
    let mut doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [<< /Type /Annot /Subtype /Stamp /Rect [100 100 200 150] /F 644 /AAPL:AKExtras << /PPKType (draw) >> >>] /AAPL:PPK 4 0 R /AAPL:PPKHash 4 0 R >>",
        b"<< /Length 0 >>
stream

endstream",
    ]);
    assert_eq!(
        set_style(&mut doc, 0, 0, StyleChange { color: Some([1.0, 0.0, 0.0]), opacity: None, width: None, endings: None }, &meta("")),
        Err(AnnotError::Invalid("the comment is locked".into()))
    );
    let page = doc.get(page_refs(&doc).unwrap()[0]);
    let page = page.as_dict().unwrap();
    assert!(page.contains(b"AAPL:PPK") && page.contains(b"AAPL:PPKHash"));
    assert!(list(&doc, 0)[0].contains(b"AAPL:AKExtras"));
    assert!(doc.repair_log().is_empty(), "{:?}", doc.repair_log());
}
