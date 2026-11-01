use super::types::{AddedImage, AddedText};
use super::{
    Content, Document, EditError, GlyphError, Level, ObjRef, Object, ParagraphBidiInfo, ShapedCluster, ShapedScript, shape_arabic, shape_hebrew,
    shape_thai,
};

pub(super) fn nums(doc: &Document, o: Option<&Object>) -> Vec<f64> {
    o.map(|o| doc.resolve(o)).and_then(|a| a.as_array().map(|a| a.iter().filter_map(Object::as_f64).collect())).unwrap_or_default()
}

pub(super) fn norm(r: [f64; 4]) -> [f64; 4] {
    [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]
}

/// The advance of `s` in the item's standard font: its glyphs' widths and the character spacing
/// after each of them, as `Tc` moves the pen (ISO 32000-2 §9.3.2).
pub(super) fn latin_width(t: &AddedText, s: &str) -> f64 {
    t.family.width(s, t.size, t.bold, t.italic) + t.char_spacing * s.chars().count() as f64
}

/// The lines of a text item after wrapping to its box width.
pub fn lines(t: &AddedText) -> Vec<String> {
    wrapped(t).into_iter().map(|(line, ..)| line).collect()
}

#[cfg(test)]
thread_local! {
    /// How often [`wrapped`] laid a text out on this thread, for the test that keeps it out of per-line loops.
    pub(crate) static WRAP_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// [`lines`], each with whether its paragraph runs right to left (its first strong character
/// does) and which paragraph it is in.
pub(super) fn wrapped(t: &AddedText) -> Vec<(String, bool, usize)> {
    #[cfg(test)]
    WRAP_CALLS.with(|c| c.set(c.get().saturating_add(1)));
    let width = (t.rect[2] - t.rect[0]).max(t.size);
    let mut out = Vec::new();
    for (index, para) in t.text.split('\n').enumerate() {
        // Arabic or Hebrew: laid out with the bidirectional algorithm and shaped.
        let rtl_script = para.chars().any(|c| is_arabic(c) || is_hebrew(c));
        let thai = para.chars().any(is_thai);
        if thai
            && !rtl_script
            && let Ok(lines) = thai_lines(t, para, width)
        {
            out.extend(lines.into_iter().map(|line| (line, false, index)));
            continue;
        }
        let rtl = rtl_script && unicode_bidi::get_base_direction(para) == unicode_bidi::Direction::Rtl;
        // Arabic words are shaped one by one (joining stops at spaces), so a line's width is the
        // sum of its words' and shaping stays linear in the paragraph's length. The space is the
        // paragraph's; a space between Latin words is drawn in the item's font, a few hundredths
        // of an em apart.
        let measure = |s: &str| if rtl_script || thai { shaped_width(t, s, rtl) } else { latin_width(t, s) };
        let space = measure(" ");
        let (mut line, mut line_w) = (String::new(), 0.0);
        for word in para.split(' ') {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            let candidate_w =
                if rtl_script || thai { if line.is_empty() { measure(word) } else { line_w + space + measure(word) } } else { measure(&candidate) };
            if !line.is_empty() && candidate_w > width {
                out.push((std::mem::take(&mut line), rtl, index));
                line = word.to_string();
                line_w = measure(word);
            } else {
                line = candidate;
                line_w = candidate_w;
            }
        }
        out.push((line, rtl, index));
    }
    out
}

/// Thai wrapping at shaped cluster boundaries never detaches a tone/vowel mark from its base.
/// This is glyph-safe wrapping, not dictionary-based Thai word segmentation.
fn thai_lines(t: &AddedText, para: &str, width: f64) -> Result<Vec<String>, EditError> {
    let (pieces, _) = shaped_layout(t, para, false)?;
    let mut atoms = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Shaped(clusters) => atoms.extend(clusters.into_iter().map(|c| (c.text, c.advance * t.size + t.char_spacing))),
            Piece::Latin(s) => atoms.extend(s.chars().map(|c| {
                let text = c.to_string();
                let w = latin_width(t, &text);
                (text, w)
            })),
        }
    }
    let mut lines = Vec::new();
    let (mut line, mut used) = (String::new(), 0.0);
    for (text, advance) in atoms {
        if !line.is_empty() && used + advance > width {
            lines.push(std::mem::take(&mut line));
            used = 0.0;
        }
        line.push_str(&text);
        used += advance;
    }
    lines.push(line);
    Ok(lines)
}

pub(super) fn is_thai(c: char) -> bool {
    matches!(u32::from(c), 0x0E00..=0x0E7F)
}

/// Arabic letters, marks, digits and punctuation: an item with any of them is shaped with the
/// craft-fonts Arabic face.
pub(super) fn is_arabic(c: char) -> bool {
    matches!(u32::from(c), 0x0600..=0x06FF | 0x0750..=0x077F | 0x0870..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFC)
}

/// Hebrew letters, points, cantillation marks and punctuation (and the presentation forms): an
/// item with any of them is drawn with the craft-fonts Hebrew face.
pub(super) fn is_hebrew(c: char) -> bool {
    matches!(u32::from(c), 0x0591..=0x05F4 | 0xFB1D..=0xFB4F)
}

/// The first character of `t` that would be drawn as `?`: outside WinAnsiEncoding and not drawn
/// with the craft-fonts Arabic (#125) or Hebrew (#128) face, or Arabic or Hebrew that the face can't
/// draw (or that has no face to draw it). Page-content tools refuse such text; the Add-text editor
/// keeps it open.
pub fn first_undrawable(t: &AddedText) -> Option<char> {
    if t.text.chars().any(is_thai) {
        if let Some(c) = t.text.chars().find(|c| is_thai(*c) && !pdfcraft_fonts::thai_has(*c)) {
            return Some(c);
        }
        let rest = AddedText { text: t.text.chars().filter(|c| !is_thai(*c)).collect(), ..t.clone() };
        return first_undrawable(&rest);
    }
    // Arabic or Hebrew that nothing can draw: there's no face, or the face lacks it.
    if let Some(c) =
        t.text.chars().find(|c| (shaped_arabic(*c) && !pdfcraft_fonts::arabic_has(*c)) || (is_hebrew(*c) && !pdfcraft_fonts::hebrew_has(*c)))
    {
        return Some(c);
    }
    if !shaped_by_a_face(t) {
        // Drawn line by line in the item's standard font (`\n` splits lines). Without the face
        // that is also how `draw` redraws an existing item, so it's checked that way.
        return t.text.split('\n').find_map(pdfcraft_fonts::first_non_win_ansi);
    }
    // What `shaped_layout` leaves to the standard font, exactly as `draw_shaped` splits it.
    wrapped(t).into_iter().find_map(|(line, rtl, _)| {
        let (pieces, _) = shaped_layout(t, &line, rtl).ok()?;
        pieces.iter().find_map(|p| match p {
            Piece::Latin(s) => pdfcraft_fonts::first_non_win_ansi(s),
            Piece::Shaped(_) => None,
        })
    })
}

/// Whether `t` has Arabic or Hebrew that its craft-fonts face draws (so `draw` lays it out with
/// [`draw_shaped`]).
fn shaped_by_a_face(t: &AddedText) -> bool {
    (pdfcraft_fonts::document_arabic_font().is_some() && t.text.chars().any(is_arabic))
        || (pdfcraft_fonts::document_hebrew_font(false).is_some() && t.text.chars().any(is_hebrew))
}

/// An Arabic character the face shapes and draws (U+061C, a direction mark, is dropped).
pub(super) fn shaped_arabic(c: char) -> bool {
    is_arabic(c) && !is_bidi_control(c)
}

/// Direction marks, embeddings, overrides and isolates: they steer the order and are not drawn.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
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

/// A piece of a line with shaped text, in drawing order.
pub(super) enum Piece {
    /// Shown in the item's own standard font.
    Latin(String),
    Shaped(Vec<ShapedCluster>),
}

/// A line of a shaped-text item in drawing order (the Unicode bidirectional algorithm, UAX #9), and
/// its width at the item's size. `rtl` is its paragraph's direction.
pub(super) fn shaped_layout(t: &AddedText, line: &str, rtl: bool) -> Result<(Vec<Piece>, f64), EditError> {
    // unicode-bidi indexes the first level of a line, so an empty one would panic.
    if line.is_empty() {
        return Ok((Vec::new(), 0.0));
    }
    let base = if rtl { Level::rtl() } else { Level::ltr() };
    let info = ParagraphBidiInfo::new(line, Some(base));
    let (levels, runs) = info.visual_runs(0..line.len());
    let mut pieces = Vec::new();
    for run in runs {
        let run_rtl = levels.get(run.start).is_some_and(Level::is_rtl);
        // Runs lie on character boundaries; a missing slice is skipped, never a panic.
        let Some(text) = line.get(run) else { continue };
        // Arabic and Hebrew letters take their face; so do spaces and punctuation between them
        // (Hebrew's when they follow Hebrew), which the shaper then mirrors. Latin text (and what
        // the faces lack) keeps the item's font.
        let mut segments: Vec<(Option<ShapedScript>, String)> = Vec::new();
        for c in text.chars().filter(|c| !is_bidi_control(*c)) {
            let after_hebrew = segments.last().is_some_and(|(kind, _)| *kind == Some(ShapedScript::Hebrew));
            let script = if is_thai(c) {
                Some(ShapedScript::Thai)
            } else if is_hebrew(c) || (run_rtl && after_hebrew && !c.is_alphanumeric() && pdfcraft_fonts::hebrew_has(c)) {
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
            pieces.push(if let Some(script) = script {
                let shaped = match script {
                    ShapedScript::Arabic => shape_arabic(&s, run_rtl).map_err(|e| arabic_error(e, &s))?,
                    ShapedScript::Hebrew => shape_hebrew(&s, run_rtl).map_err(|e| hebrew_error(e, &s))?,
                    ShapedScript::Thai => shape_thai(&s).map_err(|e| thai_error(e, &s))?,
                };
                Piece::Shaped(shaped)
            } else if run_rtl {
                Piece::Latin(s.chars().rev().map(mirrored).collect())
            } else {
                Piece::Latin(s)
            });
        }
    }
    let width = pieces
        .iter()
        .map(|p| match p {
            Piece::Latin(s) => latin_width(t, s),
            // Each cluster is shown on its own, so the spacing follows each once.
            Piece::Shaped(clusters) => clusters.iter().map(|c| c.advance * t.size + t.char_spacing).sum::<f64>(),
        })
        .sum();
    Ok((pieces, width))
}

/// The width of `s` in a shaped-text item; the item's font's estimate if it can't be shaped (drawing
/// then reports why).
fn shaped_width(t: &AddedText, s: &str, rtl: bool) -> f64 {
    shaped_layout(t, s, rtl).map(|(_, w)| w).unwrap_or_else(|_| latin_width(t, s))
}

pub(super) fn arabic_error(e: GlyphError, text: &str) -> EditError {
    EditError::Invalid(match e {
        GlyphError::NoFont => "Arabic text needs PdfCraft's Arabic font, which this build doesn't include \
                               (to build it in, set CRAFT_FONTS_DIR to a craft-fonts checkout that has an Arab face)"
            .into(),
        GlyphError::Missing => format!("the Arabic font can't show \"{text}\""),
        GlyphError::TooComplex => format!("the Arabic font's glyphs for \"{text}\" are too complex"),
    })
}

pub(super) fn hebrew_error(e: GlyphError, text: &str) -> EditError {
    EditError::Invalid(match e {
        GlyphError::NoFont => "Hebrew text needs PdfCraft's Hebrew font, which this build doesn't include \
                               (to build it in, set CRAFT_FONTS_DIR to a craft-fonts checkout that has a Hebr face)"
            .into(),
        GlyphError::Missing => format!("the Hebrew font can't show \"{text}\""),
        GlyphError::TooComplex => format!("the Hebrew font's glyphs for \"{text}\" are too complex"),
    })
}

pub(super) fn thai_error(e: GlyphError, text: &str) -> EditError {
    EditError::Invalid(match e {
        GlyphError::NoFont => {
            "Thai text needs a Thai font in this build; rebuild with CRAFT_FONTS_DIR pointing to craft-fonts with a Thai face".into()
        }
        GlyphError::Missing => format!("the Thai font can't show \"{text}\""),
        GlyphError::TooComplex => "the Thai glyphs are too complex; use a shorter text box".into(),
    })
}

/// The box a text item occupies (height from its lines).
pub fn text_rect(t: &AddedText) -> [f64; 4] {
    let r = norm(t.rect);
    let h = lines(t).len().max(1) as f64 * t.size * 1.2;
    [r[0], r[3] - h, r[2], r[3]]
}

/// How far each paste onto a page already holding the copy is offset, down and to the right, so a
/// copy doesn't hide what it was copied from (points).
pub const PASTE_STEP: f64 = 18.0;

/// Where a pasted item of box `rect` goes on a page of `size` (display space): with its top-left
/// corner at `at`, else where it was, `steps` paste steps ([`PASTE_STEP`]) down and to the right.
/// It is kept on the page; one larger than the page keeps its left and top edges on it. A page
/// or point that isn't a number leaves the box where it was.
pub fn paste_rect(rect: [f64; 4], size: [f64; 2], steps: u32, at: Option<[f64; 2]>) -> [f64; 4] {
    let [x0, y0, x1, y1] = norm(rect);
    let (w, h) = (x1 - x0, y1 - y0);
    let (left, top) = match at {
        Some([x, y]) => (x, y),
        None => {
            let d = PASTE_STEP * f64::from(steps);
            (x0 + d, y1 - d)
        }
    };
    if !size.iter().chain(&[left, top, w, h]).all(|v| v.is_finite()) {
        return rect;
    }
    let left = left.min(size[0] - w).max(0.0);
    let top = top.max(h).min(size[1]);
    [left, top - h, left + w, top]
}

/// Most paste steps [`paste_place`] tries before giving up on finding a free place.
const MAX_PASTE_STEPS: u32 = 64;

/// Where Edit ▸ Paste puts `content` on a page of `size` already holding `items` (display space):
/// where it was, or as many paste steps down and to the right as it takes not to land exactly on
/// a copy of it (the original it was copied from, or an earlier paste).
pub fn paste_place<'a>(items: impl IntoIterator<Item = &'a Content>, content: &Content, size: [f64; 2]) -> [f64; 4] {
    let same = copy_key(content);
    let copies: Vec<[f64; 4]> = items.into_iter().filter(|i| copy_key(i) == same).map(Content::rect).collect();
    let taken = |rect: [f64; 4]| copies.iter().any(|c| c.iter().zip(rect).all(|(a, b)| (a - b).abs() < 0.5));
    let mut rect = paste_rect(content.rect(), size, 0, None);
    for steps in 1..=MAX_PASTE_STEPS {
        if !taken(rect) {
            break;
        }
        rect = paste_rect(content.rect(), size, steps, None);
    }
    rect
}

/// What makes two items copies of each other for [`paste_place`]: all but the box, and for an
/// image all but the object holding its picture, which a paste into another document stores anew.
fn copy_key(c: &Content) -> Content {
    match c.with_rect([0.0; 4]) {
        Content::Image(img) => Content::Image(AddedImage { image: ObjRef::new(0, 0), ..img }),
        text => text,
    }
}

/// The widest explicit line before wrapping, measured with the PDF writing fonts.
pub fn text_width(t: &AddedText) -> f64 {
    t.text.split('\n').map(|line| shaped_width(t, line, unicode_bidi::get_base_direction(line) == unicode_bidi::Direction::Rtl)).fold(0.0, f64::max)
}
