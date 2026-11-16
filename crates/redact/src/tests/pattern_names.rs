//! The pattern names the walk records as painted, which the page's pattern cleanup keeps.

use super::*;

const CELL: &str = "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << >>";

/// The page selects a pattern whose name is 64 KiB long and paints with it 1,000 times, then in
/// each of 1,000 draws of a form without resources of its own over the mark: the name is looked
/// up once in each of apply's two walks of the page (the rewrite, and the check after it that
/// nothing readable remains under the mark), not once for each paint, nor recorded or looked up
/// again for each draw. The pattern stays.
#[test]
fn a_pattern_selected_once_is_recorded_once_however_often_it_paints() {
    let name = format!("P{}", "x".repeat((64 << 10) - 1));
    let content = [format!("/Pattern cs /{name} scn ").into_bytes(), b"0 0 1 1 re f\n".repeat(1000), b"/Fm Do\n".repeat(1000)].concat();
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"0 0 1 1 re f 0 0 1 1 re f");
    let mut doc = one_page(&content, &format!("/Pattern << /{name} 7 0 R >> /XObject << /Fm 8 0 R >>"), vec![stream(CELL, b"0 0 5 5 re f"), form]);
    mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    crate::interp::PAINTED_NAME_BYTES.with(|c| c.set(0));
    assert_eq!(apply(&mut doc, None).map(|r| r.patterns_removed), Ok(0));
    assert_eq!(crate::interp::PAINTED_NAME_BYTES.with(std::cell::Cell::get), 2 * name.len());
}

/// What a form selects and paints with counts for the page, as what the page selects as it draws a
/// form does. Beside the mark, `/Fm`, drawn with the page's fill `/P0` and stroke `/P1` selected
/// inside a `q` the page restores, fills inside a `q` of its own and strokes; `/Hm`, inside `/Gm`,
/// drawn with a plain fill, selects `/P3` itself and fills with it. The page paints with `/P2` and
/// `/P3` only under the mark. `/P0`, `/P1` and `/P3` stay; `/P2` goes.
#[test]
fn what_a_form_paints_with_counts_for_the_page() {
    let content = b"q /Pattern cs /P0 scn /Pattern CS /P1 SCN /Fm Do Q /Pattern cs /P3 scn 0 270 300 10 re f 0 g /Gm Do \
/Pattern cs /P2 scn 0 250 300 5 re f";
    let fm = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"q 0 0 1 1 re f Q 5 w 0 0 m 1 1 l S");
    let gm = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"/Hm Do");
    let hm = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"/Pattern cs /P3 scn 0 0 1 1 re f");
    let mut doc = one_page(
        content,
        "/Pattern << /P0 7 0 R /P1 7 0 R /P2 7 0 R /P3 7 0 R >> /XObject << /Fm 8 0 R /Gm 9 0 R /Hm 10 0 R >>",
        vec![stream(CELL, b"0 0 5 5 re f"), fm, gm, hm],
    );
    mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    assert_eq!(apply(&mut doc, None).map(|r| r.patterns_removed), Ok(1));
    let p = &pdfcraft_model::pages(&doc)[0];
    let res = doc.resolve(p.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let patterns = doc.resolve(res.get(b"Pattern").unwrap()).as_dict().cloned().unwrap();
    let kept: Vec<&[u8]> = patterns.iter().map(|(n, _)| n.as_slice()).collect();
    assert_eq!(kept, [b"P0".as_slice(), b"P1", b"P3"]);
}
