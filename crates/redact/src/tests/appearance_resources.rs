//! Annotation appearances and the resources the redaction cleanups prune: an appearance a kept
//! annotation still shows keeps the fonts, glyphs and images it draws, and one a removed
//! annotation showed keeps nothing in the file.

use super::surviving_references::{full_save_holds, refused_for, saved_first_page_lists_font, type3_abc};
use super::*;

/// (a) Two annotations show glyphs of one Type 3 font that only their appearances' resources
/// list. The one under the mark goes, and with it the glyph program only it showed; the glyph the
/// kept annotation shows stays.
#[test]
fn a_type3_font_only_appearances_list_loses_the_removed_annotations_glyph() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << >> /Annots [5 0 R 6 0 R] >>".to_vec(),
        stream("", b""),
        b"<< /Type /Annot /Subtype /FreeText /Rect [20 20 120 40] /DA (/T3 10 Tf 0 g) /AP << /N 7 0 R >> >>".to_vec(),
        b"<< /Type /Annot /Subtype /FreeText /Rect [150 200 250 220] /DA (/T3 10 Tf 0 g) /AP << /N 8 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 100 20] /Resources << /Font << /T3 9 0 R >> >>", b"BT /T3 10 Tf 2 5 Td (A) Tj ET"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 100 20] /Resources << /Font << /T3 9 0 R >> >>", b"BT /T3 10 Tf 2 5 Td (B) Tj ET"),
        type3_abc(10),
        stream("", b"% REMOVED_ANNOTATION_GLYPH_A"),
        stream("", b"% KEPT_ANNOTATION_GLYPH_B"),
        stream("", b"% UNUSED_GLYPH_C"),
    ]);
    mark(&mut doc, 0, &[[10.0, 10.0, 130.0, 50.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().annotations, 1);
    assert!(!full_save_holds(&doc, b"REMOVED_ANNOTATION_GLYPH_A"), "the removed annotation's glyph program survived");
    assert!(full_save_holds(&doc, b"KEPT_ANNOTATION_GLYPH_B"), "the glyph the kept annotation shows was unlinked");
}

/// (b) An appearance whose `/Resources` hold only `/ProcSet` selects the page's Type 3 font `/T4`:
/// the renderer looks a name the appearance's resources lack up in the page's. The page's content
/// never selects `/T4`, so the rewritten page must still list it, with the glyph shown.
#[test]
fn a_page_font_an_appearance_with_partial_resources_selects_stays_on_the_page() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R /Annots [8 0 R] >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (AC) Tj ET"),
        b"<< /Font << /T3 6 0 R /T4 7 0 R >> >>".to_vec(),
        type3_abc(10),
        type3_abc(13),
        b"<< /Type /Annot /Subtype /FreeText /Rect [150 200 250 220] /DA (/T4 10 Tf 0 g) /AP << /N 9 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 100 20] /Resources << /ProcSet [/PDF /Text] >>", b"BT /T4 10 Tf 2 5 Td (B) Tj ET"),
        stream("", b"% REDACTED_GLYPH_A"),
        stream("", b"% UNUSED_GLYPH_B"),
        stream("", b"% PAGE_GLYPH_C"),
        stream("", b"% UNUSED_T4_GLYPH_A"),
        stream("", b"% APPEARANCE_GLYPH_B"),
        stream("", b"% UNUSED_T4_GLYPH_C"),
    ]);
    mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
    assert!(!full_save_holds(&doc, b"REDACTED_GLYPH_A"), "the redacted glyph program survived");
    assert!(saved_first_page_lists_font(&doc, b"T4"), "the page no longer lists the font the appearance selects");
    assert!(full_save_holds(&doc, b"APPEARANCE_GLYPH_B"), "the glyph the appearance shows was unlinked");
}

/// (b) The same partial resources, the appearance showing glyph B of the page's `/T3`, whose
/// content still shows C: the glyph scan reads the appearance's `/T3` from the page, so B stays.
#[test]
fn a_page_glyph_an_appearance_with_partial_resources_shows_is_kept() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R /Annots [7 0 R] >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (AC) Tj ET"),
        b"<< /Font << /T3 6 0 R >> >>".to_vec(),
        type3_abc(9),
        b"<< /Type /Annot /Subtype /FreeText /Rect [150 200 250 220] /DA (/T3 10 Tf 0 g) /AP << /N 8 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 100 20] /Resources << /ProcSet [/PDF /Text] >>", b"BT /T3 10 Tf 2 5 Td (B) Tj ET"),
        stream("", b"% REDACTED_GLYPH_A"),
        stream("", b"% APPEARANCE_GLYPH_B"),
        stream("", b"% PAGE_GLYPH_C"),
    ]);
    mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
    assert!(!full_save_holds(&doc, b"REDACTED_GLYPH_A"), "the redacted glyph program survived");
    assert!(full_save_holds(&doc, b"APPEARANCE_GLYPH_B"), "the glyph the appearance shows was unlinked");
    assert!(full_save_holds(&doc, b"PAGE_GLYPH_C"), "the glyph the page still shows was unlinked");
}

/// (c) `/AP /N` names an object that is itself only a reference to the appearance stream, which
/// shows glyph B: the renderer follows the chain, so B stays.
#[test]
fn a_type3_glyph_an_appearance_given_through_a_reference_shows_is_kept() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R /Annots [7 0 R] >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (AC) Tj ET"),
        b"<< /Font << /T3 6 0 R >> >>".to_vec(),
        type3_abc(10),
        b"<< /Type /Annot /Subtype /FreeText /Rect [150 200 250 220] /DA (/T3 10 Tf 0 g) /AP << /N 8 0 R >> >>".to_vec(),
        b"9 0 R".to_vec(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 100 20] /Resources << /Font << /T3 6 0 R >> >>", b"BT /T3 10 Tf 2 5 Td (B) Tj ET"),
        stream("", b"% REDACTED_GLYPH_A"),
        stream("", b"% APPEARANCE_GLYPH_B"),
        stream("", b"% PAGE_GLYPH_C"),
    ]);
    mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
    assert!(!full_save_holds(&doc, b"REDACTED_GLYPH_A"), "the redacted glyph program survived");
    assert!(full_save_holds(&doc, b"APPEARANCE_GLYPH_B"), "the glyph the appearance shows was unlinked");
}

/// (c) The same chain to an appearance without resources of its own, which selects the page's
/// `/T4`: the rewritten page still lists it.
#[test]
fn a_page_font_an_appearance_given_through_a_reference_selects_stays_on_the_page() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R /Annots [8 0 R] >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (AC) Tj ET"),
        b"<< /Font << /T3 6 0 R /T4 7 0 R >> >>".to_vec(),
        type3_abc(11),
        type3_abc(14),
        b"<< /Type /Annot /Subtype /FreeText /Rect [150 200 250 220] /DA (/T4 10 Tf 0 g) /AP << /N 9 0 R >> >>".to_vec(),
        b"10 0 R".to_vec(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 100 20]", b"BT /T4 10 Tf 2 5 Td (B) Tj ET"),
        stream("", b"% REDACTED_GLYPH_A"),
        stream("", b"% UNUSED_GLYPH_B"),
        stream("", b"% PAGE_GLYPH_C"),
        stream("", b"% UNUSED_T4_GLYPH_A"),
        stream("", b"% APPEARANCE_GLYPH_B"),
        stream("", b"% UNUSED_T4_GLYPH_C"),
    ]);
    mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
    assert!(saved_first_page_lists_font(&doc, b"T4"), "the page no longer lists the font the appearance selects");
    assert!(full_save_holds(&doc, b"APPEARANCE_GLYPH_B"), "the glyph the appearance shows was unlinked");
}

/// A page with an annotation whose appearance writes a Type 3 font in its own resources, as
/// PdfCraft writes the Thai it draws in a comment or field, and `resources` for that appearance.
fn with_appearance_font(resources: &[u8], extra: Vec<Vec<u8>>) -> Document {
    let font = b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 65 /Widths [500] /Encoding << /Differences [65 /A] >> /CharProcs << /A 7 0 R >> >>";
    let with_font = |o: &[u8]| String::from_utf8_lossy(o).replace("FONT", &String::from_utf8_lossy(font));
    let resources = with_font(resources);
    let mut objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 8 0 R >> >> /Annots [5 0 R] >>".to_vec(),
        stream("", b"BT /F1 10 Tf 10 100 Td (Secret) Tj ET"),
        b"<< /Type /Annot /Subtype /FreeText /Rect [150 200 250 220] /DA (/F1 10 Tf 0 g) /AP << /N 6 0 R >> >>".to_vec(),
        stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 100 20] /Resources {resources}"), b"BT /PCTh1 10 Tf 2 5 Td (A) Tj ET"),
        stream("", b"% APPEARANCE_GLYPH_A"),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    objs.extend(extra.iter().map(|o| with_font(o).into_bytes()));
    pdf(objs)
}

/// A Type 3 font written in an appearance's own resources, not as an object of its own, belongs to
/// that appearance alone: the redaction goes through, and the glyph the appearance shows stays.
#[test]
fn a_type3_font_written_in_an_appearance_s_own_resources_is_passed_over() {
    let mut doc = with_appearance_font(b"<< /Font << /PCTh1 FONT >> >>", vec![]);
    mark(&mut doc, 0, &[[9.0, 95.0, 40.0, 110.0]], "");
    apply(&mut doc, None).unwrap();
    assert!(full_save_holds(&doc, b"APPEARANCE_GLYPH_A"), "the glyph the appearance shows was unlinked");
}

/// What the Type 3 cleanup refuses a font written in place with.
const DIRECT: &str = "a Type 3 font is not an indirect object";

/// The same font in a resource dictionary given by reference, which other appearances can share:
/// its glyph programs can't be cleaned, and the redaction is refused rather than leave them.
#[test]
fn a_type3_font_written_in_shared_appearance_resources_is_refused() {
    let mut doc = with_appearance_font(b"9 0 R", vec![b"<< /Font << /PCTh1 FONT >> >>".to_vec()]);
    mark(&mut doc, 0, &[[9.0, 95.0, 40.0, 110.0]], "");
    assert!(refused_for(&apply(&mut doc, None), DIRECT), "a direct Type 3 font other appearances can reach was not refused");
    let mut doc = with_appearance_font(b"<< /Font 9 0 R >>", vec![b"<< /PCTh1 FONT >>".to_vec()]);
    mark(&mut doc, 0, &[[9.0, 95.0, 40.0, 110.0]], "");
    assert!(refused_for(&apply(&mut doc, None), DIRECT), "a direct Type 3 font in a shared /Font dictionary was not refused");
}
