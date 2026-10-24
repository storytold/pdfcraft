use std::sync::Arc;

use pdfcraft_annot::{Markup, Meta, NewAnnotation, Shape, Style, add_annotation, add_reply, add_text_replacement, summaries};
use pdfcraft_cos::{SaveOptions, write_full, write_incremental};
use pdfcraft_forms::{NewField, add_field, fields, set_value};

use super::*;

/// Two blank pages with three fields (text, check box, list).
fn blank() -> Document {
    let mut doc = Document::new_empty();
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let mut kids = Vec::new();
    for _ in 0..2 {
        let mut p = Dict::new();
        p.set(b"Type".to_vec(), Object::name("Page"));
        p.set(b"Parent".to_vec(), Object::Ref(pages));
        p.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 600.into(), 800.into()]));
        kids.push(Object::Ref(doc.add(p)));
    }
    doc.update_dict(pages, |d| {
        d.set(b"Count".to_vec(), Object::Int(kids.len() as i64));
        d.set(b"Kids".to_vec(), Object::Array(kids));
    })
    .unwrap();
    add_field(&mut doc, 0, [50.0, 700.0, 250.0, 720.0], &NewField::Text { multiline: false }, Some("Name")).unwrap();
    add_field(&mut doc, 0, [50.0, 650.0, 64.0, 664.0], &NewField::CheckBox, Some("Agree")).unwrap();
    add_field(
        &mut doc,
        1,
        [50.0, 600.0, 250.0, 660.0],
        &NewField::List { options: vec!["A".into(), "B".into(), "C".into()], multi: true },
        Some("Pick"),
    )
    .unwrap();
    Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap()
}

/// A deeply nested field tree is refused with an error. The walk used to recurse once per level
/// with nothing bounding it, so a small file overflowed the stack - and a stack overflow aborts
/// the process, which no `catch_unwind` can intercept.
#[test]
fn deeply_nested_fields_are_refused_instead_of_overflowing_the_stack() {
    let mut xml = String::from(r#"<?xml version="1.0"?><xfdf xmlns="http://ns.adobe.com/xfdf/"><fields>"#);
    for i in 0..10_000 {
        xml.push_str(&format!(r#"<field name="f{i}">"#));
    }
    xml.push_str("<value>v</value>");
    for _ in 0..10_000 {
        xml.push_str("</field>");
    }
    xml.push_str("</fields></xfdf>");
    assert!(matches!(data_values(xml.as_bytes()), Err(DataError::Malformed(_))), "ten thousand nested fields must be refused, not walked");
}

/// Nesting inside the limit still reads, and the dotted field name is built as before.
#[test]
fn nested_fields_within_the_limit_still_read() {
    let mut xml = String::from(r#"<?xml version="1.0"?><xfdf xmlns="http://ns.adobe.com/xfdf/"><fields>"#);
    for i in 0..16 {
        xml.push_str(&format!(r#"<field name="f{i}">"#));
    }
    xml.push_str("<value>deep</value>");
    for _ in 0..16 {
        xml.push_str("</field>");
    }
    xml.push_str("</fields></xfdf>");
    let Ok(values) = data_values(xml.as_bytes()) else { panic!("sixteen levels must still read") };
    assert_eq!(values.len(), 1);
    assert_eq!(values[0].0, "f0.f1.f2.f3.f4.f5.f6.f7.f8.f9.f10.f11.f12.f13.f14.f15");
    assert_eq!(values[0].1, vec!["deep".to_string()]);
}

/// The scanner counts a tag that closes itself as one level while it is read, not as a level left
/// open, so a flat file with thousands of them stays shallow and is not refused.
#[test]
fn self_closing_elements_do_not_accumulate_in_the_depth() {
    use crate::xml_nesting_depth as d;
    assert_eq!(d("<a/>"), 1);
    assert_eq!(d("<a><b/></a>"), 2);
    assert_eq!(d(&"<empty/>".repeat(2_000)), 1);
    assert_eq!(d(&format!("<fields>{}</fields>", r#"<field name="a"/>"#.repeat(2_000))), 2);
}

/// The blank document with comments and values.
fn filled() -> Document {
    let mut doc = blank();
    let meta = |id: &str| Meta { date: Some("D:20261002120000Z".into()), id: id.into() };
    let note = Shape::Note { at: [100.0, 500.0], icon: pdfcraft_annot::NoteIcon::Comment };
    add_annotation(
        &mut doc,
        &NewAnnotation {
            page: 0,
            style: Style::default_for(&note),
            shape: note,
            contents: "Check this <now> & \"later\"".into(),
            author: "Ada".into(),
        },
        &meta("n1"),
    )
    .unwrap();
    let hl = Shape::TextMarkup { kind: Markup::Highlight, quads: vec![[10.0, 110.0, 90.0, 110.0, 10.0, 100.0, 90.0, 100.0]] };
    add_annotation(
        &mut doc,
        &NewAnnotation { page: 1, style: Style::default_for(&hl), shape: hl, contents: String::new(), author: "Bob".into() },
        &meta("h1"),
    )
    .unwrap();
    let ink = Shape::Ink { strokes: vec![vec![[10.0, 10.0], [20.0, 30.0], [40.0, 20.0]]] };
    add_annotation(
        &mut doc,
        &NewAnnotation { page: 1, style: Style::default_for(&ink), shape: ink, contents: "scribble".into(), author: "Bob".into() },
        &meta("i1"),
    )
    .unwrap();
    let note_index = summaries(&doc).iter().find(|s| s.subtype == "Text").map(|s| s.index).unwrap();
    add_reply(&mut doc, 0, note_index, "Agreed", "Bob", &meta("r1")).unwrap();
    set_value(&mut doc, "Name", &FieldValue::Text("Ada Lovelace".into())).unwrap();
    set_value(&mut doc, "Agree", &FieldValue::Check(true)).unwrap();
    set_value(&mut doc, "Pick", &FieldValue::Choice(vec!["A".into(), "C".into()])).unwrap();
    doc
}

fn comment_view(doc: &Document) -> Vec<String> {
    summaries(doc)
        .iter()
        .map(|s| {
            format!(
                "{} p{} {:?} by {:?}: {:?} reply_to={:?}",
                s.subtype,
                s.page,
                s.rect.map(|v| v.round()),
                s.author,
                s.contents,
                s.in_reply_to.is_some()
            )
        })
        .collect()
}

fn values(doc: &Document) -> Vec<(String, Vec<String>)> {
    fields(doc).into_iter().map(|f| (f.name, f.value)).collect()
}

/// #2666: Replace Text writes a StrikeOut and a Caret that replies to it. XFDF names a reply by
/// the `/NM` of what it replies to, so when both carried one id the caret's `inreplyto` was its
/// own `name`: importing it found a comment replying to itself and the pair was lost.
#[test]
fn a_replace_text_pair_survives_an_xfdf_round_trip() {
    let mut src = blank();
    let quads = [[100.0, 720.0, 200.0, 720.0, 100.0, 706.0, 200.0, 706.0]];
    let style = Style { color: [0.89, 0.13, 0.13], opacity: 1.0, width: 1.0, fill: None };
    let meta = |id: &str| Meta { date: Some("D:20261001120000Z".into()), id: id.into() };
    add_text_replacement(&mut src, 0, &quads, "new words", "Ada", &style, &style, &meta("strike"), &meta("caret")).unwrap();
    assert_eq!(summaries(&src).len(), 2, "a strikeout and a caret");

    let xfdf = export_xfdf(&src, true, false, "a.pdf");
    assert!(xfdf.contains("name=\"strike\"") && xfdf.contains("name=\"caret\""), "each gets its own name: {xfdf}");
    assert!(xfdf.contains("inreplyto=\"strike\""), "the caret replies to the strikeout: {xfdf}");
    assert!(!xfdf.contains("inreplyto=\"caret\""), "never a reply to itself");

    let mut dst = blank();
    let r = import(&mut dst, xfdf.as_bytes()).unwrap();
    assert_eq!(r.comments, 2, "both comments import: {r:?}");
    let found = summaries(&dst);
    let kinds: Vec<&str> = found.iter().map(|s| s.subtype.as_str()).collect();
    assert!(kinds.contains(&"StrikeOut") && kinds.contains(&"Caret"), "{kinds:?}");
    assert_eq!(comment_view(&dst), comment_view(&src), "the pair and its thread survive");
}

#[test]
fn xml_exports_drop_xml_noncharacters() {
    let mut src = filled();
    set_value(&mut src, "Name", &FieldValue::Text("a\u{FFFE}b\u{FFFF}c".into())).unwrap();
    let xfdf = export_xfdf(&src, false, true, "f.pdf");
    let doc = roxmltree::Document::parse(&xfdf).unwrap_or_else(|e| panic!("XFDF is not well-formed: {e}: {xfdf:?}"));
    let value = doc
        .descendants()
        .find(|n| n.has_tag_name("field") && n.attribute("name") == Some("Name"))
        .and_then(|n| n.children().find(|c| c.has_tag_name("value")))
        .and_then(|n| n.text());
    assert_eq!(value, Some("abc"));
    let data = export_data(&src, Format::Xml);
    roxmltree::Document::parse(&data).unwrap_or_else(|e| panic!("XML is not well-formed: {e}: {data:?}"));
}

#[test]
fn xml_exports_preserve_whitespace_in_field_names() {
    let names = ["Line\nName", "Line\tName", "Line\rName", "Line\r\nName", "\t Leading\nTrailing \t", "Line Name"];
    let value = "value\twith\nlines\r\nand <XML> & \"quotes\"";
    let mut src = one_field();
    pdfcraft_forms::delete_field(&mut src, "Name").unwrap();
    for (index, name) in names.iter().enumerate() {
        let placeholder = format!("field{index}");
        add_field(&mut src, 0, [50.0, 700.0, 250.0, 720.0], &NewField::Text { multiline: true }, Some(&placeholder)).unwrap();
        // Existing PDFs can have leading/trailing whitespace even though add_field trims it.
        let field = fields(&src).into_iter().find(|f| f.name == placeholder).unwrap();
        src.update_dict(field.obj, |d| d.set(b"T".to_vec(), PdfString::text(name))).unwrap();
        set_value(&mut src, name, &FieldValue::Text(format!("{index}: {value}"))).unwrap();
    }
    let source_bytes = write_full(&src, &SaveOptions::default()).unwrap();
    let src = Document::open(Arc::new(source_bytes.clone())).unwrap();
    for format in [Format::Xml, Format::Xfdf] {
        let exported = if format == Format::Xfdf { export_xfdf(&src, false, true, "form.pdf") } else { export_data(&src, format) };
        let xml = roxmltree::Document::parse(&exported).unwrap();
        let exported_names: Vec<_> = if format == Format::Xfdf {
            xml.descendants().filter(|n| n.has_tag_name("field")).map(|n| n.attribute("name").unwrap()).collect()
        } else {
            xml.root_element()
                .children()
                .filter(|n| n.is_element())
                .map(|n| n.attribute(("http://ns.adobe.com/xfdf-transition/", "original")).unwrap())
                .collect()
        };
        assert_eq!(exported_names, names, "{format:?}: {exported}");
        let mut dst = Document::open(Arc::new(source_bytes.clone())).unwrap();
        for name in names {
            set_value(&mut dst, name, &FieldValue::Text("replace me".into())).unwrap();
        }
        let report = import(&mut dst, exported.as_bytes()).unwrap();
        assert_eq!(report.fields, names.len(), "{format:?}: {report:?}");
        assert!(report.rejected.is_empty(), "{report:?}");
        let saved = write_full(&dst, &SaveOptions::default()).unwrap();
        let reopened = Document::open(Arc::new(saved)).unwrap();
        assert_eq!(values(&reopened), values(&src), "{format:?}");
    }
    assert_eq!(write_full(&src, &SaveOptions::default()).unwrap(), source_bytes, "export must not change the source PDF");
}

#[test]
fn xfdf_exports_preserve_whitespace_in_comment_attributes() {
    let mut src = one_field();
    let shape = Shape::Note { at: [100.0, 500.0], icon: pdfcraft_annot::NoteIcon::Comment };
    let names = ["note\tline\nidentity", "note line identity"];
    let author = "Ada\tLovelace\nReview";
    let subject = "Subject\twith\nlines\r\nand & quotes\"";
    for name in names {
        add_annotation(
            &mut src,
            &NewAnnotation { page: 0, style: Style::default_for(&shape), shape: shape.clone(), contents: name.into(), author: author.into() },
            &Meta { id: name.into(), date: None },
        )
        .unwrap();
    }
    let annotation_refs = |doc: &Document| {
        let page = pdfcraft_model::pages(doc)[0].obj;
        doc.get(page)
            .as_dict()
            .unwrap()
            .get(b"Annots")
            .map(|a| doc.resolve(a))
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Object::as_ref)
            .filter(|r| doc.get(*r).as_dict().unwrap().name(b"Subtype") == Some(b"Text"))
            .collect::<Vec<_>>()
    };
    for annotation in annotation_refs(&src) {
        src.update_dict(annotation, |d| d.set(b"Subj".to_vec(), PdfString::text(subject))).unwrap();
    }
    let source_bytes = write_full(&src, &SaveOptions::default()).unwrap();
    let mut dst = Document::open(Arc::new(source_bytes)).unwrap();
    let file = "form\twith\nlines.pdf";
    let xfdf = export_xfdf(&dst, true, false, file);
    let xml = roxmltree::Document::parse(&xfdf).unwrap();
    let notes: Vec<_> = xml.descendants().filter(|n| n.has_tag_name("text")).collect();
    assert_eq!(notes.iter().map(|n| n.attribute("name").unwrap()).collect::<Vec<_>>(), names);
    for note in notes {
        assert_eq!(note.attribute("title"), Some(author));
        assert_eq!(note.attribute("subject"), Some(subject));
    }
    assert_eq!(xml.descendants().find(|n| n.has_tag_name("f")).unwrap().attribute("href"), Some(file));
    let before = annotation_refs(&dst);
    let report = import(&mut dst, xfdf.as_bytes()).unwrap();
    assert_eq!(report.comments, names.len());
    assert_eq!(annotation_refs(&dst), before, "reimport must replace the original comments");
    let saved = write_full(&dst, &SaveOptions::default()).unwrap();
    let reopened = Document::open(Arc::new(saved)).unwrap();
    for (annotation, name) in annotation_refs(&reopened).iter().zip(names) {
        let dict = reopened.get(*annotation);
        assert_eq!(text_of(&reopened, dict.as_dict().unwrap(), b"NM").as_deref(), Some(name));
        assert_eq!(text_of(&reopened, dict.as_dict().unwrap(), b"T").as_deref(), Some(author));
        assert_eq!(text_of(&reopened, dict.as_dict().unwrap(), b"Subj").as_deref(), Some(subject));
    }
}

#[test]
fn xfdf_round_trips_comments_and_values() {
    let src = filled();
    let xfdf = export_xfdf(&src, true, true, "form.pdf");
    assert!(xfdf.contains("<text page=\"0\"") && xfdf.contains("&lt;now&gt; &amp; &quot;later&quot;"), "{xfdf}");
    assert!(xfdf.contains("<highlight page=\"1\"") && xfdf.contains("coords=\"10,110,90,110,10,100,90,100\""));
    assert!(xfdf.contains("<gesture>10,10;20,30;40,20</gesture>"));
    assert!(xfdf.contains("inreplyto=\""), "replies keep their thread");
    assert!(xfdf.contains("<field name=\"Pick\"><value>A</value><value>C</value></field>"));
    let mut dst = blank();
    let r = import(&mut dst, xfdf.as_bytes()).unwrap();
    assert_eq!((r.comments, r.fields), (4, 3), "{r:?}");
    assert_eq!(comment_view(&dst), comment_view(&src));
    assert_eq!(values(&dst), values(&src));
    // Importing again replaces (same names) instead of duplicating.
    import(&mut dst, xfdf.as_bytes()).unwrap();
    assert_eq!(summaries(&dst).len(), summaries(&src).len());
    // Comments only.
    let only = export_xfdf(&src, true, false, "form.pdf");
    assert!(!only.contains("<fields>"));
}

#[test]
fn xfdf_round_trip_keeps_zero_flags() {
    let square = |doc: &Document| {
        let page = pdfcraft_model::pages(doc)[0].obj;
        let annots = doc.get(page).as_dict().unwrap().get(b"Annots").map(|a| doc.resolve(a)).unwrap();
        annots
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Object::as_ref)
            .find(|r| doc.get(*r).as_dict().unwrap().name(b"Subtype") == Some(b"Square"))
            .unwrap()
    };
    let mut src = blank();
    let shape = Shape::Rectangle { rect: [10.0, 10.0, 100.0, 60.0] };
    add_annotation(
        &mut src,
        &NewAnnotation { page: 0, style: Style::default_for(&shape), shape, contents: String::new(), author: "A".into() },
        &Meta { id: "s1".into(), date: None },
    )
    .unwrap();
    let annot = square(&src);
    src.update_dict(annot, |d| d.set(b"F".to_vec(), Object::Int(0))).unwrap();
    let xfdf = export_xfdf(&src, true, false, "a.pdf");
    let mut dst = blank();
    import(&mut dst, xfdf.as_bytes()).unwrap();
    let imported = square(&dst);
    let flags = dst.get(imported).as_dict().unwrap().get(b"F").and_then(|f| dst.resolve(f).as_int());
    assert_eq!(flags, Some(0), "flags after round trip: {xfdf}");
}

#[test]
fn fdf_round_trips_values_and_comments() {
    let src = filled();
    let fdf = export_fdf(&src, true, true, "form.pdf").unwrap();
    assert!(fdf.starts_with(b"%FDF-1.2"));
    let mut dst = blank();
    let r = import(&mut dst, &fdf).unwrap();
    assert_eq!(r.fields, 3);
    assert_eq!(values(&dst), values(&src));
    assert!(r.comments >= 3, "{r:?}");
    assert!(summaries(&dst).iter().any(|s| s.contents.as_deref() == Some("scribble")));
    assert_eq!(comment_view(&dst), comment_view(&src), "replies keep their thread");
}

/// One page with a check box whose on state is the Shift-JIS name 「はい」 (`/#82#CD#82#A2`).
fn shift_jis_check_box() -> Document {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [5 0 R] >>",
        "<< /Fields [5 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /V /Off /AS /Off /Rect [50 700 64 714] /P 3 0 R /AP << /N << /Off 6 0 R /#82#CD#82#A2 6 0 R >> >> >>",
        "<< /Length 0 >>\nstream\n\nendstream",
    ];
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
    Document::open(Arc::new(out)).unwrap()
}

#[test]
fn non_utf8_check_box_states_round_trip() {
    let sjis_hai: &[u8] = b"\x82\xcd\x82\xa2";
    let mut src = shift_jis_check_box();
    set_value(&mut src, "agree", &FieldValue::Check(true)).unwrap();
    // FDF writes the state's own bytes, not its #XX text.
    let fdf = export_fdf(&src, false, true, "form.pdf").unwrap();
    let name: &[u8] = b"/#82#CD#82#A2";
    assert!(fdf.windows(name.len()).any(|w| w == name), "{}", String::from_utf8_lossy(&fdf));
    for data in [fdf, export_xfdf(&src, false, true, "form.pdf").into_bytes()] {
        let mut dst = shift_jis_check_box();
        assert_eq!(import(&mut dst, &data).unwrap().fields, 1);
        assert_eq!(values(&dst), values(&src));
        let w = fields(&dst).into_iter().next().unwrap().widgets[0].obj;
        assert_eq!(dst.get(w).as_dict().unwrap().name(b"AS"), Some(sjis_hai));
    }
}

/// Who each comment replies to, by name, with its review state.
fn threads(doc: &Document) -> Vec<(Option<String>, Option<String>, Option<String>)> {
    summaries(doc).into_iter().map(|s| (s.name, s.in_reply_to, s.state)).collect()
}

/// #333: a note with two replies and an Accepted review status keeps its thread through FDF.
#[test]
fn fdf_keeps_replies_and_review_status_on_their_note() {
    let mut src = blank();
    let meta = |id: &str| Meta { date: Some("D:20261002120000Z".into()), id: id.into() };
    let note = Shape::Note { at: [100.0, 100.0], icon: pdfcraft_annot::NoteIcon::Comment };
    add_annotation(
        &mut src,
        &NewAnnotation { page: 1, style: Style::default_for(&note), shape: note, contents: "Literal Note".into(), author: "Alpha".into() },
        &meta("note"),
    )
    .unwrap();
    let index = summaries(&src).iter().find(|s| s.subtype == "Text").map(|s| s.index).unwrap();
    add_reply(&mut src, 1, index, "Reply One", "Beta", &meta("one")).unwrap();
    add_reply(&mut src, 1, index, "Reply Two", "Gamma", &meta("two")).unwrap();
    pdfcraft_annot::set_review_state(&mut src, 1, index, pdfcraft_annot::ReviewState::Accepted, "Review", &meta("status")).unwrap();
    let want = threads(&src);
    assert_eq!(want.iter().filter(|t| t.1.as_deref() == Some("note")).count(), 3, "{want:?}");

    let fdf = export_fdf(&src, true, false, "notes.pdf").unwrap();
    // Every cross-reference entry points at its object (other readers trust the table).
    let tail = std::str::from_utf8(&fdf[fdf.windows(5).position(|w| w == b"xref\n").unwrap()..]).unwrap();
    let xref: usize = tail.lines().skip_while(|l| *l != "startxref").nth(1).unwrap().parse().unwrap();
    assert!(fdf[xref..].starts_with(b"xref\n0 6\n"), "{tail}");
    for (num, line) in tail.lines().skip(3).take(5).enumerate() {
        let at: usize = line[..10].parse().unwrap();
        assert!(fdf[at..].starts_with(format!("{} 0 obj\n", num + 1).as_bytes()), "object {}: {line}", num + 1);
    }
    let mut dst = blank();
    assert_eq!(import(&mut dst, &fdf).unwrap().comments, 4);
    assert_eq!(threads(&dst), want);
    assert_eq!(comment_view(&dst), comment_view(&src));
    // Importing again replaces the thread instead of duplicating or detaching it.
    import(&mut dst, &fdf).unwrap();
    assert_eq!(threads(&dst), want);
}

/// FDFs written before #333 (direct comment dictionaries, no /IRT) still import.
#[test]
fn fdf_with_direct_comments_still_imports() {
    let fdf = b"%FDF-1.2\n1 0 obj\n<< /FDF << /Annots [<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /Contents (old) /NM (a) /Page 0 >>] >> >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n";
    let mut dst = blank();
    assert_eq!(import(&mut dst, fdf).unwrap().comments, 1);
    assert_eq!(threads(&dst), vec![(Some("a".into()), None, None)]);
}

/// /IRT pointing at itself, at a missing object, at a non-comment or in a cycle neither panics
/// nor loops; only a real parent is linked.
#[test]
fn hostile_fdf_reply_parents_are_ignored() {
    let fdf = b"%FDF-1.2\n1 0 obj\n<< /FDF << /Annots [2 0 R 3 0 R 4 0 R 5 0 R 6 0 R] >> >>\nendobj\n\
2 0 obj\n<< /Subtype /Text /Rect [0 0 9 9] /NM (self) /IRT 2 0 R /Page 0 >>\nendobj\n\
3 0 obj\n<< /Subtype /Text /Rect [0 0 9 9] /NM (missing) /IRT 99 0 R /Page 0 >>\nendobj\n\
4 0 obj\n<< /Subtype /Text /Rect [0 0 9 9] /NM (cycle-a) /IRT 5 0 R /Page 0 >>\nendobj\n\
5 0 obj\n<< /Subtype /Text /Rect [0 0 9 9] /NM (cycle-b) /IRT 4 0 R /Page 0 >>\nendobj\n\
6 0 obj\n<< /Subtype /Text /Rect [0 0 9 9] /NM (number) /IRT 42 /Page 99 >>\nendobj\n\
trailer\n<< /Root 1 0 R >>\n%%EOF\n";
    let mut dst = blank();
    assert_eq!(import(&mut dst, fdf).unwrap().comments, 4);
    let t = threads(&dst);
    let parent = |nm: &str| t.iter().find(|x| x.0.as_deref() == Some(nm)).and_then(|x| x.1.clone());
    assert_eq!(parent("self"), None);
    assert_eq!(parent("missing"), None);
    assert_eq!((parent("cycle-a").as_deref(), parent("cycle-b").as_deref()), (Some("cycle-b"), Some("cycle-a")));
}

#[test]
fn xml_field_names_remain_valid_and_round_trip_without_collisions() {
    assert_eq!(xml_name(".Value"), "_.Value");
    let names = [
        ("Area²", "Area_"),
        ("Area_", "Area_"),
        ("Aµ", "A_"),
        ("µRate", "_Rate"),
        ("²Rate", "_Rate"),
        ("½Amount", "_Amount"),
        ("1Rate", "_1Rate"),
        ("Café", "Café"),
        ("Δείγμα", "Δείγμα"),
        ("姓名", "姓名"),
        ("٣Value", "_٣Value"),
        ("\u{345}Rate", "_\u{345}Rate"),
        ("A\u{345}", "A\u{345}"),
        ("Alpha_1-2", "Alpha_1-2"),
        ("ns:value", "ns_value"),
        ("-Value", "_-Value"),
    ];
    let mut src = one_field();
    pdfcraft_forms::delete_field(&mut src, "Name").unwrap();
    for (index, (name, _)) in names.iter().enumerate() {
        add_field(&mut src, 0, [50.0, 700.0, 250.0, 720.0], &NewField::Text { multiline: false }, Some(name)).unwrap();
        set_value(&mut src, name, &FieldValue::Text(format!("value-{index}"))).unwrap();
    }
    let source = Arc::new(write_full(&src, &SaveOptions::default()).unwrap());
    let src = Document::open(source.clone()).unwrap();
    let want = values(&src);
    for format in [Format::Xml, Format::Xdp] {
        let text = export_data(&src, format);
        let xml = roxmltree::Document::parse(&text).unwrap();
        let children: Vec<_> = xml.root_element().children().filter(|n| n.is_element()).collect();
        assert_eq!(children.len(), names.len());
        for (child, (original, tag)) in children.iter().zip(names) {
            assert_eq!(child.tag_name().name(), tag);
            assert_eq!(child.attribute(("http://ns.adobe.com/xfdf-transition/", "original")), (original != tag).then_some(original));
        }
        assert!(!src.is_modified(), "export must not change the source PDF");
        assert_eq!(src.bytes().as_slice(), source.as_slice());

        let mut dst = Document::open(source.clone()).unwrap();
        for (name, _) in names {
            set_value(&mut dst, name, &FieldValue::Text("replaced".into())).unwrap();
        }
        let report = import(&mut dst, text.as_bytes()).unwrap();
        assert_eq!(report.fields, names.len());
        assert!(report.rejected.is_empty(), "{report:?}");
        let reopened = Document::open(Arc::new(write_full(&dst, &SaveOptions::default()).unwrap())).unwrap();
        assert_eq!(values(&reopened), want, "{format:?}: sanitized names must preserve distinct original fields");
    }
}

#[test]
fn form_data_as_xml_csv_and_text() {
    let src = filled();
    let xml = export_data(&src, Format::Xml);
    assert!(xml.contains("<Name>Ada Lovelace</Name>") && xml.contains("<Agree>Yes</Agree>"), "{xml}");
    let txt = export_data(&src, Format::Txt);
    assert_eq!(txt, "Name\tAgree\tPick\nAda Lovelace\tYes\tA, C\n");
    let csv_text = export_data(&src, Format::Csv);
    assert_eq!(csv_text, "Name,Agree,Pick\nAda Lovelace,Yes,\"A, C\"\n");
    for data in [xml, txt] {
        let mut dst = blank();
        let r = import(&mut dst, data.as_bytes()).unwrap();
        assert!(r.fields >= 2, "{r:?}");
        assert_eq!(values(&dst)[0].1, ["Ada Lovelace"]);
        assert_eq!(values(&dst)[1].1, ["Yes"]);
    }
    let mut dst = blank();
    import(&mut dst, b"Name,Agree\n\"Lovelace, Ada\",Off\n").unwrap();
    assert_eq!(values(&dst)[0].1, ["Lovelace, Ada"]);
}

#[test]
fn csv_values_may_span_lines() {
    // A quoted CSV value may hold line breaks (export_data writes multiline text that way): the
    // record goes on to the closing quote, so later columns still import.
    let mut dst = blank();
    let r = import(&mut dst, b"Name,Agree\r\n\"Ada \"\"the\"\"\r\nCountess\",Yes\r\n").unwrap();
    assert_eq!(r.fields, 2, "{r:?}");
    assert!(values(&dst)[0].1[0].starts_with("Ada \"the\"") && values(&dst)[0].1[0].ends_with("Countess"), "{:?}", values(&dst));
    assert_eq!(values(&dst)[1].1, ["Yes"]);
    // A header without a record is still not a data file.
    assert!(import(&mut blank(), b"Name,Agree\r\n").is_err());
    // An unterminated quote runs to the end of the file and is read leniently, as before.
    assert!(import(&mut blank(), b"Name,Agree\r\n\"Ada, Yes\r\n").is_ok());
}

/// One page with one text field, `Name`.
fn one_field() -> Document {
    let mut doc = Document::new_empty();
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let mut p = Dict::new();
    p.set(b"Type".to_vec(), Object::name("Page"));
    p.set(b"Parent".to_vec(), Object::Ref(pages));
    p.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 600.into(), 800.into()]));
    let page = doc.add(p);
    doc.update_dict(pages, |d| {
        d.set(b"Count".to_vec(), Object::Int(1));
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
    })
    .unwrap();
    add_field(&mut doc, 0, [50.0, 700.0, 250.0, 720.0], &NewField::Text { multiline: false }, Some("Name")).unwrap();
    Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap()
}

#[test]
fn csv_exports_preserve_carriage_returns_in_names_and_values() {
    for (name, value, expected) in [
        ("Name", "plain", "Name\nplain\n"),
        ("Name", "Tail\r", "Name\n\"Tail\r\"\n"),
        ("Name", "Alpha\rBeta", "Name\n\"Alpha\rBeta\"\n"),
        ("Name", "\rLead", "Name\n\"\rLead\"\n"),
        ("Name", "\r", "Name\n\"\r\"\n"),
        ("Name", "Alpha\r\nBeta", "Name\n\"Alpha\r\nBeta\"\n"),
        ("Name", "Alpha\nBeta", "Name\n\"Alpha\nBeta\"\n"),
        ("Name", "comma,\r", "Name\n\"comma,\r\"\n"),
        ("Name", "\"quoted\"\r", "Name\n\"\"\"quoted\"\"\r\"\n"),
        ("Name", "before\r,\"after\"", "Name\n\"before\r,\"\"after\"\"\"\n"),
        ("Na\rme", "both\r", "\"Na\rme\"\n\"both\r\"\n"),
    ] {
        let mut src = one_field();
        let field = fields(&src)[0].obj;
        src.update_dict(field, |d| d.set(b"T".to_vec(), PdfString::text(name))).unwrap();
        set_value(&mut src, name, &FieldValue::Text(value.into())).unwrap();
        let source = Arc::new(write_full(&src, &SaveOptions::default()).unwrap());
        let src = Document::open(source.clone()).unwrap();
        assert_eq!(values(&src), [(name.to_string(), vec![value.to_string()])]);
        let csv = export_data(&src, Format::Csv);

        let mut dst = Document::open(source).unwrap();
        set_value(&mut dst, name, &FieldValue::Text("replaced".into())).unwrap();
        let report = import(&mut dst, csv.as_bytes()).unwrap();
        assert_eq!(report.fields, 1);
        assert!(report.rejected.is_empty(), "{report:?}");
        let reopened = Document::open(Arc::new(write_full(&dst, &SaveOptions::default()).unwrap())).unwrap();
        assert_eq!(values(&reopened), [(name.to_string(), vec![value.to_string()])]);
        assert_eq!(csv, expected, "CSV must quote CR as well as LF, commas and quotes");
    }
}

/// #1321: the text export of a one-field form has no tab to tell it from CSV, so a comma in the
/// value used to split it and only the text before the comma was imported.
#[test]
fn one_field_text_data_keeps_its_commas() {
    for value in ["Hello, world", "Say \"hi\", twice"] {
        let mut src = one_field();
        set_value(&mut src, "Name", &FieldValue::Text(value.into())).unwrap();
        let txt = export_data(&src, Format::Txt);
        assert_eq!(txt, format!("Name\n{value}\n"));
        let mut dst = one_field();
        let r = import(&mut dst, txt.as_bytes()).unwrap();
        assert_eq!(r.fields, 1, "{r:?}");
        assert!(r.rejected.is_empty(), "{r:?}");
        assert_eq!(values(&dst)[0].1, [value]);
    }
}

/// A one-column CSV is still CSV: quotes come off, and a quoted value keeps its commas and line
/// breaks.
#[test]
fn one_column_csv_still_reads_its_quotes() {
    let mut src = one_field();
    set_value(&mut src, "Name", &FieldValue::Text("Hello, world".into())).unwrap();
    let csv = export_data(&src, Format::Csv);
    assert_eq!(csv, "Name\n\"Hello, world\"\n");
    let mut dst = one_field();
    assert_eq!(import(&mut dst, csv.as_bytes()).unwrap().fields, 1);
    assert_eq!(values(&dst)[0].1, ["Hello, world"]);
    let mut dst = one_field();
    assert_eq!(import(&mut dst, b"Name\r\n\"Ada \"\"the\"\", \r\nCountess\"\r\n").unwrap().fields, 1);
    assert!(values(&dst)[0].1[0].starts_with("Ada \"the\", ") && values(&dst)[0].1[0].ends_with("Countess"), "{:?}", values(&dst));
}

#[test]
fn quoted_empty_csv_value_clears_a_field() {
    for data in ["Name\n\"\"\n", "Name\r\n\"\"\r\n", "Name\n\"\"", "Name\n\n\"\"\n", "Name\n\"\"\nGrace\n"] {
        let mut dst = one_field();
        set_value(&mut dst, "Name", &FieldValue::Text("Ada".into())).unwrap();
        let report = import(&mut dst, data.as_bytes()).unwrap();
        assert_eq!(report.fields, 1, "{data:?}");
        assert!(report.rejected.is_empty(), "{report:?}");
        assert!(values(&dst)[0].1.is_empty(), "{data:?}");
    }
}

#[test]
fn multi_select_lists_round_trip_through_xml_csv_and_text() {
    let src = filled();
    for format in [Format::Xml, Format::Csv, Format::Txt] {
        let mut dst = blank();
        let r = import(&mut dst, export_data(&src, format).as_bytes()).unwrap();
        assert!(r.rejected.is_empty(), "{format:?}: {r:?}");
        assert_eq!(values(&dst)[2].1, ["A", "C"], "{format:?}");
    }
}

#[test]
fn bad_data_is_refused_with_a_reason() {
    let mut dst = blank();
    assert!(matches!(import(&mut dst, b"<xfdf><annots>"), Err(DataError::Malformed(_))));
    assert_eq!(import(&mut dst, b"Unknown\nvalue\n"), Err(DataError::NothingImported));
    assert_eq!(Format::from_extension("XFDF"), Some(Format::Xfdf));
    assert_eq!(Format::from_extension("docx"), None);
}

#[test]
fn data_files_merge_into_a_spreadsheet() {
    let src = filled();
    let xfdf = export_xfdf(&src, false, true, "form.pdf");
    let fdf = export_fdf(&src, false, true, "form.pdf").unwrap();
    let mut other = blank();
    set_value(&mut other, "Name", &FieldValue::Text("Grace \"Amazing\" Hopper, RADM".into())).unwrap();
    let pdf = pdfcraft_cos::write_full(&other, &Default::default()).unwrap();
    let rows: Vec<_> = [xfdf.as_bytes(), &fdf[..], &pdf[..]].iter().map(|b| data_values(b).unwrap()).collect();
    assert_eq!(rows[0], rows[1], "XFDF and FDF carry the same values");
    let csv = merge_csv(&rows);
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines[0], "Name,Agree,Pick");
    assert_eq!(lines[1], "Ada Lovelace,Yes,\"A, C\"");
    assert!(lines[3].starts_with("\"Grace \"\"Amazing\"\" Hopper, RADM\",Off,"), "{csv}");
    assert_eq!(data_values(b"not data"), Err(DataError::UnknownFormat));
}

#[test]
fn unicode_annotation_colours_are_ignored_without_panicking() {
    for attr in ["color", "interior-color"] {
        let mut doc = blank();
        let xml = format!(
            r##"<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots><square page="0" rect="10,10,50,50" name="unicode-colour" {attr}="#€€" /></annots></xfdf>"##
        );
        let report = import(&mut doc, xml.as_bytes()).unwrap();
        assert_eq!(report.comments, 1);
    }
}

/// Importing the same note again must not leave the earlier pop-up on the page.
#[test]
fn reimporting_a_note_replaces_its_popup() {
    let xfdf = concat!(
        r#"<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots><text page="0" name="n1" rect="10,10,30,30"><contents>body</contents>"#,
        r#"<popup page="0" rect="40,10,140,60" open="no"/></text></annots></xfdf>"#,
    );
    let mut doc = blank();
    for _ in 0..3 {
        import(&mut doc, xfdf.as_bytes()).unwrap();
    }
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let annots = doc.get(page).as_dict().and_then(|p| p.get(b"Annots").cloned()).and_then(|a| doc.resolve(&a).as_array().cloned()).unwrap();
    let refs: Vec<ObjRef> = annots.iter().filter_map(|a| a.as_ref()).collect();
    let is_popup = |r: &ObjRef| doc.get(*r).as_dict().and_then(|d| d.name(b"Subtype")) == Some(b"Popup".as_slice());
    let popups: Vec<ObjRef> = refs.iter().copied().filter(is_popup).collect();
    assert_eq!(popups.len(), 1, "one current pop-up, not one per import");
    let parent = doc.get(popups[0]).as_dict().and_then(|d| d.reference(b"Parent")).unwrap();
    assert!(refs.contains(&parent), "the pop-up belongs to a note that is still on the page");
}

/// A partial import updates the parent without detaching replies and review statuses that
/// weren't included in the data file. FDF export only carries parents still on a page.
#[test]
fn replacing_a_note_keeps_existing_replies_and_review_status() {
    let xfdf = concat!(
        r#"<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots>"#,
        r#"<text page="0" name="n1" rect="100,500,120,520"><contents>Updated note</contents></text>"#,
        r#"</annots></xfdf>"#,
    );
    let mut incoming = blank();
    import(&mut incoming, xfdf.as_bytes()).unwrap();
    let mut original = filled();
    let index = summaries(&original).iter().find(|s| s.name.as_deref() == Some("n1")).unwrap().index;
    pdfcraft_annot::set_review_state(
        &mut original,
        0,
        index,
        pdfcraft_annot::ReviewState::Accepted,
        "Reviewer",
        &Meta { date: None, id: "status".into() },
    )
    .unwrap();
    let original = Arc::new(write_full(&original, &SaveOptions::default()).unwrap());
    for data in [xfdf.as_bytes().to_vec(), export_fdf(&incoming, true, false, "notes.pdf").unwrap()] {
        // Start with the parent, reply and review status in the existing PDF revision.
        let mut doc = Document::open(original.clone()).unwrap();
        let mut want = threads(&doc);
        want.sort();
        assert_eq!(want.iter().filter(|(_, parent, _)| parent.as_deref() == Some("n1")).count(), 2);
        assert_eq!(import(&mut doc, &data).unwrap().comments, 1);
        let note = summaries(&doc).into_iter().find(|s| s.name.as_deref() == Some("n1")).unwrap();
        assert_eq!(note.contents.as_deref(), Some("Updated note"));

        // Check the saved object graph and a subsequent export/import, rather than relying on
        // summaries resolving an obsolete parent's name from an object outside /Annots.
        for incremental in [true, false] {
            let saved = if incremental {
                let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
                assert!(saved.starts_with(original.as_slice()), "an incremental save preserves the original revision");
                saved
            } else {
                write_full(&doc, &SaveOptions::default()).unwrap()
            };
            let reopened = Document::open(Arc::new(saved)).unwrap();
            let note = summaries(&reopened).into_iter().find(|s| s.name.as_deref() == Some("n1")).unwrap();
            assert_eq!(note.contents.as_deref(), Some("Updated note"));
            let fdf = export_fdf(&reopened, true, false, "notes.pdf").unwrap();
            let mut dst = blank();
            import(&mut dst, &fdf).unwrap();
            let mut got = threads(&dst);
            got.sort();
            assert_eq!(got, want, "partial comment import must preserve reply and status threads (incremental={incremental})");
        }
    }
}

#[test]
fn replacing_an_indirectly_named_note_preserves_its_identity_and_thread() {
    let initial = concat!(
        r#"<xfdf><annots><text page="0" name="n1" rect="10,10,30,30"><contents>Original</contents></text>"#,
        r#"<text page="0" name="r1" rect="40,10,60,30" inreplyto="n1"><contents>Reply</contents></text>"#,
        r#"<text page="0" name="s1" rect="70,10,90,30" inreplyto="n1" state="Accepted" statemodel="Review"/>"#,
        r#"</annots></xfdf>"#,
    );
    let update = br#"<xfdf><annots><text page="0" name="n1" rect="10,10,30,30"><contents>Updated</contents></text></annots></xfdf>"#;
    let mut original = blank();
    for name in ["Name", "Agree", "Pick"] {
        pdfcraft_forms::delete_field(&mut original, name).unwrap();
    }
    import(&mut original, initial.as_bytes()).unwrap();
    let annotation_refs = |doc: &Document| {
        let page = pdfcraft_model::pages(doc)[0].obj;
        doc.get(page)
            .as_dict()
            .unwrap()
            .get(b"Annots")
            .map(|a| doc.resolve(a))
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Object::as_ref)
            .collect::<Vec<_>>()
    };
    let parent = annotation_refs(&original)[0];
    let indirect_name = original.add(PdfString::text("n1"));
    original.update_dict(parent, |d| d.set(b"NM".to_vec(), Object::Ref(indirect_name))).unwrap();
    let original = Arc::new(write_full(&original, &SaveOptions::default()).unwrap());
    let mut incoming = blank();
    import(&mut incoming, update).unwrap();
    for data in [update.to_vec(), export_fdf(&incoming, true, false, "notes.pdf").unwrap()] {
        let mut doc = Document::open(original.clone()).unwrap();
        let refs = annotation_refs(&doc);
        assert_eq!(refs.len(), 3);
        let parent = refs.iter().copied().find(|r| text_of(&doc, doc.get(*r).as_dict().unwrap(), b"NM").as_deref() == Some("n1")).unwrap();
        assert!(doc.get(parent).as_dict().unwrap().get(b"NM").unwrap().as_ref().is_some(), "the saved source name is indirect");
        let mut want = threads(&doc);
        want.sort();
        assert_eq!(import(&mut doc, &data).unwrap().comments, 1);
        let updated = annotation_refs(&doc);
        assert_eq!(updated.len(), 3, "a valid indirect name must replace its comment instead of duplicating it");
        assert!(updated.contains(&parent), "replacement preserves the source object's identity");
        assert_eq!(text_of(&doc, doc.get(parent).as_dict().unwrap(), b"Contents").as_deref(), Some("Updated"));
        for child in refs.into_iter().filter(|r| *r != parent) {
            assert_eq!(doc.get(child).as_dict().unwrap().reference(b"IRT"), Some(parent));
        }
        let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
        assert!(saved.starts_with(original.as_slice()));
        let reopened = Document::open(Arc::new(saved)).unwrap();
        let mut got = threads(&reopened);
        got.sort();
        assert_eq!(got, want);
        assert_eq!(summaries(&reopened).iter().find(|s| s.name.as_deref() == Some("n1")).unwrap().contents.as_deref(), Some("Updated"));
    }
}

#[test]
fn replacement_matches_named_comments_only_on_their_page() {
    let xfdf = concat!(
        r#"<xfdf><annots>"#,
        r#"<text page="0" name="same" rect="10,10,30,30"><contents>First page</contents></text>"#,
        r#"<text page="1" name="same" rect="10,10,30,30"><contents>Second page</contents></text>"#,
        r#"<text page="0" rect="40,40,60,60"><contents>Unnamed</contents></text>"#,
        r#"</annots></xfdf>"#,
    );
    let mut doc = blank();
    import(&mut doc, xfdf.as_bytes()).unwrap();
    import(&mut doc, xfdf.as_bytes()).unwrap();
    let notes = summaries(&doc);
    assert_eq!(notes.len(), 4, "named comments are replaced, unnamed comments are added");
    for (page, text) in [(0, "First page"), (1, "Second page")] {
        let note = notes.iter().find(|s| s.page == page && s.name.as_deref() == Some("same")).unwrap();
        assert_eq!(note.contents.as_deref(), Some(text));
    }
    assert_eq!(notes.iter().filter(|s| s.name.is_none()).count(), 2);
}

#[test]
fn the_last_same_named_record_controls_its_reply_parent() {
    let mut incoming = blank();
    let page = pdfcraft_model::pages(&incoming)[0].obj;
    let mut refs = Vec::new();
    for name in ["parent", "duplicate", "duplicate"] {
        let mut d = Dict::new();
        d.set(b"Subtype".to_vec(), Object::name("Text"));
        d.set(b"NM".to_vec(), PdfString::text(name));
        d.set(b"Rect".to_vec(), arr(&[10.0, 10.0, 30.0, 30.0]));
        if refs.len() == 1 {
            d.set(b"IRT".to_vec(), Object::Ref(refs[0]));
        }
        refs.push(incoming.add(d));
    }
    incoming.update_dict(page, |p| p.set(b"Annots".to_vec(), Object::Array(refs.into_iter().map(Object::Ref).collect()))).unwrap();
    for data in [export_xfdf(&incoming, true, false, "notes.pdf").into_bytes(), export_fdf(&incoming, true, false, "notes.pdf").unwrap()] {
        let mut doc = blank();
        import(&mut doc, &data).unwrap();
        let notes = summaries(&doc);
        assert_eq!(notes.len(), 2);
        let last = notes.iter().find(|s| s.name.as_deref() == Some("duplicate")).unwrap();
        assert_eq!(last.in_reply_to, None, "a replaced record's deferred /IRT must not attach the final standalone comment");
    }
}

#[test]
fn indirectly_named_protected_comments_and_popups_are_not_replaced() {
    let xfdf = br#"<xfdf><annots><redact page="0" name="n1" rect="0,0,1,1" flags="invisible,hidden,noview,print"><contents>Updated</contents></redact></annots></xfdf>"#;
    let fdf = b"%FDF-1.2\n1 0 obj\n<< /FDF << /Annots [<< /Subtype /Redact /NM (n1) /Page 0 /Rect [0 0 1 1] /F 39 /Contents (Updated) >>] >> >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n";
    let annotation_refs = |doc: &Document| {
        pdfcraft_model::pages(doc)[0]
            .dict
            .get(b"Annots")
            .map(|a| doc.resolve(a))
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Object::as_ref)
            .collect::<Vec<_>>()
    };
    for (subtype, flags) in [("Text", LOCKED), ("Redact", 0)] {
        let mut original = blank();
        let page = pdfcraft_model::pages(&original)[0].obj;
        let name = original.add(PdfString::text("n1"));
        let mut protected = Dict::new();
        protected.set(b"Type".to_vec(), Object::name("Annot"));
        protected.set(b"Subtype".to_vec(), Object::name(subtype));
        protected.set(b"NM".to_vec(), Object::Ref(name));
        protected.set(b"Rect".to_vec(), arr(&[10.0, 10.0, 30.0, 30.0]));
        protected.set(b"F".to_vec(), Object::Int(flags));
        protected.set(b"Contents".to_vec(), PdfString::text("Original"));
        protected.set(b"CustomData".to_vec(), PdfString::text("preserved"));
        let parent = original.add(protected);
        let mut popup = Dict::new();
        popup.set(b"Subtype".to_vec(), Object::name("Popup"));
        popup.set(b"Rect".to_vec(), arr(&[40.0, 10.0, 140.0, 60.0]));
        popup.set(b"Parent".to_vec(), Object::Ref(parent));
        let popup = original.add(popup);
        original.update_dict(parent, |d| d.set(b"Popup".to_vec(), Object::Ref(popup))).unwrap();
        let mut annots = annotation_refs(&original);
        annots.extend([parent, popup]);
        original.update_dict(page, |d| d.set(b"Annots".to_vec(), Object::Array(annots.into_iter().map(Object::Ref).collect()))).unwrap();
        let original = Arc::new(write_full(&original, &SaveOptions::default()).unwrap());
        for data in [xfdf.as_slice(), fdf.as_slice()] {
            let mut doc = Document::open(original.clone()).unwrap();
            let before = annotation_refs(&doc);
            let parent = before.iter().copied().find(|r| text_of(&doc, doc.get(*r).as_dict().unwrap(), b"NM").as_deref() == Some("n1")).unwrap();
            let protected = doc.get(parent);
            let popup_ref = protected.as_dict().unwrap().reference(b"Popup").unwrap();
            let popup = doc.get(popup_ref);
            assert!(protected.as_dict().unwrap().get(b"NM").unwrap().as_ref().is_some());
            assert_eq!(import(&mut doc, data).unwrap().comments, 1);
            let after = annotation_refs(&doc);
            assert_eq!(after.len(), before.len() + 1);
            assert!(after.contains(&parent) && after.contains(&popup_ref));
            assert_eq!(doc.get(parent), protected, "the protected {subtype} must not change, including its unknown data");
            assert_eq!(doc.get(popup_ref), popup, "the protected comment's popup must not change");
            let added = after.into_iter().find(|r| !before.contains(r)).unwrap();
            let added = doc.get(added);
            let added = added.as_dict().unwrap();
            assert!(added.get(b"NM").is_none(), "the imported comment must relinquish the protected name");
            assert_eq!(added.int(b"F"), Some(4), "an imported redaction stays printable with Invisible/Hidden/NoView removed");
            let reopened = Document::open(Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())).unwrap();
            assert_eq!(reopened.get(parent), protected);
            assert_eq!(reopened.get(popup_ref), popup);
        }
    }
}

#[test]
fn imported_redact_marks_cannot_replace_a_locked_mark_or_hide_themselves() {
    let mut doc = blank();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    // A locked redaction mark the user placed, with a known /NM.
    let mut mark = Dict::new();
    mark.set(b"Type".to_vec(), Object::name("Annot"));
    mark.set(b"Subtype".to_vec(), Object::name("Redact"));
    mark.set(b"NM".to_vec(), PdfString::text("mark1"));
    mark.set(b"Rect".to_vec(), Object::Array(vec![60.into(), 660.into(), 560.into(), 678.into()]));
    mark.set(b"F".to_vec(), Object::Int(128)); // Locked
    let mark_ref = doc.add(Object::Dict(mark));
    let mut annots =
        doc.get(page).as_dict().and_then(|p| p.get(b"Annots").cloned()).and_then(|a| doc.resolve(&a).as_array().cloned()).unwrap_or_default();
    annots.push(Object::Ref(mark_ref));
    doc.update_dict(page, |p| p.set(b"Annots".to_vec(), Object::Array(annots))).unwrap();

    // A reviewer's XFDF: shrink the locked mark to a 1×1 corner by reusing its /NM, and add a
    // redaction mark imported as Hidden that would remove other content unseen.
    let xfdf = concat!(
        r#"<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots>"#,
        r#"<redact page="0" name="mark1" rect="0,0,1,1" title="Reviewer"/>"#,
        r#"<redact page="0" name="ghost" rect="60,600,560,620" flags="hidden" title="Reviewer"/>"#,
        r#"</annots></xfdf>"#,
    );
    import(&mut doc, xfdf.as_bytes()).unwrap();

    let refs: Vec<ObjRef> = doc
        .get(page)
        .as_dict()
        .and_then(|p| p.get(b"Annots").cloned())
        .and_then(|a| doc.resolve(&a).as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|a| a.as_ref())
        .collect();
    // The locked mark is still there, unchanged (same object, same geometry, still its /NM).
    assert!(refs.contains(&mark_ref), "the locked redaction mark was replaced");
    let locked = doc.get(mark_ref).as_dict().cloned().unwrap();
    assert_eq!(nums_of(&doc, locked.get(b"Rect")), vec![60.0, 660.0, 560.0, 678.0], "the locked mark's geometry was changed");
    assert_eq!(locked.get(b"NM").and_then(|n| n.as_string().map(|s| s.to_text())).as_deref(), Some("mark1"));
    assert_eq!(locked.int(b"F"), Some(128), "the locked mark lost its Locked flag");
    // Every redaction mark on the page is visible (none Invisible/Hidden/NoView): the import's two
    // marks were added beside the protected one, both drawable for review.
    let redacts: Vec<Dict> =
        refs.iter().map(|r| doc.get(*r).as_dict().cloned().unwrap()).filter(|d| d.name(b"Subtype") == Some(b"Redact".as_slice())).collect();
    assert_eq!(redacts.len(), 3, "the imported marks should be added, not merged away");
    for d in &redacts {
        assert_eq!(d.int(b"F").unwrap_or(0) & (1 | 2 | 32), 0, "a redaction mark is hidden from review: {d:?}");
    }
}

/// The `/Vertices` of every annotation on the document's pages, by subtype.
fn vertices(doc: &Document) -> Vec<(String, Vec<f64>)> {
    let mut out = Vec::new();
    for p in pdfcraft_model::pages(doc) {
        for a in p.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default() {
            let Some(d) = doc.resolve(&a).as_dict().cloned() else { continue };
            let sub = String::from_utf8_lossy(d.name(b"Subtype").unwrap_or_default()).into_owned();
            let v = nums_of(doc, d.get(b"Vertices"));
            if !v.is_empty() {
                out.push((sub, v));
            }
        }
    }
    out
}

/// storytold/pdfcraft#809: XFDF exchange dropped polygon and polyline vertices, so the
/// imported comments had no shape (and no appearance).
#[test]
fn xfdf_keeps_polygon_and_polyline_vertices() {
    let mut src = blank();
    let meta = |id: &str| Meta { date: Some("D:20261002120000Z".into()), id: id.into() };
    let polygon = Shape::Polygon { vertices: vec![[100.0, 100.0], [200.0, 120.0], [150.0, 220.5]], cloud: false };
    let polyline = Shape::PolyLine {
        vertices: vec![[50.0, 300.0], [120.0, 380.0], [200.0, 310.0]],
        start: pdfcraft_annot::LineEnding::None,
        end: pdfcraft_annot::LineEnding::ClosedArrow,
    };
    for (page, shape, id) in [(0, polygon, "pg"), (1, polyline, "pl")] {
        let new = NewAnnotation { page, style: Style::default_for(&shape), shape, contents: id.into(), author: "Ada".into() };
        add_annotation(&mut src, &new, &meta(id)).unwrap();
    }
    let xfdf = export_xfdf(&src, true, false, "shapes.pdf");
    assert!(xfdf.contains("<vertices>100,100;200,120;150,220.5</vertices>"), "{xfdf}");
    assert!(xfdf.contains("<vertices>50,300;120,380;200,310</vertices>"), "{xfdf}");
    let mut dst = blank();
    let r = import(&mut dst, xfdf.as_bytes()).unwrap();
    assert_eq!(r.comments, 2, "{r:?}");
    assert_eq!(vertices(&dst), vertices(&src));
    // The imported shapes are drawn again.
    for p in pdfcraft_model::pages(&dst) {
        for a in p.dict.get(b"Annots").map(|a| dst.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default() {
            let d = dst.resolve(&a).as_dict().cloned().unwrap();
            assert!(d.get(b"AP").is_some(), "{d:?}");
        }
    }
    // Acrobat-style input: a stray trailing number is dropped and only whole points are kept.
    let odd = r#"<?xml version="1.0"?><xfdf xmlns="http://ns.adobe.com/xfdf/"><annots>
        <polygon page="0" rect="0,0,50,50" name="odd"><vertices>1,2;3,4;5,6;7</vertices></polygon>
        <polygon page="0" rect="0,0,50,50" name="none"><vertices>junk</vertices></polygon>
        </annots></xfdf>"#;
    let mut dst = blank();
    import(&mut dst, odd.as_bytes()).unwrap();
    assert_eq!(vertices(&dst), vec![("Polygon".to_string(), vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0])]);
}

#[test]
fn xfdf_keeps_callout_leader_inset_and_ending() {
    let mut src = blank();
    let shape = Shape::Callout {
        rect: [40.0, 300.0, 180.0, 360.0],
        knee: [260.0, 330.0],
        point: [320.0, 200.0],
        font_size: 12.0,
        ending: pdfcraft_annot::LineEnding::ClosedArrow,
    };
    let meta = Meta { date: Some("D:20261002120000Z".into()), id: "c1".into() };
    add_annotation(
        &mut src,
        &NewAnnotation { page: 0, style: Style::default_for(&shape), shape, contents: "leader".into(), author: "Ada".into() },
        &meta,
    )
    .unwrap();
    let xfdf = export_xfdf(&src, true, false, "form.pdf");
    let mut dst = blank();
    import(&mut dst, xfdf.as_bytes()).unwrap();
    let callout_keys = |doc: &Document| {
        let page = pdfcraft_model::pages(doc).remove(0);
        let annots = page.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        let d = annots.iter().filter_map(|a| doc.resolve(a).as_dict().cloned()).find(|d| d.name(b"Subtype") == Some(b"FreeText")).unwrap();
        (
            nums_of(doc, d.get(b"Rect")),
            nums_of(doc, d.get(b"CL")),
            nums_of(doc, d.get(b"RD")),
            d.name(b"IT").map(<[u8]>::to_vec),
            d.get(b"LE").and_then(|o| o.as_name()).map(<[u8]>::to_vec),
        )
    };
    let want = callout_keys(&src);
    assert_eq!(want.1.len(), 6, "{want:?}");
    assert_eq!(callout_keys(&dst), want, "{xfdf}");
}

/// Run `f` on a thread with a small stack, as a deep recursion would overflow it.
fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new().stack_size(256 << 10).spawn(f).unwrap().join().unwrap()
}

#[test]
fn deeply_nested_xml_is_refused_before_parsing() {
    // A file nested far deeper than any stack holds used to overflow roxmltree's recursive
    // parser, which aborts the process. It must be refused without recursing.
    let deep = |root: &str| format!("<{root}>{}{}</{root}>", "<a>".repeat(100_000), "</a>".repeat(100_000));
    let xfdf = deep("xfdf");
    let xml = deep("fields");
    let (imported, table, values) = on_small_stack(move || {
        let mut doc = blank();
        (import(&mut doc, xfdf.as_bytes()), import(&mut doc, xml.as_bytes()), data_values(xfdf.as_bytes()))
    });
    for r in [imported.map(|_| ()), table.map(|_| ()), values.map(|_| ())] {
        assert!(matches!(&r, Err(DataError::Malformed(m)) if m.contains("deep")), "{r:?}");
    }
}

#[test]
fn wide_or_commented_xml_is_not_mistaken_for_deep() {
    // Thousands of siblings, self-closing tags, `>` in attribute values, and tags inside
    // comments, CDATA and processing instructions add no depth.
    let mut x = String::from("<?xml version=\"1.0\"?><!-- <a><a><a> --><xfdf><fields>");
    for i in 0..5_000 {
        x.push_str(&format!("<field name=\"f{i} > g\"><value>v</value></field><empty/><![CDATA[<b><b>]]><?pi <c> ?>"));
    }
    x.push_str("</fields></xfdf>");
    assert_eq!(check_form_xml_depth(&x), Ok(()));
    let nested = |n: usize| format!("{}{}", "<a>".repeat(n), "</a>".repeat(n));
    assert_eq!(check_form_xml_depth(&nested(MAX_XML_DEPTH)), Ok(()));
    assert!(check_form_xml_depth(&nested(MAX_XML_DEPTH + 1)).is_err());
}

/// [`one_field`] with its field's `/T` set to `name`, periods and all, holding `value`.
fn one_field_named(name: &str, value: &str) -> Document {
    let mut doc = one_field();
    let r = fields(&doc).into_iter().find(|f| f.name == "Name").unwrap().obj;
    doc.update_dict(r, |d| {
        d.set(b"T".to_vec(), Object::String(PdfString::text(name)));
        d.set(b"V".to_vec(), Object::String(PdfString::text(value)));
    })
    .unwrap();
    doc
}

/// A `/T` can hold periods of its own, so a field name can split into more parts than the field
/// tree has levels. The XFDF export nested one `<field>` per part, and the import refuses more
/// than 64 levels, so the form's own export could not be read back.
#[test]
fn field_names_with_more_parts_than_the_import_reads_still_round_trip() {
    for parts in [1, 2, 63, 64, 65, 66, 100, 1_000] {
        let name = (0..parts).map(|i| format!("p{i}")).collect::<Vec<_>>().join(".");
        let doc = one_field_named(&name, "v");
        assert_eq!(values(&doc), vec![(name.clone(), vec!["v".to_string()])], "{parts} parts");
        let xfdf = export_xfdf(&doc, false, true, "form.pdf");
        assert_eq!(data_values(xfdf.as_bytes()), Ok(vec![(name.clone(), vec!["v".to_string()])]), "{parts} parts");
        let mut back = one_field_named(&name, "old");
        let report = import(&mut back, xfdf.as_bytes()).unwrap();
        assert_eq!((report.fields, values(&back)), (1, vec![(name.clone(), vec!["v".to_string()])]), "{parts} parts");
    }
}

/// A name of a hundred thousand parts nested as deep, and writing it out recursed once per level
/// until the stack overflowed, which aborts the process.
#[test]
fn a_field_name_of_many_parts_exports_without_overflowing_the_stack() {
    let name = vec!["p"; 100_000].join(".");
    let want = vec![(name.clone(), vec!["v".to_string()])];
    let read = on_small_stack(move || data_values(export_xfdf(&one_field_named(&name, "v"), false, true, "form.pdf").as_bytes()));
    assert_eq!(read, Ok(want));
}

#[test]
fn duplicate_xfdf_annotation_flags_do_not_turn_on_unrelated_bits() {
    // Flags are a set of bit masks, not additive integers. A repeated "print" flag
    // must not carry into "nozoom", nor repeated "readonly" into "locked".
    for (flags, expected) in [("print, PRINT, nozoom", 12), ("readonly,readonly", 64), ("print,print,print", 4)] {
        let xfdf = format!(
            r#"<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots><square page="0" rect="10,10,50,50" name="flags" flags="{flags}" /></annots></xfdf>"#
        );
        let mut doc = blank();
        assert_eq!(import(&mut doc, xfdf.as_bytes()).unwrap().comments, 1);
        let page = pdfcraft_model::pages(&doc)[0].obj;
        let annotations = doc.get(page).as_dict().and_then(|p| p.get(b"Annots")).map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap();
        let square = annotations.iter().filter_map(|a| doc.resolve(a).as_dict().cloned()).find(|d| d.name(b"Subtype") == Some(b"Square")).unwrap();
        assert_eq!(square.get(b"F").and_then(Object::as_int), Some(expected), "flags={flags}");
    }
}

// XML comments and processing instructions are not field text (XML 1.0 §§2.5–2.6).
fn split_text_values() -> [(&'static str, &'static str); 5] {
    [
        ("Ada<!--editor note--> Lovelace", "Ada Lovelace"),
        ("<!--editor note-->Ada", "Ada"),
        ("Ada<?editor keep?> Lovelace", "Ada Lovelace"),
        (" Å<!--note--><![CDATA[<&>]]><?editor keep?>&#x03A9; ", " Å<&>Ω "),
        ("<!--empty--><?editor keep?>", ""),
    ]
}

#[test]
fn xfdf_data_values_join_text_around_xml_comments_and_instructions() {
    for (xml_text, expected) in split_text_values() {
        let xml = format!(r#"<xfdf xmlns="http://ns.adobe.com/xfdf/"><fields><field name="Name"><value>{xml_text}</value></field></fields></xfdf>"#);
        assert_eq!(data_values(xml.as_bytes()).unwrap(), vec![("Name".into(), vec![expected.to_string()])], "{xml_text}");
    }
}

#[test]
fn xfdf_import_keeps_text_around_xml_comments_and_instructions() {
    for (xml_text, expected) in split_text_values() {
        let xml = format!(r#"<xfdf xmlns="http://ns.adobe.com/xfdf/"><fields><field name="Name"><value>{xml_text}</value></field></fields></xfdf>"#);
        let mut doc = one_field();
        set_value(&mut doc, "Name", &FieldValue::Text("old value".into())).unwrap();
        let report = import(&mut doc, xml.as_bytes()).unwrap();
        assert_eq!(report.fields, 1);
        assert!(report.rejected.is_empty(), "{report:?}");
        let saved = write_full(&doc, &SaveOptions::default()).unwrap();
        let reopened = Document::open(Arc::new(saved)).unwrap();
        let want: Vec<&str> = if expected.is_empty() { vec![] } else { vec![expected] };
        assert_eq!(values(&reopened)[0].1, want, "{xml_text}");
    }
}

#[test]
fn fdf_export_keeps_indirect_values_nested_in_annotation_properties() {
    let mut src = blank();
    let page = pdfcraft_model::pages(&src)[0].obj;
    let mut annotation = Dict::new();
    annotation.set(b"Type".to_vec(), Object::name("Annot"));
    annotation.set(b"Subtype".to_vec(), Object::name("Square"));
    annotation.set(b"NM".to_vec(), PdfString::text("indirect-values"));
    let rect = [50.0, 60.0, 100.0, 120.0].map(|v| Object::Ref(src.add(Object::Real(v))));
    annotation.set(b"Rect".to_vec(), Object::Array(rect.into()));
    let color = [0.25, 0.5, 0.75].map(|v| Object::Ref(src.add(Object::Real(v))));
    annotation.set(b"C".to_vec(), Object::Array(color.into()));
    let mut border = Dict::new();
    border.set(b"W".to_vec(), Object::Ref(src.add(Object::Real(2.5))));
    border.set(b"S".to_vec(), Object::name("D"));
    let dash = [3.0, 2.0].map(|v| Object::Ref(src.add(Object::Real(v))));
    border.set(b"D".to_vec(), Object::Ref(src.add(Object::Array(dash.into()))));
    annotation.set(b"BS".to_vec(), Object::Ref(src.add(border)));
    let note = src.add(annotation);
    src.update_dict(page, |p| p.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(note)]))).unwrap();

    // This is an actual saved PDF: indirect scalar numbers, arrays and dictionaries are legal
    // PDF objects, and source object numbers have no meaning in an exported FDF.
    let src = Document::open(Arc::new(write_full(&src, &SaveOptions::default()).unwrap())).unwrap();
    assert_eq!(summaries(&src)[0].rect, [50.0, 60.0, 100.0, 120.0]);
    assert_eq!(summaries(&src)[0].color, Some([0.25, 0.5, 0.75]));
    let fdf = export_fdf(&src, true, false, "source.pdf").unwrap();
    let exported = Document::open(Arc::new(fdf.clone())).unwrap();
    let body = fdf_body(&exported).unwrap();
    let annots = exported.resolve(body.get(b"Annots").unwrap());
    let note = exported.resolve(&annots.as_array().unwrap()[0]);
    let d = note.as_dict().unwrap();
    assert_eq!(nums_of(&exported, d.get(b"Rect")), [50.0, 60.0, 100.0, 120.0], "{}", String::from_utf8_lossy(&fdf));
    assert_eq!(nums_of(&exported, d.get(b"C")), [0.25, 0.5, 0.75]);
    let bs = exported.resolve(d.get(b"BS").unwrap());
    let bs = bs.as_dict().unwrap();
    assert_eq!(exported.resolve(bs.get(b"W").unwrap()).as_f64(), Some(2.5));
    assert_eq!(nums_of(&exported, bs.get(b"D")), [3.0, 2.0]);

    let mut dst = blank();
    import(&mut dst, &fdf).unwrap();
    let dst = Document::open(Arc::new(write_full(&dst, &SaveOptions::default()).unwrap())).unwrap();
    let notes = summaries(&dst);
    assert_eq!(notes[0].rect, [50.0, 60.0, 100.0, 120.0]);
    assert_eq!(notes[0].color, Some([0.25, 0.5, 0.75]));
}

/// Put extension data on a real exported comment without changing its normal properties.
fn fdf_note_with_data(doc: &mut Document, data: Object) {
    let page = pdfcraft_model::pages(doc)[0].obj;
    let mut note = Dict::new();
    note.set(b"Type".to_vec(), Object::name("Annot"));
    note.set(b"Subtype".to_vec(), Object::name("Square"));
    note.set(b"Rect".to_vec(), Object::Array(vec![10.into(), 20.into(), 30.into(), 40.into()]));
    note.set(b"PCData".to_vec(), data);
    let note = doc.add(note);
    doc.update_dict(page, |dict| dict.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(note)]))).unwrap();
}

fn exported_note_data(doc: &Document) -> Object {
    let body = fdf_body(doc).unwrap();
    let annots = doc.resolve(body.get(b"Annots").unwrap());
    let note = doc.resolve(&annots.as_array().unwrap()[0]);
    note.as_dict().unwrap().get(b"PCData").unwrap().clone()
}

#[test]
fn fdf_export_preserves_shared_cycles_and_nested_streams_without_expansion() {
    let mut src = blank();
    let leaf = src.add(PdfString::text("shared"));
    let mut shared = leaf;
    for _ in 0..35 {
        shared = src.add(Object::Array(vec![Object::Ref(shared), Object::Ref(shared)]));
    }
    let cycle = src.add(Object::Null);
    src.set(cycle, Object::Array(vec![Object::Ref(cycle)]));
    let mut stream_dict = Dict::new();
    stream_dict.set(b"PCMeta".to_vec(), Object::Ref(leaf));
    let stream = src.add(pdfcraft_cos::Stream::from_raw(stream_dict, b"original bytes".to_vec()));
    let data = Object::Array(vec![Object::Ref(shared), Object::Ref(shared), Object::Ref(cycle), Object::Ref(stream)]);
    fdf_note_with_data(&mut src, data);
    let src = Document::open(Arc::new(write_full(&src, &SaveOptions::default()).unwrap())).unwrap();
    let bytes = export_fdf(&src, true, false, "source.pdf").unwrap();
    assert!(bytes.len() < 10_000, "a shared graph must not expand");
    let fdf = Document::open(Arc::new(bytes)).unwrap();
    let data = exported_note_data(&fdf);
    let data = data.as_array().unwrap();
    assert_eq!(data[0], data[1]);
    let mut current = data[0].clone();
    for _ in 0..35 {
        let next = fdf.resolve(&current);
        let values = next.as_array().unwrap();
        assert_eq!(values[0], values[1]);
        current = values[0].clone();
    }
    assert_eq!(fdf.resolve(&current).as_string().unwrap().to_text(), "shared");
    let cycle = fdf.resolve(&data[2]);
    assert_eq!(cycle.as_array().unwrap()[0], data[2]);
    let stream = fdf.resolve(&data[3]);
    let Object::Stream(stream) = &*stream else { panic!("copied stream") };
    assert_eq!(&*stream.raw, b"original bytes");
    assert_eq!(fdf.resolve(stream.dict.get(b"PCMeta").unwrap()).as_string().unwrap().to_text(), "shared");
}

#[test]
fn fdf_export_walks_long_indirect_chains_and_nulls_absent_or_stale_refs() {
    let mut src = blank();
    let live = src.add(PdfString::text("live"));
    let mut head = live;
    for _ in 0..3_000 {
        head = src.add(Object::Array(vec![Object::Ref(head)]));
    }
    fdf_note_with_data(&mut src, Object::Array(vec![Object::Ref(head), Object::Ref(ObjRef::new(999_999, 0)), Object::Ref(ObjRef::new(live.num, 1))]));
    let fdf = Document::open(Arc::new(export_fdf(&src, true, false, "source.pdf").unwrap())).unwrap();
    let data = exported_note_data(&fdf);
    let data = data.as_array().unwrap();
    assert_eq!(*fdf.resolve(&data[1]), Object::Null);
    assert_eq!(*fdf.resolve(&data[2]), Object::Null);
    let mut current = data[0].clone();
    for _ in 0..3_000 {
        current = fdf.resolve(&current).as_array().unwrap()[0].clone();
    }
    assert_eq!(fdf.resolve(&current).as_string().unwrap().to_text(), "live");
}

#[test]
fn fdf_export_refuses_excessive_direct_depth_values_and_payload() {
    let mut src = blank();
    let mut deep = Object::Null;
    for _ in 0..70 {
        deep = Object::Array(vec![deep]);
    }
    fdf_note_with_data(&mut src, deep);
    assert_eq!(export_fdf(&src, true, false, "source.pdf"), Err(DataError::ExportLimit("64 direct nesting levels")));
    // Not exporting comments must not inspect their extension data.
    assert!(export_fdf(&src, false, true, "source.pdf").is_ok());

    let mut src = blank();
    fdf_note_with_data(&mut src, Object::Array(vec![Object::Null; MAX_FDF_OBJECTS + 1]));
    assert_eq!(export_fdf(&src, true, false, "source.pdf"), Err(DataError::ExportLimit("100,000 values")));

    let mut src = blank();
    let stream = src.add(pdfcraft_cos::Stream::from_raw(Dict::new(), vec![0; MAX_FDF_BYTES + 1]));
    fdf_note_with_data(&mut src, Object::Ref(stream));
    assert_eq!(export_fdf(&src, true, false, "source.pdf"), Err(DataError::ExportLimit("64 MiB data")));

    let mut src = blank();
    // Strings need up to four serialized bytes per source byte. Refuse before copying
    // the direct payload, just as the indirect stream case above refuses its raw data.
    fdf_note_with_data(&mut src, Object::String(PdfString::literal(vec![b'x'; MAX_FDF_BYTES / 4 + 1])));
    assert_eq!(export_fdf(&src, true, false, "source.pdf"), Err(DataError::ExportLimit("64 MiB data")));
}

#[test]
fn fdf_export_refuses_aliases_before_cloning_large_annotation_values() {
    let mut src = blank();
    fdf_note_with_data(&mut src, Object::Array(vec![Object::Null; 1_024]));
    let page = pdfcraft_model::pages(&src)[0].obj;
    let annots = src.get(page).as_dict().unwrap().get(b"Annots").unwrap().clone();
    let note = annots.as_array().unwrap()[0].as_ref().unwrap();
    // The source holds one 1,024-value dictionary. The aliases must not clone its
    // values a hundred thousand times before the comment-count refusal is reached.
    src.update_dict(page, |dict| dict.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(note); MAX_FDF_OBJECTS + 1]))).unwrap();
    assert_eq!(export_fdf(&src, true, false, "source.pdf"), Err(DataError::ExportLimit("100,000 comments")));
}

#[test]
fn fdf_export_refuses_repeated_saved_annotations_after_cache_eviction() {
    let bytes = {
        let mut src = blank();
        let page = pdfcraft_model::pages(&src)[0].obj;
        let mut notes = Vec::new();
        // More than the editor cache's 1,024 objects. Four passes force reparsing;
        // retaining each resolved body during discovery would pin all four copies.
        for _ in 0..1_025 {
            let mut note = Dict::new();
            note.set(b"Type".to_vec(), Object::name("Annot"));
            note.set(b"Subtype".to_vec(), Object::name("Square"));
            note.set(b"Rect".to_vec(), Object::Array(vec![10.into(), 20.into(), 30.into(), 40.into()]));
            note.set(b"PCData".to_vec(), Object::Array(vec![Object::Null; 1_024]));
            notes.push(Object::Ref(src.add(note)));
        }
        let aliases = (0..4).flat_map(|_| notes.iter().cloned()).collect();
        src.update_dict(page, |dict| dict.set(b"Annots".to_vec(), Object::Array(aliases))).unwrap();
        write_full(&src, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap()
    };
    // Drop the overlay-backed source before reopening: its persistent Arc objects
    // would hide the parsed-object cache eviction exercised by this fixture.
    let src = Document::open(Arc::new(bytes)).unwrap();
    assert!(src.repair_log().is_empty());
    assert_eq!(export_fdf(&src, true, false, "source.pdf"), Err(DataError::ExportLimit("100,000 values")));
}

#[test]
fn fdf_export_counts_emitted_field_values_before_allocating_lists() {
    let field_document = |kind: &str, count: usize| {
        let mut doc = Document::new_empty();
        let mut field = Dict::new();
        field.set(b"FT".to_vec(), Object::name(kind));
        field.set(b"T".to_vec(), PdfString::text("Pick"));
        field.set(b"Ff".to_vec(), Object::Int(1 << 21)); // multi-select for a /Ch list
        field.set(b"V".to_vec(), Object::Array(vec![Object::String(PdfString::text("")); count]));
        let field = doc.add(field);
        let mut form = Dict::new();
        form.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(field)]));
        doc.update_dict(doc.root().unwrap(), |root| root.set(b"AcroForm".to_vec(), Object::Dict(form))).unwrap();
        assert_eq!(fields(&doc)[0].value.len(), count);
        doc
    };

    // A field dictionary, /T string and /V array use three nodes before its entries.
    let doc = field_document("Ch", MAX_FDF_OBJECTS - 3);
    assert_eq!(fields(&doc)[0].kind, FieldKind::List);
    let bytes = export_fdf(&doc, false, true, "field.pdf").unwrap();
    assert_eq!(data_values(&bytes).unwrap()[0].1.len(), MAX_FDF_OBJECTS - 3);
    for count in [MAX_FDF_OBJECTS - 2, MAX_FDF_OBJECTS + 1] {
        let doc = field_document("Ch", count);
        assert!(matches!(export_fdf(&doc, false, true, "field.pdf"), Err(DataError::ExportLimit("100,000 values"))));
    }

    // A hostile array on a scalar field still emits just its first string. The node
    // budget counts output values, rather than charging entries that are not copied.
    let doc = field_document("Tx", MAX_FDF_OBJECTS + 1);
    assert_eq!(fields(&doc)[0].kind, FieldKind::Text);
    let bytes = export_fdf(&doc, false, true, "field.pdf").unwrap();
    let fdf = Document::open(Arc::new(bytes)).unwrap();
    let body = fdf_body(&fdf).unwrap();
    let exported = fdf.resolve(body.get(b"Fields").unwrap());
    let exported = fdf.resolve(&exported.as_array().unwrap()[0]);
    assert!(matches!(exported.as_dict().unwrap().get(b"V"), Some(Object::String(_))));
}

#[test]
fn xml_form_import_preserves_text_around_comments_and_instructions() {
    for (body, expected) in [
        ("left<!-- ignored -->right", "leftright"),
        ("<!-- first -->left<?note ignored?>middle<!-- last -->right", "leftmiddleright"),
        ("<![CDATA[<left>]]><!-- ignored -->&amp;right", "<left>&right"),
        (" left <!-- ignored --> right ", " left  right "),
        ("<!-- empty --><?note ignored?>", ""),
    ] {
        let mut original = one_field();
        let root = original.root().unwrap();
        original.update_dict(root, |dict| dict.set(b"PCUnknown".to_vec(), PdfString::text("retained"))).unwrap();
        set_value(&mut original, "Name", &FieldValue::Text("replace me".into())).unwrap();
        let source = write_full(&original, &SaveOptions::default()).unwrap();
        let mut doc = Document::open(Arc::new(source.clone())).unwrap();
        let xml = format!("<?xml version=\"1.0\"?><fields><Name>{body}</Name></fields>");
        let report = import(&mut doc, xml.as_bytes()).unwrap();
        assert_eq!(report.fields, 1);
        assert!(report.rejected.is_empty());
        for saved in [write_full(&doc, &SaveOptions::default()).unwrap(), write_incremental(&doc, &SaveOptions::default()).unwrap()] {
            let reopened = Document::open(Arc::new(saved)).unwrap();
            let name = fields(&reopened).into_iter().find(|f| f.name == "Name").unwrap();
            let want = if expected.is_empty() { Vec::new() } else { vec![expected.to_string()] };
            assert_eq!(name.value, want, "XML body: {body}");
            let root = reopened.get(reopened.root().unwrap());
            assert_eq!(root.as_dict().unwrap().get(b"PCUnknown").unwrap().as_string().unwrap().to_text(), "retained");
        }
        assert_eq!(fields(&original).into_iter().find(|f| f.name == "Name").unwrap().value, vec!["replace me"]);
    }
}

#[test]
fn fdf_export_nulls_nested_page_links_without_copying_the_source_page_tree() {
    for indirect_type in [false, true] {
        let source = {
            let mut doc = Document::new_empty();
            let root = doc.root().unwrap();
            let tree = doc.get(root).as_dict().unwrap().reference(b"Pages").unwrap();
            let mut pages = Vec::new();
            for _ in 0..3 {
                let contents = doc.add(pdfcraft_cos::Stream::from_raw(
                    Dict::new(),
                    b"% source-page-content-must-stay-in-the-PDF\nq 0 g 0 0 10 10 re f Q\n".repeat(1_024),
                ));
                let mut page = Dict::new();
                let page_type = if indirect_type { Object::Ref(doc.add(Object::name("Page"))) } else { Object::name("Page") };
                page.set(b"Type".to_vec(), page_type);
                page.set(b"Parent".to_vec(), Object::Ref(tree));
                page.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 600.into(), 800.into()]));
                page.set(b"Contents".to_vec(), Object::Ref(contents));
                pages.push(doc.add(page));
            }
            let tree_type = if indirect_type { Object::Ref(doc.add(Object::name("Pages"))) } else { Object::name("Pages") };
            doc.update_dict(tree, |dict| {
                dict.set(b"Type".to_vec(), tree_type);
                dict.set(b"Count".to_vec(), Object::Int(3));
                dict.set(b"Kids".to_vec(), Object::Array(pages.iter().copied().map(Object::Ref).collect()));
            })
            .unwrap();
            let mut action = Dict::new();
            action.set(b"S".to_vec(), Object::name("GoTo"));
            action.set(b"D".to_vec(), Object::Array(vec![Object::Ref(pages[1]), Object::name("Fit")]));
            let mut extension = Dict::new();
            extension.set(b"Type".to_vec(), Object::name("PCMetadata"));
            extension.set(b"Target".to_vec(), Object::Ref(pages[1]));
            extension.set(b"Label".to_vec(), PdfString::text("retained metadata"));
            let extension = doc.add(extension);
            let type_cycle = doc.add(Object::Null);
            doc.set(type_cycle, Object::Ref(type_cycle));
            let mut cyclic_type = Dict::new();
            cyclic_type.set(b"Type".to_vec(), Object::Ref(type_cycle));
            cyclic_type.set(b"Label".to_vec(), PdfString::text("cyclic type is not a page"));
            fdf_note_with_data(
                &mut doc,
                Object::Array(vec![Object::Dict(action), Object::Ref(extension), Object::Array(vec![Object::Ref(tree)]), Object::Dict(cyclic_type)]),
            );
            write_full(&doc, &SaveOptions::default()).unwrap()
        };
        let source = Document::open(Arc::new(source)).unwrap();
        assert!(source.repair_log().is_empty());
        assert_eq!(pdfcraft_model::pages(&source).len(), 3);
        let bytes = export_fdf(&source, true, false, "source.pdf").unwrap();
        assert!(bytes.len() < 4_096, "a comment export must not carry source pages or their content ({} bytes)", bytes.len());
        let fdf = Document::open(Arc::new(bytes)).unwrap();
        let data = exported_note_data(&fdf);
        let data = data.as_array().unwrap();
        let action = data[0].as_dict().unwrap();
        assert_eq!(action.name(b"S"), Some(b"GoTo".as_slice()));
        let destination = action.get(b"D").unwrap().as_array().unwrap();
        assert_eq!(*fdf.resolve(&destination[0]), Object::Null);
        assert_eq!(destination[1], Object::name("Fit"));
        let extension = fdf.resolve(&data[1]);
        let extension = extension.as_dict().unwrap();
        assert_eq!(extension.name(b"Type"), Some(b"PCMetadata".as_slice()));
        assert_eq!(extension.get(b"Target"), Some(&destination[0]), "the excluded page reference remains aliased");
        assert_eq!(extension.get(b"Label").unwrap().as_string().unwrap().to_text(), "retained metadata");
        let tree = data[2].as_array().unwrap();
        assert_eq!(*fdf.resolve(&tree[0]), Object::Null);

        let cyclic_type = data[3].as_dict().unwrap();
        assert_eq!(cyclic_type.get(b"Label").unwrap().as_string().unwrap().to_text(), "cyclic type is not a page");
        let type_cycle = cyclic_type.get(b"Type").unwrap();
        assert_eq!(&*fdf.resolve(type_cycle), type_cycle);
    }
}

#[test]
fn fdf_export_bounds_indirect_type_alias_lookups() {
    for extra_aliases in [MAX_DEPTH - 1, MAX_DEPTH] {
        let mut doc = blank();
        let mut page_type = doc.add(Object::name("Page"));
        for _ in 0..extra_aliases {
            page_type = doc.add(Object::Ref(page_type));
        }
        let mut page = Dict::new();
        page.set(b"Type".to_vec(), Object::Ref(page_type));
        fdf_note_with_data(&mut doc, Object::Dict(page));
        let exported = export_fdf(&doc, true, false, "");
        if extra_aliases < MAX_DEPTH {
            let fdf = Document::open(Arc::new(exported.unwrap())).unwrap();
            assert_eq!(exported_note_data(&fdf), Object::Null);
        } else {
            assert!(matches!(exported, Err(DataError::ExportLimit("64 Type reference levels"))));
        }
    }
}

#[test]
fn fdf_export_preserves_metadata_with_stale_type_references() {
    for alias in [false, true] {
        let mut doc = blank();
        let current = doc.add(Object::name("Page"));
        doc.update_dict(doc.root().unwrap(), |dict| dict.set(b"PCPageType".to_vec(), Object::Ref(current))).unwrap();
        let mut doc = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
        let current = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"PCPageType").unwrap();
        let stale = ObjRef::new(current.num, current.generation + 1);
        let page_type = if alias { doc.add(Object::Ref(stale)) } else { stale };
        let mut metadata = Dict::new();
        metadata.set(b"Type".to_vec(), Object::Ref(page_type));
        metadata.set(b"Label".to_vec(), PdfString::text("preserved despite stale type"));
        fdf_note_with_data(&mut doc, Object::Dict(metadata));
        assert_eq!(doc.generation(stale.num), current.generation);
        // A warm number-keyed cache must not make the stale Type reference live.
        assert_eq!(*doc.try_get(stale.num).unwrap(), Object::name("Page"));
        let bytes = export_fdf(&doc, true, false, "").unwrap();
        let fdf = Document::open(Arc::new(bytes)).unwrap();
        let data = exported_note_data(&fdf);
        let metadata = data.as_dict().expect("stale Type must not classify unrelated metadata as a source page");
        assert_eq!(metadata.get(b"Label").unwrap().as_string().unwrap().to_text(), "preserved despite stale type");
        let mut page_type = fdf.resolve(metadata.get(b"Type").unwrap());
        if alias {
            page_type = fdf.resolve(&page_type);
        }
        assert_eq!(*page_type, Object::Null);
    }
}

#[test]
fn csv_headers_with_tabs_survive_export_import_and_save() {
    for name in ["First\tName", "\tName", "Name\t", "Name\t\"quoted\"", "Name\nwith\ta tab"] {
        let mut source = one_field();
        add_field(&mut source, 0, [50.0, 650.0, 250.0, 670.0], &NewField::Text { multiline: false }, Some("Other")).unwrap();
        set_value(&mut source, "Other", &FieldValue::Text("other field".into())).unwrap();
        let field = fields(&source).into_iter().find(|f| f.name == "Name").unwrap().obj;
        source.update_dict(field, |dict| dict.set(b"T".to_vec(), PdfString::text(name))).unwrap();
        set_value(&mut source, name, &FieldValue::Text("original, \"value\"".into())).unwrap();
        let bytes = write_full(&source, &SaveOptions::default()).unwrap();
        let source = Document::open(Arc::new(bytes.clone())).unwrap();
        let row = data_values(&bytes).unwrap();
        for csv in [export_data(&source, Format::Csv), merge_csv(&[row])] {
            let mut doc = Document::open(Arc::new(bytes.clone())).unwrap();
            set_value(&mut doc, name, &FieldValue::Text("replacement".into())).unwrap();
            set_value(&mut doc, "Other", &FieldValue::Text("other replacement".into())).unwrap();
            let report = import(&mut doc, csv.as_bytes()).unwrap();
            assert_eq!(report.fields, 2);
            assert!(report.rejected.is_empty(), "{:?}", report.rejected);
            for saved in [write_full(&doc, &SaveOptions::default()).unwrap(), write_incremental(&doc, &SaveOptions::default()).unwrap()] {
                let reopened = Document::open(Arc::new(saved)).unwrap();
                assert_eq!(fields(&reopened).into_iter().find(|f| f.name == name).unwrap().value, vec!["original, \"value\""], "{csv:?}");
                assert_eq!(values(&reopened), values(&source), "{csv:?}");
            }
        }
    }
}

#[test]
fn tabular_import_keeps_tsv_quotes_literal_and_csv_value_tabs() {
    for name in ["Name", "Na\"me", "\"Name\"", "\"Name"] {
        let mut doc = one_field();
        add_field(&mut doc, 0, [50.0, 650.0, 250.0, 670.0], &NewField::Text { multiline: false }, Some("Other")).unwrap();
        let field = fields(&doc).into_iter().find(|f| f.name == "Name").unwrap().obj;
        doc.update_dict(field, |dict| dict.set(b"T".to_vec(), PdfString::text(name))).unwrap();
        let text = format!("{name}\tOther\r\nAda\tother field\r\n");
        let report = import(&mut doc, text.as_bytes()).unwrap();
        assert_eq!(report.fields, 2, "{text:?}");
        assert_eq!(fields(&doc).into_iter().find(|f| f.name == name).unwrap().value, vec!["Ada"]);
    }
    let mut doc = one_field();
    add_field(&mut doc, 0, [50.0, 650.0, 250.0, 670.0], &NewField::Text { multiline: false }, Some("Other")).unwrap();
    import(&mut doc, b"Name,Other\r\n\"Ada\tLovelace\",other field\r\n").unwrap();
    assert_eq!(fields(&doc).into_iter().find(|f| f.name == "Name").unwrap().value, vec!["Ada\tLovelace"]);
}

#[test]
fn a_same_named_comment_import_never_replaces_a_form_widget() {
    let mut doc = blank();
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let widget = doc
        .resolve(page.dict.get(b"Annots").unwrap())
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Object::as_ref)
        .find(|r| doc.get(*r).as_dict().is_some_and(|d| d.name(b"Subtype") == Some(b"Widget".as_slice())))
        .unwrap();
    doc.update_dict(widget, |d| d.set(b"NM".to_vec(), PdfString::text("w1"))).unwrap();
    let before = doc.get(widget);
    let fdf = b"%FDF-1.2\n1 0 obj\n<< /FDF << /Annots [<< /Subtype /Text /NM (w1) /Page 0 /Rect [0 0 1 1] /FT /Tx /T (Name) /V (forged) >>] >> >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n";
    assert_eq!(import(&mut doc, fdf).unwrap().comments, 1);
    assert_eq!(doc.get(widget), before, "the widget was overwritten by an imported comment");
}

#[test]
fn an_imported_rect_only_or_transparent_redaction_mark_is_drawn_for_review() {
    let xfdf = br#"<xfdf><annots><redact page="0" name="r1" rect="60,600,560,620" opacity="0"/></annots></xfdf>"#;
    let fdf = b"%FDF-1.2\n1 0 obj\n<< /FDF << /Annots [<< /Subtype /Redact /NM (r1) /Page 0 /Rect [60 600 560 620] /CA 0 /ca 0 >>] >> >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n";
    for data in [xfdf.as_slice(), fdf.as_slice()] {
        let mut doc = blank();
        assert_eq!(import(&mut doc, data).unwrap().comments, 1);
        let page = pdfcraft_model::pages(&doc)[0].clone();
        let mark = doc
            .resolve(page.dict.get(b"Annots").unwrap())
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|a| a.as_ref().and_then(|r| doc.get(r).as_dict().cloned()))
            .find(|d| d.name(b"Subtype") == Some(b"Redact".as_slice()))
            .unwrap();
        assert_eq!(nums_of(&doc, mark.get(b"QuadPoints")), vec![60.0, 620.0, 560.0, 620.0, 60.0, 600.0, 560.0, 600.0]);
        let ap = mark.get(b"AP").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned());
        assert!(ap.is_some_and(|ap| ap.get(b"N").is_some()), "the mark has no review appearance: {mark:?}");
        assert!(mark.get(b"CA").is_none() && mark.get(b"ca").is_none(), "the mark can be drawn transparent: {mark:?}");
    }
}

#[test]
fn merged_spreadsheet_cells_never_start_a_formula() {
    let row = vec![
        ("=HYPERLINK(\"http://x\")".to_string(), vec!["@SUM(A1)".to_string()]),
        ("Total".to_string(), vec!["-2+3".to_string()]),
        ("Plus".to_string(), vec!["\t+1+cmd".to_string()]),
        ("Balance".to_string(), vec!["-12.5".to_string()]),
        ("\tName".to_string(), vec!["Ada".to_string()]),
    ];
    let csv = merge_csv(&[row]);
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines[0], "\"'=HYPERLINK(\"\"http://x\"\")\",Total,Plus,Balance,\"\tName\"");
    assert_eq!(lines[1], "'@SUM(A1),'-2+3,\"'\t+1+cmd\",-12.5,Ada");
}

fn imported_redact_mark(data: &[u8]) -> Dict {
    let mut doc = blank();
    assert_eq!(import(&mut doc, data).unwrap().comments, 1);
    let page = pdfcraft_model::pages(&doc)[0].clone();
    doc.resolve(page.dict.get(b"Annots").unwrap())
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a.as_ref().and_then(|r| doc.get(r).as_dict().cloned()))
        .find(|d| d.name(b"Subtype") == Some(b"Redact".as_slice()))
        .unwrap()
}

#[test]
fn an_imported_redaction_mark_with_zero_area_quads_falls_back_to_its_rect() {
    let rect_quads = vec![60.0, 620.0, 560.0, 620.0, 60.0, 600.0, 560.0, 600.0];
    let fdf = |quads: &str| {
        format!(
            "%FDF-1.2\n1 0 obj\n<< /FDF << /Annots [<< /Subtype /Redact /NM (r1) /Page 0 /Rect [60 600 560 620] /QuadPoints [{quads}] >>] >> >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n"
        )
    };
    let cases = [
        fdf("100 100 100 100 100 100 100 100").into_bytes(),
        // A good quad beside one that is a line (all four corners collinear).
        fdf("60 620 560 620 60 600 560 600 10 10 20 20 30 30 40 40").into_bytes(),
        br#"<xfdf><annots><redact page="0" name="r1" rect="60,600,560,620" coords="5,5,5,5,5,5,5,5"/></annots></xfdf>"#.to_vec(),
    ];
    for data in cases {
        let mark = imported_redact_mark(&data);
        let quads: Vec<f64> = mark.get(b"QuadPoints").and_then(Object::as_array).unwrap().iter().filter_map(Object::as_f64).collect();
        assert_eq!(quads, rect_quads, "zero-area quads were kept: {mark:?}");
    }
}

#[test]
fn an_imported_redaction_rect_with_a_non_number_is_not_used() {
    let fdf = b"%FDF-1.2\n1 0 obj\n<< /FDF << /Annots [<< /Subtype /Redact /NM (r1) /Page 0 /Rect [0 0 /Bad 100 100] >>] >> >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n";
    let mark = imported_redact_mark(fdf);
    assert!(mark.get(b"QuadPoints").is_none(), "quads were drawn from a malformed /Rect: {mark:?}");
}

#[test]
fn a_colour_with_non_hex_digits_is_rejected() {
    assert!(parse_hex("#+f+f+f").is_none());
    assert!(parse_hex("#+1+2+3").is_none());
    assert!(parse_hex("#-1-2-3").is_none());
    assert!(parse_hex("#ff00zz").is_none());
    assert_eq!(parse_hex("#FF8000").map(|c| c[0]), Some(1.0));
}

/// An FDF body with `body` as its `/FDF` dictionary contents and `objects` (numbered from 2).
fn fdf_with(body: &str, objects: &[String]) -> Vec<u8> {
    let mut out = format!("%FDF-1.2\n1 0 obj\n<< /FDF << {body} >> >>\nendobj\n");
    for (i, o) in objects.iter().enumerate() {
        out.push_str(&format!("{} 0 obj\n{o}\nendobj\n", i + 2));
    }
    out.push_str("trailer\n<< /Root 1 0 R >>\n%%EOF\n");
    out.into_bytes()
}

/// A field that lists itself as a kid is read once.
#[test]
fn an_fdf_field_listing_itself_is_read_once() {
    let cyclic = fdf_with("/Fields [2 0 R]", &["<< /T (a) /V (x) /Kids [2 0 R] >>".into()]);
    assert_eq!(data_values(&cyclic).unwrap(), vec![("a".to_string(), vec!["x".to_string()])]);
}

/// A field tree whose `/Kids` repeat one subtree level after level is refused rather than
/// expanded into millions of rows.
#[test]
fn fdf_field_trees_that_repeat_references_are_bounded() {
    let levels = 19;
    let mut objects: Vec<String> = (0..levels).map(|i| format!("<< /T (f{i}) /V (x) /Kids [{n} 0 R {n} 0 R] >>", n = i + 3)).collect();
    objects.push("<< /T (leaf) /V (x) >>".into());
    let shared = fdf_with("/Fields [2 0 R]", &objects);
    let read = data_values(&shared);
    assert!(read.is_err(), "{} rows", read.map_or(0, |v| v.len()));
    assert!(matches!(import(&mut blank(), &shared), Err(DataError::Malformed(_))));
}

fn fdf_note(data: &str) -> String {
    format!("<< /Subtype /Text /Rect [0 0 9 9] /NM (n) /Page 0 /PCData {data} >>")
}

/// A comment property that refers back to the comment is copied once, not nested level after
/// level.
#[test]
fn an_fdf_comment_referring_to_itself_is_copied_once() {
    let cyclic = fdf_with("/Annots [2 0 R]", &[fdf_note("2 0 R")]);
    let mut dst = blank();
    assert_eq!(import(&mut dst, &cyclic).unwrap().comments, 1);
    let placed = find_by_name(&dst, "n").unwrap();
    let data = dst.get(placed).as_dict().unwrap().get(b"PCData").cloned();
    assert!(!matches!(data, Some(Object::Dict(_))), "{data:?}");
}

/// A comment whose properties repeat one reference level after level is refused rather than
/// copied into millions of values.
#[test]
fn fdf_comment_graphs_that_repeat_references_are_bounded() {
    let note = fdf_note;
    let width = 6;
    let mut objects = vec![note("3 0 R")];
    for i in 0..8 {
        objects.push(format!("[{}]", vec![format!("{} 0 R", i + 4); width].join(" ")));
    }
    objects.push("(leaf)".into());
    let shared = fdf_with("/Annots [2 0 R]", &objects);
    assert!(matches!(import(&mut blank(), &shared), Err(DataError::Malformed(_))));
}

/// Line-ending names are attribute values: a name holding a quote or angle bracket cannot add
/// attributes or elements to the exported XFDF.
#[test]
fn xfdf_line_ending_names_are_escaped() {
    let mut doc = blank();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let hostile = b"a\" tail=\"x\"><field name=\"Name\"><value>pwned</value></field><b".to_vec();
    let mut line = Dict::new();
    line.set(b"Type".to_vec(), Object::name("Annot"));
    line.set(b"Subtype".to_vec(), Object::name("Line"));
    line.set(b"Rect".to_vec(), Object::Array(vec![10.into(), 20.into(), 30.into(), 40.into()]));
    line.set(b"L".to_vec(), Object::Array(vec![10.into(), 20.into(), 30.into(), 40.into()]));
    line.set(b"LE".to_vec(), Object::Array(vec![Object::Name(hostile.clone()), Object::Name(hostile.clone())]));
    let line = doc.add(line);
    doc.update_dict(page, |d| d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(line)]))).unwrap();
    let xml = export_xfdf(&doc, true, false, "form.pdf");
    let parsed = roxmltree::Document::parse(&xml).unwrap();
    assert!(!parsed.descendants().any(|n| n.has_tag_name("field")), "{xml}");
    let el = parsed.descendants().find(|n| n.has_tag_name("line")).unwrap();
    let want = String::from_utf8(hostile).unwrap();
    assert_eq!((el.attribute("head"), el.attribute("tail")), (Some(want.as_str()), Some(want.as_str())));
}

/// A comment locked through an indirect `/F` is as protected from a same-named import as one
/// with the flag written inline.
#[test]
fn a_comment_locked_by_an_indirect_flag_is_not_replaced_by_an_import() {
    let mut doc = blank();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let flag = doc.add(Object::Int(128)); // Locked
    let mut note = Dict::new();
    note.set(b"Type".to_vec(), Object::name("Annot"));
    note.set(b"Subtype".to_vec(), Object::name("Text"));
    note.set(b"NM".to_vec(), PdfString::text("n1"));
    note.set(b"Rect".to_vec(), Object::Array(vec![10.into(), 10.into(), 30.into(), 30.into()]));
    note.set(b"F".to_vec(), Object::Ref(flag));
    let note = doc.add(Object::Dict(note));
    doc.update_dict(page, |p| p.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(note)]))).unwrap();
    let before = doc.get(note);
    let xfdf = br#"<xfdf><annots><text page="0" name="n1" rect="0,0,1,1"/></annots></xfdf>"#;
    assert_eq!(import(&mut doc, xfdf).unwrap().comments, 1);
    let annots = doc.get(page).as_dict().and_then(|p| p.get(b"Annots").and_then(Object::as_array).cloned()).unwrap();
    assert!(annots.contains(&Object::Ref(note)), "the locked comment was replaced");
    assert_eq!(doc.get(note), before, "the locked comment was changed");
}

#[test]
fn import_skips_annotations_with_non_finite_coordinates() {
    for rect in ["nan,10,100,100", "0,inf,100,100", "0,10,infinity,100", "0,10,100,-inf"] {
        let mut doc = blank();
        let xfdf = format!(
            r#"<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots><square page="0" rect="0,0,50,50" name="good"/><square page="0" rect="{rect}" name="bad"/></annots></xfdf>"#
        );
        let r = import(&mut doc, xfdf.as_bytes()).unwrap();
        assert_eq!(r.comments, 1, "rect {rect}: {r:?}");
        let names = annots_of(&doc);
        assert_eq!(names.len(), 1, "rect {rect}: {names:?}");
        assert_eq!(names[0].get(b"NM"), Some(&PdfString::literal(b"good".to_vec()).into()), "rect {rect}");
    }
}

#[test]
fn import_drops_non_finite_numbers_from_every_coordinate_list() {
    let xfdf = r#"<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots>
        <line page="0" rect="0,0,50,50" name="line" start="nan,0" end="10,10"/>
        <polygon page="0" rect="0,0,50,50" name="poly"><vertices>1,2;inf,4;5,6</vertices></polygon>
        <ink page="0" rect="0,0,50,50" name="ink"><inklist><gesture>1,2;3,-inf</gesture></inklist></ink>
        <freetext page="0" rect="0,0,50,50" name="callout" callout="1,2,nan,4,5,6"/>
        <highlight page="0" rect="0,0,50,50" name="hl" coords="0,0,infinity,0,0,10,10,10"/>
        </annots></xfdf>"#;
    let mut doc = blank();
    let r = import(&mut doc, xfdf.as_bytes()).unwrap();
    assert_eq!(r.comments, 5, "{r:?}");
    fn finite(doc: &Document, o: &Object) -> bool {
        match &*doc.resolve(o) {
            Object::Real(x) => x.is_finite(),
            Object::Array(a) => a.iter().all(|e| finite(doc, e)),
            _ => true,
        }
    }
    let annots = annots_of(&doc);
    assert_eq!(annots.len(), 5);
    for d in &annots {
        for (k, v) in d.iter() {
            if k.as_slice() != b"P" {
                assert!(finite(&doc, v), "{d:?}");
            }
        }
        assert!(d.get(b"L").is_none() && d.get(b"CL").is_none() && d.get(b"Vertices").is_none(), "{d:?}");
    }
}

fn annots_of(doc: &Document) -> Vec<Dict> {
    let page = &pdfcraft_model::pages(doc)[0];
    page.dict
        .get(b"Annots")
        .map(|a| doc.resolve(a))
        .and_then(|a| a.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|a| doc.resolve(a).as_dict().cloned().unwrap())
        .filter(|d| d.get(b"Subtype") != Some(&Object::name("Widget")))
        .collect()
}
