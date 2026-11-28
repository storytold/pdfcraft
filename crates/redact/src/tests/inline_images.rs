//! Inline image data ends where the renderer ends it, so the operators after it are the ones the
//! page draws.

use super::*;

/// The image data `7FEI 1 0 0 1 200 0 cm` holds an `EI` straight after `7F`. The renderer ends
/// the image there (text follows, so what comes after reads as content) and runs the `cm`,
/// drawing SECRET at x = 210, not 10.
const FAKE_EI: &[u8] = b"BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 7FEI 1 0 0 1 200 0 cm EI BT /F1 10 Tf 10 100 Td (SECRET) Tj ET";

#[test]
fn text_after_an_inline_image_is_redacted_where_the_renderer_draws_it() {
    let mut doc = one_page(FAKE_EI, "", vec![]);
    assert_eq!(under(&mut doc, 0, &[[5.0, 95.0, 45.0, 110.0]]), 0, "nothing is drawn at x = 10");
    mark(&mut doc, 0, &[[205.0, 95.0, 245.0, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 6);
    assert!(!content(&doc, 0).contains("SECRET"));
}

#[test]
fn an_inline_image_with_one_possible_end_is_redacted() {
    let mut doc = one_page(b"BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 7F> EI BT /F1 10 Tf 10 100 Td (SECRET) Tj ET", "", vec![]);
    mark(&mut doc, 0, &[[5.0, 95.0, 45.0, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 6);
    assert!(!content(&doc, 0).contains("SECRET"));
}

/// Without an `EI` the renderer accepts, where the operators resume is a guess: refused.
#[test]
fn a_page_whose_inline_image_the_renderer_cannot_end_is_refused() {
    let mut doc = one_page(b"BT /F1 10 Tf 10 100 Td (SECRET) Tj ET BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 7F", "", vec![]);
    mark(&mut doc, 0, &[[5.0, 95.0, 45.0, 110.0]], "");
    assert_eq!(apply(&mut doc, None), Err(RedactError::Unreadable(1)));
}

/// The renderer draws an image whose parameters the content parser can't read (a stray operand
/// before `ID`): it would stay under the mark unseen, so the page is refused.
#[test]
fn a_page_whose_inline_image_only_the_renderer_reads_is_refused() {
    let mut doc = one_page(b"q 10 0 0 10 50 50 cm BI /W 1 /H 1 /CS /G /BPC 8 7 ID \x80 EI Q", "", vec![]);
    mark(&mut doc, 0, &[[45.0, 45.0, 65.0, 65.0]], "");
    assert_eq!(apply(&mut doc, None), Err(RedactError::Unreadable(1)));
}

/// A mark over part of an inline image clears the pixels it covers in a copy drawn with `Do`, as
/// for an image XObject, whatever the image's abbreviations: the rest of the picture stays.
#[test]
fn a_partly_covered_inline_image_keeps_its_uncovered_part() {
    let flate = Stream::flate(Dict::new(), &[123, 234]).raw;
    let cases: Vec<(Vec<u8>, &str, Vec<u8>)> = vec![
        (b"BI /W 2 /H 1 /CS /G /BPC 8 ID \x7b\xea EI".to_vec(), "", vec![0, 234]),
        (b"BI /W 2 /H 1 /CS /G /BPC 8 /F /AHx ID 7BEA> EI".to_vec(), "", vec![0, 234]),
        (b"BI /W 2 /H 1 /CS /G /BPC 8 /F [/A85] ID Hg1~> EI".to_vec(), "", vec![0, 234]),
        (
            [
                b"BI /Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode /L ".as_slice(),
                flate.len().to_string().as_bytes(),
                b" ID ",
                &flate,
                b" EI",
            ]
            .concat(),
            "",
            vec![0, 234],
        ),
        (b"BI /W 2 /H 1 /CS [/I /RGB 1 <FF000000FF00>] /BPC 8 ID \x01\x01 EI".to_vec(), "", vec![0, 1]),
        (
            b"BI /W 2 /H 1 /CS /Cs0 /BPC 8 ID \x7b\x7b\x7b\xea\xea\xea EI".to_vec(),
            "/ColorSpace << /Cs0 [/CalRGB << /WhitePoint [1 1 1] >>] >>",
            vec![0, 0, 0, 234, 234, 234],
        ),
        (b"BI /W 2 /H 1 /CS [/I /Cs0 1 <00FF>] /BPC 8 ID \x01\x01 EI".to_vec(), "/ColorSpace << /Cs0 /DeviceGray >>", vec![0, 1]),
        (b"BI /W 8 /H 1 /IM true ID \x00 EI".to_vec(), "", vec![0b1110_0000]),
    ];
    for (image, resources, cleared) in cases {
        let label = String::from_utf8_lossy(&image).into_owned();
        let mut doc = one_page(&[b"q 200 0 0 100 0 0 cm ".as_slice(), &image, b" Q"].concat(), resources, vec![]);
        mark(&mut doc, 0, &[[10.0, 10.0, 70.0, 90.0]], "");
        let r = apply(&mut doc, None).unwrap();
        assert_eq!((r.images_cleared, r.images_removed), (1, 0), "{label}: {r:?}");
        let c = content(&doc, 0);
        assert!(!c.contains("BI") && c.contains("/PCRedacted1 Do"), "{label}: {c}");
        let saved = reopen(&doc);
        assert_eq!(named_image(&saved, 0, b"PCRedacted1"), Some(cleared), "{label}");
    }
}

/// Nothing of what a mark covers stays in the file, in the content or in the copy.
#[test]
fn an_inline_images_covered_samples_leave_the_file() {
    let mut doc = one_page(b"q 80 0 0 10 0 0 cm BI /W 8 /H 1 /CS /G /BPC 8 ID SECRETok EI Q", "", vec![]);
    mark(&mut doc, 0, &[[0.0, 0.0, 55.0, 10.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().images_cleared, 1);
    assert_no_secret(&doc, b"SECR");
    assert_no_secret(&doc, b"RET");
    assert_eq!(named_image(&reopen(&doc), 0, b"PCRedacted1"), Some(b"\0\0\0\0\0\0ok".to_vec()));
}

/// An inline image that can't be cleared is still removed whole: a codec not decoded here, a
/// colour space the resources don't define, or a mark over all of it.
#[test]
fn an_inline_image_that_cant_be_cleared_is_removed_whole() {
    for (image, rect) in [
        (b"BI /W 2 /H 1 /CS /G /BPC 8 /F /DCT ID \xff\xd8\xff\xd9 EI".as_slice(), [10.0, 10.0, 70.0, 90.0]),
        (b"BI /W 2 /H 1 /CS /Nowhere /BPC 8 ID \x7b\xea EI", [10.0, 10.0, 70.0, 90.0]),
        (b"BI /W 2 /H 1 /CS /I /BPC 8 ID \x7b\xea EI", [10.0, 10.0, 70.0, 90.0]),
        (b"BI /W 2 /H 1 /CS /G /BPC 8 ID \x7b\xea EI", [-5.0, -5.0, 205.0, 105.0]),
    ] {
        let label = String::from_utf8_lossy(image).into_owned();
        let mut doc = one_page(&[b"q 200 0 0 100 0 0 cm ".as_slice(), image, b" Q"].concat(), "", vec![]);
        mark(&mut doc, 0, &[rect], "");
        let r = apply(&mut doc, None).unwrap();
        assert_eq!((r.images_cleared, r.images_removed), (0, 1), "{label}: {r:?}");
        let c = content(&doc, 0);
        assert!(!c.contains("BI") && !c.contains("Do"), "{label}: {c}");
    }
}

/// Dimensions whose product overflows (2^32 × 2^32) are refused, not multiplied into a panic: the
/// partly covered image, inline or an XObject, is removed whole.
#[test]
fn an_image_whose_dimensions_overflow_is_removed_not_a_crash() {
    let huge = "/W 4294967296 /H 4294967296 /CS /G /BPC 8";
    let mut doc = one_page(format!("q 200 0 0 100 0 0 cm BI {huge} ID \x55 EI Q").as_bytes(), "", vec![]);
    mark(&mut doc, 0, &[[10.0, 10.0, 70.0, 90.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
    let dict = "/Type /XObject /Subtype /Image /Width 4294967296 /Height 4294967296 /ColorSpace /DeviceGray /BitsPerComponent 8";
    let mut doc = one_page(b"q 200 0 0 100 0 0 cm /Im Do Q", "/XObject << /Im 7 0 R >>", vec![stream(dict, [123])]);
    mark(&mut doc, 0, &[[10.0, 10.0, 70.0, 90.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
}
