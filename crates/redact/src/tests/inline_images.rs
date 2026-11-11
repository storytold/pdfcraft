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

/// The renderer ends an image inside `/GSEI`, passing over an `EI` other readers end it at: they
/// draw the form after it, under the mark, which the renderer reads as data. Where the page's
/// operators resume can't be told, so the page is refused rather than the form's text left.
#[test]
fn a_page_whose_inline_image_other_readers_end_earlier_is_refused() {
    let form =
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >>", b"BT /F1 10 Tf 20 260 Td (SECRET) Tj ET");
    let mut doc = one_page(
        b"q 10 0 0 10 0 0 cm BI /W 2 /H 1 /CS /G /BPC 8 /F /AHx ID 00FF> EI Q /Fm Do q /GSEI gs Q",
        "/XObject << /Fm 7 0 R >> /ExtGState << /GSEI << >> >>",
        vec![form],
    );
    mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    assert_eq!(apply(&mut doc, None), Err(RedactError::Unreadable(1)));
}

/// ASCII85 of `bytes`, a multiple of four long, without the end-of-data marker.
fn a85(bytes: &[u8]) -> String {
    let mut out = String::new();
    for group in bytes.chunks(4) {
        let mut v = group.iter().fold(0u32, |v, &b| v << 8 | u32::from(b));
        if v == 0 {
            out.push('z');
            continue;
        }
        let mut digits = [0u8; 5];
        for d in digits.iter_mut().rev() {
            *d = b'!' + (v % 85) as u8;
            v /= 85;
        }
        out.extend(digits.iter().map(|&d| d as char));
    }
    out
}

/// What decoding inline images may give, to tell where a reader that decodes them ends them, is
/// shared by a page's walk: a form drawn again is parsed again, and decodes them again within what
/// is left, not within an allowance of its own. A form holds an image behind ASCII85 and Flate
/// whose `EI` group lies in its samples, a stored block, before 40 MiB of zeros: drawn once, its
/// stop is told and the page is redacted; drawn twice, what the first draw left isn't enough to
/// tell it again, and the page is refused.
#[test]
fn a_form_drawn_again_decodes_its_inline_images_within_what_is_left() {
    const EI: [u8; 4] = [0x71, 0x79, 0x7d, 0xec];
    let zeros = pdfcraft_filters::encode_flate(&vec![0u8; 40 << 20]);
    let mut data = vec![0x78, 0x01, 0x00, 5, 0, 0xfa, 0xff, 1, EI[0], EI[1], EI[2], EI[3]];
    data.extend_from_slice(zeros.get(2..zeros.len() - 4).unwrap());
    data.resize(data.len().next_multiple_of(4), 0);
    let samples = 5 + (40 << 20);
    let image = format!("q BI /W {samples} /H 1 /CS /G /BPC 8 /F [/A85 /Fl] ID {}~> EI Q", a85(&data));
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", image.as_bytes());
    let drawn = |draws: usize| {
        let content = format!("BT /F1 10 Tf 10 100 Td (SECRET) Tj ET {}", "q /Fm Do Q ".repeat(draws));
        let mut doc = one_page(content.as_bytes(), "/XObject << /Fm 7 0 R >>", vec![form.clone()]);
        mark(&mut doc, 0, &[[5.0, 95.0, 45.0, 110.0]], "");
        apply(&mut doc, None).map(|r| r.glyphs)
    };
    assert_eq!(drawn(1), Ok(6), "drawn once");
    assert_eq!(drawn(2), Err(RedactError::Unreadable(1)), "drawn twice");
}
