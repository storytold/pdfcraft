//! Hebrew and Arabic in the text of text boxes, callouts and Fill & Sign text.
//!
//! The standard fonts have no Hebrew or Arabic, so such text used to be written as `?`. When this
//! build has the craft-fonts face for the script (`CRAFT_FONTS_DIR`), each line is laid out in
//! display order (the Unicode bidirectional algorithm, UAX #9), the Hebrew and Arabic runs are
//! shaped with the face and their glyphs are drawn as filled outlines right in the appearance
//! stream. Latin text on the same line keeps Helvetica. An appearance is built from the
//! annotation dictionary alone ([`crate::appearance::build`] has no document to add font objects
//! to, and the display-only copies share one cache), so no font is embedded or subset: the
//! outlines carry the look, and each line is wrapped in an `/ActualText` span, with invisible
//! Helvetica glyphs under the outlines, so copy, search and extraction return the typed text
//! after flattening too.
//!
//! Without the face the text is drawn as `?` and reported, as before (`first_undrawable`).

use std::collections::HashMap;

use pdfcraft_fonts::{GlyphOutline, ShapedCluster, ShapedScript, helvetica_width, literal, shape_arabic, shape_hebrew, shaped_glyph, win_ansi};
use unicode_bidi::{Level, ParagraphBidiInfo};

use crate::{Rgb, n};

/// Arabic letters, marks, digits and punctuation (and the presentation forms), as Add text
/// classifies them.
fn is_arabic(c: char) -> bool {
    matches!(u32::from(c), 0x0600..=0x06FF | 0x0750..=0x077F | 0x0870..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFC)
}

/// Hebrew letters, points, cantillation marks and punctuation (and the presentation forms).
fn is_hebrew(c: char) -> bool {
    matches!(u32::from(c), 0x0591..=0x05F4 | 0xFB1D..=0xFB4F)
}

/// Direction marks, embeddings, overrides and isolates: they steer the order and are not drawn.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// The glyph a bracket takes in a right-to-left run.
fn mirrored(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '«' => '»',
        '»' => '«',
        other => other,
    }
}

/// Whether `text` has Hebrew or Arabic that a craft-fonts face in this build draws, so the
/// appearance lays it out with [`draw`] instead of writing it as `?`.
pub(crate) fn shaped_by_a_face(text: &str) -> bool {
    (pdfcraft_fonts::document_hebrew_font(false).is_some() && text.chars().any(is_hebrew))
        || (pdfcraft_fonts::document_arabic_font().is_some() && text.chars().any(is_arabic))
}

/// Whether a paragraph reads right to left: it has Hebrew or Arabic and its first strong
/// character is right-to-left.
pub(crate) fn paragraph_rtl(para: &str) -> bool {
    para.chars().any(|c| is_hebrew(c) || is_arabic(c)) && unicode_bidi::get_base_direction(para) == unicode_bidi::Direction::Rtl
}

/// A piece of a line in drawing order, left to right.
enum Piece {
    /// Drawn in Helvetica (WinAnsi; what it lacks comes out as `?`).
    Latin(String),
    Shaped(Vec<ShapedCluster>),
}

/// `line` in drawing order. `rtl` is its paragraph's direction. Hebrew and Arabic runs the
/// face shapes become [`Piece::Shaped`]; everything else, and a run the face can't shape (a
/// missing glyph, no face), stays Helvetica text, reversed and with its brackets mirrored in a
/// right-to-left run.
fn layout(line: &str, rtl: bool) -> Vec<Piece> {
    // unicode-bidi indexes the first level of a line, so an empty one would panic.
    if line.is_empty() {
        return Vec::new();
    }
    let base = if rtl { Level::rtl() } else { Level::ltr() };
    let info = ParagraphBidiInfo::new(line, Some(base));
    let (levels, runs) = info.visual_runs(0..line.len());
    let mut pieces = Vec::new();
    for run in runs {
        let run_rtl = levels.get(run.start).is_some_and(Level::is_rtl);
        // Runs lie on character boundaries; a missing slice is skipped, never a panic.
        let Some(text) = line.get(run) else { continue };
        // Hebrew and Arabic letters take their face; so do spaces and punctuation between them
        // (Hebrew's when they follow Hebrew), which the shaper then mirrors.
        let mut segments: Vec<(Option<ShapedScript>, String)> = Vec::new();
        for c in text.chars().filter(|c| !is_bidi_control(*c)) {
            let after_hebrew = segments.last().is_some_and(|(kind, _)| *kind == Some(ShapedScript::Hebrew));
            let script = if is_hebrew(c) || (run_rtl && after_hebrew && !c.is_alphanumeric() && pdfcraft_fonts::hebrew_has(c)) {
                Some(ShapedScript::Hebrew)
            } else if is_arabic(c) || (run_rtl && !c.is_alphanumeric() && pdfcraft_fonts::arabic_has(c)) {
                Some(ShapedScript::Arabic)
            } else {
                None
            };
            match segments.last_mut() {
                Some((kind, s)) if *kind == script => s.push(c),
                _ => segments.push((script, c.to_string())),
            }
        }
        if run_rtl {
            segments.reverse();
        }
        for (script, s) in segments {
            let shaped = match script {
                Some(ShapedScript::Hebrew) => shape_hebrew(&s, run_rtl).ok(),
                Some(ShapedScript::Arabic) => shape_arabic(&s, run_rtl).ok(),
                _ => None,
            };
            pieces.push(match shaped {
                Some(clusters) => Piece::Shaped(clusters),
                None if run_rtl => Piece::Latin(s.chars().rev().map(mirrored).collect()),
                None => Piece::Latin(s),
            });
        }
    }
    pieces
}

fn piece_width(p: &Piece, size: f64) -> f64 {
    match p {
        Piece::Latin(s) => helvetica_width(s, size),
        Piece::Shaped(clusters) => clusters.iter().map(|c| c.advance).sum::<f64>() * size,
    }
}

/// The width of one line at `size` points as the appearance draws it: Hebrew and Arabic the
/// face draws are measured shaped, the rest as Helvetica.
pub(crate) fn width(s: &str, size: f64) -> f64 {
    if !shaped_by_a_face(s) {
        return helvetica_width(s, size);
    }
    layout(s, paragraph_rtl(s)).iter().map(|p| piece_width(p, size)).sum()
}

/// [`pdfcraft_fonts::wrap`] measuring with [`width`], so Hebrew and Arabic lines break where
/// the drawing does.
pub(crate) fn wrap(text: &str, size: f64, width: f64) -> Vec<String> {
    pdfcraft_fonts::wrap_with(text, size, width, self::width)
}

/// The first character of `text` the appearance would draw as `?`: one outside WinAnsiEncoding
/// that no face draws, or Hebrew or Arabic the face lacks (or that has no face in this build).
pub(crate) fn first_undrawable(text: &str) -> Option<char> {
    if !shaped_by_a_face(text) {
        return pdfcraft_fonts::first_undrawable(text);
    }
    text.replace("\r\n", "\n").split(['\n', '\r']).find_map(|para| {
        let rtl = paragraph_rtl(para);
        layout(para, rtl).into_iter().find_map(|p| match p {
            // A run the face couldn't shape is Hebrew or Arabic here, so this names its letter.
            Piece::Latin(s) => pdfcraft_fonts::first_non_win_ansi(&s),
            Piece::Shaped(_) => None,
        })
    })
}

/// Most shaped clusters one appearance draws as outlines; the rest of the text is still there
/// in `/Contents`. Bounds the stream a hostile text could grow (each glyph is a path).
const MAX_CLUSTERS: usize = 4_000;

/// Draw `text` into the box `rect` (inset by `pad`) at `size` points in `color`, aligned by
/// `q` (`/Q`: 0 left, 1 centre, 2 right; a right-to-left paragraph with 0 starts at the right
/// edge). Lines wrap to the box; those below it are not drawn. Writes `out` after the box's
/// clip, outside any text object.
pub(crate) fn draw(out: &mut Vec<u8>, text: &str, rect: [f64; 4], pad: f64, size: f64, q: i64, color: Rgb) {
    let width = (rect[2] - rect[0] - 2.0 * pad).max(1.0);
    let [cr, cg, cb] = color.map(|v| v.clamp(0.0, 1.0));
    out.extend(format!("{} {} {} rg\n", n(cr), n(cg), n(cb)).bytes());
    let mut outlines: HashMap<(ShapedScript, u32), Option<GlyphOutline>> = HashMap::new();
    let mut drawn = 0usize;
    let mut y = rect[3] - pad - size * 0.9;
    for para in text.replace("\r\n", "\n").split(['\n', '\r']) {
        let rtl = paragraph_rtl(para);
        for line in wrap(para, size, width) {
            if y < rect[1] - size {
                return;
            }
            let pieces = layout(&line, rtl);
            let lw: f64 = pieces.iter().map(|p| piece_width(p, size)).sum();
            let x = match q {
                1 => rect[0] + pad + (width - lw) / 2.0,
                2 => rect[2] - pad - lw,
                _ if rtl => rect[2] - pad - lw,
                _ => rect[0] + pad,
            };
            // The typed line, so copy, search and extraction return it whatever is drawn.
            let logical: String = line.encode_utf16().map(|u| format!("{u:04X}")).collect();
            out.extend(format!("/Span <</ActualText <FEFF{logical}>>> BDC\n").bytes());
            let mut pen = x;
            for piece in &pieces {
                match piece {
                    Piece::Latin(s) => {
                        out.extend(format!("BT /Helv {} Tf 1 0 0 1 {} {} Tm ", n(size), n(pen), n(y)).bytes());
                        out.extend(literal(&win_ansi(s)));
                        out.extend_from_slice(b" Tj ET\n");
                        pen += helvetica_width(s, size);
                    }
                    Piece::Shaped(clusters) => {
                        let pw = piece_width(piece, size);
                        // Invisible glyphs under the outlines, one per cluster, stretched over
                        // the run (`Tz`): what carries the `/ActualText` for text extraction.
                        let count = clusters.len();
                        let qw = helvetica_width("?", size) * count as f64;
                        if count > 0 && pw > 0.0 && qw > 0.0 {
                            let tz = (100.0 * pw / qw).clamp(1.0, 10_000.0);
                            out.extend(format!("q BT 3 Tr {} Tz /Helv {} Tf 1 0 0 1 {} {} Tm ", n(tz), n(size), n(pen), n(y)).bytes());
                            out.extend(literal(&vec![b'?'; count]));
                            out.extend_from_slice(b" Tj ET Q\n");
                        }
                        for c in clusters {
                            if drawn < MAX_CLUSTERS {
                                drawn += 1;
                                cluster_outline(out, c, &mut outlines, pen, y, size);
                            }
                            pen += c.advance * size;
                        }
                    }
                }
            }
            out.extend_from_slice(b"EMC\n");
            y -= size * 1.2;
        }
    }
}

/// The filled outlines of one cluster's glyphs, with the pen at (`x`, `y`): glyph space (1000
/// units per em) scaled to `size` by a `cm`, so the coordinates stay integers.
fn cluster_outline(
    out: &mut Vec<u8>,
    c: &ShapedCluster,
    outlines: &mut HashMap<(ShapedScript, u32), Option<GlyphOutline>>,
    x: f64,
    y: f64,
    size: f64,
) {
    let mut paths = Vec::new();
    for (id, [dx, dy]) in &c.glyphs {
        let g = outlines.entry((c.script, *id)).or_insert_with(|| shaped_glyph(c.script, *id).ok());
        let Some(g) = g else { continue };
        for contour in &g.contours {
            for (j, p) in contour.iter().enumerate() {
                let (px, py) = (((p[0] + dx) * 1000.0).round(), ((p[1] + dy) * 1000.0).round());
                if !px.is_finite() || !py.is_finite() {
                    continue;
                }
                paths.extend(format!("{} {} {}\n", n(px), n(py), if j == 0 { "m" } else { "l" }).bytes());
            }
            paths.extend_from_slice(b"h\n");
        }
    }
    if paths.is_empty() {
        return;
    }
    let s = size / 1000.0;
    out.extend(format!("q {} 0 0 {} {} {} cm\n", n(s), n(s), n(x), n(y)).bytes());
    out.extend(paths);
    out.extend_from_slice(b"f Q\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hebrew_face() -> bool {
        if pdfcraft_fonts::document_hebrew_font(false).is_none() {
            eprintln!("skipping: built without a craft-fonts Hebr face (set CRAFT_FONTS_DIR)");
            return false;
        }
        true
    }

    #[test]
    fn without_a_face_hebrew_is_reported_and_measured_as_helvetica() {
        // Independent of the build: Thai has no path through this module.
        assert!(!shaped_by_a_face("ไทย"));
        assert_eq!(first_undrawable("a ไทย"), Some('ไ'));
        assert_eq!(width("ไทย", 10.0), helvetica_width("ไทย", 10.0));
        assert_eq!(first_undrawable("plain (text) 100%"), None);
        if hebrew_face() {
            return;
        }
        assert!(!shaped_by_a_face("שלום"));
        assert_eq!(first_undrawable("שלום"), Some('ש'));
    }

    #[test]
    fn hebrew_lines_read_right_to_left_with_latin_kept_in_helvetica() {
        if !hebrew_face() {
            return;
        }
        assert!(shaped_by_a_face("שלום") && !shaped_by_a_face("hello"));
        assert_eq!(first_undrawable("שלום (World) 123"), None);
        // A character no face draws is still named.
        assert_eq!(first_undrawable("שלום ไทย"), Some('ไ'));
        // Drawing order: in a right-to-left paragraph "World" is drawn left of the Hebrew.
        let pieces = layout("שלום World", true);
        assert!(matches!(pieces.first(), Some(Piece::Latin(s)) if s == "World"), "{}", pieces.len());
        // The space between them belongs to the right-to-left run, drawn first (leftmost).
        assert!(matches!(pieces.last(), Some(Piece::Shaped(c)) if c.iter().map(|c| c.text.as_str()).collect::<String>() == " םולש"));
        // Brackets mirror in a right-to-left run that stays Helvetica (no face shapes "(").
        let w = width("שלום (World)", 12.0);
        assert!(w > helvetica_width("(World) ", 12.0) && w.is_finite());
        let mut out = Vec::new();
        draw(&mut out, "שלום\nWorld", [0.0, 0.0, 200.0, 40.0], 2.0, 12.0, 0, [0.0, 0.0, 0.0]);
        let s = String::from_utf8_lossy(&out);
        // The Hebrew line: an ActualText span, invisible carrier glyphs and filled outlines.
        assert!(s.contains("/Span <</ActualText <FEFF05E905DC05D505DD>>> BDC"), "{s}");
        assert!(s.contains("3 Tr") && s.contains("(????) Tj ET Q"), "{s}");
        assert!(s.contains("f Q\n"), "{s}");
        // The Latin line is ordinary Helvetica text.
        assert!(s.contains("(World) Tj ET"), "{s}");
        // A right-to-left paragraph with /Q 0 ends at the right edge; a Latin one starts left.
        let right =
            s.find("Tz /Helv 12 Tf 1 0 0 1 ").and_then(|i| s.get(i + 23..)).and_then(|t| t.split(' ').next()).and_then(|x| x.parse::<f64>().ok());
        assert!(right.is_some_and(|x| x > 100.0), "{s}");
        assert!(s.contains("1 0 0 1 2 "), "{s}");
    }

    #[test]
    fn odd_text_never_panics() {
        for text in ["", "\u{05B0}", "\u{200F}\u{202E}", "a\u{05D0}\n\r\n\u{0627}", &"ש".repeat(5_000), "שלום\u{0}end", "(((", "\u{FB1D}\u{FB4F}"]
        {
            let _ = first_undrawable(text);
            let _ = width(text, 12.0);
            let _ = wrap(text, 12.0, 50.0);
            let mut out = Vec::new();
            draw(&mut out, text, [0.0, 0.0, 100.0, 50.0], 2.0, 12.0, 2, [0.0, 0.0, 1.0]);
            draw(&mut out, text, [0.0, 0.0, 1.0, 1.0], 2.0, 0.0, 1, [2.0, -1.0, f64::NAN]);
        }
    }
}
