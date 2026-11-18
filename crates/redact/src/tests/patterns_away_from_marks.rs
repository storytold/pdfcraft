//! A tiling pattern whose cell carries text or images is refused only when it may have painted
//! something a mark reached (#7696): a page that lists such a pattern, or paints with it away
//! from every mark, is redacted, and the cell stays as it was.

use super::*;

/// A pattern whose cell draws image 8, as object 7, and the image.
fn image_pattern() -> Vec<Vec<u8>> {
    vec![
        stream(
            "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 50 50] /XStep 50 /YStep 50 /Resources << /XObject << /Im 8 0 R >> >>",
            b"q 50 0 0 50 0 0 cm /Im Do Q",
        ),
        stream("/Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"OK"),
    ]
}

const PATTERN: &str = "/Pattern << /P1 7 0 R >>";
/// The text is at (10, 100), 6 glyphs of 5 pt.
const TEXT: [f64; 4] = [5.0, 95.0, 45.0, 110.0];
/// A box filled with the pattern at (200, 200), 50 pt square.
const FILLED: &str = "q /Pattern cs /P1 scn 200 200 50 50 re f Q";

fn refused(doc: &mut Document) -> bool {
    matches!(apply(doc, None), Err(RedactError::Pattern { page: 1, reason: PatternProblem::Content }))
}

/// The pattern's cell and its image are in the saved file, unchanged.
fn pattern_kept(doc: &Document) -> bool {
    let saved = reopen(doc);
    let page = pdfcraft_model::pages(&saved).swap_remove(0);
    let resources = saved.resolve(page.dict.get(b"Resources").unwrap());
    let patterns = saved.resolve(resources.as_dict().unwrap().get(b"Pattern").unwrap());
    let Some(cell) = patterns.as_dict().and_then(|p| p.get(b"P1")).map(|p| saved.resolve(p)) else { return false };
    let Object::Stream(cell) = &*cell else { return false };
    cell.decoded().unwrap() == b"q 50 0 0 50 0 0 cm /Im Do Q"
        && saved.scan_objects().any(|(_, o)| matches!(&*o, Object::Stream(s) if s.decoded().is_ok_and(|d| d == b"OK")))
}

#[test]
fn a_page_that_only_lists_a_pattern_with_images_is_redacted() {
    let mut doc = one_page(b"BT /F1 10 Tf 10 100 Td (SECRET) Tj ET", PATTERN, image_pattern());
    mark(&mut doc, 0, &[TEXT], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 6);
    assert!(!content(&doc, 0).contains("SECRET"));
    assert!(pattern_kept(&doc));
}

/// The shapes a pattern fills away from the marks stay, drawn as they were, whether the text is
/// shown before them, after the `Q` that restores the colour, or after a colour operator.
#[test]
fn a_pattern_painting_away_from_the_marks_stays() {
    for page in [
        format!("BT /F1 10 Tf 10 100 Td (SECRET) Tj ET {FILLED}"),
        format!("{FILLED} BT /F1 10 Tf 10 100 Td (SECRET) Tj ET"),
        "/Pattern cs /P1 scn 200 200 50 50 re f 0 g BT /F1 10 Tf 10 100 Td (SECRET) Tj ET".to_string(),
        "/Pattern cs /P1 scn 200 200 50 50 re f /DeviceRGB cs 0 0 0 sc BT /F1 10 Tf 10 100 Td (SECRET) Tj ET".to_string(),
        "/Pattern cs /P1 scn 200 200 50 50 re f /Icc cs 0 0 0 scn BT /F1 10 Tf 10 100 Td (SECRET) Tj ET".to_string(),
        "/Pattern CS /P1 SCN 200 200 50 50 re S 0 G BT /F1 10 Tf 10 100 Td (SECRET) Tj ET".to_string(),
    ] {
        let resources = format!("{PATTERN} /ColorSpace << /Icc [/ICCBased 9 0 R] >>");
        let mut objects = image_pattern();
        objects.push(stream("/N 3", b""));
        let mut doc = one_page(page.as_bytes(), &resources, objects);
        mark(&mut doc, 0, &[TEXT], "");
        assert_eq!(apply(&mut doc, None).unwrap().glyphs, 6, "{page}");
        let c = content(&doc, 0);
        assert!(!c.contains("SECRET") && (c.contains("/P1 scn") || c.contains("/P1 SCN")) && c.contains("200 200 50 50 re"), "{page}: {c}");
        assert!(pattern_kept(&doc), "{page}");
    }
}

/// Whatever a mark reaches that a pattern may have painted still refuses the page while a cell
/// with images stays: the cell may hold what the mark covers.
#[test]
fn a_pattern_that_may_paint_under_a_mark_is_still_refused() {
    let type3 = "/Font << /T3 9 0 R >>";
    let glyph = "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 10 0 R >> \
                 /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] /Resources << >> >>";
    let cases: Vec<(&str, String, String, Vec<Vec<u8>>)> = vec![
        // The marked text is painted with the pattern.
        ("text in the pattern", "/Pattern cs /P1 scn BT /F1 10 Tf 10 100 Td (SECRET) Tj ET".into(), String::new(), vec![]),
        // So is text a mark doesn't reach: a glyph a mark only grazes would stay.
        (
            "pattern text elsewhere",
            "/Pattern cs /P1 scn BT /F1 10 Tf 10 250 Td (PUBLIC) Tj ET 0 g BT /F1 10 Tf 10 100 Td (SECRET) Tj ET".into(),
            String::new(),
            vec![],
        ),
        // A marked shape the pattern fills, and another it fills elsewhere, which keeps the cell.
        ("a filled shape under the mark", format!("/Pattern cs /P1 scn 0 90 50 30 re f {FILLED}"), String::new(), vec![]),
        (
            "a stroked shape under the mark",
            format!("/Pattern CS /P1 SCN 10 W 0 100 m 50 100 l S {FILLED}").replace("10 W", "10 w"),
            String::new(),
            vec![],
        ),
        // A numbers-only `scn` doesn't prove the pattern left.
        ("numbers after the pattern", format!("/Pattern cs /P1 scn 0.5 scn 0 90 50 30 re f {FILLED}"), String::new(), vec![]),
        // A Pattern space the resources name, and a space nothing names.
        ("a named Pattern space", format!("/Pat cs /P1 scn 0 90 50 30 re f {FILLED}"), "/ColorSpace << /Pat /Pattern >>".into(), vec![]),
        (
            "a named Pattern space array",
            format!("/Pat cs 0 /P1 scn 0 90 50 30 re f {FILLED}"),
            "/ColorSpace << /Pat [/Pattern /DeviceGray] >>".into(),
            vec![],
        ),
        ("an unknown space", "/Nowhere cs BT /F1 10 Tf 10 100 Td (SECRET) Tj ET".into(), String::new(), vec![]),
        ("an unknown stroke space", "/Nowhere CS BT /F1 10 Tf 10 100 Td (SECRET) Tj ET".into(), String::new(), vec![]),
        // Selecting the space is enough: no `scn` has to name a pattern after it.
        ("a named Pattern space alone", "/Pat cs BT /F1 10 Tf 10 100 Td (SECRET) Tj ET".into(), "/ColorSpace << /Pat /Pattern >>".into(), vec![]),
        (
            "a named Pattern space array alone",
            "/Pat cs BT /F1 10 Tf 10 100 Td (SECRET) Tj ET".into(),
            "/ColorSpace << /Pat [/Pattern /DeviceGray] >>".into(),
            vec![],
        ),
        ("a space that is no colour space", "/Pat cs BT /F1 10 Tf 10 100 Td (SECRET) Tj ET".into(), "/ColorSpace << /Pat 12 >>".into(), vec![]),
        // A default colour space that may be a Pattern space: `g` proves nothing.
        (
            "a default space that is a Pattern space",
            "0 g BT /F1 10 Tf 10 100 Td (SECRET) Tj ET".into(),
            "/ColorSpace << /DefaultGray /Pattern >>".into(),
            vec![],
        ),
        // The colour survives `q`, and comes back with `Q`.
        ("a restored pattern", "/Pattern cs /P1 scn q 0 g Q BT /F1 10 Tf 10 100 Td (SECRET) Tj ET".into(), String::new(), vec![]),
        // A soft mask's group may paint with patterns of its own.
        (
            "a soft mask",
            "/Gs gs BT /F1 10 Tf 10 100 Td (SECRET) Tj ET".into(),
            "/ExtGState << /Gs << /SMask << /S /Luminosity /G 9 0 R >> >> >>".into(),
            vec![stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"")],
        ),
        // A Type 3 glyph is a content stream this pass does not enter.
        (
            "Type 3 text",
            "BT /T3 10 Tf 10 100 Td (a) Tj ET".into(),
            type3.into(),
            vec![glyph.as_bytes().to_vec(), stream("", b"1000 0 d0 0 0 1000 1000 re f")],
        ),
        // A form under the mark that can't be read is removed with whatever it painted.
        (
            "a form removed unread",
            "/Fm Do".into(),
            "/XObject << /Fm 9 0 R >>".into(),
            vec![stream("/Type /XObject /Subtype /Form /BBox [0 90 50 120] /Filter /FlateDecode", b"not Flate")],
        ),
        // A form paints with the pattern it inherits, and with one it selects.
        (
            "a form inheriting the pattern",
            format!("/Pattern cs /P1 scn /Fm Do {FILLED}"),
            "/XObject << /Fm 9 0 R >>".into(),
            vec![stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"0 90 50 30 re f")],
        ),
        (
            "a form selecting the pattern",
            format!("/Fm Do {FILLED}"),
            "/XObject << /Fm 9 0 R >>".into(),
            vec![stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Pattern << /P1 7 0 R >> >>",
                b"/Pattern cs /P1 scn 0 90 50 30 re f",
            )],
        ),
    ];
    for (label, page, resources, extra) in cases {
        let mut objects = image_pattern();
        objects.extend(extra);
        let mut doc = one_page(page.as_bytes(), &format!("{PATTERN} {resources}"), objects);
        mark(&mut doc, 0, &[TEXT], "");
        assert!(refused(&mut doc), "{label}");
    }
}
