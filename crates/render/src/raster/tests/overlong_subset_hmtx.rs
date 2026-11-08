//! A TrueType subset whose `hmtx` entry in the table directory still gives the length the table
//! had in the full font draws its glyphs (vendored hayro-interpret patch 76). That length runs
//! past the end of the subset, so skrifa found no `hmtx` and every glyph of the font drew
//! nothing. OpenType's `hmtx` holds four bytes for each of `hhea`'s `numberOfHMetrics` and two
//! for each other glyph `maxp` counts; that length is read instead when it ends before the next
//! table.
//!
//! The font is `symbolic_truetype_cmap`'s, built in memory, with only that length changed.

use super::render_first_page;
use super::symbolic_truetype_cmap::{disagreeing_font, page_with_symbolic_font};

/// What a 300-glyph font's `hmtx` takes: four bytes a glyph.
const FULL_FONT_HMTX_LENGTH: u32 = 4 * 300;

#[test]
fn a_subset_whose_hmtx_keeps_the_full_fonts_length_still_draws() {
    let mut font = disagreeing_font();
    let tables = usize::from(u16::from_be_bytes([font[4], font[5]]));
    let record = (0..tables).map(|i| 12 + 16 * i).find(|&at| font[at..at + 4] == *b"hmtx").expect("the test font has hmtx");
    let offset = u32::from_be_bytes([font[record + 8], font[record + 9], font[record + 10], font[record + 11]]);
    assert!(offset + FULL_FONT_HMTX_LENGTH > font.len() as u32, "the stale length runs past the end of the font");
    font[record + 12..record + 16].copy_from_slice(&FULL_FONT_HMTX_LENGTH.to_be_bytes());

    // The square covers (5, 5)–(35, 35) in page space; a substitute's `A` would not fill it.
    let page = render_first_page(page_with_symbolic_font(&font, "A"));
    let at = |x: u32, y: u32| page.rgba.get(((y * page.width + x) * 4) as usize).copied().unwrap_or(255);
    assert_eq!(at(2, 2), 255, "nothing is drawn outside the square");
    for (x, y) in [(8, 8), (31, 8), (20, 20), (8, 31), (31, 31)] {
        assert!(at(x, y) < 128, "the subset's square covers ({x}, {y})");
    }
}
