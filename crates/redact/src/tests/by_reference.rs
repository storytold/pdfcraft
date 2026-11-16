//! Objects redaction reads that are given by reference where they may be: a `/Subtype` given by
//! reference (ISO 32000-2 §7.3.10), and an XObject entry that reaches its stream through a
//! reference to a reference. The renderer reads both, so redaction does too.

use super::*;

const IMAGE: &str = "/Type /XObject /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8";
const HELVETICA: &[u8] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>";

/// A 300 × 300 page of `content` with `resources`, and objects from 5 on: `extra`.
fn page(content: &[u8], resources: &str, extra: Vec<Vec<u8>>) -> Document {
    let mut objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << {resources} >> >>").into_bytes(),
        stream("", content),
    ];
    objs.extend(extra);
    pdf(objs)
}

/// Whether a full save of `doc` holds `needle` in any stream's decoded data.
fn saved_holds(doc: &Document, needle: &[u8]) -> bool {
    let saved = Document::open(Arc::new(write_full(doc, &SaveOptions::default()).unwrap())).unwrap();
    saved.object_numbers().into_iter().any(|n| match &*saved.try_get(n).unwrap() {
        Object::Stream(s) => s.decoded().unwrap_or_default().windows(needle.len()).any(|w| w == needle),
        _ => false,
    })
}

/// A form, and an image, under the mark, each with its `/Subtype` given by reference, or reached
/// through a reference to a reference: the form's text goes, and so does the image, and the saved
/// file holds neither, as with both given directly.
#[test]
fn xobjects_given_by_reference_under_the_mark_are_redacted() {
    let form = |subtype: &str| {
        stream(
            &format!("/Type /XObject /Subtype {subtype} /BBox [0 0 300 300] /Resources << /Font << /F1 7 0 R >> >>"),
            b"BT /F1 12 Tf 20 250 Td (FORMSECRET) Tj ET",
        )
    };
    let image = |subtype: &str| stream(&format!("{IMAGE} /Subtype {subtype}"), b"IMAGESECRET_BYTES!");
    let draw_image = b"q 100 0 0 30 20 240 cm /X Do Q".as_slice();
    // Each: what it is, the page's content, its objects from 5 on, its secret, and how many glyphs
    // or images go.
    type Row<'a> = (&'a str, &'a [u8], Vec<Vec<u8>>, &'a [u8], usize);
    let rows: [Row<'_>; 6] = [
        ("a form", b"/X Do", vec![form("/Form"), b"null".to_vec(), HELVETICA.to_vec()], b"FORMSECRET", 10),
        ("a form, /Subtype by reference", b"/X Do", vec![form("6 0 R"), b"/Form".to_vec(), HELVETICA.to_vec()], b"FORMSECRET", 10),
        ("a form through a reference to a reference", b"/X Do", vec![b"6 0 R".to_vec(), form("/Form"), HELVETICA.to_vec()], b"FORMSECRET", 10),
        ("an image", draw_image, vec![image("/Image")], b"IMAGESECRET", 1),
        ("an image, /Subtype by reference", draw_image, vec![image("6 0 R"), b"/Image".to_vec()], b"IMAGESECRET", 1),
        ("an image through a reference to a reference", draw_image, vec![b"6 0 R".to_vec(), image("/Image")], b"IMAGESECRET", 1),
    ];
    for (label, content, objs, secret, removed) in rows {
        let mut doc = page(content, "/XObject << /X 5 0 R >>", objs);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        let report = apply(&mut doc, None).unwrap();
        assert_eq!(report.glyphs + report.images_removed, removed, "{label}: {report:?}");
        assert!(!saved_holds(&doc, secret), "{label}");
    }
}

/// Text in a Type 3 font whose glyphs are 40 units wide, starting left of the mark: glyphs 2 to 5
/// lie under it. With the font's `/Subtype` given by reference, it is still measured as a Type 3
/// font, so the same four go; read as a Type 1 font, each would be 0.04 wide, and none would.
#[test]
fn a_type3_font_given_its_subtype_by_reference_is_placed_by_its_widths() {
    for subtype in ["/Type3", "7 0 R"] {
        let font = format!(
            "<< /Type /Font /Subtype {subtype} /FontBBox [0 0 40 40] /FontMatrix [1 0 0 1 0 0] /CharProcs << /a 6 0 R >> /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [40] >>"
        );
        let mut doc = page(
            b"BT /T3 1 Tf 0 250 Td (aaaaa) Tj ET",
            "/Font << /T3 5 0 R >>",
            vec![font.into_bytes(), stream("", b"40 0 d0 0 0 40 40 re f"), b"/Type3".to_vec()],
        );
        mark(&mut doc, 0, &[[50.0, 230.0, 300.0, 280.0]], "");
        assert_eq!(apply(&mut doc, None).map(|r| r.glyphs), Ok(4), "{subtype}");
    }
}

/// A composite font writing vertically, its `/Subtype` given by reference: still read as one, so
/// its text under the mark refuses the redaction, as with the `/Subtype` given directly.
#[test]
fn a_vertical_composite_font_given_its_subtype_by_reference_refuses_the_redaction() {
    for subtype in ["/Type0", "6 0 R"] {
        let font = format!(
            "<< /Type /Font /Subtype {subtype} /BaseFont /X /Encoding /Identity-V /DescendantFonts [<< /Type /Font /Subtype /CIDFontType2 /BaseFont /X /DW 1000 >>] >>"
        );
        let mut doc =
            page(b"BT /F2 10 Tf 0 1 -1 0 100 250 Tm <0001000200030004> Tj ET", "/Font << /F2 5 0 R >>", vec![font.into_bytes(), b"/Type0".to_vec()]);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        assert_eq!(apply(&mut doc, None).unwrap_err(), RedactError::VerticalText(1), "{subtype}");
    }
}

/// The proof, on a page whose mark was never applied, finds the image left under it, its
/// `/Subtype` given by reference, as with it given directly.
#[test]
fn the_proof_finds_an_image_given_its_subtype_by_reference_under_the_mark() {
    for (subtype, more) in [("/Image", b"null".to_vec()), ("6 0 R", b"/Image".to_vec())] {
        let mut doc = page(
            b"q 100 0 0 30 20 240 cm /X Do Q",
            "/XObject << /X 5 0 R >>",
            vec![stream(&format!("{IMAGE} /Subtype {subtype}"), [0x80; 18]), more],
        );
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        assert!(!crate::tests_verify::prove_unapplied(&mut doc).passed(), "{subtype}");
    }
}

/// The proof, on a page whose mark was never applied, reads the text left under it in a form, and
/// in a Type 3 font measured by its own widths, each with its `/Subtype` given by reference, as
/// with it given directly: it says the text survives.
#[test]
fn the_proof_reads_text_given_its_subtype_by_reference_under_the_mark() {
    let survives = |mut doc: Document| {
        mark(&mut doc, 0, &[[50.0, 230.0, 300.0, 280.0]], "");
        let proof = crate::tests_verify::prove_unapplied(&mut doc);
        crate::tests_verify::surface(&proof, crate::verify::Surface::ExtractedText).verdict == crate::verify::Verdict::Survivor
    };
    for (subtype, more) in [("/Form", b"null".to_vec()), ("6 0 R", b"/Form".to_vec())] {
        let form = stream(
            &format!("/Type /XObject /Subtype {subtype} /BBox [0 0 300 300] /Resources << /Font << /F1 7 0 R >> >>"),
            b"BT /F1 12 Tf 60 250 Td (FORMSECRET) Tj ET",
        );
        assert!(survives(page(b"/X Do", "/XObject << /X 5 0 R >>", vec![form, more, HELVETICA.to_vec()])), "a form, {subtype}");
    }
    for subtype in ["/Type3", "7 0 R"] {
        let font = format!(
            "<< /Type /Font /Subtype {subtype} /FontBBox [0 0 40 40] /FontMatrix [1 0 0 1 0 0] /CharProcs << /a 6 0 R >> /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [40] >>"
        );
        let doc = page(
            b"BT /T3 1 Tf 0 250 Td (aaaaa) Tj ET",
            "/Font << /T3 5 0 R >>",
            vec![font.into_bytes(), stream("", b"40 0 d0 0 0 40 40 re f"), b"/Type3".to_vec()],
        );
        assert!(survives(doc), "a Type 3 font, {subtype}");
    }
}

/// The proof's text reader places text in a Type 3 font, and in a composite font, each with its
/// `/Subtype` given by reference, where it places them given directly: four Type 3 glyphs 40 units
/// wide, four of them in a region from x 50; and four composite glyphs 10 points wide, two-byte
/// codes, three of them in a region from x 12. And Courier without `/Widths`, its `/BaseFont`
/// given by reference: five glyphs 6 points wide, three of them in a region from x 13.
#[test]
fn the_proofs_text_reader_places_fonts_given_their_subtype_by_reference() {
    let shown = |doc: &Document, region: [f64; 4]| crate::verify::tests_support::shown_in(doc, 0, &[region]);
    for subtype in ["/Type3", "7 0 R"] {
        let font = format!(
            "<< /Type /Font /Subtype {subtype} /FontBBox [0 0 40 40] /FontMatrix [1 0 0 1 0 0] /CharProcs << /a 6 0 R >> /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [40] >>"
        );
        let doc = page(
            b"BT /T3 1 Tf 0 250 Td (aaaaa) Tj ET",
            "/Font << /T3 5 0 R >>",
            vec![font.into_bytes(), stream("", b"40 0 d0 0 0 40 40 re f"), b"/Type3".to_vec()],
        );
        assert_eq!(shown(&doc, [50.0, 230.0, 300.0, 280.0]), Some(vec![4]), "a Type 3 font, {subtype}");
    }
    for subtype in ["/Type0", "6 0 R"] {
        let font = format!(
            "<< /Type /Font /Subtype {subtype} /BaseFont /X /Encoding /Identity-H /DescendantFonts [<< /Type /Font /Subtype /CIDFontType2 /BaseFont /X /DW 1000 >>] >>"
        );
        let doc = page(b"BT /F2 10 Tf 0 250 Td <0001000200030004> Tj ET", "/Font << /F2 5 0 R >>", vec![font.into_bytes(), b"/Type0".to_vec()]);
        assert_eq!(shown(&doc, [12.0, 230.0, 300.0, 280.0]), Some(vec![3]), "a composite font, {subtype}");
    }
    for base in ["/Courier", "6 0 R"] {
        let font = format!("<< /Type /Font /Subtype /Type1 /BaseFont {base} /Encoding /WinAnsiEncoding >>");
        let doc = page(b"BT /F1 10 Tf 0 250 Td (iiiii) Tj ET", "/Font << /F1 5 0 R >>", vec![font.into_bytes(), b"/Courier".to_vec()]);
        assert_eq!(shown(&doc, [13.0, 230.0, 300.0, 280.0]), Some(vec![3]), "a standard font, {base}");
    }
}

/// A form under the mark that the page reaches through a reference to a reference, which another
/// page names in its resources without drawing it: the form's text goes, and the reference on the
/// way goes with the form, so the saved file doesn't hold it through the other page.
#[test]
fn the_references_on_the_way_to_a_redacted_form_go_with_it() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 8 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /XObject << /X 5 0 R >> >> >>".to_vec(),
        stream("", b"/X Do"),
        b"6 0 R".to_vec(),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 7 0 R >> >>",
            b"BT /F1 12 Tf 20 250 Td (FORMSECRET) Tj ET",
        ),
        HELVETICA.to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 9 0 R /Resources << /XObject << /Y 5 0 R >> >> >>".to_vec(),
        stream("", b""),
    ]);
    mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
    assert_eq!(apply(&mut doc, None).map(|r| r.glyphs), Ok(10));
    assert!(!saved_holds(&doc, b"FORMSECRET"));
}

/// A redaction mark given its `/Subtype` by reference is a mark, and its own appearance, overlay
/// text and all, is exempt from the proof's check of what is left under it, as given directly.
#[test]
fn the_proof_exempts_a_mark_given_its_subtype_by_reference() {
    for by_reference in [false, true] {
        let mut doc = page(b"BT /F1 10 Tf 20 20 Td (Elsewhere) Tj ET", "/Font << /F1 5 0 R >>", vec![HELVETICA.to_vec()]);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "WITHHELD");
        if by_reference {
            let held = doc.add(Object::name("Redact"));
            let annot = annots_of(&doc, &pdfcraft_model::pages(&doc)[0].dict)[0].as_ref().unwrap();
            doc.update_dict(annot, |d| d.set(b"Subtype".to_vec(), Object::Ref(held))).unwrap();
        }
        assert_eq!(crate::marks(&doc).len(), 1, "by reference: {by_reference}");
        let proof = crate::tests_verify::prove_unapplied(&mut doc);
        assert!(proof.passed(), "by reference: {by_reference}: {}", proof.to_text());
    }
}

/// A form without resources of its own beside the mark draws the page's `/Im` through the page's
/// names, given directly, with its `/Subtype` by reference, or through a reference to a reference:
/// the page's draw of `/Im` under the mark goes, and the page keeps `/Im` for the form.
#[test]
fn an_image_a_form_given_by_reference_draws_beside_the_mark_is_kept() {
    let form = |s: &str| stream(&format!("/Type /XObject /Subtype {s} /BBox [0 0 300 100]"), b"q 100 0 0 30 20 20 cm /Im Do Q");
    for (label, extra) in [
        ("direct", vec![form("/Form"), b"null".to_vec()]),
        ("by reference", vec![form("7 0 R"), b"/Form".to_vec()]),
        ("a chain", vec![b"7 0 R".to_vec(), form("/Form")]),
    ] {
        let mut objs = vec![stream(&format!("{IMAGE} /Subtype /Image"), b"IMAGESECRET_BYTES!")];
        objs.extend(extra);
        let mut doc = page(b"q 100 0 0 30 20 240 cm /Im Do Q /Fm Do", "/XObject << /Im 5 0 R /Fm 6 0 R >>", objs);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        apply(&mut doc, None).unwrap();
        let p = &pdfcraft_model::pages(&doc)[0];
        let res = doc.resolve(p.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
        let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
        assert!(xo.contains(b"Im") && saved_holds(&doc, b"IMAGESECRET"), "{label}");
    }
}

/// The page and a form beside the mark share one resource dictionary listing the image the page
/// draws under the mark, which the form never draws; the form given directly, with its `/Subtype`
/// by reference, or through a reference to a reference: the saved file doesn't hold the image.
#[test]
fn an_image_a_shared_dictionary_lists_for_a_form_given_by_reference_goes() {
    let form = |s: &str| stream(&format!("/Type /XObject /Subtype {s} /BBox [0 0 300 100] /Resources 5 0 R"), b"0 0 10 10 re f");
    for (label, extra) in [
        ("direct", vec![form("/Form"), b"null".to_vec()]),
        ("by reference", vec![form("8 0 R"), b"/Form".to_vec()]),
        ("a chain", vec![b"8 0 R".to_vec(), form("/Form")]),
    ] {
        let mut objs = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
            stream("", b"q 100 0 0 30 20 240 cm /Im Do Q /Fm Do"),
            b"<< /XObject << /Im 6 0 R /Fm 7 0 R >> >>".to_vec(),
            stream(&format!("{IMAGE} /Subtype /Image"), b"IMAGESECRET_BYTES!"),
        ];
        objs.extend(extra);
        let mut doc = pdf(objs);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        assert_eq!(apply(&mut doc, None).map(|r| r.images_removed), Ok(1), "{label}");
        assert!(!saved_holds(&doc, b"IMAGESECRET"), "{label}");
    }
}

/// A Type 3 font, its `/Subtype` given directly or by reference, shows glyph A under the mark and
/// glyph B beside it, each glyph drawing an image of its own: A's program and image leave the
/// saved file, B's stay.
#[test]
fn a_type3_font_given_its_subtype_by_reference_loses_the_glyph_redacted() {
    for subtype in ["/Type3", "11 0 R"] {
        let mut doc = pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
            stream("", b"BT /T3 10 Tf 10 100 Td (AB) Tj ET"),
            b"<< /Font << /T3 6 0 R >> >>".to_vec(),
            format!("<< /Type /Font /Subtype {subtype} /FontBBox [0 -200 1000 800] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 66 /Widths [500 500] /Encoding << /Differences [65 /A /B] >> /CharProcs << /A 7 0 R /B 8 0 R >> /Resources << /XObject << /ImA 9 0 R /ImB 10 0 R >> >> >>").into_bytes(),
            stream("", b"/ImA Do % SECRET_GLYPH_A_PAYLOAD"),
            stream("", b"/ImB Do % SURVIVING_GLYPH_B_PAYLOAD"),
            stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"CONFIDENTIAL_IMAGE_A"),
            stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"PUBLIC_IMAGE_B"),
            b"/Type3".to_vec(),
        ]);
        mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
        assert_eq!(apply(&mut doc, None).map(|r| r.glyphs), Ok(1), "{subtype}");
        let held: Vec<bool> = [&b"SECRET_GLYPH_A_PAYLOAD"[..], b"CONFIDENTIAL_IMAGE_A", b"SURVIVING_GLYPH_B_PAYLOAD", b"PUBLIC_IMAGE_B"]
            .iter()
            .map(|n| saved_holds(&doc, n))
            .collect();
        assert_eq!(held, [false, false, true, true], "{subtype}");
    }
}

/// Page 1 draws a form under the mark, by its own reference or through a reference to it; page 2
/// lists the same form without drawing it, through another reference to it: the saved file doesn't
/// hold the form's text, as where page 2 lists it by its own reference.
#[test]
fn a_form_redacted_goes_by_every_reference_to_it() {
    for (first, second) in [("6 0 R", "6 0 R"), ("6 0 R", "10 0 R"), ("5 0 R", "10 0 R")] {
        let mut doc = pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R 8 0 R] /Count 2 >>".to_vec(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /XObject << /X {first} >> >> >>")
                .into_bytes(),
            stream("", b"/X Do"),
            b"6 0 R".to_vec(),
            stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 7 0 R >> >>",
                b"BT /F1 12 Tf 20 250 Td (FORMSECRET) Tj ET",
            ),
            HELVETICA.to_vec(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 9 0 R /Resources << /XObject << /Y {second} >> >> >>")
                .into_bytes(),
            stream("", b""),
            b"6 0 R".to_vec(),
        ]);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        assert_eq!(apply(&mut doc, None).map(|r| r.glyphs), Ok(10), "{first} {second}");
        assert!(!saved_holds(&doc, b"FORMSECRET"), "{first} {second}");
    }
}

/// A form without resources of its own beside the mark marks content with the page's `/oc1`,
/// given directly, with its `/Subtype` by reference, or through a reference to a reference: the
/// page keeps `/oc1` for it when its text under the mark goes.
#[test]
fn a_property_a_form_given_by_reference_uses_is_kept() {
    let form = |s: &str| stream(&format!("/Type /XObject /Subtype {s} /BBox [0 0 300 100]"), b"/OC /oc1 BDC 0 0 50 50 re f EMC");
    for (label, extra) in [
        ("direct", vec![form("/Form"), b"null".to_vec()]),
        ("by reference", vec![form("7 0 R"), b"/Form".to_vec()]),
        ("a chain", vec![b"7 0 R".to_vec(), form("/Form")]),
    ] {
        let mut objs = vec![HELVETICA.to_vec()];
        objs.extend(extra);
        objs.push(b"<< /Type /OCG /Name (Hidden) >>".to_vec());
        let mut doc = page(
            b"BT /F1 12 Tf 20 250 Td (SECRET) Tj ET /Fm Do",
            "/Font << /F1 5 0 R >> /XObject << /Fm 6 0 R >> /Properties << /oc1 8 0 R >>",
            objs,
        );
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        assert_eq!(apply(&mut doc, None).map(|r| r.glyphs), Ok(6), "{label}");
        let p = &pdfcraft_model::pages(&doc)[0];
        let res = doc.resolve(p.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
        let properties = res.get(b"Properties").and_then(|p| doc.resolve(p).as_dict().cloned()).unwrap_or_default();
        assert!(properties.contains(b"oc1"), "{label}");
    }
}

/// A tagged form reached through a reference to a reference, its marked content named in the
/// structure tree by either reference: the element loses its `/ActualText` with the redacted
/// text, and the saved file holds neither.
#[test]
fn a_tagged_form_given_by_reference_loses_its_actual_text() {
    for stm in ["6 0 R", "5 0 R"] {
        let mut doc = pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 8 0 R /MarkInfo << /Marked true >> >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /XObject << /X 5 0 R >> >> >>".to_vec(),
            stream("", b"/X Do"),
            b"6 0 R".to_vec(),
            stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 7 0 R >> >>",
                b"/P << /MCID 0 >> BDC BT /F1 12 Tf 20 250 Td (FORMSECRET) Tj ET EMC",
            ),
            HELVETICA.to_vec(),
            b"<< /Type /StructTreeRoot /K 9 0 R >>".to_vec(),
            format!("<< /Type /StructElem /S /P /P 8 0 R /Pg 3 0 R /K << /Type /MCR /Pg 3 0 R /Stm {stm} /MCID 0 >> /ActualText (ACTUALSECRET) >>")
                .into_bytes(),
        ]);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        assert_eq!(apply(&mut doc, None).map(|r| r.tags), Ok(1), "{stm}");
        assert!(!doc.get(ObjRef::new(9, 0)).as_dict().unwrap().contains(b"ActualText"), "{stm}");
        assert!(!saved_holds(&doc, b"FORMSECRET") && !saved_holds(&doc, b"ACTUALSECRET"), "{stm}");
    }
}

/// A standard 14 font without `/Widths`, its `/BaseFont` given directly or by reference: measured
/// as Courier either way, so the same glyphs, SECRET's, go.
#[test]
fn a_standard_font_named_by_reference_is_measured_as_itself() {
    let content = |base: &str| {
        let mut doc = page(
            b"BT /F1 12 Tf 0 250 Td (XXXXXXXXXXSECRET) Tj ET",
            "/Font << /F1 5 0 R >>",
            vec![format!("<< /Type /Font /Subtype /Type1 /BaseFont {base} /Encoding /WinAnsiEncoding >>").into_bytes(), b"/Courier".to_vec()],
        );
        mark(&mut doc, 0, &[[72.5, 230.0, 115.0, 280.0]], "");
        assert_eq!(apply(&mut doc, None).map(|r| r.glyphs), Ok(6), "{base}");
        super::content(&doc, 0)
    };
    assert_eq!(content("6 0 R"), content("/Courier"));
}

/// Remove Hidden Information unlinks an image its page lists but doesn't draw, its `/Subtype`
/// given by reference, as given directly.
#[test]
fn an_undrawn_image_given_its_subtype_by_reference_is_unlinked() {
    use crate::sanitize::{Hidden, remove_hidden};
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << /XObject << /Deleted 4 0 R >> >> >>".to_vec(),
        stream("/Type /XObject /Subtype 5 0 R /Width 17 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"DELETEDIMAGEBYTES"),
        b"/Image".to_vec(),
    ]);
    assert_eq!(remove_hidden(&mut doc, &[Hidden::Metadata]), Ok(Vec::new()));
    assert!(!saved_holds(&doc, b"DELETEDIMAGEBYTES"));
}

/// Remove Hidden Information keeps the images a Type 3 glyph, and a form in its font's resources,
/// draw through the page's names, with the font's and the form's `/Subtype` given by reference.
#[test]
fn images_drawn_by_a_type3_glyph_given_by_reference_stay() {
    let bytes = super::hidden_information_removed(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"BT /T3 12 Tf (a) Tj ET"),
        b"<< /XObject << /ByGlyph 7 0 R /Deleted 8 0 R /ByFontForm 11 0 R >> /Font << /T3 6 0 R >> >>".to_vec(),
        b"<< /Type /Font /Subtype 12 0 R /FontBBox [0 0 10 10] /FontMatrix [0.1 0 0 0.1 0 0] /CharProcs << /a 9 0 R >> \
           /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [10] /Resources << /XObject << /Fx 10 0 R >> >> >>"
            .to_vec(),
        super::gray_image(b"BYGLYPHIMAGE"),
        super::gray_image(b"DELETEDIMAGEBYTES"),
        stream("", b"10 0 d0 /ByGlyph Do /Fx Do"),
        stream("/Type /XObject /Subtype 13 0 R /BBox [0 0 10 10]", b"/ByFontForm Do"),
        super::gray_image(b"BYFONTFORMIMAGE"),
        b"/Type3".to_vec(),
        b"/Form".to_vec(),
    ]);
    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    assert!(!has(b"DELETEDIMAGEBYTES"));
    assert!(has(b"BYGLYPHIMAGE"));
    assert!(has(b"BYFONTFORMIMAGE"));
}
