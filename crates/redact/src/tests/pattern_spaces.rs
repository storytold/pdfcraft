//! `cs` and `CS` reset the colour, and with it the pattern selected before, unless the renderer
//! takes the space for a Pattern space: then it keeps the pattern (ISO 32000-2 §8.6.8 and Poppler
//! reset that too). It reads the name as the renderer does: `/Pattern` itself, a device space's
//! name never, and otherwise the first `/ColorSpace` that has the name, the scope's own resources
//! first, then those of the scopes that drew it. A pattern the renderer still paints with is kept;
//! one it doesn't leaves the page.

use super::*;
use crate::interp::SPACE_READS;

/// A tiling pattern whose cell draws a rectangle and nothing else (object 7), which can stay.
fn cell() -> Vec<u8> {
    stream("/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 300 300] /XStep 300 /YStep 300 /Resources << >>", b"0 0 300 300 re f")
}

/// Fills a box under the mark with `/P0`, then a box beside it after `{space} cs`.
fn two_boxes(space: &str) -> String {
    format!("/Pattern cs /P0 scn 20 140 100 30 re f {space} cs 20 40 100 30 re f")
}

/// How many patterns applying a mark over the first box removes from a page drawing `content`,
/// with `res` its resources (besides the font), the cell as object 7 and `extra` from object 8.
fn removed(content: &str, res: &str, extra: Vec<Vec<u8>>) -> usize {
    let mut objs = vec![cell()];
    objs.extend(extra);
    let mut doc = one_page(content.as_bytes(), res, objs);
    mark(&mut doc, 0, &[[15.0, 135.0, 130.0, 175.0]], "");
    apply(&mut doc, None).unwrap().patterns_removed
}

/// A space the resources give as `/Pattern` or `[/Pattern base]`, directly or by reference at any
/// level, keeps the pattern the box beside the mark is filled with, as does `/Pattern` itself even
/// where the resources name something else so; and a `cs` whose operand isn't a name, which the
/// renderer skips. A grey space, a device space's name (read before the resources), a name the
/// resources don't have, a name naming a name, and `null` reset it, and the pattern leaves the
/// page. `CS` likewise for the stroke.
#[test]
fn a_named_pattern_space_keeps_the_pattern() {
    let pattern = "/Pattern << /P0 7 0 R >>";
    for (space, spaces, extra, gone) in [
        ("/CSP", "<< /CSP /Pattern >>", None, 0),
        ("/CSP", "<< /CSP [/Pattern /DeviceRGB] >>", None, 0),
        ("/CSP", "<< /CSP [/Pattern] >>", None, 0),
        ("/Pattern", "<< /Pattern /DeviceGray >>", None, 0),
        ("(x)", "<< >>", None, 0),
        ("/CSP", "8 0 R", Some("<< /CSP /Pattern >>"), 0),
        ("/CSP", "<< /CSP 8 0 R >>", Some("/Pattern"), 0),
        ("/CSP", "<< /CSP 8 0 R >>", Some("[/Pattern /DeviceRGB]"), 0),
        ("/CSP", "<< /CSP [8 0 R /DeviceRGB] >>", Some("/Pattern"), 0),
        ("/CSG", "<< /CSG /DeviceGray >>", None, 1),
        ("/G", "<< /G /Pattern >>", None, 1),
        ("/CSX", "<< /CSP /Pattern >>", None, 1),
        ("/CSP", "<< /CSP /CSQ /CSQ /Pattern >>", None, 1),
        ("/CSP", "<< /CSP null >>", None, 1),
        ("/CSP", "<< /CSP 8 0 R >>", Some("/DeviceGray"), 1),
    ] {
        let extra: Vec<Vec<u8>> = extra.map(|e| e.as_bytes().to_vec()).into_iter().collect();
        let res = format!("{pattern} /ColorSpace {spaces}");
        assert_eq!(removed(&two_boxes(space), &res, extra), gone, "{space} cs with /ColorSpace {spaces}");
    }
    let stroked = |space: &str| format!("/Pattern CS /P0 SCN 20 140 100 30 re S {space} CS 20 40 100 30 re S");
    let res = format!("{pattern} /ColorSpace << /CSP /Pattern /CSG /DeviceGray >>");
    assert_eq!(removed(&stroked("/CSP"), &res, vec![]), 0, "CS of a named Pattern space");
    assert_eq!(removed(&stroked("/CSG"), &res, vec![]), 1, "CS of a grey space");
}

/// A form without resources of its own paints with the page's patterns and reads its colour space
/// names in the page's resources, as the renderer does: the page selects `/P0` and fills the box
/// under the mark, and the form, drawn after `0 g`, selects it again and fills after `/CSP cs`, so
/// the page's `/CSP` decides whether the pattern still paints.
#[test]
fn a_form_without_resources_reads_the_pages_colour_spaces() {
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"/P0 scn /CSP cs 20 40 100 30 re f");
    let content = "/Pattern cs /P0 scn 20 140 100 30 re f 0 g /Fm Do";
    for (spaces, gone) in [("<< /CSP /Pattern >>", 0), ("<< /CSP /DeviceGray >>", 1)] {
        let res = format!("/Pattern << /P0 7 0 R >> /XObject << /Fm 8 0 R >> /ColorSpace {spaces}");
        assert_eq!(removed(content, &res, vec![form.clone()]), gone, "/ColorSpace {spaces}");
    }
}

/// What `cs` reads by reference is read once for the page, however often `cs` names it, however
/// many times a form naming it is drawn, and however deep the forms nest: a large object, read
/// again, can be parsed again, all of it. A `/ColorSpace` given by reference, an entry of it given
/// by reference and an entry's first element given by reference are read as often for 20,000 `cs`
/// as for one; and a `/ColorSpace` of 5,000 spaces as often for 1,000 drawings of a form as for
/// one, and for forms 40 deep as for one.
#[test]
fn a_colour_space_dictionary_is_read_once_for_the_page() {
    let spaces: String = (0..5000).map(|i| format!("/C{i} /DeviceGray ")).collect();
    let spaces = format!("<< {spaces}/CSP /Pattern >>").into_bytes();
    let reads = |content: &str, res: &str, extra: Vec<Vec<u8>>| {
        SPACE_READS.with(|c| c.set(0));
        assert_eq!(removed(content, res, extra), 0, "{}", &content[..content.len().min(60)]);
        SPACE_READS.with(std::cell::Cell::get)
    };
    let many = format!("/Pattern cs /P0 scn 20 140 100 30 re f {}20 40 100 30 re f", "/CSP cs ".repeat(20_000));
    // The dictionary by reference, an entry by reference, and an entry's first element by
    // reference.
    for (spaces_entry, extra) in
        [("8 0 R", spaces.clone()), ("<< /CSP 8 0 R >>", b"[/Pattern /DeviceRGB]".to_vec()), ("<< /CSP [8 0 R /DeviceRGB] >>", b"/Pattern".to_vec())]
    {
        let res = format!("/Pattern << /P0 7 0 R >> /ColorSpace {spaces_entry}");
        let once = reads(&two_boxes("/CSP"), &res, vec![extra.clone()]);
        assert!(once >= 1, "{spaces_entry} is read");
        assert_eq!(reads(&many, &res, vec![extra]), once, "20,000 cs with /ColorSpace {spaces_entry}");
    }

    // A form with its own `/ColorSpace` (object 9) drawn once, then 1,000 times.
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /ColorSpace 9 0 R >>", b"/CSP cs 0 0 1 1 re f");
    let drawn = |n: usize| reads(&"/Fm Do ".repeat(n), "/XObject << /Fm 8 0 R >>", vec![form.clone(), spaces.clone()]);
    let one = drawn(1);
    assert!(one >= 1, "the form's dictionary is read");
    assert_eq!(drawn(1000), one, "a form drawn 1,000 times");

    // Forms 8 to 47, each with the same `/ColorSpace` (object 48), each naming a space and drawing
    // the next.
    let nested = |depth: usize| {
        let mut extra: Vec<Vec<u8>> = (0..depth)
            .map(|k| {
                let next = if k + 1 < depth { format!("/XObject << /Fn {} 0 R >>", 9 + k) } else { String::new() };
                let draw = if k + 1 < depth { "/Fn Do" } else { "" };
                stream(
                    &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /ColorSpace {} 0 R {next} >>", 8 + depth),
                    format!("/CSP cs {draw}").as_bytes(),
                )
            })
            .collect();
        extra.push(spaces.clone());
        reads("/Fm Do", "/XObject << /Fm 8 0 R >>", extra)
    };
    assert_eq!(nested(40), nested(1), "forms 40 deep");
}
