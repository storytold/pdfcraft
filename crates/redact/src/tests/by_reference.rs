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
/// codes, three of them in a region from x 12.
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
