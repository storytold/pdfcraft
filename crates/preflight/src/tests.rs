use super::*;

fn build(objs: &[&str], trailer_extra: &str) -> Document {
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
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R {trailer_extra} >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(std::sync::Arc::new(out)).unwrap()
}

/// A page with embedded-font-free text, a note without the print flag, a link with a
/// JavaScript action, an interpolated image, and a document JavaScript.
fn messy() -> Document {
    build(
        &[
            "<< /Type /Catalog /Pages 2 0 R /Names << /JavaScript << /Names [(x) 9 0 R] >> >> /AA << /WC 9 0 R >> >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Im 8 0 R >> >> /Annots [6 0 R 7 0 R] >>",
            "<< /Length 44 >>\nstream\n1 0 0 rg BT /F1 12 Tf 10 10 Td (Hi) Tj ET /Im Do\nendstream",
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
            "<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /P 3 0 R /AP << /N 10 0 R >> >>",
            "<< /Type /Annot /Subtype /Link /Rect [40 10 80 30] /P 3 0 R /F 4 /A 9 0 R >>",
            "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Interpolate true /Length 1 >>\nstream\n\u{0}\nendstream",
            "<< /Type /Action /S /JavaScript /JS (app.alert(1)) >>",
            "<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length 0 >>\nstream\n\nendstream",
        ],
        "/Info << /Title (Report) /Producer (Test) /CreationDate (D:20240105093000Z) >>",
    )
}

fn clauses(issues: &[Issue]) -> Vec<&'static str> {
    let mut c: Vec<&str> = issues.iter().map(|i| i.clause).collect();
    c.dedup();
    c
}

#[test]
fn verify_reports_what_breaks_pdfa() {
    let issues = verify(&messy(), Level::A2b);
    let msgs: Vec<&str> = issues.iter().map(|i| i.message.as_str()).collect();
    for want in [
        "The document has no XMP metadata",
        "Device colour is used, but there is no PDF/A output intent",
        "The document has additional actions (/AA)",
        "The document has document-level JavaScript",
        "A JavaScript action isn't allowed",
        "A Text annotation isn't set to print, or is hidden",
        "The font Helvetica isn't embedded",
        "An image asks to be interpolated",
    ] {
        assert!(msgs.contains(&want), "missing {want:?} in {msgs:#?}");
    }
    assert!(issues.iter().find(|i| i.message.starts_with("The font")).is_some_and(|i| !i.fixable));
    assert!(clauses(&issues).contains(&"6.6.2.1"));
    // Each problem is also worded separately from what the document calls things, so an interface
    // can say it in another language; in English the two come to the same sentence.
    for i in &issues {
        assert_eq!(i.said(&in_english), i.message);
        assert!(WORDINGS.contains(&i.pattern), "{:?} is not in WORDINGS", i.pattern);
    }
    let font = issues.iter().find(|i| i.pattern == "The font {} isn't embedded").unwrap();
    assert_eq!(font.said(&|w: &str| format!("«{w}»")), "«The font {} isn't embedded»".replace("{}", "Helvetica"));
}

/// Every problem the rules can report is listed in `WORDINGS`, so an interface showing them in
/// another language has something to translate. The wording is `verify`'s second argument, which is
/// what this reads off the file; the sentences `convert` reports as fixed are not wordings.
#[test]
fn wordings_are_complete() {
    let source = include_str!("lib.rs");
    let mut said = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find("issue(\"") {
        rest = &rest[at + 7..];
        // Step over the clause, then take the wording that follows it.
        let Some(end) = rest.find('"') else { break };
        rest = &rest[end + 1..];
        let Some(text) = rest.strip_prefix(", \"") else { continue };
        let Some(end) = text.find('"') else { break };
        said.push(&text[..end]);
        rest = text;
    }
    assert!(said.len() >= 24, "the wordings are not being read off the file: {said:#?}");
    let missing: Vec<&&str> = said.iter().filter(|word| !WORDINGS.contains(word)).collect();
    assert!(missing.is_empty(), "add these to WORDINGS, and a translation of each to every interface: {missing:#?}");
    let unsaid: Vec<&&str> = WORDINGS.iter().filter(|word| !said.contains(word)).collect();
    assert!(unsaid.is_empty(), "WORDINGS lists problems the rules no longer report: {unsaid:#?}");
}

#[test]
fn convert_fixes_all_but_fonts() {
    let mut doc = messy();
    let r = convert(&mut doc, Level::A2b).unwrap();
    assert!(r.fixed.iter().any(|f| f.contains("output intent")), "{:?}", r.fixed);
    let left: Vec<&str> = r.remaining.iter().map(|i| i.message.as_str()).collect();
    assert_eq!(left, ["The font Helvetica isn't embedded"], "{:#?}", r.remaining);
    let d = declared(&doc);
    assert_eq!(d.pdfa, Some((2, "B".into())));
    assert_eq!(d.output_intents, ["sRGB IEC61966-2.1"]);
    // The metadata carries the document information.
    let x = metadata(&doc).unwrap();
    assert_eq!(xmp::value(&x, "dc:title").as_deref(), Some("Report"));
    assert_eq!(xmp::value(&x, "xmp:CreateDate").as_deref(), Some("2024-01-05T09:30:00Z"));
    // It survives a save.
    let bytes = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
    let back = Document::open(std::sync::Arc::new(bytes)).unwrap();
    assert_eq!(verify(&back, Level::A2b).len(), 1);
}

#[test]
fn embedded_files_differ_between_parts() {
    let doc = build(
        &[
            "<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [(a.txt) 4 0 R] >> >> >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R >>",
            "<< /Type /Filespec /F (a.txt) /EF << /F 5 0 R >> >>",
            "<< /Type /EmbeddedFile /Length 2 >>\nstream\nhi\nendstream",
        ],
        "",
    );
    assert!(verify(&doc, Level::A2b).iter().any(|i| i.clause == "6.8" && !i.fixable));
    let mut d3 = doc.clone();
    assert!(verify(&d3, Level::A3b).iter().any(|i| i.message.contains("AFRelationship")));
    let r = convert(&mut d3, Level::A3b).unwrap();
    assert!(r.remaining.is_empty(), "{:#?}", r.remaining);
}

/// A page that paints a DeviceCMYK image (as `doc_create` makes from a CMYK JPEG), with
/// `intent` as the catalog's extra entries and `extra` as more objects (from object 6 on).
fn cmyk_page(intent: &str, content: &str, extra: &[&str]) -> Document {
    let catalog = format!("<< /Type /Catalog /Pages 2 0 R {intent} >>");
    let stream = format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len());
    let mut objs = vec![
        catalog.as_str(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Im0 5 0 R >> >> >>",
        stream.as_str(),
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceCMYK /BitsPerComponent 8 /Length 4 >>\nstream\n\u{0}\u{0}\u{0}\u{0}\nendstream",
    ];
    objs.extend_from_slice(extra);
    build(&objs, "")
}

fn messages(issues: &[Issue]) -> Vec<&str> {
    issues.iter().map(|i| i.message.as_str()).collect()
}

/// Issue #667: converting a CMYK-only document doesn't add an sRGB output intent (it would
/// describe a colour space the document doesn't use); the CMYK output intent it needs is
/// reported as not fixable instead.
#[test]
fn convert_does_not_give_cmyk_documents_an_srgb_output_intent() {
    let mut doc = cmyk_page("", "q 200 0 0 200 0 0 cm /Im0 Do Q", &[]);
    let r = convert(&mut doc, Level::A2b).unwrap();
    assert!(!r.fixed.iter().any(|f| f.contains("output intent")), "{:?}", r.fixed);
    assert!(declared(&doc).output_intents.is_empty());
    let left = messages(&r.remaining);
    assert!(left.contains(&"Device colour is used, but there is no PDF/A output intent"), "{left:#?}");
    assert!(left.contains(&"DeviceCMYK is used: it needs a CMYK output intent (not added automatically)"), "{left:#?}");
    assert!(r.remaining.iter().all(|i| !i.fixable));
}

/// Issue #667: verify checks the output intent's colour space against the device colour the
/// pages use, not only that an output intent exists.
#[test]
fn verify_reports_an_output_intent_that_does_not_match_device_colour() {
    let intent = "/OutputIntents [<< /Type /OutputIntent /S /GTS_PDFA1 /OutputConditionIdentifier (X) /DestOutputProfile 6 0 R >>]";
    let profile = |n: u8| format!("<< /N {n} /Length 0 >>\nstream\n\nendstream");
    let (rgb, cmyk) = (profile(3), profile(4));
    let cmyk_msg = "DeviceCMYK is used, but the PDF/A output intent isn't a CMYK profile";
    let rgb_msg = "DeviceRGB is used, but the PDF/A output intent isn't an RGB profile";

    // CMYK content under an sRGB output intent (what convert used to write).
    let issues = verify(&cmyk_page(intent, "/Im0 Do", &[&rgb]), Level::A2b);
    assert!(messages(&issues).contains(&cmyk_msg), "{issues:#?}");
    assert!(issues.iter().find(|i| i.message == cmyk_msg).is_some_and(|i| i.clause == "6.2.4.3" && !i.fixable));

    // CMYK content under a CMYK output intent is fine; RGB and gray content beside it is not
    // and is, respectively.
    assert!(!messages(&verify(&cmyk_page(intent, "/Im0 Do 0 g", &[&cmyk]), Level::A2b)).iter().any(|m| m.contains("output intent")));
    let issues = verify(&cmyk_page(intent, "/Im0 Do 1 0 0 rg", &[&cmyk]), Level::A2b);
    assert!(messages(&issues).contains(&rgb_msg) && !messages(&issues).contains(&cmyk_msg), "{issues:#?}");

    // DefaultCMYK in the page's resources draws DeviceCMYK in that space, which PDF/A accepts
    // under any output intent.
    let catalog = format!("<< /Type /Catalog /Pages 2 0 R {intent} >>");
    let doc = build(
        &[
            catalog.as_str(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ColorSpace << /DefaultCMYK [/ICCBased 7 0 R] >> /XObject << /Im0 5 0 R >> >> >>",
            "<< /Length 7 >>\nstream\n/Im0 Do\nendstream",
            "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceCMYK /BitsPerComponent 8 /Length 4 >>\nstream\n\u{0}\u{0}\u{0}\u{0}\nendstream",
            rgb.as_str(),
            cmyk.as_str(),
        ],
        "",
    );
    assert!(!messages(&verify(&doc, Level::A2b)).contains(&cmyk_msg), "DefaultCMYK covers DeviceCMYK");

    // Without /N, the ICC header's colour space decides.
    let mut header = vec![b' '; 128];
    header[16..20].copy_from_slice(b"CMYK");
    let header = String::from_utf8(header).unwrap();
    let headed = format!("<< /Length 128 >>\nstream\n{header}\nendstream");
    assert!(!messages(&verify(&cmyk_page(intent, "/Im0 Do", &[&headed]), Level::A2b)).contains(&cmyk_msg));
}

/// Issue #1334: a colour operator is found whichever of the six PDF whitespace characters
/// separates the tokens, not only after a space or a line feed.
#[test]
fn device_colour_is_found_after_any_whitespace() {
    let page = |content: &[u8]| {
        let content = String::from_utf8(content.to_vec()).unwrap();
        let stream = format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len());
        build(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>",
                "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
                "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>",
                stream.as_str(),
            ],
            "",
        )
    };
    let cmyk_msg = "DeviceCMYK is used: it needs a CMYK output intent (not added automatically)";
    for separator in [b" ".as_slice(), b"\n", b"\r", b"\t", b"\x0C", b"\0"] {
        let cmyk: &[&[u8]] = &[b"0", b"0", b"0", b"1", b"k", b"10", b"10", b"50", b"50", b"re", b"f"];
        let mut doc = page(&cmyk.join(separator));
        assert!(messages(&verify(&doc, Level::A2b)).contains(&cmyk_msg), "separator {separator:?}");
        let r = convert(&mut doc, Level::A2b).unwrap();
        assert!(declared(&doc).output_intents.is_empty(), "separator {separator:?}");
        assert!(messages(&r.remaining).contains(&cmyk_msg), "separator {separator:?}");

        // Gray and RGB still get the sRGB output intent.
        for colour in [[b"0".as_slice(), b"g"].as_slice(), &[b"1", b"0", b"0", b"rg"]] {
            let mut doc = page(&colour.join(separator));
            let r = convert(&mut doc, Level::A2b).unwrap();
            assert!(r.fixed.iter().any(|f| f.contains("output intent")), "separator {separator:?}: {:?}", r.fixed);
            assert_eq!(declared(&doc).output_intents, ["sRGB IEC61966-2.1"], "separator {separator:?}");
        }
    }
}

/// A page that draws `content` with `resources`, followed by `extra` objects (from object 5 on).
fn drawing(content: &str, resources: &str, extra: &[&str]) -> Document {
    let stream = format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len());
    let page = format!("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << {resources} >> >>");
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
        page.as_str(),
        stream.as_str(),
    ];
    objs.extend_from_slice(extra);
    build(&objs, "")
}

fn uses_cmyk(doc: &Document) -> bool {
    messages(&verify(doc, Level::A2b)).contains(&"DeviceCMYK is used: it needs a CMYK output intent (not added automatically)")
}

/// Issue #2259: a `k` is DeviceCMYK only when it is an operator. Inside a string, a comment, an inline image's
/// data, a font program, an ICC profile or a mesh shading's data it is just a byte, and the
/// document still gets its sRGB output intent.
#[test]
fn device_colour_is_read_from_operators_not_bytes() {
    let rgb = "1 0 0 rg 0 0 10 10 re f";
    let cases: &[(&str, String, &str, Vec<&str>)] = &[
        (
            "a string",
            format!("{rgb} BT /F1 12 Tf 10 10 Td (press k now) Tj ET"),
            "/Font << /F1 5 0 R >>",
            vec![
                "<< /Type /Font /Subtype /TrueType /BaseFont /Sans /FontDescriptor 6 0 R >>",
                "<< /Type /FontDescriptor /FontName /Sans /FontFile2 7 0 R >>",
                // A font program whose bytes read " k ", as Chromium and krilla write them.
                "<< /Length1 3 /Length 3 >>\nstream\n k \nendstream",
            ],
        ),
        ("a comment", format!("{rgb} % 0 0 0 1 k"), "", vec![]),
        // A 3×1 gray image whose three bytes read " k ".
        ("inline image data", format!("{rgb} q 10 0 0 10 0 0 cm BI /W 3 /H 1 /CS /G /BPC 8 ID  k  EI Q"), "", vec![]),
        ("an ICC profile", rgb.to_owned(), "/ColorSpace << /CS0 [/ICCBased 5 0 R] >>", vec!["<< /N 3 /Length 3 >>\nstream\n k \nendstream"]),
        (
            "a mesh shading",
            format!("{rgb} /Sh0 sh"),
            "/Shading << /Sh0 5 0 R >>",
            vec![
                "<< /ShadingType 4 /ColorSpace /DeviceRGB /BitsPerCoordinate 8 /BitsPerComponent 8 /BitsPerFlag 8 /Decode [0 1 0 1 0 1 0 1 0 1] /BBox [0 0 10 10] /Length 3 >>\nstream\n k \nendstream",
            ],
        ),
    ];
    for (what, content, resources, extra) in cases {
        let mut doc = drawing(content, resources, extra);
        assert!(!uses_cmyk(&doc), "a k in {what} was read as DeviceCMYK");
        let r = convert(&mut doc, Level::A2b).unwrap();
        assert_eq!(declared(&doc).output_intents, ["sRGB IEC61966-2.1"], "{what}: {:?}", r.fixed);
    }
}

/// Issue #2259: DeviceCMYK is found however the content sets it: `k`/`K` right before a delimiter, `cs`/`CS`
/// with the space's name, or an inline image in `/CMYK` or indexed over it.
#[test]
fn device_colour_is_found_however_content_sets_it() {
    for content in [
        "0 0 0 1 k/GS0 gs 10 10 50 50 re f",
        "0 0 0 1 K[]0 d 10 10 m 50 50 l S",
        "/DeviceCMYK cs 0 0 0 1 sc 10 10 50 50 re f",
        "/DeviceCMYK CS 0 0 0 1 SC 10 10 m 50 50 l S",
        "q 10 0 0 10 0 0 cm BI /W 1 /H 1 /CS /CMYK /BPC 8 ID \u{0}\u{0}\u{0}\u{0} EI Q",
        "q 10 0 0 10 0 0 cm BI /W 1 /H 1 /ColorSpace /DeviceCMYK /BitsPerComponent 8 ID \u{0}\u{0}\u{0}\u{0} EI Q",
        "q 10 0 0 10 0 0 cm BI /W 1 /H 1 /CS [/I /DeviceCMYK 0 <00000000>] /BPC 8 ID \u{0} EI Q",
    ] {
        assert!(uses_cmyk(&drawing(content, "", &[])), "{content:?}");
    }
    // A resource named like a device space's abbreviation is a resource, not that space.
    let named =
        drawing("/CMYK cs 1 sc 10 10 50 50 re f", "/ColorSpace << /CMYK [/ICCBased 5 0 R] >>", &["<< /N 1 /Length 0 >>\nstream\n\nendstream"]);
    assert!(!uses_cmyk(&named));
}

/// Issue #2259: content is not only the page's own stream: form XObjects (with `/Type /XObject`, as almost
/// every writer gives them), appearance streams, tiling patterns and Type 3 glyphs draw too. A
/// CMYK logo placed as a form must not get the document an sRGB output intent.
#[test]
fn device_colour_in_forms_patterns_and_glyphs_is_found() {
    let cmyk = "0.75 0 1 0 k 0 0 10 10 re f";
    let stream = |dict: &str| format!("<< {dict} /Length {} >>\nstream\n{cmyk}\nendstream", cmyk.len());
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10]");
    let appearance = stream("/BBox [0 0 10 10]");
    let pattern = stream("/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << >>");
    let glyph = format!("<< /Length {} >>\nstream\n1 0 d0 {cmyk}\nendstream", cmyk.len() + 7);
    let cases: [(&str, Document); 4] = [
        ("a form XObject", drawing("/Fm0 Do", "/XObject << /Fm0 5 0 R >>", &[&form])),
        (
            "an appearance stream without /Subtype",
            build(
                &[
                    "<< /Type /Catalog /Pages 2 0 R >>",
                    "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
                    "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
                    "<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /F 4 /P 3 0 R /AP << /N 5 0 R >> >>",
                    &appearance,
                ],
                "",
            ),
        ),
        ("a tiling pattern", drawing("/Pattern cs /P0 scn 0 0 100 100 re f", "/Pattern << /P0 5 0 R >>", &[&pattern])),
        (
            "a Type 3 glyph in an inline font",
            drawing(
                "BT /T3 1 Tf (a) Tj ET",
                "/Font << /T3 << /Type /Font /Subtype /Type3 /FontBBox [0 0 10 10] /FontMatrix [1 0 0 1 0 0] /CharProcs << /a 5 0 R >> /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [10] >> >>",
                &[&glyph],
            ),
        ),
    ];
    for (what, mut doc) in cases {
        assert!(uses_cmyk(&doc), "DeviceCMYK in {what} wasn't found");
        let r = convert(&mut doc, Level::A2b).unwrap();
        assert!(declared(&doc).output_intents.is_empty(), "{what}: an sRGB output intent was added to a CMYK document: {:?}", r.fixed);
    }
}

#[test]
fn standards_identification_uses_namespace_not_prefix() {
    fn with_xmp(xmp: &str) -> Document {
        let stream = format!("<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n{xmp}\nendstream", xmp.len());
        build(&["<< /Type /Catalog /Pages 2 0 R /Metadata 3 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", &stream], "")
    }
    let attribute = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:a="http://www.aiim.org/pdfa/ns/id/" xmlns:u="http://www.aiim.org/pdfua/ns/id/" a:part="2" a:conformance="b" u:part="1"/></rdf:RDF>"#;
    let element = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description><part xmlns="http://www.aiim.org/pdfa/ns/id/">2</part><x:conformance xmlns:x="http://www.aiim.org/pdfa/ns/id/" >B</x:conformance></rdf:Description></rdf:RDF>"#;
    for xml in [attribute, element] {
        let doc = with_xmp(xml);
        assert_eq!(declared(&doc).pdfa, Some((2, "B".into())));
        assert!(!verify(&doc, Level::A2b).iter().any(|i| i.clause == "6.6.4"));
    }
    assert_eq!(declared(&with_xmp(attribute)).pdfua, Some(1));
    let wrong = r#"<root xmlns:pdfaid="urn:wrong" pdfaid:part="2" pdfaid:conformance="B"/>"#;
    let doc = with_xmp(wrong);
    assert_eq!(declared(&doc).pdfa, None);
    assert!(verify(&doc, Level::A2b).iter().any(|i| i.clause == "6.6.4"));
    let nested = format!("{}{}", "<n>".repeat(1100), "</n>".repeat(1100));
    assert_eq!(declared(&with_xmp(&nested)).pdfa, None);
}

#[test]
fn oversized_xmp_is_refused_while_decoding() {
    let xmp = " ".repeat(xmp::MAX_XMP_BYTES + 1);
    let stream = format!("<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n{xmp}\nendstream", xmp.len());
    let doc = build(&["<< /Type /Catalog /Pages 2 0 R /Metadata 3 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", &stream], "");
    assert!(metadata(&doc).is_none());
    assert_eq!(declared(&doc).pdfa, None);

    use std::io::Write as _;
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(xmp.as_bytes()).unwrap();
    let compressed = z.finish().unwrap();
    let hex: String = compressed.iter().map(|byte| format!("{byte:02X}")).collect();
    let data = format!("{hex}>");
    let stream =
        format!("<< /Type /Metadata /Subtype /XML /Filter [/ASCIIHexDecode /FlateDecode] /Length {} >>\nstream\n{data}\nendstream", data.len());
    let doc = build(&["<< /Type /Catalog /Pages 2 0 R /Metadata 3 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", &stream], "");
    let cat = catalog(&doc);
    let raw = doc.resolve(cat.get(b"Metadata").unwrap());
    let Object::Stream(stream) = &*raw else { panic!("fixture has no metadata stream") };
    assert_eq!(stream.decoded().unwrap().len(), xmp::MAX_XMP_BYTES + 1, "fixture must actually decompress");
    assert!(metadata(&doc).is_none(), "compressed metadata must be limited during decoding");
}
