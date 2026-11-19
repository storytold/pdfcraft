//! Self-contained shaped text for generated appearances. Font streams remain inline in the
//! returned resource template; its owner must promote them to indirect objects before saving.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;

use pdfcraft_cos::{Dict, Document, Object, PdfString, Stream};
use unicode_bidi::{Level, ParagraphBidiInfo};
use unicode_segmentation::UnicodeSegmentation;

use crate::{GlyphError, GlyphOutline, ShapedCluster, ShapedScript, Std14, literal, shaped_glyph, win_ansi};

/// The resource names of the fonts this writer makes begin with this.
const SHAPED_FONT: &str = "PCShaped";

/// Direction marks affect ordering, but have no visible glyph.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

fn script(c: char) -> Option<ShapedScript> {
    match u32::from(c) {
        0x0600..=0x06ff | 0x0750..=0x077f | 0x0870..=0x08ff | 0xfb50..=0xfdff | 0xfe70..=0xfefc => Some(ShapedScript::Arabic),
        0x0591..=0x05f4 | 0xfb1d..=0xfb4f => Some(ShapedScript::Hebrew),
        0x0e00..=0x0e7f => Some(ShapedScript::Thai),
        _ => None,
    }
}

fn has(kind: ShapedScript, c: char) -> bool {
    match kind {
        ShapedScript::Arabic => crate::arabic_has(c),
        ShapedScript::Hebrew => crate::hebrew_has(c),
        ShapedScript::Thai => crate::thai_has(c),
        // [`script`] never names the bundled Latin face: Latin here is drawn in the standard font.
        ShapedScript::Latin => crate::latin_has(c),
    }
}

/// Whether `text` needs this writer: it has Arabic or Hebrew, which are laid out in logical
/// order. Any other text keeps the writer it had, invisible controls and all: Thai alone is
/// drawn by [`crate::shaped_text`] (its appearances in forms, comments and page marks); with
/// Arabic or Hebrew this writer shapes it too.
pub fn has_shaped_text(text: &str) -> bool {
    // (U+061C, the Arabic letter mark, is a direction mark in the Arabic block: not a letter.)
    text.chars().any(|c| matches!(script(c), Some(ShapedScript::Arabic | ShapedScript::Hebrew)) && !crate::embedding::default_ignorable(c))
}

/// Whether contextual-script runs have their face. Other unsupported characters may draw as
/// marked placeholders without replacing the supported Arabic run with placeholders too.
pub fn can_shape(text: &str) -> bool {
    text.chars().all(|c| crate::embedding::default_ignorable(c) || script(c).is_none_or(|kind| has(kind, c)))
}

pub fn paragraph_rtl(text: &str) -> bool {
    unicode_bidi::get_base_direction(text) == unicode_bidi::Direction::Rtl
}

/// Characters that cannot be drawn are still kept in the annotation's original Contents.
pub fn first_undrawable(text: &str) -> Option<char> {
    text.chars().find(|&c| {
        if crate::embedding::default_ignorable(c) || matches!(c, '\n' | '\r' | '\t' | '\u{0085}' | '\u{2028}' | '\u{2029}') {
            false
        } else if let Some(kind) = script(c) {
            !has(kind, c)
        } else {
            !crate::win_ansi_encodable(c)
        }
    })
}

#[derive(Clone, Debug)]
enum Piece {
    Latin(String),
    Shaped(Vec<ShapedCluster>),
}

#[derive(Clone, Debug)]
pub struct Line {
    /// Unmodified logical text, including spaces and joining controls.
    pub text: String,
    pub rtl: bool,
    pub width: f64,
    /// Safe baseline-to-top/bottom distances in em units, including positioned marks.
    pub ascent: f64,
    pub descent: f64,
    /// The standard face the line's Latin is measured and drawn in.
    latin: Std14,
    pieces: Vec<Piece>,
}

/// Positioned cluster ink in em units. Cached font bounds include every attached mark.
fn cluster_ink_bounds(cluster: &ShapedCluster) -> Result<[f64; 4], GlyphError> {
    let mut bounds = [0.0f64; 4];
    for (glyph, [dx, dy]) in &cluster.glyphs {
        let glyph = crate::shaped_bounds(cluster.script, *glyph)?;
        bounds = [bounds[0].min(glyph[0] + dx), bounds[1].min(glyph[1] + dy), bounds[2].max(glyph[2] + dx), bounds[3].max(glyph[3] + dy)];
    }
    Ok(bounds)
}

fn layout(text: &str, bidi: &ParagraphBidiInfo<'_>, range: Range<usize>, size: f64, rtl: bool, width: f64, latin: Std14) -> Result<Line, GlyphError> {
    let line = layout_pass(text, bidi, range.clone(), size, rtl, false, latin)?;
    if line.width > width && ends_its_line(text, range.end) && has_hanging_space(&line.text) {
        layout_pass(text, bidi, range, size, rtl, true, latin)
    } else {
        Ok(line)
    }
}

fn layout_pass(
    text: &str,
    bidi: &ParagraphBidiInfo<'_>,
    range: Range<usize>,
    size: f64,
    rtl: bool,
    hang_final: bool,
    latin: Std14,
) -> Result<Line, GlyphError> {
    let continued = !ends_its_line(text, range.end) || hang_final;
    let logical = text.get(range.clone()).ok_or(GlyphError::Missing)?;
    let displayed = display_line(logical, continued);
    let text = displayed.as_ref();
    let mut pieces = Vec::new();
    if !text.is_empty() {
        let line = line_bidi(bidi, range.clone(), text)?;
        let (levels, runs) = line.visual_runs(0..text.len());
        for run in runs {
            let run_rtl = levels.get(run.start).is_some_and(Level::is_rtl);
            let Some(input) = text.get(run.clone()) else { continue };
            let mut segments: Vec<(Option<ShapedScript>, String)> = Vec::new();
            let first_script = input.chars().find_map(script);
            for c in input.chars().filter(|c| !is_bidi_control(*c)) {
                let previous = segments.last().and_then(|(kind, _)| *kind);
                if crate::embedding::default_ignorable(c) && previous.or(first_script).is_none() {
                    continue;
                }
                let kind = script(c)
                    .or_else(|| crate::embedding::default_ignorable(c).then_some(previous.or(first_script)).flatten())
                    .or_else(|| (run_rtl && !c.is_alphanumeric()).then(|| previous.or(first_script).filter(|kind| has(*kind, c))).flatten());
                match segments.last_mut() {
                    Some((last, s)) if *last == kind => s.push(c),
                    _ => segments.push((kind, c.to_string())),
                }
            }
            if run_rtl {
                segments.reverse();
            }
            for (kind, s) in segments {
                pieces.push(match kind {
                    Some(ShapedScript::Arabic) => Piece::Shaped(crate::shape_arabic(&s, run_rtl)?),
                    Some(ShapedScript::Hebrew) => Piece::Shaped(crate::shape_hebrew(&s, run_rtl)?),
                    Some(ShapedScript::Thai) => Piece::Shaped(crate::shape_thai(&s)?),
                    Some(ShapedScript::Latin) => Piece::Shaped(crate::shape_latin(&s)?),
                    None if run_rtl => Piece::Latin(s.chars().rev().map(mirrored).collect()),
                    None => Piece::Latin(s),
                });
            }
        }
    }
    let width = pieces
        .iter()
        .map(|piece| match piece {
            Piece::Latin(s) => latin.text_width(s, size),
            Piece::Shaped(cs) => cs.iter().map(|c| c.advance * size).sum(),
        })
        .sum();
    let (mut ascent, mut descent) = (0.9f64, 0.3f64);
    for piece in &pieces {
        let Piece::Shaped(clusters) = piece else { continue };
        for cluster in clusters {
            let bounds = cluster_ink_bounds(cluster)?;
            ascent = ascent.max(bounds[3] + 0.05);
            descent = descent.max(-bounds[1] + 0.05);
        }
    }
    Ok(Line { text: logical.to_owned(), rtl, width, ascent, descent, latin, pieces })
}

/// Whether byte `end` of paragraph `text` is where a line ends by itself: the paragraph's end, or
/// a line separator. A line that ends there was not wrapped, so it shows no discretionary hyphen
/// and its trailing spaces keep their advance.
fn ends_its_line(text: &str, end: usize) -> bool {
    text.get(end..).is_none_or(|rest| rest.is_empty() || rest.starts_with('\u{2028}'))
}

/// Text used only for measuring/drawing a wrapped line. A soft hyphen is invisible inside a
/// word; at a selected continuation break it draws a hyphen. Retain the original logical line
/// separately for ActualText and future editing. Soft-wrap separators hang at the line end;
/// indentation, entirely whitespace rows and paragraph-final spaces retain their advances.
fn display_line(text: &str, continued: bool) -> Cow<'_, str> {
    match text.strip_suffix('\u{00ad}').filter(|_| continued) {
        Some(prefix) => Cow::Owned(format!("{prefix}-")),
        None if continued && has_hanging_space(text) => Cow::Borrowed(text.trim_end_matches(soft_space)),
        None => Cow::Borrowed(text),
    }
}

/// A space a line may wrap at and hang past its end (not a no-break space or a line break).
fn soft_space(c: char) -> bool {
    c.is_whitespace()
        && !matches!(c, '\u{00a0}' | '\u{2007}' | '\u{202f}' | '\n' | '\r' | '\u{000b}' | '\u{000c}' | '\u{0085}' | '\u{2028}' | '\u{2029}')
}

/// Whether trailing breakable spaces can hang without removing visible content, indentation,
/// nonbreaking spaces or an intentionally entirely whitespace row.
fn has_hanging_space(text: &str) -> bool {
    text.chars().next_back().is_some_and(soft_space) && text.chars().any(|c| !c.is_whitespace() && !crate::embedding::default_ignorable(c))
}

/// Slice a paragraph's already resolved bidi levels for one display line. Only the line is
/// cloned before UAX #9 L1/L2 processing, bounding work on paragraphs with many short lines.
/// `displayed` is the logical range or its [`display_line`] discretionary-hyphen replacement.
fn line_bidi<'a>(paragraph: &ParagraphBidiInfo<'_>, range: Range<usize>, displayed: &'a str) -> Result<ParagraphBidiInfo<'a>, GlyphError> {
    let logical = paragraph.text.get(range.clone()).ok_or(GlyphError::Missing)?;
    if displayed.len() > logical.len() {
        return Err(GlyphError::Missing);
    }
    let mut line = ParagraphBidiInfo {
        text: displayed,
        original_classes: paragraph.original_classes.get(range.clone()).ok_or(GlyphError::Missing)?.to_vec(),
        levels: paragraph.levels.get(range).ok_or(GlyphError::Missing)?.to_vec(),
        paragraph_level: paragraph.paragraph_level,
        is_pure_ltr: paragraph.is_pure_ltr,
    };
    line.original_classes.truncate(displayed.len());
    line.levels.truncate(displayed.len());
    if logical.ends_with('\u{00ad}') && displayed.ends_with('-') && displayed.len().saturating_add(1) == logical.len() {
        // A visible discretionary hyphen inherits its word's resolved direction. Its old BN
        // class must not make UAX #9 L1 reset the visible character to the paragraph base.
        let previous = line.levels.get(displayed.len().saturating_sub(2)).copied().unwrap_or(line.paragraph_level);
        if let Some(class) = line.original_classes.last_mut() {
            *class = unicode_bidi::BidiClass::ES;
        }
        if let Some(level) = line.levels.last_mut() {
            *level = previous;
        }
    }
    Ok(line)
}

/// Required ink-safe height at the chosen point size, shared by appearance drawing and fitting.
pub fn height(lines: &[Line], size: f64) -> f64 {
    lines.iter().map(|line| (line.ascent + line.descent) * size).sum()
}

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

/// Wrap in logical order at word boundaries, breaking an overwide word only between graphemes.
/// Every character is retained, so indentation and repeated spaces survive editing and saving.
/// The paragraph direction stays the same on its continuation lines.
pub fn wrap(text: &str, size: f64, width: f64) -> Result<Vec<Line>, GlyphError> {
    wrap_with_latin(text, size, width, Std14::Helvetica)
}

/// [`wrap`] in a text box's own standard face: Arabic and Hebrew keep their script faces, and
/// the Latin beside them is measured and drawn in `latin`.
pub fn wrap_with_latin(text: &str, size: f64, width: f64, latin: Std14) -> Result<Vec<Line>, GlyphError> {
    if text.len() > 262_144 || !size.is_finite() || size <= 0.0 || !width.is_finite() || width <= 0.0 {
        return Err(GlyphError::TooComplex);
    }
    // The symbol faces have no WinAnsi letters to draw Latin with.
    if matches!(latin, Std14::Symbol | Std14::ZapfDingbats) {
        return Err(GlyphError::Missing);
    }
    // A CR, a CRLF pair, a next line (NEL) and a paragraph separator each end a paragraph, as a
    // line feed does (UAX #9 P1 resolves each paragraph on its own).
    let normalized = text.replace("\r\n", "\n").replace(['\r', '\u{0085}', '\u{2029}'], "\n");
    let mut out = Vec::new();
    for paragraph in normalized.split('\n') {
        let rtl = unicode_bidi::get_base_direction(paragraph) == unicode_bidi::Direction::Rtl;
        // UAX #9 resolves embeddings and isolates for the paragraph, then reorders each line.
        // Re-running it on a detached line loses an opener that was on a previous line.
        let bidi = ParagraphBidiInfo::new(paragraph, Some(if rtl { Level::rtl() } else { Level::ltr() }));
        // A line separator starts a new line inside the paragraph: to UAX #9 it is whitespace,
        // so the lines after it keep the paragraph's direction and its open embeddings.
        let mut from = 0usize;
        for forced in paragraph.split('\u{2028}') {
            let within = |range: Range<usize>| from.saturating_add(range.start)..from.saturating_add(range.end);
            let ranges = wrap_ranges(forced, width, |range| {
                layout(paragraph, &bidi, within(range), size, rtl, width, latin).map_or(f64::INFINITY, |l| l.width)
            });
            for range in ranges {
                out.push(layout(paragraph, &bidi, within(range), size, rtl, width, latin)?);
            }
            from = from.saturating_add(forced.len()).saturating_add('\u{2028}'.len_utf8());
        }
    }
    Ok(out)
}

/// Logical paragraph ranges after wrapping. Measurement receives original byte ranges so
/// callers can reuse paragraph-level bidi state instead of resolving each detached prefix.
fn wrap_ranges(paragraph: &str, width: f64, measure: impl Fn(Range<usize>) -> f64) -> Vec<Range<usize>> {
    let mut boundaries: Vec<usize> = paragraph.grapheme_indices(true).map(|(at, _)| at).collect();
    boundaries.push(paragraph.len());
    let count = boundaries.len().saturating_sub(1);
    if count == 0 {
        return std::iter::once(0..0).collect();
    }
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < count {
        let start = boundaries.get(at).copied().unwrap_or(paragraph.len());
        let range = |end: usize| start..boundaries.get(end).copied().unwrap_or(paragraph.len());
        let ends_in_hyphen = |end: usize| paragraph.get(range(end)).is_some_and(|s| s.ends_with('\u{00ad}'));
        // How wide the text up to `end` is where the line goes on past it: a discretionary
        // hyphen at its end is not shown. Shown, it would make a shorter prefix wider than a
        // longer one, and the search below would stop at a hyphen inside a word that fits.
        let unbroken = |end: usize| {
            let whole = range(end);
            let shown = paragraph.get(whole.clone()).map_or(whole.end, |s| whole.start + s.trim_end_matches('\u{00ad}').len());
            if shown > whole.start { measure(whole.start..shown) } else { 0.0 }
        };
        // Exponential probing then binary search avoids reshaping every growing prefix of an
        // arbitrarily long word. Each emitted line is measured again after its chosen break.
        let mut fit = at;
        let mut probe = at + 1;
        let mut step = 1usize;
        loop {
            let measured = unbroken(probe);
            if measured > width || !measured.is_finite() {
                break;
            }
            fit = probe;
            if fit == count {
                break;
            }
            step = step.saturating_mul(2);
            probe = at.saturating_add(step).min(count);
        }
        let mut high = probe;
        while high.saturating_sub(fit) > 1 {
            let middle = fit + (high - fit) / 2;
            let measured = unbroken(middle);
            if measured.is_finite() && measured <= width {
                fit = middle;
            } else {
                high = middle;
            }
        }
        // A single overwide grapheme cannot be split without separating a mark from its base.
        let mut end = fit.max(at + 1);
        // Nor is it put on a row of its own under the characters without width before it (soft
        // hyphens, zero-width spaces): they would make a row with nothing on it, one each.
        let no_width = |from: usize, to: usize| paragraph.get(from..to).is_some_and(|s| s.chars().all(crate::embedding::default_ignorable));
        if end < count && no_width(start, range(end).end) {
            while end < count {
                let added = range(end).end..range(end + 1).end;
                end += 1;
                if !no_width(added.start, added.end) {
                    break;
                }
            }
        }
        if end < count {
            // Where the row's first character with ink ends. A break before it would leave a row
            // of indentation or of invisible characters above the text.
            // (Looked for in this row alone, so rows of spaces don't each read the rest of the text.)
            let ink = paragraph.get(range(end)).and_then(|row| {
                let first = row.char_indices().find(|(_, c)| !c.is_whitespace() && !crate::embedding::default_ignorable(*c));
                first.map(|(i, c)| start.saturating_add(i).saturating_add(c.len_utf8()))
            });
            // A line may end after a space, a zero-width space or a discretionary hyphen.
            let breaks_after = |boundary: usize| {
                let previous = boundaries.get(boundary.saturating_sub(1)).copied().unwrap_or(start);
                let next = boundaries.get(boundary).copied().unwrap_or(previous);
                ink.is_some_and(|ink| next >= ink)
                    && paragraph.get(previous..next).is_some_and(|s| s.chars().all(|c| soft_space(c) || matches!(c, '\u{200b}' | '\u{00ad}')))
            };
            // The last such place where the line fits as it is drawn when broken there. A space
            // or a zero-width space hangs or has no width, so the last of those fits; a hyphen is
            // shown, so it may not.
            let plain = (at + 1..=end).rev().find(|boundary| breaks_after(*boundary) && !ends_in_hyphen(*boundary));
            // The hyphens after that place, in order. The text before a later one is no shorter,
            // so the ones that fit come first: the last of them is found by halving, in a number
            // of measurements that grows with the logarithm of how many there are.
            let hyphens: Vec<usize> =
                (plain.unwrap_or(at) + 1..=end).filter(|boundary| ends_in_hyphen(*boundary) && breaks_after(*boundary)).collect();
            let (mut fitting, mut over) = (0usize, hyphens.len());
            while fitting < over {
                let middle = fitting + (over - fitting) / 2;
                if hyphens.get(middle).is_some_and(|boundary| measure(range(*boundary)) <= width) {
                    fitting = middle + 1;
                } else {
                    over = middle;
                }
            }
            match fitting.checked_sub(1).and_then(|last| hyphens.get(last)).copied().or(plain) {
                Some(boundary) => end = boundary,
                // With none, the line is cut between graphemes, before a hyphen it would show.
                None => {
                    while end > at + 1 && ends_in_hyphen(end) {
                        end -= 1;
                    }
                }
            }
        }
        out.push(range(end));
        at = end;
    }
    out
}

/// Promote the streams of the Type 3 font templates [`draw`] returns to indirect objects. This is
/// limited to those fonts, by the names [`draw`] gives them: never a recursive PDF walk, and
/// never another writer's fonts (Thai's keep the form their own writer stores them in).
pub fn materialize_fonts(doc: &mut Document, fonts: &Dict) -> Dict {
    fonts
        .iter()
        .map(|(name, value)| {
            let mut value = value.clone();
            if let Object::Dict(ref mut font) = value
                && name.starts_with(SHAPED_FONT.as_bytes())
                && font.name(b"Subtype") == Some(b"Type3")
            {
                if let Some(Object::Dict(procs)) = font.get(b"CharProcs") {
                    let procs: Dict = procs
                        .iter()
                        .map(|(name, value)| {
                            let value = match value {
                                Object::Stream(stream) => Object::Ref(doc.add(Object::Stream(stream.clone()))),
                                value => value.clone(),
                            };
                            (name.clone(), value)
                        })
                        .collect();
                    font.set(b"CharProcs".to_vec(), Object::Dict(procs));
                }
                if let Some(Object::Stream(cmap)) = font.get(b"ToUnicode") {
                    let cmap = doc.add(Object::Stream(cmap.clone()));
                    font.set(b"ToUnicode".to_vec(), Object::Ref(cmap));
                }
                if let Some(Object::Dict(descriptor)) = font.get(b"FontDescriptor") {
                    let descriptor = doc.add(Object::Dict(descriptor.clone()));
                    font.set(b"FontDescriptor".to_vec(), Object::Ref(descriptor));
                }
            }
            (name.clone(), value)
        })
        .collect()
}

fn n(value: f64) -> String {
    let value = if value.abs() < 0.0000005 { 0.0 } else { value };
    format!("{value:.6}").trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Content plus a Font dictionary template. No document IDs are stored, so a cached appearance
/// can safely be installed into separate display copies of the same document.
pub fn draw(lines: &[Line], size: f64, rect: [f64; 4], align: i64, color: [f64; 3]) -> Result<(Vec<u8>, Dict), GlyphError> {
    draw_aligned(lines, size, rect, align, color, false)
}

/// [`draw`] with direction-aware default quadding: at `align = 0`, each right-to-left paragraph
/// starts at the right edge. Explicit centered and right-aligned quadding still apply to every
/// paragraph. Callers requiring explicit left alignment use [`draw`] instead.
pub fn draw_directional(lines: &[Line], size: f64, rect: [f64; 4], align: i64, color: [f64; 3]) -> Result<(Vec<u8>, Dict), GlyphError> {
    draw_aligned(lines, size, rect, align, color, true)
}

fn draw_aligned(
    lines: &[Line],
    size: f64,
    rect: [f64; 4],
    align: i64,
    color: [f64; 3],
    direction_aware: bool,
) -> Result<(Vec<u8>, Dict), GlyphError> {
    if rect.iter().any(|v| !v.is_finite()) {
        return Err(GlyphError::TooComplex);
    }
    let mut positions = Vec::new();
    let mut top = rect[3];
    for line in lines {
        let y = top - size * line.ascent;
        if y < rect[1] - size {
            break;
        }
        let x = match align {
            1 => rect[0] + (rect[2] - rect[0] - line.width) / 2.0,
            2 => rect[2] - line.width,
            0 if direction_aware && line.rtl => rect[2] - line.width,
            _ => rect[0],
        };
        positions.push([x, y]);
        top = y - size * line.descent;
    }
    draw_positioned(lines.get(..positions.len()).unwrap_or_default(), size, &positions, color)
}

/// Draw already measured lines at explicit baseline positions.
fn draw_positioned(lines: &[Line], size: f64, positions: &[[f64; 2]], color: [f64; 3]) -> Result<(Vec<u8>, Dict), GlyphError> {
    if !size.is_finite()
        || size <= 0.0
        || positions.iter().flatten().any(|v| !v.is_finite())
        || positions.len() != lines.len()
        || color.iter().any(|v| !v.is_finite())
        || lines.iter().any(|l| !l.width.is_finite())
        || lines.iter().fold(0usize, |n, line| n.saturating_add(line.text.len())) > 262_144
    {
        return Err(GlyphError::TooComplex);
    }
    // Each face gets its own fonts, so a font's descriptor names the face it draws.
    let mut groups: Vec<(ShapedScript, Vec<&ShapedCluster>)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for c in lines.iter().flat_map(|l| &l.pieces).flat_map(|p| match p {
        Piece::Shaped(cs) => cs.as_slice(),
        Piece::Latin(_) => &[],
    }) {
        if seen.insert(key(c)) {
            match groups.iter_mut().find(|(script, _)| *script == c.script) {
                Some((_, group)) => group.push(c),
                None => groups.push((c.script, vec![c])),
            }
        }
    }
    let clusters: Vec<&ShapedCluster> = groups.iter().flat_map(|(_, group)| group.iter().copied()).collect();
    if clusters.len() > 65_534 {
        return Err(GlyphError::TooComplex);
    }
    // Every cluster's font number and code in it.
    let mut indices = HashMap::new();
    let mut chunks = Vec::new();
    for (_, group) in &groups {
        for chunk in group.chunks(240) {
            for (at, c) in chunk.iter().enumerate() {
                indices.insert(key(c), (chunks.len(), u8::try_from(at + 1).unwrap_or(1)));
            }
            chunks.push(chunk);
        }
    }
    let mut outlines = HashMap::new();
    for c in &clusters {
        for (id, _) in &c.glyphs {
            if let std::collections::hash_map::Entry::Vacant(entry) = outlines.entry((c.script, *id)) {
                entry.insert(shaped_glyph(c.script, *id)?);
            }
        }
    }
    let mut fonts = Dict::new();
    for line in lines {
        let name = latin_resource(line.latin);
        if !fonts.contains(name.as_bytes()) {
            let mut font = Dict::new();
            font.set(b"Type".to_vec(), Object::name("Font"));
            font.set(b"Subtype".to_vec(), Object::name("Type1"));
            font.set(b"BaseFont".to_vec(), Object::name(line.latin.base_font()));
            font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
            fonts.set(name.into_bytes(), Object::Dict(font));
        }
    }
    for (i, chunk) in chunks.iter().enumerate() {
        fonts.set(format!("{SHAPED_FONT}{i}").into_bytes(), Object::Dict(type3(chunk, &outlines)));
    }
    let mut out = format!("BT\n{} {} {} rg\n", n(color[0]), n(color[1]), n(color[2])).into_bytes();
    for (line, [x, y]) in lines.iter().zip(positions) {
        let mut pen = *x;
        let logical: String = line.text.encode_utf16().map(|u| format!("{u:04X}")).collect();
        out.extend(format!("/Span <</ActualText <FEFF{logical}>>> BDC\n").bytes());
        for piece in &line.pieces {
            match piece {
                Piece::Latin(s) => {
                    out.extend(format!("/{} {} Tf 1 0 0 1 {} {} Tm ", latin_resource(line.latin), n(size), n(pen), n(*y)).bytes());
                    out.extend(literal(&win_ansi(s)));
                    out.extend_from_slice(b" Tj\n");
                    pen += line.latin.text_width(s, size);
                }
                Piece::Shaped(cs) => {
                    for c in cs {
                        let Some((font, code)) = indices.get(&key(c)).copied() else { continue };
                        out.extend(format!("/{SHAPED_FONT}{font} {} Tf 1 0 0 1 {} {} Tm ", n(size), n(pen), n(*y)).bytes());
                        out.extend(literal(&[code]));
                        out.extend_from_slice(b" Tj\n");
                        pen += c.advance * size;
                    }
                }
            }
        }
        out.extend_from_slice(b"EMC\n");
    }
    out.extend_from_slice(b"ET\n");
    Ok((out, fonts))
}

/// The resource name of a standard face in the fonts [`draw`] returns.
fn latin_resource(face: Std14) -> String {
    if face == Std14::Helvetica { "Helv".into() } else { format!("PCStd{}", face as usize) }
}

type Key = (ShapedScript, String, Vec<(u32, u64, u64)>, u64);
fn key(c: &ShapedCluster) -> Key {
    (c.script, c.text.clone(), c.glyphs.iter().map(|(g, p)| (*g, p[0].to_bits(), p[1].to_bits())).collect(), c.advance.to_bits())
}

fn type3(clusters: &[&ShapedCluster], outlines: &HashMap<(ShapedScript, u32), GlyphOutline>) -> Dict {
    let mut procs = Dict::new();
    let mut differences = vec![Object::Int(1)];
    let mut widths = Vec::new();
    let mut bounds = [0.0f64; 4];
    let mut unicode = Vec::new();
    for (i, c) in clusters.iter().enumerate() {
        let code = i + 1;
        let mut paths = String::new();
        let mut ink = [0.0f64; 4];
        for (id, [dx, dy]) in &c.glyphs {
            let Some(glyph) = outlines.get(&(c.script, *id)) else { continue };
            for contour in &glyph.contours {
                for (at, p) in contour.iter().enumerate() {
                    paths.push_str(&format!("{} {} {}\n", n((p[0] + dx) * 1000.0), n((p[1] + dy) * 1000.0), if at == 0 { "m" } else { "l" }));
                }
                paths.push_str("h\n");
            }
            ink = [ink[0].min(glyph.bbox[0] + dx), ink[1].min(glyph.bbox[1] + dy), ink[2].max(glyph.bbox[2] + dx), ink[3].max(glyph.bbox[3] + dy)];
        }
        let ink = [(ink[0] * 1000.0).floor(), (ink[1] * 1000.0).floor(), (ink[2] * 1000.0).ceil(), (ink[3] * 1000.0).ceil()];
        bounds = [bounds[0].min(ink[0]), bounds[1].min(ink[1]), bounds[2].max(ink[2]), bounds[3].max(ink[3])];
        let mut program = format!("{} 0 {} {} {} {} d1\n", n(c.advance * 1000.0), n(ink[0]), n(ink[1]), n(ink[2]), n(ink[3]));
        if !paths.is_empty() {
            program.push_str(&paths);
            program.push_str("f\n");
        }
        let name = format!("g{code:02X}");
        procs.set(name.clone().into_bytes(), Object::Stream(Stream::flate(Dict::new(), program.as_bytes())));
        differences.push(Object::name(&name));
        widths.push(Object::Real(c.advance * 1000.0));
        if !c.text.is_empty() {
            let hex: String = c.text.encode_utf16().map(|u| format!("{u:04X}")).collect();
            unicode.push(format!("<{code:02X}> <{hex}>\n"));
        }
    }
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapName /PCShapedUnicode def\n/CMapType 2 def\n1 begincodespacerange\n<01> <FF>\nendcodespacerange\n",
    );
    for chunk in unicode.chunks(100) {
        cmap.push_str(&format!("{} beginbfchar\n{}endbfchar\n", chunk.len(), chunk.concat()));
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let mut encoding = Dict::new();
    encoding.set(b"Type".to_vec(), Object::name("Encoding"));
    encoding.set(b"Differences".to_vec(), Object::Array(differences));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type3"));
    let script = clusters.first().map(|c| c.script).filter(|s| clusters.iter().all(|c| c.script == *s));
    let family = script.and_then(ShapedScript::font).map_or("PdfCraft Shaped Text", |f| f.family);
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name(&family.replace(' ', "")));
    descriptor.set(b"FontFamily".to_vec(), PdfString::text(family));
    descriptor.set(b"Flags".to_vec(), Object::Int(4));
    descriptor.set(b"ItalicAngle".to_vec(), Object::Int(0));
    font.set(b"FontDescriptor".to_vec(), Object::Dict(descriptor));
    font.set(b"FontBBox".to_vec(), Object::Array(bounds.into_iter().map(Object::Real).collect()));
    font.set(b"FontMatrix".to_vec(), Object::Array([0.001, 0.0, 0.0, 0.001, 0.0, 0.0].into_iter().map(Object::Real).collect()));
    font.set(b"FirstChar".to_vec(), Object::Int(1));
    font.set(b"LastChar".to_vec(), Object::Int(clusters.len() as i64));
    font.set(b"Widths".to_vec(), Object::Array(widths));
    font.set(b"Encoding".to_vec(), Object::Dict(encoding));
    font.set(b"CharProcs".to_vec(), Object::Dict(procs));
    font.set(b"ToUnicode".to_vec(), Object::Stream(Stream::flate(Dict::new(), cmap.as_bytes())));
    font
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arabic_appearance_layout_preserves_logical_text_controls_and_width() {
        if crate::document_arabic_font().is_none() {
            return;
        }
        for text in ["سَلَام 123 (PDF)", "ب\u{200c}ب", "ب\u{200d}ب", "\u{2067}مرحبا 123\u{2069}", "  العربية  بالعربية  "]
        {
            assert_eq!(first_undrawable(text), None);
            let lines = wrap(text, 12.0, 60.0).unwrap();
            assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<String>(), text);
            assert!(lines.iter().all(|l| l.width <= 60.0 + 1e-5), "{lines:?}");
            let (bytes, fonts) = draw(&lines, 12.0, [0.0, 0.0, 60.0, 400.0], 2, [0.0; 3]).unwrap();
            let content = String::from_utf8(bytes).unwrap();
            assert!(fonts.len() >= 2);
            assert!(!content.contains("(?) Tj"));
            for line in lines {
                let hex: String = line.text.encode_utf16().map(|u| format!("{u:04X}")).collect();
                assert!(content.contains(&format!("/ActualText <FEFF{hex}>")));
            }
        }
    }

    #[test]
    fn wrapped_lines_keep_the_paragraphs_directional_override_context() {
        let lines = wrap("\u{202e}ABC DEF GHI JKL\u{202c}", 12.0, 30.0).unwrap();
        let middle = lines.iter().find(|line| line.text.contains("DEF")).unwrap();
        let drawn: String = middle
            .pieces
            .iter()
            .filter_map(|piece| match piece {
                Piece::Latin(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert!(drawn.contains("FED"), "the override must survive wrapping: {middle:?}");
    }

    #[test]
    fn orphan_join_controls_have_no_ink_or_placeholder() {
        let text = "\u{200c}\u{200d}";
        assert!(!has_shaped_text(text), "controls alone keep the writer their text had");
        let lines = wrap(text, 12.0, 100.0).unwrap();
        assert_eq!(lines[0].width, 0.0);
        let (content, _) = draw(&lines, 12.0, [0.0, 0.0, 100.0, 50.0], 0, [0.0; 3]).unwrap();
        assert!(!String::from_utf8(content).unwrap().contains(" Tj"));
    }

    #[test]
    fn copied_default_ignorable_controls_keep_logical_text_without_ink_or_missing_glyphs() {
        if crate::document_arabic_font().is_none() {
            return;
        }
        for control in ['\u{200b}', '\u{2060}', '\u{034f}', '\u{fe0f}', '\u{00ad}', '\u{feff}'] {
            for text in [format!("ب{control}ب"), format!("A{control}B"), control.to_string()] {
                assert_eq!(first_undrawable(&text), None, "{text:?}");
                let lines = wrap(&text, 12.0, 100.0).unwrap();
                let plain = text.replace(control, "");
                assert!((lines[0].width - wrap(&plain, 12.0, 100.0).unwrap()[0].width).abs() < 1e-6, "{text:?}");
                assert_eq!(lines[0].text, text);
                let (content, _) = draw(&lines, 12.0, [0.0, 0.0, 100.0, 100.0], 0, [0.0; 3]).unwrap();
                let hex: String = text.encode_utf16().map(|u| format!("{u:04X}")).collect();
                assert!(String::from_utf8(content).unwrap().contains(&format!("/ActualText <FEFF{hex}>")));
            }
        }
        assert_eq!(first_undrawable("نص 😀"), Some('😀'), "a visible missing character must remain marked");
    }

    #[test]
    fn zero_width_breaks_and_discretionary_hyphens_preserve_logical_text_and_fit() {
        let width = Std14::Helvetica.text_width("one-", 12.0);
        for (logical, drawn) in [("one\u{200b}two", "one"), ("one\u{00ad}two", "one-")] {
            let lines = wrap(logical, 12.0, width).unwrap();
            assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<String>(), logical);
            assert_eq!(lines[0].text, logical.chars().take(4).collect::<String>());
            let latin: String = lines[0].pieces.iter().filter_map(|p| if let Piece::Latin(s) = p { Some(s.as_str()) } else { None }).collect();
            assert_eq!(latin, drawn);
            assert!(lines.iter().all(|line| line.width <= width + 1e-6));
        }
        let unbroken = wrap("one\u{00ad}two", 12.0, 1000.0).unwrap();
        assert_eq!(unbroken[0].width, Std14::Helvetica.text_width("onetwo", 12.0));
    }

    #[test]
    fn soft_wrap_separators_hang_without_inserting_empty_visual_rows() {
        let width = Std14::Helvetica.text_width("ABC", 12.0);
        for logical in ["ABC DEF", "ABC   DEF", "ABC DEF GHI", "ABC ", "ABC   "] {
            let lines = wrap(logical, 12.0, width).unwrap();
            assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<String>(), logical);
            assert!(lines.iter().all(|line| !line.text.trim().is_empty()), "separator became a blank soft row: {lines:?}");
            assert!(lines.iter().all(|line| line.width <= width + 1e-6));
        }
        assert_eq!(display_line("  ABC  ", false), "  ABC  ", "paragraph-final spaces keep their advances");
        assert_eq!(display_line("   ", true), "   ", "an entirely whitespace row is intentional");
        assert_eq!(display_line("  ABC  ", true), "  ABC", "soft hanging spaces must preserve indentation");
        for separator in ['\u{00a0}', '\u{2007}', '\u{202f}', '\n', '\u{2028}'] {
            let text = format!("ABC{separator}");
            assert!(!has_hanging_space(&text));
            assert_eq!(display_line(&text, true), text);
        }
        let wide = wrap("ABC   ", 12.0, 1000.0).unwrap();
        assert_eq!(wide[0].width, Std14::Helvetica.text_width("ABC   ", 12.0));
        let whitespace = wrap("    \n  ABC  ", 12.0, 1000.0).unwrap();
        assert_eq!(whitespace.iter().map(|line| line.text.as_str()).collect::<Vec<_>>(), ["    ", "  ABC  "]);
    }

    #[test]
    fn unicode_line_and_paragraph_separators_start_a_new_line() {
        for separator in ['\u{2028}', '\u{2029}'] {
            let lines = wrap(&format!("ABC{separator}DEF"), 12.0, 1000.0).unwrap();
            assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["ABC", "DEF"], "U+{:04X}", separator as u32);
            let (content, _) = draw(&lines, 12.0, [0.0, 0.0, 1000.0, 100.0], 0, [0.0; 3]).unwrap();
            assert!(!String::from_utf8(content).unwrap().contains('?'));
        }
    }

    #[test]
    fn a_word_that_fits_is_not_broken_at_its_discretionary_hyphen() {
        // "iii-" is wider than "iiii": probing the break at the hyphen must not end the search.
        let width = Std14::Helvetica.text_width("iiii", 12.0);
        assert!(Std14::Helvetica.text_width("iii-", 12.0) > width);
        let lines = wrap("iii\u{00ad}i", 12.0, width).unwrap();
        assert_eq!(lines.len(), 1, "{lines:?}");
        // Where the word does not fit, the break at the hyphen shows it and still fits.
        let narrow = Std14::Helvetica.text_width("ii-", 12.0);
        let lines = wrap("ii\u{00ad}iiii", 12.0, narrow).unwrap();
        assert_eq!(lines[0].text, "ii\u{00ad}", "{lines:?}");
        assert!(lines.iter().all(|line| line.width <= narrow + 1e-6), "{lines:?}");
    }

    #[test]
    fn indentation_stays_with_the_first_text_that_fits() {
        let width = Std14::Helvetica.text_width("  AB", 12.0);
        let lines = wrap("  ABCDEF", 12.0, width).unwrap();
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<String>(), "  ABCDEF");
        assert_eq!(lines[0].text, "  AB", "no blank first row: {lines:?}");
        // A paragraph of spaces alone is still one intended row.
        let spaces = wrap("    ", 12.0, 1000.0).unwrap();
        assert_eq!(spaces.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["    "]);
    }

    #[test]
    fn a_line_separator_breaks_the_line_inside_its_paragraph() {
        if crate::document_hebrew_font(false).is_none() {
            return;
        }
        // U+2028 is whitespace to UAX #9, not a paragraph boundary: the digits after it stay in
        // the right-to-left paragraph. After U+2029 they start a paragraph of their own.
        let same = wrap("\u{5e9}\u{5dc}\u{5d5}\u{5dd}\u{2028}123", 12.0, 1000.0).unwrap();
        assert_eq!(same.iter().map(|l| (l.text.as_str(), l.rtl)).collect::<Vec<_>>(), [("\u{5e9}\u{5dc}\u{5d5}\u{5dd}", true), ("123", true)]);
        let apart = wrap("\u{5e9}\u{5dc}\u{5d5}\u{5dd}\u{2029}123", 12.0, 1000.0).unwrap();
        assert_eq!(apart.iter().map(|l| (l.text.as_str(), l.rtl)).collect::<Vec<_>>(), [("\u{5e9}\u{5dc}\u{5d5}\u{5dd}", true), ("123", false)]);
        // A line that ends at the separator was not wrapped: no discretionary hyphen is drawn.
        let unwrapped = wrap("ab\u{00ad}\u{2028}cd", 12.0, 1000.0).unwrap();
        assert_eq!(unwrapped[0].text, "ab\u{00ad}");
        assert_eq!(unwrapped[0].width, Std14::Helvetica.text_width("ab", 12.0));
        // Leading, trailing and doubled separators each give their own (empty) row.
        let rows = wrap("\u{2028}A\u{2028}\u{2028}B\u{2028}", 12.0, 1000.0).unwrap();
        assert_eq!(rows.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["", "A", "", "B", ""]);
    }

    #[test]
    fn invisible_characters_before_the_text_do_not_make_a_row_of_their_own() {
        let width = Std14::Helvetica.text_width("AB", 12.0);
        for invisible in ["\u{200b}", "\u{00ad}", "\u{200b}\u{200b} ", " \u{200b}"] {
            let text = format!("{invisible}ABCDEF");
            let lines = wrap(&text, 12.0, width).unwrap();
            assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<String>(), text);
            // (A space before the text has a width, so the first row may hold less of the text.)
            assert!(lines[0].text.contains('A'), "{invisible:?}: no row without ink above the text: {lines:?}");
        }
        // A break inside the text is still taken, and the zero-width space kept.
        let lines = wrap("one\u{200b}two", 12.0, Std14::Helvetica.text_width("one", 12.0)).unwrap();
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["one\u{200b}", "two"]);
    }

    #[test]
    fn the_last_discretionary_hyphen_that_fits_is_taken_however_many_after_it_do_not() {
        // "ii-" fits the width of "iiii"; every later hyphen would show "iiii-", which does not.
        // Word joiners keep the later hyphens from being one run.
        let width = Std14::Helvetica.text_width("iiii", 12.0);
        for later in [1usize, 8, 9, 40] {
            let text = format!("ii\u{00ad}ii{}X", "\u{2060}\u{00ad}".repeat(later));
            let lines = wrap(&text, 12.0, width).unwrap();
            assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<String>(), text);
            assert_eq!(lines[0].text, "ii\u{00ad}", "{later} later hyphens: {:?}", lines.iter().map(|l| &l.text).collect::<Vec<_>>());
            assert!(lines.iter().all(|line| line.width <= width + 1e-6), "{later} later hyphens");
        }
        // A space after the last hyphen that fits is the later place, and is taken.
        let lines = wrap("i\u{00ad}i ii\u{00ad}X", 12.0, width).unwrap();
        assert_eq!(lines[0].text, "i\u{00ad}i ");
        // A run of hyphens none of which fits, before a letter too wide for the row: the run
        // goes with that letter, not one hyphen to a row.
        let width = Std14::Helvetica.text_width("iii", 12.0);
        let text = format!("iii{}W", "\u{00ad}".repeat(1000));
        let lines = wrap(&text, 12.0, width).unwrap();
        assert_eq!(lines.iter().map(|l| l.text.chars().count()).collect::<Vec<_>>(), [3, 1001], "two rows");
        assert_eq!(lines[1].width, Std14::Helvetica.text_width("W", 12.0), "no hyphen is drawn before the letter");
        // No hyphen fits and there is no other place: the word is cut, and no hyphen is drawn.
        let narrow = Std14::Helvetica.text_width("ii", 12.0);
        let lines = wrap("ii\u{00ad}\u{00ad}ii", 12.0, narrow).unwrap();
        assert!(lines.iter().all(|line| line.width <= narrow + 1e-6), "{lines:?}");
    }

    #[test]
    fn separator_controls_inside_a_paragraph_are_kept_and_laid_out() {
        if crate::document_hebrew_font(false).is_none() {
            return;
        }
        // The information separators, VT and FF are not places this writer starts a line at:
        // they stay in the line's text (and are drawn and reported as any character the
        // standard font lacks is).
        for control in ['\u{001c}', '\u{001d}', '\u{001e}', '\u{001f}', '\u{000b}', '\u{000c}'] {
            let text = format!("\u{5d0}{control}\u{5d1} 123{control}abc");
            let lines = wrap(&text, 12.0, 1000.0).unwrap();
            assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<String>(), text, "U+{:04X}", control as u32);
            assert!(lines.iter().all(|l| l.width.is_finite()), "U+{:04X}", control as u32);
            let narrow = wrap(&text, 12.0, 14.0).unwrap();
            assert_eq!(narrow.iter().map(|l| l.text.as_str()).collect::<String>(), text, "U+{:04X}", control as u32);
            draw(&lines, 12.0, [0.0, 0.0, 1000.0, 100.0], 0, [0.0; 3]).unwrap();
        }
    }

    #[test]
    fn a_next_line_control_ends_the_paragraph_and_is_not_drawn() {
        if crate::document_hebrew_font(false).is_none() {
            return;
        }
        // U+0085 (NEL) is a new line function, a paragraph separator to UAX #9. WinAnsi has no
        // code for it, so drawn it would be a `?`.
        let text = "\u{5d0}\u{5d1}\u{0085}123\u{0085}abc";
        assert_eq!(first_undrawable(text), None);
        // Nor is a line or a paragraph separator a character the box fails to draw.
        assert_eq!(first_undrawable("\u{5d0}\u{2028}\u{5d1}\u{2029}\u{5d2}"), None);
        let lines = wrap(text, 12.0, 1000.0).unwrap();
        assert_eq!(lines.iter().map(|l| (l.text.as_str(), l.rtl)).collect::<Vec<_>>(), [("\u{5d0}\u{5d1}", true), ("123", false), ("abc", false)]);
        let (content, _) = draw(&lines, 12.0, [0.0, 0.0, 1000.0, 100.0], 0, [0.0; 3]).unwrap();
        assert!(!String::from_utf8(content).unwrap().contains("(?)"));
    }

    #[test]
    fn an_arabic_letter_mark_alone_does_not_take_this_writer() {
        // U+061C is in the Arabic block but is a direction mark, not a letter.
        assert!(!has_shaped_text("\u{061c}12345"));
        assert!(!has_shaped_text("\u{061c}\u{200f}\u{200e}abc"));
        assert!(has_shaped_text("\u{061c}\u{0628}"));
        assert!(has_shaped_text("\u{05d0}"));
    }

    #[test]
    fn rejected_discretionary_hyphens_bound_the_measured_work() {
        // Break opportunities whose hyphen does not fit: a run of them is one candidate, and a
        // word of them is given up on after a bounded number, then cut between graphemes.
        let measured = |text: &str, width: f64| {
            let work = std::cell::Cell::new(0usize);
            let lines = wrap_ranges(text, width, |range| {
                work.set(work.get() + range.len());
                let shown = &text[range.clone()];
                let hyphen = if range.end < text.len() && shown.ends_with('\u{00ad}') { 1.5 } else { 0.0 };
                // A `W` is twice as wide as an `i`, as in Helvetica.
                shown
                    .chars()
                    .map(|c| {
                        if c == '\u{00ad}' {
                            0.0
                        } else if c == 'W' {
                            2.0
                        } else {
                            1.0
                        }
                    })
                    .sum::<f64>()
                    + hyphen
            });
            assert_eq!(lines.iter().map(|range| &text[range.clone()]).collect::<String>(), text);
            (work.get(), lines.len())
        };
        let run = format!("iii{}W", "\u{00ad}".repeat(100_000));
        let (work, _) = measured(&run, 4.0);
        assert!(work < run.len() * 40, "measured {work} bytes for {} input bytes", run.len());
        // Hyphens that word joiners keep apart are not one run: still a bounded search.
        let joined = format!("iii{}W", "\u{2060}\u{00ad}".repeat(40_000));
        let (work, _) = measured(&joined, 4.0);
        assert!(work < joined.len() * 60, "measured {work} bytes for {} input bytes", joined.len());
        let alternating = "i\u{00ad}".repeat(50_000);
        let (work, rows) = measured(&alternating, 4.0);
        assert!(work < alternating.len() * 200, "measured {work} bytes for {} input bytes", alternating.len());
        assert!(rows >= 50_000 / 4, "{rows} rows");
        // One visible hyphen that fits is still taken.
        let (_, rows) = measured("ii\u{00ad}iiii", 4.0);
        assert_eq!(rows, 2);
    }

    #[test]
    fn wrapping_a_large_word_bounds_total_measured_prefix_work() {
        let text = "a".repeat(262_144);
        let work = std::cell::Cell::new(0usize);
        let lines = wrap_ranges(&text, 50.0, |range| {
            work.set(work.get() + range.len());
            range.len() as f64
        });
        assert_eq!(lines.iter().map(|range| &text[range.clone()]).collect::<String>(), text);
        assert!(work.get() < text.len() * 20, "measured {} bytes for {} input bytes", work.get(), text.len());
    }

    #[test]
    fn vocalized_arabic_ink_stays_below_the_text_box_top_at_large_sizes() {
        if crate::document_arabic_font().is_none() {
            return;
        }
        let phrase = "بِسْمِ اللَّهِ الرَّحْمَٰنِ الرَّحِيمِ";
        let lines = wrap(phrase, 40.0, 1000.0).unwrap();
        let ink_top = lines[0]
            .pieces
            .iter()
            .flat_map(|piece| match piece {
                Piece::Shaped(cs) => cs.as_slice(),
                Piece::Latin(_) => &[],
            })
            .flat_map(|cluster| {
                cluster.glyphs.iter().map(move |(glyph, offset)| crate::shaped_glyph(cluster.script, *glyph).unwrap().bbox[3] + offset[1])
            })
            .fold(0.0, f64::max);
        let (content, _) = draw(&lines, 40.0, [0.0, 0.0, 1000.0, 100.0], 2, [0.0; 3]).unwrap();
        let content = String::from_utf8(content).unwrap();
        let operation = content.lines().find(|line| line.contains(" Tm ")).unwrap();
        let tokens = operation.split_whitespace().collect::<Vec<_>>();
        let tm = tokens.iter().position(|token| *token == "Tm").unwrap();
        let baseline: f64 = tokens[tm - 1].parse().unwrap();
        assert!(baseline + ink_top * 40.0 <= 100.0, "mark clipped above box: baseline={baseline}, ascent={ink_top}");
    }
}
