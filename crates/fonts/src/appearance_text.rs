//! Self-contained shaped text for generated appearances. Font streams remain inline in the
//! returned resource template; its owner must promote them to indirect objects before saving.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;

use pdfcraft_cos::{Dict, Document, Object, PdfString, Stream};
use unicode_bidi::{Level, ParagraphBidiInfo};
use unicode_segmentation::UnicodeSegmentation;

use crate::{GlyphError, GlyphOutline, ShapedCluster, ShapedScript, Std14, literal, shaped_glyph, win_ansi};

/// Direction marks affect ordering, but have no visible glyph.
pub fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Joining controls must reach the shaper in the same segment as their letters.
pub fn is_join_control(c: char) -> bool {
    matches!(c, '\u{200c}' | '\u{200d}')
}

fn script(c: char) -> Option<ShapedScript> {
    match u32::from(c) {
        0x0600..=0x06ff | 0x0750..=0x077f | 0x0870..=0x08ff | 0xfb50..=0xfdff | 0xfe70..=0xfefc => Some(ShapedScript::Arabic),
        0x0591..=0x05f4 | 0xfb1d..=0xfb4f => Some(ShapedScript::Hebrew),
        0x0e00..=0x0e7f => Some(ShapedScript::Thai),
        _ => None,
    }
}

/// The face that draws `c` in `look`: the Arabic face's bold instance for bold Arabic.
fn script_in(c: char, look: Look) -> Option<ShapedScript> {
    script(c).map(|kind| if kind == ShapedScript::Arabic && look.bold { ShapedScript::ArabicBold } else { kind })
}

fn has(kind: ShapedScript, c: char) -> bool {
    match kind {
        ShapedScript::Arabic | ShapedScript::ArabicBold => crate::arabic_has(c),
        ShapedScript::Hebrew => crate::hebrew_has(c),
        ShapedScript::Thai => crate::thai_has(c),
    }
}

pub fn has_shaped_text(text: &str) -> bool {
    text.chars().any(|c| script(c).is_some() || crate::embedding::default_ignorable(c))
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
        if crate::embedding::default_ignorable(c) || matches!(c, '\n' | '\r' | '\t') {
            false
        } else if let Some(kind) = script(c) {
            !has(kind, c)
        } else {
            !crate::win_ansi_encodable(c)
        }
    })
}

/// The look of one run of a styled paragraph (see [`wrap_styled`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpanStyle {
    /// Point size.
    pub size: f64,
    /// Fill colour.
    pub color: [f64; 3],
    /// Bold: the Arabic face's bold instance, and the bold standard Latin face.
    pub bold: bool,
    /// Italic: the oblique standard Latin face. Arabic script has no italic and stays upright.
    pub italic: bool,
}

/// How a piece is drawn: its Latin face, its size relative to the size the lines were wrapped
/// at, its own fill colour (None keeps the caller's) and whether Arabic uses the bold instance.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Look {
    latin: Std14,
    scale: f64,
    color: Option<[f64; 3]>,
    bold: bool,
}

impl Look {
    fn plain(latin: Std14) -> Self {
        Look { latin, scale: 1.0, color: None, bold: false }
    }
}

/// The looks of one paragraph: `base` everywhere outside `spans`, whose byte ranges are counted
/// from the start of the whole text (sorted, disjoint); the paragraph starts at byte `offset`.
#[derive(Clone, Copy)]
struct Styles<'a> {
    base: Look,
    offset: usize,
    spans: &'a [(Range<usize>, Look)],
}

impl Styles<'_> {
    fn plain(latin: Std14) -> Styles<'static> {
        Styles { base: Look::plain(latin), offset: 0, spans: &[] }
    }

    /// The look at byte `local` of the paragraph.
    fn at(&self, local: usize) -> Look {
        let at = self.offset.saturating_add(local);
        let after = self.spans.partition_point(|(range, _)| range.start <= at);
        after.checked_sub(1).and_then(|i| self.spans.get(i)).filter(|(range, _)| at < range.end).map_or(self.base, |(_, look)| *look)
    }
}

#[derive(Clone, Debug)]
enum Piece {
    Latin(String, Look),
    Shaped(Vec<ShapedCluster>, Look),
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
    latin: Std14,
    pieces: Vec<Piece>,
}

/// Positioned cluster ink in em units. Cached font bounds include every attached mark.
pub fn cluster_ink_bounds(cluster: &ShapedCluster) -> Result<[f64; 4], GlyphError> {
    let mut bounds = [0.0f64; 4];
    for (glyph, [dx, dy]) in &cluster.glyphs {
        let glyph = crate::shaped_bounds(cluster.script, *glyph)?;
        bounds = [bounds[0].min(glyph[0] + dx), bounds[1].min(glyph[1] + dy), bounds[2].max(glyph[2] + dx), bounds[3].max(glyph[3] + dy)];
    }
    Ok(bounds)
}

fn layout(
    text: &str,
    bidi: &ParagraphBidiInfo<'_>,
    range: Range<usize>,
    size: f64,
    rtl: bool,
    width: f64,
    styles: Styles<'_>,
) -> Result<Line, GlyphError> {
    let line = layout_pass(text, bidi, range.clone(), size, rtl, false, styles)?;
    if line.width > width && range.end == text.len() && has_hanging_space(&line.text) {
        layout_pass(text, bidi, range, size, rtl, true, styles)
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
    styles: Styles<'_>,
) -> Result<Line, GlyphError> {
    let continued = range.end < text.len() || hang_final;
    let logical = text.get(range.clone()).ok_or(GlyphError::Missing)?;
    let displayed = display_line(logical, continued);
    let text = displayed.as_ref();
    let mut pieces = Vec::new();
    if !text.is_empty() {
        let line = line_bidi(bidi, range.clone(), text)?;
        let (levels, runs) = line.visual_runs(0..text.len());
        // The last logical byte, for a displayed discretionary hyphen past the logical text.
        let last = range.end.saturating_sub(1).max(range.start);
        for run in runs {
            let run_rtl = levels.get(run.start).is_some_and(Level::is_rtl);
            let Some(input) = text.get(run.clone()) else { continue };
            let looked: Vec<(char, Look)> =
                input.char_indices().map(|(at, c)| (c, styles.at(range.start.saturating_add(run.start).saturating_add(at).min(last)))).collect();
            let mut segments: Vec<(Option<ShapedScript>, Look, String)> = Vec::new();
            let first_script = looked.iter().find_map(|(c, look)| script_in(*c, *look));
            for (c, look) in looked.into_iter().filter(|(c, _)| !is_bidi_control(*c)) {
                let previous = segments.last().and_then(|(kind, _, _)| *kind);
                if crate::embedding::default_ignorable(c) && previous.or(first_script).is_none() {
                    continue;
                }
                let kind = script_in(c, look)
                    .or_else(|| crate::embedding::default_ignorable(c).then_some(previous.or(first_script)).flatten())
                    .or_else(|| (run_rtl && !c.is_alphanumeric()).then(|| previous.or(first_script).filter(|kind| has(*kind, c))).flatten());
                match segments.last_mut() {
                    Some((last, last_look, s)) if *last == kind && *last_look == look => s.push(c),
                    _ => segments.push((kind, look, c.to_string())),
                }
            }
            if run_rtl {
                segments.reverse();
            }
            for (kind, look, s) in segments {
                pieces.push(match kind {
                    Some(ShapedScript::Arabic) => Piece::Shaped(crate::shape_arabic(&s, run_rtl)?, look),
                    Some(ShapedScript::ArabicBold) => Piece::Shaped(crate::shape_arabic_bold(&s, run_rtl)?, look),
                    Some(ShapedScript::Hebrew) => Piece::Shaped(crate::shape_hebrew(&s, run_rtl)?, look),
                    Some(ShapedScript::Thai) => Piece::Shaped(crate::shape_thai(&s)?, look),
                    None if run_rtl => Piece::Latin(s.chars().rev().map(mirrored).collect(), look),
                    None => Piece::Latin(s, look),
                });
            }
        }
    }
    let width = pieces
        .iter()
        .map(|piece| match piece {
            Piece::Latin(s, look) => look.latin.text_width(s, size * look.scale),
            Piece::Shaped(cs, look) => cs.iter().map(|c| c.advance * size * look.scale).sum(),
        })
        .sum();
    // Em units of `size`: a row holding larger runs needs their room.
    let empty = if pieces.is_empty() { styles.at(range.start).scale } else { 0.0 };
    let tallest = pieces
        .iter()
        .map(|piece| match piece {
            Piece::Latin(_, look) | Piece::Shaped(_, look) => look.scale,
        })
        .fold(empty, f64::max);
    let (mut ascent, mut descent) = (0.9 * tallest, 0.3 * tallest);
    for piece in &pieces {
        let Piece::Shaped(clusters, look) = piece else { continue };
        for cluster in clusters {
            let bounds = cluster_ink_bounds(cluster)?;
            ascent = ascent.max((bounds[3] + 0.05) * look.scale);
            descent = descent.max((-bounds[1] + 0.05) * look.scale);
        }
    }
    Ok(Line { text: logical.to_owned(), rtl, width, ascent, descent, latin: styles.base.latin, pieces })
}

/// Text used only for measuring/drawing a wrapped line. A soft hyphen is invisible inside a
/// word; at a selected continuation break it draws a hyphen. Retain the original logical line
/// separately for ActualText and future editing. Soft-wrap separators hang at the line end;
/// indentation, entirely whitespace rows and paragraph-final spaces retain their advances.
pub fn display_line(text: &str, continued: bool) -> Cow<'_, str> {
    match text.strip_suffix('\u{00ad}').filter(|_| continued) {
        Some(prefix) => Cow::Owned(format!("{prefix}-")),
        None if continued && has_hanging_space(text) => Cow::Borrowed(text.trim_end_matches(soft_space)),
        None => Cow::Borrowed(text),
    }
}

fn soft_space(c: char) -> bool {
    c.is_whitespace()
        && !matches!(c, '\u{00a0}' | '\u{2007}' | '\u{202f}' | '\n' | '\r' | '\u{000b}' | '\u{000c}' | '\u{0085}' | '\u{2028}' | '\u{2029}')
}

/// Whether trailing breakable spaces can hang without removing visible content, indentation,
/// nonbreaking spaces or an intentionally entirely whitespace row.
pub fn has_hanging_space(text: &str) -> bool {
    text.chars().next_back().is_some_and(soft_space) && text.chars().any(|c| !c.is_whitespace() && !crate::embedding::default_ignorable(c))
}

/// Preserve paragraph-final space advances when they fit. If only their invisible suffix
/// overflows, hang it on the same logical row rather than creating an extra blank soft row.
pub fn fit_display_line(text: &str, continued: bool, width: f64, measure: impl Fn(&str) -> f64) -> Cow<'_, str> {
    let displayed = display_line(text, continued);
    if !continued && has_hanging_space(text) && measure(&displayed) > width { display_line(text, true) } else { displayed }
}

/// Slice a paragraph's already resolved bidi levels for one display line. Only the line is
/// cloned before UAX #9 L1/L2 processing, bounding work on paragraphs with many short lines.
/// `displayed` is the logical range or its [`display_line`] discretionary-hyphen replacement.
pub fn line_bidi<'a>(paragraph: &ParagraphBidiInfo<'_>, range: Range<usize>, displayed: &'a str) -> Result<ParagraphBidiInfo<'a>, GlyphError> {
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

/// [`wrap`] with a selected standard Latin face. Arabic keeps its approved script face, while
/// Latin pieces use this font's actual metrics for wrapping, alignment and final drawing.
pub fn wrap_with_latin(text: &str, size: f64, width: f64, latin: Std14) -> Result<Vec<Line>, GlyphError> {
    if text.len() > 262_144 || !size.is_finite() || size <= 0.0 || !width.is_finite() || width <= 0.0 {
        return Err(GlyphError::TooComplex);
    }
    if matches!(latin, Std14::Symbol | Std14::ZapfDingbats) {
        return Err(GlyphError::Missing);
    }
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = Vec::new();
    for paragraph in normalized.split('\n') {
        let rtl = unicode_bidi::get_base_direction(paragraph) == unicode_bidi::Direction::Rtl;
        // UAX #9 resolves embeddings and isolates for the paragraph, then reorders each line.
        // Re-running it on a detached line loses an opener that was on a previous line.
        let bidi = ParagraphBidiInfo::new(paragraph, Some(if rtl { Level::rtl() } else { Level::ltr() }));
        let styles = Styles::plain(latin);
        let ranges =
            wrap_ranges(paragraph, width, |range| layout(paragraph, &bidi, range, size, rtl, width, styles).map_or(f64::INFINITY, |l| l.width));
        for range in ranges {
            out.push(layout(paragraph, &bidi, range, size, rtl, width, styles)?);
        }
    }
    Ok(out)
}

/// [`wrap`] for a paragraph whose runs differ in size, colour and weight: `spans` give the look
/// of byte ranges of `text` (sorted, disjoint); text outside them is drawn like `base`. Lines are
/// measured in em units of `size` (the size later passed to [`draw`] or [`draw_positioned`]),
/// each run drawn at its own size and in its own colour. Bold Arabic uses the face's bold
/// instance ([`GlyphError::NoFont`] when it has none); paragraphs split at line feeds only.
pub fn wrap_styled(text: &str, size: f64, width: f64, base: SpanStyle, spans: &[(Range<usize>, SpanStyle)]) -> Result<Vec<Line>, GlyphError> {
    if text.len() > 262_144
        || spans.len() > 65_536
        || !size.is_finite()
        || size <= 0.0
        || !width.is_finite()
        || width <= 0.0
        || std::iter::once(&base)
            .chain(spans.iter().map(|(_, s)| s))
            .any(|s| !s.size.is_finite() || s.size <= 0.0 || s.size / size > 1000.0 || s.color.iter().any(|c| !c.is_finite()))
        || spans.windows(2).any(|pair| pair[0].0.end > pair[1].0.start)
        || spans.iter().any(|(range, _)| range.start > range.end || text.get(range.clone()).is_none())
    {
        return Err(GlyphError::TooComplex);
    }
    let look = |style: &SpanStyle| Look {
        latin: match (style.bold, style.italic) {
            (false, false) => Std14::Helvetica,
            (true, false) => Std14::HelveticaBold,
            (false, true) => Std14::HelveticaOblique,
            (true, true) => Std14::HelveticaBoldOblique,
        },
        scale: style.size / size,
        color: Some(style.color),
        bold: style.bold,
    };
    let looks: Vec<(Range<usize>, Look)> = spans.iter().map(|(range, style)| (range.clone(), look(style))).collect();
    let mut out = Vec::new();
    let mut offset = 0usize;
    for paragraph in text.split('\n') {
        let start = offset;
        offset = offset.saturating_add(paragraph.len()).saturating_add(1);
        // A pasted CRLF ends its paragraph at the LF; the CR has no glyph.
        let paragraph = paragraph.strip_suffix('\r').unwrap_or(paragraph);
        let rtl = unicode_bidi::get_base_direction(paragraph) == unicode_bidi::Direction::Rtl;
        let bidi = ParagraphBidiInfo::new(paragraph, Some(if rtl { Level::rtl() } else { Level::ltr() }));
        let styles = Styles { base: look(&base), offset: start, spans: &looks };
        let ranges =
            wrap_ranges(paragraph, width, |range| layout(paragraph, &bidi, range, size, rtl, width, styles).map_or(f64::INFINITY, |l| l.width));
        for range in ranges {
            out.push(layout(paragraph, &bidi, range, size, rtl, width, styles)?);
        }
    }
    Ok(out)
}

/// Wrap one logical paragraph without dropping whitespace or splitting a grapheme. Callers
/// measure each candidate using its final font/shaping and retain their paragraph direction.
pub fn wrap_logical(paragraph: &str, width: f64, measure: impl Fn(&str) -> f64) -> Vec<String> {
    wrap_ranges(paragraph, width, |range| {
        measure(&fit_display_line(paragraph.get(range.clone()).unwrap_or_default(), range.end < paragraph.len(), width, &measure))
    })
    .into_iter()
    .map(|range| paragraph.get(range).unwrap_or_default().to_owned())
    .collect()
}

/// Logical paragraph ranges after wrapping. Measurement receives original byte ranges so
/// callers can reuse paragraph-level bidi state instead of resolving each detached prefix.
pub fn wrap_ranges(paragraph: &str, width: f64, measure: impl Fn(Range<usize>) -> f64) -> Vec<Range<usize>> {
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
        // Exponential probing then binary search avoids reshaping every growing prefix of an
        // arbitrarily long word. Each emitted line is measured again after its chosen break.
        let mut fit = at;
        let mut probe = at + 1;
        let mut step = 1usize;
        loop {
            let measured = measure(range(probe));
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
            let measured = measure(range(middle));
            if measured.is_finite() && measured <= width {
                fit = middle;
            } else {
                high = middle;
            }
        }
        // A single overwide grapheme cannot be split without separating a mark from its base.
        let mut end = fit.max(at + 1);
        if end < count {
            for boundary in (at + 1..=end).rev() {
                let previous = boundaries.get(boundary.saturating_sub(1)).copied().unwrap_or(start);
                let next = boundaries.get(boundary).copied().unwrap_or(previous);
                if paragraph.get(previous..next).is_some_and(|s| s.chars().all(|c| soft_space(c) || matches!(c, '\u{200b}' | '\u{00ad}'))) {
                    end = boundary;
                    break;
                }
            }
        }
        out.push(range(end));
        at = end;
    }
    out
}

/// Promote the streams of our generated Type 3 font templates to indirect objects. This is
/// deliberately limited to known generated font resources, never a recursive PDF walk.
pub fn materialize_fonts(doc: &mut Document, fonts: &Dict) -> Dict {
    fonts
        .iter()
        .map(|(name, value)| {
            let mut value = value.clone();
            if let Object::Dict(ref mut font) = value
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
            _ => rect[0],
        };
        positions.push([x, y]);
        top = y - size * line.descent;
    }
    draw_positioned(lines.get(..positions.len()).unwrap_or_default(), size, &positions, Some(color))
}

/// Draw already measured lines at explicit baseline positions, retaining the caller's current
/// colour when `color` is None. Forms use this for list rows, comb cells and vertical centering.
pub fn draw_positioned(lines: &[Line], size: f64, positions: &[[f64; 2]], color: Option<[f64; 3]>) -> Result<(Vec<u8>, Dict), GlyphError> {
    if !size.is_finite()
        || size <= 0.0
        || positions.iter().flatten().any(|v| !v.is_finite())
        || positions.len() != lines.len()
        || color.iter().flatten().any(|v| !v.is_finite())
        || lines.iter().any(|l| !l.width.is_finite())
        || lines.iter().fold(0usize, |n, line| n.saturating_add(line.text.len())) > 262_144
    {
        return Err(GlyphError::TooComplex);
    }
    // Each face (the bold Arabic instance included) gets its own fonts, so a font's descriptor
    // names the face and weight it draws: a later edit reads bold Arabic back as bold.
    let mut groups: Vec<(ShapedScript, Vec<&ShapedCluster>)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for c in lines.iter().flat_map(|l| &l.pieces).flat_map(|p| match p {
        Piece::Shaped(cs, _) => cs.as_slice(),
        _ => &[],
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
    let mut latin = Dict::new();
    latin.set(b"Type".to_vec(), Object::name("Font"));
    latin.set(b"Subtype".to_vec(), Object::name("Type1"));
    latin.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
    latin.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    fonts.set(b"Helv".to_vec(), Object::Dict(latin));
    let faces = lines.iter().flat_map(|line| {
        std::iter::once(line.latin).chain(line.pieces.iter().filter_map(|piece| match piece {
            Piece::Latin(_, look) => Some(look.latin),
            Piece::Shaped(..) => None,
        }))
    });
    for face in faces {
        let name = latin_resource(face);
        if !fonts.contains(name.as_bytes()) {
            let mut font = Dict::new();
            font.set(b"Type".to_vec(), Object::name("Font"));
            font.set(b"Subtype".to_vec(), Object::name("Type1"));
            font.set(b"BaseFont".to_vec(), Object::name(face.base_font()));
            font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
            fonts.set(name.into_bytes(), Object::Dict(font));
        }
    }
    for (i, chunk) in chunks.iter().enumerate() {
        fonts.set(format!("PCShaped{i}").into_bytes(), Object::Dict(type3(chunk, &outlines)));
    }
    let mut out = b"BT\n".to_vec();
    let mut current = None;
    // A piece without its own colour keeps the caller's (styled lines give every piece one).
    let mut fill = |out: &mut Vec<u8>, wanted: Option<[f64; 3]>| {
        if let Some(c) = wanted.or(color)
            && current != Some(c)
        {
            out.extend(format!("{} {} {} rg\n", n(c[0]), n(c[1]), n(c[2])).bytes());
            current = Some(c);
        }
    };
    fill(&mut out, None);
    for (line, [x, y]) in lines.iter().zip(positions) {
        let mut pen = *x;
        let logical: String = line.text.encode_utf16().map(|u| format!("{u:04X}")).collect();
        out.extend(format!("/Span <</ActualText <FEFF{logical}>>> BDC\n").bytes());
        for piece in &line.pieces {
            match piece {
                Piece::Latin(s, look) => {
                    fill(&mut out, look.color);
                    let at = size * look.scale;
                    out.extend(format!("/{} {} Tf 1 0 0 1 {} {} Tm ", latin_resource(look.latin), n(at), n(pen), n(*y)).bytes());
                    out.extend(literal(&win_ansi(s)));
                    out.extend_from_slice(b" Tj\n");
                    pen += look.latin.text_width(s, at);
                }
                Piece::Shaped(cs, look) => {
                    fill(&mut out, look.color);
                    let at = size * look.scale;
                    for c in cs {
                        let Some((font, code)) = indices.get(&key(c)).copied() else { continue };
                        out.extend(format!("/PCShaped{font} {} Tf 1 0 0 1 {} {} Tm ", n(at), n(pen), n(*y)).bytes());
                        out.extend(literal(&[code]));
                        out.extend_from_slice(b" Tj\n");
                        pen += c.advance * at;
                    }
                }
            }
        }
        out.extend_from_slice(b"EMC\n");
    }
    out.extend_from_slice(b"ET\n");
    Ok((out, fonts))
}

fn latin_resource(face: Std14) -> String {
    if face == Std14::Helvetica { "Helv".into() } else { format!("PCStd{}", face as usize) }
}

/// Comb fields place one grapheme in each cell. Return logical drawing order and its visual
/// cell indices, so RTL letters, LTR digits and attached marks occupy their correct cells.
pub fn comb(text: &str, size: f64, cells: usize) -> Result<(Vec<Line>, Vec<usize>), GlyphError> {
    comb_with_latin(text, size, cells, Std14::Helvetica)
}

/// [`comb`] preserving the field's selected standard Latin face and advance widths.
pub fn comb_with_latin(text: &str, size: f64, cells: usize, latin: Std14) -> Result<(Vec<Line>, Vec<usize>), GlyphError> {
    if text.len() > 262_144 || cells == 0 {
        return Err(GlyphError::TooComplex);
    }
    if matches!(latin, Std14::Symbol | Std14::ZapfDingbats) {
        return Err(GlyphError::Missing);
    }
    let rtl = paragraph_rtl(text);
    let bidi = ParagraphBidiInfo::new(text, Some(if rtl { Level::rtl() } else { Level::ltr() }));
    let mut lines = Vec::new();
    let mut levels = Vec::new();
    for (at, grapheme) in text.grapheme_indices(true).filter(|(_, g)| !g.chars().all(is_bidi_control)).take(cells) {
        lines.push(layout(text, &bidi, at..at + grapheme.len(), size, rtl, f64::INFINITY, Styles::plain(latin))?);
        levels.push(bidi.levels.get(at).copied().unwrap_or(bidi.paragraph_level));
    }
    let mut slots = vec![0usize; lines.len()];
    for (visual, logical) in unicode_bidi::BidiInfo::reorder_visual(&levels).into_iter().enumerate() {
        if let Some(slot) = slots.get_mut(logical) {
            *slot = visual + if rtl { cells.saturating_sub(lines.len()) } else { 0 };
        }
    }
    Ok((lines, slots))
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
    let bold = script == Some(ShapedScript::ArabicBold);
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    let name = family.replace(' ', "");
    descriptor.set(b"FontName".to_vec(), Object::name(&if bold { format!("{name}-Bold") } else { name }));
    descriptor.set(b"FontFamily".to_vec(), PdfString::text(family));
    if bold {
        descriptor.set(b"FontWeight".to_vec(), Object::Int(700));
    }
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
    fn wrapping_preserves_spaces_and_keeps_marks_with_their_base() {
        let source = "  سَلَام  بالعالم  ";
        let lines = wrap_logical(source, 4.0, |s| s.graphemes(true).count() as f64);
        assert_eq!(lines.concat(), source);
        assert!(lines.iter().enumerate().all(|(i, line)| {
            fit_display_line(line, i + 1 < lines.len(), 4.0, |s| s.graphemes(true).count() as f64).graphemes(true).count() <= 4
        }));
        for line in &lines {
            assert!(!line.starts_with(['\u{064e}', '\u{064f}', '\u{0650}']), "{lines:?}");
        }
        assert_eq!(wrap_logical("", 1.0, |s| s.len() as f64), [""]);
        assert_eq!(wrap_logical("abcdef", 2.0, |s| s.len() as f64), ["ab", "cd", "ef"]);
    }

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
    fn font_coverage_does_not_require_invisible_control_glyphs() {
        let text = "A\u{200c}\u{200d}\u{200e}\u{200f}\u{061c}\u{202b}\u{2067}\u{2069}\u{fe0f}";
        assert!(crate::embedding::covers(crate::INTER_REGULAR, text));
        assert!(!crate::embedding::covers(crate::INTER_REGULAR, "A\u{10ffff}"));
        let shaped = crate::source_shaping::shape(crate::INTER_REGULAR, text).unwrap();
        assert!(shaped.advance.is_finite());
        assert!(shaped.glyphs.iter().all(|g| g.id.to_u32() > 0));
    }

    #[test]
    fn wrapped_lines_keep_the_paragraphs_directional_override_context() {
        let lines = wrap("\u{202e}ABC DEF GHI JKL\u{202c}", 12.0, 30.0).unwrap();
        let middle = lines.iter().find(|line| line.text.contains("DEF")).unwrap();
        let drawn: String = middle
            .pieces
            .iter()
            .filter_map(|piece| match piece {
                Piece::Latin(s, _) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert!(drawn.contains("FED"), "the override must survive wrapping: {middle:?}");
    }

    #[test]
    fn orphan_join_controls_have_no_ink_or_placeholder() {
        let text = "\u{200c}\u{200d}";
        assert!(has_shaped_text(text));
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
            let latin: String = lines[0].pieces.iter().filter_map(|p| if let Piece::Latin(s, _) = p { Some(s.as_str()) } else { None }).collect();
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
            let generic = wrap_logical(logical, width, |s| Std14::Helvetica.text_width(s, 12.0));
            assert_eq!(generic, lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>());
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
    fn comb_cells_keep_marks_together_and_numbers_in_ltr_order() {
        if crate::document_arabic_font().is_none() {
            return;
        }
        let (lines, slots) = comb("بَاب12", 12.0, 7).unwrap();
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["بَ", "ا", "ب", "1", "2"]);
        assert_eq!(slots, [6, 5, 4, 2, 3]);
    }

    #[test]
    fn wrapping_a_large_word_bounds_total_measured_prefix_work() {
        let text = "a".repeat(262_144);
        let work = std::cell::Cell::new(0usize);
        let lines = wrap_logical(&text, 50.0, |s| {
            work.set(work.get() + s.len());
            s.len() as f64
        });
        assert_eq!(lines.concat(), text);
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
                Piece::Shaped(cs, _) => cs.as_slice(),
                _ => &[],
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
