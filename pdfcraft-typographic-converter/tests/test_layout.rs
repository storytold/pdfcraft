use pdf_typographic_converter::layout::{build_layout, ScriptType};
use pdf_typographic_converter::extract::RawGlyph;

#[allow(clippy::too_many_arguments)]
fn make_glyph(
    char_code: char,
    font_name: &str,
    font_size: f32,
    is_bold: bool,
    is_italic: bool,
    left: f32,
    right: f32,
    origin_x: f32,
    origin_y: f32,
) -> RawGlyph {
    // top is baseline + 0.8 * font_size, bottom is baseline - 0.2 * font_size
    let top = origin_y + font_size * 0.8;
    let bottom = origin_y - font_size * 0.2;
    RawGlyph {
        char_code,
        font_name: font_name.to_string(),
        font_size,
        is_bold,
        is_italic,
        bounds: (left, top, right, bottom),
        origin_x,
        origin_y,
    }
}

#[test]
fn test_line_clustering_and_sorting() {
    // 3 lines at different heights: y=700, y=650, y=600
    // Shuffled input order to test sorting
    let glyphs = vec![
        make_glyph('B', "Helvetica", 12.0, false, false, 50.0, 58.0, 50.0, 650.0),
        make_glyph('C', "Helvetica", 12.0, false, false, 50.0, 58.0, 50.0, 600.0),
        make_glyph('A', "Helvetica", 12.0, false, false, 50.0, 58.0, 50.0, 700.0),
        // A glyph on line A slightly offset within 2.0 pt (y = 701.2)
        make_glyph('2', "Helvetica", 12.0, false, false, 60.0, 68.0, 60.0, 701.2),
    ];

    let page_data = vec![(0, 595.0, 842.0, glyphs)];
    let layout = build_layout(page_data);

    assert_eq!(layout.pages.len(), 1);
    let page = &layout.pages[0];
    assert_eq!(page.lines.len(), 3, "Expected 3 clustered lines");

    // Lines must be sorted top-to-bottom (descending baseline_y)
    assert!(page.lines[0].baseline_y > page.lines[1].baseline_y);
    assert!(page.lines[1].baseline_y > page.lines[2].baseline_y);

    assert!((page.lines[0].baseline_y - 700.6).abs() < 1.0);
    assert_eq!(page.lines[1].baseline_y, 650.0);
    assert_eq!(page.lines[2].baseline_y, 600.0);

    // Line 0 should contain 'A' and '2' sorted left to right
    assert_eq!(page.lines[0].runs[0].text, "A2");
}

#[test]
fn test_superscript_and_subscript_detection() {
    // Simulating: E = m c 2 (where 2 is superscript at y=501.8, font_size=8)
    // and H 2 O (where 2 is subscript at y=498.8, font_size=8)
    let glyphs = vec![
        make_glyph('E', "Times", 12.0, false, false, 50.0, 58.0, 50.0, 500.0),
        make_glyph('=', "Times", 12.0, false, false, 62.0, 70.0, 62.0, 500.0),
        make_glyph('m', "Times", 12.0, false, false, 74.0, 84.0, 74.0, 500.0),
        make_glyph('c', "Times", 12.0, false, false, 84.0, 91.0, 84.0, 500.0),
        // Superscript '2': font_size/12 = 8/12 = 0.667 < 0.78, y = 501.8 > 500 + 1.0
        make_glyph('2', "Times", 8.0, false, false, 91.5, 96.0, 91.5, 501.8),
    ];

    let page_data = vec![(0, 595.0, 842.0, glyphs)];
    let layout = build_layout(page_data);
    let line = &layout.pages[0].lines[0];

    assert_eq!(line.runs.len(), 2, "Expected 2 runs: base and superscript");
    assert_eq!(line.runs[0].text, "E = mc");
    assert_eq!(line.runs[0].script, ScriptType::Normal);
    assert_eq!(line.runs[1].text, "2");
    assert_eq!(line.runs[1].script, ScriptType::Superscript);

    // Subscript test: H 2 O
    let sub_glyphs = vec![
        make_glyph('H', "Times", 12.0, false, false, 50.0, 60.0, 50.0, 400.0),
        // Subscript '2': font_size=8, y=398.5 < 400 - 0.5
        make_glyph('2', "Times", 8.0, false, false, 60.5, 65.0, 60.5, 398.5),
        make_glyph('O', "Times", 12.0, false, false, 65.5, 75.0, 65.5, 400.0),
    ];

    let sub_layout = build_layout(vec![(0, 595.0, 842.0, sub_glyphs)]);
    let sub_line = &sub_layout.pages[0].lines[0];

    assert_eq!(sub_line.runs.len(), 3, "Expected 3 runs for H2O");
    assert_eq!(sub_line.runs[0].text, "H");
    assert_eq!(sub_line.runs[0].script, ScriptType::Normal);
    assert_eq!(sub_line.runs[1].text, "2");
    assert_eq!(sub_line.runs[1].script, ScriptType::Subscript);
    assert_eq!(sub_line.runs[2].text, "O");
    assert_eq!(sub_line.runs[2].script, ScriptType::Normal);
}

#[test]
fn test_word_gap_and_tab_stop() {
    let font_size = 10.0;
    // Word gap: >= 0.22 * 10 = 2.2, < 1.5 * 10 = 15.0
    // Tab stop: >= 1.5 * 10 = 15.0
    let glyphs = vec![
        // Word 1: "Hi"
        make_glyph('H', "Helvetica", font_size, false, false, 50.0, 58.0, 50.0, 500.0),
        make_glyph('i', "Helvetica", font_size, false, false, 58.0, 61.0, 58.0, 500.0),
        // Gap between 'i' (right=61.0) and 'Y' (left=64.0) is 3.0 -> word space!
        make_glyph('Y', "Helvetica", font_size, false, false, 64.0, 71.0, 64.0, 500.0),
        make_glyph('o', "Helvetica", font_size, false, false, 71.0, 77.0, 71.0, 500.0),
        make_glyph('u', "Helvetica", font_size, false, false, 77.0, 83.0, 77.0, 500.0),
        // Gap between 'u' (right=83.0) and 'E' (left=105.0) is 22.0 >= 15.0 -> TAB STOP!
        make_glyph('E', "Helvetica", font_size, false, false, 105.0, 112.0, 105.0, 500.0),
        make_glyph('n', "Helvetica", font_size, false, false, 112.0, 118.0, 112.0, 500.0),
        make_glyph('d', "Helvetica", font_size, false, false, 118.0, 125.0, 118.0, 500.0),
    ];

    let layout = build_layout(vec![(0, 595.0, 842.0, glyphs)]);
    let line = &layout.pages[0].lines[0];

    assert_eq!(line.runs.len(), 2, "Expected 2 runs separated by tab stop");
    assert_eq!(line.runs[0].text, "Hi You");
    assert!(!line.runs[0].has_leading_tab);

    assert_eq!(line.runs[1].text, "End");
    assert!(line.runs[1].has_leading_tab, "Run 2 must have has_leading_tab = true");
}

#[test]
fn test_style_change_run_splitting() {
    let glyphs = vec![
        make_glyph('R', "Helvetica", 12.0, false, false, 50.0, 58.0, 50.0, 500.0),
        make_glyph('e', "Helvetica", 12.0, false, false, 58.0, 64.0, 58.0, 500.0),
        make_glyph('g', "Helvetica", 12.0, false, false, 64.0, 70.0, 64.0, 500.0),
        // Bold starts here
        make_glyph('B', "Helvetica", 12.0, true, false, 70.0, 78.0, 70.0, 500.0),
        make_glyph('o', "Helvetica", 12.0, true, false, 78.0, 84.0, 78.0, 500.0),
        // Italic starts here
        make_glyph('I', "Helvetica", 12.0, false, true, 84.0, 88.0, 84.0, 500.0),
        make_glyph('t', "Helvetica", 12.0, false, true, 88.0, 92.0, 88.0, 500.0),
    ];

    let layout = build_layout(vec![(0, 595.0, 842.0, glyphs)]);
    let line = &layout.pages[0].lines[0];

    assert_eq!(line.runs.len(), 3, "Expected 3 runs for Normal, Bold, Italic");
    assert_eq!(line.runs[0].text, "Reg");
    assert!(!line.runs[0].is_bold && !line.runs[0].is_italic);

    assert_eq!(line.runs[1].text, "Bo");
    assert!(line.runs[1].is_bold && !line.runs[1].is_italic);

    assert_eq!(line.runs[2].text, "It");
    assert!(!line.runs[2].is_bold && line.runs[2].is_italic);
}

#[test]
fn test_overlapping_accents_kerning() {
    // Gap < -1.0 should preserve glyph without space
    let glyphs = vec![
        make_glyph('e', "Helvetica", 12.0, false, false, 50.0, 58.0, 50.0, 500.0),
        // Acute accent positioned over 'e', bounds.0 = 53.0 < 58.0 - 1.0 (gap = -5.0)
        make_glyph('´', "Helvetica", 12.0, false, false, 52.0, 56.0, 52.0, 500.0),
        make_glyph('s', "Helvetica", 12.0, false, false, 58.0, 64.0, 58.0, 500.0),
    ];

    let layout = build_layout(vec![(0, 595.0, 842.0, glyphs)]);
    let line = &layout.pages[0].lines[0];

    assert_eq!(line.runs.len(), 1);
    assert_eq!(line.runs[0].text, "e´s");
    assert!(!line.runs[0].text.contains(' '));
}
