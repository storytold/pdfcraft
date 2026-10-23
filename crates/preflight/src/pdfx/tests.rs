use super::*;

/// A PDF of `objs` (object 1 is the catalog) with `trailer` added to its trailer dictionary.
fn pdf(objs: &[Vec<u8>], trailer: &str) -> Document {
    let mut out = b"%PDF-1.6\n".to_vec();
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
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R {trailer} >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut s = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    s.extend_from_slice(data);
    s.extend_from_slice(b"\nendstream");
    s
}

/// An ICC profile header and tag table: `class` and `space` as ICC.1 spells them, and one empty
/// tag entry per signature in `tags`.
fn icc(class: &[u8; 4], space: &[u8; 4], tags: &[&[u8; 4]]) -> Vec<u8> {
    let size = 132 + 12 * tags.len();
    let mut p = vec![0u8; 128];
    p[0..4].copy_from_slice(&(size as u32).to_be_bytes());
    p[12..16].copy_from_slice(class);
    p[16..20].copy_from_slice(space);
    p[20..24].copy_from_slice(b"XYZ ");
    p[36..40].copy_from_slice(b"acsp");
    p.extend_from_slice(&(tags.len() as u32).to_be_bytes());
    for tag in tags {
        p.extend_from_slice(*tag);
        p.extend_from_slice(&[0; 8]);
    }
    p
}

const ID: &str = "/ID [<00112233445566778899AABBCCDDEEFF> <00112233445566778899AABBCCDDEEFF>]";

/// One page that conforms to `level`: CMYK content, an embedded font, a CMYK printer profile as
/// the PDF/X output intent, a TrimBox inside a BleedBox inside the MediaBox. Each test changes
/// one part of it. Objects from 11 on are `extra`.
#[derive(Clone)]
struct Fixture {
    level: Level,
    /// The document information's entries.
    info: String,
    id: &'static str,
    /// The catalog's /Metadata entry.
    metadata: String,
    catalog: String,
    /// The catalog's /OutputIntents.
    intents: String,
    /// Object 7, the output intent.
    intent: String,
    /// Object 10, the output intent's profile: its dictionary and its bytes.
    profile_dict: String,
    profile: Vec<u8>,
    xmp: String,
    boxes: String,
    page: String,
    resources: String,
    content: String,
    /// Object 5, the font.
    font: String,
    extra: Vec<Vec<u8>>,
}

fn xmp(version: &str, extra: &str) -> String {
    format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
         <rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" \
         xmlns:xmpMM=\"http://ns.adobe.com/xap/1.0/mm/\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\" xmlns:pdfxid=\"http://www.npes.org/pdfx/ns/id/\">\
         <pdfxid:GTS_PDFXVersion>{version}</pdfxid:GTS_PDFXVersion>{extra}</rdf:Description></rdf:RDF></x:xmpmeta>"
    )
}

const XMP_FIELDS: [&str; 7] = [
    "<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Proof</rdf:li></rdf:Alt></dc:title>",
    "<xmp:CreateDate>2026-10-15T09:00:00Z</xmp:CreateDate>",
    "<xmp:ModifyDate>2026-10-15T09:00:00Z</xmp:ModifyDate>",
    "<xmpMM:DocumentID>uuid:6f1d1c4e-0000-4000-8000-000000000001</xmpMM:DocumentID>",
    "<xmpMM:VersionID>1</xmpMM:VersionID>",
    "<xmpMM:RenditionClass>default</xmpMM:RenditionClass>",
    "<pdf:Trapped>False</pdf:Trapped>",
];

impl Fixture {
    fn new(level: Level) -> Fixture {
        let conformance = if level == Level::X1a2001 { " /GTS_PDFXConformance (PDF/X-1a:2001)" } else { "" };
        Fixture {
            level,
            info: format!("/Title (Proof) /GTS_PDFXVersion ({}){conformance} /Trapped /False", level.version_key()),
            id: ID,
            metadata: "/Metadata 8 0 R".into(),
            catalog: String::new(),
            intents: "[7 0 R]".into(),
            intent: "<< /Type /OutputIntent /S /GTS_PDFX /OutputConditionIdentifier (FOGRA39) /RegistryName (http://www.color.org) \
                     /Info (Coated FOGRA39) /DestOutputProfile 10 0 R >>"
                .into(),
            profile_dict: "/N 4".into(),
            profile: icc(b"prtr", b"CMYK", &[b"desc", b"A2B1"]),
            xmp: xmp(level.version_key(), &XMP_FIELDS.concat()),
            boxes: "/MediaBox [0 0 620 820] /BleedBox [1 1 619 819] /TrimBox [10 10 610 810]".into(),
            page: String::new(),
            resources: String::new(),
            content: "0 0 0 1 k 10 10 100 100 re f BT /F1 12 Tf 20 20 Td (Proof) Tj ET".into(),
            font: "<< /Type /Font /Subtype /TrueType /BaseFont /Sans /FirstChar 32 /LastChar 126 /FontDescriptor 6 0 R >>".into(),
            extra: Vec::new(),
        }
    }

    fn doc(&self) -> Document {
        let mut objs = vec![
            format!("<< /Type /Catalog /Pages 2 0 R /OutputIntents {} {} {} >>", self.intents, self.metadata, self.catalog).into_bytes(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            format!(
                "<< /Type /Page /Parent 2 0 R {} /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> {} >> {} >>",
                self.boxes, self.resources, self.page
            )
            .into_bytes(),
            stream("", self.content.as_bytes()),
            self.font.clone().into_bytes(),
            b"<< /Type /FontDescriptor /FontName /Sans /Flags 32 /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 800 /Descent -200 \
              /CapHeight 700 /StemV 80 /FontFile2 9 0 R >>"
                .to_vec(),
            self.intent.clone().into_bytes(),
            stream("/Type /Metadata /Subtype /XML", self.xmp.as_bytes()),
            stream("/Length1 4", b"true"),
            stream(&self.profile_dict, &self.profile),
        ];
        objs.extend(self.extra.iter().cloned());
        pdf(&objs, &format!("/Info << {} >> {}", self.info, self.id))
    }
}

/// What `verify` finds, each finding checked against the wordings an interface translates.
fn findings(doc: &Document, level: Level) -> Vec<Finding> {
    let found = verify(doc, level);
    for f in &found {
        assert!(WORDINGS.contains(&f.pattern), "{:?} is not in WORDINGS", f.pattern);
        assert_eq!(f.said(&crate::in_english), f.message);
        assert!(f.rule.starts_with("pdfx.") && !f.clause.is_empty());
    }
    found
}

/// The finding with `pattern`, which must be there, and every finding of `rule`.
#[track_caller]
fn only(fixture: &Fixture, rule: &str, pattern: &str) -> Finding {
    let level = fixture.level;
    let found = findings(&fixture.doc(), level);
    assert!(found.iter().all(|f| f.rule == rule), "{}: expected only {rule}: {found:#?}", level.label());
    found.into_iter().find(|f| f.pattern == pattern).unwrap_or_else(|| panic!("{}: no {pattern:?}", level.label()))
}

#[track_caller]
fn clean(fixture: &Fixture) {
    let found = findings(&fixture.doc(), fixture.level);
    assert!(found.is_empty(), "{}: {found:#?}", fixture.level.label());
}

#[test]
fn each_level_accepts_a_conforming_page() {
    for level in Level::ALL {
        clean(&Fixture::new(level));
        let doc = Fixture::new(level).doc();
        assert_eq!(declared(&doc), Some(level));
        // The Standards panel names the level, PDF/X-1a:2001 by its conformance level.
        assert_eq!(crate::declared(&doc).pdfx.as_deref(), Some(level.label()));
    }
}

#[test]
fn levels_read_their_ids_and_labels() {
    for level in Level::ALL {
        assert_eq!(Level::from_id(level.label()), Some(level));
        assert!(level.standard().starts_with("ISO 15930-"));
    }
    assert_eq!(Level::from_id("x1a"), Some(Level::X1a2003));
    assert_eq!(Level::from_id("X3"), Some(Level::X3_2003));
    assert_eq!(Level::from_id("x1a2001"), Some(Level::X1a2001));
    assert_eq!(Level::from_id("pdfx-4"), Some(Level::X4));
    assert_eq!(Level::from_id("x5"), None);
    assert_eq!(Level::from_version("PDF/X-3:2002"), Some(Level::X3_2002));
}

#[test]
fn identification_names_the_level() {
    // PDF/X-1a and -3 identify themselves in the document information.
    let mut f = Fixture::new(Level::X1a2003);
    f.info = "/Trapped /False /GTS_PDFXVersion (PDF/X-3:2003)".into();
    let found = only(&f, "pdfx.identification", "The document information doesn't identify the file as {}");
    assert_eq!(found.value.as_deref(), Some("PDF/X-1a:2003"));
    f.info = "/Trapped /False".into();
    only(&f, "pdfx.identification", "The document information doesn't identify the file as {}");
    // PDF/X-1a:2001 is a conformance level of PDF/X-1:2001, and says which.
    let mut f = Fixture::new(Level::X1a2001);
    f.info = f.info.replace("/GTS_PDFXConformance (PDF/X-1a:2001)", "");
    only(&f, "pdfx.identification", "The document information doesn't identify the file as {}");
    assert_eq!(declared(&f.doc()), None, "PDF/X-1:2001 without a conformance level");
    // PDF/X-4 identifies itself in its metadata, and the document information may not disagree.
    let mut f = Fixture::new(Level::X4);
    f.xmp = xmp("PDF/X-3:2003", &XMP_FIELDS.concat());
    only(&f, "pdfx.identification", "The metadata doesn't identify the file as {}");
    let mut f = Fixture::new(Level::X4);
    f.info = f.info.replace("(PDF/X-4)", "(PDF/X-3:2003)");
    only(&f, "pdfx.identification", "The document information and the metadata name different PDF/X versions");
    let mut f = Fixture::new(Level::X4);
    f.metadata = String::new();
    f.info = "/Trapped /False".into();
    only(&f, "pdfx.identification", "The document has no XMP metadata");
}

#[test]
fn pdfx4_metadata_has_its_title_dates_and_identity() {
    for (i, field) in XMP_FIELDS.iter().enumerate().take(6) {
        let mut f = Fixture::new(Level::X4);
        let mut fields = XMP_FIELDS.to_vec();
        fields.remove(i);
        f.xmp = xmp("PDF/X-4", &fields.concat());
        let found = only(&f, "pdfx.metadata", "The metadata has no {}");
        let property = found.value.unwrap();
        assert!(field.contains(&format!("<{property}>")), "{property} for {field}");
    }
    // Older packets spell the XMP schemas `xap`.
    let mut f = Fixture::new(Level::X4);
    f.xmp = f.xmp.replace("xmp:", "xap:").replace("xmpMM:", "xapMM:");
    clean(&f);
    // An empty title is no title.
    let mut f = Fixture::new(Level::X4);
    f.xmp = f.xmp.replace(">Proof<", "><");
    assert_eq!(only(&f, "pdfx.metadata", "The metadata has no {}").value.as_deref(), Some("dc:title"));
    // The other levels don't ask for it.
    let mut f = Fixture::new(Level::X3_2003);
    f.xmp = xmp("PDF/X-3:2003", "");
    clean(&f);
}

#[test]
fn the_trailer_has_a_document_id() {
    for level in Level::ALL {
        let mut f = Fixture::new(level);
        f.id = "";
        only(&f, "pdfx.trailer-id", "The file trailer has no document ID");
        f.id = "/ID [<00112233>]";
        only(&f, "pdfx.trailer-id", "The file trailer has no document ID");
    }
}

#[test]
fn trapped_is_true_or_false() {
    let mut f = Fixture::new(Level::X1a2003);
    f.info = f.info.replace("/Trapped /False", "");
    only(&f, "pdfx.trapped", "The document doesn't say whether it is trapped (Trapped)");
    for (written, shown) in [("/Unknown", "/Unknown"), ("true", "true"), ("(True)", "(True)")] {
        f.info = format!("/GTS_PDFXVersion (PDF/X-1a:2003) /Trapped {written}");
        let found = only(&f, "pdfx.trapped", "Trapped is {}, but it must be /True or /False");
        assert_eq!(found.value.as_deref(), Some(shown));
    }
    f.info = "/GTS_PDFXVersion (PDF/X-1a:2003) /Trapped /True".into();
    clean(&f);
    // PDF/X-4 may say it in its metadata alone; where both say it they agree.
    let mut f = Fixture::new(Level::X4);
    f.info = "/GTS_PDFXVersion (PDF/X-4)".into();
    clean(&f);
    f.xmp = f.xmp.replace("<pdf:Trapped>False", "<pdf:Trapped>Unknown");
    assert_eq!(only(&f, "pdfx.trapped", "Trapped is {}, but it must be /True or /False").value.as_deref(), Some("Unknown"));
    let mut f = Fixture::new(Level::X4);
    f.info = f.info.replace("/Trapped /False", "/Trapped /True");
    only(&f, "pdfx.trapped", "The Trapped key and the metadata's pdf:Trapped disagree");
    let mut f = Fixture::new(Level::X4);
    f.info = "/GTS_PDFXVersion (PDF/X-4)".into();
    f.xmp = f.xmp.replace("<pdf:Trapped>False</pdf:Trapped>", "");
    only(&f, "pdfx.trapped", "The document doesn't say whether it is trapped (Trapped)");
}

#[test]
fn an_encrypted_file_is_not_pdfx() {
    for level in Level::ALL {
        let mut doc = Fixture::new(level).doc();
        doc.trailer_mut().set(b"Encrypt".to_vec(), Object::Ref(ObjRef::new(11, 0)));
        let found = findings(&doc, level);
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!((found[0].rule, found[0].pattern), ("pdfx.encryption", "The document is encrypted"));
    }
}

#[test]
fn there_is_one_pdfx_output_intent() {
    for level in Level::ALL {
        let mut f = Fixture::new(level);
        f.intents = "[]".into();
        only(&f, "pdfx.output-intent", "There is no PDF/X output intent (GTS_PDFX)");
        f.intents = "[<< /Type /OutputIntent /S /GTS_PDFA1 /OutputConditionIdentifier (sRGB) >>]".into();
        only(&f, "pdfx.output-intent", "There is no PDF/X output intent (GTS_PDFX)");
        f.intents = "[7 0 R << /Type /OutputIntent /S /GTS_PDFX /OutputConditionIdentifier (Other) /DestOutputProfile 10 0 R >>]".into();
        only(&f, "pdfx.output-intent", "There is more than one PDF/X output intent");
        let mut f = Fixture::new(level);
        f.intent = f.intent.replace("/OutputConditionIdentifier (FOGRA39)", "");
        assert_eq!(only(&f, "pdfx.output-intent", "The PDF/X output intent has no OutputConditionIdentifier").object, Some(7));
    }
}

#[test]
fn the_output_intent_embeds_its_profile_or_names_a_registered_one() {
    let without = |level: Level, registry: bool| {
        let mut f = Fixture::new(level);
        f.intent = f.intent.replace("/DestOutputProfile 10 0 R", "");
        if !registry {
            f.intent = f.intent.replace("/RegistryName (http://www.color.org)", "");
        }
        f
    };
    // PDF/X-4 always embeds it.
    only(&without(Level::X4, true), "pdfx.output-intent", "The PDF/X output intent has no embedded profile (DestOutputProfile)");
    for level in [Level::X1a2001, Level::X1a2003, Level::X3_2002, Level::X3_2003] {
        // A characterization in the ICC registry stands in for it (CGATS notes 3.3, 5.1).
        clean(&without(level, true));
        only(
            &without(level, false),
            "pdfx.output-intent",
            "The PDF/X output intent has neither an embedded profile nor a characterization in the ICC registry",
        );
    }
    // Unless PDF/X-3 content uses device-independent colour.
    let mut f = without(Level::X3_2003, true);
    f.resources = "/ColorSpace << /CS0 [/ICCBased 10 0 R] >>".into();
    only(&f, "pdfx.output-intent", "Device-independent colour is used, so the output intent needs an embedded profile");
}

#[test]
fn the_output_profile_is_a_printer_profile_the_level_allows() {
    let with = |level: Level, n: u8, profile: Vec<u8>, content: &str| {
        let mut f = Fixture::new(level);
        f.profile_dict = format!("/N {n}");
        f.profile = profile;
        f.content = content.into();
        f
    };
    let cmyk = "0 0 0 1 k 0 0 10 10 re f";
    for level in Level::ALL {
        let found =
            only(&with(level, 4, b"not a profile".to_vec(), cmyk), "pdfx.output-profile", "The output intent's profile isn't a valid ICC profile");
        assert_eq!(found.object, Some(10));
        // A tag count the profile can't hold.
        let mut damaged = icc(b"prtr", b"CMYK", &[b"A2B1"]);
        damaged[128..132].copy_from_slice(&u32::MAX.to_be_bytes());
        only(&with(level, 4, damaged, cmyk), "pdfx.output-profile", "The output intent's profile isn't a valid ICC profile");
        only(
            &with(level, 4, icc(b"mntr", b"CMYK", &[b"A2B1"]), cmyk),
            "pdfx.output-profile",
            "The output intent's profile isn't an output (printer) profile",
        );
        only(
            &with(level, 3, icc(b"prtr", b"CMYK", &[b"A2B1"]), cmyk),
            "pdfx.output-profile",
            "The output intent's profile has /N {}, which doesn't match its colour space",
        );
        only(
            &with(level, 4, icc(b"prtr", b"CMYK", &[b"desc", b"B2A1"]), cmyk),
            "pdfx.output-profile",
            "The output intent's profile has no device-to-PCS transform (AToB1 or grayTRC)",
        );
    }
    // PDF/X-1a prints on CMYK; PDF/X-3 and -4 on gray, RGB or CMYK.
    for level in [Level::X1a2001, Level::X1a2003] {
        let found = only(
            &with(level, 3, icc(b"prtr", b"RGB ", &[b"A2B1"]), cmyk),
            "pdfx.output-profile",
            "The output intent's profile describes {} colour, but PDF/X-1a needs a CMYK printing condition",
        );
        assert_eq!(found.value.as_deref(), Some("RGB"));
    }
    for level in [Level::X3_2002, Level::X3_2003, Level::X4] {
        clean(&with(level, 3, icc(b"prtr", b"RGB ", &[b"A2B1"]), "1 0 0 rg 0 0 10 10 re f"));
        clean(&with(level, 1, icc(b"prtr", b"GRAY", &[b"kTRC"]), "0.5 g 0 0 10 10 re f"));
        let found = only(
            &with(level, 3, icc(b"prtr", b"Lab ", &[b"A2B1"]), cmyk),
            "pdfx.output-profile",
            "The output intent's profile describes {} colour; it must be gray, RGB or CMYK",
        );
        assert_eq!(found.value.as_deref(), Some("Lab"));
    }
}

#[test]
fn page_boxes_nest() {
    let boxes = |level: Level, boxes: &str| {
        let mut f = Fixture::new(level);
        f.boxes = boxes.into();
        f
    };
    for level in Level::ALL {
        let page = only(&boxes(level, "/MediaBox [0 0 620 820]"), "pdfx.page-boxes", "The page has neither a TrimBox nor an ArtBox");
        assert_eq!((page.page, page.object), (Some(0), Some(3)));
        only(
            &boxes(level, "/MediaBox [0 0 620 820] /TrimBox [10 10 610 810] /ArtBox [10 10 610 810]"),
            "pdfx.page-boxes",
            "The page has both a TrimBox and an ArtBox",
        );
        // An ArtBox alone does, and the corners may come in any order.
        clean(&boxes(level, "/MediaBox [620 820 0 0] /BleedBox [1 1 619 819] /ArtBox [610 810 10 10]"));
        // Within the BleedBox, the CropBox and the MediaBox.
        let found = only(
            &boxes(level, "/MediaBox [0 0 620 820] /BleedBox [5 5 615 815] /TrimBox [0 0 610 810]"),
            "pdfx.page-boxes",
            "The {} extends beyond the BleedBox",
        );
        assert_eq!(found.value.as_deref(), Some("TrimBox"));
        only(
            &boxes(level, "/MediaBox [0 0 620 820] /CropBox [20 20 600 800] /ArtBox [10 10 610 810]"),
            "pdfx.page-boxes",
            "The {} extends beyond the CropBox",
        );
        only(&boxes(level, "/MediaBox [0 0 620 820] /TrimBox [10 10 630 810]"), "pdfx.page-boxes", "The {} extends beyond the MediaBox");
        let found = only(
            &boxes(level, "/MediaBox [0 0 620 820] /BleedBox [-5 -5 619 819] /TrimBox [10 10 610 810]"),
            "pdfx.page-boxes",
            "The {} extends beyond the MediaBox",
        );
        assert_eq!(found.value.as_deref(), Some("BleedBox"));
        // A writer's rounding is not a box out of place.
        clean(&boxes(level, "/MediaBox [0 0 620 820] /BleedBox [1 1 619 819] /TrimBox [0.999 10 610.004 810]"));
        only(&boxes(level, "/TrimBox [10 10 610 810]"), "pdfx.page-boxes", "The page has no valid MediaBox");
        only(&boxes(level, "/MediaBox [0 0 620] /TrimBox [10 10 610 810]"), "pdfx.page-boxes", "The page has no valid MediaBox");
    }
}

#[test]
fn annotations_stay_outside_the_bleed() {
    let annotated = |level: Level, annot: &str| {
        let mut f = Fixture::new(level);
        f.page = "/Annots [11 0 R]".into();
        f.extra = vec![annot.as_bytes().to_vec()];
        f
    };
    for level in Level::ALL {
        let found = only(
            &annotated(level, "<< /Type /Annot /Subtype /Text /Rect [100 100 120 120] /F 4 >>"),
            "pdfx.annotations",
            "A {} annotation lies within the BleedBox",
        );
        assert_eq!((found.value.as_deref(), found.page, found.object), (Some("Text"), Some(0), Some(11)));
        // Between the BleedBox and the MediaBox, or off the page, is outside.
        clean(&annotated(level, "<< /Type /Annot /Subtype /Text /Rect [0 0 0.9 0.9] /F 4 >>"));
        clean(&annotated(level, "<< /Type /Annot /Subtype /Link /Rect [700 700 720 720] >>"));
        // A printer mark may be in the bleed, but not in the TrimBox.
        clean(&annotated(level, "<< /Type /Annot /Subtype /PrinterMark /Rect [2 2 9 9] /F 4 >>"));
        only(
            &annotated(level, "<< /Type /Annot /Subtype /PrinterMark /Rect [5 5 20 20] /F 4 >>"),
            "pdfx.annotations",
            "A PrinterMark annotation overlaps the TrimBox or ArtBox",
        );
        // Pop-ups, trap networks and annotations with no area draw nothing of their own.
        clean(&annotated(level, "<< /Type /Annot /Subtype /Popup /Rect [100 100 300 200] >>"));
        clean(&annotated(level, "<< /Type /Annot /Subtype /TrapNet /Rect [0 0 620 820] /F 4 >>"));
        clean(&annotated(level, "<< /Type /Annot /Subtype /Link /Rect [100 100 100 120] >>"));
    }
    // Without a BleedBox the bleed is the CropBox.
    let mut f = annotated(Level::X4, "<< /Type /Annot /Subtype /Widget /Rect [605 400 615 410] >>");
    f.boxes = "/MediaBox [0 0 620 820] /CropBox [0 0 620 820] /TrimBox [10 10 600 810]".into();
    only(&f, "pdfx.annotations", "A {} annotation lies within the BleedBox");
}

#[test]
fn fonts_are_embedded() {
    for level in Level::ALL {
        let mut f = Fixture::new(level);
        f.font = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into();
        let found = only(&f, "pdfx.fonts", "The font {} isn't embedded");
        assert_eq!((found.value.as_deref(), found.object), (Some("Helvetica"), Some(5)));
    }
}

#[test]
fn no_lzw_and_no_jbig2_before_pdfx4() {
    let image = |filter: &str| {
        stream(&format!("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter {filter}"), b"\x80")
    };
    for level in Level::ALL {
        let mut f = Fixture::new(level);
        f.extra = vec![image("[/LZWDecode]")];
        assert_eq!(only(&f, "pdfx.compression", "A stream uses LZW compression").object, Some(11));
        f.extra = vec![image("/JBIG2Decode")];
        if level == Level::X4 {
            clean(&f);
        } else {
            only(&f, "pdfx.compression", "A stream uses JBIG2 compression");
        }
    }
}

#[test]
fn everything_is_in_the_file() {
    for level in Level::ALL {
        let mut f = Fixture::new(level);
        f.extra = vec![b"<< /F (picture.tif) /Length 0 >>\nstream\n\nendstream".to_vec()];
        only(&f, "pdfx.external-content", "A stream refers to an external file");
        f.extra = vec![stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Ref << /F (ad.pdf) /Page 0 >>", b"")];
        only(&f, "pdfx.external-content", "A form XObject refers to a page of another file (reference XObject)");
        f.extra = vec![stream(
            "/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /OPI << /2.0 << /Type /OPI /Version 2.0 /F (hires.tif) >> >>",
            b"\0",
        )];
        only(&f, "pdfx.external-content", "An image or form has OPI information");
    }
}

#[test]
fn no_postscript() {
    for level in Level::ALL {
        let mut f = Fixture::new(level);
        f.extra = vec![stream("/Type /XObject /Subtype /PS", b"showpage")];
        only(&f, "pdfx.postscript", "PostScript XObjects aren't allowed");
        f.extra = vec![stream("/Type /XObject /Subtype /Form /Subtype2 /PS /BBox [0 0 1 1]", b"")];
        only(&f, "pdfx.postscript", "PostScript XObjects aren't allowed");
    }
}

#[test]
fn no_javascript() {
    for level in Level::ALL {
        let mut f = Fixture::new(level);
        f.catalog = "/Names << /JavaScript << /Names [(init) 11 0 R] >> >>".into();
        f.extra = vec![b"<< /Type /Action /S /JavaScript /JS (this.print()) >>".to_vec()];
        let found = findings(&f.doc(), level);
        assert!(found.iter().all(|x| x.rule == "pdfx.javascript"), "{found:#?}");
        assert!(found.iter().any(|x| x.pattern == "The document has document-level JavaScript"));
        assert!(found.iter().any(|x| x.message == "A JavaScript action isn't allowed" && x.object == Some(11)));
        // Written into the catalog's open action, or into a link off the page.
        let mut f = Fixture::new(level);
        f.catalog = "/OpenAction << /S /JavaScript /JS (1) >>".into();
        only(&f, "pdfx.javascript", "A {} action isn't allowed");
        let mut f = Fixture::new(level);
        f.page = "/Annots [11 0 R]".into();
        f.extra = vec![b"<< /Type /Annot /Subtype /Link /Rect [700 700 720 720] /A << /S /JavaScript /JS (1) >> >>".to_vec()];
        assert_eq!(only(&f, "pdfx.javascript", "A {} action isn't allowed").page, Some(0));
    }
}

#[test]
fn nothing_runs_by_itself() {
    for level in Level::ALL {
        let mut f = Fixture::new(level);
        f.catalog = "/AA << /WP 11 0 R >>".into();
        f.extra = vec![b"<< /Type /Action /S /GoTo /D [3 0 R /Fit] >>".to_vec()];
        only(&f, "pdfx.actions", "The document has additional actions (/AA)");
        let mut f = Fixture::new(level);
        f.page = "/AA << /O << /S /GoTo /D [3 0 R /Fit] >> >>".into();
        assert_eq!(only(&f, "pdfx.actions", "Page {} has additional actions").message, "Page 1 has additional actions");
        let mut f = Fixture::new(level);
        f.page = "/Annots [11 0 R]".into();
        f.extra =
            vec![b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [0 0 0.9 0.9] /AA << /K << /S /GoTo /D [3 0 R /Fit] >> >> >>".to_vec()];
        only(&f, "pdfx.actions", "A {} annotation has additional actions");
        let mut f = Fixture::new(level);
        f.extra = vec![b"<< /FT /Tx /T (name) /Kids [] /AA << /C << /S /GoTo /D [3 0 R /Fit] >> >> >>".to_vec()];
        only(&f, "pdfx.actions", "A form field has additional actions");
        // A plain link action, off the page, is fine.
        let mut f = Fixture::new(level);
        f.page = "/Annots [11 0 R]".into();
        f.extra = vec![b"<< /Type /Annot /Subtype /Link /Rect [700 700 720 720] /A << /S /URI /URI (https://example.org) >> >>".to_vec()];
        clean(&f);
    }
}

#[test]
fn transparency_only_in_pdfx4() {
    let with = |level: Level, resources: &str, extra: Vec<Vec<u8>>| {
        let mut f = Fixture::new(level);
        f.resources = resources.into();
        f.extra = extra;
        f
    };
    let soft = stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"\x80");
    let masked = stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceCMYK /BitsPerComponent 8 /SMask 11 0 R", b"\0\0\0\xff");
    let cases: [(&str, Vec<Vec<u8>>, &str); 6] = [
        ("/ExtGState << /GS0 << /ca 0.5 >> >>", vec![], "A graphics state sets an opacity below 1"),
        ("/ExtGState << /GS0 << /CA 0 >> >>", vec![], "A graphics state sets an opacity below 1"),
        ("/ExtGState << /GS0 << /BM /Multiply >> >>", vec![], "A graphics state uses the {} blend mode"),
        ("/ExtGState << /GS0 << /BM [/Compatible /Screen] >> >>", vec![], "A graphics state uses the {} blend mode"),
        (
            "/ExtGState << /GS0 11 0 R >>",
            vec![b"<< /SMask << /S /Luminosity /G 12 0 R >> >>".to_vec(), stream("/Subtype /Form /BBox [0 0 1 1]", b"")],
            "A graphics state uses a soft mask",
        ),
        ("/XObject << /Im0 12 0 R >>", vec![soft, masked], "An image has a soft mask"),
    ];
    for (resources, extra, pattern) in cases {
        for level in Level::ALL {
            let f = with(level, resources, extra.clone());
            if level == Level::X4 {
                clean(&f);
            } else {
                only(&f, "pdfx.transparency", pattern);
            }
        }
    }
    // Opaque, normal blending and no soft mask is not transparency.
    clean(&with(Level::X1a2003, "/ExtGState << /GS0 << /ca 1 /CA 1.0 /BM /Normal /SMask /None >> /GS1 << /BM [/Compatible] >> >>", vec![]));
}

#[test]
fn pdfx1a_is_cmyk_gray_and_spot_only() {
    let tint = "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 0.5 1 0] /N 1 >>";
    let rgb_tint = "<< /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [1 0.5 0] /N 1 >>";
    let image =
        |space: &str| stream(&format!("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace {space} /BitsPerComponent 8"), b"\0\0\0");
    for level in [Level::X1a2001, Level::X1a2003] {
        let with = |resources: String, content: &str, extra: Vec<Vec<u8>>| {
            let mut f = Fixture::new(level);
            f.resources = resources;
            f.content = content.into();
            f.extra = extra;
            f
        };
        let not_allowed = "The {} colour space isn't allowed: PDF/X-1a allows only CMYK, gray and spot colour";
        // An image's own space, with the object it is in.
        let found = only(&with("/XObject << /Im0 11 0 R >>".into(), "/Im0 Do", vec![image("/DeviceRGB")]), "pdfx.colour", not_allowed);
        assert_eq!((found.value.as_deref(), found.object), (Some("DeviceRGB"), Some(11)));
        // A space a resource dictionary lists, on the page that lists it.
        for (space, family) in [
            ("[/ICCBased 10 0 R]", "ICCBased"),
            ("[/Lab << /WhitePoint [0.9505 1 1.089] >>]", "Lab"),
            ("[/CalRGB << /WhitePoint [0.9505 1 1.089] >>]", "CalRGB"),
        ] {
            let found = only(&with(format!("/ColorSpace << /CS0 {space} >>"), "/CS0 cs 1 0 0 sc 0 0 1 1 re f", vec![]), "pdfx.colour", not_allowed);
            assert_eq!((found.value.as_deref(), found.page, found.object), (Some(family), Some(0), Some(3)));
        }
        // A spot colour drawn in RGB where its ink is missing, or an indexed image over RGB.
        let spot = format!("/ColorSpace << /CS0 [/Separation /Orange /DeviceRGB {rgb_tint}] >>");
        assert_eq!(only(&with(spot, "/CS0 cs 1 scn 0 0 1 1 re f", vec![]), "pdfx.colour", not_allowed).value.as_deref(), Some("DeviceRGB"));
        only(&with("/XObject << /Im0 11 0 R >>".into(), "/Im0 Do", vec![image("[/Indexed /DeviceRGB 0 <FF8000>]")]), "pdfx.colour", not_allowed);
        // RGB set by content operators.
        only(
            &with(String::new(), "1 0 0 rg 0 0 10 10 re f", vec![]),
            "pdfx.colour",
            "Content uses DeviceRGB colour: PDF/X-1a allows only CMYK, gray and spot colour",
        );
        // CMYK, gray and spot colour over CMYK are fine.
        clean(&with(
            format!("/ColorSpace << /CS0 [/Separation /Orange /DeviceCMYK {tint}] /CS1 [/DeviceN [/Orange /Black] /DeviceCMYK {tint}] >>"),
            "0.5 g /CS0 cs 1 scn 0 0 1 1 re f",
            vec![],
        ));
        clean(&with("/XObject << /Im0 11 0 R >>".into(), "/Im0 Do", vec![image("/DeviceGray")]));
    }
}

#[test]
fn pdfx3_and_4_device_colour_matches_the_output_intent() {
    for level in [Level::X3_2002, Level::X3_2003, Level::X4] {
        let with = |resources: &str, content: &str, extra: Vec<Vec<u8>>| {
            let mut f = Fixture::new(level);
            f.resources = resources.into();
            f.content = content.into();
            f.extra = extra;
            f
        };
        // Under a CMYK output intent: RGB set by content, or an RGB image.
        let found = only(
            &with("", "1 0 0 rg 0 0 10 10 re f", vec![]),
            "pdfx.colour",
            "Content uses {} colour, which doesn't match the output intent's colour space",
        );
        assert_eq!(found.value.as_deref(), Some("DeviceRGB"));
        let image = stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"\0\0\0");
        let found = only(
            &with("/XObject << /Im0 11 0 R >>", "/Im0 Do", vec![image]),
            "pdfx.colour",
            "{} colour doesn't match the output intent's colour space",
        );
        assert_eq!(found.object, Some(11));
        // DefaultRGB maps DeviceRGB into a device-independent space, which the profile renders.
        clean(&with(
            "/ColorSpace << /DefaultRGB [/ICCBased 11 0 R] >>",
            "1 0 0 rg 0 0 10 10 re f",
            vec![stream("/N 3", &icc(b"mntr", b"RGB ", &[b"A2B0"]))],
        ));
        // Device-independent colour, gray and spot colour are fine.
        clean(&with("/ColorSpace << /CS0 [/Lab << /WhitePoint [0.9505 1 1.089] >>] >>", "0.5 g /CS0 cs 50 0 0 sc 0 0 1 1 re f", vec![]));
        // Under an RGB output intent, CMYK and gray are what don't match.
        let mut f = with("", "0 0 0 1 k 0.5 g 0 0 10 10 re f", vec![]);
        f.profile_dict = "/N 3".into();
        f.profile = icc(b"prtr", b"RGB ", &[b"A2B1"]);
        let found = findings(&f.doc(), level);
        let values: Vec<_> = found.iter().map(|x| (x.rule, x.value.as_deref())).collect();
        assert_eq!(values, [("pdfx.colour", Some("DeviceGray")), ("pdfx.colour", Some("DeviceCMYK"))], "{found:#?}");
    }
}

/// Every problem the rules can report is listed in `WORDINGS`, so an interface showing them in
/// another language has something to translate.
#[test]
fn wordings_are_complete() {
    let source = include_str!("../pdfx.rs");
    let source = source.split("#[cfg(test)]").next().unwrap_or_default();
    let mut said = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find(".add(") {
        // Step over the rule, then take the wording if it is written here.
        let after = rest[at + 5..].trim_start();
        let rule = after.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(after.len());
        let after = after[rule..].trim_start().strip_prefix(',').unwrap_or(after).trim_start();
        rest = after;
        let Some(text) = after.strip_prefix('"') else { continue };
        let Some(end) = text.find('"') else { break };
        said.push(&text[..end]);
        rest = &text[end..];
    }
    assert!(said.len() >= 45, "the wordings are not being read off the file: {said:#?}");
    let missing: Vec<&&str> = said.iter().filter(|w| !WORDINGS.contains(w)).collect();
    assert!(missing.is_empty(), "add these to WORDINGS: {missing:#?}");
    // The two font wordings come from PDF/A's font check.
    let unsaid: Vec<&&str> = WORDINGS.iter().filter(|w| !said.contains(w) && !w.starts_with("The font") && !w.starts_with("A font")).collect();
    assert!(unsaid.is_empty(), "WORDINGS lists problems the rules no longer report: {unsaid:#?}");
}

/// Damaged files are verified, not a reason to stop: nothing here may panic.
#[test]
fn damaged_documents_are_verified() {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /OutputIntents 5 0 R /Metadata 7 0 R /Names << /JavaScript 3 >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 3 0 R 4 0 R] /Count 3 /MediaBox [0 0 1e308 -1e308] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /TrimBox [0 0 (a) 1] /BleedBox 9 0 R /Annots [null 3 0 R << /Subtype /Text /Rect [1 1] >>] \
          /Resources << /ColorSpace << /A [/Indexed [/Indexed [/Indexed [/Indexed [/Indexed [/Indexed /DeviceRGB]]]]]] /B 6 0 R >> /ExtGState << /G 3 0 R >> >> >>"
            .to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /ArtBox [0 0 10 10] /TrimBox [0 0 10 10] >>".to_vec(),
        b"[<< /S /GTS_PDFX /DestOutputProfile 8 0 R >> << /S /GTS_PDFX /DestOutputProfile 3 >> 42]".to_vec(),
        b"[/Separation /S 6 0 R null]".to_vec(),
        b"<< /Length 5 >>\nstream\n<rdf:\nendstream".to_vec(),
        stream("/N 99", &icc(b"prtr", b"CMYK", &[])[..130]),
    ];
    let doc = pdf(&objs, "/Info << /Trapped 3 0 R >> /ID 4");
    for level in Level::ALL {
        let found = findings(&doc, level);
        assert!(!found.is_empty());
    }
}
