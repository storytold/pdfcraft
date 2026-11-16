//! What the walk reads of a font or graphics state it sets again and again.

use super::*;

/// The page sets `/F1` 1,000 times, and a graphics state with a `/Font` and a line width 1,000
/// times: each name is read once in each of apply's two walks of the page (the rewrite, and the
/// check after it that nothing readable remains under the mark), not once for each time it is
/// set. The text under the mark goes, set either way.
#[test]
fn a_font_or_graphics_state_set_again_and_again_is_read_once() {
    let content = [
        b"BT ".to_vec(),
        b"/F1 10 Tf ".repeat(1000),
        b"20 250 Td (SECRET) Tj ".to_vec(),
        b"/G gs ".repeat(1000),
        b"0 -100 Td (OTHER) Tj ET".to_vec(),
    ]
    .concat();
    let mut doc = one_page(&content, "/ExtGState << /G << /Type /ExtGState /LW 2 /Font [5 0 R 10] >> >>", vec![]);
    mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
    crate::interp::FONT_STATE_READS.with(|c| c.set(0));
    assert_eq!(apply(&mut doc, None).map(|r| r.glyphs), Ok(6));
    assert_eq!(crate::interp::FONT_STATE_READS.with(std::cell::Cell::get), 4);
}

/// Two fonts set by turns, and two graphics states whose `/Font`s differ: each name is measured as
/// the font it sets, not as the one read first. The wide font's text moves the narrow text after
/// it under the mark, and only that goes.
#[test]
fn fonts_and_graphics_states_set_by_turns_are_each_read_as_themselves() {
    let font =
        |width: u32| format!("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 65 /LastChar 67 /Widths [{width} {width} {width}] >>");
    for (narrow, wide) in [("/F1 10 Tf", "/F2 10 Tf"), ("/G1 gs", "/G2 gs")] {
        let mut doc = pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R >> \
               /ExtGState << /G1 << /Font [5 0 R 10] >> /G2 << /Font [6 0 R 10] >> >> >> >>"
                .to_vec(),
            stream("", format!("BT 0 250 Td {narrow} (AAAA) Tj {wide} (BBBB) Tj {narrow} (CCCC) Tj ET").as_bytes()),
            font(100).into_bytes(),
            font(1000).into_bytes(),
        ]);
        mark(&mut doc, 0, &[[44.2, 230.0, 300.0, 280.0]], "");
        assert_eq!(apply(&mut doc, None).map(|r| r.glyphs), Ok(4), "{wide}");
        let c = content(&doc, 0);
        assert!(c.contains("AAAA") && c.contains("BBBB") && !c.contains("CCCC"), "{wide}: {c}");
    }
}

/// A graphics state's line width, cap, join and miter limit size the stroke the mark is tested
/// against, as their operators do: a 12-point width or a projecting cap reaches the mark, a round
/// join or a miter limit of 1 keeps a corner's reach short of it. Without either, the reverse.
#[test]
fn a_graphics_states_line_parameters_size_the_stroke_as_their_operators_do() {
    // The state's entry, its operator, the path, the mark, and whether the stroke reaches it.
    let rows = [
        ("/LW 12", "12 w", "10 60 m 100 60 l S", [40.0, 45.0, 60.0, 55.0], true),
        ("/LC 2", "2 J", "2 w 100 100 m 110 100 l S", [111.6, 100.0, 111.8, 100.1], true),
        ("/LJ 1", "1 j", "2 w 100 100 m 110 100 l 110 110 l S", [115.0, 105.0, 116.0, 106.0], false),
        ("/ML 1", "1 M", "2 w 100 100 m 110 100 l 110 110 l S", [115.0, 105.0, 116.0, 106.0], false),
    ];
    for (entry, operator, path, region, reaches) in rows {
        for (set, expected) in [("/G gs", reaches), (operator, reaches), ("", !reaches)] {
            let mut doc = one_page(format!("{set} {path}").as_bytes(), &format!("/ExtGState << /G << {entry} >> >>"), vec![]);
            mark(&mut doc, 0, &[region], "");
            assert_eq!(apply(&mut doc, None).map(|r| r.paths_removed), Ok(usize::from(expected)), "{entry}, set by `{set}`");
        }
    }
}

/// Names the resources don't have are looked up each time, not kept: a page making up a thousand
/// of them, each set twice by `Tf`, and a thousand more for `gs`, reads every one each time in each
/// of apply's two walks (a `gs` name twice, for its font and its line parameters).
#[test]
fn names_the_resources_do_not_have_are_not_kept() {
    let fonts: String = (0..1000).map(|i| format!("/N{i} 10 Tf /N{i} 10 Tf ")).collect();
    let states: String = (0..1000).map(|i| format!("/S{i} gs ")).collect();
    let content = format!("BT {fonts}{states}/F1 10 Tf 20 250 Td (SECRET) Tj ET");
    let mut doc = one_page(content.as_bytes(), "", vec![]);
    mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
    crate::interp::FONT_STATE_READS.with(|c| c.set(0));
    assert_eq!(apply(&mut doc, None).map(|r| r.glyphs), Ok(6));
    assert_eq!(crate::interp::FONT_STATE_READS.with(std::cell::Cell::get), 2 * (2 * 1000 + 2 * 1000 + 1));
}

/// A dash (`d`) whose operand is a graphics state's name is malformed, and the renderer doesn't read
/// it as that state: the stroke keeps the width it had, so a 1-point line stays beside a mark a
/// 12-point one would reach. (The walk once read such a `d` as a `gs` while a path was open.)
#[test]
fn a_dash_naming_a_graphics_state_is_not_read_as_one() {
    let mut doc = one_page(b"10 195 m /G d 100 195 l S", "/ExtGState << /G << /LW 12 >> >>", vec![]);
    mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    assert_eq!(apply(&mut doc, None).map(|r| r.paths_removed), Ok(0));
}

/// After the page is rewritten, the Type 3 font cleanup's pruning of the page's resources works out
/// which fonts its graphics states set only when the page lists a Type 3 font, and then reads each
/// state once, however often it is set: 10 or 1,000 `gs` cost its search for Type 3 fonts and its
/// pruning the same reads. (Its scan of a page that keeps drawing with a Type 3 font, as this one
/// does, still reads the state at each `gs`, and isn't counted here.)
#[test]
fn the_type3_cleanups_pruning_reads_a_graphics_state_once_and_only_for_a_type3_font() {
    let reads = |type3: bool, times: usize| {
        let shown = if type3 { "BT /T3 10 Tf 10 100 Td (A) Tj ET " } else { "" };
        let listed = if type3 { "/T3 6 0 R" } else { "" };
        let content = format!("{shown}{}BT /F1 10 Tf 20 250 Td (SECRET) Tj ET", "/G gs ".repeat(times));
        let mut doc = pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R {listed} >> /ExtGState << /G << /LW 2 /Font [5 0 R 10] >> >> >> >>")
                .into_bytes(),
            stream("", content.as_bytes()),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec(),
            b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 65 /Widths [500] /Encoding << /Differences [65 /A] >> /CharProcs << /A 7 0 R >> >>".to_vec(),
            stream("", b"% GLYPH_A"),
        ]);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        crate::type3::GS_FONT_READS.with(|c| c.set(0));
        assert_eq!(apply(&mut doc, None).map(|r| r.glyphs), Ok(6), "{type3} {times}");
        crate::type3::GS_FONT_READS.with(std::cell::Cell::get)
    };
    // One read is the cleanup looking for Type 3 fonts among the page's graphics states; with one
    // listed, a second works out whether `/G` sets it.
    for times in [10, 1000] {
        assert_eq!(reads(false, times), 1, "{times}");
        assert_eq!(reads(true, times), 2, "{times}");
    }
}

/// A font name holding a null byte (`/#00G`) is a name of its own: set by `Tf`, it doesn't stand for
/// the font a graphics state `/G` sets, nor the other way round. Ten A's shown in the state's wide
/// font after the narrow one was set by name run under the mark from x 70, five of them, which go;
/// shown in the narrow one after the state set the wide one, none do.
#[test]
fn a_font_name_with_a_null_byte_is_not_a_graphics_states_font() {
    let font = |width: u32| format!("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 65 /LastChar 65 /Widths [{width}] >>");
    for name in ["/XG", "/#00G"] {
        for (shows, glyphs) in [
            (format!("BT {name} 10 Tf ET /G gs BT 20 100 Td (AAAAAAAAAA) Tj ET"), 11),
            (format!("/G gs BT {name} 10 Tf 20 100 Td (AAAAAAAAAA) Tj ET"), 6),
        ] {
            let mut doc = pdf(vec![
                b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
                b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
                format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R {name} 6 0 R >> /ExtGState << /G << /Font [7 0 R 10] >> >> >> >>")
                    .into_bytes(),
                stream("", format!("BT /F1 10 Tf 20 250 Td (SECRET) Tj ET {shows}").as_bytes()),
                b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec(),
                font(100).into_bytes(),
                font(1000).into_bytes(),
            ]);
            mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0], [70.0, 95.0, 130.0, 115.0]], "");
            assert_eq!(apply(&mut doc, None).map(|r| r.glyphs), Ok(glyphs), "{shows}");
        }
    }
}
