use std::sync::Arc;

use pdfcraft_cos::{Document, ObjRef, Object, SaveOptions, write_incremental};

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

#[test]
fn appearance_signature_size_query_bounds_subject_before_decoding() {
    let mut doc = fixture();
    let index =
        add_annotation(&mut doc, &new(0, Shape::Signature { strokes: vec![vec![[10.0, 20.0], [50.0, 40.0]]] }), &meta("signature-size")).unwrap();
    let (_, r) = annot_ref(&mut doc, 0, index).unwrap();
    let original = appearance_signature_rect(&doc, 0, index).unwrap();
    assert!(original.is_some());
    for subject in [
        PdfString::text(" Signature "),
        PdfString::literal([vec![0xfe, 0xff], "Signature".encode_utf16().flat_map(u16::to_be_bytes).collect()].concat()),
    ] {
        doc.update_dict(r, |d| d.set(b"Subj".to_vec(), Object::String(subject))).unwrap();
        assert_eq!(appearance_signature_rect(&doc, 0, index).unwrap(), original);
    }
    // The old general text reader would trim these spaces and accept Signature.
    // This query refuses the raw input before allocating its decoded text.
    let padded = PdfString::text(&format!("{}Signature", " ".repeat(129)));
    doc.update_dict(r, |d| d.set(b"Subj".to_vec(), Object::String(padded))).unwrap();
    assert_eq!(appearance_signature_rect(&doc, 0, index).unwrap(), None);
}

fn new(page: usize, shape: Shape) -> NewAnnotation {
    NewAnnotation {
        page,
        shape,
        style: Style { color: [0.0, 0.4, 1.0], opacity: 1.0, width: 2.0, fill: None },
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
    // The leader line keeps its start and knee; the attach point follows the re-fitted box.
    assert_eq!(&cl[..4], &[200.0, 520.0, 260.0, 560.0]);
    let want = expected_box(612.0, 200.0, long, 12.0, 2.0);
    assert_eq!((cl[4], cl[5]), callout_attach([300.0, want[1], 500.0, 612.0], [260.0, 560.0]).into());
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
    use crate::links::{self, Highlight, LinkAction, LinkStyle};
    let mut doc = fixture();
    let before = links::list(&doc).len();
    let i = links::add(&mut doc, 0, [10.0, 10.0, 110.0, 30.0], &LinkAction::Uri("https://example.org".into()), &LinkStyle::default()).unwrap();
    let style = LinkStyle { visible: true, color: [1.0, 0.0, 0.0], width: 2.0, dashed: true, underline: false, highlight: Highlight::Outline };
    links::add(&mut doc, 1, [10.0, 10.0, 60.0, 30.0], &LinkAction::Page(0), &style).unwrap();
    let doc = reopen(&doc);
    let all = links::list(&doc);
    assert_eq!(all.len(), before + 2);
    let web = all.iter().find(|l| l.page == 0 && l.index == i).unwrap();
    assert_eq!(
        (web.action.clone(), web.style.visible, web.style.highlight),
        (LinkAction::Uri("https://example.org".into()), false, Highlight::Invert)
    );
    let page = all.iter().find(|l| l.page == 1 && l.action == LinkAction::Page(0)).unwrap().clone();
    assert_eq!(page.style, style);
    let mut doc = doc;
    links::set(&mut doc, 1, page.index, Some([0.0, 0.0, 50.0, 20.0]), Some(&LinkAction::Uri("mailto:ada@example.org".into())), None).unwrap();
    let changed = links::list(&doc).into_iter().find(|l| l.page == 1 && l.index == page.index).unwrap();
    assert_eq!((changed.rect, changed.action), ([0.0, 0.0, 50.0, 20.0], LinkAction::Uri("mailto:ada@example.org".into())));
    links::delete(&mut doc, 1, page.index).unwrap();
    assert!(links::delete(&mut doc, 0, 999).is_err(), "no such annotation");
    let n = links::remove_all(&mut doc, None).unwrap();
    assert!(n >= 1 && links::list(&doc).is_empty());
    assert!(links::add(&mut doc, 0, [0.0, 0.0, 1.0, 1.0], &LinkAction::Page(0), &LinkStyle::default()).is_err(), "too small");
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
    let red = Style { color: [0.89, 0.13, 0.13], opacity: 1.0, width: 1.0, fill: None };
    let blue = Style { color: [0.0, 0.47, 0.84], opacity: 1.0, width: 1.0, fill: None };
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
    let strike = &a[i];
    assert_eq!((strike.name(b"Subtype"), strike.name(b"IT")), (Some(&b"StrikeOut"[..]), Some(&b"StrikeOutTextEdit"[..])));
    let caret = a.iter().find(|d| d.name(b"Subtype") == Some(b"Caret")).unwrap();
    assert_eq!((text(caret, b"Contents").as_str(), caret.name(b"RT")), ("new words", Some(&b"Group"[..])));
    assert!(caret.reference(b"IRT").is_some());
    // #2666: the pair needs its own `/NM` each. XFDF names a reply by the `/NM` of what it
    // replies to, so sharing one id writes `inreplyto` equal to the caret's own `name` and the
    // pair does not survive an export and import.
    assert_eq!(text(caret, b"NM").as_str(), "r-caret");
    assert_eq!(text(strike, b"NM").as_str(), "r");
    assert_ne!(text(caret, b"NM"), text(strike, b"NM"), "the caret must not carry the strikeout's name");
    let irt = caret.reference(b"IRT").unwrap();
    assert_eq!(text(doc.get(irt).as_dict().unwrap(), b"NM").as_str(), "r", "the caret replies to the strikeout");
    let r = rect(caret);
    assert!(r[0] < 200.0 && r[2] > 200.0 && r[1] < 706.0, "at the end of the line, below its baseline: {r:?}");
    // Deleting the strikeout takes the caret with it (the group).
    let mut doc = doc;
    delete_annotation(&mut doc, 0, i).unwrap();
    assert!(!list(&doc, 0).iter().any(|d| d.name(b"Subtype") == Some(b"Caret")));
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
        assert!(appearance::build(&annotation).is_some(), "malformed optional /IC must not hide a valid {name} stroke");
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
    assert!(appearance::build(&stamp_dict("/Name /Draft")).is_none());
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

    assert!(appearance::build(&line_dict([f64::NAN, 0.0, 10.0, 0.0], "OpenArrow")).is_none(), "a non-finite endpoint is rejected");
    assert!(appearance::build(&line_dict([0.0, 0.0, 10.0, 0.0], "Foo")).is_none(), "an unknown ending is not replaced");
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
    assert_eq!(props(&doc, 0, index).unwrap().width, Some(0.0));
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
    crate::appearance::build(&d).expect("a note is drawn").decoded().unwrap()
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
    let stream = appearance::build(&d).expect("drawn");
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
        let c = String::from_utf8_lossy(&appearance::build(&d).unwrap().decoded().unwrap()).into_owned();
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
        let s = appearance::build(&d).unwrap();
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
    assert!(appearance::build(&d).is_some());
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
    let s = crate::appearance::build(d).expect("the line is drawn");
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
    let s = crate::appearance::build(&d).unwrap().decoded().unwrap();
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

fn sized_signature_input(strokes: Vec<Vec<[f64; 2]>>, width: f64) -> NewAnnotation {
    let shape = Shape::Signature { strokes };
    NewAnnotation {
        page: 0,
        style: Style { color: [0.0, 0.0, 0.0], opacity: 1.0, width, fill: None },
        shape,
        contents: String::new(),
        author: "Ada".into(),
    }
}

fn signature_width(d: &Dict) -> f64 {
    d.get(b"BS").and_then(Object::as_dict).and_then(|bs| bs.get(b"W")).and_then(Object::as_f64).expect("signature /BS /W")
}

fn signature_strokes(d: &Dict) -> Vec<Vec<[f64; 2]>> {
    d.get(b"InkList")
        .and_then(Object::as_array)
        .expect("signature /InkList")
        .iter()
        .map(|stroke| {
            let values = stroke.as_array().expect("stroke array");
            values.as_chunks::<2>().0.iter().map(|pair| [pair[0].as_f64().expect("x"), pair[1].as_f64().expect("y")]).collect()
        })
        .collect()
}

#[test]
fn sized_signature_fit_boundary_is_physical_and_strict_only_on_nonzero_axes() {
    let horizontal = vec![vec![[0.0, 0.0], [2.0, 0.0]]];
    assert!(validate_signature_size(&horizontal, 1.5, [3.499, 3.5]).is_err(), "just below 1.5 points plus 1 point of padding on each side");
    assert!(
        validate_signature_size(&horizontal, 1.5, [3.5, 3.5]).is_err(),
        "the nondegenerate horizontal axis needs positive interior after padding"
    );
    assert!(
        validate_signature_size(&horizontal, 1.5, [3.501, 3.5]).is_ok(),
        "just above the physical minimum fits while the degenerate vertical axis is exact"
    );
    assert!(validate_signature_size(&horizontal, 1.5, [3.501, 3.499]).is_err(), "a degenerate axis still needs both line-width margins");
    assert!(
        validate_signature_size(&[vec![[0.0, 0.0], [0.0, 0.0]]], 1.5, [3.5, 3.5]).is_ok(),
        "a positive-width dot fits at the exact two-axis boundary"
    );

    let mut doc = fixture();
    let page = page_refs(&doc).expect("page tree")[0];
    doc.update_dict(page, |d| d.set(b"UserUnit".to_vec(), Object::Int(2))).expect("sets page UserUnit");
    let input = sized_signature_input(horizontal, 1.5);
    let i = add_sized_signature(&mut doc, &input, [100.0, 100.0, 101.7505, 101.75], &meta("just-above-fit"))
        .expect("just-above 3.5 by 3.5 physical-point fit adds");
    let d = list(&doc, 0)[i].clone();
    assert_eq!(rect(&d), [100.0, 100.0, 101.7505, 101.75]);
    assert_eq!(signature_width(&d) * 2.0, 1.5, "raw border width is converted from physical points through /UserUnit");
    assert!(ap_content(&doc, &d).contains(" w"), "a real appearance was generated");

    let reopened = reopen(&doc);
    let saved = &list(&reopened, 0)[i];
    assert_eq!(rect(saved), [100.0, 100.0, 101.7505, 101.75]);
    assert_eq!(signature_width(saved) * 2.0, 1.5);
    assert!(!ap_content(&reopened, saved).is_empty());
}

#[test]
fn signature_fit_uses_the_spline_control_hull_not_only_input_vertices() {
    let strokes = vec![vec![[0.0, 0.0], [20.0, 0.0], [21.0, 100.0], [22.0, 0.0]]];
    // The generated Catmull-Rom controls extend to x=23.5 and y=-100/6.
    let mut raw_doc = fixture();
    let raw_i = add_annotation(&mut raw_doc, &sized_signature_input(strokes.clone(), 1.5), &meta("spline-bounds")).expect("adds signature");
    let raw = rect(&list(&raw_doc, 0)[raw_i]);
    let epsilon = 1e-8;
    assert!((raw[0] - -1.75).abs() < epsilon, "left padding includes the stroke width: {raw:?}");
    assert!((raw[1] - (-100.0 / 6.0 - 1.75)).abs() < epsilon, "Rect includes the negative spline control and padding: {raw:?}");
    assert!((raw[2] - 25.25).abs() < epsilon, "Rect includes x=23.5 spline control and padding: {raw:?}");
    assert!((raw[3] - 101.75).abs() < epsilon);
    let raw_saved = reopen(&raw_doc);
    assert!(ap_content(&raw_saved, &list(&raw_saved, 0)[raw_i]).contains(" c\n"), "saved bounds have a smooth curve appearance");

    assert!(validate_signature_size(&strokes, 1.5, [27.0, 120.0]).is_ok(), "a target can scale the complete control hull");
    assert!(validate_signature_size(&strokes, 1.5, [27.1, 120.2]).is_ok());

    let mut input = sized_signature_input(strokes, 1.5);
    input.page = 0;
    let mut doc = fixture();
    let i = add_sized_signature(&mut doc, &input, [40.0, 40.0, 67.1, 160.2], &meta("spline-hull")).expect("control-hull fit adds");
    let saved = reopen(&doc);
    let d = &list(&saved, 0)[i];
    assert_eq!(rect(d), [40.0, 40.0, 67.1, 160.2]);
    assert_eq!(signature_strokes(d)[0].len(), 4, "input vertices survive the size fit");
    let points: Vec<(f64, f64)> = signature_strokes(d)[0].iter().map(|p| (p[0], p[1])).collect();
    let controls = appearance::smooth_segments(&points);
    for &(x, y) in points.iter().chain(controls.iter().flat_map(|segment| segment.iter())) {
        assert!(
            (41.75 - 1e-8..=65.35 + 1e-8).contains(&x) && (41.75 - 1e-8..=158.45 + 1e-8).contains(&y),
            "all actual curve controls retain physical padding: {x}/{y}"
        );
    }
    let ap = ap_content(&saved, d);
    assert!(ap.contains(" c\n"), "reopened signature appearance contains the smooth curve: {ap}");
}

#[test]
fn resized_and_restyled_signature_keeps_multiple_strokes_and_coincident_dots() {
    let strokes = vec![
        vec![[10.0, 10.0], [50.0, 40.0], [90.0, 10.0]],
        vec![[40.0, 30.0], [40.0, 30.0]],
        vec![[40.0, 30.0], [40.0, 30.0]],
        vec![[20.0, 20.0], [80.0, 20.0]],
    ];
    let mut doc = fixture();
    let i = add_annotation(&mut doc, &sized_signature_input(strokes, 2.0), &meta("strokes")).expect("adds");
    set_rect(&mut doc, 0, i, [200.0, 200.0, 300.0, 260.0], &meta("resize")).expect("resizes");
    set_style(&mut doc, 0, i, StyleChange::new(Some([1.0, 0.0, 0.0]), None, Some(3.0), None), &meta("restyle")).expect("restyles");

    let saved = reopen(&doc);
    let d = &list(&saved, 0)[i];
    let after = signature_strokes(d);
    assert_eq!(after.len(), 4, "separate paths remain separate");
    assert_eq!(after.iter().map(Vec::len).collect::<Vec<_>>(), [3, 2, 2, 2]);
    assert_eq!(after[1][0], after[1][1], "first coincident-point stroke stays a dot");
    assert_eq!(after[2][0], after[2][1], "second coincident-point stroke stays a dot");
    assert_eq!(after[1][0], after[2][0], "coincident dot strokes remain coincident");
    assert_eq!(signature_width(d), 3.0);
    let ap = ap_content(&saved, d);
    assert!(ap.contains("1 0 0 RG"), "restyled appearance has the new color: {ap}");
    assert!(ap.matches(" c\n").count() >= 3, "curved stroke and both dot paths remain visible in the appearance: {ap}");
}

#[test]
fn signature_dash_and_tiny_positive_width_remain_distinct_from_hairline() {
    let mut doc = fixture();
    let tiny = add_annotation(&mut doc, &sized_signature_input(vec![vec![[0.0, 0.0], [100.0, 0.0]]], 0.000001), &meta("tiny-dashed"))
        .expect("tiny positive-width stroke adds");
    let (_, tiny_ref) = annot_ref(&mut doc, 0, tiny).expect("signature ref");
    let mut bs = Dict::new();
    bs.set(b"W".to_vec(), Object::Real(0.000001));
    bs.set(b"S".to_vec(), Object::name("D"));
    bs.set(b"D".to_vec(), Object::Array(vec![Object::Real(0.000001), Object::Real(0.25)]));
    doc.update_dict(tiny_ref, |d| d.set(b"BS".to_vec(), Object::Dict(bs))).expect("sets dash style");
    set_rect(&mut doc, 0, tiny, [100.0, 100.0, 300.0, 150.0], &meta("tiny-resize")).expect("tiny dashed stroke resizes");
    let tiny_saved = reopen(&doc);
    let tiny_d = &list(&tiny_saved, 0)[tiny];
    assert!(signature_width(tiny_d) > 0.0, "tiny width remains positive in the PDF dictionary");
    let tiny_ap = ap_content(&tiny_saved, tiny_d);
    assert!(tiny_ap.contains(" w") && !tiny_ap.contains("0 w"), "tiny positive line width was not collapsed to hairline: {tiny_ap}");
    assert!(tiny_ap.contains(" d\n"), "dash operator remains in the appearance: {tiny_ap}");
    let tokens: Vec<&str> = tiny_ap.split_whitespace().collect();
    let width_at = tokens.iter().position(|token| *token == "w").unwrap();
    let local_width = tokens[width_at - 1].parse::<f64>().unwrap();
    assert!((local_width - 0.000005).abs() < 1e-15, "the tiny positive width is scaled and preserved: {tiny_ap}");
    let dash_line = tiny_ap.lines().find(|line| line.ends_with(" 0 d")).unwrap();
    let dash = dash_line.split(']').next().unwrap().trim_start_matches('[').split_whitespace().map(|v| v.parse::<f64>().unwrap()).collect::<Vec<_>>();
    assert_eq!(dash.len(), 2);
    assert!((dash[0] - 0.000005).abs() < 1e-15 && (dash[1] - 1.25).abs() < 1e-12, "dash lengths scale with the local frame: {tiny_ap}");

    let mut hairline_doc = fixture();
    let hairline = add_annotation(&mut hairline_doc, &sized_signature_input(vec![vec![[0.0, 0.0], [100.0, 0.0]]], 0.0), &meta("hairline"))
        .expect("non-dot hairline adds");
    set_rect(&mut hairline_doc, 0, hairline, [100.0, 100.0, 300.0, 150.0], &meta("hairline-resize")).expect("hairline resizes");
    let hairline_saved = reopen(&hairline_doc);
    let hairline_d = &list(&hairline_saved, 0)[hairline];
    assert_eq!(signature_width(hairline_d), 0.0);
    let hairline_ap = ap_content(&hairline_saved, hairline_d);
    assert!(hairline_ap.contains("0 w"), "actual hairline remains a zero-width stroke: {hairline_ap}");
    assert!(!hairline_ap.contains(" d\n"), "solid hairline stays distinct from the dashed tiny stroke");
}

#[test]
fn refused_signature_resizes_preserve_dictionary_and_appearance() {
    fn unchanged_after_refusal(mut doc: Document, i: usize, target: [f64; 4]) {
        let before = list(&doc, 0)[i].clone();
        let before_ap = ap_content(&doc, &before);
        assert!(set_rect(&mut doc, 0, i, target, &meta("refused")).is_err());
        let after = list(&doc, 0)[i].clone();
        assert_eq!(after, before, "failed resize leaves all dictionary entries untouched");
        assert_eq!(ap_content(&doc, &after), before_ap, "failed resize leaves the saved normal appearance untouched");
    }

    let make = || {
        let mut doc = fixture();
        let i = add_annotation(&mut doc, &sized_signature_input(vec![vec![[10.0, 10.0], [30.0, 20.0], [50.0, 10.0]]], 2.0), &meta("refusal"))
            .expect("adds signature");
        (doc, i)
    };

    let (mut locked, i) = make();
    set_locked(&mut locked, 0, i, true).expect("locks signature");
    unchanged_after_refusal(locked, i, [100.0, 100.0, 200.0, 160.0]);

    for malformed in ["InkList", "BS", "Rect"] {
        let (mut doc, i) = make();
        let (_, r) = annot_ref(&mut doc, 0, i).expect("signature ref");
        doc.update_dict(r, |d| match malformed {
            "InkList" => {
                d.set(b"InkList".to_vec(), Object::Array(vec![Object::Array(vec![Object::Real(1.0), Object::Real(2.0), Object::Real(3.0)])]))
            }
            "BS" => d.set(b"BS".to_vec(), Object::Int(7)),
            "Rect" => d.set(b"Rect".to_vec(), Object::Array(vec![Object::Real(1.0), Object::Real(2.0), Object::Real(3.0)])),
            _ => {}
        })
        .expect("sets malformed field");
        unchanged_after_refusal(doc, i, [100.0, 100.0, 200.0, 160.0]);
    }

    let (doc, i) = make();
    unchanged_after_refusal(doc, i, [1_000_000.0, 1_000_000.0, 1_000_000.001, 1_000_000.001]);
}

#[test]
fn ordinary_ink_resize_remains_permissive_and_uses_its_existing_path() {
    let mut doc = fixture();
    let i = add_annotation(&mut doc, &new(0, Shape::Ink { strokes: vec![vec![[10.0, 10.0], [30.0, 20.0]]] }), &meta("ordinary-ink"))
        .expect("adds ordinary ink");
    set_rect(&mut doc, 0, i, [100.0, 100.0, 160.0, 150.0], &meta("ordinary-resize")).expect("ordinary ink still resizes");
    let saved = reopen(&doc);
    let d = &list(&saved, 0)[i];
    assert_eq!(d.name(b"Subtype"), Some(&b"Ink"[..]));
    assert_ne!(text(d, b"Subj"), "Signature");
    assert_eq!(rect(d), [100.0, 100.0, 160.0, 150.0]);
    assert!(!ap_content(&saved, d).is_empty());
}

fn safety_signature() -> (Document, usize, ObjRef) {
    let mut doc = fixture();
    let index =
        add_annotation(&mut doc, &sized_signature_input(vec![vec![[10.0, 10.0], [30.0, 30.0], [50.0, 10.0]]], 2.0), &meta("safety-signature"))
            .expect("adds the baseline drawn signature");
    let (_, reference) = annot_ref(&mut doc, 0, index).expect("signature reference");
    (doc, index, reference)
}

fn safety_bytes(doc: &Document) -> Vec<u8> {
    write_incremental(doc, &SaveOptions::default()).expect("serializes the current document state")
}

fn safety_has_normal(doc: &Document, d: &Dict) -> bool {
    d.get(b"AP").is_some_and(|ap| doc.resolve(ap).as_dict().is_some_and(|ap| ap.contains(b"N")))
}

fn safety_corrupt_signature(doc: &mut Document, r: ObjRef, case: &str) {
    match case {
        "bad_width" => {
            let mut bs = Dict::new();
            bs.set(b"W".to_vec(), Object::String(PdfString::text("not a number")));
            doc.update_dict(r, |d| d.set(b"BS".to_vec(), Object::Dict(bs))).expect("sets malformed width");
        }
        "bad_rect_coordinate" => doc
            .update_dict(r, |d| {
                d.set(
                    b"Rect".to_vec(),
                    Object::Array(vec![Object::Real(1.0), Object::String(PdfString::text("bad")), Object::Real(50.0), Object::Real(50.0)]),
                )
            })
            .expect("sets malformed rectangle coordinate"),
        "short_rect" => doc
            .update_dict(r, |d| d.set(b"Rect".to_vec(), Object::Array(vec![Object::Real(1.0), Object::Real(2.0), Object::Real(3.0)])))
            .expect("sets malformed rectangle length"),
        name => {
            let ink_list = match name {
                "non_array" => Object::Int(4),
                "non_numeric_coordinate" => {
                    Object::Array(vec![Object::Array(vec![Object::Real(1.0), Object::String(PdfString::text("bad coordinate"))])])
                }
                "empty_ink_list" => Object::Array(Vec::new()),
                "empty_stroke" => Object::Array(vec![Object::Array(Vec::new())]),
                "unmatched_coordinate" => Object::Array(vec![Object::Array(vec![Object::Real(1.0)])]),
                "too_many_points" => {
                    let coordinates = (0..=100_000).flat_map(|x| [Object::Real(x as f64), Object::Real(0.0)]).collect();
                    Object::Array(vec![Object::Array(coordinates)])
                }
                "too_many_strokes" => Object::Array((0..=100_000).map(|_| Object::Array(vec![Object::Real(1.0), Object::Real(2.0)])).collect()),
                "cyclic_ink_list" => {
                    let cycle = doc.add(Object::Array(Vec::new()));
                    doc.set(cycle, Object::Array(vec![Object::Ref(cycle)]));
                    Object::Ref(cycle)
                }
                _ => panic!("unknown safety case: {name}"),
            };
            doc.update_dict(r, |d| d.set(b"InkList".to_vec(), ink_list)).expect("sets malformed InkList");
        }
    }
}

fn safety_expect_invalid(result: Result<(), AnnotError>, expected: &str) {
    match result {
        Err(AnnotError::Invalid(message)) => assert!(message.contains(expected), "expected {expected:?}, got {message:?}"),
        Err(other) => panic!("expected an input-validation error containing {expected:?}, got {other:?}"),
        Ok(()) => panic!("expected refusal containing {expected:?}"),
    }
}

#[test]
fn malformed_or_unbounded_drawn_signatures_refuse_without_changing_dictionary_or_appearance() {
    let cases = [
        ("non_array", "InkList is not an array"),
        ("non_numeric_coordinate", "coordinate is not a number"),
        ("empty_ink_list", "has no strokes"),
        ("empty_stroke", "has no strokes"),
        ("unmatched_coordinate", "unmatched coordinate"),
        ("too_many_points", "more than 100,000 points"),
        ("too_many_strokes", "too many strokes"),
        ("cyclic_ink_list", "unmatched coordinate"),
        ("bad_width", "line width is invalid"),
        ("bad_rect_coordinate", "rectangle coordinate is not a number"),
        ("short_rect", "rectangle is invalid"),
    ];

    for (case, expected) in cases {
        let (mut doc, index, reference) = safety_signature();
        safety_corrupt_signature(&mut doc, reference, case);
        let before_bytes = safety_bytes(&doc);
        let before_ap = ap_content(&doc, &list(&doc, 0)[index]);

        safety_expect_invalid(set_rect(&mut doc, 0, index, [200.0, 200.0, 300.0, 260.0], &meta("refused")), expected);
        assert_eq!(safety_bytes(&doc), before_bytes, "{case}: refused resize changed serialized document bytes");
        assert_eq!(ap_content(&doc, &list(&doc, 0)[index]), before_ap, "{case}: refused resize replaced the stored AP");

        safety_expect_invalid(set_appearance(&mut doc, reference), expected);
        assert_eq!(safety_bytes(&doc), before_bytes, "{case}: refused appearance rebuild changed serialized document bytes");
        assert_eq!(ap_content(&doc, &list(&doc, 0)[index]), before_ap, "{case}: refused appearance rebuild replaced the stored AP");
    }
}

#[test]
fn display_defaults_only_raw_malformed_signature_styles_and_keeps_derived_failures_ap_free() {
    let cases = [
        ("malformed_dash_defaults_solid", true),
        ("empty_dash_is_valid_solid", true),
        ("zero_dash_defaults_solid", true),
        ("malformed_width_defaults", true),
        ("huge_valid_dash_is_not_defaulted", false),
        ("tiny_valid_dash_is_not_defaulted", false),
        ("width_at_upper_bound_is_not_defaulted", false),
    ];

    for (case, expect_display_appearance) in cases {
        let (mut doc, index, reference) = safety_signature();
        match case {
            "malformed_dash_defaults_solid"
            | "empty_dash_is_valid_solid"
            | "zero_dash_defaults_solid"
            | "huge_valid_dash_is_not_defaulted"
            | "tiny_valid_dash_is_not_defaulted" => {
                let dash = match case {
                    "malformed_dash_defaults_solid" => Object::String(PdfString::text("invalid dash")),
                    "empty_dash_is_valid_solid" => Object::Array(Vec::new()),
                    "zero_dash_defaults_solid" => Object::Array(vec![Object::Real(0.0), Object::Real(0.0)]),
                    "huge_valid_dash_is_not_defaulted" => Object::Array(vec![Object::Real(1e308), Object::Real(1.0)]),
                    _ => Object::Array(vec![Object::Real(1e-320), Object::Real(1.0)]),
                };
                let mut bs = Dict::new();
                bs.set(b"W".to_vec(), Object::Real(2.0));
                bs.set(b"S".to_vec(), Object::name("D"));
                bs.set(b"D".to_vec(), dash);
                doc.update_dict(reference, |d| {
                    d.set(b"BS".to_vec(), Object::Dict(bs));
                    d.remove(b"AP");
                })
                .expect("sets display dash case");
            }
            "malformed_width_defaults" => {
                let mut bs = Dict::new();
                bs.set(b"W".to_vec(), Object::String(PdfString::text("invalid width")));
                doc.update_dict(reference, |d| {
                    d.set(b"BS".to_vec(), Object::Dict(bs));
                    d.remove(b"AP");
                })
                .expect("sets display width case");
            }
            "width_at_upper_bound_is_not_defaulted" => {
                let mut bs = Dict::new();
                bs.set(b"W".to_vec(), Object::Real(1e7));
                doc.update_dict(reference, |d| {
                    d.set(b"BS".to_vec(), Object::Dict(bs));
                    d.remove(b"AP");
                })
                .expect("sets display width upper bound");
            }
            _ => panic!("unknown display case: {case}"),
        }

        let source_before = safety_bytes(&doc);
        assert!(!safety_has_normal(&doc, &list(&doc, 0)[index]), "{case}: source signature starts without /AP /N");
        // The fixture's Square also needs a display appearance, so `Some(copy)` alone does not
        // prove that this signature was drawn. Inspect the specific signature entry below.
        let display = with_missing_appearances(&doc).expect("the fixture Square needs a display appearance");
        let displayed_signature = list(&display, 0)[index].clone();
        assert_eq!(safety_has_normal(&display, &displayed_signature), expect_display_appearance, "{case}");
        if expect_display_appearance && case.contains("dash") {
            let shown = ap_content(&display, &displayed_signature);
            assert!(shown.contains(" m\n"), "{case}: compatibility default produces visible paths");
            assert!(!shown.contains(" d\n"), "{case}: solid appearance contains no dash operator");
        }
        assert_eq!(safety_bytes(&doc), source_before, "{case}: display synthesis mutated saved source bytes");
        assert!(!safety_has_normal(&doc, &list(&doc, 0)[index]), "{case}: source signature remains AP-free");
    }
}

#[test]
fn appearance_rebuild_accepts_one_level_indirect_inklist_and_border_style() {
    let (mut doc, index, reference) = safety_signature();
    let stroke = doc.add(Object::Array(vec![Object::Real(10.0), Object::Real(10.0), Object::Real(30.0), Object::Real(30.0)]));
    let ink_list = doc.add(Object::Array(vec![Object::Ref(stroke)]));
    let mut bs = Dict::new();
    bs.set(b"W".to_vec(), Object::Real(2.0));
    bs.set(b"S".to_vec(), Object::name("D"));
    bs.set(b"D".to_vec(), Object::Array(vec![Object::Real(3.0), Object::Real(2.0)]));
    let border_style = doc.add(Object::Dict(bs));
    doc.update_dict(reference, |d| {
        d.set(b"InkList".to_vec(), Object::Ref(ink_list));
        d.set(b"BS".to_vec(), Object::Ref(border_style));
    })
    .expect("sets supported one-level indirect style objects");

    // This exercises the appearance reader directly; set_style's separate raw preflight remains
    // a distinct, already-qualified boundary.
    set_appearance(&mut doc, reference).expect("a valid signature may use one-level indirect InkList and BS objects");
    let saved = &list(&doc, 0)[index];
    assert_eq!(saved.reference(b"InkList"), Some(ink_list));
    assert_eq!(saved.reference(b"BS"), Some(border_style));
    assert!(ap_content(&doc, saved).contains(" d\n"), "the valid indirect dash remains dashed");
}

#[test]
fn pc_fill_sign_and_wrong_subject_do_not_route_ordinary_ink_through_signature_resize() {
    let mut doc = fixture();
    let index = add_annotation(&mut doc, &new(0, Shape::Ink { strokes: vec![vec![[10.0, 10.0], [30.0, 20.0]]] }), &meta("ordinary-ink-control"))
        .expect("adds ordinary ink");
    let (_, reference) = annot_ref(&mut doc, 0, index).expect("ordinary ink ref");
    let mut malformed_bs = Dict::new();
    malformed_bs.set(b"W".to_vec(), Object::String(PdfString::text("not a signature width")));
    malformed_bs.set(b"S".to_vec(), Object::name("S"));
    doc.update_dict(reference, |d| {
        d.set(b"Subj".to_vec(), Object::String(PdfString::text("Not a signature")));
        d.set(b"PCFillSign".to_vec(), Object::Bool(true));
        d.set(b"BS".to_vec(), Object::Dict(malformed_bs));
    })
    .expect("sets Fill and Sign marker with an unrelated subject");

    set_rect(&mut doc, 0, index, [100.0, 100.0, 160.0, 150.0], &meta("ordinary-resize")).expect("ordinary Ink keeps its established resize route");
    let saved = &list(&reopen(&doc), 0)[index];
    assert_eq!(saved.name(b"Subtype"), Some(&b"Ink"[..]));
    assert_eq!(text(saved, b"Subj"), "Not a signature");
    assert!(matches!(saved.get(b"PCFillSign"), Some(Object::Bool(true))));
    assert_eq!(rect(saved), [100.0, 100.0, 160.0, 150.0]);
    assert!(matches!(saved.get(b"BS").and_then(Object::as_dict).and_then(|bs| bs.get(b"W")), Some(Object::String(_))));
    assert!(saved.get(b"AP").is_some(), "ordinary Ink receives its normal established appearance");
}

#[test]
fn signature_raw_appearance_defaults_only_an_absent_dash_array() {
    let dictionary = |pattern: Option<&str>| {
        let pattern = pattern.map_or(String::new(), |value| format!("/D {value}"));
        let source =
            format!("<< /Subtype /Ink /Subj (Signature) /Rect [0 0 100 100] /C [0 0 0] /InkList [[10 10 90 90]] /BS << /W 1 /S /D {pattern} >> >>");
        let doc = document_of(&[source.as_bytes()]);
        doc.get(ObjRef::new(1, 0)).as_dict().unwrap().clone()
    };
    let explicit = appearance::build(&dictionary(Some("[3]"))).expect("explicit [3] positive control");
    let implicit = appearance::build(&dictionary(None)).expect("optional BS/D must use the valid default [3]");
    assert_eq!(implicit.decoded().unwrap(), explicit.decoded().unwrap(), "raw preflight and direct drawing must agree on the optional default");
    assert!(String::from_utf8(implicit.decoded().unwrap()).unwrap().contains("[3] 0 d\n"));
    let too_many = format!("[{}]", vec!["1"; 65].join(" "));
    for malformed in ["5", "(bad)", "[0 0]", "[-1 2]", "[(bad) 2]", too_many.as_str()] {
        assert!(appearance::build(&dictionary(Some(malformed))).is_none(), "present malformed D is not an absent default: {malformed}");
    }
    let mut nonfinite = dictionary(Some("[3]"));
    let Object::Dict(bs) = nonfinite.get_mut(b"BS").unwrap() else { panic!("direct BS dictionary") };
    bs.set(b"D".to_vec(), Object::Array(vec![Object::Real(f64::INFINITY)]));
    assert!(appearance::build(&nonfinite).is_none(), "nonfinite present dash remains refused");
}

#[test]
fn signature_with_optional_default_dash_can_be_restyled_after_reopen() {
    let mut doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        b"<< /Type /Annot /Subtype /Ink /Subj (Signature) /Rect [0 0 100 100] /C [0 0 0] /InkList [[10 10 90 90]] /BS << /W 1 /S /D >> >>",
    ]);
    set_style(&mut doc, 0, 0, StyleChange::new(Some([1.0, 0.0, 0.0]), Some(0.5), Some(2.0), None), &meta("default-dash-style"))
        .expect("valid optional default dash must pass raw preflight and normalized redraw");
    let d = list(&doc, 0)[0].clone();
    let bs = d.get(b"BS").unwrap().as_dict().unwrap();
    assert_eq!(bs.name(b"S"), Some(&b"D"[..]));
    assert!(bs.get(b"D").is_none(), "redraw must preserve the stored optional-entry form");
    assert_eq!(bs.get(b"W").unwrap().as_f64(), Some(2.0));
    assert_eq!(d.get(b"CA").unwrap().as_f64(), Some(0.5));
    let content = ap_content(&doc, &d);
    assert!(content.contains("[30] 0 d\n"), "central normalized AP must scale raw default3 by10: {content}");
    assert!(content.contains("20 w"), "central normalized AP must scale new width2 by10: {content}");
    let mut saved = reopen(&doc);
    set_style(&mut saved, 0, 0, StyleChange::new(Some([0.0, 0.0, 1.0]), None, None, None), &meta("default-dash-after-reopen"))
        .expect("the optional default remains editable after save/reopen");
    assert!(ap_content(&saved, &list(&saved, 0)[0]).contains("[30] 0 d\n"));
}

fn signature_optional_dash_fixture(pattern: Option<&str>) -> Document {
    let pattern = pattern.map_or(String::new(), |value| format!("/D {value}"));
    let annotation = format!(
        "<< /Type /Annot /Subtype /Ink /Subj (Signature) /Rect [0 0 100 100] /C [0 0 0] /InkList [[10 10 90 90]] /NM (stable-id) /T (Ada) /Foo (kept) /BS << /W 1 /S /D /PrivateStyle (kept-style) {pattern} >> >>"
    );
    document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        annotation.as_bytes(),
    ])
}

#[test]
fn signature_raw_null_dash_uses_optional_default() {
    let explicit = signature_optional_dash_fixture(Some("[3]"));
    let null = signature_optional_dash_fixture(Some("null"));
    let explicit = appearance::build(&list(&explicit, 0)[0]).expect("explicit dash draws");
    let null = appearance::build(&list(&null, 0)[0]).expect("null dictionary entry uses optional default");
    assert_eq!(null.decoded().unwrap(), explicit.decoded().unwrap());
}

#[test]
fn signature_raw_empty_dash_is_solid() {
    let doc = signature_optional_dash_fixture(Some("[]"));
    let empty = list(&doc, 0)[0].clone();
    let mut solid = empty.clone();
    let Object::Dict(bs) = solid.get_mut(b"BS").unwrap() else { panic!("direct BS") };
    bs.set(b"S".to_vec(), Object::name("S"));
    bs.remove(b"D");
    let solid = appearance::build(&solid).expect("solid control draws");
    let empty = appearance::build(&empty).expect("valid empty dash array draws solid");
    assert_eq!(empty.decoded().unwrap(), solid.decoded().unwrap());
}

fn assert_signature_normalized_optional_dash(pattern: &str, dashed: bool) {
    let mut doc = signature_optional_dash_fixture(Some(pattern));
    // This enters document-aware normalization directly, bypassing set_style's raw preflight.
    set_appearance(&mut doc, ObjRef::new(4, 0)).expect("valid optional dash form normalizes");
    let content = ap_content(&doc, &list(&doc, 0)[0]);
    assert_eq!(content.contains("[30] 0 d\n"), dashed, "{pattern}: {content}");
    assert_eq!(content.contains(" d\n"), dashed, "{pattern}: {content}");
    assert!(content.contains(" m\n"), "valid signature has visible paths: {content}");
}

#[test]
fn signature_normalized_null_dash_uses_optional_default() {
    assert_signature_normalized_optional_dash("null", true);
}

#[test]
fn signature_normalized_empty_dash_is_solid() {
    assert_signature_normalized_optional_dash("[]", false);
}

#[test]
fn signature_normalized_optional_dash_preserves_one_level_indirect_values() {
    for (pattern, dashed) in [(Object::Null, true), (Object::Array(Vec::new()), false)] {
        let mut doc = signature_optional_dash_fixture(Some("[3]"));
        let value = doc.add(pattern);
        doc.update_dict(ObjRef::new(4, 0), |d| {
            let Object::Dict(bs) = d.get_mut(b"BS").unwrap() else { panic!("direct BS") };
            bs.set(b"D".to_vec(), Object::Ref(value));
        })
        .unwrap();
        set_appearance(&mut doc, ObjRef::new(4, 0)).expect("one-level optional dash value normalizes");
        let d = list(&doc, 0)[0].clone();
        assert_eq!(d.get(b"BS").unwrap().as_dict().unwrap().reference(b"D"), Some(value));
        assert_eq!(ap_content(&doc, &d).contains("[30] 0 d\n"), dashed);
        assert_eq!(ap_content(&doc, &d).contains(" d\n"), dashed);
    }
}

#[test]
fn signature_legacy_border_empty_is_solid_but_null_array_element_is_invalid() {
    for (dash, valid) in [(Object::Array(Vec::new()), true), (Object::Null, false)] {
        let mut doc = signature_optional_dash_fixture(None);
        doc.update_dict(ObjRef::new(4, 0), |d| {
            d.remove(b"BS");
            d.set(b"Border".to_vec(), Object::Array(vec![Object::Real(0.0), Object::Real(0.0), Object::Real(1.0), dash]));
        })
        .unwrap();
        let before = safety_bytes(&doc);
        let result = set_appearance(&mut doc, ObjRef::new(4, 0));
        if valid {
            result.expect("legacy empty dash is a valid solid pattern");
            let content = ap_content(&doc, &list(&doc, 0)[0]);
            assert!(!content.contains(" d\n"));
            assert!(content.contains(" m\n"));
        } else {
            safety_expect_invalid(result, "dash pattern is invalid");
            assert_eq!(safety_bytes(&doc), before, "dictionary null-as-absent semantics do not apply to array elements");
        }
    }
}

#[test]
fn signature_optional_dash_forms_preserve_stored_metadata_after_restyle_and_reopen() {
    for pattern in [None, Some("null"), Some("[3]"), Some("[]")] {
        let mut doc = signature_optional_dash_fixture(pattern);
        let before = list(&doc, 0)[0].clone();
        let expected_dash = before.get(b"BS").unwrap().as_dict().unwrap().get(b"D").cloned();
        set_style(&mut doc, 0, 0, StyleChange::new(Some([1.0, 0.0, 0.0]), Some(0.5), Some(2.0), None), &meta("style"))
            .expect("valid optional dash form passes raw preflight and normalized restyle");
        let mut saved = reopen(&doc);
        set_style(&mut saved, 0, 0, StyleChange::new(Some([0.0, 0.0, 1.0]), None, None, None), &meta("after-reopen"))
            .expect("valid optional dash form stays editable after reopening");
        let saved = reopen(&saved);
        let d = list(&saved, 0)[0].clone();
        let bs = d.get(b"BS").unwrap().as_dict().unwrap();
        assert_eq!(bs.name(b"S"), Some(&b"D"[..]));
        assert_eq!(bs.get(b"D").cloned(), expected_dash, "stored D spelling preserved: {pattern:?}");
        assert_eq!(text(bs, b"PrivateStyle"), "kept-style");
        assert_eq!(text(&d, b"Foo"), "kept");
        assert_eq!(text(&d, b"NM"), "stable-id");
        assert_eq!(text(&d, b"T"), "Ada");
        assert_eq!(bs.get(b"W").unwrap().as_f64(), Some(2.0));
        assert_eq!(d.get(b"CA").unwrap().as_f64(), Some(0.5));
        assert_eq!(d.get(b"C").unwrap().as_array().unwrap().iter().map(Object::as_f64).collect::<Vec<_>>(), vec![Some(0.0), Some(0.0), Some(1.0)]);
        let content = ap_content(&saved, &d);
        let dashed = pattern != Some("[]");
        assert_eq!(content.contains("[30] 0 d\n"), dashed, "{pattern:?}: {content}");
        assert_eq!(content.contains(" d\n"), dashed, "{pattern:?}: {content}");
        assert!(content.contains("20 w"), "physical line width preserved: {pattern:?}: {content}");
    }
}

fn signature_indirect_dash_fixture(pattern: Object) -> (Document, ObjRef) {
    let mut doc = signature_optional_dash_fixture(Some("[3]"));
    set_appearance(&mut doc, ObjRef::new(4, 0)).expect("existing AP control");
    let value = doc.add(pattern);
    doc.update_dict(ObjRef::new(4, 0), |d| {
        let Object::Dict(bs) = d.get_mut(b"BS").unwrap() else { panic!("direct BS") };
        bs.set(b"D".to_vec(), Object::Ref(value));
    })
    .unwrap();
    (doc, value)
}

#[test]
fn signature_indirect_dash_props_and_restyle_preserve_reference_after_reopen() {
    for (pattern, dashed) in [(Object::Array(vec![Object::Real(3.0)]), true), (Object::Null, true), (Object::Array(Vec::new()), false)] {
        let (mut doc, value) = signature_indirect_dash_fixture(pattern);
        let before = safety_bytes(&doc);
        assert!(props(&doc, 0, 0).unwrap().restylable, "supported reference enables actual properties controls");
        assert_eq!(safety_bytes(&doc), before, "eligibility is pure");
        set_style(&mut doc, 0, 0, StyleChange::new(Some([1.0, 0.0, 0.0]), Some(0.5), Some(2.0), None), &meta("indirect"))
            .expect("supported indirect D restyles");
        let mut saved = reopen(&doc);
        assert!(props(&saved, 0, 0).unwrap().restylable);
        set_style(&mut saved, 0, 0, StyleChange::new(Some([0.0, 0.0, 1.0]), None, None, None), &meta("indirect-reopen"))
            .expect("supported indirect D stays editable");
        let saved = reopen(&saved);
        let d = list(&saved, 0)[0].clone();
        let bs = d.get(b"BS").unwrap().as_dict().unwrap();
        assert_eq!(bs.reference(b"D"), Some(value));
        assert_eq!(bs.get(b"W").unwrap().as_f64(), Some(2.0));
        assert_eq!(text(bs, b"PrivateStyle"), "kept-style");
        assert_eq!(text(&d, b"Foo"), "kept");
        assert_eq!(text(&d, b"NM"), "stable-id");
        assert_eq!(text(&d, b"T"), "Ada");
        assert_eq!(d.get(b"CA").unwrap().as_f64(), Some(0.5));
        let content = ap_content(&saved, &d);
        assert_eq!(content.contains("[30] 0 d\n"), dashed);
        assert_eq!(content.contains(" d\n"), dashed);
        assert!(content.contains("20 w") && content.contains(" m\n"));
    }
}

#[test]
fn signature_indirect_dash_invalid_values_refuse_without_mutation() {
    for case in ["wrong_type", "cycle", "deep", "too_long", "zero", "negative", "huge", "scaled_huge", "tiny"] {
        let pattern = match case {
            "wrong_type" => Object::String(PdfString::text("bad")),
            "too_long" => Object::Array(vec![Object::Real(1.0); 65]),
            "zero" => Object::Array(vec![Object::Real(0.0), Object::Real(0.0)]),
            "negative" => Object::Array(vec![Object::Real(-1.0), Object::Real(2.0)]),
            "huge" => Object::Array(vec![Object::Real(1e308), Object::Real(1.0)]),
            "scaled_huge" => Object::Array(vec![Object::Real(1e38), Object::Real(1.0)]),
            "tiny" => Object::Array(vec![Object::Real(1e-320), Object::Real(1.0)]),
            _ => Object::Null,
        };
        let (mut doc, value) = signature_indirect_dash_fixture(pattern);
        if case == "cycle" {
            doc.set(value, Object::Ref(value));
        } else if case == "deep" {
            let second = doc.add(Object::Array(vec![Object::Real(3.0)]));
            doc.set(value, Object::Ref(second));
        }
        let before = safety_bytes(&doc);
        let before_ap = ap_content(&doc, &list(&doc, 0)[0]);
        assert!(!props(&doc, 0, 0).unwrap().restylable, "{case}");
        assert_eq!(safety_bytes(&doc), before, "{case}: properties preflight mutated source");
        assert!(set_style(&mut doc, 0, 0, StyleChange::new(Some([1.0, 0.0, 0.0]), Some(0.5), Some(2.0), None), &meta("refused")).is_err(), "{case}");
        assert_eq!(safety_bytes(&doc), before, "{case}: refusal changed source");
        assert_eq!(ap_content(&doc, &list(&doc, 0)[0]), before_ap, "{case}: refusal changed AP");
    }
}

#[test]
fn signature_document_free_builder_does_not_guess_indirect_dash_values() {
    for pattern in [Object::Array(vec![Object::Real(3.0)]), Object::Null, Object::Array(Vec::new()), Object::String(PdfString::text("bad"))] {
        let (doc, _) = signature_indirect_dash_fixture(pattern);
        assert!(appearance::build(&list(&doc, 0)[0]).is_none(), "raw builder lacks document context and must refuse Ref D");
    }
}

#[test]
fn signature_indirect_missing_dash_uses_canonical_null_default() {
    let mut doc = signature_optional_dash_fixture(Some("[3]"));
    let missing = ObjRef::new(9999, 0);
    assert!(matches!(doc.get(missing).as_ref(), Object::Null));
    doc.update_dict(ObjRef::new(4, 0), |d| {
        let Object::Dict(bs) = d.get_mut(b"BS").unwrap() else { panic!("direct BS") };
        bs.set(b"D".to_vec(), Object::Ref(missing));
    })
    .unwrap();
    assert!(props(&doc, 0, 0).unwrap().restylable);
    set_style(&mut doc, 0, 0, StyleChange::new(Some([1.0, 0.0, 0.0]), None, None, None), &meta("missing-default"))
        .expect("missing object resolves to canonical dictionary Null and default [3]");
    let d = list(&doc, 0)[0].clone();
    assert_eq!(d.get(b"BS").unwrap().as_dict().unwrap().reference(b"D"), Some(missing));
    assert!(ap_content(&doc, &d).contains("[30] 0 d\n"));
}
