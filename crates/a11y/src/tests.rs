use std::sync::Arc;

use pdfcraft_cos::{Document, Object};

use super::*;

/// A PDF from object bodies (object n = bodies[n - 1]) and extra trailer entries.
fn pdf(bodies: &[String], trailer: &str) -> Document {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, b) in bodies.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{b}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R {trailer} >>\nstartxref\n{xref}\n%%EOF\n", bodies.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

fn stream(data: &str) -> String {
    format!("<< /Length {} >>\nstream\n{data}\nendstream", data.len())
}

fn status(r: &Report, rule: Rule) -> Status {
    r.result(rule).unwrap().status
}

fn failed(r: &Report) -> Vec<Rule> {
    r.results.iter().filter(|x| x.status == Status::Failed).map(|x| x.rule).collect()
}

const FONT: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";

#[test]
fn autotag_text_writes_marked_content_and_parent_tree_across_pages() {
    let mut doc = pdf(
        &[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
            stream("BT /F1 18 Tf 20 160 Td (Heading) Tj ET BT /F1 12 Tf 20 120 Td (Body) Tj ET"),
            FONT.into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 7 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
            stream("BT /F1 12 Tf 20 120 Td (Second page) Tj ET"),
        ],
        "",
    );
    let result = autotag_text(&mut doc).unwrap();
    assert_eq!(result, AutoTagResult { pages: 2, paragraphs: 3 });
    let catalog = doc.get(doc.root().unwrap());
    let root = catalog.as_dict().unwrap().reference(b"StructTreeRoot").unwrap();
    let tree = doc.get(root);
    let parent_tree = tree.as_dict().unwrap().reference(b"ParentTree").unwrap();
    let numbers = doc.get(parent_tree);
    let entries = numbers.as_dict().unwrap().get(b"Nums").unwrap().as_array().unwrap();
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[0].as_int(), Some(0));
    assert_eq!(entries[2].as_int(), Some(1));
    assert_eq!(entries[1].as_array().unwrap().len(), 2);
    assert_eq!(entries[3].as_array().unwrap().len(), 1);
    for page in pdfcraft_model::pages(&doc) {
        let content = page.dict.get(b"Contents").unwrap();
        let stream = doc.resolve(content);
        let Object::Stream(stream) = &*stream else { panic!("marked page content") };
        let text = String::from_utf8(stream.decoded_strict().unwrap()).unwrap();
        assert!(text.contains("/P <</MCID 0>> BDC"), "{text}");
        assert!(text.contains("EMC"), "{text}");
    }
    let report = check(&doc, &Options::default());
    assert_eq!(status(&report, Rule::TaggedPdf), Status::Passed);
    assert_eq!(status(&report, Rule::TaggedContent), Status::Passed);
    let bytes = pdfcraft_cos::write_incremental(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
    let reopened = Document::open(Arc::new(bytes)).unwrap();
    assert_eq!(tags(&reopened).len(), 4);
    assert_eq!(status(&check(&reopened, &Options::default()), Rule::TaggedContent), Status::Passed);
}

#[test]
fn autotag_text_refuses_paint_and_does_not_modify_the_document() {
    let mut doc = pdf(
        &[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
            stream("BT /F1 12 Tf 20 100 Td (Hello) Tj ET 0 0 50 50 re f"),
            FONT.into(),
        ],
        "",
    );
    assert!(matches!(autotag_text(&mut doc), Err(AutoTagError::Unsupported { .. })));
    assert!(!doc.get(doc.root().unwrap()).as_dict().unwrap().contains(b"StructTreeRoot"));
    assert_eq!(autotag_text(&mut pdf(&good(), "")), Err(AutoTagError::AlreadyTagged));
}

#[test]
fn autotag_form_fields_writes_widget_structure_and_parent_tree() {
    let mut doc = pdf(
        &[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [5 0 R 6 0 R] >>".into(),
            "<< /Length 0 >>\nstream\n\nendstream".into(),
            "<< /FT /Tx /T (name) /Kids [6 0 R] >>".into(),
            "<< /Type /Annot /Subtype /Widget /Parent 5 0 R /Rect [20 60 120 80] >>".into(),
        ],
        "",
    );
    assert_eq!(tag_form_fields(&mut doc).unwrap(), 1);
    let report = check(&doc, &Options { rules: [Rule::TaggedPdf, Rule::TaggedFormFields].into(), pages: None });
    assert_eq!(status(&report, Rule::TaggedPdf), Status::Passed);
    assert_eq!(status(&report, Rule::TaggedFormFields), Status::Passed);
    let tree = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"StructTreeRoot").unwrap();
    assert!(doc.get(tree).as_dict().unwrap().contains(b"ParentTree"));
    let widget = pdfcraft_model::pages(&doc)[0].dict.get(b"Annots").and_then(|a| doc.resolve(a).as_array().cloned()).unwrap()[1].as_ref().unwrap();
    assert_eq!(doc.get(widget).as_dict().unwrap().get(b"StructParent").and_then(Object::as_int), Some(0));
    let written = pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
    let reopened = Document::open(Arc::new(written)).unwrap();
    assert_eq!(status(&check(&reopened, &Options { rules: [Rule::TaggedFormFields].into(), pages: None }), Rule::TaggedFormFields), Status::Passed);
}

#[test]
fn autotag_form_fields_tags_a_widget_listed_on_two_pages_once() {
    let mut doc = pdf(
        &[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>".into(),
            "<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [5 0 R] >>".into(),
            "<< /Length 0 >>\nstream\n\nendstream".into(),
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [20 60 120 80] >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [5 0 R] >>".into(),
        ],
        "",
    );
    assert_eq!(tag_form_fields(&mut doc).unwrap(), 1);
    let tree = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"StructTreeRoot").unwrap();
    let parent_tree = doc.get(tree).as_dict().unwrap().reference(b"ParentTree").unwrap();
    let nums = doc.get(parent_tree).as_dict().unwrap().get(b"Nums").and_then(|n| n.as_array().cloned()).unwrap();
    assert_eq!(nums.len(), 2, "one key and one /Form element for the shared widget");
}

#[test]
fn autotag_form_fields_extends_a_direct_existing_structure_tree() {
    let mut doc = pdf(
        &[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> /MarkInfo << /Marked true >> /StructTreeRoot 6 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [5 0 R] /StructParents 0 >>".into(),
            "<< /Length 0 >>\nstream\n\nendstream".into(),
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [20 60 120 80] >>".into(),
            "<< /Type /StructTreeRoot /K 7 0 R /ParentTree 8 0 R >>".into(),
            "<< /S /Document /P 6 0 R /K 9 0 R >>".into(),
            "<< /Nums [0 [9 0 R]] >>".into(),
            "<< /S /P /P 7 0 R /Pg 3 0 R /K 0 >>".into(),
        ],
        "",
    );
    assert_eq!(tag_form_fields(&mut doc).unwrap(), 1);
    let widget = pdfcraft_model::pages(&doc)[0].dict.get(b"Annots").and_then(|a| doc.resolve(a).as_array().cloned()).unwrap()[0].as_ref().unwrap();
    assert_eq!(doc.get(widget).as_dict().unwrap().get(b"StructParent").and_then(Object::as_int), Some(1));
    let report = check(&doc, &Options { rules: [Rule::TaggedFormFields].into(), pages: None });
    assert_eq!(status(&report, Rule::TaggedFormFields), Status::Passed);
    let document_object = doc.get(pdfcraft_cos::ObjRef::new(7, 0));
    let document = document_object.as_dict().unwrap();
    assert_eq!(document.get(b"K").and_then(Object::as_array).unwrap().len(), 2, "the existing paragraph and new form remain siblings");
}

#[test]
fn an_untagged_document_fails_the_document_rules() {
    let doc = pdf(
        &[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
            stream("BT /F1 12 Tf 20 100 Td (Hello) Tj ET 0 0 50 50 re f"),
            FONT.into(),
        ],
        "",
    );
    let r = check(&doc, &Options::default());
    assert_eq!(r.results.len(), 32);
    assert_eq!(failed(&r), [Rule::TaggedPdf, Rule::PrimaryLanguage, Rule::Title, Rule::TaggedContent]);
    assert_eq!(status(&r, Rule::ColorContrast), Status::Skipped, "off by default");
    assert_eq!(status(&r, Rule::LogicalReadingOrder), Status::Manual);
    assert_eq!(status(&r, Rule::ImageOnly), Status::Passed);
    assert_eq!(r.result(Rule::TaggedPdf).unwrap().findings.len(), 2);
    // Only the chosen rules run.
    let only = check(&doc, &Options { rules: [Rule::Title].into(), pages: None });
    assert_eq!((only.count(Status::Failed), only.count(Status::Skipped)), (1, 31));
}

/// `/MarkInfo /Marked` may be written as an indirect reference, like any value. The rule read the
/// reference itself and so reported a tagged document as untagged (#3671).
#[test]
fn marked_as_an_indirect_reference_still_counts_as_tagged() {
    let mut bodies = good();
    // The catalog is object 1; point /Marked at a new object holding `true`.
    let last = bodies.len() + 1;
    bodies[0] = bodies[0].replace("/MarkInfo << /Marked true >>", &format!("/MarkInfo << /Marked {last} 0 R >>"));
    bodies.push("true".into());
    let doc = pdf(&bodies, "");
    let report = check(&doc, &Options { rules: [Rule::TaggedPdf].into(), pages: None });
    assert_eq!(status(&report, Rule::TaggedPdf), Status::Passed, "{:?}", failed(&report));

    // A reference to a reference is followed too: the render inspector follows chains, so the
    // two readers of the flag would otherwise disagree about one file.
    let mut chain = good();
    let first = chain.len() + 1;
    chain[0] = chain[0].replace("/MarkInfo << /Marked true >>", &format!("/MarkInfo << /Marked {first} 0 R >>"));
    chain.push(format!("{} 0 R", first + 1));
    chain.push("true".into());
    let report = check(&pdf(&chain, ""), &Options { rules: [Rule::TaggedPdf].into(), pages: None });
    assert_eq!(status(&report, Rule::TaggedPdf), Status::Passed, "{:?}", failed(&report));

    // A reference that points at itself is no value, not a hang.
    let mut loops = good();
    let at = loops.len() + 1;
    loops[0] = loops[0].replace("/MarkInfo << /Marked true >>", &format!("/MarkInfo << /Marked {at} 0 R >>"));
    loops.push(format!("{at} 0 R"));
    let report = check(&pdf(&loops, ""), &Options { rules: [Rule::TaggedPdf].into(), pages: None });
    assert_eq!(status(&report, Rule::TaggedPdf), Status::Failed);

    // The direct form is unchanged, and a document with no flag at all still fails.
    assert_eq!(status(&check(&pdf(&good(), ""), &Options { rules: [Rule::TaggedPdf].into(), pages: None }), Rule::TaggedPdf), Status::Passed);
    let mut none = good();
    none[0] = none[0].replace("/MarkInfo << /Marked true >>", "");
    assert_eq!(status(&check(&pdf(&none, ""), &Options { rules: [Rule::TaggedPdf].into(), pages: None }), Rule::TaggedPdf), Status::Failed);
}

/// A tagged document that passes every automatic rule.
fn good() -> Vec<String> {
    vec![
        // 1 catalog, 2 pages, 3 page, 4 content, 5 font
        "<< /Type /Catalog /Pages 2 0 R /Lang (en-US) /MarkInfo << /Marked true >> /StructTreeRoot 6 0 R /ViewerPreferences << /DisplayDocTitle true >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Tabs /S /Annots [9 0 R 10 0 R] /StructParents 0 \
         /Resources << /Font << /F1 5 0 R >> /XObject << /Im 11 0 R >> >> >>"
            .into(),
        stream(
            "/H1 << /MCID 0 >> BDC BT /F1 12 Tf 20 180 Td (Title) Tj ET EMC \
             /Figure << /MCID 1 >> BDC q 10 0 0 10 20 120 cm /Im Do Q EMC \
             /Artifact BMC 0 0 200 2 re f EMC \
             /TH << /MCID 2 >> BDC BT (A) Tj ET EMC /TH << /MCID 3 >> BDC BT (B) Tj ET EMC \
             /TD << /MCID 4 >> BDC BT (1) Tj ET EMC /TD << /MCID 5 >> BDC BT (2) Tj ET EMC \
             /Lbl << /MCID 6 >> BDC BT (1.) Tj ET EMC /LBody << /MCID 7 >> BDC BT (Item) Tj ET EMC",
        ),
        FONT.into(),
        // 6 struct tree root, 7 Document element, 8 info
        "<< /Type /StructTreeRoot /K 7 0 R /RoleMap << /Heading1 /H1 >> >>".into(),
        "<< /S /Document /P 6 0 R /Pg 3 0 R /K [\
         << /S /Heading1 /Pg 3 0 R /K 0 >> \
         << /S /Figure /Pg 3 0 R /Alt (A red square) /K 1 >> \
         << /S /Table /Pg 3 0 R /A << /O /Table /Summary (Two columns) >> /K [\
            << /S /TR /K [ << /S /TH /K 2 >> << /S /TH /K 3 >> ] >> \
            << /S /TR /K [ << /S /TD /K 4 >> << /S /TD /K 5 >> ] >> ] >> \
         << /S /L /K [ << /S /LI /K [ << /S /Lbl /K 6 >> << /S /LBody /K 7 >> ] >> ] >> \
         << /S /Link /K << /Type /OBJR /Obj 9 0 R >> >> \
         << /S /Form /K << /Type /OBJR /Obj 10 0 R >> >> ] >>"
            .into(),
        "<< /Title (Quarterly report) >>".into(),
        // 9 link, 10 field, 11 image
        "<< /Type /Annot /Subtype /Link /Rect [20 20 80 40] /StructParent 1 >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /TU (Your name) /Rect [20 60 120 80] /StructParent 2 >>".into(),
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\n\u{0}\nendstream"
            .into(),
    ]
}

#[test]
fn a_well_tagged_document_passes() {
    let doc = pdf(&good(), "/Info 8 0 R");
    let r = check(&doc, &Options::default());
    assert!(failed(&r).is_empty(), "{:?}", r.results.iter().filter(|x| x.status == Status::Failed).collect::<Vec<_>>());
    assert_eq!((r.count(Status::Manual), r.count(Status::Skipped), r.count(Status::Passed)), (5, 1, 26));
}

#[test]
fn structure_problems_are_found() {
    let mut objs = good();
    objs[6] = "<< /S /Document /P 6 0 R /Pg 3 0 R /K [\
         << /S /H1 /K 0 >> << /S /H3 /K 2 >> \
         << /S /Figure /K 1 >> \
         << /S /Sect /Alt (Outer) /K [ << /S /P /Alt (Inner) /K 3 >> ] >> \
         << /S /Div /Alt (Nothing) >> \
         << /S /P /Alt (Over a link) /K << /Type /OBJR /Obj 9 0 R >> >> \
         << /S /Formula /K 4 >> \
         << /S /Table /K [ << /S /TR /K [ << /S /TD /K 5 >> << /S /TD >> ] >> << /S /TR /K [ << /S /TD /K 6 >> ] >> ] >> \
         << /S /TD /K 7 >> << /S /LI >> << /S /LBody >> << /S /TR >> ] >>"
        .into();
    let doc = pdf(&objs, "/Info 8 0 R");
    let r = check(&doc, &Options::default());
    let mut want = vec![
        Rule::FiguresAltText,
        Rule::NestedAltText,
        Rule::AltTextAssociated,
        Rule::AltTextHidesAnnotation,
        Rule::OtherElementsAltText,
        Rule::TableRows,
        Rule::TableCells,
        Rule::TableHeaders,
        Rule::TableRegularity,
        Rule::TableSummary,
        Rule::ListItems,
        Rule::LblLBody,
        Rule::HeadingNesting,
    ];
    want.sort();
    let mut got = failed(&r);
    got.sort();
    assert_eq!(got, want);
    let h = &r.result(Rule::HeadingNesting).unwrap().findings[0].message;
    assert!(h.contains("H3") && h.contains("H1"), "{h}");
    assert!(r.result(Rule::TableRegularity).unwrap().findings[0].message.contains("between 1 and 2"));
}

#[test]
fn row_and_column_spans_keep_a_table_regular() {
    let mut objs = good();
    // Row 1: a cell spanning two rows and one spanning two columns; row 2 then has one cell.
    objs[6] = "<< /S /Document /P 6 0 R /K [ << /S /Table /A << /O /Table /Summary (s) >> /K [\
         << /S /TR /K [ << /S /TH /A << /O /Table /RowSpan 2 >> /K 0 >> << /S /TH /A [ << /O /Table /ColSpan 2 >> ] /K 1 >> ] >> \
         << /S /TR /K [ << /S /TD /K 2 >> << /S /TD /K 3 >> ] >> ] >> ] >>"
        .into();
    let r = check(&pdf(&objs, "/Info 8 0 R"), &Options { rules: [Rule::TableRegularity].into(), pages: None });
    assert_eq!(status(&r, Rule::TableRegularity), Status::Passed, "{:?}", r.result(Rule::TableRegularity));
}

/// A tagged document whose structure tree is the Document element `document` (object 3).
fn structure_doc(objects: &[String]) -> Document {
    let mut objs = vec!["<< /Type /Catalog /StructTreeRoot 2 0 R >>".to_string(), "<< /Type /StructTreeRoot /K 3 0 R >>".to_string()];
    objs.extend_from_slice(objects);
    pdf(&objs, "")
}

#[test]
fn reachable_tag_properties_round_trip_and_reject_unrelated_objects() {
    let mut doc =
        structure_doc(&["<< /S /Document /K [4 0 R] >>".into(), "<< /S /Figure /T (Old) /Alt (A chart) >>".into(), "<< /S /Figure >>".into()]);
    let tag = pdfcraft_cos::ObjRef::new(4, 0);
    let listed = tags(&doc);
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[1].object, tag);
    assert_eq!(listed[1].title.as_deref(), Some("Old"));
    for (property, value) in [
        (TagProperty::Type, "P"),
        (TagProperty::Title, "Figure title"),
        (TagProperty::ActualText, " Revenue increased "),
        (TagProperty::AlternateText, "A rising chart"),
        (TagProperty::Language, "en-GB"),
    ] {
        set_tag_property(&mut doc, tag, property, Some(value)).unwrap();
    }
    let edited = &tags(&doc)[1];
    assert_eq!(edited.kind, "P");
    assert_eq!(edited.title.as_deref(), Some("Figure title"));
    assert_eq!(edited.actual_text.as_deref(), Some(" Revenue increased "));
    assert_eq!(edited.alt.as_deref(), Some("A rising chart"));
    assert_eq!(edited.language.as_deref(), Some("en-GB"));
    set_tag_property(&mut doc, tag, TagProperty::Title, None).unwrap();
    assert_eq!(tags(&doc)[1].title, None);
    assert!(matches!(set_tag_property(&mut doc, tag, TagProperty::Type, None), Err(TagError::InvalidType)));
    assert!(matches!(set_tag_property(&mut doc, tag, TagProperty::Type, Some("bad type")), Err(TagError::InvalidType)));
    assert!(matches!(set_tag_property(&mut doc, pdfcraft_cos::ObjRef::new(5, 0), TagProperty::Title, Some("No")), Err(TagError::NotATag(_))));
    assert!(matches!(set_tag_property(&mut doc, pdfcraft_cos::ObjRef::new(4, 1), TagProperty::Title, Some("No")), Err(TagError::NotATag(_))));
}

#[test]
fn tag_hierarchy_keeps_indirect_parent_through_inline_tags() {
    let doc = structure_doc(&["<< /S /Document /K 4 0 R >>".into(), "<< /S /Sect /K << /S /Div /K 5 0 R >> >>".into(), "<< /S /Figure >>".into()]);
    let listed = tags(&doc);
    assert_eq!(
        listed.iter().map(|t| (t.object.num, t.parent.map(|p| p.num), t.depth)).collect::<Vec<_>>(),
        [(3, None, 0), (4, Some(3), 1), (5, Some(4), 2)]
    );
}

fn checked(doc: &Document, rules: &[Rule]) -> Report {
    check(doc, &Options { rules: rules.iter().copied().collect(), pages: None })
}

#[test]
fn figures_and_tables_past_the_old_element_cut_off_are_checked() {
    // Five thousand paragraphs, then a figure without alternate text and a table whose second row
    // is narrower than its first.
    let mut kids = vec!["<< /S /P >>".to_string(); 5_000];
    kids.push("<< /S /Figure >>".into());
    kids.push(
        "<< /S /Table /A << /O /Table /Summary (s) >> /K [ << /S /TR /K [ << /S /TD >> << /S /TD >> ] >> << /S /TR /K [ << /S /TD >> ] >> ] >>"
            .into(),
    );
    let doc = structure_doc(&[format!("<< /S /Document /K [{}] >>", kids.join(" "))]);
    let r = checked(&doc, &[Rule::FiguresAltText, Rule::TableRegularity]);
    assert_eq!(r.result(Rule::FiguresAltText).unwrap().findings.len(), 1);
    assert!(r.result(Rule::TableRegularity).unwrap().findings[0].message.contains("between 1 and 2"));
}

#[test]
fn findings_are_listed_in_document_order_not_in_the_order_they_close() {
    // Each element has alternate text but no content. The outer ones close last.
    let doc = structure_doc(&["<< /S /Document /K [ << /S /Sect /Alt (a) /K [ << /S /P /Alt (b) /K [ << /S /Div /Alt (c) >> ] >> ] >> ] >>".into()]);
    let r = checked(&doc, &[Rule::AltTextAssociated]);
    let kinds: Vec<&str> = r.result(Rule::AltTextAssociated).unwrap().findings.iter().map(|f| f.message.split(' ').next().unwrap_or("")).collect();
    assert_eq!(kinds, ["Sect", "P", "Div"]);
}

#[test]
fn a_rule_lists_its_first_findings_and_counts_the_rest() {
    let figures = vec!["<< /S /Figure >>".to_string(); 300].join(" ");
    let doc = structure_doc(&[format!("<< /S /Document /K [{figures}] >>")]);
    let r = checked(&doc, &[Rule::FiguresAltText]);
    let found = &r.result(Rule::FiguresAltText).unwrap().findings;
    assert_eq!(found.len(), 201);
    assert_eq!(found[200].message, "… and 100 more");
    assert!(found[..200].iter().all(|f| f.message == "Figure element: no alternate text"));
}

#[test]
fn cycles_shared_kids_and_missing_kids_are_read_once() {
    // Object 4 holds itself, a figure twice, and a missing object is listed too.
    let doc = structure_doc(&[
        "<< /S /Document /K [4 0 R 4 0 R 99 0 R] >>".into(),
        "<< /S /Sect /K [4 0 R 5 0 R 5 0 R] >>".into(),
        "<< /S /Figure >>".into(),
    ]);
    let r = checked(&doc, &[Rule::FiguresAltText]);
    assert_eq!(r.result(Rule::FiguresAltText).unwrap().findings.len(), 1);
}

#[test]
fn structure_findings_identify_indirect_elements_without_guessing_inline_objects() {
    let doc = structure_doc(&["<< /S /Document /K [4 0 R << /S /Figure >>] >>".into(), "<< /S /Figure >>".into()]);
    let r = checked(&doc, &[Rule::FiguresAltText]);
    let found = &r.result(Rule::FiguresAltText).unwrap().findings;
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].object.map(|r| r.num), Some(4));
    assert_eq!(found[1].object, None);
}

#[test]
fn nesting_is_read_to_the_depth_limit_and_no_further() {
    // Sections, each the kid of the one before, ending in a figure without alternate text. The
    // figure is at depth `sections`; the walk reads depths up to 256.
    let figure_within = |sections: usize| {
        let mut objs: Vec<String> = (0..sections).map(|i| format!("<< /S /Sect /K {} 0 R >>", 4 + i)).collect();
        objs.push("<< /S /Figure >>".into());
        let r = checked(&structure_doc(&objs), &[Rule::FiguresAltText]);
        r.result(Rule::FiguresAltText).unwrap().findings.len()
    };
    assert_eq!(figure_within(250), 1, "within the depth limit");
    assert_eq!(figure_within(300), 0, "past the depth limit");
}

#[test]
fn arrays_nested_deeper_than_the_limit_are_not_read() {
    let nested = |n: usize| format!("<< /S /Document /K {}<< /S /Figure >>{} >>", "[".repeat(n), "]".repeat(n));
    let shallow = checked(&structure_doc(&[nested(100)]), &[Rule::FiguresAltText]);
    assert_eq!(shallow.result(Rule::FiguresAltText).unwrap().findings.len(), 1);
    let deep = checked(&structure_doc(&[nested(300)]), &[Rule::FiguresAltText]);
    assert_eq!(deep.result(Rule::FiguresAltText).unwrap().findings.len(), 0);
}

#[test]
fn a_long_list_of_marked_content_counts_as_content() {
    // The first paragraph holds 20,000 marked-content items, so its alternate text has content.
    let items = (0..20_000).map(|i| i.to_string()).collect::<Vec<_>>().join(" ");
    let doc = structure_doc(&[format!("<< /S /Document /K [ << /S /P /Alt (a) /K [{items}] >> << /S /P /Alt (b) >> ] >>")]);
    let r = checked(&doc, &[Rule::AltTextAssociated]);
    assert_eq!(r.result(Rule::AltTextAssociated).unwrap().findings.len(), 1);
}

#[test]
fn huge_spans_do_not_size_the_table() {
    // A row of 2,000 cells, each spanning 10,000 columns, then a row of one cell.
    let wide = vec!["<< /S /TD /A << /O /Table /ColSpan 10000 >> >>".to_string(); 2_000].join(" ");
    let doc = structure_doc(&[format!(
        "<< /S /Document /K [ << /S /Table /A << /O /Table /Summary (s) >> /K [ << /S /TR /K [{wide}] >> << /S /TR /K [ << /S /TD >> ] >> ] >> ] >>"
    )]);
    let r = checked(&doc, &[Rule::TableRegularity]);
    let found = &r.result(Rule::TableRegularity).unwrap().findings;
    assert_eq!(found[0].message, "Table element: rows have between 1 and 20000000 columns");
}

#[test]
fn row_spans_carry_past_their_row() {
    // Row 1: 3,000 cells, each spanning 10,000 rows. Row 2: 3,000 cells, which start after them.
    let row1 = vec!["<< /S /TD /A << /O /Table /RowSpan 10000 >> >>".to_string(); 3_000].join(" ");
    let row2 = vec!["<< /S /TD >>".to_string(); 3_000].join(" ");
    let doc = structure_doc(&[format!(
        "<< /S /Document /K [ << /S /Table /A << /O /Table /Summary (s) >> /K [ << /S /TR /K [{row1}] >> << /S /TR /K [{row2}] >> ] >> ] >>"
    )]);
    let r = checked(&doc, &[Rule::TableRegularity]);
    let found = &r.result(Rule::TableRegularity).unwrap().findings;
    assert_eq!(found[0].message, "Table element: rows have between 3000 and 6000 columns");
}

#[test]
fn spans_that_are_not_positive_integers_read_as_one() {
    // ColSpan -3 and 0 read as 1, and 2.0 reads as 2; the first row is 4 columns wide, the second 5.
    let doc = structure_doc(&[
        "<< /S /Document /K [ << /S /Table /A << /O /Table /Summary (s) >> /K [ \
         << /S /TR /K [ << /S /TD /A << /O /Table /ColSpan -3 >> >> << /S /TD /A << /O /Table /ColSpan 0 >> >> << /S /TD /A << /O /Table /ColSpan 2.0 >> >> ] >> \
         << /S /TR /K [ << /S /TD >> << /S /TD >> << /S /TD >> << /S /TD >> << /S /TD >> ] >> ] >> ] >>"
            .into(),
    ]);
    let r = checked(&doc, &[Rule::TableRegularity]);
    assert_eq!(r.result(Rule::TableRegularity).unwrap().findings[0].message, "Table element: rows have between 4 and 5 columns");
}

#[test]
fn page_content_and_annotation_problems_are_found() {
    let mut objs = good();
    // Untagged text, no tab order, an untagged link and a field without a tooltip.
    objs[2] = objs[2].replace("/Tabs /S ", "");
    objs[3] = stream("BT /F1 12 Tf (stray) Tj ET /P << /MCID 0 >> BDC BT (ok) Tj ET EMC");
    objs[8] = "<< /Type /Annot /Subtype /Link /Rect [20 20 80 40] >>".into();
    objs[9] = "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [20 60 120 80] /StructParent 2 >>".into();
    let r = check(&pdf(&objs, "/Info 8 0 R"), &Options::default());
    let mut got = failed(&r);
    got.sort();
    assert_eq!(got, [Rule::TaggedContent, Rule::TaggedAnnotations, Rule::TabOrder, Rule::FieldDescriptions]);
    assert_eq!(r.result(Rule::TaggedContent).unwrap().findings[0].message, "Page 1: 1 untagged content item");
    assert!(r.result(Rule::FieldDescriptions).unwrap().findings[0].message.contains("\"name\""));
    assert_eq!(r.result(Rule::TaggedAnnotations).unwrap().findings[0].bbox, Some([20.0, 20.0, 80.0, 40.0]));
    assert_eq!(r.result(Rule::FieldDescriptions).unwrap().findings[0].bbox, Some([20.0, 60.0, 120.0, 80.0]));
}

#[test]
fn annotation_findings_only_locate_valid_rectangles() {
    let cases = [("[80 40 20 20]", Some([20.0, 20.0, 80.0, 40.0])), ("[20 20 20 40]", None), ("[20 20 80]", None), ("[20 20 80 (bad)]", None)];
    for (rect, expected) in cases {
        let mut objs = good();
        objs[8] = format!("<< /Type /Annot /Subtype /Link /Rect {rect} >>");
        let r = checked(&pdf(&objs, "/Info 8 0 R"), &[Rule::TaggedAnnotations]);
        assert_eq!(r.result(Rule::TaggedAnnotations).unwrap().findings[0].bbox, expected, "{rect}");
    }
}

#[test]
fn scans_identity_fonts_and_long_documents() {
    // An image-only page.
    let doc = pdf(
        &[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << /XObject << /Im 5 0 R >> >> >>".into(),
            stream("q 200 0 0 200 0 0 cm /Im Do Q"),
            "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\n\u{0}\nendstream"
                .into(),
        ],
        "",
    );
    assert_eq!(status(&check(&doc, &Options::default()), Rule::ImageOnly), Status::Failed);
    // An Identity-H font without ToUnicode; 21 pages without bookmarks.
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{}] /Count 21 >>", (0..21).map(|i| format!("{} 0 R", 6 + i)).collect::<Vec<_>>().join(" ")),
        stream("BT /F1 12 Tf <0001> Tj ET"),
        "<< /Type /Font /Subtype /Type0 /BaseFont /Custom /Encoding /Identity-H /DescendantFonts [5 0 R] >>".into(),
        "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Custom /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> >>".into(),
    ];
    for _ in 0..21 {
        objs.push("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 3 0 R /Resources << /Font << /F1 4 0 R >> >> >>".into());
    }
    let r = check(&pdf(&objs, ""), &Options::default());
    assert_eq!(status(&r, Rule::Bookmarks), Status::Failed);
    let enc = r.result(Rule::CharacterEncoding).unwrap();
    assert_eq!((enc.status, enc.findings.len()), (Status::Failed, 1), "each font once");
    assert!(enc.findings[0].message.contains("Custom"));
    // A page range limits the page rules.
    let some = check(&pdf(&objs, ""), &Options { pages: Some(vec![3]), ..Options::default() });
    assert_eq!(some.result(Rule::CharacterEncoding).unwrap().findings[0].page, Some(3));
}

fn cid_encoding_pdf(encoding: &str, system: &str, to_unicode: bool) -> Document {
    let unicode = if to_unicode { "/ToUnicode 7 0 R" } else { "" };
    pdf(
        &[
            "<< /Type /Catalog /Pages 2 0 R /MarkInfo << /Marked true >> >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
            stream("/P << /MCID 0 >> BDC BT /F1 12 Tf <002200230024> Tj ET EMC"),
            format!("<< /Type /Font /Subtype /Type0 /BaseFont /SyntheticCID /Encoding {encoding} /DescendantFonts [6 0 R] {unicode} >>"),
            format!("<< /Type /Font /Subtype /CIDFontType0 /BaseFont /SyntheticCID /CIDSystemInfo {system} >>"),
            stream(
                "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /SyntheticUnicode def /CMapType 2 def 1 begincodespacerange <0000> <FFFF> endcodespacerange 3 beginbfchar <0022> <0041> <0023> <0042> <0024> <0043> endbfchar endcmap CMapName currentdict /CMap defineresource pop end end",
            ),
        ],
        "",
    )
}

#[test]
fn character_encoding_accepts_identity_cjk_collections() {
    for encoding in ["/Identity-H", "/Identity-V"] {
        for ordering in ["Japan1", "GB1", "CNS1", "Korea1", "KR"] {
            let system = format!("<< /Registry (Adobe) /Ordering ({ordering}) /Supplement 0 >>");
            let doc = cid_encoding_pdf(encoding, &system, false);
            let report = checked(&doc, &[Rule::CharacterEncoding]);
            let result = report.result(Rule::CharacterEncoding).unwrap();
            assert_eq!(result.status, Status::Passed, "{encoding}, Adobe-{ordering}: {:?}", result.findings);
            assert!(result.findings.is_empty());
        }
    }
}

#[test]
fn character_encoding_accepts_predefined_cmaps_without_a_known_collection() {
    // The predefined CMap and the descendant's character collection are alternative routes.
    let system = "<< /Registry (Private) /Ordering (Custom) /Supplement 0 >>";
    for name in [
        "GB-EUC-H",
        "GB-EUC-V",
        "GBK2K-H",
        "GBK2K-V",
        "UniGB-UTF16-H",
        "UniGB-UTF16-V",
        "B5pc-H",
        "B5pc-V",
        "UniCNS-UCS2-H",
        "UniCNS-UCS2-V",
        "83pv-RKSJ-H",
        "90pv-RKSJ-H",
        "90ms-RKSJ-H",
        "90ms-RKSJ-V",
        "H",
        "V",
        "UniJIS-UCS2-HW-H",
        "UniJIS-UCS2-HW-V",
        "KSCpc-EUC-H",
        "KSC-EUC-V",
        "KSCms-UHC-HW-H",
        "KSCms-UHC-HW-V",
        "UniKS-UTF16-H",
        "UniKS-UTF16-V",
        "UniAKR-UTF8-H",
        "UniAKR-UTF16-H",
        "UniAKR-UTF32-H",
    ] {
        let doc = cid_encoding_pdf(&format!("/{name}"), system, false);
        let report = checked(&doc, &[Rule::CharacterEncoding]);
        assert_eq!(status(&report, Rule::CharacterEncoding), Status::Passed, "{name}");
    }
}

#[test]
fn character_encoding_rejects_unknown_cmaps_and_foreign_collections() {
    for encoding in
        ["/Identity-H", "/Identity-V", "/Custom-H", "/UniCustom-UTF16-H", "/UniJIS-UCS2-HW-extra-H", "/83pv-RKSJ-V", "/KSCpc-EUC-V", "null"]
    {
        for system in [
            "<< /Registry (Adobe) /Ordering (Identity) /Supplement 0 >>",
            "<< /Registry (Adobe) /Ordering (Custom) /Supplement 0 >>",
            "<< /Registry (Private) /Ordering (Japan1) /Supplement 0 >>",
            "<< /Registry (adobe) /Ordering (Japan1) /Supplement 0 >>",
            "<< /Ordering (Japan1) /Supplement 0 >>",
            "<< /Registry /Adobe /Ordering /Japan1 /Supplement 0 >>",
            "null",
        ] {
            let doc = cid_encoding_pdf(encoding, system, false);
            let report = checked(&doc, &[Rule::CharacterEncoding]);
            let result = report.result(Rule::CharacterEncoding).unwrap();
            assert_eq!(result.status, Status::Failed, "{encoding}, {system}");
            assert_eq!(result.findings.len(), 1);
            assert_eq!(result.findings[0].page, Some(0));
            assert!(result.findings[0].message.contains("SyntheticCID"));
        }
    }
    let doc = cid_encoding_pdf("/Identity-H", "<< /Registry (Private) /Ordering (Custom) /Supplement 0 >>", true);
    assert_eq!(status(&checked(&doc, &[Rule::CharacterEncoding]), Rule::CharacterEncoding), Status::Passed, "ToUnicode is still sufficient");
}

#[test]
fn character_encoding_resolves_indirect_cid_metadata() {
    use pdfcraft_cos::{Dict, ObjRef, PdfString};
    let mut doc = cid_encoding_pdf("/Identity-H", "null", false);
    let registry = doc.add(Object::String(PdfString::literal(b"Adobe".to_vec())));
    let ordering = doc.add(Object::String(PdfString::literal(b"Japan1".to_vec())));
    let mut system = Dict::new();
    system.set(b"Registry".to_vec(), Object::Ref(registry));
    system.set(b"Ordering".to_vec(), Object::Ref(ordering));
    let system = doc.add(Object::Dict(system));
    doc.update_dict(ObjRef::new(6, 0), |d| d.set(b"CIDSystemInfo".to_vec(), Object::Ref(system))).unwrap();
    let descendants = doc.add(Object::Array(vec![Object::Ref(ObjRef::new(6, 0))]));
    let encoding = doc.add(Object::Name(b"Identity-V".to_vec()));
    doc.update_dict(ObjRef::new(5, 0), |d| {
        d.set(b"DescendantFonts".to_vec(), Object::Ref(descendants));
        d.set(b"Encoding".to_vec(), Object::Ref(encoding));
    })
    .unwrap();
    let report = checked(&doc, &[Rule::CharacterEncoding]);
    assert_eq!(status(&report, Rule::CharacterEncoding), Status::Passed);
}

#[test]
fn rules_have_ids_and_the_report_lists_them() {
    assert_eq!(Rule::ALL.len(), 32);
    for r in Rule::ALL {
        assert_eq!(Rule::from_id(r.id()), Some(r));
    }
    let per: Vec<usize> = Category::ALL.iter().map(|c| Rule::ALL.iter().filter(|r| r.category() == *c).count()).collect();
    assert_eq!(per, [8, 9, 2, 5, 5, 2, 1]);
    let doc = pdf(&good(), "/Info 8 0 R");
    let report = check(&doc, &Options::default());
    let html = report_html(&report, "a<b>.pdf", "2026-10-02", "en", &in_english);
    assert!(html.contains("a&lt;b&gt;.pdf") && html.contains("Accessibility permission flag") && html.contains("Needs manual check: 5"));
    assert!(html.contains("<h3>Headings</h3>"));
    // In another language every word of ours is said in it, down to the `lang`; the file name is the
    // file's own and stays as it is.
    let other = report_html(&report, "a<b>.pdf", "2026-10-02", "es", &|w| format!("«{w}»"));
    assert!(other.contains("<html lang=\"es\">") && other.contains("<h3>«Headings»</h3>") && other.contains("a&lt;b&gt;.pdf"));
    assert!(!other.contains(">Headings<"), "an English heading is left in the report: {other}");
    let manual_only = Report { results: vec![RuleResult { rule: Rule::LogicalReadingOrder, status: Status::Manual, findings: Vec::new() }] };
    let manual_html = report_html(&manual_only, "review.pdf", "2026-10-02", "en", &in_english);
    assert!(manual_html.contains("<p>Needs manual check: 1</p>"));
    assert!(!manual_html.contains("no problems"));
    let all_passed = Report { results: vec![RuleResult { rule: Rule::LogicalReadingOrder, status: Status::Passed, findings: Vec::new() }] };
    let passed_html = report_html(&all_passed, "review.pdf", "2026-10-02", "en", &in_english);
    assert!(passed_html.contains("<p>The checker found no problems in this document.</p>"), "{passed_html}");
}

fn figures_doc() -> Document {
    pdf(
        &[
            "<< /Type /Catalog /Pages 2 0 R /MarkInfo << /Marked true >> /StructTreeRoot 5 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /StructParents 0 /Resources << /XObject << /Im 10 0 R >> >> >>"
                .into(),
            stream(
                "/Figure << /MCID 0 >> BDC q 40 0 0 20 10 150 cm /Im Do Q EMC \
                 /Photo << /MCID 1 >> BDC 100 100 50 30 re f EMC",
            ),
            "<< /Type /StructTreeRoot /K 6 0 R /ParentTree 9 0 R /RoleMap << /Photo /Figure >> >>".into(),
            "<< /S /Document /P 5 0 R /K [7 0 R 8 0 R] >>".into(),
            "<< /S /Figure /P 6 0 R /Pg 3 0 R /K 0 >>".into(),
            "<< /S /Photo /P 6 0 R /Pg 3 0 R /Alt (A bar) /K 1 >>".into(),
            "<< /Nums [0 [7 0 R 8 0 R]] >>".into(),
            "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\n\u{0}\nendstream"
                .into(),
        ],
        "",
    )
}

#[test]
fn figures_are_listed_with_their_alt_text_and_place() {
    let doc = figures_doc();
    let f = figures(&doc);
    assert_eq!(f.len(), 2, "role-mapped Photo counts");
    assert_eq!((f[0].page, f[0].alt.as_deref(), f[0].bbox), (Some(0), None, Some([10.0, 150.0, 50.0, 170.0])));
    assert_eq!((f[1].alt.as_deref(), f[1].bbox), (Some("A bar"), Some([100.0, 100.0, 150.0, 130.0])));
    let report = check(&doc, &Options { rules: [Rule::FiguresAltText].into(), pages: None });
    let finding = &report.result(Rule::FiguresAltText).unwrap().findings[0];
    assert_eq!((finding.object, finding.page, finding.bbox), (Some(f[0].obj), Some(0), f[0].bbox));
}

#[test]
fn tag_content_lists_direct_marked_content() {
    let doc = figures_doc();
    let tags = tags(&doc);
    let first = tag_content(&doc, tags[1].object).unwrap();
    let second = tag_content(&doc, tags[2].object).unwrap();
    assert_eq!((first[0].page, first[0].mcid, first[0].text.as_str()), (Some(0), 0, ""));
    assert_eq!((second[0].page, second[0].mcid, second[0].text.as_str()), (Some(0), 1, ""));
}

#[test]
fn sibling_tags_move_in_reading_order_without_changing_content() {
    let mut doc = figures_doc();
    let before: Vec<_> = tags(&doc).into_iter().map(|tag| tag.object.num).collect();
    assert_eq!(before, [6, 7, 8]);
    let content = doc.get(pdfcraft_cos::ObjRef::new(4, 0));
    move_tag(&mut doc, pdfcraft_cos::ObjRef::new(8, 0), pdfcraft_cos::ObjRef::new(7, 0), TagPlacement::Before).unwrap();
    assert_eq!(tags(&doc).into_iter().map(|tag| tag.object.num).collect::<Vec<_>>(), [6, 8, 7]);
    assert_eq!(doc.get(pdfcraft_cos::ObjRef::new(4, 0)), content, "page content stays unchanged");
    assert_eq!(
        move_tag(&mut doc, pdfcraft_cos::ObjRef::new(8, 0), pdfcraft_cos::ObjRef::new(7, 0), TagPlacement::Before),
        Err(TagError::AlreadyThere)
    );
    move_tag(&mut doc, pdfcraft_cos::ObjRef::new(8, 0), pdfcraft_cos::ObjRef::new(7, 0), TagPlacement::After).unwrap();
    assert_eq!(tags(&doc).into_iter().map(|tag| tag.object.num).collect::<Vec<_>>(), [6, 7, 8]);
    assert_eq!(
        move_tag(&mut doc, pdfcraft_cos::ObjRef::new(8, 0), pdfcraft_cos::ObjRef::new(6, 0), TagPlacement::Before),
        Err(TagError::NotSiblings)
    );
    assert_eq!(
        move_tag(&mut doc, pdfcraft_cos::ObjRef::new(8, 1), pdfcraft_cos::ObjRef::new(7, 0), TagPlacement::Before),
        Err(TagError::NotSiblings)
    );
    doc.update_dict(pdfcraft_cos::ObjRef::new(6, 0), |d| {
        d.set(
            b"K".to_vec(),
            pdfcraft_cos::Object::Array(vec![
                pdfcraft_cos::Object::Ref(pdfcraft_cos::ObjRef::new(7, 0)),
                pdfcraft_cos::Object::Ref(pdfcraft_cos::ObjRef::new(8, 0)),
                pdfcraft_cos::Object::Ref(pdfcraft_cos::ObjRef::new(7, 0)),
            ]),
        );
    })
    .unwrap();
    assert_eq!(
        move_tag(&mut doc, pdfcraft_cos::ObjRef::new(8, 0), pdfcraft_cos::ObjRef::new(7, 0), TagPlacement::After),
        Err(TagError::AmbiguousChildren)
    );
}

#[test]
fn setting_alt_text_and_marking_decorative() {
    let mut doc = figures_doc();
    let f = figures(&doc);
    set_alt(&mut doc, f[0].obj, Some("  A grey square ")).unwrap();
    assert_eq!(figures(&doc)[0].alt.as_deref(), Some("A grey square"));
    let r = check(&doc, &Options { rules: [Rule::FiguresAltText].into(), pages: None });
    assert_eq!(r.result(Rule::FiguresAltText).unwrap().status, Status::Passed);
    set_alt(&mut doc, f[0].obj, None).unwrap();
    assert_eq!(figures(&doc)[0].alt, None);
    assert_eq!(set_alt(&mut doc, pdfcraft_cos::ObjRef::new(6, 0), Some("x")), Err(AltError::NotAFigure(6)));
    // Decorative: the figure leaves the tags and its content becomes an artifact.
    mark_decorative(&mut doc, f[0].obj).unwrap();
    let left = figures(&doc);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].alt.as_deref(), Some("A bar"));
    let r = check(&doc, &Options { rules: [Rule::FiguresAltText, Rule::TaggedContent].into(), pages: None });
    assert_eq!(failed(&r), Vec::<Rule>::new(), "{:?}", r.results);
    let pages = pdfcraft_model::pages(&doc);
    let c = doc.resolve(pages[0].dict.get(b"Contents").unwrap());
    let pdfcraft_cos::Object::Stream(c) = &*c else { panic!("one content stream") };
    let text = String::from_utf8(c.decoded().unwrap()).unwrap();
    assert!(text.contains("/Artifact BMC") && !text.contains("/MCID 0"), "{text}");
}

#[test]
fn marking_a_leaf_tag_as_an_artifact_preserves_appearance_and_structure_integrity() {
    let mut doc = figures_doc();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    mark_tag_artifact(&mut doc, pdfcraft_cos::ObjRef::new(7, 0)).unwrap();
    assert_eq!(tags(&doc).into_iter().map(|tag| tag.object.num).collect::<Vec<_>>(), [6, 8]);
    let c = doc.resolve(doc.get(page).as_dict().unwrap().get(b"Contents").unwrap());
    let pdfcraft_cos::Object::Stream(c) = &*c else { panic!("one content stream") };
    let text = String::from_utf8(c.decoded().unwrap()).unwrap();
    assert!(text.contains("/Artifact BMC") && !text.contains("/MCID 0"), "{text}");
    let report = check(&doc, &Options { rules: [Rule::TaggedContent, Rule::FiguresAltText].into(), pages: None });
    assert_eq!(failed(&report), Vec::<Rule>::new(), "{:?}", report.results);
    assert_eq!(mark_tag_artifact(&mut doc, pdfcraft_cos::ObjRef::new(6, 0)), Err(ArtifactError::HasChildren));
    assert_eq!(mark_tag_artifact(&mut doc, pdfcraft_cos::ObjRef::new(7, 0)), Err(ArtifactError::NotATag(7)));
}

#[test]
fn marking_decorative_survives_a_parent_tree_with_an_unpaired_key() {
    // A trailing key without a value, matching the page's StructParents (#1809), and the same
    // with no valid pair at all.
    let tree = |nums: &str| pdf(&[format!("<< /Nums {nums} >>")], "").get(pdfcraft_cos::ObjRef::new(1, 0)).as_ref().clone();
    for (nums, cleared) in [("[0 [7 0 R 8 0 R] 0]", "[0 [null 8 0 R] 0]"), ("[0]", "[0]")] {
        let mut doc = figures_doc();
        let pt = pdfcraft_cos::ObjRef::new(9, 0);
        doc.set(pt, tree(nums));
        let f = figures(&doc);
        mark_decorative(&mut doc, f[0].obj).unwrap();
        assert_eq!(figures(&doc).len(), 1, "{nums}");
        // The pair is cleared and the unpaired key is kept as it was.
        assert_eq!(*doc.get(pt), tree(cleared), "{nums}");
    }
}

/// `figures_doc` with its property lists named in the page resources (one inline, one indirect)
/// instead of written inline in the content.
fn named_figures_doc() -> Document {
    pdf(
        &[
            "<< /Type /Catalog /Pages 2 0 R /MarkInfo << /Marked true >> /StructTreeRoot 5 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /StructParents 0 \
             /Resources << /XObject << /Im 10 0 R >> /Properties << /P0 << /MCID 0 >> /P1 11 0 R /Other << /Note (kept) >> >> >> >>"
                .into(),
            stream(
                "/Figure /P0 BDC q 40 0 0 20 10 150 cm /Im Do Q EMC \
                 /Photo /P1 BDC 100 100 50 30 re f EMC",
            ),
            "<< /Type /StructTreeRoot /K 6 0 R /ParentTree 9 0 R /RoleMap << /Photo /Figure >> >>".into(),
            "<< /S /Document /P 5 0 R /K [7 0 R 8 0 R] >>".into(),
            "<< /S /Figure /P 6 0 R /Pg 3 0 R /K 0 >>".into(),
            "<< /S /Photo /P 6 0 R /Pg 3 0 R /Alt (A bar) /K 1 >>".into(),
            "<< /Nums [0 [7 0 R 8 0 R]] >>".into(),
            "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\n\u{0}\nendstream"
                .into(),
            "<< /MCID 1 >>".into(),
        ],
        "",
    )
}

#[test]
fn named_property_lists_are_found_and_marked_decorative_like_inline_ones() {
    let (mut inline, mut named) = (figures_doc(), named_figures_doc());
    let place = |doc: &Document| figures(doc).into_iter().map(|f| (f.obj, f.page, f.alt, f.bbox)).collect::<Vec<_>>();
    assert_eq!(place(&named), place(&inline), "both figures, placed where their content is drawn");
    for doc in [&mut inline, &mut named] {
        mark_decorative(doc, pdfcraft_cos::ObjRef::new(8, 0)).unwrap();
        let left = figures(doc);
        assert_eq!((left.len(), left[0].obj.num, left[0].bbox), (1, 7, Some([10.0, 150.0, 50.0, 170.0])));
        let pages = pdfcraft_model::pages(doc);
        let c = doc.resolve(pages[0].dict.get(b"Contents").unwrap());
        let Object::Stream(c) = &*c else { panic!("one content stream") };
        let text = String::from_utf8(c.decoded().unwrap()).unwrap();
        assert!(text.contains("/Artifact BMC") && !text.contains("/P1") && !text.contains("/MCID 1"), "{text}");
        assert!(text.contains("/Im Do"), "the drawing itself stays: {text}");
        let nums = doc.get(pdfcraft_cos::ObjRef::new(9, 0)).as_dict().and_then(|d| d.get(b"Nums").cloned());
        assert_eq!(
            nums,
            Some(Object::Array(vec![Object::Int(0), Object::Array(vec![Object::Ref(pdfcraft_cos::ObjRef::new(7, 0)), Object::Null])])),
            "the parent tree no longer points MCID 1 at the removed figure"
        );
        let r = check(doc, &Options { rules: [Rule::FiguresAltText, Rule::TaggedContent].into(), pages: None });
        assert_eq!(r.result(Rule::TaggedContent).unwrap().status, Status::Passed, "{:?}", r.results);
    }
    let props = pdfcraft_model::pages(&named)[0].dict.get(b"Resources").and_then(|r| named.resolve(r).as_dict().cloned());
    let props = props.and_then(|r| r.get(b"Properties").and_then(|p| named.resolve(p).as_dict().cloned())).unwrap();
    assert!(props.contains(b"P0") && props.contains(b"P1") && props.contains(b"Other"), "resources are left as they were");
}

fn ref_(n: u32) -> Option<pdfcraft_cos::ObjRef> {
    Some(pdfcraft_cos::ObjRef::new(n, 0))
}

/// Two tagged pages: role-mapped and nested headings, actual text, a ToUnicode font, a heading
/// without text and a heading that lists itself among its kids.
fn headings_doc() -> Vec<String> {
    vec![
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 8 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R /Resources << /Font << /F1 7 0 R >> >> >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 6 0 R /Resources << /Font << /F1 7 0 R /F2 9 0 R >> >> >>".into(),
        stream(
            "/H1 << /MCID 0 >> BDC BT /F1 12 Tf 20 180 Td (Chapter) Tj ( One) Tj ET EMC \
             /P << /MCID 1 >> BDC BT (Body) Tj ET EMC \
             /H << /MCID 2 >> BDC BT /F1 10 Tf [(Sec) -500 (A)] TJ ET EMC",
        ),
        stream("/H2 << /MCID 0 >> BDC BT /F1 12 Tf (Ignored) Tj ET EMC /H3 << /MCID 1 >> BDC BT /F2 12 Tf <0102> Tj ET EMC"),
        FONT.into(),
        "<< /Type /StructTreeRoot /K 10 0 R /RoleMap << /Chapter /H1 >> >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Custom /ToUnicode 11 0 R >>".into(),
        "<< /S /Document /K [12 0 R << /S /P /Pg 3 0 R /K 1 >> << /S /Sect /K << /S /Sect /K 13 0 R >> >> 14 0 R 15 0 R 16 0 R] >>".into(),
        stream(
            "/CIDInit /ProcSet findresource begin 12 dict begin begincmap 1 begincodespacerange <00> <FF> endcodespacerange \
             2 beginbfchar <01> <0041> <02> <0042> endbfchar endcmap end end",
        ),
        "<< /S /Chapter /Pg 3 0 R /K 0 >>".into(),
        "<< /S /H /Pg 3 0 R /K << /Type /MCR /MCID 2 >> >>".into(),
        "<< /S /H2 /Pg 4 0 R /ActualText (Override) /K 0 >>".into(),
        "<< /S /H3 /Pg 4 0 R /K [1 15 0 R] >>".into(),
        "<< /S /H1 /Pg 4 0 R /K 9 >>".into(),
    ]
}

#[test]
fn headings_are_found_with_their_levels_titles_and_pages() {
    let doc = pdf(&headings_doc(), "");
    let found: Vec<_> = headings(&doc).into_iter().map(|h| (h.title, h.level, h.page, h.obj)).collect();
    assert_eq!(
        found,
        [
            ("Chapter One".to_string(), 1, 0, ref_(12)),
            ("Sec A".to_string(), 2, 0, ref_(13)),
            ("Override".to_string(), 2, 1, ref_(14)),
            ("AB".to_string(), 3, 1, ref_(15)),
        ]
    );
}

#[test]
fn tag_content_reads_the_text_of_each_marked_content_reference() {
    let doc = pdf(&headings_doc(), "");
    let items = |n| tag_content(&doc, ref_(n).unwrap()).unwrap().into_iter().map(|i| (i.page, i.mcid, i.text)).collect::<Vec<_>>();
    assert_eq!(items(12), [(Some(0), 0, "Chapter One".to_string())]);
    assert_eq!(items(13), [(Some(0), 2, "Sec A".to_string())]);
    assert_eq!(items(15), [(Some(1), 1, "AB".to_string())]);
}

/// Helvetica advances (standard 14 metrics, per 1000 em) of the characters the tests below draw.
fn helvetica_width(c: char) -> f64 {
    match c {
        'C' | 'w' => 722.0,
        'P' => 667.0,
        'T' | 'F' => 611.0,
        'v' => 500.0,
        'r' => 333.0,
        't' | ' ' => 278.0,
        'i' => 222.0,
        _ => 556.0, // a e h o p
    }
}

/// `text` drawn one glyph at a time from (72, y), each glyph moved into place with its own `Td`,
/// as Chromium writes text. With `spaces`, the space is drawn too; without, the next glyph is
/// just moved past it.
fn glyph_by_glyph(text: &str, size: f64, y: f64, spaces: bool) -> String {
    let mut ops = format!("BT /F1 {size} Tf 72 {y} Td");
    let mut step = 0.0;
    for c in text.chars() {
        if c == ' ' && !spaces {
            step += helvetica_width(c) / 1000.0 * size;
            continue;
        }
        ops.push_str(&format!(" {step:.3} 0 Td ({c}) Tj"));
        step = helvetica_width(c) / 1000.0 * size;
    }
    ops + " ET"
}

/// `text` drawn one glyph at a time with `/F1 1 Tf` and a `Tm` per glyph scaled to `size`; the
/// space is skipped over, not drawn.
fn glyph_by_tm(text: &str, size: f64, y: f64) -> String {
    let mut ops = "BT /F1 1 Tf".to_string();
    let mut x = 72.0;
    for c in text.chars() {
        if c != ' ' {
            ops.push_str(&format!(" {size} 0 0 {size} {x:.3} {y} Tm ({c}) Tj"));
        }
        x += helvetica_width(c) / 1000.0 * size;
    }
    ops + " ET"
}

/// One tagged page whose H1 headings show `contents` (one marked-content sequence each).
fn heading_page(contents: &[String]) -> Document {
    let marked: Vec<String> = contents.iter().enumerate().map(|(i, c)| format!("/H1 << /MCID {i} >> BDC {c} EMC")).collect();
    let kids: Vec<String> = (0..contents.len()).map(|i| format!("{} 0 R", 7 + i)).collect();
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 5 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 6 0 R >> >> >>".into(),
        stream(&marked.join("\n")),
        format!("<< /Type /StructTreeRoot /K [{}] >>", kids.join(" ")),
        FONT.into(),
    ];
    objs.extend((0..contents.len()).map(|i| format!("<< /S /H1 /Pg 3 0 R /K {i} >>")));
    pdf(&objs, "")
}

/// Chromium (Skia) places each glyph with its own `Td`: a heading must read as its words, not as
/// "C h a p t e r T w o", whether the space is drawn or only skipped over. A heading set with a
/// negative font size or `Tz` keeps its words too.
#[test]
fn headings_drawn_glyph_by_glyph_keep_their_words() {
    let doc = heading_page(&[
        "BT /F1 20 Tf 72 700 Td (Chapter One) Tj ET".into(),
        glyph_by_glyph("Chapter Two", 20.0, 650.0, true),
        glyph_by_glyph("Chapter Three", 20.0, 600.0, false),
        // Each glyph placed with its own Tm, at font size 1 scaled by the matrix.
        glyph_by_tm("Part Five", 18.0, 550.0),
        // Kerning inside a word, then a word space as a TJ adjustment, then a wrapped line.
        "BT /F1 16 Tf 72 450 Td [(Ch) 40 (apter) -350 (Four)] TJ 0 -20 Td (continued) Tj ET".into(),
        // A negative font size runs the glyphs backwards in text space: alone, in a mirrored
        // text matrix (upright on the page), with a word space as a TJ adjustment, and the same
        // with a negative horizontal scale instead (reset after, as Tz outlives the text object).
        "BT /F1 -20 Tf 72 380 Td (Chapter Six) Tj ET".into(),
        "BT /F1 -20 Tf -1 0 0 -1 300 330 Tm (Chapter Seven) Tj ET".into(),
        "BT /F1 -20 Tf 72 280 Td [(Chapter) -300 (Eight)] TJ ET".into(),
        "BT /F1 20 Tf -100 Tz 300 230 Td [(Chapter) -300 (Nine)] TJ ET 100 Tz".into(),
    ]);
    let titles: Vec<String> = headings(&doc).into_iter().map(|h| h.title).collect();
    assert_eq!(
        titles,
        [
            "Chapter One",
            "Chapter Two",
            "Chapter Three",
            "Part Five",
            "Chapter Four continued",
            "Chapter Six",
            "Chapter Seven",
            "Chapter Eight",
            "Chapter Nine"
        ]
    );
}

#[test]
fn an_untagged_document_has_no_headings() {
    let mut objs = headings_doc();
    objs[0] = "<< /Type /Catalog /Pages 2 0 R >>".into();
    assert!(headings(&pdf(&objs, "")).is_empty());
}

/// `[(FIRSTARR)] [(LASTARR)] TJ` is malformed (a surplus array operand), but the operator still
/// draws only the one immediately before it, as the renderer and every other reader agree (#3569)
/// -- not the first, which is never drawn at all.
#[test]
fn a_heading_with_a_tj_surplus_array_reads_the_one_the_page_draws() {
    let doc = heading_page(&["BT /F1 20 Tf 72 700 Td [(FIRSTARR)] [(LASTARR)] TJ ET".into()]);
    let titles: Vec<String> = headings(&doc).into_iter().map(|h| h.title).collect();
    assert_eq!(titles, ["LASTARR"]);
}

/// The text of every `"…"` literal with a space in it, outside comments and byte strings.
fn sentences(source: &str) -> Vec<String> {
    let b = source.as_bytes();
    let (mut out, mut i) = (Vec::new(), 0);
    while i < b.len() {
        if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i] == b'"' {
            let byte_string = i > 0 && b[i - 1] == b'b';
            let start = i + 1;
            i = start;
            while i < b.len() && b[i] != b'"' {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            // Quotes and backslashes are ASCII, so this never cuts a character in two.
            let text = source.get(start..i.min(b.len())).unwrap_or_default();
            if !byte_string && text.contains(' ') {
                out.push(text.replace("\\\"", "\""));
            }
            i += 1;
        } else {
            i += 1;
        }
    }
    out
}

/// Every sentence the rules can say is listed in `WORDINGS`, so an interface showing the report in
/// another language has something to translate. The two rule files hold no prose of their own, so a
/// literal with a space in it is a sentence a person reads.
#[test]
fn wordings_are_complete() {
    let mut missing = Vec::new();
    for source in [include_str!("content.rs"), include_str!("structure.rs")] {
        for sentence in sentences(source) {
            if !WORDINGS.contains(&sentence.as_str()) {
                missing.push(sentence);
            }
        }
    }
    assert!(missing.is_empty(), "add these to WORDINGS, and a translation of each to every interface: {missing:#?}");
}

/// Every word the report page says of its own is listed in `REPORT_WORDS`, and every word on that
/// list is one the page says: the page hands each over to `words` as a literal, so the two can be
/// read off one another.
#[test]
fn report_words_are_complete() {
    let source = include_str!("report.rs");
    let mut said = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find("words(\"") {
        rest = &rest[at + 7..];
        let Some(end) = rest.find('"') else { break };
        said.push(&rest[..end]);
        rest = &rest[end..];
    }
    let missing: Vec<&&str> = said.iter().filter(|word| !REPORT_WORDS.contains(word)).collect();
    assert!(missing.is_empty(), "add these to REPORT_WORDS, and a translation of each to every interface: {missing:#?}");
    let unsaid: Vec<&&str> = REPORT_WORDS.iter().filter(|word| !said.contains(word)).collect();
    assert!(unsaid.is_empty(), "REPORT_WORDS lists words the page no longer says: {unsaid:#?}");
}

/// `say` puts the values in, and what it puts in is never read as a hole of its own: a document that
/// names a font `{p}` cannot make a finding say which page it is on.
#[test]
fn say_fills_holes_once() {
    let values = [("{p}", "7".to_string()), ("{}", "{p} {n}".to_string())];
    assert_eq!(
        say("Font {} on page {p} doesn't map its characters to Unicode", &values),
        "Font {p} {n} on page 7 doesn't map its characters to Unicode"
    );
    // A hole with nothing for it, and a brace with nothing closing it, are left as they stand.
    assert_eq!(say("{a} {p} {", &values), "{a} 7 {");
}

/// A finding whose frame holds another wording says the whole thing in English, holes and all
/// filled, and still hands the interface the two wordings to translate.
#[test]
fn a_nested_finding_says_itself() {
    let values = vec![("{min}", "1".to_string()), ("{max}", "2".to_string()), ("{ty}", "Table".to_string()), ("{p}", "3".to_string())];
    let f = Finding::nested(Some(2), "{ty} element on page {p}: {what}", "rows have between {min} and {max} columns", values);
    assert_eq!(f.message, "Table element on page 3: rows have between 1 and 2 columns");
    assert_eq!(f.said(&in_english), f.message, "saying it in English is the message");
    // Both wordings are translated, each on its own, and the document's words are left alone.
    assert_eq!(f.said(&|w: &str| format!("«{w}»")), "«Table element on page 3: «rows have between 1 and 2 columns»»");
}

/// Horizontal stretching must not raise the vertical line-break threshold.
#[test]
fn horizontally_scaled_wrapped_heading_keeps_a_space_between_lines() {
    let doc = heading_page(&["BT /F1 20 Tf 400 Tz 72 700 Td (I) Tj 0 -30 Td (Overview) Tj ET".into()]);
    let titles: Vec<String> = headings(&doc).into_iter().map(|h| h.title).collect();
    assert_eq!(titles, ["I Overview"]);
}
