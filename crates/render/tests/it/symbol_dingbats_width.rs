//! Symbol and ZapfDingbats named without `/Widths` or `/Encoding` draw each glyph within the
//! advance their standard metrics give (ISO 32000-2 §9.6.2.2, Annex D.5 and D.6), through their
//! bundled stand-ins as the text fonts do: the next glyph starts one standard advance on, and the
//! glyph's ink stays inside that advance. ZapfDingbats `4` (a20, the check mark) is what a check
//! box's `/MK /CA` caption draws.

use crate::euro_width::{SIZE, ink_runs_in};
use pdfcraft_fonts::Std14;

/// The page's left margin, where the text starts, in pixels.
const ORIGIN: f64 = 20.0;

/// `codes` as a PDF literal string body, every byte octal-escaped.
fn literal(codes: &[u8]) -> String {
    codes.iter().map(|c| format!("\\{c:03o}")).collect()
}

/// For each `(code, glyph)` of `font` (`name`): drawn alone, the glyph's ink lies within its
/// standard advance (one pixel of rounding either side, as for the Euro), and drawn before
/// `follower`, the follower's ink starts one standard advance later than when drawn alone.
fn fits(name: &str, font: Std14, follower: u8, glyphs: &[(u8, &str)]) {
    let [(follower_alone, _), ..] = ink_runs_in(name, None, &literal(&[follower]))[..] else { panic!("{name}: the follower draws") };
    for &(code, glyph) in glyphs {
        let advance = font.advance(code, SIZE);
        assert!(advance > 0.0, "{name} {glyph}: a standard width");
        let alone = ink_runs_in(name, None, &literal(&[code]));
        let (Some(&(start, _)), Some(&(_, end))) = (alone.first(), alone.last()) else { panic!("{name} {glyph}: no ink") };
        assert!(f64::from(start) >= ORIGIN - 1.0, "{name} {glyph}: ink starts at its origin or after: {alone:?}");
        assert!(f64::from(end) + 1.0 <= ORIGIN + advance + 1.0, "{name} {glyph}: ink ends within {advance} px: {alone:?}");
        // The glyph is fitted, not squeezed: its ink fills most of the advance, as each of these does.
        assert!(f64::from(end - start + 1) >= 0.7 * advance, "{name} {glyph}: ink spans most of {advance} px: {alone:?}");

        let pair = ink_runs_in(name, None, &literal(&[code, follower]));
        let Some(&(next, _)) = pair.last() else { panic!("{name} {glyph}: no ink") };
        assert!(pair.len() > alone.len(), "{name} {glyph}: the follower stands apart: {pair:?}");
        assert!(
            (f64::from(next) - f64::from(follower_alone) - advance).abs() <= 1.0,
            "{name} {glyph}: the follower starts {advance} px on: {pair:?}"
        );
    }
}

/// Symbol's Greek letters and signs by their built-in codes (Annex D.5).
#[test]
fn symbol_glyphs_keep_their_standard_advances() {
    let glyphs = [
        (0x61, "alpha"),
        (0x62, "beta"),
        (0x70, "pi"),
        (0x53, "Sigma"),
        (0x57, "Omega"),
        (0x44, "Delta"),
        (0xA5, "infinity"),
        (0xB7, "bullet"),
        (0x2B, "plus"),
    ];
    fits("Symbol", Std14::Symbol, 0x53, &glyphs);
}

/// ZapfDingbats' check marks, crosses, dot, square, diamond and star by their built-in codes
/// (Annex D.6), the check box caption `4` among them.
#[test]
fn zapf_dingbats_glyphs_keep_their_standard_advances() {
    let glyphs = [
        (0x34, "a20 check"),
        (0x33, "a19 light check"),
        (0x37, "a23 ballot x"),
        (0x38, "a24 heavy ballot x"),
        (0x6C, "a71 dot"),
        (0x6E, "a73 square"),
        (0x75, "a77 diamond"),
        (0x48, "a28 star"),
    ];
    fits("ZapfDingbats", Std14::ZapfDingbats, 0x6E, &glyphs);
}
