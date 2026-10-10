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
