use std::sync::Arc;

use pdfcraft_cos::Document;

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
fn an_untagged_document_has_no_headings() {
    let mut objs = headings_doc();
    objs[0] = "<< /Type /Catalog /Pages 2 0 R >>".into();
    assert!(headings(&pdf(&objs, "")).is_empty());
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
