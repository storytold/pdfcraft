use std::sync::Arc;

use pdfcraft_annot::{Markup, Meta, NewAnnotation, Shape, Style, add_annotation, add_reply, summaries};
use pdfcraft_cos::{SaveOptions, write_full};
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
