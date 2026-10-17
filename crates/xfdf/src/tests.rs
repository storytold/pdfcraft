use std::sync::Arc;

use pdfcraft_annot::{Markup, Meta, NewAnnotation, Shape, Style, add_annotation, add_reply, summaries};
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
fn fdf_round_trips_values_and_comments() {
    let src = filled();
    let fdf = export_fdf(&src, true, true, "form.pdf");
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
    let fdf = export_fdf(&src, false, true, "form.pdf");
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

    let fdf = export_fdf(&src, true, false, "notes.pdf");
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

/// FDF field import walked `/Kids` references with a depth cap but no visited set, so a `/Kids`
/// array that repeats one next-level reference drove branch^levels work: a ~1 KB file hung the
/// import thread. The walk now skips a reference it has already visited, so a crafted aliased tree
/// is read in time proportional to its distinct nodes.
#[test]
fn fdf_import_bounds_aliased_kids() {
    let levels = 24;
    let mut fdf = String::from("%FDF-1.2\n1 0 obj\n<< /FDF << /Fields [2 0 R] >> >>\nendobj\n");
    for i in 0..levels {
        let obj = 2 + i;
        // Each level lists the next level twice; no /V, so the walk is pure traversal (as the
        // audit measured): on the old code branch^levels, now linear in the distinct nodes.
        if i + 1 < levels {
            let next = obj + 1;
            fdf.push_str(&format!("{obj} 0 obj\n<< /T (a) /Kids [{next} 0 R {next} 0 R] >>\nendobj\n"));
        } else {
            fdf.push_str(&format!("{obj} 0 obj\n<< /T (leaf) >>\nendobj\n"));
        }
    }
    fdf.push_str("trailer\n<< /Root 1 0 R >>\n%%EOF\n");
    let start = std::time::Instant::now();
    let values = data_values(fdf.as_bytes()).unwrap();
    let elapsed = start.elapsed();
    assert!(values.is_empty(), "no field carries a value");
    assert!(elapsed.as_millis() < 500, "data_values took {elapsed:?}");

    // A non-aliased tree with a value still reads it, so the dedup did not break ordinary import.
    let ok = b"%FDF-1.2\n1 0 obj\n<< /FDF << /Fields [2 0 R] >> >>\nendobj\n\
2 0 obj\n<< /T (parent) /Kids [3 0 R] >>\nendobj\n\
3 0 obj\n<< /T (child) /V (hello) >>\nendobj\n\
trailer\n<< /Root 1 0 R >>\n%%EOF\n";
    assert_eq!(data_values(ok).unwrap(), vec![("parent.child".to_string(), vec!["hello".to_string()])]);
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
    let fdf = export_fdf(&src, false, true, "form.pdf");
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
    for data in [xfdf.as_bytes().to_vec(), export_fdf(&incoming, true, false, "notes.pdf")] {
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
            let fdf = export_fdf(&reopened, true, false, "notes.pdf");
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
    for data in [update.to_vec(), export_fdf(&incoming, true, false, "notes.pdf")] {
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
    for data in [export_xfdf(&incoming, true, false, "notes.pdf").into_bytes(), export_fdf(&incoming, true, false, "notes.pdf")] {
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
