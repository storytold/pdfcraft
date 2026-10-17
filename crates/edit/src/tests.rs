use std::sync::Arc;

use pdfcraft_cos::{Document, SaveOptions, write_full, write_incremental};

use super::*;

/// Three pages: an upright page with content that leaves the graphics state changed (an
/// unbalanced `cm`), a page rotated 90° and one with inherited, shared resources.
fn fixture() -> Document {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 600 800] /Resources 6 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 7 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /Rotate 90 /Contents [7 0 R] >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
        "<< /Font << /F1 8 0 R >> >>".into(),
        "<< /Length 37 >>\nstream\n2 0 0 2 0 0 cm BT /F1 9 Tf (Hi) Tj ET\nendstream".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>".into(),
    ];
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

fn reopen(doc: &Document) -> Document {
    let bytes = write_incremental(doc, &SaveOptions::default()).unwrap();
    hayro_syntax::Pdf::new(bytes.clone()).expect("parses");
    Document::open(Arc::new(bytes)).unwrap()
}

/// The decoded content streams of a page, in order.
fn streams(doc: &Document, page: usize) -> Vec<String> {
    let p = &pdfcraft_model::pages(doc)[page];
    let c = p.dict.get(b"Contents").cloned();
    let list = match c.map(|c| doc.resolve(&c)).as_deref() {
        Some(Object::Array(a)) => a.clone(),
        Some(_) => vec![p.dict.get(b"Contents").cloned().unwrap()],
        None => vec![],
    };
    list.iter().map(|o| String::from_utf8_lossy(&stream_bytes(doc, o).unwrap()).into_owned()).collect()
}

fn cx() -> Context {
    Context { date: (2026, 10, 1) }
}

#[test]
fn tokens_expand() {
    let c = cx();
    assert_eq!(expand("Page <<1>> of <<n>>", 3, 10, 0, &c), "Page 3 of 10");
    assert_eq!(expand("<<1 of n>> · <<Page 1>> · <<1/n>>", 2, 5, 0, &c), "2 of 5 · Page 2 · 2/5");
    assert_eq!(expand("<<m/d/yyyy>> <<yyyy-mm-dd>> <<mmmm d, yyyy>>", 1, 1, 0, &c), "10/1/2026 2026-10-01 October 1, 2026");
    assert_eq!(expand("<<Bates Number#6#100#ABC#-X>>", 1, 1, 104, &c), "ABC000104-X");
    assert_eq!(expand("keep <<unknown>> and <<unclosed", 1, 1, 0, &c), "keep <<unknown>> and <<unclosed");
    assert_eq!(bates_start("x <<Bates Number#6#100#A#B>>"), Some(100));
}

#[test]
fn header_and_footer_are_drawn_in_display_space_and_wrap_the_original_content() {
    let mut doc = fixture();
    let hf = HeaderFooter {
        text: ["Left".into(), String::new(), "<<Page 1 of n>>".into(), String::new(), "Confidential (draft)".into(), String::new()],
        ..HeaderFooter::default()
    };
    add_header_footer(&mut doc, &[0, 1, 2], &hf, false, &cx()).unwrap();
    let doc = reopen(&doc);
    let s0 = streams(&doc, 0);
    // q-wrapper, original, Q-wrapper, header/footer.
    assert_eq!(s0.len(), 4, "{s0:?}");
    assert_eq!((s0[0].as_str(), s0[2].as_str()), ("q %PdfCraft\n", "Q %PdfCraft\n"));
    let mark = &s0[3];
    assert!(mark.contains("/PCMark /HeaderFooter") && mark.contains("(Page 1 of 3) Tj") && mark.contains("(Confidential \\(draft\\)) Tj"), "{mark}");
    assert!(mark.contains("1 0 0 1 0 0 cm"), "upright page: identity");
    // The rotated page maps display space onto its rotated crop box.
    let s1 = streams(&doc, 1);
    assert!(s1.last().unwrap().contains("0 1 -1 0 600 0 cm") && s1.last().unwrap().contains("(Page 2 of 3)"), "{s1:?}");
    // A page without content gets just the mark; its inherited resources are copied, not changed.
    assert_eq!(streams(&doc, 2).len(), 1);
    let p2 = &pdfcraft_model::pages(&doc)[2];
    let res = doc.resolve(p2.dict.get(b"Resources").unwrap());
    let fonts = doc.resolve(res.as_dict().unwrap().get(b"Font").unwrap());
    assert!(fonts.as_dict().unwrap().contains(b"PCHelv") && fonts.as_dict().unwrap().contains(b"F1"));
    let shared = doc.get(pdfcraft_cos::ObjRef::new(6, 0));
    let shared_fonts = doc.resolve(shared.as_dict().unwrap().get(b"Font").unwrap());
    assert!(!shared_fonts.as_dict().unwrap().contains(b"PCHelv"), "the shared dictionary is untouched");
    assert_eq!(marks_present(&doc), [MarkKind::HeaderFooter]);
}

#[test]
fn header_start_number_that_would_wrap_is_refused_without_drawing() {
    // u32::MAX is a valid start_number on its own, but there is no room for a second page
    // number after it. The old code wrapped the second page to zero; now the whole run is
    // refused and the document keeps no header at all.
    let mut doc = fixture();
    let hf = HeaderFooter {
        text: [String::new(), String::new(), "<<1>>".into(), String::new(), String::new(), String::new()],
        start_number: u32::MAX,
        ..HeaderFooter::default()
    };
    let err = add_header_footer(&mut doc, &[0, 1], &hf, false, &cx()).unwrap_err();
    assert!(err.to_string().contains("leaves no room"), "{err}");
    assert!(marks_present(&doc).is_empty(), "nothing is drawn when the run is refused");
    // A single page still fits the start number itself.
    add_header_footer(&mut doc, &[0], &hf, false, &cx()).unwrap();
    assert_eq!(marks_present(&doc).len(), 1);
}

#[test]
fn header_bates_start_that_would_overflow_is_refused_without_drawing() {
    // A Bates start of u64::MAX has no room for a second page. The run must be refused up front
    // instead of overflowing (a panic in debug builds, a wrap to 0 in release builds).
    let mut doc = fixture();
    let hf = HeaderFooter {
        text: [String::new(), String::new(), "<<Bates Number#6#18446744073709551615#A#B>>".into(), String::new(), String::new(), String::new()],
        ..HeaderFooter::default()
    };
    let err = add_header_footer(&mut doc, &[0, 1], &hf, false, &cx()).unwrap_err();
    assert!(err.to_string().contains("Bates"), "{err}");
    assert!(marks_present(&doc).is_empty(), "nothing is drawn when the run is refused");
    // A single page still fits the start number itself.
    add_header_footer(&mut doc, &[0], &hf, false, &cx()).unwrap();
    assert_eq!(marks_present(&doc).len(), 1);
}

#[test]
fn replace_and_remove_restore_the_original_content() {
    let mut doc = fixture();
    let original = streams(&doc, 0);
    let mut hf = HeaderFooter::default();
    hf.text[1] = "First".into();
    add_header_footer(&mut doc, &[0], &hf, false, &cx()).unwrap();
    hf.text[1] = "Second".into();
    add_header_footer(&mut doc, &[0], &hf, true, &cx()).unwrap();
    let s = streams(&doc, 0);
    assert_eq!(s.iter().filter(|x| x.contains("/PCMark")).count(), 1, "replaced, not added");
    assert!(s.last().unwrap().contains("(Second)"));
    add_watermark(&mut doc, &[0], &Watermark { text: "DRAFT".into(), ..Watermark::default() }, false).unwrap();
    add_background(&mut doc, &[0], &Background { color: [1.0, 1.0, 0.9], opacity: 1.0, ..Background::default() }, false).unwrap();
    assert_eq!(marks_present(&doc).len(), 3);
    assert_eq!(remove_marks(&mut doc, &[0], MarkKind::HeaderFooter).unwrap(), 1);
    assert_eq!(remove_marks(&mut doc, &[0], MarkKind::Watermark).unwrap(), 1);
    let s = streams(&doc, 0);
    assert!(s[0].contains("/PCMark /Background"), "the background stays behind: {s:?}");
    assert_eq!(remove_marks(&mut doc, &[0], MarkKind::Background).unwrap(), 1);
    assert_eq!(streams(&reopen(&doc), 0), original, "back to exactly the original content");
}

#[test]
fn watermarks_rotate_fade_and_can_go_behind() {
    let mut doc = fixture();
    let wm = Watermark { text: "CONFIDENTIAL\nDo not copy".into(), opacity: 0.3, rotation: 45.0, behind: true, ..Watermark::default() };
    add_watermark(&mut doc, &[0], &wm, false).unwrap();
    let s = streams(&doc, 0);
    assert!(s[0].contains("/PCMark /Watermark"), "behind: first");
    assert!(s[0].contains("/PCGS30 gs") && s[0].contains("0.707 0.707 -0.707 0.707 300 400 cm"), "{}", s[0]);
    assert!(s[0].contains("(CONFIDENTIAL) Tj") && s[0].contains("(Do not copy) Tj"));
    assert_eq!(s.len(), 2, "no wrapper needed for content behind");
}

#[test]
fn watermark_offset_moves_the_centre_right_and_up() {
    let mut doc = fixture();
    let wm = Watermark { text: "DRAFT".into(), rotation: 0.0, offset: [-100.0, 50.0], ..Watermark::default() };
    add_watermark(&mut doc, &[0], &wm, false).unwrap();
    let s = streams(&doc, 0);
    let mark = s.iter().find(|c| c.contains("/PCMark /Watermark")).expect("the watermark stream");
    assert!(mark.contains("1 0 0 1 200 450 cm"), "{mark}");
}

#[test]
fn a_watermark_offset_that_is_not_finite_is_refused() {
    for offset in [[f64::NAN, 0.0], [0.0, f64::INFINITY], [f64::NEG_INFINITY, 0.0]] {
        let mut doc = fixture();
        let wm = Watermark { text: "DRAFT".into(), offset, ..Watermark::default() };
        assert!(matches!(add_watermark(&mut doc, &[0], &wm, false), Err(EditError::Invalid(_))), "{offset:?}");
        assert!(!doc.is_modified());
    }
}

#[test]
fn invalid_requests_change_nothing() {
    let mut doc = fixture();
    assert!(matches!(add_header_footer(&mut doc, &[0], &HeaderFooter::default(), false, &cx()), Err(EditError::Invalid(_))));
    assert!(matches!(add_watermark(&mut doc, &[0], &Watermark::default(), false), Err(EditError::Invalid(_))));
    assert_eq!(
        add_background(&mut doc, &[9], &Background { color: [1.0; 3], opacity: 1.0, ..Background::default() }, false),
        Err(EditError::NoSuchPage(9))
    );
    assert_eq!(EditError::NoSuchPage(9).to_string(), "page 10 does not exist", "the message counts pages from 1 (#1550)");
    assert!(!doc.is_modified());
}

#[test]
fn flattening_draws_appearances_into_the_page_and_removes_the_comments() {
    use pdfcraft_annot::{Meta, NewAnnotation, NoteIcon, Shape, Style, add_annotation, add_reply};
    let mut doc = fixture();
    let meta = Meta { date: None, id: "x".into() };
    let add = |doc: &mut Document, shape: Shape| {
        let style = Style::default_for(&shape);
        add_annotation(doc, &NewAnnotation { page: 0, shape, style, contents: "c".into(), author: "a".into() }, &meta).unwrap()
    };
    add(&mut doc, Shape::Rectangle { rect: [10.0, 10.0, 110.0, 60.0] });
    let note = add(&mut doc, Shape::Note { at: [200.0, 700.0], icon: NoteIcon::Comment });
    add_reply(&mut doc, 0, note, "reply", "b", &meta).unwrap();
    let before = streams(&doc, 0);
    let n = flatten(&mut doc, &[0], true, false).unwrap();
    assert_eq!(n, 2, "the rectangle and the note icon are drawn; the reply has nothing to draw");
    let doc = reopen(&doc);
    let p = &pdfcraft_model::pages(&doc)[0];
    assert!(!p.dict.contains(b"Annots"), "comments, pop-up and reply are gone");
    let s = streams(&doc, 0);
    assert_eq!(s.len(), before.len() + 3, "wrapped original + flattened content: {s:?}");
    let flat = s.last().unwrap();
    assert!(flat.contains("/PCFl0 Do") && flat.contains("/PCFl1 Do"), "{flat}");
    // The rectangle's appearance is drawn at its rectangle (bbox = rect, identity mapping).
    assert!(flat.contains("q 1 0 0 1 0 0 cm /PCFl0 Do Q"), "{flat}");
    // The note's 20×20 icon box is mapped onto its rect at (200, 680).
    assert!(flat.contains("1 0 0 1 200 680 cm /PCFl1 Do"), "{flat}");
    let res = doc.resolve(p.dict.get(b"Resources").unwrap());
    let xo = doc.resolve(res.as_dict().unwrap().get(b"XObject").unwrap());
    assert!(xo.as_dict().unwrap().contains(b"PCFl1"));
    assert!(marks_present(&doc).is_empty(), "flattened content is not a removable mark");
}

/// #1317: a cloudy square this builder cannot draw stays on the page. A solid square, a hidden
/// cloudy square and a cloudy square that already has an appearance keep their previous results.
#[test]
fn flattening_keeps_a_cloudy_square_it_cannot_draw() {
    let page = stream("", " ");
    let form = "<< /Type /XObject /Subtype /Form /BBox [100 600 200 650] /Length 28 >>\nstream\n1 0 0 RG 100 600 100 50 re S\nendstream";
    let case = |extra: &str, drawn: usize, left: bool| {
        let annot = format!("<< /Type /Annot /Subtype /Square /Rect [100 600 200 650] /C [1 0 0] /Contents (Synthetic comment) {extra} >>");
        let mut doc = build(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << >> /Contents 5 0 R /Annots [4 0 R] >>",
            &annot,
            &page,
            form,
        ]);
        let before = streams(&doc, 0);
        assert_eq!(flatten(&mut doc, &[0], true, false).unwrap(), drawn);
        let doc = reopen(&doc);
        let p = &pdfcraft_model::pages(&doc)[0];
        assert_eq!(p.dict.contains(b"Annots"), left);
        if drawn == 0 {
            assert_eq!(streams(&doc, 0), before, "nothing was drawn in place of the comment");
        }
    };
    case("/BE << /S /C /I 2 >>", 0, true);
    case("/BE << /S /S >>", 1, false);
    case("/BE << /S /C /I 2 >> /F 2", 0, true);
    case("/BE << /S /C /I 2 >> /AP << /N 6 0 R >>", 1, false);
}

/// #805: a supported note with no stored appearance is still drawn by viewers. Flattening must
/// generate that appearance and bake it into the page, not delete a comment that shows an icon.
#[test]
fn flattening_keeps_a_note_whose_appearance_is_missing() {
    let page = stream("", " ");
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R /Annots [5 0 R] >>",
        &page,
        "<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /Contents (Synthetic note) /Name /Comment /Open false /F 0 >>",
    ]);
    assert_eq!(flatten(&mut doc, &[0], true, false).unwrap(), 1, "the note icon is drawn");
    let doc = reopen(&doc);
    let p = &pdfcraft_model::pages(&doc)[0];
    assert!(!p.dict.contains(b"Annots"), "the note is gone once its icon is baked in");
    let flat = streams(&doc, 0).pop().unwrap();
    assert!(flat.contains("q 1 0 0 1 10 10 cm /PCFl0 Do Q"), "{flat}");
}

/// #1470: a form field whose widget is on no page (left behind by page extraction or combining)
/// has nothing to draw, but flattening every form field still takes it out of `/AcroForm /Fields`,
/// or other software keeps treating the flattened file as a form. A hidden widget on a page is
/// left alone, as before.
#[test]
fn flattening_fields_drops_the_ones_on_no_page() {
    let page = stream("", " ");
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R 7 0 R 8 0 R] >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R /Annots [5 0 R 7 0 R] >>",
        &page,
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (on_page) /Rect [10 10 150 40] /F 4 /AP << /N 10 0 R >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (orphan) /Rect [10 50 150 80] /F 4 /AP << /N 10 0 R >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (hidden) /Rect [10 90 150 120] /F 2 /AP << /N 10 0 R >> >>",
        "<< /FT /Tx /T (parent) /Kids [9 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /Parent 8 0 R /Rect [10 130 150 160] /F 4 /AP << /N 10 0 R >> >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 140 30] /Length 0 >>\nstream\n\nendstream",
    ]);
    assert_eq!(flatten(&mut doc, &[0], false, true).unwrap(), 1, "only the field on the page is drawn");
    let doc = reopen(&doc);
    let root = doc.root().unwrap();
    let af = doc.resolve(doc.get(root).as_dict().unwrap().get(b"AcroForm").unwrap());
    let fields = doc.resolve(af.as_dict().unwrap().get(b"Fields").unwrap());
    let left: Vec<pdfcraft_cos::ObjRef> = fields.as_array().unwrap().iter().filter_map(|f| f.as_ref()).collect();
    assert_eq!(left, [pdfcraft_cos::ObjRef::new(7, 0)], "only the hidden field on the page stays");
}

/// #1794: a form field whose widget has no usable appearance (none stored, or none for its
/// selected state) has nothing to draw. Flattening used to remove it anyway, losing its value
/// with no ink in its place. It stays, in the page and in the form, like a comment that can't be
/// drawn; a field with an appearance is still baked in and removed.
#[test]
fn flattening_keeps_a_valued_widget_it_cannot_draw() {
    let page = stream("", " ");
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R 7 0 R] >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R /Annots [5 0 R 6 0 R 7 0 R] >>",
        &page,
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Ada) /Rect [10 10 150 40] /F 4 >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /V /Yes /AS /Yes /Rect [10 50 30 70] /F 4 /AP << /N << /Off 8 0 R >> >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (drawn) /V (Bob) /Rect [10 90 150 120] /F 4 /AP << /N 8 0 R >> >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 140 30] /Length 0 >>\nstream\n\nendstream",
    ]);
    assert_eq!(flatten(&mut doc, &[0], false, true).unwrap(), 1, "only the field with an appearance is drawn");
    let doc = reopen(&doc);
    let refs = |o: &Object| -> Vec<u32> { doc.resolve(o).as_array().unwrap().iter().filter_map(|f| f.as_ref()).map(|r| r.num).collect() };
    let p = &pdfcraft_model::pages(&doc)[0];
    assert_eq!(refs(p.dict.get(b"Annots").unwrap()), [5, 6], "the undrawable widgets stay on the page");
    let root = doc.root().unwrap();
    let af = doc.resolve(doc.get(root).as_dict().unwrap().get(b"AcroForm").unwrap());
    assert_eq!(refs(af.as_dict().unwrap().get(b"Fields").unwrap()), [5, 6], "and in the form");
    let value = |n: u32| doc.get(pdfcraft_cos::ObjRef::new(n, 0)).as_dict().unwrap().get(b"V").cloned();
    assert_eq!(value(5), Some(Object::String(pdfcraft_cos::PdfString::literal(b"Ada".to_vec()))));
    assert_eq!(value(6), Some(Object::name("Yes")));
}

/// #1794: a reply this builder cannot draw stays on the page even when the comment it replies to
/// is flattened; the comment's pop-up still goes with it.
#[test]
fn flattening_keeps_an_undrawn_reply_to_a_flattened_comment() {
    let page = stream("", " ");
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << >> /Contents 4 0 R /Annots [5 0 R 6 0 R 7 0 R] >>",
        &page,
        "<< /Type /Annot /Subtype /Square /Rect [100 600 200 650] /C [1 0 0] /Contents (Synthetic comment) /AP << /N 8 0 R >> /Popup 7 0 R >>",
        "<< /Type /Annot /Subtype /Square /Rect [100 500 200 550] /C [1 0 0] /Contents (Synthetic reply) /BE << /S /C /I 2 >> /IRT 5 0 R >>",
        "<< /Type /Annot /Subtype /Popup /Rect [300 600 400 700] /Parent 5 0 R >>",
        "<< /Type /XObject /Subtype /Form /BBox [100 600 200 650] /Length 28 >>\nstream\n1 0 0 RG 100 600 100 50 re S\nendstream",
    ]);
    assert_eq!(flatten(&mut doc, &[0], true, false).unwrap(), 1, "the comment is drawn; the cloudy reply cannot be");
    let doc = reopen(&doc);
    let p = &pdfcraft_model::pages(&doc)[0];
    let left: Vec<u32> = doc.resolve(p.dict.get(b"Annots").unwrap()).as_array().unwrap().iter().filter_map(|f| f.as_ref()).map(|r| r.num).collect();
    assert_eq!(left, [6], "the undrawn reply stays; the comment and its pop-up are gone");
}

/// Flattening never makes a `?` permanent: it refuses, changing nothing, a text box without an
/// appearance whose text Helvetica can't draw (another program's), and a comment or form field
/// whose appearance PdfCraft drew with `?` (marked `PCUndrawn`). A comment another program drew
/// with a font that has those characters (an appearance, no mark) flattens as before.
#[test]
fn flattening_refuses_what_would_bake_in_question_marks() {
    let page = stream("", " ");
    let refused = |annot: &str, comments: bool, fields: bool, what: &str| {
        let mut doc = build(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R /Annots [5 0 R] >>",
            &page,
            annot,
            "<< /Type /XObject /Subtype /Form /BBox [0 0 140 30] /Length 0 >>\nstream\n\nendstream",
        ]);
        let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
        let err = flatten(&mut doc, &[0], comments, fields).unwrap_err().to_string();
        assert!(err.contains(what) && err.contains("page 1") && err.contains("\"欢\" (U+6B22)") && err.contains("nothing was flattened"), "{err}");
        assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before, "a refused flatten changes nothing");
        assert!(pdfcraft_model::pages(&doc)[0].dict.contains(b"Annots"), "the annotation is still there");
    };
    refused(
        "<< /Type /Annot /Subtype /FreeText /Rect [10 10 150 40] /DA (0 g /Helv 12 Tf) /Contents <FEFF6B228FCE> /F 4 >>",
        true,
        false,
        "a comment",
    );
    refused(
        "<< /Type /Annot /Subtype /FreeText /Rect [10 10 150 40] /DA (0 g /Helv 12 Tf) /Contents <FEFF6B228FCE> /F 4 /AP << /N 6 0 R >> /PCUndrawn <FEFF6B22> >>",
        true,
        false,
        "a comment",
    );
    refused(
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V <FEFF6B228FCE> /Rect [10 10 150 40] /F 4 /AP << /N 6 0 R >> /PCUndrawn <FEFF6B22> >>",
        false,
        true,
        "a form field",
    );
    // Drawn by another program, with a font that has 欢: no mark, so it flattens.
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R /Annots [5 0 R] >>",
        &page,
        "<< /Type /Annot /Subtype /FreeText /Rect [10 10 150 40] /DA (0 g /Helv 12 Tf) /Contents <FEFF6B228FCE> /F 4 /AP << /N 6 0 R >> >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 140 30] /Length 0 >>\nstream\n\nendstream",
    ]);
    assert_eq!(flatten(&mut doc, &[0], true, false).unwrap(), 1);
}

/// Display space is in points, so on a `/UserUnit 2` page an item's box maps to half as many
/// user-space units. Items record the `/UserUnit` they were written for; one without it was
/// written when display space was in user-space units, and reads back scaled to points.
#[test]
fn added_items_follow_the_page_user_unit() {
    let mut doc = fixture();
    let page = pdfcraft_model::pages(&doc)[2].obj;
    doc.update_dict(page, |d| d.set(b"UserUnit".to_vec(), Object::Int(2))).unwrap();
    let text = AddedText { rect: [72.0, 600.0, 300.0, 700.0], text: "Scaled".into(), size: 14.0, ..AddedText::default() };
    add_content(&mut doc, 2, &Content::Text(text.clone())).unwrap();
    add_content(&mut doc, 0, &Content::Text(text.clone())).unwrap();
    let doc = reopen(&doc);
    let all = list_added(&doc);
    let on = |page: usize| all.iter().find(|a| a.page == page).unwrap();
    let Content::Text(t) = &on(2).content else { panic!() };
    assert_eq!((t.rect[0], t.rect[2], t.rect[3], t.size), (72.0, 300.0, 700.0, 14.0), "read back as written");
    assert!(streams(&doc, 2).iter().any(|s| s.starts_with("q 0.5 0 0 0.5 0 0 cm")), "display points to user space");
    let recorded = |doc: &Document, a: &Added| {
        doc.get(a.obj).as_dict().and_then(|d| d.get(b"PCAdded").cloned()).and_then(|p| p.as_dict().and_then(|p| p.get(b"UserUnit").cloned()))
    };
    assert!(recorded(&doc, on(0)).is_none(), "a page without /UserUnit records nothing");
    assert_eq!(recorded(&doc, on(2)).and_then(|u| u.as_f64()), Some(2.0));
    // The same item as an earlier version wrote it: no record, box and size in user-space units.
    let mut legacy = doc.clone();
    let Object::Stream(mut s) = (*legacy.get(on(2).obj)).clone() else { panic!() };
    let mut params = s.dict.get(b"PCAdded").and_then(|p| p.as_dict().cloned()).unwrap();
    params.remove(b"UserUnit");
    s.dict.set(b"PCAdded".to_vec(), Object::Dict(params));
    legacy.set(on(2).obj, Object::Stream(s));
    let Content::Text(old) = &list_added(&legacy).into_iter().find(|a| a.page == 2).unwrap().content else { panic!() };
    assert_eq!((old.rect[0], old.rect[2], old.rect[3], old.size), (144.0, 600.0, 1400.0, 28.0), "scaled to points");
}

fn tagged_dictionary(tag: &str, added: Option<Object>) -> Dict {
    let mut dict = Dict::new();
    dict.set(b"PCMark".to_vec(), Object::name(tag));
    if let Some(added) = added {
        dict.set(b"PCAdded".to_vec(), added);
    }
    dict
}

fn tagged_stream(doc: &mut Document, tag: &str, added: Option<Object>) -> Object {
    Object::Ref(doc.add(Object::Stream(Stream::from_raw(tagged_dictionary(tag, added), Vec::new()))))
}

fn added_text_params() -> Dict {
    let mut params = Dict::new();
    params.set(b"Rect".to_vec(), Object::Array([10, 20, 110, 40].into_iter().map(Object::Int).collect()));
    params.set(b"Kind".to_vec(), Object::name("Text"));
    params.set(b"Text".to_vec(), Object::String(pdfcraft_cos::PdfString::text("Added text")));
    params.set(b"Size".to_vec(), Object::Int(12));
    params
}

fn added_image_params(image: pdfcraft_cos::ObjRef) -> Dict {
    let mut params = Dict::new();
    params.set(b"Rect".to_vec(), Object::Array([10, 20, 110, 70].into_iter().map(Object::Int).collect()));
    params.set(b"Kind".to_vec(), Object::name("Image"));
    params.set(b"Image".to_vec(), Object::Ref(image));
    params
}

fn set_page_contents(doc: &mut Document, contents: Object, user_unit: Option<Object>) {
    let page = pdfcraft_model::pages(doc)[0].obj;
    doc.update_dict(page, |dict| {
        dict.set(b"Contents".to_vec(), contents);
        if let Some(user_unit) = user_unit {
            dict.set(b"UserUnit".to_vec(), user_unit);
        }
    })
    .unwrap();
}

fn assert_marker_scan_matches_legacy(doc: &Document, label: &str, marks: &[MarkKind], added: usize) -> MarkerScan {
    let expected_marks = marks_present(doc);
    let expected_added = list_added(doc);
    let scan = scan_markers(doc);
    assert_eq!(scan.marks, expected_marks, "{label}: marks differ from the independent scan");
    assert_eq!(scan.added, expected_added, "{label}: added content differs from the independent scan");
    assert_eq!(scan.marks, marks, "{label}: fixture's mark order or deduplication is wrong");
    assert_eq!(scan.added.len(), added, "{label}: fixture's added-item count is wrong");
    scan
}

#[test]
fn combined_marker_scan_matches_the_independent_scans() {
    let empty = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 >>", "<< /Type /Page /Parent 2 0 R >>"]);
    assert_marker_scan_matches_legacy(&empty, "empty page", &[], 0);
    assert_marker_scan_matches_legacy(&fixture(), "ordinary streams and a page without contents", &[], 0);

    for (tag, kind) in [("HeaderFooter", MarkKind::HeaderFooter), ("Watermark", MarkKind::Watermark), ("Background", MarkKind::Background)] {
        let mut doc = fixture();
        let marker = tagged_stream(&mut doc, tag, None);
        set_page_contents(&mut doc, marker, None);
        assert_marker_scan_matches_legacy(&doc, tag, &[kind], 0);
    }

    let mut all_marks = fixture();
    let background = tagged_stream(&mut all_marks, "Background", None);
    let watermark = tagged_stream(&mut all_marks, "Watermark", None);
    let header = tagged_stream(&mut all_marks, "HeaderFooter", None);
    set_page_contents(
        &mut all_marks,
        Object::Array(vec![background.clone(), watermark.clone(), header.clone(), watermark, header, background]),
        None,
    );
    assert_marker_scan_matches_legacy(&all_marks, "all mark kinds repeated", &[MarkKind::Background, MarkKind::Watermark, MarkKind::HeaderFooter], 0);

    let mut single_reference = fixture();
    let marker = tagged_stream(&mut single_reference, "Watermark", None);
    set_page_contents(&mut single_reference, marker, None);
    assert_marker_scan_matches_legacy(&single_reference, "contents as one reference", &[MarkKind::Watermark], 0);

    let mut text_only = fixture();
    let text = tagged_stream(&mut text_only, "Added", Some(Object::Dict(added_text_params())));
    set_page_contents(&mut text_only, text, None);
    let text_scan = assert_marker_scan_matches_legacy(&text_only, "one added text item", &[], 1);
    assert!(matches!(&text_scan.added[0].content, Content::Text(text) if text.text == "Added text"));

    let mut indirect_params = fixture();
    let params = indirect_params.add(Object::Dict(added_text_params()));
    let text = tagged_stream(&mut indirect_params, "Added", Some(Object::Ref(params)));
    set_page_contents(&mut indirect_params, text, None);
    let indirect_scan = assert_marker_scan_matches_legacy(&indirect_params, "indirect PCAdded dictionary", &[], 1);
    assert!(matches!(&indirect_scan.added[0].content, Content::Text(text) if text.text == "Added text"));

    let mut image_only = fixture();
    let image = image_only.add(Object::Stream(Stream::from_raw(Dict::new(), vec![0])));
    let image_item = tagged_stream(&mut image_only, "Added", Some(Object::Dict(added_image_params(image))));
    set_page_contents(&mut image_only, image_item, None);
    let image_scan = assert_marker_scan_matches_legacy(&image_only, "one added image item", &[], 1);
    assert!(matches!(&image_scan.added[0].content, Content::Image(item) if item.image == image));

    let mut interleaved = fixture();
    let text = tagged_stream(&mut interleaved, "Added", Some(Object::Dict(added_text_params())));
    let watermark = tagged_stream(&mut interleaved, "Watermark", None);
    let image = interleaved.add(Object::Stream(Stream::from_raw(Dict::new(), vec![0])));
    let image_item = tagged_stream(&mut interleaved, "Added", Some(Object::Dict(added_image_params(image))));
    let header = tagged_stream(&mut interleaved, "HeaderFooter", None);
    set_page_contents(&mut interleaved, Object::Array(vec![text, watermark, image_item, header]), None);
    let interleaved_scan =
        assert_marker_scan_matches_legacy(&interleaved, "marks and added content interleaved", &[MarkKind::Watermark, MarkKind::HeaderFooter], 2);
    assert!(matches!(&interleaved_scan.added[0].content, Content::Text(_)));
    assert!(matches!(&interleaved_scan.added[1].content, Content::Image(_)));

    let mut array_contents = fixture();
    let background = tagged_stream(&mut array_contents, "Background", None);
    let array_ref = array_contents.add(Object::Array(vec![background]));
    set_page_contents(&mut array_contents, Object::Ref(array_ref), None);
    assert_marker_scan_matches_legacy(&array_contents, "contents as an indirect array", &[MarkKind::Background], 0);

    let mut malformed = fixture();
    let indirect_mark = malformed.add(Object::Dict(tagged_dictionary("HeaderFooter", None)));
    let missing_added = tagged_stream(&mut malformed, "Added", None);
    let invalid_added = tagged_stream(&mut malformed, "Added", Some(Object::String(pdfcraft_cos::PdfString::text("bad params"))));
    let unknown = tagged_stream(&mut malformed, "FutureKind", None);
    let direct_watermark = Object::Dict(tagged_dictionary("Watermark", None));
    let direct_added = Object::Dict(tagged_dictionary("Added", Some(Object::Dict(added_text_params()))));
    set_page_contents(
        &mut malformed,
        Object::Array(vec![
            Object::Null,
            Object::Int(7),
            Object::name("not-a-stream"),
            direct_watermark,
            Object::Ref(indirect_mark),
            missing_added,
            invalid_added,
            unknown,
            direct_added,
        ]),
        None,
    );
    assert_marker_scan_matches_legacy(
        &malformed,
        "malformed, non-stream, unknown and direct entries",
        &[MarkKind::Watermark, MarkKind::HeaderFooter],
        0,
    );

    let mut shared = fixture();
    let added = tagged_stream(&mut shared, "Added", Some(Object::Dict(added_text_params())));
    let mark = tagged_stream(&mut shared, "Background", None);
    set_page_contents(&mut shared, Object::Array(vec![added.clone(), mark.clone(), added.clone(), mark]), None);
    let shared_scan = assert_marker_scan_matches_legacy(&shared, "repeated shared stream references", &[MarkKind::Background], 2);
    assert_eq!(shared_scan.added[0].obj, shared_scan.added[1].obj);

    let mut user_unit = fixture();
    let added = tagged_stream(&mut user_unit, "Added", Some(Object::Dict(added_text_params())));
    set_page_contents(&mut user_unit, added, Some(Object::Int(2)));
    let unit_scan = assert_marker_scan_matches_legacy(&user_unit, "non-1.0 UserUnit", &[], 1);
    let Content::Text(text) = &unit_scan.added[0].content else { panic!() };
    assert_eq!(text.rect, [20.0, 40.0, 220.0, 80.0]);
    assert_eq!(text.size, 24.0);
}

#[test]
fn added_text_and_images_are_page_content_that_stays_editable() {
    let mut doc = fixture();
    let text = AddedText { rect: [72.0, 600.0, 300.0, 700.0], text: "Approved by Ada\nSecond line".into(), size: 14.0, ..AddedText::default() };
    assert_eq!(add_content(&mut doc, 0, &Content::Text(text.clone())).unwrap(), 0);
    // An 1×1 gray image, placed on the rotated page.
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(1));
    d.set(b"Height".to_vec(), Object::Int(1));
    d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    let img = doc.add(Object::Stream(Stream::from_raw(d, vec![128])));
    add_content(&mut doc, 1, &Content::Image(AddedImage::new([10.0, 10.0, 110.0, 60.0], img))).unwrap();
    let doc2 = reopen(&doc);
    let all = list_added(&doc2);
    assert_eq!(all.len(), 2);
    let Content::Text(t) = &all[0].content else { panic!() };
    assert_eq!((t.text.as_str(), t.size), ("Approved by Ada\nSecond line", 14.0));
    assert_eq!(t.rect, [72.0, 700.0 - 2.0 * 14.0 * 1.2, 300.0, 700.0], "the box height follows the two lines");
    let page0 = streams(&doc2, 0).join("\n");
    assert!(page0.contains("(Approved by Ada) Tj") && page0.contains("(Second line) Tj") && page0.contains("/PCFHelvetica 14 Tf"), "{page0}");
    // The rotated page draws in display space: the view matrix comes first.
    let page1 = streams(&doc2, 1).join("\n");
    assert!(page1.contains("q 0 1 -1 0 600 0 cm") && page1.contains(&format!("/PCImg{} Do", img.num)), "{page1}");
    // Edit: move, restyle, retype; then delete.
    let mut doc = doc2;
    let moved = AddedText {
        rect: [100.0, 500.0, 300.0, 520.0],
        text: "Approved".into(),
        bold: true,
        family: Family::Times,
        align: Align::Right,
        color: [1.0, 0.0, 0.0],
        ..text
    };
    update_content(&mut doc, 0, 0, &Content::Text(moved)).unwrap();
    let page0 = streams(&doc, 0).join("\n");
    assert!(page0.contains("/PCFTimesBold 14 Tf 1 0 0 rg") && !page0.contains("Second line"), "{page0}");
    assert!(
        update_content(&mut doc, 1, 0, &Content::Text(AddedText { text: "x".into(), rect: [0.0, 0.0, 50.0, 10.0], ..AddedText::default() })).is_err(),
        "kinds don't change"
    );
    delete_content(&mut doc, 0, 0).unwrap();
    assert_eq!(list_added(&doc).len(), 1);
    assert!(!streams(&doc, 0).join("").contains("Approved"));
    assert!(add_content(&mut doc, 0, &Content::Text(AddedText { rect: [0.0, 0.0, 100.0, 10.0], ..AddedText::default() })).is_err(), "empty text");
    // Page marks ignore added items.
    assert!(marks_present(&doc).is_empty());
}

#[test]
fn added_arabic_text_is_shaped_in_display_order_and_stays_searchable() {
    let mut doc = fixture();
    let text =
        AddedText { rect: [72.0, 600.0, 400.0, 700.0], text: "مرحبا World".into(), size: 14.0, align: Align::Right, ..AddedText::default() };
    if pdfcraft_fonts::document_arabic_font().is_none() {
        eprintln!("skipping the Arabic drawing checks: built without a craft-fonts Arab face (set CRAFT_FONTS_DIR)");
        let err = add_content(&mut doc, 0, &Content::Text(text)).unwrap_err();
        assert!(matches!(&err, EditError::Invalid(m) if m.contains("CRAFT_FONTS_DIR")), "{err:?}");
        assert!(!doc.is_modified(), "nothing is written when the font is missing");
        return;
    }
    add_content(&mut doc, 0, &Content::Text(text.clone())).unwrap();
    // A second Arabic item on the same page keeps its own fonts.
    add_content(&mut doc, 0, &Content::Text(AddedText { text: "شكرا".into(), rect: [72.0, 500.0, 400.0, 520.0], ..text.clone() })).unwrap();
    let doc = reopen(&doc);
    let added = list_added(&doc);
    let Content::Text(t) = &added[0].content else { panic!() };
    assert_eq!(t.text, "مرحبا World", "the logical text is kept for editing");
    let s = streams(&doc, 0);
    let item = s.iter().find(|s| s.contains("(World) Tj")).unwrap();
    // A right-to-left paragraph: "World" is drawn left of the Arabic word.
    let x_before = |s: &str, at: usize| -> f64 {
        let tm = s[..at].rfind("1 0 0 1 ").unwrap() + 8;
        s[tm..].split(' ').next().unwrap().parse().unwrap()
    };
    let latin_x = x_before(item, item.find("(World) Tj").unwrap());
    let arabic_at = item.find("/PCAr").unwrap();
    let arabic_x = x_before(item, item[arabic_at..].find(" Tj").unwrap() + arabic_at);
    assert!(latin_x < arabic_x, "{item}");
    // The Arabic glyphs come from Type 3 fonts whose ToUnicode maps give back each letter.
    let p = &pdfcraft_model::pages(&doc)[0];
    let res = doc.resolve(p.dict.get(b"Resources").unwrap());
    let fonts = doc.resolve(res.as_dict().unwrap().get(b"Font").unwrap()).as_dict().unwrap().clone();
    let arabic: Vec<_> = fonts.iter().filter(|(k, _)| k.starts_with(b"PCAr")).map(|(_, v)| doc.resolve(v).as_dict().unwrap().clone()).collect();
    assert_eq!(arabic.len(), 2, "one font per item");
    let mut mapped = String::new();
    for f in &arabic {
        assert_eq!(f.name(b"Subtype"), Some(&b"Type3"[..]));
        mapped += &String::from_utf8_lossy(&stream_bytes(&doc, f.get(b"ToUnicode").unwrap()).unwrap());
    }
    for letter in ["0645", "0631", "062D", "0628", "0627", "0634", "0643"] {
        assert!(mapped.contains(&format!("<{letter}>")), "{letter} in {mapped}");
    }
}

#[test]
fn added_hebrew_text_is_drawn_right_to_left_and_stays_searchable() {
    let mut doc = fixture();
    let text = AddedText {
        rect: [72.0, 600.0, 400.0, 700.0],
        text: "שלום (עולם) 2026 World".into(),
        size: 14.0,
        align: Align::Right,
        ..AddedText::default()
    };
    if pdfcraft_fonts::document_hebrew_font(false).is_none() {
        eprintln!("skipping the Hebrew drawing checks: built without a craft-fonts Hebr face (set CRAFT_FONTS_DIR)");
        assert_eq!(first_undrawable(&text), Some('ש'));
        let err = add_content(&mut doc, 0, &Content::Text(text)).unwrap_err();
        assert!(matches!(&err, EditError::Invalid(m) if m.contains("U+05E9")), "{err:?}");
        assert!(!doc.is_modified(), "nothing is written when the font is missing");
        return;
    }
    assert_eq!(first_undrawable(&text), None);
    add_content(&mut doc, 0, &Content::Text(text.clone())).unwrap();
    // Points stay on their letters; a Hebrew line in an Arabic item and Arabic in a Hebrew one.
    for other in ["שָׁלוֹם", "שלום مرحبا"] {
        let other = AddedText { text: other.into(), rect: [72.0, 500.0, 400.0, 520.0], ..text.clone() };
        if other.text.chars().any(|c| ('\u{0600}'..='\u{06FF}').contains(&c)) && pdfcraft_fonts::document_arabic_font().is_none() {
            continue;
        }
        add_content(&mut doc, 0, &Content::Text(other)).unwrap();
    }
    let doc = reopen(&doc);
    let added = list_added(&doc);
    let Content::Text(t) = &added[0].content else { panic!() };
    assert_eq!(t.text, "שלום (עולם) 2026 World", "the logical text is kept for editing");
    let s = streams(&doc, 0);
    let item = s.iter().find(|s| s.contains("(World) Tj")).unwrap();
    // A right-to-left paragraph: "World", typed last, is drawn left of the Hebrew.
    let x_before = |s: &str, at: usize| -> f64 {
        let tm = s[..at].rfind("1 0 0 1 ").unwrap() + 8;
        s[tm..].split(' ').next().unwrap().parse().unwrap()
    };
    let latin_x = x_before(item, item.find("(World) Tj").unwrap());
    let hebrew_at = item.find("/PCAr").unwrap();
    let hebrew_x = x_before(item, item[hebrew_at..].find(" Tj").unwrap() + hebrew_at);
    assert!(latin_x < hebrew_x, "{item}");
    // Copy and search get the typing order back.
    let logical: String = t.text.encode_utf16().map(|u| format!("{u:04X}")).collect();
    assert!(item.contains(&format!("/ActualText <FEFF{logical}>")), "{item}");
    // The Hebrew glyphs come from a Type 3 font named after the face, whose ToUnicode map gives
    // back each letter.
    let p = &pdfcraft_model::pages(&doc)[0];
    let res = doc.resolve(p.dict.get(b"Resources").unwrap());
    let fonts = doc.resolve(res.as_dict().unwrap().get(b"Font").unwrap()).as_dict().unwrap().clone();
    let shaped: Vec<_> = fonts.iter().filter(|(k, _)| k.starts_with(b"PCAr")).map(|(_, v)| doc.resolve(v).as_dict().unwrap().clone()).collect();
    let mut mapped = String::new();
    for f in &shaped {
        assert_eq!(f.name(b"Subtype"), Some(&b"Type3"[..]));
        mapped += &String::from_utf8_lossy(&stream_bytes(&doc, f.get(b"ToUnicode").unwrap()).unwrap());
    }
    for letter in ["05E9", "05DC", "05D5", "05DD", "05E2"] {
        assert!(mapped.contains(&format!("<{letter}>")), "{letter} in {mapped}");
    }
    let font_name = |f: &Dict| doc.resolve(f.get(b"FontDescriptor").unwrap()).as_dict().unwrap().name(b"FontName").map(<[u8]>::to_vec);
    assert!(shaped.iter().any(|f| font_name(f).as_deref() == Some(&b"NotoSansHebrew"[..])));
    // A pointed letter is one glyph with its points, mapped back whole.
    assert!(mapped.contains("<05E905"), "{mapped}");
}

#[test]
fn odd_hebrew_text_never_panics() {
    let mut doc = fixture();
    let long = "שלום עולם ".repeat(2_000);
    for s in ["\u{202E}ש\u{05B8}\u{05B8} (]", "\u{05B8}", "א\u{200F}\u{2067}b\u{2069}", "א\n\nב\tc", "׳״ 456", long.as_str()] {
        let r = add_content(&mut doc, 0, &Content::Text(AddedText { rect: [0.0, 800.0, 300.0, 780.0], text: s.into(), ..AddedText::default() }));
        assert_eq!(r.is_ok(), pdfcraft_fonts::document_hebrew_font(false).is_some(), "{:?}: {r:?}", s.chars().take(12).collect::<String>());
    }
}

/// The `PCAr` font names in page `page`'s resources.
fn arabic_font_names(doc: &Document, page: usize) -> Vec<String> {
    let p = &pdfcraft_model::pages(doc)[page];
    let Some(res) = p.dict.get(b"Resources").map(|r| doc.resolve(r)) else { return vec![] };
    let Some(fonts) = res.as_dict().and_then(|r| r.get(b"Font")).map(|f| doc.resolve(f)) else { return vec![] };
    fonts.as_dict().unwrap().iter().map(|(k, _)| String::from_utf8_lossy(k).into_owned()).filter(|k| k.starts_with("PCAr")).collect()
}

#[test]
fn arabic_items_keep_their_own_fonts_through_saves_updates_and_deletes() {
    let mut doc = fixture();
    let text = AddedText { rect: [72.0, 600.0, 400.0, 700.0], text: "مرحبا".into(), size: 14.0, ..AddedText::default() };
    if pdfcraft_fonts::document_arabic_font().is_none() {
        // An item that already holds Arabic (stored by an older version, drawn as "?") can still
        // be moved and retyped without the face.
        add_content(&mut doc, 0, &Content::Text(AddedText { text: "x".into(), ..text.clone() })).unwrap();
        update_content(&mut doc, 0, 0, &Content::Text(text)).unwrap();
        assert!(streams(&doc, 0).join("").contains("(?????) Tj"));
        return;
    }
    // A full save renumbers objects but keeps resource names, so a font named after its object
    // number may meet a name already on the page: here, every name it could take.
    let mut taken = Dict::new();
    for num in 1..2_000 {
        taken.set(format!("PCAr{num}").into_bytes(), Object::name("Taken"));
    }
    let mut res = Dict::new();
    res.set(b"Font".to_vec(), Object::Dict(taken));
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(page, |d| d.set(b"Resources".to_vec(), Object::Dict(res))).unwrap();
    add_content(&mut doc, 0, &Content::Text(text.clone())).unwrap();
    add_content(&mut doc, 0, &Content::Text(AddedText { text: "شكرا".into(), rect: [72.0, 500.0, 400.0, 520.0], ..text.clone() })).unwrap();
    let names = arabic_font_names(&doc, 0);
    assert_eq!(names.len(), 1_999 + 2, "nothing replaced");
    let p = &pdfcraft_model::pages(&doc)[0];
    let fonts = doc.resolve(p.dict.get(b"Resources").unwrap()).as_dict().unwrap().get(b"Font").cloned().unwrap();
    let fonts = doc.resolve(&fonts).as_dict().unwrap().clone();
    assert_eq!(fonts.iter().filter(|(_, v)| v.as_name() == Some(&b"Taken"[..])).count(), 1_999);
    let first = own(&doc, 0);
    assert!(first.len() == 1 && first[0].contains('_'), "a free name, not a taken one: {first:?}");
    // Updating an item swaps its fonts; deleting it removes them.
    update_content(&mut doc, 0, 0, &Content::Text(AddedText { text: "أهلا".into(), ..text })).unwrap();
    let names = arabic_font_names(&doc, 0);
    assert!(names.len() == 2_001 && !names.contains(&first[0]), "the replaced item's font is gone");
    delete_content(&mut doc, 0, 0).unwrap();
    delete_content(&mut doc, 0, 0).unwrap();
    assert_eq!(arabic_font_names(&doc, 0).len(), 1_999, "only the page's own names are left");
    reopen(&doc);
}

/// The `PCAr` fonts the stream of added item `index` on page 0 shows text with.
fn own(doc: &Document, index: usize) -> Vec<String> {
    let s = String::from_utf8_lossy(&stream_bytes(doc, &Object::Ref(list_added(doc)[index].obj)).unwrap()).into_owned();
    let mut names: Vec<String> =
        s.split_whitespace().filter_map(|w| w.strip_prefix('/')).filter(|w| w.starts_with("PCAr")).map(String::from).collect();
    names.dedup();
    names
}

#[test]
fn odd_arabic_text_never_panics() {
    let mut doc = fixture();
    let long = "بسم الله ".repeat(2_000);
    for s in ["\u{202E}ب\u{064B}\u{064B} (]", "\u{064B}", "ا\u{200F}\u{2067}b\u{2069}", "ا\n\nب\tc", "ﷺ ١٢٣ 456", long.as_str()] {
        let r = add_content(&mut doc, 0, &Content::Text(AddedText { rect: [0.0, 800.0, 300.0, 780.0], text: s.into(), ..AddedText::default() }));
        assert_eq!(r.is_ok(), pdfcraft_fonts::document_arabic_font().is_some(), "{:?}: {r:?}", s.chars().take(12).collect::<String>());
    }
    // U+2029 and U+0085 (paragraph and line separators) have no WinAnsi code, so the standard font
    // would draw them as `?`: refused, by name, rather than written (#125). Laying it out still
    // mustn't panic.
    let r = add_content(
        &mut doc,
        0,
        &Content::Text(AddedText { rect: [0.0, 800.0, 300.0, 780.0], text: "ا\u{2029}ب\rc\u{85}د\n".into(), ..AddedText::default() }),
    );
    assert!(r.is_err(), "{r:?}");
    if pdfcraft_fonts::document_arabic_font().is_some() {
        assert!(matches!(&r, Err(EditError::Invalid(m)) if m.contains("U+2029") || m.contains("U+0085")), "{r:?}");
    }
    // A character the face lacks is an error that names it, not a box or a crash.
    if pdfcraft_fonts::document_arabic_font().is_some() && !pdfcraft_fonts::arabic_has('\u{FDFD}') {
        let r = add_content(
            &mut doc,
            0,
            &Content::Text(AddedText { rect: [0.0, 800.0, 300.0, 780.0], text: "ب \u{FDFD}".into(), ..AddedText::default() }),
        );
        assert!(matches!(&r, Err(EditError::Invalid(m)) if m.contains('\u{FDFD}')), "{r:?}");
    }
}

/// One page with Helvetica (WinAnsi) and a subset font that has only the glyphs it uses.
fn text_page(content: &str) -> Document {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        // Subset: glyphs for a (97) and b (98) only.
        "<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+Arial /FirstChar 97 /LastChar 99 /Widths [500 520 0] /Encoding /WinAnsiEncoding >>"
            .into(),
    ];
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

fn descriptor_text_page() -> Document {
    let content = "BT /F1 12 Tf 72 700 Td (ab) Tj ET";
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+NeutralFace /FirstChar 97 /LastChar 98 /Widths [500 500] /FontDescriptor 6 0 R /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /FontDescriptor /FontName /NeutralFace /Flags 262208 /ItalicAngle -12 /FontWeight 700 /Ascent 900 /Descent -250 >>".into(),
    ];
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

/// The Japanese fallback face comes from craft-fonts, an optional build input.
fn without_craft_fonts(test: &str) -> bool {
    if pdfcraft_fonts::document_japanese_font().is_some() {
        return false;
    }
    eprintln!("skipping {test}: built without craft-fonts (set CRAFT_FONTS_DIR to run it)");
    true
}

#[test]
fn japanese_line_uses_unicode_type3_fallback() {
    if without_craft_fonts("japanese_line_uses_unicode_type3_fallback") {
        return;
    }
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    let replacement = "25362738こんにちは、お元気ですか？ 2.3444";
    let result = text::replace_line(&mut doc, 0, 0, replacement).unwrap();
    assert_eq!(result.substituted.as_deref(), Some("BIZ UDPGothic Type3"));
    let bytes = page_content_bytes(&doc, 0);
    let content = String::from_utf8_lossy(&bytes);
    assert!(content.contains("/PCJp"), "{content}");
    let reopened = reopen(&doc);
    assert_eq!(text::text_lines(&reopened, 0).unwrap()[0].text, replacement);
}

fn styled_text_page(base: &str) -> Document {
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Original) Tj ET");
    let r = pdfcraft_cos::ObjRef::new(5, 0);
    let mut font = doc.get(r).as_dict().unwrap().clone();
    font.set(b"BaseFont".to_vec(), pdfcraft_cos::Object::name(base));
    doc.set(r, pdfcraft_cos::Object::Dict(font));
    doc
}

fn fallback_paths(doc: &Document) -> Vec<Vec<u8>> {
    let p = pdfcraft_model::pages(doc).swap_remove(0);
    let res = doc.resolve(p.dict.get(b"Resources").unwrap());
    let fonts = doc.resolve(res.as_dict().unwrap().get(b"Font").unwrap());
    let font = doc.resolve(fonts.as_dict().unwrap().get(b"PCJp").unwrap());
    let procs = doc.resolve(font.as_dict().unwrap().get(b"CharProcs").unwrap());
    procs
        .as_dict()
        .unwrap()
        .iter()
        .map(|(_, object)| match &*doc.resolve(object) {
            pdfcraft_cos::Object::Stream(s) => s.decoded().unwrap(),
            other => panic!("expected a glyph stream, got {other:?}"),
        })
        .collect()
}

#[test]
fn japanese_line_fallback_uses_real_source_family_and_weight() {
    if without_craft_fonts("japanese_line_fallback_uses_real_source_family_and_weight") {
        return;
    }
    let mut sans = styled_text_page("Helvetica");
    let mut bold = descriptor_text_page();
    let mut serif = styled_text_page("ABCDEF+ShipporiMincho-Regular");
    let replacement = "見本商会　御中";
    text::replace_line(&mut sans, 0, 0, replacement).unwrap();
    text::replace_line(&mut bold, 0, 0, replacement).unwrap();
    text::replace_line(&mut serif, 0, 0, replacement).unwrap();
    assert_ne!(fallback_paths(&sans), fallback_paths(&serif), "sans source must not use Mincho outlines");
    if pdfcraft_fonts::CRAFT_FONTS.iter().any(|f| f.family == "BIZ UDPGothic" && f.style == "Bold") {
        assert_ne!(fallback_paths(&sans), fallback_paths(&bold), "bold must change the actual glyph paths");
    }
    for doc in [&sans, &bold, &serif] {
        assert_eq!(text::text_lines(&reopen(doc), 0).unwrap()[0].text, replacement);
    }
}

#[test]
fn japanese_paragraph_fallback_honors_requested_family_and_weight() {
    if without_craft_fonts("japanese_paragraph_fallback_honors_requested_family_and_weight") {
        return;
    }
    let replacement = "見本商会　御中";
    let mut regular = styled_text_page("Helvetica");
    let mut bold = styled_text_page("Helvetica");
    let mut serif = styled_text_page("Helvetica");
    text::replace_block(&mut regular, 0, 0, replacement).unwrap();
    text::rewrite_block(
        &mut bold,
        0,
        0,
        Some(replacement),
        &text::BlockStyle { family: Some((added::Family::Helvetica, true, false)), ..Default::default() },
    )
    .unwrap();
    text::rewrite_block(
        &mut serif,
        0,
        0,
        Some(replacement),
        &text::BlockStyle { family: Some((added::Family::Times, false, false)), ..Default::default() },
    )
    .unwrap();
    assert_ne!(fallback_paths(&regular), fallback_paths(&serif), "requested serif must change actual outlines");
    if pdfcraft_fonts::CRAFT_FONTS.iter().any(|f| f.family == "BIZ UDPGothic" && f.style == "Bold") {
        assert_ne!(fallback_paths(&regular), fallback_paths(&bold), "requested bold must change actual outlines");
        let mut source_bold = styled_text_page("Helvetica-Bold");
        text::replace_block(&mut source_bold, 0, 0, replacement).unwrap();
        assert_eq!(fallback_paths(&bold), fallback_paths(&source_bold));
    }
    for doc in [&regular, &bold, &serif] {
        assert_eq!(text::text_blocks(&reopen(doc), 0).unwrap()[0].text, replacement);
    }
}

#[test]
fn japanese_fallback_reports_the_face_after_save_and_reopen() {
    if without_craft_fonts("japanese_fallback_reports_the_face_after_save_and_reopen") {
        return;
    }
    // F2 is (a subset of) Arial: a regular sans face, so the fallback is the regular Gothic.
    let face = pdfcraft_fonts::document_japanese_font_for_style(false, false).unwrap();
    let expected = format!("{}-{}", face.family, face.style).replace(' ', "");
    for paragraph in [false, true] {
        let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
        let replacement = "見本商会　御中";
        if paragraph {
            text::replace_block(&mut doc, 0, 0, replacement).unwrap();
        } else {
            text::replace_line(&mut doc, 0, 0, replacement).unwrap();
        }
        for d in [&doc, &reopen(&doc)] {
            let lines = text::text_lines(d, 0).unwrap();
            assert_eq!(lines[0].text, replacement);
            assert_eq!(lines[0].base_font, expected, "report the real fallback face, not an empty BaseFont");
            assert_eq!(text::text_blocks(d, 0).unwrap()[0].base_font, expected);
        }
    }
}

#[test]
fn cyrillic_in_a_serif_line_falls_back_to_a_face_that_has_it() {
    if without_craft_fonts("cyrillic_in_a_serif_line_falls_back_to_a_face_that_has_it") {
        return;
    }
    let replacement = "Привет, world";
    let has_all = |f: &pdfcraft_fonts::CraftFont| replacement.chars().all(|ch| pdfcraft_fonts::japanese_glyph_from(f, ch).is_ok());
    if !pdfcraft_fonts::CRAFT_FONTS.iter().any(|f| f.covers("Jpan") && has_all(f)) {
        eprintln!("skipping: no Japanese fallback face has Cyrillic");
        return;
    }
    for paragraph in [false, true] {
        let mut doc = styled_text_page("Times-Roman");
        if paragraph {
            text::replace_block(&mut doc, 0, 0, replacement).unwrap();
        } else {
            text::replace_line(&mut doc, 0, 0, replacement).unwrap();
        }
        assert_eq!(text::text_lines(&reopen(&doc), 0).unwrap()[0].text, replacement);
    }
}

#[test]
fn japanese_paragraph_uses_unicode_type3_fallback() {
    if without_craft_fonts("japanese_paragraph_uses_unicode_type3_fallback") {
        return;
    }
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    let replacement = "こんにちは、お元気ですか？ 2.3444 日本語の文章";
    text::replace_block(&mut doc, 0, 0, replacement).unwrap();
    let reopened = reopen(&doc);
    assert_eq!(text::text_blocks(&reopened, 0).unwrap()[0].text, replacement);
}

/// The first `n` Japanese characters, in code point order, that every craft-fonts face can draw, so
/// whichever face an edit picks has them all. Without craft-fonts there are no faces, every character
/// passes, and the text is still long.
fn covered_japanese_chars(n: usize) -> String {
    let faces = pdfcraft_fonts::document_japanese_fonts_for_style(false, false);
    (0x4E00..0xA000)
        .filter_map(char::from_u32)
        .filter(|ch| faces.iter().all(|face| pdfcraft_fonts::japanese_glyph_from(face, *ch).is_ok()))
        .take(n)
        .collect()
}

/// The page's Type 3 fallback fonts (`PCJp`, `PCJp1`, ...) by resource name.
fn fallback_fonts(doc: &Document) -> Vec<(Vec<u8>, Dict)> {
    let p = pdfcraft_model::pages(doc).swap_remove(0);
    let res = doc.resolve(p.dict.get(b"Resources").unwrap());
    let fonts = doc.resolve(res.as_dict().unwrap().get(b"Font").unwrap());
    fonts
        .as_dict()
        .unwrap()
        .iter()
        .filter(|(name, _)| name.starts_with(b"PCJp"))
        .map(|(name, font)| (name.clone(), doc.resolve(font).as_dict().unwrap().clone()))
        .collect()
}

/// More unique fallback glyphs than one Type 3 font holds (240) are split across several generated
/// fonts, each within that limit, and the text still reads back as one line.
#[test]
fn japanese_replacement_over_240_unique_glyphs_is_split_across_fallback_fonts() {
    let replacement = covered_japanese_chars(300);
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    if without_craft_fonts("japanese_replacement_over_240_unique_glyphs_is_split_across_fallback_fonts") {
        // Without craft-fonts the long text gets the clear error, not the old limit message.
        let err = text::replace_line(&mut doc, 0, 0, &replacement).unwrap_err().to_string();
        assert!(err.contains("CRAFT_FONTS_DIR"), "{err}");
        return;
    }
    text::replace_line(&mut doc, 0, 0, &replacement).unwrap();
    let reopened = reopen(&doc);
    let lines = text::text_lines(&reopened, 0).unwrap();
    assert_eq!(lines.len(), 1, "one line: {:?}", lines.iter().map(|l| &l.text).collect::<Vec<_>>());
    assert_eq!(lines[0].text, replacement);
    let fonts = fallback_fonts(&reopened);
    assert_eq!(fonts.len(), 2, "300 glyphs take a 240-glyph font and a 60-glyph font");
    let mut glyphs = 0;
    for (name, font) in &fonts {
        let procs = reopened.resolve(font.get(b"CharProcs").unwrap());
        let count = procs.as_dict().unwrap().len();
        let last = font.get(b"LastChar").and_then(|o| o.as_int()).unwrap_or(i64::MAX);
        assert!(count <= 240 && last <= 240, "{name:?}: {count} glyphs, LastChar {last}");
        glyphs += count;
    }
    assert_eq!(glyphs, 300);
}

/// A replacement whose last glyph no face draws (in a later font) changes nothing: neither the
/// document nor a snapshot taken before the edit, which is what undo restores.
#[test]
fn japanese_replacement_missing_a_glyph_after_240_changes_nothing() {
    if without_craft_fonts("japanese_replacement_missing_a_glyph_after_240_changes_nothing") {
        return;
    }
    // The first CJK character that no face draws: the 242nd unique character, in a second font.
    let faces = pdfcraft_fonts::document_japanese_fonts_for_style(false, false);
    let missing = (0x4E00u32..0x9FFF)
        .filter_map(char::from_u32)
        .find(|ch| faces.iter().all(|face| pdfcraft_fonts::japanese_glyph_from(face, *ch).is_err()))
        .expect("some CJK character is outside every face");
    let replacement = format!("{}{missing}", covered_japanese_chars(241));
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let snapshot = doc.clone();
    let err = text::replace_line(&mut doc, 0, 0, &replacement).unwrap_err().to_string();
    assert!(err.contains(&format!("U+{:04X}", missing as u32)), "{err}");
    assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), saved);
    assert_eq!(write_incremental(&snapshot, &SaveOptions::default()).unwrap(), saved);
}

/// Runs that change font move nothing: with two glyphs per font the text reads back the same, in the
/// same box as with one font, and the font is switched only where the text moves to another font.
#[test]
fn japanese_fallback_split_across_fonts_keeps_order_and_positions() {
    if without_craft_fonts("japanese_fallback_split_across_fonts_keeps_order_and_positions") {
        return;
    }
    // 見 and 商 are in the first font, 御 in the second: runs [見商], [御], [見], three switches.
    let replacement = "見商御見";
    let mut whole = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    let mut split = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    text::replace_line_with_limit(&mut whole, 0, 0, replacement, 240).unwrap();
    text::replace_line_with_limit(&mut split, 0, 0, replacement, 2).unwrap();
    let (whole, split) = (reopen(&whole), reopen(&split));
    assert_eq!(fallback_fonts(&whole).len(), 1);
    assert_eq!(fallback_fonts(&split).len(), 2);
    let (one, many) = (text::text_lines(&whole, 0).unwrap(), text::text_lines(&split, 0).unwrap());
    assert_eq!(many.len(), 1, "one line: {:?}", many.iter().map(|l| &l.text).collect::<Vec<_>>());
    assert_eq!(many[0].text, replacement);
    for (a, b) in one[0].rect.iter().zip(many[0].rect.iter()) {
        assert!((a - b).abs() < 1e-6, "box {:?} vs {:?}", one[0].rect, many[0].rect);
    }
    // One `Tf` per run plus the page's own switch to F2 and the switch back after the text.
    let content = String::from_utf8_lossy(&page_content_bytes(&split, 0)).into_owned();
    assert_eq!(content.matches(" Tf").count(), 5, "{content}");
    let content = String::from_utf8_lossy(&page_content_bytes(&whole, 0)).into_owned();
    assert_eq!(content.matches(" Tf").count(), 3, "{content}");
}

/// Only a change of font gets a `Tf`: alternating characters with one glyph per font switch at every
/// character, and a run of one font's characters switches nowhere.
#[test]
fn japanese_fallback_switches_font_only_between_runs() {
    if without_craft_fonts("japanese_fallback_switches_font_only_between_runs") {
        return;
    }
    let alternating = "見商".repeat(500);
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    text::replace_line_with_limit(&mut doc, 0, 0, &alternating, 1).unwrap();
    let reopened = reopen(&doc);
    assert_eq!(text::text_lines(&reopened, 0).unwrap()[0].text, alternating);
    let content = String::from_utf8_lossy(&page_content_bytes(&reopened, 0)).into_owned();
    assert_eq!(content.matches(" Tf").count(), 1000 + 2, "1000 runs, the F2 switch before and after");
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    text::replace_line_with_limit(&mut doc, 0, 0, "見見見", 1).unwrap();
    let content = String::from_utf8_lossy(&page_content_bytes(&reopen(&doc), 0)).into_owned();
    assert_eq!(content.matches(" Tf").count(), 1 + 1 + 1, "{content}");
}

/// A paragraph set in several fallback fonts reads back as one block with the same text, and it
/// wraps across lines.
#[test]
fn japanese_paragraph_split_across_fonts_reads_back_as_one_block() {
    if without_craft_fonts("japanese_paragraph_split_across_fonts_reads_back_as_one_block") {
        return;
    }
    // Spaces are glyphs too, so two glyphs per font needs several fonts; the narrow box wraps the text.
    let replacement = "見 商 御 見 商";
    let style = text::BlockStyle { width: Some(40.0), ..Default::default() };
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    text::rewrite_block_with_limit(&mut doc, 0, 0, Some(replacement), &style, 2).unwrap();
    let reopened = reopen(&doc);
    assert!(fallback_fonts(&reopened).len() >= 2);
    let blocks = text::text_blocks(&reopened, 0).unwrap();
    assert_eq!(blocks.len(), 1, "one paragraph: {:?}", blocks.iter().map(|b| &b.text).collect::<Vec<_>>());
    assert_eq!(blocks[0].text, replacement);
    assert!(blocks[0].lines.len() > 1, "wrapped to {} line(s)", blocks[0].lines.len());
}

/// A paragraph whose move or width is refused (not finite, or drawn in a space it cannot be moved in)
/// leaves the document as it was: the fallback fonts are written only once the edit is accepted.
#[test]
fn japanese_paragraph_refused_before_its_fallback_fonts_are_written() {
    if without_craft_fonts("japanese_paragraph_refused_before_its_fallback_fonts_are_written") {
        return;
    }
    let plain = "BT /F2 12 Tf 72 700 Td (ab) Tj ET";
    let cases = [
        (plain, text::BlockStyle { offset: Some([f64::NAN, 0.0]), ..Default::default() }, "can't be moved that far"),
        (plain, text::BlockStyle { width: Some(f64::INFINITY), ..Default::default() }, "width must be a number"),
        (
            "0 0 0 0 0 0 cm BT /F2 12 Tf 72 700 Td (ab) Tj ET",
            text::BlockStyle { offset: Some([5.0, 0.0]), ..Default::default() },
            "can't be moved in",
        ),
    ];
    for (content, style, message) in cases {
        let mut doc = text_page(content);
        let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
        let err = text::rewrite_block(&mut doc, 0, 0, Some("見本商会　御中"), &style).unwrap_err().to_string();
        assert!(err.contains(message), "{err}");
        assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), saved, "{message}: the document changed");
    }
}

/// Every Type 3 fallback glyph starts with a well-formed `d1`: six operands, the second 0, and a
/// box that encloses every point of the glyph. Acrobat shows a bullet for a glyph whose `d1`
/// has the wrong operand count.
#[test]
fn type3_fallback_glyphs_declare_a_well_formed_d1() {
    if without_craft_fonts("type3_fallback_glyphs_declare_a_well_formed_d1") {
        return;
    }
    // Cyrillic is respaced (shifted by its side bearing), and its box must move with it.
    let mut glyphs = Vec::new();
    for replacement in ["見本商会 御中", "София 2027"] {
        let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
        text::replace_line(&mut doc, 0, 0, replacement).unwrap();
        glyphs.extend(fallback_paths(&reopen(&doc)));
    }
    assert!(!glyphs.is_empty());
    for glyph in glyphs {
        let text = String::from_utf8(glyph).unwrap();
        let mut lines = text.lines();
        let d1: Vec<&str> = lines.next().unwrap().split_whitespace().collect();
        assert_eq!(d1.len(), 7, "six operands and d1: {d1:?}");
        assert_eq!(d1[6], "d1");
        let n: Vec<f64> = d1[..6].iter().map(|v| v.parse().unwrap()).collect();
        assert_eq!(n[1], 0.0, "wy");
        assert!(n[2] <= n[4] && n[3] <= n[5], "box: {n:?}");
        for line in lines.filter(|l| l.ends_with(" m") || l.ends_with(" l")) {
            let p: Vec<f64> = line.split_whitespace().take(2).map(|v| v.parse().unwrap()).collect();
            assert!(p[0] >= n[2] && p[0] <= n[4] && p[1] >= n[3] && p[1] <= n[5], "{line} outside {n:?}");
        }
    }
}

/// Shippori Mincho, the serif fallback, has no Cyrillic: Bulgarian replacement text in a serif
/// line takes a fallback face that has every letter instead of failing on the first one.
#[test]
fn cyrillic_replacement_in_a_serif_line_uses_a_face_with_the_letters() {
    if without_craft_fonts("cyrillic_replacement_in_a_serif_line_uses_a_face_with_the_letters") {
        return;
    }
    let replacement = "София 2027";
    for paragraph in [false, true] {
        let mut doc = styled_text_page("ABCDEF+Garamond");
        if paragraph {
            text::replace_block(&mut doc, 0, 0, replacement).unwrap();
        } else {
            text::replace_line(&mut doc, 0, 0, replacement).unwrap();
        }
        let lines = text::text_lines(&reopen(&doc), 0).unwrap();
        assert_eq!(lines[0].text, replacement);
        // Letters are spaced by their shape, not one em each (the faces' full-width Cyrillic).
        let widths = fallback_widths(&doc);
        assert!(widths.iter().all(|w| *w > 300.0 && *w < 900.0), "{widths:?}");
        assert!(widths.windows(2).any(|w| w[0] != w[1]), "proportional: {widths:?}");
    }
}

/// The `/Widths` of the page's Type 3 fallback font, for its Cyrillic letters.
fn fallback_widths(doc: &Document) -> Vec<f64> {
    let p = pdfcraft_model::pages(doc).swap_remove(0);
    let res = doc.resolve(p.dict.get(b"Resources").unwrap());
    let fonts = doc.resolve(res.as_dict().unwrap().get(b"Font").unwrap());
    let font = doc.resolve(fonts.as_dict().unwrap().get(b"PCJp").unwrap());
    let font = font.as_dict().unwrap();
    let widths = doc.resolve(font.get(b"Widths").unwrap()).as_array().unwrap().iter().filter_map(|w| w.as_f64()).collect::<Vec<_>>();
    // "София 2027": the first five codes are the letters.
    widths[..5].to_vec()
}

#[test]
fn japanese_paragraph_keeps_ideographic_spaces() {
    if without_craft_fonts("japanese_paragraph_keeps_ideographic_spaces") {
        return;
    }
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Original paragraph) Tj ET");
    let replacement = "　見本商会　　御中　";
    let style = text::BlockStyle { width: Some(400.0), ..Default::default() };
    text::rewrite_block(&mut doc, 0, 0, Some(replacement), &style).unwrap();
    let reopened = reopen(&doc);
    let lines = text::text_lines(&reopened, 0).unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, replacement);
    assert_eq!(text::text_blocks(&reopened, 0).unwrap()[0].text, replacement);
    // Restyling the saved paragraph without new text must also preserve its spacing.
    let mut doc = reopened;
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { size: Some(14.0), ..style }).unwrap();
    assert_eq!(text::text_lines(&reopen(&doc), 0).unwrap()[0].text, replacement);
}

/// Without craft-fonts there is no Japanese face: editing in Japanese is a clear error that
/// leaves the page untouched, never a panic; Latin edits work as before.
#[test]
fn japanese_edit_without_craft_fonts_is_a_clear_error() {
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    let before = page_content_bytes(&doc, 0);
    let line = text::replace_line(&mut doc, 0, 0, "日本語の文字");
    let block = text::replace_block(&mut doc, 0, 0, "日本語の文字");
    if pdfcraft_fonts::document_japanese_font().is_some() {
        eprintln!("built with craft-fonts: the Japanese edits succeed (checked by the tests above)");
        assert!(line.is_ok() && block.is_ok());
        return;
    }
    for result in [line.map(|_| ()), block.map(|_| ())] {
        let Err(EditError::Invalid(msg)) = result else { panic!("expected a clear error, got {result:?}") };
        assert!(msg.contains("Japanese fallback font") && msg.contains("CRAFT_FONTS_DIR"), "{msg}");
    }
    assert_eq!(page_content_bytes(&doc, 0), before);
    let latin = text::replace_line(&mut doc, 0, 0, "Hello").unwrap();
    assert_eq!(text::text_lines(&reopen(&doc), 0).unwrap()[0].text, "Hello", "{latin:?}");
}
#[test]
fn font_descriptor_style_is_exposed_even_with_a_neutral_name() {
    let doc = descriptor_text_page();
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].bold && lines[0].italic, "{:?}", lines[0]);
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert!(blocks[0].bold && blocks[0].italic, "{:?}", blocks[0]);
}

#[test]
fn mixed_font_runs_are_not_merged_into_one_source_style() {
    let doc = text_page("BT /F1 12 Tf 72 700 Td (Regular) Tj /F2 12 Tf 150 700 Td (Subset) Tj ET");
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 2);
    assert_ne!(lines[0].font, lines[1].font);
}

#[test]
fn missing_glyphs_preserve_source_style_in_fallback() {
    let mut doc = descriptor_text_page();
    let result = text::replace_line(&mut doc, 0, 0, "Styled €").unwrap();
    assert_eq!(result.substituted.as_deref(), Some("Helvetica-BoldOblique"));
    let bytes = page_content_bytes(&doc, 0);
    let content = String::from_utf8_lossy(&bytes);
    assert!(content.contains("/PCEdHelveticaBoldOblique 12 Tf"), "{content}");
}

#[test]
fn a_subset_without_a_space_glyph_keeps_its_font_when_editing_words() {
    for paragraph in [false, true] {
        let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj /F1 12 Tf ( ) Tj /F2 12 Tf (ba) Tj ET");
        assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].text, "ab ba");
        let edit = if paragraph { text::replace_block(&mut doc, 0, 0, "ba ab ba") } else { text::replace_line(&mut doc, 0, 0, "ba ab ba") };
        assert_eq!(edit.unwrap().substituted, None);
        let lines = text::text_lines(&reopen(&doc), 0).unwrap();
        assert_eq!(lines[0].text, "ba ab ba");
        assert_eq!(lines[0].font, "F2");
        assert!((lines[0].rect[2] - lines[0].rect[0]) > 3.0 * 12.0);
    }
}

/// A Chrome (Skia) style subset: Type0/Identity-H over a TrueType program, CID = glyph id.
/// `/W` lists most used glyphs, writes 0 for some it dropped, and leaves `a` and `y` to `/DW`;
/// `z` has a ToUnicode entry but no glyph in the program (its CID is past the last glyph).
fn skia_subset_page() -> Document {
    // Inter's glyph ids: P 375, o 790, l 736, a 507, n 773, d 597, G 141, e 614, r 852, m 766, y 998, space 1777.
    let mut doc = text_page(
        "BT /F3 12 Tf 72 700 Td <0177031602E001FB03050255> Tj ET \
         BT /F3 12 Tf 72 650 Td <008D0266035402FE01FB030503E606F101FB0305025506F102FE031603540266> Tj ET",
    );
    let program = doc.add(Object::Stream(Stream::flate(Dict::new(), include_bytes!("../../../assets/fonts/Inter-Regular.ttf"))));
    let unicode = doc.add(Object::Stream(Stream::flate(
        Dict::new(),
        b"1 begincodespacerange <0000> <FFFF> endcodespacerange 13 beginbfchar <0177> <0050> <0316> <006F> <02E0> <006C> \
          <01FB> <0061> <0305> <006E> <0255> <0064> <008D> <0047> <0266> <0065> <0354> <0072> <02FE> <006D> <03E6> <0079> \
          <06F1> <0020> <1000> <007A> endbfchar",
    )));
    let widths: Vec<Object> = [(0x8D, 746), (0x177, 639), (0x255, 612), (0x266, 583), (0x2E0, 242), (0x2FE, 876), (0x305, 591), (0x316, 600)]
        .into_iter()
        .chain([(0x354, 376), (0x355, 0), (0x356, 0), (0x6F1, 281)])
        .flat_map(|(cid, width)| [Object::Int(cid), Object::Array(vec![Object::Int(width)])])
        .collect();
    let descriptor = Dict::from_iter([
        (b"Type".to_vec(), Object::name("FontDescriptor")),
        (b"FontName".to_vec(), Object::name("BAAAAA+SampleSans")),
        (b"Flags".to_vec(), Object::Int(32)),
        (b"FontFile2".to_vec(), Object::Ref(program)),
    ]);
    let cid_font = Dict::from_iter([
        (b"Type".to_vec(), Object::name("Font")),
        (b"Subtype".to_vec(), Object::name("CIDFontType2")),
        (b"BaseFont".to_vec(), Object::name("BAAAAA+SampleSans")),
        (b"CIDToGIDMap".to_vec(), Object::name("Identity")),
        (b"DW".to_vec(), Object::Int(562)),
        (b"W".to_vec(), Object::Array(widths)),
        (b"FontDescriptor".to_vec(), Object::Dict(descriptor)),
    ]);
    let font = Dict::from_iter([
        (b"Type".to_vec(), Object::name("Font")),
        (b"Subtype".to_vec(), Object::name("Type0")),
        (b"BaseFont".to_vec(), Object::name("BAAAAA+SampleSans")),
        (b"Encoding".to_vec(), Object::name("Identity-H")),
        (b"DescendantFonts".to_vec(), Object::Array(vec![Object::Dict(cid_font)])),
        (b"ToUnicode".to_vec(), Object::Ref(unicode)),
    ]);
    let p = pdfcraft_model::pages(&doc)[0].obj;
    let fonts = Dict::from_iter([(b"F3".to_vec(), Object::Dict(font))]);
    doc.update_dict(p, |d| d.set(b"Resources".to_vec(), Object::Dict(Dict::from_iter([(b"Font".to_vec(), Object::Dict(fonts))])))).unwrap();
    doc
}

#[test]
fn a_subset_keeps_its_font_for_glyphs_left_to_the_default_width() {
    let doc = skia_subset_page();
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Poland", "Germany and more"]);
    // Deleting words only, and replacing a word with one drawn elsewhere on the page (`y` and `a`
    // have no /W entry): every glyph is in the subset, so the font stays.
    for (block, text) in [(1, "Germany"), (0, "Germany")] {
        let mut doc = doc.clone();
        assert_eq!(text::replace_block(&mut doc, 0, block, text).unwrap().substituted, None, "{text}");
        let lines = text::text_lines(&reopen(&doc), 0).unwrap();
        assert_eq!(lines.iter().find(|l| l.text == text).unwrap().font, "F3");
    }
    // `z` maps to a CID the program has no glyph for: that still needs another font.
    let mut doc = doc.clone();
    assert!(text::replace_block(&mut doc, 0, 0, "Graz").unwrap().substituted.is_some());
}

#[test]
fn spaces_in_another_subset_do_not_split_a_line() {
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Hello) Tj /F2 12 Tf ( ) Tj /F1 12 Tf (world) Tj ET");
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].text, "Hello world");
    text::replace_block(&mut doc, 0, 0, "Goodbye").unwrap();
    assert_eq!(text::text_blocks(&reopen(&doc), 0).unwrap()[0].text, "Goodbye");
}

#[test]
fn editor_background_removes_glyphs_but_preserves_following_text_advance() {
    for show in ["(Hello) Tj", "[(He) -20 (llo)] TJ", "(Hello) '", "2 1 (Hello) \""] {
        let mut doc = text_page(&format!("BT /F1 12 Tf 14 TL 72 700 Td {show} [-3000] TJ /F2 12 Tf (ab) Tj ET"));
        let before = text::text_lines(&doc, 0).unwrap();
        assert_eq!(before.len(), 2);
        text::clear_block(&mut doc, 0, 0).unwrap();
        let after = text::text_lines(&reopen(&doc), 0).unwrap();
        assert_eq!(after.len(), 1, "{show}");
        assert_eq!(after[0].text, "ab");
        for (a, b) in after[0].rect.iter().zip(before[1].rect) {
            assert!((a - b).abs() < 1e-6, "unselected text moved: {show}");
        }
    }
}

#[test]
fn editor_background_refuses_clipping_text_without_mutation() {
    let mut doc = text_page("BT /F1 12 Tf 7 Tr 72 700 Td (Clip) Tj ET");
    let before = page_content_bytes(&doc, 0);
    assert!(text::clear_block(&mut doc, 0, 0).is_err());
    assert_eq!(page_content_bytes(&doc, 0), before);
}

#[test]
fn explicit_colour_spaces_expose_the_text_colour() {
    let doc = text_page("/DeviceRGB cs 0 0.23 0.45 sc BT /F1 12 Tf 72 700 Td (Blue) Tj ET");
    assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].color, [0.0, 0.23, 0.45]);
}

#[test]
fn words_in_separate_text_objects_form_one_editable_paragraph() {
    let mut doc = text_page(
        "BT /F1 12 Tf 1 0 0 1 72 700 Tm (Hello) Tj ET \
         BT /F1 12 Tf 1 0 0 1 103 700 Tm (world) Tj ET \
         BT /F1 12 Tf 1 0 0 1 72 685 Tm (Second) Tj ET \
         BT /F1 12 Tf 1 0 0 1 117 685 Tm (line) Tj ET \
         BT /F1 12 Tf 1 0 0 1 300 700 Tm (Other column) Tj ET",
    );
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].text, "Hello world Second line");
    assert_eq!(blocks[0].lines.len(), 2);
    let other = blocks[1].rect;
    text::replace_block(&mut doc, 0, 0, "Replacement").unwrap();
    let blocks = text::text_blocks(&reopen(&doc), 0).unwrap();
    assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Replacement", "Other column"]);
    assert_eq!(blocks[1].rect, other);
}

#[test]
fn independent_text_objects_with_different_transforms_stay_separate() {
    let doc = text_page(
        "BT /F1 12 Tf 1 0 0 1 72 700 Tm (Hello) Tj ET \
         BT /F1 12 Tf 0 1 -1 0 103 700 Tm (Vertical) Tj ET",
    );
    assert_eq!(text::text_lines(&doc, 0).unwrap().len(), 2);
}

/// A negative font size or `Tz` runs the glyphs backwards along the text matrix: each word's
/// `ink` box still spans all its glyphs, not only the first.
#[test]
fn word_boxes_span_whole_words_when_glyphs_run_backwards() {
    // "someone@example.com" in Helvetica 11 is 121.814 pt wide.
    for content in [
        "BT /F1 -11 Tf -1 0 0 1 300 742 Tm (Mail the team: someone@example.com) Tj ET",
        "BT /F1 11 Tf -100 Tz 300 742 Td (Mail the team: someone@example.com) Tj ET",
    ] {
        let lines = text::text_lines(&text_page(content), 0).unwrap();
        assert_eq!(lines.len(), 1, "{content}");
        let ink = &lines[0].ink;
        assert_eq!(ink.len(), 4, "{content}: {ink:?}");
        let width = |r: &[f64; 4]| r[2] - r[0];
        assert!(ink.iter().all(|r| width(r) > 5.0), "{content}: {ink:?}");
        let address = ink.iter().map(width).fold(0.0, f64::max);
        assert!((address - 121.814).abs() < 0.01, "{content}: {ink:?}");
        let (x0, x1) = ink.iter().fold((f64::MAX, f64::MIN), |(a, b), r| (a.min(r[0]), b.max(r[2])));
        assert!(x0 >= lines[0].rect[0] - 0.01 && x1 <= lines[0].rect[2] + 0.01, "{content}: {ink:?} {:?}", lines[0].rect);
    }
}

#[test]
fn text_lines_are_found_and_replaced_in_place() {
    let mut doc =
        text_page("BT /F1 12 Tf 72 700 Td (Hello) Tj 40 0 Td [(wor) -20 (ld)] TJ 0 -20 Td (Second line) Tj ET BT /F2 10 Tf 72 600 Td (ab) Tj ET");
    let lines = text::text_lines(&doc, 0).unwrap();
    let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["Hello world", "Second line", "ab"]);
    assert!((lines[0].rect[0] - 72.0).abs() < 0.01 && lines[0].rect[1] < 700.0 && lines[0].rect[3] > 700.0, "{:?}", lines[0].rect);
    assert!((lines[0].size - 12.0).abs() < 1e-9 && lines[0].base_font == "Helvetica");
    let second = lines[1].rect;
    let r = text::replace_line(&mut doc, 0, 0, "Goodbye, café").unwrap();
    assert_eq!(r.substituted, None);
    let doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines[0].text, "Goodbye, café");
    assert_eq!(lines[1].text, "Second line");
    assert_eq!(lines[1].rect, second);
}

/// A page whose content is one stream in three pieces, split between tokens: a marked-content
/// dictionary closes at the start of the middle piece, and the `TJ` array that ends it is shown
/// by the operator that starts the last one.
fn split_streams_page() -> Document {
    let piece = |s: &str| format!("<< /Length {} >>\nstream\n{s}\nendstream", s.len());
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents [4 0 R 5 0 R 6 0 R] /Resources << /Font << /F1 7 0 R >> >> >>".into(),
        piece("/P << /MCID 0"),
        piece(">> BDC BT /F1 12 Tf 72 700 Td (Target) Tj 0 -20 Td [(After) -20 (wards)]"),
        piece("TJ ET EMC"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
    ];
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

/// The page's pieces joined back into one stream: every operator must still have its operands.
fn assert_split_tokens_kept(doc: &Document, new_text: &str) {
    let joined = streams(doc, 0).join("\n");
    assert!(joined.contains(new_text) && !joined.contains("Target"), "{joined}");
    assert!(joined.contains("[(After) -20 (wards)]"), "the text after the line is gone: {joined}");
    for op in pdfcraft_content::parse(joined.as_bytes()).ops {
        let want = match op.op.as_slice() {
            b"BDC" => 2,
            b"TJ" => 1,
            _ => continue,
        };
        assert_eq!(op.operands.len(), want, "{} lost its operands: {joined}", String::from_utf8_lossy(&op.op));
    }
}

#[test]
fn editing_text_keeps_the_tokens_a_stream_shares_with_its_neighbours() {
    let mut doc = split_streams_page();
    assert_eq!(text::text_lines(&doc, 0).unwrap()[0].text, "Target");
    text::replace_line(&mut doc, 0, 0, "Edited").unwrap();
    assert_split_tokens_kept(&reopen(&doc), "Edited");
    // A paragraph rewrite: "Target" and the line after it (split across pieces) are one
    // paragraph, rewritten whole without stray tokens.
    let mut doc = split_streams_page();
    assert_eq!(text::text_blocks(&doc, 0).unwrap().len(), 1);
    text::replace_block(&mut doc, 0, 0, "Rewrapped").unwrap();
    let (joined, ops) = joined_ops(&reopen(&doc));
    assert!(joined.contains("Rewrapped") && !joined.contains("Target") && !joined.contains("wards"), "{joined}");
    assert!(ops.iter().find(|o| o.is("BDC")).is_some_and(|o| o.operands.len() == 2), "{joined}");
}

/// The operators of the page's pieces joined back into one stream, each checked for operands.
fn joined_ops(doc: &Document) -> (String, Vec<pdfcraft_content::Op>) {
    let joined = streams(doc, 0).join("\n");
    let ops = pdfcraft_content::parse(joined.as_bytes());
    assert_eq!(ops.skipped, 0, "stray tokens: {joined}");
    (joined, ops.ops)
}

#[test]
fn a_line_whose_operator_starts_the_next_stream_can_be_edited() {
    // #155: `[(After) -20 (wards)]` ends one piece and its `TJ` starts the next.
    let doc = split_streams_page();
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Target", "Afterwards"]);
    assert!(lines[1].rect[1] < lines[0].rect[1] && lines[1].rect[2] > lines[1].rect[0] + 40.0, "{:?}", lines[1].rect);

    // Retyping it replaces the operator whole, in both pieces it spans.
    let mut doc = split_streams_page();
    text::replace_line(&mut doc, 0, 1, "Later").unwrap();
    let doc = reopen(&doc);
    let (joined, ops) = joined_ops(&doc);
    assert!(!joined.contains("After") && !joined.contains("wards") && joined.contains("Target"), "{joined}");
    assert!(ops.iter().filter(|o| o.is("TJ")).count() == 0, "{joined}");
    assert!(ops.iter().find(|o| o.is("BDC")).is_some_and(|o| o.operands.len() == 2), "{joined}");
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Target", "Later"]);
    // The pieces stay pieces: the first is untouched, the marked content still closes.
    let pieces = streams(&doc, 0);
    assert_eq!(pieces.len(), 3);
    assert_eq!(pieces[0].trim(), "/P << /MCID 0");
    assert!(pieces[2].contains("EMC") && !pieces[2].contains("TJ"), "{pieces:?}");

    // As a paragraph with the line before it: both lines are rewritten.
    let mut doc = split_streams_page();
    let blocks = text::text_blocks(&doc, 0).unwrap();
    let last = blocks.iter().position(|b| b.text.contains("Afterwards")).unwrap();
    text::replace_block(&mut doc, 0, last, "Rewrapped").unwrap();
    let doc = reopen(&doc);
    let (joined, ops) = joined_ops(&doc);
    assert!(!joined.contains("wards") && joined.contains("Rewrapped"), "{joined}");
    assert!(ops.iter().find(|o| o.is("BDC")).is_some_and(|o| o.operands.len() == 2), "{joined}");
    assert!(text::text_lines(&doc, 0).unwrap().iter().any(|l| l.text == "Rewrapped"));
}

#[test]
fn win_ansi_high_glyphs_use_helvetica_not_the_fallback_font() {
    // š ž Š Ž Œ Ÿ ƒ † ‰ (WinAnsi 0x80–0x9F) are standard-font glyphs (#347): no Type3 fallback.
    let text = "Šumava žaba œuvre Ÿ ƒ † ‰ €";
    let mut doc = text_page("BT /F2 10 Tf 72 600 Td (ab) Tj ET");
    let r = text::replace_line(&mut doc, 0, 0, text).unwrap();
    assert_eq!(r.substituted.as_deref(), Some("Helvetica"));
    let lines = text::text_lines(&reopen(&doc), 0).unwrap();
    assert_eq!((lines[0].text.as_str(), lines[0].base_font.as_str()), (text, "Helvetica"));
    let mut doc = text_page("BT /F2 10 Tf 72 600 Td (ab) Tj ET");
    let r = text::replace_block(&mut doc, 0, 0, text).unwrap();
    assert!(r.substituted.as_deref().is_none_or(|s| !s.contains("Type3")), "{:?}", r.substituted);
    let lines = text::text_lines(&reopen(&doc), 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(" "), text);
}

#[test]
fn missing_glyphs_substitute_helvetica_and_impossible_text_is_refused() {
    let mut doc = text_page("BT /F2 10 Tf 72 600 Td (ab) Tj ET");
    // "c" has no glyph in the subset: Helvetica takes over for this line.
    let r = text::replace_line(&mut doc, 0, 0, "abc").unwrap();
    assert_eq!(r.substituted.as_deref(), Some("Helvetica"));
    let doc2 = reopen(&doc);
    let lines = text::text_lines(&doc2, 0).unwrap();
    assert_eq!((lines[0].text.as_str(), lines[0].base_font.as_str()), ("abc", "Helvetica"));
    // Neither the standard font nor the craft-fonts Japanese faces have this emoji.
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Hello) Tj ET");
    assert!(text::replace_line(&mut doc, 0, 0, "\u{1f4a9}").is_err());
    assert!(text::replace_line(&mut doc, 0, 5, "x").is_err(), "no such line");
}

#[test]
fn paragraphs_are_found_and_rewrapped() {
    let mut doc = text_page(
        "BT 0 0 1 rg /F1 10 Tf 12 TL 72 700 Td (The quick brown fox) Tj T* (jumps over the) Tj T* (lazy dog.) Tj ET \
         BT /F1 10 Tf 72 600 Td (Next paragraph) Tj ET",
    );
    let blocks = text::text_blocks(&doc, 0).unwrap();
    let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
    assert_eq!(texts, ["The quick brown fox jumps over the lazy dog.", "Next paragraph"]);
    assert_eq!(blocks[0].lines, [0, 1, 2]);
    let width = blocks[0].rect[2] - blocks[0].rect[0];
    let next = blocks[1].rect;
    let long = "PdfCraft rewraps a paragraph to its own width when its text changes, keeping the font, size, colour and line spacing.";
    assert_eq!(text::replace_block(&mut doc, 0, 0, long).unwrap().substituted, None);
    let doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks[0].text, long);
    assert!(blocks[0].lines.len() > 3, "more lines: {}", blocks[0].lines.len());
    for i in &blocks[0].lines {
        assert!(lines[*i].rect[2] - lines[*i].rect[0] <= width + 1.0, "line {} fits", lines[*i].text);
    }
    // Same left edge, same spacing (12 pt).
    assert!((lines[0].rect[0] - 72.0).abs() < 0.01);
    let spacing = lines[blocks[0].lines[0]].origin_baseline() - lines[blocks[0].lines[1]].origin_baseline();
    assert!((spacing - 12.0).abs() < 0.01, "{spacing}");
    // The next paragraph is untouched, and the new text is blue like the old.
    assert_eq!(blocks[1].text, "Next paragraph");
    assert_eq!(blocks[1].rect, next);
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert!(content.contains("0 0 1 rg"), "{content}");
    // Shorter text: fewer lines.
    let mut doc = doc;
    text::replace_block(&mut doc, 0, 0, "Short.").unwrap();
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!((blocks[0].text.as_str(), blocks[0].lines.len()), ("Short.", 1));
}

fn page_content_bytes(doc: &Document, page: usize) -> Vec<u8> {
    let p = pdfcraft_model::pages(doc).swap_remove(page);
    let c = p.dict.get(b"Contents").unwrap();
    match &*doc.resolve(c) {
        pdfcraft_cos::Object::Stream(s) => s.decoded().unwrap(),
        pdfcraft_cos::Object::Array(a) => a
            .iter()
            .flat_map(|x| match &*doc.resolve(x) {
                pdfcraft_cos::Object::Stream(s) => s.decoded().unwrap(),
                _ => Vec::new(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A page drawing image XObject /Im0 at 100,100 size 200 × 100 (pixels 4 × 2).
fn image_page() -> Document {
    image_page_with("q 200 0 0 100 100 100 cm /Im0 Do Q BT /F1 12 Tf 72 700 Td (Caption) Tj ET")
}

fn image_page_with(content: &str) -> Document {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Im0 6 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /XObject /Subtype /Image /Width 4 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 8 >>\nstream\n\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\nendstream".into(),
    ];
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

/// The box of the first `re` on the page in user space, after the `cm`s before it.
fn first_rect_in_user_space(doc: &Document) -> [f64; 4] {
    let mut ctm = pdfcraft_content::Matrix::IDENTITY;
    for op in pdfcraft_content::parse(&page_content_bytes(doc, 0)).ops {
        if op.is("cm") {
            ctm = pdfcraft_content::Matrix(op.nums::<6>().unwrap()).then(&ctm);
        } else if op.is("re") {
            let [x, y, w, h] = op.nums::<4>().unwrap();
            return ctm.bbox([x, y, x + w, y + h]);
        }
    }
    panic!("no clip rectangle");
}

#[test]
fn moving_an_image_moves_its_own_clip_too() {
    let content = "q 100 100 200 100 re W* n 200 0 0 100 100 100 cm /Im0 Do Q BT /F1 12 Tf 72 700 Td (Caption) Tj ET";
    let target = [50.0, 400.0, 350.0, 600.0];
    let mut doc = image_page_with(content);
    let before = images::page_images(&doc, 0).unwrap()[0].rect;
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(images::rect_to_rect(before, target))).unwrap();
    let mut doc = reopen(&doc);
    assert!(close(images::page_images(&doc, 0).unwrap()[0].rect, target));
    assert!(close(first_rect_in_user_space(&doc), target), "{:?}", first_rect_in_user_space(&doc));
    // A second edit still finds the clip, now behind the `cm` the first one put before it.
    let moved = [80.0, 420.0, 380.0, 620.0];
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(images::rect_to_rect(target, moved))).unwrap();
    assert!(close(first_rect_in_user_space(&doc), moved));
    images::translate_images(&mut doc, 0, &[0], [10.0, 0.0]).unwrap();
    assert!(close(images::page_images(&doc, 0).unwrap()[0].rect, [90.0, 420.0, 390.0, 620.0]));
    assert!(close(first_rect_in_user_space(&doc), [90.0, 420.0, 390.0, 620.0]));
    // Moving it with the group move helper takes the clip along as well.
    let mut doc = image_page_with(content);
    images::translate_images(&mut doc, 0, &[0], [30.0, -20.0]).unwrap();
    assert!(close(images::page_images(&doc, 0).unwrap()[0].rect, [130.0, 80.0, 330.0, 180.0]));
    assert!(close(first_rect_in_user_space(&doc), [130.0, 80.0, 330.0, 180.0]));
    // A clip shared with other drawing is left where it is.
    let shared = "q 100 100 200 100 re W n q 200 0 0 100 100 100 cm /Im0 Do Q 0 0 10 10 re f Q";
    let mut doc = image_page_with(shared);
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(images::rect_to_rect(before, target))).unwrap();
    assert!(close(first_rect_in_user_space(&doc), [100.0, 100.0, 300.0, 200.0]));
}

fn close(a: [f64; 4], b: [f64; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)
}

/// The deepest `q` nesting on the page.
fn q_depth(doc: &Document) -> usize {
    let mut depth = 0_usize;
    let mut deepest = 0;
    for op in pdfcraft_content::parse(&page_content_bytes(doc, 0)).ops {
        if op.is("q") {
            depth += 1;
            deepest = deepest.max(depth);
        } else if op.is("Q") {
            depth = depth.saturating_sub(1);
        }
    }
    deepest
}

#[test]
fn repeated_image_edits_change_one_transform_instead_of_nesting() {
    let variants = [
        // Its own placement, a private crop, and a bare `Do` that the first edit wraps.
        "q 200 0 0 100 100 100 cm /Im0 Do Q BT /F1 12 Tf 72 700 Td (Caption) Tj ET",
        "q 100 100 200 100 re W* n 200 0 0 100 100 100 cm /Im0 Do Q BT /F1 12 Tf 72 700 Td (Caption) Tj ET",
        "200 0 0 100 100 100 cm /Im0 Do",
    ];
    for content in variants {
        let mut doc = image_page_with(content);
        let resize = |doc: &mut Document, n: usize| -> usize {
            let mut depth = 0;
            for i in 0..n {
                let r = images::page_images(doc, 0).unwrap()[0].visible_rect.unwrap();
                // Grow by 3% about the lower-left corner and move right by 2, staying on the page.
                let target = [r[0] + 2.0, r[1], r[0] + 2.0 + (r[2] - r[0]) * 1.03, r[1] + (r[3] - r[1]) * 1.03];
                images::change_image(doc, 0, 0, &images::ImageChange::Transform(images::rect_to_rect(r, target))).unwrap();
                let after = images::page_images(doc, 0).unwrap()[0].visible_rect.unwrap();
                assert!(after.iter().zip(target).all(|(a, b)| (a - b).abs() < 1e-6), "{content}: {after:?} vs {target:?}");
                if i == 0 {
                    depth = q_depth(doc);
                }
            }
            depth
        };
        let first = resize(&mut doc, 20);
        assert_eq!(q_depth(&doc), first, "{content}: twenty resizes nest no deeper than one");
        let mut doc = reopen(&doc);
        let before = images::page_images(&doc, 0).unwrap()[0].rect;
        for _ in 0..20 {
            images::translate_images(&mut doc, 0, &[0], [1.0, -2.0]).unwrap();
        }
        let after = images::page_images(&doc, 0).unwrap()[0].rect;
        assert!(close(after, [before[0] + 20.0, before[1] - 40.0, before[2] + 20.0, before[3] - 40.0]), "{content}: {after:?}");
        assert_eq!(q_depth(&doc), first, "{content}: group moves change the same transform");
        let caption: Vec<_> = text::text_lines(&doc, 0).unwrap().into_iter().map(|l| l.text).collect();
        assert_eq!(caption.is_empty(), !content.contains("Caption"), "{content}: the text after the image is untouched");
    }
}

#[test]
fn page_images_move_turn_replace_and_delete() {
    let mut doc = image_page();
    let imgs = images::page_images(&doc, 0).unwrap();
    assert_eq!(imgs.len(), 1);
    assert!(close(imgs[0].rect, [100.0, 100.0, 300.0, 200.0]), "{:?}", imgs[0].rect);
    assert_eq!((imgs[0].width, imgs[0].height, imgs[0].name.as_str()), (4, 2, "Im0"));
    // Move and resize.
    let t = images::rect_to_rect(imgs[0].rect, [50.0, 400.0, 150.0, 450.0]);
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(t)).unwrap();
    let doc = reopen(&doc);
    let r = images::page_images(&doc, 0).unwrap()[0].rect;
    assert!(close(r, [50.0, 400.0, 150.0, 450.0]), "{r:?}");
    // A quarter turn about the centre: 100 × 50 becomes 50 × 100 around (100, 425).
    let mut doc = doc;
    let t = images::turn_about_centre(r, 1, false, false);
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(t)).unwrap();
    let r = images::page_images(&doc, 0).unwrap()[0].rect;
    assert!(close(r, [75.0, 375.0, 125.0, 475.0]), "{r:?}");
    // The text after it is untouched.
    assert_eq!(text::text_lines(&doc, 0).unwrap()[0].text, "Caption");
    // Replace with another image object, in the same place.
    let mut d = pdfcraft_cos::Dict::new();
    for (k, v) in [
        (&b"Type"[..], pdfcraft_cos::Object::name("XObject")),
        (b"Subtype", pdfcraft_cos::Object::name("Image")),
        (b"Width", pdfcraft_cos::Object::Int(1)),
        (b"Height", pdfcraft_cos::Object::Int(1)),
    ] {
        d.set(k.to_vec(), v);
    }
    let other = doc.add(pdfcraft_cos::Object::Stream(pdfcraft_cos::Stream::from_raw(d, vec![0])));
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Replace(other)).unwrap();
    let imgs = images::page_images(&doc, 0).unwrap();
    assert_eq!((imgs[0].object, imgs[0].width), (Some(other), 1));
    assert!(close(imgs[0].rect, r));
    // Delete.
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Delete).unwrap();
    assert!(images::page_images(&doc, 0).unwrap().is_empty());
    assert!(images::change_image(&mut doc, 0, 0, &images::ImageChange::Delete).is_err());
}

#[test]
fn form_artwork_edits_only_the_selected_placement() {
    let mut doc = fixture(); // three pages share resources; pages 0 and 1 also share contents
    let mut form = Dict::new();
    form.set(b"Subtype".to_vec(), Object::name("Form"));
    form.set(b"BBox".to_vec(), Object::Array([10, 20, 110, 70].map(Object::Int).to_vec()));
    form.set(b"Matrix".to_vec(), Object::Array([2, 0, 0, 3, -20, -60].map(Object::Int).to_vec()));
    let artwork = doc.add(Object::Stream(Stream::flate(form, b"1 0 0 rg 10 20 100 50 re f")));
    doc.update_dict(pdfcraft_cos::ObjRef::new(6, 0), |d| {
        let mut xo = Dict::new();
        xo.set(b"Figure".to_vec(), Object::Ref(artwork));
        d.set(b"XObject".to_vec(), Object::Dict(xo));
    })
    .unwrap();
    let content = b"q 1 0 0 1 40 80 cm /Figure Do Q q 1 0 0 1 300 400 cm /Figure Do Q BT /F1 9 Tf (Caption) Tj ET";
    doc.set(pdfcraft_cos::ObjRef::new(7, 0), Object::Stream(Stream::flate(Dict::new(), content)));
    let mut doc = reopen(&doc);
    let original = doc.get(artwork);
    let imgs = images::page_images(&doc, 0).unwrap();
    assert_eq!(imgs.len(), 2);
    assert!(imgs[0].is_form);
    assert_eq!((imgs[0].width, imgs[0].height), (0, 0));
    assert!(close(imgs[0].rect, [40.0, 80.0, 240.0, 230.0]));
    let target = [60.0, 100.0, 160.0, 175.0];
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(images::rect_to_rect(imgs[0].rect, target))).unwrap();
    let mut doc = reopen(&doc);
    let moved = images::page_images(&doc, 0).unwrap();
    assert!(close(moved[0].rect, target));
    assert_eq!(moved[1].rect, imgs[1].rect);
    assert_eq!(images::page_images(&doc, 1).unwrap()[0].rect, imgs[0].rect);
    assert_eq!(*doc.get(artwork), *original, "the shared Form and its unknown data are untouched");
    assert_eq!(text::text_lines(&doc, 0).unwrap()[0].text, "Caption");
    let t = images::turn_about_centre(target, 1, false, false);
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(t)).unwrap();
    assert!(close(images::page_images(&doc, 0).unwrap()[0].rect, [72.5, 87.5, 147.5, 187.5]));
    assert!(images::change_image(&mut doc, 0, 0, &images::ImageChange::Replace(artwork)).is_err());
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Delete).unwrap();
    let doc = reopen(&doc);
    assert_eq!(images::page_images(&doc, 0).unwrap().len(), 1);
    assert_eq!(images::page_images(&doc, 1).unwrap().len(), 2);
    // Invalid bounds are skipped without interpreting the Form stream.
    let mut doc = doc;
    let mut broken = (*doc.get(artwork)).clone();
    if let Object::Stream(s) = &mut broken {
        s.dict.set(b"BBox".to_vec(), Object::Array(vec![Object::Int(0)]));
    }
    doc.set(artwork, broken);
    assert!(images::page_images(&doc, 0).unwrap().is_empty());
}

#[test]
fn paragraphs_take_new_formatting() {
    let mut doc = text_page("BT /F1 10 Tf 12 TL 100 700 Td (One two three four five six seven) Tj T* (eight nine ten eleven twelve) Tj ET");
    let before = text::text_blocks(&doc, 0).unwrap()[0].clone();
    let style = text::BlockStyle {
        family: Some((added::Family::Times, true, false)),
        size: Some(12.0),
        color: Some([1.0, 0.0, 0.0]),
        align: Some(added::Align::Center),
        ..Default::default()
    };
    text::rewrite_block(&mut doc, 0, 0, None, &style).unwrap();
    let doc = reopen(&doc);
    let blocks = text::text_blocks(&doc, 0).unwrap();
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(blocks[0].text, before.text, "same words");
    assert_eq!((blocks[0].base_font.as_str(), blocks[0].size), ("Times-Bold", 12.0));
    // Centred in the paragraph's width.
    let mid = (before.rect[0] + before.rect[2]) / 2.0;
    for i in &blocks[0].lines {
        let r = lines[*i].rect;
        assert!(((r[0] + r[2]) / 2.0 - mid).abs() < 2.0, "line {:?} centred on {mid}", lines[*i].text);
    }
    assert!(String::from_utf8_lossy(&page_content_bytes(&doc, 0)).contains("1 0 0 rg"));
    // Right alignment keeps lines flush with the right edge.
    let mut doc = text_page("BT /F1 10 Tf 12 TL 100 700 Td (One two three four five six seven) Tj T* (eight nine ten eleven twelve) Tj ET");
    let right = text::text_blocks(&doc, 0).unwrap()[0].rect[2];
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { align: Some(added::Align::Right), ..Default::default() }).unwrap();
    let lines = text::text_lines(&doc, 0).unwrap();
    for l in &lines {
        assert!((l.rect[2] - right).abs() < 1.0, "{:?} ends at {} not {right}", l.text, l.rect[2]);
    }
}

#[test]
fn a_new_paragraph_colour_does_not_spill_into_the_text_after_it() {
    let mut doc = text_page("BT /F1 10 Tf 72 700 Td (First paragraph) Tj 0 -40 Td (Second paragraph) Tj ET");
    let style = text::BlockStyle { color: Some([1.0, 0.0, 0.0]), ..Default::default() };
    text::rewrite_block(&mut doc, 0, 0, None, &style).unwrap();
    let doc = reopen(&doc);
    // The fill colour in force where the second paragraph is shown (q/Q nest it).
    let ops = pdfcraft_content::parse(&page_content_bytes(&doc, 0)).ops;
    let (mut fill, mut stack) = (String::from("0 g"), Vec::new());
    for op in &ops {
        match op.op.as_slice() {
            b"q" => stack.push(fill.clone()),
            b"Q" => fill = stack.pop().unwrap_or_default(),
            b"g" | b"rg" | b"k" => fill = String::from_utf8_lossy(&pdfcraft_content::serialize_ops(std::slice::from_ref(op))).trim().to_string(),
            b"Tj" if op.operands.first().and_then(|o| o.as_string()).is_some_and(|s| s.to_text() == "Second paragraph") => break,
            _ => {}
        }
    }
    assert_eq!(fill, "0 g", "{ops:?}");
    assert_eq!(text::text_blocks(&doc, 0).unwrap()[1].text, "Second paragraph");
}

#[test]
fn a_rewritten_paragraph_keeps_its_place_in_a_shared_text_object() {
    // Three paragraphs in one BT … ET: rewriting the second keeps the order (paragraph numbers
    // and reading order) and every paragraph's position.
    let src = "BT /F1 10 Tf 72 700 Td (First paragraph) Tj 0 -40 Td (Second paragraph) Tj 0 -40 Td (Third paragraph) Tj ET";
    let mut doc = text_page(src);
    let before = text::text_blocks(&doc, 0).unwrap();
    text::replace_block(&mut doc, 0, 1, "Edited second").unwrap();
    let doc = reopen(&doc);
    let after = text::text_blocks(&doc, 0).unwrap();
    let texts: Vec<&str> = after.iter().map(|b| b.text.as_str()).collect();
    assert_eq!(texts, ["First paragraph", "Edited second", "Third paragraph"]);
    assert_eq!(after[0].rect, before[0].rect);
    assert_eq!(after[2].rect, before[2].rect);
    assert!((after[1].rect[0] - before[1].rect[0]).abs() < 0.01 && (after[1].rect[1] - before[1].rect[1]).abs() < 0.01, "{:?}", after[1].rect);
}

#[test]
fn recolouring_a_paragraph_in_a_shared_text_object_keeps_order_and_nesting() {
    // Splitting the text object (to keep the order) and q … Q (to keep the colour in) together:
    // q/Q stay outside text objects, and the paragraph after it keeps the original colour.
    let src = "BT /F1 10 Tf 72 700 Td (First paragraph) Tj 0 -40 Td (Second paragraph) Tj 0 -40 Td (Third paragraph) Tj ET";
    let mut doc = text_page(src);
    let style = text::BlockStyle { color: Some([1.0, 0.0, 0.0]), ..Default::default() };
    text::rewrite_block(&mut doc, 0, 1, None, &style).unwrap();
    let doc = reopen(&doc);
    let texts: Vec<String> = text::text_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(texts, ["First paragraph", "Second paragraph", "Third paragraph"]);
    let ops = pdfcraft_content::parse(&page_content_bytes(&doc, 0)).ops;
    let (mut in_text, mut depth, mut red) = (false, 0usize, Vec::new());
    for op in &ops {
        match op.op.as_slice() {
            b"BT" => in_text = true,
            b"ET" => in_text = false,
            b"q" | b"Q" => {
                assert!(!in_text, "q/Q inside a text object: {ops:?}");
                depth = if op.is("q") { depth + 1 } else { depth.saturating_sub(1) };
            }
            b"rg" => red.push(depth),
            b"Tj" if op.operands.first().and_then(|o| o.as_string()).is_some_and(|s| s.to_text() == "Third paragraph") => {
                assert!(red.iter().all(|d| *d > depth), "red still in force for the third paragraph: {ops:?}");
            }
            _ => {}
        }
    }
    assert!(!red.is_empty(), "{ops:?}");
}

#[test]
fn justify_underline_and_spacing() {
    let src = "BT /F1 10 Tf 12 TL 100 700 Td (One two three four five six seven) Tj T* (eight nine ten eleven twelve) Tj T* (end) Tj ET";
    // Justified: every line but the last reaches the right edge.
    let mut doc = text_page(src);
    let right = text::text_blocks(&doc, 0).unwrap()[0].rect[2];
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { align: Some(added::Align::Justify), ..Default::default() }).unwrap();
    let lines = text::text_lines(&doc, 0).unwrap();
    for l in &lines[..lines.len() - 1] {
        assert!((l.rect[2] - right).abs() < 1.0, "{:?} ends at {} not {right}", l.text, l.rect[2]);
    }
    // Double line spacing, underlined.
    let mut doc = text_page(src);
    let style = text::BlockStyle { line_spacing: Some(2.0), underline: Some(true), ..Default::default() };
    text::rewrite_block(&mut doc, 0, 0, None, &style).unwrap();
    let lines = text::text_lines(&doc, 0).unwrap();
    assert!((lines[0].origin_baseline() - lines[1].origin_baseline() - 20.0).abs() < 0.01);
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert!(content.contains(" l\nS\n") || content.contains(" l S"), "{content}");
    // Character spacing widens, horizontal scale narrows.
    let mut doc = text_page("BT /F1 10 Tf 100 700 Td (Wide text) Tj ET");
    let w0 = text::text_lines(&doc, 0).unwrap()[0].rect;
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { char_spacing: Some(2.0), ..Default::default() }).unwrap();
    let w1 = text::text_lines(&doc, 0).unwrap()[0].rect;
    assert!((w1[2] - w1[0]) - (w0[2] - w0[0]) > 15.0, "{w0:?} → {w1:?}");
    let mut doc = text_page("BT /F1 10 Tf 100 700 Td (Wide text) Tj ET");
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { scale: Some(50.0), ..Default::default() }).unwrap();
    let w2 = text::text_lines(&doc, 0).unwrap()[0].rect;
    assert!(((w2[2] - w2[0]) - (w0[2] - w0[0]) / 2.0).abs() < 1.0, "{w0:?} → {w2:?}");
}

#[test]
fn a_line_drawn_twice_is_replaced_everywhere() {
    // Fake bold: a line drawn twice at the same spot (each copy its own text object). The two
    // copies group apart, but editing one replaces both — a surviving copy would show the old
    // text under the new.
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Hello) Tj ET BT /F1 12 Tf 72 700 Td (Hello) Tj ET");
    assert_eq!(text::text_blocks(&doc, 0).unwrap().len(), 2, "the copies group apart");
    text::replace_block(&mut doc, 0, 0, "Hello world").unwrap();
    let doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Hello world"], "one line, replaced");
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert_eq!(content.matches("(Hello").count(), 1, "the second copy's operators are gone: {content}");
    // Copies in one text object (the matrix re-issued at the same spot) go too.
    let mut doc = text_page("BT /F1 12 Tf 1 0 0 1 72 700 Tm (Hi) Tj 1 0 0 1 72.3 700 Tm (Hi) Tj ET");
    text::replace_line(&mut doc, 0, 0, "Hey").unwrap();
    let doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Hey"], "one line, replaced");
    // A neighbouring line is not coincident and stays (40 pt spacing groups apart at 12 pt).
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Hello) Tj 0 -40 Td (world) Tj ET");
    text::replace_block(&mut doc, 0, 0, "Hello there").unwrap();
    let doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Hello there", "world"]);
}

#[test]
fn replacing_a_line_keeps_a_distinct_nearby_line() {
    // 12 pt lines 6 pt apart: the glyph boxes overlap by more than half the smaller box,
    // but the baselines are different lines (#1316).
    let src = "BT /F1 12 Tf 1 0 0 1 72 700 Tm (Alpha) Tj 0 -6 Td (Beta) Tj ET";
    let listed = |doc: &Document| text::text_lines(doc, 0).unwrap().into_iter().map(|l| l.text).collect::<Vec<_>>();
    for paragraph in [false, true] {
        let mut doc = text_page(src);
        assert_eq!(listed(&doc), ["Alpha", "Beta"]);
        assert_eq!(text::text_blocks(&doc, 0).unwrap().len(), 2);
        if paragraph {
            text::rewrite_block(&mut doc, 0, 0, Some("ALPHA"), &text::BlockStyle::default()).unwrap();
        } else {
            text::replace_line(&mut doc, 0, 0, "ALPHA").unwrap();
        }
        assert_eq!(listed(&doc), ["ALPHA", "Beta"]);
        assert!(String::from_utf8_lossy(&page_content_bytes(&doc, 0)).contains("(Beta)"));
        for bytes in [write_incremental(&doc, &SaveOptions::default()).unwrap(), write_full(&doc, &SaveOptions::default()).unwrap()] {
            let back = Document::open(Arc::new(bytes)).unwrap();
            assert!(back.repair_log().is_empty());
            assert_eq!(listed(&back), ["ALPHA", "Beta"]);
        }
    }
    // The same word on the next line is still a different line.
    let mut same = text_page("BT /F1 12 Tf 1 0 0 1 72 700 Tm (Alpha) Tj 0 -6 Td (Alpha) Tj ET");
    text::replace_line(&mut same, 0, 0, "ALPHA").unwrap();
    assert_eq!(listed(&same), ["ALPHA", "Alpha"]);
    // Ordinary leading, and a paragraph far enough to group apart, keep the neighbour.
    let mut spaced = text_page("BT /F1 12 Tf 1 0 0 1 72 700 Tm (Alpha) Tj 0 -14.4 Td (Beta) Tj ET");
    text::replace_line(&mut spaced, 0, 0, "ALPHA").unwrap();
    assert_eq!(listed(&spaced), ["ALPHA", "Beta"]);
    let mut apart = text_page("BT /F1 12 Tf 1 0 0 1 72 700 Tm (Alpha) Tj 0 -40 Td (Beta) Tj ET");
    text::rewrite_block(&mut apart, 0, 0, Some("ALPHA"), &text::BlockStyle::default()).unwrap();
    assert_eq!(listed(&apart), ["ALPHA", "Beta"]);
    // A second copy at the same position is still one replacement.
    for paragraph in [false, true] {
        let mut duplicate = text_page("BT /F1 12 Tf 1 0 0 1 72 700 Tm (Alpha) Tj ET BT /F1 12 Tf 1 0 0 1 72 700 Tm (Alpha) Tj ET");
        if paragraph {
            text::rewrite_block(&mut duplicate, 0, 0, Some("ALPHA"), &text::BlockStyle::default()).unwrap();
        } else {
            text::replace_line(&mut duplicate, 0, 0, "ALPHA").unwrap();
        }
        assert_eq!(listed(&duplicate), ["ALPHA"]);
    }
}

#[test]
fn paragraphs_move_and_rewrap_to_a_new_width() {
    let para = "BT /F1 10 Tf 12 TL 100 700 Td (One two three four five six seven) Tj T* (eight nine ten eleven twelve) Tj ET";
    let close = |a: f64, b: f64| (a - b).abs() < 0.5;
    // Moved 50 right and 200 down, same words and wrapping.
    let mut doc = text_page(para);
    let before = text::text_blocks(&doc, 0).unwrap()[0].clone();
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { offset: Some([50.0, -200.0]), ..Default::default() }).unwrap();
    let doc = reopen(&doc);
    let after = text::text_blocks(&doc, 0).unwrap()[0].clone();
    assert_eq!((after.text.as_str(), after.lines.len()), (before.text.as_str(), before.lines.len()));
    assert!(close(after.rect[0], before.rect[0] + 50.0) && close(after.rect[1], before.rect[1] - 200.0), "{:?} → {:?}", before.rect, after.rect);
    // Drawn at half scale: the move is still in page space.
    let mut doc = text_page(&format!("q 0.5 0 0 0.5 0 0 cm {} Q", para.replace("100 700 Td", "200 1400 Td")));
    let before = text::text_blocks(&doc, 0).unwrap()[0].clone();
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { offset: Some([30.0, 40.0]), ..Default::default() }).unwrap();
    let after = text::text_blocks(&reopen(&doc), 0).unwrap()[0].clone();
    assert!(close(after.rect[0], before.rect[0] + 30.0) && close(after.rect[1], before.rect[1] + 40.0), "{:?} → {:?}", before.rect, after.rect);
    // A narrower box rewraps into more lines that fit it; a wider one into fewer.
    let mut doc = text_page(para);
    let left = text::text_blocks(&doc, 0).unwrap()[0].rect[0];
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { width: Some(80.0), ..Default::default() }).unwrap();
    let narrow = text::text_blocks(&reopen(&doc), 0).unwrap()[0].clone();
    assert!(narrow.lines.len() > 2, "{narrow:?}");
    assert!(narrow.rect[2] <= left + 80.5, "{narrow:?}");
    let mut doc = text_page(para);
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { width: Some(400.0), ..Default::default() }).unwrap();
    assert_eq!(text::text_blocks(&reopen(&doc), 0).unwrap()[0].lines.len(), 1);
    // Untrusted numbers (the automation tools pass them through) are refused, not drawn.
    let mut doc = text_page(para);
    for style in [
        text::BlockStyle { offset: Some([f64::NAN, 0.0]), ..Default::default() },
        text::BlockStyle { offset: Some([0.0, f64::INFINITY]), ..Default::default() },
        text::BlockStyle { width: Some(f64::NAN), ..Default::default() },
    ] {
        assert!(text::rewrite_block(&mut doc, 0, 0, None, &style).is_err(), "{style:?}");
    }
    // A zero width is clamped to one character's width rather than looping or vanishing.
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { width: Some(0.0), ..Default::default() }).unwrap();
    assert_eq!(text::text_blocks(&reopen(&doc), 0).unwrap().iter().map(|b| b.text.split_whitespace().count()).sum::<usize>(), 12);
}

/// A page split into several content streams, as AutoCAD writes them: the first scales the
/// page (`0.12 0 0 0.12 0 0 cm`, no `q`), the text comes in a later one.
fn split_page(first: &str, second: &str) -> Document {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents [4 0 R 5 0 R] /Resources << /Font << /F1 6 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{first}\nendstream", first.len()),
        format!("<< /Length {} >>\nstream\n{second}\nendstream", second.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
    ];
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

#[test]
fn the_graphics_state_carries_over_between_content_streams() {
    let close = |a: f64, b: f64| (a - b).abs() < 0.5;
    // Text at (1000, 2000) in a space scaled by 0.12: on the page at (120, 240), 12 pt.
    let mut doc = split_page("0.12 0 0 0.12 0 0 cm", "BT /F1 100 Tf 1000 2000 Td (Site plan) Tj ET");
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 1, "{lines:?}");
    let l = &lines[0];
    assert!(close(l.rect[0], 120.0) && l.rect[1] > 230.0 && l.rect[3] < 260.0, "{:?}", l.rect);
    assert!(close(l.size, 12.0), "{}", l.size);
    // Editing keeps the paragraph where it was, at its size; moving it moves it in page space.
    text::rewrite_block(&mut doc, 0, 0, Some("Site plan, rev. B"), &text::BlockStyle { offset: Some([10.0, -20.0]), ..Default::default() }).unwrap();
    let after = text::text_blocks(&reopen(&doc), 0).unwrap();
    assert_eq!(after[0].text, "Site plan, rev. B");
    assert!(close(after[0].rect[0], 130.0) && close(after[0].rect[1], l.rect[1] - 20.0), "{:?}", after[0].rect);
    assert!(close(after[0].size, 12.0), "{}", after[0].size);
    // A `q` left open in one stream and closed in the next still restores the state.
    let doc = split_page("q 0.5 0 0 0.5 0 0 cm", "Q BT /F1 10 Tf 100 100 Td (Footpath) Tj ET");
    let l = &text::text_lines(&doc, 0).unwrap()[0];
    assert!(close(l.rect[0], 100.0) && close(l.size, 10.0), "{l:?}");
}

/// One page whose font draws a few Hebrew letters (ToUnicode): A ב, B ד, C י, D ו, E ק, F ש,
/// G ל, H ם. Content streams draw Hebrew in visual order, so "בדיוק" is `(EDCBA)`.
fn hebrew_page(content: &str) -> Document {
    let cmap = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapType 2 def \
                1 begincodespacerange <00> <FF> endcodespacerange 9 beginbfchar \
                <20> <0020> <41> <05D1> <42> <05D3> <43> <05D9> <44> <05D5> <45> <05E7> <46> <05E9> <47> <05DC> <48> <05DD> \
                endbfchar endcmap CMapName currentdict /CMap defineresource pop end end";
    // Widths for codes 32..=72: a space, then the eight letters at 65..=72.
    let widths: Vec<&str> = (32..=72)
        .map(|c| {
            if c == 32 {
                "250"
            } else if c >= 65 {
                "500"
            } else {
                "0"
            }
        })
        .collect();
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+FakeHebrew /FirstChar 32 /LastChar 72 /Widths [{}] /ToUnicode 6 0 R >>",
            widths.join(" ")
        ),
        format!("<< /Length {} >>\nstream\n{cmap}\nendstream", cmap.len()),
    ];
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

/// The report: Edit text showed "בדיוק" (exactly) reversed, because the line was read in the
/// order the page draws it.
#[test]
fn hebrew_lines_read_in_typing_order() {
    let doc = hebrew_page("BT /F1 12 Tf 300 700 Td (EDCBA) Tj ET BT /F1 12 Tf 300 600 Td (EDCBA HDGF) Tj ET");
    let lines: Vec<String> = text::text_lines(&doc, 0).unwrap().into_iter().map(|l| l.text).collect();
    assert_eq!(lines, ["בדיוק", "שלום בדיוק"]);
    let blocks: Vec<String> = text::text_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(blocks, ["בדיוק", "שלום בדיוק"]);
}

/// Text the page's own font can show is written back in drawing order, right-aligned to where
/// the line ended, and reads back as typed.
#[test]
fn hebrew_edits_are_drawn_right_to_left_and_keep_their_right_edge() {
    let mut doc = hebrew_page("BT /F1 12 Tf 300 700 Td (EDCBA) Tj ET");
    let before = text::text_lines(&doc, 0).unwrap()[0].rect;
    // One line: the bytes are the visual order of the typed text.
    assert_eq!(text::replace_line(&mut doc, 0, 0, "שלום בדיוק").unwrap().substituted, None);
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert!(content.contains("(EDCBA HDGF)"), "{content}");
    assert_eq!(text::text_lines(&reopen(&doc), 0).unwrap()[0].text, "שלום בדיוק");

    // A paragraph: shorter text stays right-aligned at the old right edge.
    let mut doc = hebrew_page("BT /F1 12 Tf 300 700 Td (EDCBA) Tj ET");
    assert_eq!(text::replace_block(&mut doc, 0, 0, "דוב").unwrap().substituted, None);
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert!(content.contains("(ADB)"), "{content}");
    let doc = reopen(&doc);
    let line = text::text_lines(&doc, 0).unwrap().swap_remove(0);
    assert_eq!(line.text, "דוב");
    assert!((line.rect[2] - before[2]).abs() < 0.5, "right edge {} → {}", before[2], line.rect[2]);
    assert!(line.rect[0] > before[0] + 1.0, "shorter text starts further right: {:?} vs {before:?}", line.rect);

    // Longer text grows to the left, not off the right edge.
    let mut doc = hebrew_page("BT /F1 12 Tf 300 700 Td (EDCBA) Tj ET");
    text::replace_block(&mut doc, 0, 0, "שלום בדיוק שלום").unwrap();
    let doc = reopen(&doc);
    let line = text::text_lines(&doc, 0).unwrap().swap_remove(0);
    assert_eq!(line.text, "שלום בדיוק שלום");
    assert!((line.rect[2] - before[2]).abs() < 0.5, "right edge {} → {}", before[2], line.rect[2]);
    assert!(line.rect[0] < before[0] - 1.0, "longer text grows left: {:?} vs {before:?}", line.rect);

    // An alignment asked for still wins.
    let mut doc = hebrew_page("BT /F1 12 Tf 300 700 Td (EDCBA) Tj ET");
    let left = text::BlockStyle { align: Some(Align::Left), ..Default::default() };
    text::rewrite_block(&mut doc, 0, 0, Some("דוב"), &left).unwrap();
    let line = text::text_lines(&reopen(&doc), 0).unwrap().swap_remove(0);
    assert!((line.rect[0] - before[0]).abs() < 0.5, "left-aligned: {:?} vs {before:?}", line.rect);
}

/// A generated Type 3 font numbers its glyphs from 1, so code 32 is a character, not the space:
/// justified rows are stretched after each space, never with word spacing.
#[test]
fn justified_text_in_the_type3_fallback_stretches_its_spaces() {
    let src = "BT /F1 12 Tf 14 TL 100 700 Td (Some words to fill a row of text) Tj T* (and another one below it) Tj T* (end) Tj ET";
    let mut doc = text_page(src);
    let rect = text::text_blocks(&doc, 0).unwrap()[0].rect;
    // More than 32 different characters, so one of them gets code 32.
    let typed = "日本語の 文章を 書きます 東京 大阪 京都 名古屋 札幌 福岡 神戸 横浜 仙台 広島 千葉 新潟 静岡 熊本 岡山 金沢";
    let style = text::BlockStyle { align: Some(added::Align::Justify), ..Default::default() };
    let rewritten = text::rewrite_block(&mut doc, 0, 0, Some(typed), &style);
    if pdfcraft_fonts::document_japanese_font().is_none() {
        assert!(rewritten.is_err());
        eprintln!("built without a craft-fonts Japanese face: the Type 3 path is checked with CRAFT_FONTS_DIR");
        return;
    }
    rewritten.unwrap();
    let lines = text::text_lines(&doc, 0).unwrap();
    assert!(lines.len() > 2, "{lines:?}");
    for l in &lines[..lines.len() - 1] {
        assert!((l.rect[2] - rect[2]).abs() < 1.0, "{:?} spans {:?}, not {rect:?}", l.text, l.rect);
    }
    let ops = pdfcraft_content::parse(&page_content_bytes(&doc, 0)).ops;
    assert!(ops.iter().any(|o| o.is("TJ")), "{ops:?}");
    assert!(ops.iter().filter(|o| o.is("Tw")).all(|o| o.operands.first().and_then(|n| n.as_f64()) == Some(0.0)), "{ops:?}");
}

/// Letters the page's font lacks are drawn with the Hebrew face from craft-fonts; a build
/// without it says so and changes nothing. Hebrew points are refused, not misdrawn.
#[test]
fn hebrew_letters_the_page_font_lacks_use_the_hebrew_face() {
    let mut doc = hebrew_page("BT /F1 12 Tf 300 700 Td (EDCBA) Tj ET");
    let before = page_content_bytes(&doc, 0);
    let pointed = text::replace_line(&mut doc, 0, 0, "שָׁלוֹם עוֹלָם");
    let Err(EditError::Invalid(msg)) = pointed else { panic!("expected a clear error, got {pointed:?}") };
    assert!(msg.contains("niqqud"), "{msg}");
    assert_eq!(page_content_bytes(&doc, 0), before);

    // ע and ה aren't in the page's font.
    let typed = "שלום עולם, עמוד 12 (PDF)";
    let line = text::replace_line(&mut doc, 0, 0, typed);
    if pdfcraft_fonts::document_hebrew_font(false).is_none() {
        let Err(EditError::Invalid(msg)) = line else { panic!("expected a clear error, got {line:?}") };
        assert!(msg.contains("Hebrew fallback font") && msg.contains("CRAFT_FONTS_DIR"), "{msg}");
        assert_eq!(page_content_bytes(&doc, 0), before);
        eprintln!("built without a craft-fonts Hebrew face: the Type 3 path is checked with CRAFT_FONTS_DIR");
        return;
    }
    assert_eq!(line.unwrap().substituted.as_deref(), Some("Noto Sans Hebrew Type3"));
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert!(content.contains("/PCHe"), "{content}");
    let reopened = reopen(&doc);
    assert_eq!(text::text_lines(&reopened, 0).unwrap()[0].text, typed);
    // Every glyph has an outline, so nothing is drawn as a box.
    let p = pdfcraft_model::pages(&reopened).swap_remove(0);
    let res = reopened.resolve(p.dict.get(b"Resources").unwrap());
    let fonts = reopened.resolve(res.as_dict().unwrap().get(b"Font").unwrap());
    let font = reopened.resolve(fonts.as_dict().unwrap().get(b"PCHe").unwrap());
    let procs = reopened.resolve(font.as_dict().unwrap().get(b"CharProcs").unwrap());
    let procs = procs.as_dict().unwrap();
    assert_eq!(procs.iter().count(), typed.chars().collect::<std::collections::HashSet<_>>().len());
    for (_, glyph) in procs.iter() {
        let pdfcraft_cos::Object::Stream(s) = &*reopened.resolve(glyph) else { panic!("glyph stream") };
        let path = String::from_utf8(s.decoded().unwrap()).unwrap();
        assert!(path.starts_with(|c: char| c.is_ascii_digit()) && path.contains(" d1"), "{path}");
    }

    // A whole paragraph too, right-aligned.
    let mut doc = hebrew_page("BT /F1 12 Tf 300 700 Td (EDCBA) Tj ET");
    assert_eq!(text::replace_block(&mut doc, 0, 0, "עברית מלאה").unwrap().substituted.as_deref(), Some("Noto Sans Hebrew Type3"));
    assert_eq!(text::text_blocks(&reopen(&doc), 0).unwrap()[0].text, "עברית מלאה");
}

/// #2013: the dialog asks `first_undrawable` before offering OK, so it must refuse exactly what
/// applying refuses: a Cyrillic footer was previewed (in the interface font) and then failed.
#[test]
fn header_and_watermark_drawability_matches_what_applying_refuses() {
    let footer = |t: &str| HeaderFooter {
        text: [String::new(), String::new(), String::new(), String::new(), String::new(), t.into()],
        ..HeaderFooter::default()
    };
    for (text, refused) in [
        ("Страница <<1>> из 1 / Page <<1>> of 1", Some('С')),
        ("TEST-001 <<Page 1 of n>>", None),
        ("Café — 5€ ™ <<Bates Number#6#1#ACME-#>>", None),
        ("Left\r\nRight", None),
        ("A\rB", Some('\r')),
        ("line one\nΕλληνικά", Some('Ε')),
    ] {
        let hf = footer(text);
        assert_eq!(hf.first_undrawable(), refused, "{text:?}");
        let mut doc = fixture();
        let applied = add_header_footer(&mut doc, &[0], &hf, false, &cx());
        match refused {
            Some(c) => assert_eq!(applied.unwrap_err().to_string(), undrawable_message(c), "{text:?}"),
            None => applied.unwrap(),
        }
        let wm = Watermark { text: text.into(), ..Watermark::default() };
        assert_eq!(wm.first_undrawable(), refused, "{text:?}");
        let applied = add_watermark(&mut fixture(), &[0], &wm, false);
        assert_eq!(applied.err().map(|e| e.to_string()), refused.map(undrawable_message), "{text:?}");
    }
}

/// #125: page-content tools refuse text the standard fonts can't draw instead of writing `?`,
/// and leave the document untouched.
#[test]
fn page_text_the_standard_fonts_cant_draw_is_refused() {
    let mut doc = fixture();
    let before: Vec<Vec<String>> = (0..3).map(|p| streams(&reopen(&doc), p)).collect();
    let text = AddedText { rect: [72.0, 600.0, 300.0, 700.0], text: "日本語のテキスト".into(), ..AddedText::default() };
    let err = add_content(&mut doc, 0, &Content::Text(text)).unwrap_err().to_string();
    assert!(err.contains("\"日\" (U+65E5)"), "{err}");
    let hf = HeaderFooter {
        text: ["Ελληνικά".into(), String::new(), String::new(), String::new(), String::new(), String::new()],
        ..HeaderFooter::default()
    };
    assert!(add_header_footer(&mut doc, &[0, 1], &hf, true, &cx()).unwrap_err().to_string().contains("U+0395"));
    let wm = Watermark { text: "机密".into(), ..Watermark::default() };
    assert!(add_watermark(&mut doc, &[0], &wm, true).unwrap_err().to_string().contains("U+673A"));
    let after: Vec<Vec<String>> = (0..3).map(|p| streams(&reopen(&doc), p)).collect();
    assert_eq!(before, after, "a refused edit changes nothing");
    // Western European text, including what only WinAnsi (not Latin-1) has, still works.
    let text = AddedText { rect: [72.0, 600.0, 300.0, 700.0], text: "Café — 5€ ™".into(), ..AddedText::default() };
    add_content(&mut doc, 0, &Content::Text(text.clone())).unwrap();
    // Retyping existing text is refused the same way, and the whole document is left as it was.
    let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let retyped = AddedText { text: "Café 中".into(), ..text.clone() };
    assert!(update_content(&mut doc, 0, 0, &Content::Text(retyped)).unwrap_err().to_string().contains("U+4E2D"));
    assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), saved, "a refused update changes nothing");
    // A carriage return is checked where it would be drawn: added text splits on LF only, so
    // CRLF would leave a `?`; headers and watermarks split CRLF into lines, but not a lone CR.
    let crlf = AddedText { text: "A\r\nB".into(), ..text };
    assert!(add_content(&mut doc, 0, &Content::Text(crlf)).unwrap_err().to_string().contains("U+000D"));
    let hf = |t: &str| HeaderFooter {
        text: [t.into(), String::new(), String::new(), String::new(), String::new(), String::new()],
        ..HeaderFooter::default()
    };
    add_header_footer(&mut doc, &[0], &hf("Left\r\n<<Bates Number#6#1#ACME-#>>"), true, &cx()).unwrap();
    assert!(add_header_footer(&mut doc, &[0], &hf("A\rB"), true, &cx()).unwrap_err().to_string().contains("U+000D"));
    // Arabic is shaped with the craft-fonts Arabic face (#403): drawable when that face is built
    // in, refused (never written as `?`) when it isn't. Text left to the standard font still counts.
    let arabic = |text: &str| AddedText { rect: [72.0, 600.0, 300.0, 700.0], text: text.into(), ..AddedText::default() };
    // U+061C is in the Arabic block but is a direction mark that's dropped, not shaped: it never
    // makes text drawable, nor (with the face) undrawable.
    assert!(add_content(&mut doc, 0, &Content::Text(arabic("\u{061C}中"))).is_err());
    if pdfcraft_fonts::arabic_has('م') {
        add_content(&mut doc, 0, &Content::Text(arabic("مرحبا Hello"))).unwrap();
        add_content(&mut doc, 0, &Content::Text(arabic("\u{061C}Hello"))).unwrap();
        for text in ["مرحبا 中", "\u{061C}中"] {
            let err = add_content(&mut doc, 0, &Content::Text(arabic(text))).unwrap_err().to_string();
            assert!(err.contains("U+4E2D"), "{text:?}: {err}");
        }
    } else {
        // #403's own refusal says what's missing; the Add-text editor still keeps the draft.
        let err = add_content(&mut doc, 0, &Content::Text(arabic("مرحبا Hello"))).unwrap_err().to_string();
        assert!(err.contains("CRAFT_FONTS_DIR"), "{err}");
        assert_eq!(first_undrawable(&arabic("مرحبا Hello")), Some('م'));
        // An existing item may still take Arabic (#403 keeps it movable), but nothing else that
        // the standard font would write as `?`.
        let n = list_added(&doc).iter().filter(|a| a.page == 0).count();
        add_content(&mut doc, 0, &Content::Text(arabic("x"))).unwrap();
        let err = update_content(&mut doc, 0, n, &Content::Text(arabic("ب中"))).unwrap_err().to_string();
        assert!(err.contains("U+4E2D"), "{err}");
        update_content(&mut doc, 0, n, &Content::Text(arabic("ب"))).unwrap();
    }
}

fn xobject(doc: &Document, page: usize, name: &[u8]) -> Option<pdfcraft_cos::ObjRef> {
    let p = &pdfcraft_model::pages(doc)[page];
    let res = doc.resolve(p.dict.get(b"Resources")?);
    let xo = doc.resolve(res.as_dict()?.get(b"XObject")?);
    xo.as_dict()?.get(name).and_then(Object::as_ref)
}

fn annot_at(doc: &Document, page: usize, index: usize) -> pdfcraft_cos::ObjRef {
    let p = &pdfcraft_model::pages(doc)[page];
    let list = doc.resolve(p.dict.get(b"Annots").unwrap());
    list.as_array().unwrap()[index].as_ref().unwrap()
}

#[test]
fn fill_sign_flatten_bakes_marks_keeps_other_comments_and_does_not_reuse_pcfl0() {
    use pdfcraft_annot::{FillMark, Markup, Meta, NewAnnotation, Shape, Style, add_annotation, summaries};
    let mut doc = fixture();
    assert_eq!(flatten_fill_sign(&mut doc, &[0]).unwrap(), 0);
    assert!(!doc.is_modified(), "nothing to flatten leaves the file alone");

    let meta = Meta { date: None, id: "x".into() };
    let add = |doc: &mut Document, shape: Shape, contents: &str| {
        let style = Style::default_for(&shape);
        add_annotation(doc, &NewAnnotation { page: 0, shape, style, contents: contents.into(), author: "a".into() }, &meta).unwrap()
    };
    add(&mut doc, Shape::Rectangle { rect: [10.0, 10.0, 80.0, 40.0] }, "box");
    assert_eq!(flatten(&mut doc, &[0], true, false).unwrap(), 1);
    let kept = xobject(&doc, 0, b"PCFl0").expect("the first flatten's XObject");

    add(&mut doc, Shape::Typewriter { rect: [20.0, 500.0, 140.0, 520.0], font_size: 10.0 }, "Hello");
    doc.update_dict(annot_at(&doc, 0, 0), |d| {
        d.remove(b"PCFillSign");
    })
    .unwrap();
    add(&mut doc, Shape::Mark { rect: [100.0, 100.0, 120.0, 120.0], mark: FillMark::Check }, "");
    add(&mut doc, Shape::Signature { strokes: vec![vec![[10.0, 50.0], [40.0, 80.0], [70.0, 50.0]]] }, "");
    doc.update_dict(annot_at(&doc, 0, 2), |d| d.set(b"Subj".to_vec(), pdfcraft_cos::PdfString::text("Pencil"))).unwrap();
    add(&mut doc, Shape::TypedSignature { rect: [20.0, 40.0, 90.0, 70.0], contours: vec![vec![[0.1, 0.2], [0.5, 0.9], [0.9, 0.2]]] }, "");
    add(&mut doc, Shape::TextMarkup { kind: Markup::Highlight, quads: vec![[20.0, 400.0, 80.0, 400.0, 20.0, 390.0, 80.0, 390.0]] }, "keep");
    add(&mut doc, Shape::TextBox { rect: [20.0, 300.0, 140.0, 340.0], font_size: 12.0 }, "Note");
    add(&mut doc, Shape::Ink { strokes: vec![vec![[200.0, 200.0], [220.0, 220.0], [240.0, 200.0]]] }, "pencil");
    let old = add(&mut doc, Shape::Signature { strokes: vec![vec![[300.0, 50.0], [330.0, 80.0], [360.0, 50.0]]] }, "");
    doc.update_dict(annot_at(&doc, 0, old), |d| {
        d.remove(b"PCFillSign");
    })
    .unwrap();
    let hidden = add(&mut doc, Shape::Typewriter { rect: [20.0, 600.0, 140.0, 620.0], font_size: 10.0 }, "secret");
    doc.update_dict(annot_at(&doc, 0, hidden), |d| d.set(b"F".to_vec(), Object::Int(2))).unwrap();

    let n = flatten_fill_sign(&mut doc, &[0]).unwrap();
    assert_eq!(n, 5, "text, check, both signatures and the typed signature; the hidden one stays");
    assert_eq!(xobject(&doc, 0, b"PCFl0"), Some(kept), "an existing PCFl0 is not replaced");
    let newest = streams(&doc, 0).last().unwrap().clone();
    assert!(newest.contains("/PCFl1 Do") && !newest.contains("/PCFl0 Do"), "{newest}");

    let doc = reopen(&doc);
    let mut left: Vec<String> = summaries(&doc).iter().map(|s| s.contents.clone().unwrap_or_default()).collect();
    left.sort();
    assert_eq!(left, ["Note", "keep", "pencil", "secret"]);
    assert!(summaries(&doc).iter().any(|s| s.intent.as_deref() == Some("FreeTextTypeWriter")), "the hidden typewriter stays");
    assert_eq!(summaries(&doc).iter().filter(|s| s.intent.as_deref() == Some("FreeTextTypeWriter")).count(), 1);
}

/// A one-page document from its objects (object 1 is the catalog).
fn build(objs: &[&str]) -> Document {
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

/// #314: text and images drawn by (nested) form XObjects are read for export, placed through
/// every form matrix and set in each form's own fonts; editing still sees only the page's text.
#[test]
fn reading_follows_form_xobjects() {
    let page = stream("", "q 1 0 0 1 10 0 cm /Fm1 Do Q BT /F1 12 Tf 72 100 Td (Page text) Tj ET");
    let fm1 = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 600 800] /Matrix [1 0 0 1 0 50] /Resources << /Font << /F1 7 0 R >> /XObject << /Fm2 8 0 R >> >>",
        "BT /F1 18 Tf 72 700 Td (Hello from a test invoice) Tj ET /Fm2 Do",
    );
    let fm2 = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 400] /Matrix [2 0 0 2 0 0] /Resources << /Font << /F2 9 0 R >> /XObject << /Im1 10 0 R >> >>",
        "BT /F2 10 Tf 50 100 Td (Nested) Tj ET q 20 0 0 10 5 5 cm /Im1 Do Q",
    );
    let image = stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", "A");
    let doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 5 0 R >> /XObject << /Fm1 6 0 R >> >> /Contents 4 0 R >>",
        &page,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>",
        &fm1,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        &fm2,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Times-Bold >>",
        &image,
    ]);
    let close = |a: f64, b: f64| (a - b).abs() < 0.01;

    let blocks = text::reading_blocks(&doc, 0).unwrap();
    let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
    assert_eq!(texts, ["Hello from a test invoice", "Nested", "Page text"]);
    let (hello, nested, own) = (&blocks[0], &blocks[1], &blocks[2]);
    // Hello: (72, 700) moved by the form's (0, 50) and the page's (10, 0).
    assert_eq!(hello.base_font, "Helvetica", "the form's own /F1, not the page's");
    assert!(close(hello.rect[0], 82.0) && close(hello.size, 18.0), "{hello:?}");
    // Nested: (50, 100) doubled by Fm2, then Fm1 and the page: (110, 250), 20 pt.
    assert_eq!(nested.base_font, "Times-Bold");
    assert!(nested.bold);
    assert!(close(nested.rect[0], 110.0) && close(nested.size, 20.0), "{nested:?}");
    assert_eq!(own.base_font, "Courier");

    let images = images::reading_images(&doc, 0).unwrap();
    assert_eq!(images.len(), 1, "{images:?}");
    let r = images[0].rect;
    assert!(close(r[0], 20.0) && close(r[1], 60.0) && close(r[2], 60.0) && close(r[3], 80.0), "{r:?}");
    assert_eq!((images[0].width, images[0].height), (1, 1));

    // Editing works on the page's own streams only, unchanged.
    let editable: Vec<String> = text::text_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(editable, ["Page text"]);
    // The page draws one figure of its own (Fm1, edited as a whole); the image inside Fm2 is not
    // a page image.
    let own = images::page_images(&doc, 0).unwrap();
    assert_eq!(own.len(), 1, "{own:?}");
    assert!(own[0].is_form && own[0].object == Some(pdfcraft_cos::ObjRef::new(6, 0)), "{own:?}");
}

/// A chain of forms, each drawing the next twenty times, twelve deep: within the depth cap but
/// 20^11 visits without a budget. Reading stops at the page's visit budget instead of hanging.
#[test]
fn form_xobjects_that_fan_out_stop_at_the_visit_budget() {
    let font = "/Font << /F1 5 0 R >>";
    let levels = 12;
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 5 0 R >> /XObject << /N 6 0 R >> >> /Contents 4 0 R >>"
            .to_string(),
        stream("", "/N Do"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    for level in 0..levels {
        let next = 7 + level;
        objs.push(if level + 1 == levels {
            stream(&format!("/Subtype /Form /BBox [0 0 9 9] /Resources << {font} >>"), "BT /F1 10 Tf 10 10 Td (leaf) Tj ET")
        } else {
            stream(&format!("/Subtype /Form /BBox [0 0 9 9] /Resources << {font} /XObject << /N {next} 0 R >> >>"), &"/N Do ".repeat(20))
        });
    }
    let refs: Vec<&str> = objs.iter().map(String::as_str).collect();
    let doc = build(&refs);
    let start = std::time::Instant::now();
    let _ = text::reading_blocks(&doc, 0).unwrap();
    assert!(images::reading_images(&doc, 0).unwrap().is_empty());
    assert!(start.elapsed() < std::time::Duration::from_secs(30), "took {:?}", start.elapsed());
}

/// Forms that draw themselves or each other, have no resources, a malformed matrix, or are
/// missing are read once (or skipped) without panicking or looping.
#[test]
fn hostile_form_xobjects_are_read_once() {
    let page = stream("", "/Loop Do /A Do /NoRes Do /Bad Do /Missing Do /F1 Do");
    let font = "/Font << /F1 5 0 R >>";
    let lp = stream(
        &format!("/Subtype /Form /BBox [0 0 9 9] /Resources << {font} /XObject << /Loop 6 0 R >> >>"),
        "/Loop Do BT /F1 10 Tf 10 10 Td (once) Tj ET",
    );
    let a = stream(
        &format!("/Subtype /Form /BBox [0 0 9 9] /Resources << {font} /XObject << /B 8 0 R >> >>"),
        "/B Do BT /F1 10 Tf 10 100 Td (from a) Tj ET",
    );
    let b = stream(
        &format!("/Subtype /Form /BBox [0 0 9 9] /Resources << {font} /XObject << /A 7 0 R >> >>"),
        "/A Do BT /F1 10 Tf 10 200 Td (from b) Tj ET",
    );
    let nores = stream("/Subtype /Form /BBox [0 0 9 9]", "BT /F1 10 Tf 10 300 Td (inherited) Tj ET");
    let bad = stream(&format!("/Subtype /Form /BBox [0 0 9 9] /Matrix [1 0 0] /Resources << {font} >>"), "BT /F1 10 Tf 10 400 Td (bad matrix) Tj ET");
    let doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 5 0 R >> /XObject << /Loop 6 0 R /A 7 0 R /NoRes 9 0 R /Bad 10 0 R >> >> /Contents 4 0 R >>",
        &page,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        &lp,
        &a,
        &b,
        &nores,
        &bad,
    ]);
    let texts: Vec<String> = text::reading_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(texts, ["once", "from b", "from a", "inherited", "bad matrix"]);
    assert!(images::reading_images(&doc, 0).unwrap().is_empty());
}

#[test]
fn added_thai_text_wraps_clusters_and_survives_reopen() {
    let mut doc = fixture();
    let t = AddedText {
        rect: [20.0, 100.0, 250.0, 300.0], text: "สวัสดี น้ำ ผู้ใช้ ปู่ ๑๒๓ Hello 123".into(), size: 16.0, ..AddedText::default()
    };
    if pdfcraft_fonts::document_thai_font().is_none() {
        assert_eq!(first_undrawable(&t), Some('ส'));
        let before = list_added(&doc);
        let err = add_content(&mut doc, 0, &Content::Text(t)).unwrap_err().to_string();
        assert!(err.contains("Thai") && err.contains("CRAFT_FONTS_DIR"), "{err}");
        assert_eq!(list_added(&doc), before);
        return;
    }
    assert_eq!(first_undrawable(&t), None);
    add_content(&mut doc, 0, &Content::Text(t.clone())).unwrap();
    let reopened = reopen(&doc);
    assert_eq!(list_added(&reopened)[0].content, Content::Text(AddedText { rect: super::added::text_rect(&t), ..t.clone() }));
    let narrow = AddedText { rect: [0.0, 0.0, 16.0, 200.0], text: "กิ่กิ่กิ่".into(), ..t.clone() };
    assert_eq!(super::added::lines(&narrow), ["กิ่", "กิ่", "กิ่"]);
    update_content(&mut doc, 0, 0, &Content::Text(AddedText { text: "แก้ไข น้ำ".into(), ..t.clone() })).unwrap();
    assert!(!own(&doc, 0).is_empty());
    let bad = Content::Text(AddedText { text: "ไทย 日".into(), ..t });
    let before = list_added(&doc);
    assert!(update_content(&mut doc, 0, 0, &bad).is_err());
    assert_eq!(list_added(&doc), before);
    delete_content(&mut doc, 0, 0).unwrap();
    assert!(list_added(&doc).is_empty());
    assert!(arabic_font_names(&doc, 0).is_empty());
}

#[test]
fn group_moves_preserve_glyph_bytes_and_unselected_text_state() {
    for content in [
        "BT /F1 12 Tf 1.3 Tc 2 Tw 80 Tz 14 TL 1 0.2 -0.1 1 50 600 Tm (A\\101) Tj 1 0 0 rg (Neighbour) Tj T* (Next) Tj ET",
        "BT /F1 12 Tf 14 TL 50 600 Td (Before) Tj 0 0 1 rg (Quoted) ' 1 0 0 rg (After) Tj ET",
        "BT /F1 12 Tf 14 TL 50 600 Td (Before) Tj 0 0 1 rg 2 1 (Quoted) \" 1 0 0 rg [(After) -120 (wards)] TJ T* (Last) Tj ET",
        "q 1.2 0.3 -0.1 0.9 20 30 cm BT /F1 12 Tf 50 600 Td [(A) -120 (B)] TJ 1 0 0 rg (After) Tj ET Q %keep-this-comment\n",
        "BT /F1 12.3456789 Tf 81.23456789 Tz 0.123456789 Ts 14 TL 50 600 Td (Before) Tj 0 0 1 rg 2.123456789 1.987654321 (Quoted) \" 1 0 0 rg (After) Tj ET",
    ] {
        let mut doc = text_page(content);
        let before = text::text_lines(&doc, 0).unwrap();
        let blocks = text::text_blocks(&doc, 0).unwrap();
        let index = blocks.iter().position(|b| b.text.contains("Quoted")).unwrap_or(0);
        let selected: Vec<_> = blocks[index].lines.iter().map(|i| before[*i].text.clone()).collect();
        let glyph_bytes = |doc: &Document| {
            pdfcraft_content::parse(&page_content_bytes(doc, 0))
                .ops
                .iter()
                .flat_map(|op| {
                    op.operands
                        .iter()
                        .flat_map(|o| match o {
                            Object::String(s) => vec![s.bytes.clone()],
                            Object::Array(a) => a.iter().filter_map(|v| v.as_string().map(|s| s.bytes.clone())).collect(),
                            _ => Vec::new(),
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        };
        let original_bytes = glyph_bytes(&doc);
        let target = ObjectTarget { kind: ObjectKind::Text, index };
        move_objects(&mut doc, 0, &[target], [35.0, -27.0]).unwrap();
        let doc = reopen(&doc);
        assert_eq!(glyph_bytes(&doc), original_bytes, "original codes and kerning strings survive");
        let after = text::text_lines(&doc, 0).unwrap();
        assert_eq!(after.len(), before.len(), "{content}");
        for old in before {
            let new = after.iter().find(|l| l.text == old.text).unwrap();
            let expected = if selected.contains(&old.text) {
                [old.rect[0] + 35.0, old.rect[1] - 27.0, old.rect[2] + 35.0, old.rect[3] - 27.0]
            } else {
                old.rect
            };
            assert!(new.rect.iter().zip(expected).all(|(a, b)| (a - b).abs() < 1e-6), "{}: {:?} != {:?}", old.text, new.rect, expected);
            assert_eq!((new.font.clone(), new.color), (old.font, old.color));
        }
        if content.contains("%keep-this-comment") {
            assert!(String::from_utf8_lossy(&page_content_bytes(&doc, 0)).contains("%keep-this-comment"));
        }
    }
}

#[test]
fn group_moves_preserve_small_offsets_under_large_transforms() {
    let mut doc = image_page();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let content = doc.add(Object::Stream(Stream::flate(
        Dict::new(),
        b"q 100000 0 0 100000 100 100 cm /Im0 Do Q q 10000 0 0 10000 0 0 cm BT /F1 0.00123456789 Tf 0.02 0.06 Td (Small) Tj 1 0 0 rg (Neighbour) Tj ET Q",
    )));
    doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Ref(content))).unwrap();
    let before = editable_objects(&doc, 0).unwrap();
    let targets = before.iter().filter(|o| o.text.as_deref() != Some("Neighbour")).map(|o| o.target).collect::<Vec<_>>();
    assert_eq!(targets.len(), 2);
    let image_before = images::page_images(&doc, 0).unwrap()[0].rect;
    let offset = [0.00123456789, -0.00987654321];
    move_objects(&mut doc, 0, &targets, offset).unwrap();
    let reopened = reopen(&doc);
    let image_after = images::page_images(&reopened, 0).unwrap()[0].clone();
    assert!(
        close(image_after.rect, [image_before[0] + offset[0], image_before[1] + offset[1], image_before[2] + offset[0], image_before[3] + offset[1]]),
        "full image placement preserves sub-point movement"
    );
    let after = editable_objects(&reopened, 0).unwrap();
    for old in before {
        let new = after.iter().find(|o| o.target == old.target).unwrap();
        let [dx, dy] = if targets.contains(&old.target) { offset } else { [0.0, 0.0] };
        if old.target.kind == crate::ObjectKind::Image {
            // The full placement moves; the visible right/top edges stay at the page crop.
            assert_eq!(Some(new.rect), image_after.visible_rect);
        } else {
            assert!(close(new.rect, [old.rect[0] + dx, old.rect[1] + dy, old.rect[2] + dx, old.rect[3] + dy]), "{old:?}: {new:?}");
        }
    }
}

#[test]
fn mixed_object_inventory_does_not_select_added_content_twice() {
    let mut doc = image_page();
    add_content(&mut doc, 0, &Content::Text(AddedText { text: "Added label".into(), rect: [50.0, 350.0, 200.0, 380.0], ..AddedText::default() }))
        .unwrap();
    let before = editable_objects(&doc, 0).unwrap();
    assert_eq!(before.len(), 3, "one native paragraph, one native image, one added text");
    let targets = before.iter().map(|o| o.target).collect::<Vec<_>>();
    let image = images::page_images(&doc, 0).unwrap()[0].object.unwrap();
    let original_image = doc.get(image).clone();
    move_objects(&mut doc, 0, &targets, [20.0, -15.0]).unwrap();
    let doc = reopen(&doc);
    let after = editable_objects(&doc, 0).unwrap();
    assert_eq!(after.len(), before.len());
    for old in before {
        let new = after.iter().find(|o| o.target == old.target).unwrap();
        assert!(close(new.rect, [old.rect[0] + 20.0, old.rect[1] - 15.0, old.rect[2] + 20.0, old.rect[3] - 15.0]));
        assert_eq!(new.text, old.text);
    }
    assert_eq!(doc.get(image).clone(), original_image, "pixel data is shared, not recompressed");
}

#[test]
fn group_move_refusals_leave_the_object_graph_untouched() {
    let mut doc = image_page();
    let target = editable_objects(&doc, 0).unwrap()[0].target;
    let original = pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap();
    for (targets, offset) in [
        (vec![], [1.0, 2.0]),
        (vec![target, target], [1.0, 2.0]),
        (vec![target, ObjectTarget { kind: ObjectKind::Image, index: usize::MAX }], [1.0, 2.0]),
        (vec![target], [f64::NAN, 2.0]),
        (vec![target], [f64::INFINITY, 2.0]),
        (vec![target], [0.0, 0.0]),
        (vec![target], [1e300, 2.0]),
        (vec![target; MAX_MOVE_OBJECTS + 1], [1.0, 2.0]),
    ] {
        assert!(move_objects(&mut doc, 0, &targets, offset).is_err());
        assert_eq!(pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap(), original);
    }
    let mut clipped = text_page("BT /F1 12 Tf 50 600 Td (Ordinary) Tj ET BT /F1 12 Tf 4 Tr 50 400 Td (Clipping) Tj ET");
    let targets = editable_objects(&clipped, 0).unwrap().iter().map(|o| o.target).collect::<Vec<_>>();
    let original = pdfcraft_cos::write_full(&clipped, &SaveOptions::default()).unwrap();
    assert!(move_objects(&mut clipped, 0, &targets, [10.0, 5.0]).unwrap_err().to_string().contains("clipping"));
    assert_eq!(pdfcraft_cos::write_full(&clipped, &SaveOptions::default()).unwrap(), original);
}

#[test]
fn added_and_native_objects_move_together_on_cropped_rotated_user_unit_pages() {
    for rotation in [0, 90, 180, 270] {
        let mut doc = image_page();
        let page = pdfcraft_model::pages(&doc)[0].obj;
        doc.update_dict(page, |d| {
            d.set(b"CropBox".to_vec(), Object::Array([10, 20, 500, 750].map(Object::Int).to_vec()));
            d.set(b"Rotate".to_vec(), Object::Int(rotation));
            d.set(b"UserUnit".to_vec(), Object::Int(2));
        })
        .unwrap();
        add_content(&mut doc, 0, &Content::Text(AddedText { text: "Added".into(), rect: [60.0, 100.0, 180.0, 125.0], ..AddedText::default() }))
            .unwrap();
        let before = editable_objects(&doc, 0).unwrap();
        move_objects(&mut doc, 0, &before.iter().map(|o| o.target).collect::<Vec<_>>(), [12.0, -8.0]).unwrap();
        let after = editable_objects(&reopen(&doc), 0).unwrap();
        for (a, b) in before.iter().zip(after) {
            assert!(
                close(b.rect, [a.rect[0] + 12.0, a.rect[1] - 8.0, a.rect[2] + 12.0, a.rect[3] - 8.0]),
                "rotation {rotation}: {:?} -> {:?}",
                a.rect,
                b.rect
            );
        }
    }
}

#[test]
fn group_moves_keep_split_stream_tokens_and_shared_form_resources() {
    let mut doc = split_streams_page();
    let before = text::text_lines(&doc, 0).unwrap();
    let objects: Vec<_> = editable_objects(&doc, 0).unwrap().iter().map(|o| o.target).collect();
    move_objects(&mut doc, 0, &objects, [30.0, -20.0]).unwrap();
    let doc = reopen(&doc);
    let after = text::text_lines(&doc, 0).unwrap();
    assert_eq!(before.len(), after.len());
    for (a, b) in before.iter().zip(&after) {
        assert_eq!(a.text, b.text);
        assert!(close(b.rect, [a.rect[0] + 30.0, a.rect[1] - 20.0, a.rect[2] + 30.0, a.rect[3] - 20.0]));
    }
    let joined = page_content_bytes(&doc, 0);
    let ops = pdfcraft_content::parse(&joined).ops;
    assert_eq!(ops.iter().find(|o| o.is("BDC")).unwrap().operands.len(), 2);
    assert!(ops.iter().any(|o| o.is("EMC")));
    // Two pages share a content stream and a Form; only one placement on one page moves.
    let mut doc = fixture();
    let mut form = Dict::new();
    form.set(b"Subtype".to_vec(), Object::name("Form"));
    form.set(b"BBox".to_vec(), Object::Array([0, 0, 60, 30].map(Object::Int).to_vec()));
    form.set(b"PrivateData".to_vec(), Object::String(pdfcraft_cos::PdfString::literal(b"retain this".to_vec())));
    let artwork = doc.add(Object::Stream(Stream::flate(form, b"1 0 0 rg 0 0 60 30 re f")));
    doc.update_dict(pdfcraft_cos::ObjRef::new(6, 0), |d| {
        let mut xo = Dict::new();
        xo.set(b"Figure".to_vec(), Object::Ref(artwork));
        d.set(b"XObject".to_vec(), Object::Dict(xo));
    })
    .unwrap();
    doc.set(
        pdfcraft_cos::ObjRef::new(7, 0),
        Object::Stream(Stream::flate(
            Dict::new(),
            b"q 1 0 0 1 40 80 cm /Figure Do Q q 1 0 0 1 300 400 cm /Figure Do Q BT /F1 9 Tf 40 160 Td (Caption) Tj ET",
        )),
    );
    let original_form = doc.get(artwork);
    let source_page = page_content_bytes(&doc, 1);
    let images = images::page_images(&doc, 0).unwrap();
    move_objects(
        &mut doc,
        0,
        &[ObjectTarget { kind: ObjectKind::Image, index: 0 }, ObjectTarget { kind: ObjectKind::Text, index: 0 }],
        [15.0, -10.0],
    )
    .unwrap();
    let after = images::page_images(&doc, 0).unwrap();
    assert!(close(after[0].rect, [55.0, 70.0, 115.0, 100.0]));
    assert_eq!(after[1].rect, images[1].rect);
    assert_eq!(*doc.get(artwork), *original_form);
    assert_eq!(page_content_bytes(&doc, 1), source_page);
}

#[test]
fn justified_text_in_a_composite_font_fills_each_row_with_adjustments_after_its_spaces() {
    // Two-byte codes, so word spacing (single-byte code 32 only) can't stretch the spaces.
    let hex = |s: &str| s.bytes().map(|b| format!("00{b:02x}")).collect::<String>();
    let src = format!(
        "BT /F1 10 Tf 12 TL 100 700 Td <{}> Tj T* <{}> Tj T* <{}> Tj ET",
        hex("One two three four five six seven"),
        hex("eight nine ten eleven twelve"),
        hex("and then the end of it")
    );
    let mut doc = text_page(&src);
    let unicode = doc.add(Object::Stream(Stream::flate(
        Dict::new(),
        b"1 begincodespacerange <0000> <ffff> endcodespacerange 1 beginbfrange <0020> <007e> <0020> endbfrange",
    )));
    let font = Dict::from_iter([
        (b"Type".to_vec(), Object::name("Font")),
        (b"Subtype".to_vec(), Object::name("Type0")),
        (b"BaseFont".to_vec(), Object::name("SyntheticCID")),
        (b"Encoding".to_vec(), Object::name("Identity-H")),
        (b"ToUnicode".to_vec(), Object::Ref(unicode)),
        (
            b"DescendantFonts".to_vec(),
            Object::Array(vec![Object::Dict(Dict::from_iter([
                (b"Subtype".to_vec(), Object::name("CIDFontType2")),
                (b"DW".to_vec(), Object::Int(500)),
            ]))]),
        ),
    ]);
    let p = pdfcraft_model::pages(&doc)[0].obj;
    let resources = Dict::from_iter([(b"Font".to_vec(), Object::Dict(Dict::from_iter([(b"F1".to_vec(), Object::Dict(font))])))]);
    doc.update_dict(p, |d| d.set(b"Resources".to_vec(), Object::Dict(resources))).unwrap();
    let before = text::text_blocks(&doc, 0).unwrap()[0].clone();
    let right = before.rect[2];
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { align: Some(added::Align::Justify), ..Default::default() }).unwrap();
    let lines = text::text_lines(&doc, 0).unwrap();
    assert!(lines.len() > 2, "{lines:?}");
    for l in &lines[..lines.len() - 1] {
        assert!((l.rect[2] - right).abs() < 1.0, "{:?} ends at {} not {right}", l.text, l.rect[2]);
    }
    // The source font shows the text, with its spaces, and the text reads as before, rewrapped.
    let flat = |s: &str| s.replace('\n', " ");
    assert_eq!(flat(&text::text_blocks(&doc, 0).unwrap()[0].editor_text), flat(&before.editor_text));
    let ops = pdfcraft_content::parse(&page_content_bytes(&doc, 0)).ops;
    assert!(ops.iter().all(|o| !o.is("Tf") || o.operands.first().and_then(|n| n.as_name()) == Some(b"F1".as_slice())), "{ops:?}");
    assert!(ops.iter().any(|o| o.is("TJ")), "{ops:?}");
    assert!(ops.iter().filter(|o| o.is("Tw")).all(|o| o.operands.first().and_then(|n| n.as_f64()) == Some(0.0)), "{ops:?}");
}

#[test]
fn group_moves_preserve_encoded_glyphs_and_refuse_vertical_cmaps() {
    let mut doc = text_page("BT /F1 12 Tf 40 400 Td <004100420043> Tj ET");
    let mut font = Dict::from_iter([
        (b"Type".to_vec(), Object::name("Font")),
        (b"Subtype".to_vec(), Object::name("Type0")),
        (b"BaseFont".to_vec(), Object::name("SyntheticCID")),
        (b"Encoding".to_vec(), Object::name("Identity-H")),
    ]);
    let unicode = doc.add(Object::Stream(Stream::flate(
        Dict::new(),
        b"1 begincodespacerange <0000> <ffff> endcodespacerange 3 beginbfchar <0041> <0041> <0042> <0042> <0043> <0043> endbfchar",
    )));
    font.set(b"ToUnicode".to_vec(), Object::Ref(unicode));
    font.set(
        b"DescendantFonts".to_vec(),
        Object::Array(vec![Object::Dict(Dict::from_iter([(b"Subtype".to_vec(), Object::name("CIDFontType2")), (b"DW".to_vec(), Object::Int(600))]))]),
    );
    // No external font asset: this synthetic font is enough to exercise raw CID bytes.
    let p = pdfcraft_model::pages(&doc)[0].obj;
    let mut resources = Dict::new();
    let mut fonts = Dict::new();
    fonts.set(b"F1".to_vec(), Object::Dict(font.clone()));
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(p, |d| d.set(b"Resources".to_vec(), Object::Dict(resources.clone()))).unwrap();
    let original = text::text_lines(&doc, 0).unwrap();
    assert!(!original.is_empty());
    move_objects(&mut doc, 0, &[ObjectTarget { kind: ObjectKind::Text, index: 0 }], [25.0, 15.0]).unwrap();
    let encoded =
        |doc: &Document| pdfcraft_content::parse(&page_content_bytes(doc, 0)).ops.into_iter().find(|o| o.is("Tj")).unwrap().operands[0].clone();
    assert_eq!(encoded(&doc).as_string().unwrap().bytes, [0, 65, 0, 66, 0, 67]);
    let after = text::text_lines(&doc, 0).unwrap();
    assert_eq!(original[0].text, after[0].text);
    for encoding in [
        Object::name("Identity-V"),
        Object::Stream(Stream::flate(Dict::new(), b"/WMode 1 def")),
        Object::Stream(Stream::flate(Dict::from_iter([(b"WMode".to_vec(), Object::Int(1))]), b"")),
    ] {
        let mut f = font.clone();
        f.set(b"Encoding".to_vec(), encoding);
        let mut fonts = Dict::new();
        fonts.set(b"F1".to_vec(), Object::Dict(f));
        resources.set(b"Font".to_vec(), Object::Dict(fonts));
        doc.update_dict(p, |d| d.set(b"Resources".to_vec(), Object::Dict(resources.clone()))).unwrap();
        let before = pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap();
        assert!(move_objects(&mut doc, 0, &[ObjectTarget { kind: ObjectKind::Text, index: 0 }], [5.0, 5.0]).is_err());
        assert_eq!(pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap(), before);
    }
}

#[test]
fn moving_added_content_after_page_rotation_preserves_streams_and_resources() {
    let mut doc = text_page("BT /F1 12 Tf 40 400 Td (Native) Tj ET");
    added::add_content(
        &mut doc,
        0,
        &Content::Text(added::AddedText { text: "Added label".into(), rect: [50.0, 300.0, 200.0, 330.0], ..Default::default() }),
    )
    .unwrap();
    let p = pdfcraft_model::pages(&doc)[0].obj;
    let resources = pdfcraft_model::pages(&doc)[0].dict.get(b"Resources").cloned();
    let item = added::list_added(&doc)[0].clone();
    let source = stream_bytes(&doc, &Object::Ref(item.obj)).unwrap();
    let content_after_cm = |bytes: &[u8]| {
        let ops = pdfcraft_content::parse(bytes).ops;
        bytes[ops[1].span.end..].to_vec()
    };
    for rotation in [90, 180, 270, 0] {
        doc.update_dict(p, |d| d.set(b"Rotate".to_vec(), Object::Int(rotation))).unwrap();
        let before_lines = text::text_lines(&doc, 0).unwrap();
        let before_objects = editable_objects(&doc, 0).unwrap();
        let a = before_objects.iter().find(|o| o.target.kind == ObjectKind::Added).unwrap();
        // The logical added box must contain the actual source glyph geometry after rotation.
        let label = before_lines.iter().find(|l| l.text == "Added label").unwrap();
        assert!(
            label.rect[0] >= a.rect[0] - 0.01
                && label.rect[2] <= a.rect[2] + 0.01
                && label.rect[1] >= a.rect[1] - 0.01
                && label.rect[3] <= a.rect[3] + 0.01
        );
        move_objects(&mut doc, 0, &[a.target], [13.0, -17.0]).unwrap();
        let after_lines = text::text_lines(&doc, 0).unwrap();
        for (old, new) in before_lines.iter().zip(&after_lines) {
            let delta = if old.text == "Added label" { [13.0, -17.0] } else { [0.0, 0.0] };
            assert_eq!(old.text, new.text);
            assert!(close(new.rect, [old.rect[0] + delta[0], old.rect[1] + delta[1], old.rect[2] + delta[0], old.rect[3] + delta[1]]));
        }
        let current = added::list_added(&doc)[0].clone();
        let bytes = stream_bytes(&doc, &Object::Ref(current.obj)).unwrap();
        assert_eq!(content_after_cm(&bytes), content_after_cm(&source), "all drawing bytes after the placement, including glyphs, remain identical");
        assert_eq!(pdfcraft_model::pages(&doc)[0].dict.get(b"Resources").cloned(), resources);
        doc = reopen(&doc);
    }
}

#[test]
fn group_moves_of_shared_added_streams_change_only_the_selected_page() {
    let mut doc = fixture();
    added::add_content(
        &mut doc,
        0,
        &Content::Text(added::AddedText { text: "Shared added label".into(), rect: [40.0, 400.0, 240.0, 420.0], ..Default::default() }),
    )
    .unwrap();
    let pages = pdfcraft_model::pages(&doc);
    let contents = pages[0].dict.get(b"Contents").cloned().unwrap();
    doc.update_dict(pages[1].obj, |d| d.set(b"Contents".to_vec(), contents)).unwrap();
    let doc = reopen(&doc);
    let mut doc = doc;
    let other = page_content_bytes(&doc, 1);
    let before = editable_objects(&doc, 0).unwrap();
    let target = before.iter().find(|o| o.target.kind == ObjectKind::Added).unwrap().target;
    move_objects(&mut doc, 0, &[target], [15.0, -20.0]).unwrap();
    assert_eq!(page_content_bytes(&doc, 1), other, "a shared stream is copied on write");
    assert_ne!(added::list_added(&doc).iter().find(|a| a.page == 0).unwrap().obj, added::list_added(&doc).iter().find(|a| a.page == 1).unwrap().obj);
}

#[test]
fn group_moves_handle_added_content_under_inherited_graphics_state() {
    let mut doc = text_page("2 0 0 2 10 20 cm BT /F1 12 Tf 40 250 Td (Native) Tj ET");
    added::add_content(
        &mut doc,
        0,
        &Content::Text(added::AddedText { text: "Added label".into(), rect: [50.0, 100.0, 200.0, 130.0], ..Default::default() }),
    )
    .unwrap();
    let before = text::text_lines(&doc, 0).unwrap();
    let target = editable_objects(&doc, 0).unwrap().iter().find(|o| o.target.kind == ObjectKind::Added).unwrap().target;
    move_objects(&mut doc, 0, &[target], [30.0, -20.0]).unwrap();
    let after = text::text_lines(&doc, 0).unwrap();
    for (old, new) in before.iter().zip(&after) {
        let delta = if old.text == "Added label" { [30.0, -20.0] } else { [0.0, 0.0] };
        assert!(close(new.rect, [old.rect[0] + delta[0], old.rect[1] + delta[1], old.rect[2] + delta[0], old.rect[3] + delta[1]]));
    }
}

#[test]
fn group_selection_refuses_incomplete_content_and_resource_budget_overruns() {
    for data in [
        format!("{} BT /F1 12 Tf 40 400 Td (Target) Tj ET", "q ".repeat(1025)),
        format!("{} BT /F1 12 Tf 40 400 Td (Target) Tj ET", "n ".repeat(100001)),
    ] {
        let mut doc = text_page(&data);
        let before = pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap();
        assert!(editable_objects(&doc, 0).is_err());
        assert!(move_objects(&mut doc, 0, &[ObjectTarget { kind: ObjectKind::Text, index: 0 }], [10.0, 10.0]).is_err());
        assert_eq!(pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap(), before);
    }
    let mut doc = text_page("BT /F1 12 Tf 40 400 Td (Target) Tj ET");
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Ref(pdfcraft_cos::ObjRef::new(999999, 0)))).unwrap();
    assert!(editable_objects(&doc, 0).is_err(), "a missing stream cannot produce a partial inventory");
    let mut filter = Dict::new();
    filter.set(b"Filter".to_vec(), Object::name("NotAFilter"));
    let broken = doc.add(Object::Stream(Stream::from_raw(filter, vec![1, 2, 3])));
    doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Ref(broken))).unwrap();
    assert!(editable_objects(&doc, 0).is_err(), "an undecodable stream is refused");
}

#[test]
fn group_move_budget_refusals_keep_the_original_page_selectable() {
    let text = text_page(&format!("{} BT /F1 12 Tf 40 400 Td (Target) Tj ET", "n ".repeat(99_995)));
    assert_eq!(pdfcraft_content::parse(&page_content_bytes(&text, 0)).ops.len(), 100_000);
    let mut image = image_page();
    let page = pdfcraft_model::pages(&image)[0].obj;
    let bytes = format!("{} /Im0 Do {}", "q ".repeat(1024), "Q ".repeat(1024));
    let content = image.add(Object::Stream(Stream::flate(Dict::new(), bytes.as_bytes())));
    image.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Ref(content))).unwrap();
    for mut doc in [text, image] {
        let objects = editable_objects(&doc, 0).unwrap();
        assert_eq!(objects.len(), 1);
        let before = pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap();
        let error = move_objects(&mut doc, 0, &[objects[0].target], [10.0, 5.0]).unwrap_err();
        assert!(error.to_string().contains("too many content operations") || error.to_string().contains("nested too deeply"), "{error}");
        assert_eq!(pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap(), before);
        assert_eq!(editable_objects(&doc, 0).unwrap(), objects, "refusal retains a usable inventory");
    }
}

#[test]
fn arabic_actual_text_keeps_typing_order_when_opened_for_editing() {
    for logical in ["مرحبا بالعالم", "سَلَام 123 (PDF)", "English مرحبا ١٢٣", "PDFمرحبا", "مرحبا", "שלום עולם", "Latin text"]
    {
        let hex: String = logical.encode_utf16().map(|u| format!("{u:04X}")).collect();
        for drawing in ["(abcd) Tj (efgh) Tj", "(X) Tj"] {
            let doc = text_page(&format!("/Span <</ActualText <FEFF{hex}>>> BDC BT /F1 12 Tf 72 700 Td {drawing} ET EMC"));
            for doc in [&doc, &reopen(&doc)] {
                let lines = text::text_lines(doc, 0).unwrap();
                assert_eq!(lines.len(), 1);
                assert_eq!(lines[0].text, logical, "ActualText is already in typing order");
                assert_eq!(text::text_blocks(doc, 0).unwrap()[0].editor_text, logical);
            }
        }
    }
}

#[test]
fn latin_ligature_actual_text_stays_on_its_line() {
    // Word and Chrome wrap ligatures in ActualText spans mid-word ("ﬁ" drawn, "fi" meant).
    let hex: String = "fi".encode_utf16().map(|u| format!("{u:04X}")).collect();
    let doc = text_page(&format!("BT /F1 12 Tf 72 700 Td (de) Tj /Span <</ActualText <FEFF{hex}>>> BDC (X) Tj EMC (ned well) Tj ET"));
    for doc in [&doc, &reopen(&doc)] {
        let lines = text::text_lines(doc, 0).unwrap();
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["defined well"]);
        assert_eq!(text::text_blocks(doc, 0).unwrap()[0].editor_text, "defined well");
    }
}

#[test]
fn actual_text_and_visual_rtl_glyphs_keep_their_own_reading_order() {
    let hex: String = "שלום".encode_utf16().map(|u| format!("{u:04X}")).collect();
    let doc = hebrew_page(&format!("BT /F1 12 Tf 72 700 Td (EDCBA) Tj /Span <</ActualText <FEFF{hex}>>> BDC (FGHD) Tj EMC (EDCBA) Tj ET"));
    for doc in [&doc, &reopen(&doc)] {
        let lines = text::text_lines(doc, 0).unwrap();
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["בדיוק", "שלום", "בדיוק"]);
    }
}

/// Synthetic codes A س, B م and X a lam-alef glyph, drawn from left to right.
fn arabic_ligature_page(content: &str, composite: bool) -> Document {
    let prefix = if composite { "00" } else { "" };
    let cmap = format!(
        "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapType 2 def \
         1 begincodespacerange <{prefix}00> <{prefix}FF> endcodespacerange 4 beginbfchar \
         <{prefix}20> <0020> <{prefix}41> <0633> <{prefix}42> <0645> <{prefix}58> <FEFB> \
         endbfchar endcmap CMapName currentdict /CMap defineresource pop end end"
    );
    let widths: Vec<_> = (32..=88).map(|c| if c == 32 { "250" } else { "500" }).collect();
    let font = if composite {
        "<< /Type /Font /Subtype /Type0 /BaseFont /FakeArabic /Encoding /Identity-H /DescendantFonts [7 0 R] /ToUnicode 6 0 R >>".into()
    } else {
        format!("<< /Type /Font /Subtype /Type1 /BaseFont /FakeArabic /FirstChar 32 /LastChar 88 /Widths [{}] /ToUnicode 6 0 R >>", widths.join(" "))
    };
    build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        &stream("", content),
        &font,
        &stream("", &cmap),
        "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /FakeArabic /CIDSystemInfo << /Registry (Synthetic) /Ordering (Identity) /Supplement 0 >> /DW 500 /W [32 [250]] >>",
    ])
}

#[test]
fn arabic_ligature_actual_text_stays_on_its_line() {
    for (ligature, composite) in [("لا", false), ("لأ", false), ("لإ", false), ("لآ", false), ("لَا", false), ("لا", true)] {
        let hex: String = ligature.encode_utf16().map(|u| format!("{u:04X}")).collect();
        let (left, glyph, right) = if composite { ("<0042>", "<0058>", "<0041>") } else { ("(B)", "(X)", "(A)") };
        let doc = arabic_ligature_page(
            &format!("BT /F1 12 Tf 72 700 Td {left} Tj /Span <</ActualText <FEFF{hex}>>> BDC [{glyph}] TJ EMC {right} Tj ET"),
            composite,
        );
        let expected = format!("س{ligature}م");
        for doc in [&doc, &reopen(&doc)] {
            let lines = text::text_lines(doc, 0).unwrap();
            assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), [expected.as_str()]);
            assert_eq!(text::text_blocks(doc, 0).unwrap()[0].editor_text, expected);
        }
    }
}

#[test]
fn multi_glyph_rtl_actual_text_keeps_its_own_reading_order() {
    // Two encoded glyphs are a logical span, even if their replacement looks like a ligature.
    for drawing in ["(XX) Tj", "(X) Tj (X) Tj", "[(X) (X)] TJ"] {
        let doc =
            arabic_ligature_page(&format!("BT /F1 12 Tf 72 700 Td (B) Tj /Span <</ActualText <FEFF06440627>>> BDC {drawing} EMC (A) Tj ET"), false);
        let lines = text::text_lines(&doc, 0).unwrap();
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["م", "لا", "س"]);
    }
}

#[test]
fn replacing_word_with_actual_text_ligature_removes_all_its_source_operators() {
    let mut doc = arabic_ligature_page(
        "BT /F1 12 Tf 72 700 Td (B) Tj /Span <</ActualText <FEFF06440627>>> BDC /Span BMC (X) Tj EMC EMC (A) Tj ET \
         BT /F1 12 Tf 72 600 Td (AB) Tj ET",
        false,
    );
    assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].editor_text, "سلام");
    text::replace_block(&mut doc, 0, 0, "Replacement").unwrap();
    let doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Replacement", "مس"]);
    let bytes = page_content_bytes(&doc, 0);
    let content = String::from_utf8_lossy(&bytes);
    assert!(!content.contains("ActualText") && !content.contains("(X)") && !content.contains("(B)") && !content.contains("(A)"), "{content}");
}

#[test]
fn replacing_arabic_actual_text_removes_the_whole_original_drawing() {
    let logical = "مرحبا بالعالم";
    let hex: String = logical.encode_utf16().map(|u| format!("{u:04X}")).collect();
    let mut doc = text_page(&format!(
        "/Span <</ActualText <FEFF{hex}>>> BDC BT /F1 12 Tf 72 700 Td (abcd) Tj (efgh) Tj ET EMC \
         BT /F1 12 Tf 72 600 Td (Keep this line) Tj ET"
    ));
    assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].editor_text, logical);
    text::replace_block(&mut doc, 0, 0, "Replacement").unwrap();
    let doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Replacement", "Keep this line"]);
    let bytes = page_content_bytes(&doc, 0);
    let content = String::from_utf8_lossy(&bytes);
    assert!(!content.contains("ActualText") && !content.contains("(abcd)") && !content.contains("(efgh)"));
}

fn mixed_region_page() -> Document {
    let mut doc = text_page(
        "BT /F2 1 Tf 18 0 0 18 123 700 Tm (TITLE) Tj ET \
         BT /F2 1 Tf 12 0 0 12 117.6 676 Tm (two lines) Tj ET \
         BT /F2 1 Tf 12 0 0 12 135.6 652 Tm (last) Tj ET \
         BT 0 0.2 0.5 rg /F2 10 Tf 72 600 Td (About) Tj ET \
         BT 0 g /F1 10 Tf 72 582 Td (Plain ) Tj /F2 10 Tf 0 0.2 0.5 rg (bold) Tj /F1 10 Tf 0 g ( text.) Tj ET \
         BT 0.2 0.4 0.7 rg /F3 8 Tf 72 566 Td (Italic footer) Tj ET \
         BT 0 g /F1 10 Tf 400 600 Td (Other column) Tj ET",
    );
    for (obj, base) in [(5, "Courier"), (6, "Courier-Bold")] {
        let mut f = Dict::new();
        f.set(b"Type".to_vec(), Object::name("Font"));
        f.set(b"Subtype".to_vec(), Object::name("Type1"));
        f.set(b"BaseFont".to_vec(), Object::name(base));
        f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
        doc.set(pdfcraft_cos::ObjRef::new(obj, 0), Object::Dict(f));
    }
    let mut f = Dict::new();
    f.set(b"Type".to_vec(), Object::name("Font"));
    f.set(b"Subtype".to_vec(), Object::name("Type1"));
    f.set(b"BaseFont".to_vec(), Object::name("Courier-Oblique"));
    f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    let font = doc.add(Object::Dict(f));
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let mut resources = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().unwrap().clone();
    let mut fonts = doc.resolve(resources.get(b"Font").unwrap()).as_dict().unwrap().clone();
    fonts.set(b"F3".to_vec(), Object::Ref(font));
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(page.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(resources))).unwrap();
    doc
}

#[test]
fn mixed_styles_and_matrix_sizes_form_visual_regions_without_merging_columns() {
    let doc = mixed_region_page();
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[0].editor_text, "TITLE\ntwo lines\nlast");
    assert_eq!(blocks[0].rows.len(), 3);
    assert_eq!(blocks[1].editor_text, "About\nPlain bold text.\nItalic footer");
    assert_eq!(blocks[1].rows.len(), 3);
    assert_eq!(blocks[1].lines.len(), 5);
    assert_eq!(blocks[2].text, "Other column");
}

#[test]
fn a_single_row_with_mixed_styles_grows_before_wrapping() {
    let mut doc = text_page("BT /F1 10 Tf 72 582 Td (Plain ) Tj /F2 10 Tf (bold) Tj /F1 10 Tf ( text.) Tj ET");
    doc.update_dict(pdfcraft_cos::ObjRef::new(6, 0), |f| {
        *f = Dict::new();
        f.set(b"Type".to_vec(), Object::name("Font"));
        f.set(b"Subtype".to_vec(), Object::name("Type1"));
        f.set(b"BaseFont".to_vec(), Object::name("Helvetica-Bold"));
    })
    .unwrap();
    let blocks = text::text_blocks(&doc, 0).unwrap();
    let index = blocks.iter().position(|b| b.editor_text == "Plain bold text.").unwrap();
    let source = &blocks[index];
    assert_eq!(source.rows.len(), 1);
    assert!(source.lines.len() > 1);
    let draft = "Plain bold text. More words on the same line";
    text::replace_block(&mut doc, 0, index, draft).unwrap();
    let edited = text::text_blocks(&reopen(&doc), 0).unwrap();
    let block = edited.iter().find(|b| b.text == draft).unwrap();
    assert_eq!(block.rows.len(), 1, "a style boundary must not pin a single row to its original width");
    assert!((block.rect[0] - source.rect[0]).abs() < 0.1);
    assert!(block.rect[2] > source.rect[2] + 20.0);
}

#[test]
fn explicit_editor_rows_survive_single_run_edit_and_reopen() {
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Original heading) Tj ET");
    let text = "First row\nSecond row\nLast row";
    let layout = text::text_edit_layout(&doc, 0, 0, text, &text::BlockStyle::default()).unwrap();
    text::replace_block(&mut doc, 0, 0, text).unwrap();
    let blocks = text::text_blocks(&reopen(&doc), 0).unwrap();
    assert_eq!(blocks[0].editor_text, text);
    assert_eq!(blocks[0].rows.len(), 3);
    assert_eq!(Some(blocks[0].rect), layout.draft_rect);
}

#[test]
fn rich_region_edits_preserve_styles_positions_and_delete_all_runs_after_reopen() {
    let original = mixed_region_page();
    let before = text::text_lines(&original, 0).unwrap();
    let block = text::text_blocks(&original, 0).unwrap()[1].clone();
    let mut doc = original.clone();
    text::replace_block(&mut doc, 0, 1, &block.editor_text.replace("Plain", "Other")).unwrap();
    let reopened = reopen(&doc);
    let blocks = text::text_blocks(&reopened, 0).unwrap();
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[1].editor_text, "About\nOther bold text.\nItalic footer");
    let lines = text::text_lines(&reopened, 0).unwrap();
    for word in ["About", "bold", "Italic footer"] {
        let source = before.iter().find(|l| l.text == word).unwrap();
        let after = lines.iter().find(|l| l.text == word).unwrap();
        assert_eq!(
            (after.base_font.as_str(), after.size, after.color, after.bold, after.italic),
            (source.base_font.as_str(), source.size, source.color, source.bold, source.italic)
        );
        assert!((after.origin_baseline() - source.origin_baseline()).abs() < 0.001);
    }
    for i in [0, 2] {
        assert_eq!(blocks[i].editor_text, text::text_blocks(&original, 0).unwrap()[i].editor_text);
        assert_eq!(blocks[i].rect, text::text_blocks(&original, 0).unwrap()[i].rect);
    }
    let mut removed = original.clone();
    text::replace_block(&mut removed, 0, 1, "").unwrap();
    let after = text::text_blocks(&reopen(&removed), 0).unwrap();
    assert_eq!(after.len(), 2);
    assert_eq!(after[0].editor_text, "TITLE\ntwo lines\nlast");
    assert_eq!(after[1].text, "Other column");
    let mut preview = original.clone();
    text::clear_block(&mut preview, 0, 1).unwrap();
    assert_eq!(text::text_blocks(&reopen(&preview), 0).unwrap().len(), 2);
    assert_eq!(text::text_blocks(&original, 0).unwrap().len(), 3);
}

/// Two edits applied together (heading and footer edited before leaving the editor, #2198): the
/// runs between them are unchanged and keep their own styles instead of taking the heading's.
#[test]
fn unchanged_runs_between_two_edits_keep_their_styles() {
    let original = mixed_region_page();
    let look =
        |doc: &Document| text::text_lines(doc, 0).unwrap().into_iter().map(|l| (l.text.trim().to_string(), l.base_font, l.color)).collect::<Vec<_>>();
    let before = look(&original);
    let block = text::text_blocks(&original, 0).unwrap()[1].clone();
    let mut doc = original.clone();
    let draft = block.editor_text.replace("About", "About us").replace("footer", "footnote");
    text::replace_block(&mut doc, 0, 1, &draft).unwrap();
    let after = look(&reopen(&doc));
    let style = |lines: &[(String, String, [f64; 3])], word: &str| lines.iter().find(|l| l.0 == word).map(|l| (l.1.clone(), l.2));
    for word in ["Plain", "bold", "text."] {
        assert_eq!(style(&after, word), style(&before, word), "{word:?} keeps its own style; lines after the edit: {after:?}");
    }
    assert_eq!(style(&after, "About us"), style(&before, "About"));
    assert_eq!(style(&after, "Italic footnote"), style(&before, "Italic footer"));
}

#[test]
fn disjoint_style_ranges_preserve_unselected_runs_and_validate_utf8_atomically() {
    let mut doc = mixed_region_page();
    let block = text::text_blocks(&doc, 0).unwrap()[1].clone();
    let a = block.editor_text.find("Plain").unwrap();
    let b = block.editor_text.find("bold").unwrap();
    let style = BlockStyle {
        ranges: vec![
            RangeStyle {
                range: a..a + 5,
                family: None,
                resource: None,
                size: Some(15.0),
                color: Some([0.8, 0.1, 0.2]),
                underline: Some(true),
                char_spacing: None,
                scale: None,
            },
            RangeStyle {
                range: b..b + 4,
                family: Some((Family::Courier, false, true)),
                resource: None,
                size: None,
                color: None,
                underline: None,
                char_spacing: Some(0.2),
                scale: Some(90.0),
            },
        ],
        width: Some(300.0),
        ..Default::default()
    };
    text::rewrite_block(&mut doc, 0, 1, Some(&block.editor_text), &style).unwrap();
    doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    let plain = lines.iter().find(|l| l.text == "Plain").unwrap();
    assert_eq!((plain.size, plain.color), (15.0, [0.8, 0.1, 0.2]));
    let bold = lines.iter().find(|l| l.text == "bold").unwrap();
    assert_eq!(bold.base_font, "Courier-Oblique");
    let original = mixed_region_page();
    for word in ["About", "Italic footer"] {
        let prior = text::text_lines(&original, 0).unwrap().into_iter().find(|l| l.text == word).unwrap();
        let after = lines.iter().find(|l| l.text == word).unwrap();
        assert_eq!((after.size, after.color, &after.font, after.baseline()), (prior.size, prior.color, &prior.font, prior.baseline()));
    }
    let before = pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap();
    let invalid = BlockStyle {
        ranges: vec![RangeStyle {
            range: 1..2,
            family: None,
            resource: None,
            size: Some(12.0),
            color: None,
            underline: None,
            char_spacing: None,
            scale: None,
        }],
        ..Default::default()
    };
    assert!(text::rewrite_block(&mut doc, 0, 1, Some("é bad"), &invalid).is_err());
    assert_eq!(pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap(), before);
}

#[test]
fn centered_mixed_size_title_keeps_three_rows_when_changed() {
    let mut doc = mixed_region_page();
    let title = text::text_blocks(&doc, 0).unwrap()[0].clone();
    text::replace_block(&mut doc, 0, 0, &title.editor_text.replace("last", "tail")).unwrap();
    let reopened = reopen(&doc);
    let blocks = text::text_blocks(&reopened, 0).unwrap();
    assert_eq!(blocks[0].editor_text, "TITLE\ntwo lines\ntail");
    assert_eq!(blocks[0].rows.len(), 3);
    let lines = text::text_lines(&reopened, 0).unwrap();
    for row in &blocks[0].rows {
        let line = &lines[row[0]];
        assert!(((line.rect[0] + line.rect[2]) / 2.0 - 150.0).abs() < 0.01);
    }
}

#[test]
fn image_selection_bounds_respect_clips_and_restore_graphics_state() {
    let mut doc = image_page();
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let content = b"q 1 0 0 1 10 20 cm 20 30 100 80 re W* n q 1200 0 0 1400 -500 -600 cm /Im0 Do Q Q q 200 0 0 100 100 100 cm /Im0 Do Q q 0 0 10 10 re W n 100 0 0 100 200 200 cm /Im0 Do Q";
    let stream = doc.add(Object::Stream(Stream::flate(Dict::new(), content)));
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Ref(stream))).unwrap();
    let before = images::page_images(&doc, 0).unwrap();
    assert_eq!(before.len(), 3, "fully clipped placements retain source indexes");
    assert_eq!(before[0].visible_rect, Some([30.0, 50.0, 130.0, 130.0]));
    assert_eq!(before[1].visible_rect, Some(before[1].rect), "Q restores the prior clip");
    assert_eq!(before[2].visible_rect, None);
    let objects = crate::editable_objects(&doc, 0).unwrap();
    assert_eq!(objects.len(), 2, "hidden images cannot intercept selection");
    assert_eq!(objects[0].rect, before[0].visible_rect.unwrap());
    let offset = [12.0, 14.0];
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(pdfcraft_content::Matrix::translate(offset[0], offset[1]).0)).unwrap();
    let reopened = reopen(&doc);
    let after = images::page_images(&reopened, 0).unwrap();
    assert_eq!(after[0].visible_rect, Some([42.0, 64.0, 142.0, 144.0]), "an exclusive crop moves with the image even across nested graphics states");
    for (a, b) in after[0].rect.iter().zip(before[0].rect.iter()).enumerate() {
        assert!((b.0 - b.1 - offset[a % 2]).abs() < 0.001);
    }
    assert_eq!(after[1].rect, before[1].rect, "neighbor placement unchanged");
    assert_eq!(after[1].visible_rect, before[1].visible_rect);
    assert_eq!(after[1].matrix, before[1].matrix);
    assert_eq!(after[1].object, before[1].object);
}

#[test]
fn image_clip_path_and_graphics_state_continue_across_content_streams() {
    let mut doc = image_page();
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let pieces: [&[u8]; 3] = [b"q 20 30 40 50 re W", b"n 1000 0 0 1000 -300 -400 cm /Im0 Do", b"Q q 50 0 0 50 100 200 cm /Im0 Do Q"];
    let refs: Vec<_> = pieces.into_iter().map(|b| Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), b))))).collect();
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(refs))).unwrap();
    let images = images::page_images(&doc, 0).unwrap();
    assert_eq!(images[0].visible_rect, Some([20.0, 30.0, 60.0, 80.0]));
    assert_eq!(images[1].visible_rect, Some([100.0, 200.0, 150.0, 250.0]));
    images::change_image(
        &mut doc,
        0,
        0,
        &images::ImageChange::Transform(images::rect_to_rect(images[0].visible_rect.unwrap(), [20.0, 30.0, 80.0, 105.0])),
    )
    .unwrap();
    let after = images::page_images(&reopen(&doc), 0).unwrap();
    assert!(close(after[0].visible_rect.unwrap(), [20.0, 30.0, 80.0, 105.0]), "an exclusive crop scales with its image across streams");
    assert_eq!(after[1].matrix, images[1].matrix, "graphics state restored across stream boundaries");
}

#[test]
fn cropped_image_resizing_moves_private_clips_and_keeps_shared_ones() {
    let mut doc = image_page();
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let stream = doc.add(Object::Stream(Stream::flate(
        Dict::new(),
        b"q 30 170 130 80 re W* n 800 0 0 900 -300 -400 cm /Im0 Do Q BT /F1 12 Tf 30 130 Td (Caption) Tj ET",
    )));
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Ref(stream))).unwrap();
    for target in [[30.0, 170.0, 190.0, 270.0], [30.0, 170.0, 110.0, 220.0], [70.0, 210.0, 150.0, 260.0]] {
        let before = images::page_images(&doc, 0).unwrap()[0].clone();
        let transform = images::rect_to_rect(before.visible_rect.unwrap(), target);
        images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(transform)).unwrap();
        doc = reopen(&doc);
        let after = images::page_images(&doc, 0).unwrap()[0].clone();
        // The moved clip's `cm` is written with the content stream's number precision.
        let near = |a: [f64; 4], b: [f64; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.05);
        assert!(near(after.visible_rect.unwrap(), target), "{:?} vs {target:?}", after.visible_rect);
        assert!(near(after.rect, pdfcraft_content::Matrix(transform).bbox(before.rect)), "full pixels use the same crop transform");
        assert_eq!(after.object, before.object, "pixels are never resampled or replaced");
        assert_eq!(text::text_lines(&doc, 0).unwrap()[0].baseline(), [30.0, 130.0]);
    }
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let stream = doc.add(Object::Stream(Stream::flate(
        Dict::new(),
        b"q 30 170 130 80 re W n q 800 0 0 900 -300 -400 cm /Im0 Do Q BT /F1 12 Tf 40 200 Td (Shared caption) Tj ET Q",
    )));
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Ref(stream))).unwrap();
    let before = images::page_images(&doc, 0).unwrap();
    let caption = text::text_lines(&doc, 0).unwrap();
    let transform = [1.2, 0.0, 0.0, 1.2, 0.0, 0.0];
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(transform)).unwrap();
    doc = reopen(&doc);
    let after = images::page_images(&doc, 0).unwrap();
    assert!(close(after[0].visible_rect.unwrap(), before[0].visible_rect.unwrap()), "a shared clip stays where it is");
    assert_eq!(after[0].object, before[0].object);
    assert_eq!(
        text::text_lines(&doc, 0).unwrap().iter().map(|l| (&l.text, l.rect, l.baseline())).collect::<Vec<_>>(),
        caption.iter().map(|l| (&l.text, l.rect, l.baseline())).collect::<Vec<_>>()
    );
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform([0.5, 0.0, 0.0, 0.5, 0.0, 0.0])).unwrap();
    assert_eq!(text::text_lines(&doc, 0).unwrap().len(), 1);
}

#[test]
fn private_image_crop_with_intervening_state_resizes_across_streams() {
    let variants: Vec<Vec<&[u8]>> = vec![
        vec![b"q 1 0 0 1 10 20 cm 20 150 130 80 re W* n 0.5 g q 800 0 0 900 -300 -400 cm /Im0 Do Q Q BT /F1 12 Tf 30 130 Td (Caption) Tj ET"],
        vec![b"q 1 0 0 1 10 20 cm 20 150 130 80 re W*", b"n 0.5 g q 800 0 0 900 -300 -400 cm /Im0 Do", b"Q Q BT /F1 12 Tf 30 130 Td (Caption) Tj ET"],
    ];
    for pieces in variants {
        let mut doc = image_page();
        let page = pdfcraft_model::pages(&doc)[0].obj;
        let refs = pieces.iter().map(|b| Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), b))))).collect();
        doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Array(refs))).unwrap();
        let caption = text::text_lines(&doc, 0).unwrap();
        for target in [[30.0, 170.0, 190.0, 270.0], [30.0, 170.0, 110.0, 220.0], [70.0, 210.0, 150.0, 260.0]] {
            let before = images::page_images(&doc, 0).unwrap()[0].clone();
            let transform = images::rect_to_rect(before.visible_rect.unwrap(), target);
            images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(transform)).unwrap();
            doc = reopen(&doc);
            let after = images::page_images(&doc, 0).unwrap()[0].clone();
            assert!(close(after.visible_rect.unwrap(), target), "the private crop follows the image without a literal operator pattern");
            assert!(close(after.rect, pdfcraft_content::Matrix(transform).bbox(before.rect)));
            assert_eq!(after.object, before.object);
            assert_eq!(
                text::text_lines(&doc, 0).unwrap().iter().map(|l| (&l.text, l.rect, l.baseline())).collect::<Vec<_>>(),
                caption.iter().map(|l| (&l.text, l.rect, l.baseline())).collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn curved_image_clips_have_analytic_bounds_and_holes_do_not_capture_points() {
    let mut doc = image_page();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let bytes =
        b"q 0 0 m 0 200 100 200 100 0 c h W n 300 0 0 300 0 0 cm /Im0 Do Q q 100 100 100 100 re 125 125 50 50 re W* n 400 0 0 400 0 0 cm /Im0 Do Q";
    let content = doc.add(Object::Stream(Stream::flate(Dict::new(), bytes)));
    doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Ref(content))).unwrap();
    let images = images::page_images(&doc, 0).unwrap();
    assert!(close(images[0].visible_rect.unwrap(), [0.0, 0.0, 100.0, 150.0]), "cubic extrema, not controls");
    assert!(images[0].contains([50.0, 100.0]));
    assert!(!images[0].contains([2.0, 140.0]), "empty corners of curve bounds are not artwork");
    assert!(images[1].contains([110.0, 110.0]));
    assert!(!images[1].contains([150.0, 150.0]), "even-odd hole");
    images::change_image(&mut doc, 0, 1, &images::ImageChange::Transform([1.2, 0.0, 0.0, 1.2, 10.0, 15.0])).unwrap();
    let after = images::page_images(&reopen(&doc), 0).unwrap();
    assert!(!after[1].contains([190.0, 195.0]), "the transformed even-odd hole remains empty");
    assert!(after[1].contains([142.0, 147.0]), "the transformed filled corner remains selectable");
    assert_eq!(after[0].visible_rect, images[0].visible_rect, "the neighbouring curved clip stays put");
}

#[test]
fn marked_paragraph_boundaries_and_cross_stream_transforms_survive_rewrite() {
    let mut doc =
        text_page("/P << /MCID 0 >> BDC BT /F1 12 Tf 40 500 Td (One) Tj ET EMC /P << /MCID 1 >> BDC BT /F1 12 Tf 40 482 Td (Two) Tj ET EMC");
    assert_eq!(text::text_blocks(&doc, 0).unwrap().len(), 2, "distinct explicit paragraphs win over nearby geometry");
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let parts: [&[u8]; 2] =
        [b"/P << /MCID 2 >> BDC BT /F1 12 Tf 40 500 Td (First row) Tj ET", b"q 1 0 0 1 10 12 cm BT /F1 12 Tf 30 470 Td (Second row) Tj ET Q EMC"];
    let refs = parts.iter().map(|p| Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), p))))).collect();
    doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Array(refs))).unwrap();
    let block = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(block.len(), 1, "one marked paragraph across streams and coordinate spaces");
    assert_eq!(block[0].editor_text, "First row\nSecond row");
    let original = text::text_lines(&doc, 0).unwrap();
    text::rewrite_block(&mut doc, 0, 0, Some("First row\nChanged row"), &text::BlockStyle { width: Some(200.0), ..Default::default() }).unwrap();
    let after = text::text_lines(&reopen(&doc), 0).unwrap();
    for (a, b) in after.iter().zip(original) {
        assert!(close(a.rect, b.rect) || a.baseline() == b.baseline());
    }
    assert_eq!(after[1].text, "Changed row");
}

#[test]
fn logical_indic_with_nuktas_not_reordered_on_standard_font() {
    // Already logical Hindi text with nuktas (बढ़िया) must not be corrupted by pre-base reordering
    // when the font does not require visual order.
    let hex_actual = "<FEFF092C0922093C093F092F093E>";
    let doc = text_page(&format!("/Span << /ActualText {hex_actual} >> BDC BT /F1 12 Tf 40 500 Td (ab) Tj ET EMC"));
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "\u{092C}\u{0922}\u{093C}\u{093F}\u{092F}\u{093E}");
    assert!(!lines[0].visual_order);
}

/// One page in a font named `base_font` whose ToUnicode maps A ब, B ि, C ढ़, D य, E ा, F क, G त.
/// A legacy visual-order font draws "बढ़िया" as `(ABCDE)` (ब ि ढ़ या) and "किताब" as `(BFGEA)`.
fn devanagari_page(base_font: &str, content: &str) -> Document {
    let cmap = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapType 2 def \
                1 begincodespacerange <00> <FF> endcodespacerange 8 beginbfchar \
                <20> <0020> <41> <092C> <42> <093F> <43> <0922093C> <44> <092F> <45> <093E> <46> <0915> <47> <0924> \
                endbfchar endcmap CMapName currentdict /CMap defineresource pop end end";
    let widths: Vec<&str> = (32..=71)
        .map(|c| {
            if c == 32 {
                "250"
            } else if c >= 65 {
                "500"
            } else {
                "0"
            }
        })
        .collect();
    let font = format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /{base_font} /FirstChar 32 /LastChar 71 /Widths [{}] /ToUnicode 6 0 R >>",
        widths.join(" ")
    );
    build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        &stream("", content),
        &font,
        &stream("", cmap),
    ])
}

/// A legacy visual-order font (Kruti Dev) whose ToUnicode gives the glyphs in drawing order: the
/// pre-base vowel sign is read back after its consonant, for editing and for reading.
#[test]
fn legacy_indic_font_flags_visual_order_and_reorders_pre_base_vowels() {
    let doc = devanagari_page("KrutiDev010", "BT /F1 12 Tf 40 500 Td (ABCDE) Tj ET");
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].visual_order);
    assert_eq!(lines[0].text, "\u{092C}\u{0922}\u{093C}\u{093F}\u{092F}\u{093E}");
    let reading: Vec<String> = text::reading_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(reading, ["\u{092C}\u{0922}\u{093C}\u{093F}\u{092F}\u{093E}"]);
}

/// Typed (logical) text is written back in the font's visual order and reads back as typed.
#[test]
fn legacy_indic_font_edits_are_written_in_visual_order() {
    let mut doc = devanagari_page("ABCDEF+KrutiDev010", "BT /F1 12 Tf 40 500 Td (ABCDE) Tj ET");
    let kitab = "\u{0915}\u{093F}\u{0924}\u{093E}\u{092C}";
    assert_eq!(text::replace_line(&mut doc, 0, 0, kitab).unwrap().substituted, None);
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert!(content.contains("(BFGEA)"), "{content}");
    assert_eq!(text::text_lines(&reopen(&doc), 0).unwrap()[0].text, kitab);
}

/// `/ActualText` is already in logical order, even in a visual-order font: "बढ़िया" must not
/// become "बढ़यिा".
#[test]
fn logical_actual_text_in_a_legacy_indic_font_is_not_reordered() {
    let doc = devanagari_page("KrutiDev010", "/Span << /ActualText <FEFF092C0922093C093F092F093E> >> BDC BT /F1 12 Tf 40 500 Td (ABCDE) Tj ET EMC");
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "\u{092C}\u{0922}\u{093C}\u{093F}\u{092F}\u{093E}");
    let reading: Vec<String> = text::reading_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(reading, ["\u{092C}\u{0922}\u{093C}\u{093F}\u{092F}\u{093E}"]);
}

/// A modern subset font with a logical ToUnicode and no GSUB in the PDF (subsets and bare CFF
/// often drop it) is not visual order: its text is read as it is.
#[test]
fn subset_indic_font_without_legacy_name_keeps_logical_text() {
    let doc = devanagari_page("ABCDEF+NotoSansDevanagari-Regular", "BT /F1 12 Tf 40 500 Td (ACBDE) Tj ET");
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 1);
    assert!(!lines[0].visual_order);
    assert_eq!(lines[0].text, "\u{092C}\u{0922}\u{093C}\u{093F}\u{092F}\u{093E}");
}

/// Lines of a paragraph drawn turned on the page (a landscape page drawn a quarter turn round)
/// are separate rows, and adding a word keeps them apart (#1443).
#[test]
fn turned_paragraph_keeps_its_line_breaks_when_a_word_is_added() {
    for cm in ["0 1 -1 0 600 0", "0 -1 1 0 0 800", "-1 0 0 -1 600 800"] {
        let original =
            text_page(&format!("q {cm} cm BT /F1 12 Tf 72 500 Td (First line here) Tj 0 -14 Td (Second line) Tj 0 -14 Td (Third) Tj ET Q"));
        let block = text::text_blocks(&original, 0).unwrap().remove(0);
        assert_eq!(block.editor_text, "First line here\nSecond line\nThird", "{cm}");
        let mut doc = original.clone();
        text::rewrite_block(&mut doc, 0, 0, Some("First line here\nSecond line\nThird added"), &text::BlockStyle::default()).unwrap();
        let reopened = reopen(&doc);
        assert_eq!(text::text_blocks(&reopened, 0).unwrap()[0].editor_text, "First line here\nSecond line\nThird added", "{cm}");
        let (before, after) = (text::text_lines(&original, 0).unwrap(), text::text_lines(&reopened, 0).unwrap());
        assert_eq!(after.len(), 3, "{cm}");
        for (a, b) in after.iter().zip(&before).take(2) {
            assert!(close(a.rect, b.rect), "{cm}: untouched rows stay put: {:?} {:?}", a.rect, b.rect);
        }
        // The last row grows along its own baseline from where it started.
        let (grown, old) = (after[2].rect, before[2].rect);
        assert!((0..2).all(|i| grown[i] <= old[i] + 0.01) && (2..4).all(|i| grown[i] >= old[i] - 0.01), "{cm}: {grown:?} {old:?}");
        assert!((after[2].origin_baseline() - before[2].origin_baseline()).abs() < 0.01, "{cm}");
    }
    // A turned single line grows rather than wrapping across the page's other axis.
    let mut doc = text_page("BT /F1 12 Tf 0 1 -1 0 300 300 Tm (Only line) Tj ET");
    text::rewrite_block(&mut doc, 0, 0, Some("Only line with several more words added"), &text::BlockStyle::default()).unwrap();
    let blocks = text::text_blocks(&reopen(&doc), 0).unwrap();
    assert_eq!(blocks[0].editor_text, "Only line with several more words added");
    // Formatting part of turned text isn't laid out yet: refused, not drawn along the page's x axis.
    let mut doc = text_page("BT /F1 12 Tf 0 1 -1 0 300 300 Tm (Only line) Tj ET");
    let range = text::RangeStyle {
        range: 0..4,
        family: None,
        resource: None,
        size: Some(20.0),
        color: None,
        underline: None,
        char_spacing: None,
        scale: None,
    };
    let style = text::BlockStyle { ranges: vec![range], ..Default::default() };
    assert!(text::rewrite_block(&mut doc, 0, 0, Some("Only line"), &style).is_err());
}

/// A paragraph in a Chrome-style subset font: `/W` leaves out the glyphs of the `/DW` width.
/// Adding a word keeps every row as it was and shows the word in the page's own font (#1443).
#[test]
fn default_width_subset_paragraph_keeps_its_rows_when_a_word_is_added() {
    let hex = |s: &str| s.chars().map(|c| format!("00{:02X}", u32::from(c))).collect::<String>();
    let rows = ["This is the first line", "the second", "and third"];
    let content = rows
        .iter()
        .enumerate()
        .map(|(i, row)| format!("BT /F3 12 Tf 1 0 0 1 72 {} Tm <{}> Tj ET", 700 - 14 * i, hex(row)))
        .collect::<Vec<_>>()
        .join(" ");
    let mut doc = text_page(&content);
    let cmap = b"/CIDInit /ProcSet findresource begin 1 begincodespacerange <0000> <FFFF> endcodespacerange \
        1 beginbfrange <0020> <007E> <0020> endbfrange end";
    let to_unicode = doc.add(Object::Stream(Stream::from_raw(Dict::new(), cmap.to_vec())));
    let mut cid = Dict::new();
    cid.set(b"Type".to_vec(), Object::name("Font"));
    cid.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
    cid.set(b"BaseFont".to_vec(), Object::name("AAAAAA+LiberationSans"));
    cid.set(b"CIDToGIDMap".to_vec(), Object::name("Identity"));
    cid.set(b"DW".to_vec(), Object::Int(500));
    cid.set(
        b"W".to_vec(),
        Object::Array(vec![Object::Int(32), Object::Array(vec![Object::Int(278)]), Object::Int(84), Object::Array(vec![Object::Int(611)])]),
    );
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type0"));
    font.set(b"BaseFont".to_vec(), Object::name("AAAAAA+LiberationSans"));
    font.set(b"Encoding".to_vec(), Object::name("Identity-H"));
    font.set(b"DescendantFonts".to_vec(), Object::Array(vec![Object::Dict(cid)]));
    font.set(b"ToUnicode".to_vec(), Object::Ref(to_unicode));
    let font = doc.add(Object::Dict(font));
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let mut resources = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().unwrap().clone();
    let mut fonts = doc.resolve(resources.get(b"Font").unwrap()).as_dict().unwrap().clone();
    fonts.set(b"F3".to_vec(), Object::Ref(font));
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(page.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(resources))).unwrap();
    let block = text::text_blocks(&doc, 0).unwrap().remove(0);
    assert_eq!(block.editor_text, rows.join("\n"));
    let before = text::text_lines(&doc, 0).unwrap();
    let edit = text::rewrite_block(&mut doc, 0, 0, Some(&format!("{} added", block.editor_text)), &text::BlockStyle::default()).unwrap();
    assert_eq!(edit.substituted, None, "the subset shows every letter of the new word");
    let reopened = reopen(&doc);
    assert_eq!(text::text_blocks(&reopened, 0).unwrap()[0].editor_text, "This is the first line\nthe second\nand third added");
    let after = text::text_lines(&reopened, 0).unwrap();
    for (a, b) in after.iter().zip(&before).take(2) {
        assert_eq!(a.text, b.text);
        assert!(close(a.rect, b.rect), "untouched rows keep their place and width: {:?} {:?}", a.rect, b.rect);
    }
}

/// An added item is written out as a specific standard-14 face (`/Times-Italic`, `/Helvetica-Bold`
/// …), so it has to be *measured* with that same face. Measuring the upright face while writing
/// the italic one mis-sizes every box: Times-Italic `A` is 611 against Times-Roman's 722.
#[test]
fn an_items_measured_face_is_the_face_it_is_written_as() {
    for family in [Family::Helvetica, Family::Times, Family::Courier] {
        for bold in [false, true] {
            for italic in [false, true] {
                let written = family.base_font(bold, italic);
                let measured = family.std14(bold, italic);
                assert_eq!(written, measured.base_font(), "{family:?} bold={bold} italic={italic}");
                // ... and the public measure reaches that face, rather than an upright stand-in.
                assert_eq!(family.width("Anna", 12.0, bold, italic), measured.text_width("Anna", 12.0), "{family:?} bold={bold} italic={italic}");
            }
        }
    }
    // The faces really do differ, so the assertions above are not all comparing the same number.
    let times = |italic| Family::Times.width("Anna", 12.0, false, italic);
    assert_ne!(times(false), times(true), "Times-Italic is not Times-Roman");
    // Courier is monospaced in every style: equal widths here are correct, not a collapsed face.
    let courier = |italic| Family::Courier.width("Anna", 12.0, false, italic);
    assert_eq!(courier(false), courier(true));
}

#[test]
fn page_marks_and_added_content_preserve_fractional_user_unit_matrices() {
    for rotation in [0, 90] {
        let mut doc = fixture();
        let page = pdfcraft_model::pages(&doc)[2].obj;
        doc.update_dict(page, |d| {
            let bounds = Object::Array([0.0, 0.0, 0.1, 0.1].into_iter().map(Object::Real).collect());
            d.set(b"MediaBox".to_vec(), bounds.clone());
            d.set(b"CropBox".to_vec(), bounds);
            d.set(b"UserUnit".to_vec(), Object::Int(4096));
            d.set(b"Rotate".to_vec(), Object::Int(rotation));
        })
        .unwrap();
        let mut hf = HeaderFooter::default();
        hf.text[0] = "Matrix header".into();
        add_header_footer(&mut doc, &[2], &hf, false, &cx()).unwrap();
        let text = AddedText { rect: [72.0, 100.0, 200.0, 144.0], text: "Matrix text".into(), size: 12.0, ..AddedText::default() };
        add_content(&mut doc, 2, &Content::Text(text)).unwrap();
        let saved = reopen(&doc);
        let contents = streams(&saved, 2);
        for marker in ["/PCMark /HeaderFooter", "(Matrix text) Tj"] {
            let matching: Vec<_> = contents.iter().filter(|s| s.contains(marker)).collect();
            assert_eq!(matching.len(), 1, "{contents:?}");
            let stream = matching[0];
            let parsed = pdfcraft_content::parse(stream.as_bytes());
            assert_eq!(parsed.skipped, 0);
            let cm = parsed.ops.iter().find(|op| op.is("cm")).unwrap();
            assert_eq!(cm.operands.len(), 6);
            assert!(!stream[cm.span.clone()].contains('e') && !stream[cm.span.clone()].contains('E'), "PDF numbers cannot use exponents");
            let matrix = cm.nums::<6>().unwrap();
            let expected = if rotation == 0 {
                [0.000244140625, 0.0, 0.0, 0.000244140625, 0.0, 0.0]
            } else {
                [0.0, 0.000244140625, -0.000244140625, 0.0, 0.1, 0.0]
            };
            assert_eq!(matrix, expected, "rotation={rotation}, stream={stream}");
            // A fixed physical display point must remain at its intended raw page position.
            let raw = [matrix[0] * 72.0 + matrix[2] * 144.0 + matrix[4], matrix[1] * 72.0 + matrix[3] * 144.0 + matrix[5]];
            let expected_raw = if rotation == 0 { [0.017578125, 0.03515625] } else { [0.06484375, 0.017578125] };
            for (actual, expected) in raw.into_iter().zip(expected_raw) {
                assert!((actual - expected).abs() < 1e-12);
            }
        }
    }
}

#[test]
fn rows_the_edit_does_not_reach_keep_their_source_operators() {
    // Word gaps in TJ, as justified text from a layout program has; the text state alone
    // can't reproduce them, so a rewritten row would lose them.
    let rows = [
        "[(justified) -400 ( text) -400 ( uses) -400 ( wide) -400 ( gaps)] TJ",
        "[(between) -400 ( every) -400 ( single) -400 ( word) -400 ( here)] TJ",
        "(last line.) Tj",
    ];
    let content = format!("BT /F1 12 Tf 14.4 TL 1 0 0 1 72 700 Tm {} T* {} T* {} ET", rows[0], rows[1], rows[2]);
    let row_texts = |doc: &Document| -> Vec<(String, [f64; 2])> {
        text::text_lines(doc, 0).unwrap().into_iter().map(|l| (l.text.clone(), l.baseline())).collect()
    };
    let source = row_texts(&text_page(&content));
    assert_eq!(source.len(), 3);
    for (row, from, to) in [(2, "last line.", "last line!"), (0, "justified", "justify"), (1, "single", "lone")] {
        let mut doc = text_page(&content);
        let block = text::text_blocks(&doc, 0).unwrap().into_iter().position(|b| b.text.contains("justified")).unwrap();
        let draft = text::text_blocks(&doc, 0).unwrap()[block].editor_text.replacen(from, to, 1);
        text::rewrite_block(&mut doc, 0, block, Some(&draft), &text::BlockStyle::default()).unwrap();
        let doc = reopen(&doc);
        let bytes = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
        let after = row_texts(&doc);
        assert_eq!(after.len(), 3, "{to}: {after:?}");
        for (i, ((text, at), (old, was))) in after.iter().zip(&source).enumerate() {
            assert!((at[0] - was[0]).abs() < 0.01 && (at[1] - was[1]).abs() < 0.01, "{to}: row {i} stays at {was:?}, not {at:?}");
            if i == row {
                assert_eq!(*text, old.replacen(from, to, 1), "{to}: the edited row");
                assert!(!bytes.contains(rows[i]), "{to}: the edited row is rewritten");
            } else {
                assert_eq!(text, old, "{to}: row {i}");
                assert_eq!(bytes.matches(rows[i]).count(), 1, "{to}: row {i} keeps its own operator, kerning included:\n{bytes}");
            }
        }
    }
    // Colouring a selection in the last row leaves the rows above it as they were.
    let mut doc = text_page(&content);
    let block = text::text_blocks(&doc, 0).unwrap().into_iter().position(|b| b.text.contains("justified")).unwrap();
    let draft = text::text_blocks(&doc, 0).unwrap()[block].editor_text.clone();
    let at = draft.find("line").unwrap();
    let range = text::RangeStyle {
        range: at..at + 4,
        family: None,
        resource: None,
        size: None,
        color: Some([1.0, 0.0, 0.0]),
        underline: None,
        char_spacing: None,
        scale: None,
    };
    text::rewrite_block(&mut doc, 0, block, Some(&draft), &text::BlockStyle { ranges: vec![range], ..Default::default() }).unwrap();
    let bytes = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert!(rows[..2].iter().all(|row| bytes.matches(row).count() == 1) && !bytes.contains(rows[2]), "{bytes}");
    assert!(bytes.contains("1 0 0 rg"), "{bytes}");
    // The coloured word is its own run now, so only the rows above compare whole.
    assert_eq!(row_texts(&reopen(&doc))[..2], source[..2]);
}

#[test]
fn kept_rows_stay_in_place_when_rows_wrap_or_move_with_quote_operators() {
    let lines_of = |doc: &Document| -> Vec<(String, [f64; 2])> {
        text::text_lines(doc, 0).unwrap().into_iter().map(|l| (l.text.clone(), l.baseline())).collect()
    };
    let edit = |content: &str, from: &str, to: &str| {
        let mut doc = text_page(content);
        let block = text::text_blocks(&doc, 0).unwrap().into_iter().position(|b| b.editor_text.contains(from)).unwrap();
        let draft = text::text_blocks(&doc, 0).unwrap()[block].editor_text.replacen(from, to, 1);
        text::rewrite_block(&mut doc, 0, block, Some(&draft), &text::BlockStyle::default()).unwrap();
        reopen(&doc)
    };
    // `'` moves to the next line as it shows: a row after a rewritten `'` row can't stay as it is.
    for (quoted, from, to) in [
        ("BT /F1 12 Tf 14 TL 72 700 Td (first row here) Tj (second row here) ' (third row here) ' ET", "second", "2nd"),
        ("BT /F1 12 Tf 14 TL 72 700 Td (first row here) Tj (second row here) ' (third row here) ' ET", "third", "3rd"),
        // Here the first row's own `'` makes the step down that the rows after it build on.
        ("BT /F1 12 Tf 14 TL 72 714 Td (first row here) ' (second row here) ' (third row here) ' ET", "first", "1st"),
    ] {
        let source = lines_of(&text_page(quoted));
        let after = lines_of(&edit(quoted, from, to));
        assert_eq!(after.len(), 3, "{to}: {after:?}");
        for ((text, at), (old, was)) in after.iter().zip(&source) {
            assert_eq!(*text, old.replacen(from, to, 1));
            assert!((at[0] - was[0]).abs() < 0.01 && (at[1] - was[1]).abs() < 0.01, "{to}: {text} stays at {was:?}, not {at:?}");
        }
    }
    // A row that grows past the paragraph's width wraps and pushes the rows below it down;
    // those are rewritten in their new places, never left behind as well.
    let rows = "BT /F1 12 Tf 14.4 TL 72 700 Td (one two three four) Tj T* (five six seven eight) Tj T* (nine ten eleven twelve) Tj ET";
    let doc = edit(rows, "two", "two and a great many more words");
    let text: String = lines_of(&doc).into_iter().map(|(t, _)| t + " ").collect();
    for word in ["one", "five", "nine", "twelve"] {
        assert_eq!(text.matches(word).count(), 1, "{word} appears once: {text}");
    }
    assert!(lines_of(&doc).len() > 3, "the edited row wrapped: {text}");
}

/// A page whose `/F1` is Inter embedded as a simple WinAnsi TrueType font, under `base_font`.
fn embedded_program_page(content: &str, base_font: &str) -> Document {
    let mut doc = text_page(content);
    let program = doc.add(Object::Stream(Stream::flate(Dict::new(), include_bytes!("../../../assets/fonts/Inter-Regular.ttf"))));
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name(base_font));
    descriptor.set(b"FontFile2".to_vec(), Object::Ref(program));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("TrueType"));
    font.set(b"BaseFont".to_vec(), Object::name(base_font));
    font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    font.set(b"FirstChar".to_vec(), Object::Int(32));
    font.set(b"LastChar".to_vec(), Object::Int(126));
    font.set(b"Widths".to_vec(), Object::Array((32..=126).map(|_| Object::Int(560)).collect()));
    font.set(b"FontDescriptor".to_vec(), Object::Dict(descriptor));
    doc.set(pdfcraft_cos::ObjRef::new(5, 0), Object::Dict(font));
    reopen(&doc)
}

/// The font programs and subtypes of the page's font resources.
fn page_font_programs(doc: &Document) -> Vec<(Vec<u8>, Option<Vec<u8>>)> {
    let page = pdfcraft_model::pages(doc).remove(0);
    let resources = page.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    let fonts = resources.get(b"Font").and_then(|f| doc.resolve(f).as_dict().cloned()).unwrap_or_default();
    fonts
        .iter()
        .filter_map(|(_, f)| doc.resolve(f).as_dict().cloned())
        .map(|f| (f.name(b"Subtype").unwrap_or_default().to_vec(), pdfcraft_fonts::pdf::embedded_sfnt(doc, &f)))
        .collect()
}

#[test]
fn a_new_character_comes_from_the_source_fonts_own_embedded_program() {
    let inter = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
    for (content, from, to) in [
        // One row (the single-style path) and a row of a two-row paragraph (the row editor's path).
        ("BT /F1 12 Tf 72 700 Td (Stable text) Tj ET", "text", "tőxt"),
        ("BT /F1 12 Tf 14 TL 72 700 Td (First row) Tj T* (Second row) Tj ET", "Second", "Sőcond"),
    ] {
        let mut doc = embedded_program_page(content, "Inter-Regular");
        let block = text::text_blocks(&doc, 0).unwrap()[0].clone();
        let draft = block.editor_text.replacen(from, to, 1);
        let edit = text::rewrite_block(&mut doc, 0, 0, Some(&draft), &text::BlockStyle::default()).unwrap();
        assert_eq!(edit.substituted, None, "{to}: no other family stands in");
        let doc = reopen(&doc);
        // The new character is its own run in the other resource, read back into the same word.
        assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].editor_text, draft, "{to}");
        let fonts = page_font_programs(&doc);
        assert!(fonts.iter().all(|(subtype, _)| subtype != b"Type3"), "{to}: no fallback glyphs");
        assert!(
            fonts.iter().any(|(subtype, program)| subtype == b"Type0" && program.as_deref() == Some(inter.as_slice())),
            "{to}: the source program draws it"
        );
    }
}

#[test]
fn a_subset_program_is_not_trusted_for_new_characters_and_the_error_names_the_character() {
    let mut doc = embedded_program_page("BT /F1 12 Tf 72 700 Td (Stable text) Tj ET", "ABCDEF+Inter-Regular");
    let before = page_content_bytes(&doc, 0);
    let result = text::rewrite_block(&mut doc, 0, 0, Some("Stable tőxt"), &text::BlockStyle::default());
    if pdfcraft_fonts::document_japanese_font().is_some() {
        eprintln!("built with craft-fonts: the fallback font draws the new character");
        assert!(result.is_ok(), "{result:?}");
        return;
    }
    let Err(EditError::Invalid(msg)) = result else { panic!("expected a clear error, got {result:?}") };
    assert!(msg.contains("ő (U+0151)") && msg.contains("Load font") && !msg.contains("Japanese"), "{msg}");
    assert_eq!(page_content_bytes(&doc, 0), before);
}

#[test]
fn draft_layout_matches_the_paragraph_an_applied_edit_draws() {
    let content = "BT /F1 12 Tf 14 TL 72 700 Td (A first paragraph of words) Tj T* (with a second row) Tj ET \
                   BT /F1 12 Tf 72 600 Td (Another paragraph below) Tj ET BT /F1 12 Tf 320 700 Td (Side note) Tj ET";
    let doc = text_page(content);
    let block = text::text_blocks(&doc, 0).unwrap().into_iter().position(|b| b.text.starts_with("A first")).unwrap();
    let source = text::text_blocks(&doc, 0).unwrap()[block].clone();
    for draft in [
        source.editor_text.replacen("words", "many more words than before", 1),
        source.editor_text.replacen(" of words", "", 1),
        format!("{}\nand a third row", source.editor_text),
    ] {
        let layout = text::text_edit_layout(&doc, 0, block, &draft, &text::BlockStyle::default()).unwrap();
        // Apply the same edit and measure every run that isn't one of the page's other runs.
        let mut applied = doc.clone();
        text::rewrite_block(&mut applied, 0, block, Some(&draft), &text::BlockStyle::default()).unwrap();
        let others: Vec<_> =
            text::text_lines(&doc, 0).unwrap().into_iter().enumerate().filter(|(i, _)| !source.lines.contains(i)).map(|(_, l)| l.rect).collect();
        let drawn = text::text_lines(&applied, 0)
            .unwrap()
            .into_iter()
            .map(|l| l.rect)
            .filter(|r| !others.contains(r))
            .reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]);
        assert_eq!(layout.draft_rect, drawn, "{draft:?}");
        assert_eq!(layout.source_rect, source.rect);
    }
}

#[test]
fn japanese_paragraphs_wrap_between_characters_without_closing_marks_at_a_row_start() {
    if pdfcraft_fonts::document_japanese_font().is_none() {
        eprintln!("skipping the Japanese wrapping checks: built without craft-fonts (set CRAFT_FONTS_DIR)");
        return;
    }
    let text = "日本語の文章は単語の間に空白がないので、どの文字の間でも行を折り返せます。ただし「句読点」や小さい「っ」は行頭に来ません。";
    // One paragraph (the single-style path), and a heading row before it (the row editor's path).
    for draft in [text.to_string(), format!("見出し\n{text}")] {
        let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Placeholder text) Tj ET");
        text::rewrite_block(&mut doc, 0, 0, Some(&draft), &text::BlockStyle { width: Some(120.0), ..Default::default() }).unwrap();
        let block = text::text_blocks(&reopen(&doc), 0).unwrap().remove(0);
        let rows: Vec<&str> = block.editor_text.split('\n').collect();
        assert!(rows.len() > 4, "{rows:?}");
        assert_eq!(rows.concat(), draft.replace('\n', ""), "no space is added between the pieces");
        assert!(block.rect[2] - block.rect[0] <= 120.5, "{:?} {rows:?}", block.rect);
        for row in &rows {
            assert!(!row.starts_with(['、', '。', '」', 'っ']) && !row.ends_with('「'), "{row:?} in {rows:?}");
        }
    }
}

/// A line break in the edited text keeps a row of its own (the editor's rows); with a paragraph
/// spacing set, that much more space goes before it. A row the page already had
/// stays where it was, shifted by the spacing added above it.
#[test]
fn paragraph_breaks_get_their_own_spacing() {
    let baselines = |doc: &Document| -> Vec<f64> { text::text_lines(&reopen(doc), 0).unwrap().iter().map(|l| l.rect[1]).collect() };
    let gap_after_first = |b: &[f64]| b[0] - b[1];
    // A one-line source has no row step of its own: 1.5 × the size stands in.
    let mut doc = text_page("BT /F1 10 Tf 72 700 Td (The quick brown fox) Tj ET");
    text::rewrite_block(&mut doc, 0, 0, Some("First one\nSecond one"), &text::BlockStyle::default()).unwrap();
    let b = baselines(&doc);
    assert_eq!(b.len(), 2, "{b:?}");
    assert!((gap_after_first(&b) - 15.0).abs() < 0.5, "the row step alone: {b:?}");
    let mut doc = text_page("BT /F1 10 Tf 72 700 Td (The quick brown fox) Tj ET");
    text::rewrite_block(&mut doc, 0, 0, Some("First one\nSecond one"), &text::BlockStyle { paragraph_spacing: Some(20.0), ..Default::default() })
        .unwrap();
    assert!((gap_after_first(&baselines(&doc)) - 35.0).abs() < 0.5, "{:?}", baselines(&doc));
    let mut doc = text_page("BT /F1 10 Tf 72 700 Td (The quick brown fox) Tj ET");
    text::rewrite_block(&mut doc, 0, 0, Some("First one\nSecond one"), &text::BlockStyle { paragraph_spacing: Some(0.0), ..Default::default() })
        .unwrap();
    assert!((gap_after_first(&baselines(&doc)) - 15.0).abs() < 0.5);
    // Without a break, nothing changes: one paragraph, the ordinary line step.
    let mut doc = text_page("BT /F1 10 Tf 72 700 Td (The quick brown fox) Tj ET");
    text::rewrite_block(
        &mut doc,
        0,
        0,
        Some("First one Second one and then some more words to wrap"),
        &text::BlockStyle { width: Some(90.0), ..Default::default() },
    )
    .unwrap();
    let b = baselines(&doc);
    assert!(b.len() >= 2 && (gap_after_first(&b) - 12.0).abs() < 0.5, "{b:?}");
}
