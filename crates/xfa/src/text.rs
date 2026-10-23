//! Text measurement and line breaking with the standard 14 fonts that stand in for the
//! template's typefaces (no font programs are embedded in dynamic XFA forms, and none are
//! bundled here).

use crate::model::{Font, HAlign, Para, Rect, RichText, Run};

/// The standard-14 family a typeface maps to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    Helvetica,
    Courier,
    Times,
}

impl Family {
    pub fn of(typeface: &str) -> Family {
        let t = typeface.to_ascii_lowercase();
        if t.contains("courier") || t.contains("mono") {
            Family::Courier
        } else if t.contains("times")
            || t.contains("georgia")
            || t.contains("garamond")
            || t.contains("minion")
            || t.contains("serif") && !t.contains("sans")
        {
            Family::Times
        } else {
            Family::Helvetica
        }
    }

    /// The PDF base font name for a face.
    pub fn base_font(self, bold: bool, italic: bool) -> &'static str {
        match (self, bold, italic) {
            (Family::Helvetica, false, false) => "Helvetica",
            (Family::Helvetica, true, false) => "Helvetica-Bold",
            (Family::Helvetica, false, true) => "Helvetica-Oblique",
            (Family::Helvetica, true, true) => "Helvetica-BoldOblique",
            (Family::Courier, false, false) => "Courier",
            (Family::Courier, true, false) => "Courier-Bold",
            (Family::Courier, false, true) => "Courier-Oblique",
            (Family::Courier, true, true) => "Courier-BoldOblique",
            (Family::Times, false, false) => "Times-Roman",
            (Family::Times, true, false) => "Times-Bold",
            (Family::Times, false, true) => "Times-Italic",
            (Family::Times, true, true) => "Times-BoldItalic",
        }
    }

    /// The resource name used in content streams and `/DA` strings (Acrobat's conventions).
    pub fn resource_name(self, bold: bool, italic: bool) -> &'static str {
        match (self, bold, italic) {
            (Family::Helvetica, false, false) => "Helv",
            (Family::Helvetica, true, false) => "HeBo",
            (Family::Helvetica, false, true) => "HeOb",
            (Family::Helvetica, true, true) => "HeBO",
            (Family::Courier, false, false) => "Cour",
            (Family::Courier, true, false) => "CoBo",
            (Family::Courier, false, true) => "CoOb",
            (Family::Courier, true, true) => "CoBO",
            (Family::Times, false, false) => "TiRo",
            (Family::Times, true, false) => "TiBo",
            (Family::Times, false, true) => "TiIt",
            (Family::Times, true, true) => "TiBI",
        }
    }
}

/// A resolved face: family, weight, posture, size and colour.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub family: Family,
    pub bold: bool,
    pub italic: bool,
    pub size: f64,
    pub color: [f64; 3],
    pub underline: bool,
    /// Horizontal scale of the glyphs (`fontHorizontalScale`), as a fraction: widths are
    /// measured at it and the text is written with `Tz`.
    pub h_scale: f64,
}

impl Face {
    /// The face a resolved font is drawn in: the standard-14 family its typeface maps to,
    /// with its size and horizontal scale clamped to a drawable range.
    pub fn of(font: &Font) -> Face {
        Face {
            family: Family::of(&font.typeface),
            bold: font.bold,
            italic: font.italic,
            size: font.size.clamp(1.0, 300.0),
            color: font.color.0,
            underline: font.underline,
            h_scale: font.h_scale.clamp(0.01, 10.0),
        }
    }

    /// This face with what a rich-text run sets in place of its own: weight, posture,
    /// underline, size and horizontal scale. The family and color stay.
    pub fn with_run(&self, run: &Run) -> Face {
        Face {
            family: self.family,
            bold: run.bold.unwrap_or(self.bold),
            italic: run.italic.unwrap_or(self.italic),
            size: run.size.unwrap_or(self.size).clamp(1.0, 300.0),
            color: self.color,
            underline: run.underline.unwrap_or(self.underline),
            h_scale: run.h_scale.unwrap_or(self.h_scale).clamp(0.01, 10.0),
        }
    }

    /// This face as designed, without a horizontal scale: field text is drawn by the forms
    /// layer, whose appearances have none, so it is measured that way too.
    pub fn unscaled(self) -> Face {
        Face { h_scale: 1.0, ..self }
    }

    /// The resource name of this face's standard font, as content streams and `/DA` strings
    /// refer to it.
    pub fn resource_name(&self) -> &'static str {
        self.family.resource_name(self.bold, self.italic)
    }

    /// Advance of `s` at this face's size.
    pub fn width(&self, s: &str) -> f64 {
        #[cfg(test)]
        MEASURED.with(|m| m.set(m.get().saturating_add(s.len() as u64)));
        let designed = match self.family {
            Family::Courier => s.chars().count() as f64 * 0.6 * self.size,
            Family::Helvetica => pdfcraft_fonts::helvetica_width(s, self.size) * if self.bold { 1.04 } else { 1.0 },
            Family::Times => pdfcraft_fonts::helvetica_width(s, self.size) * if self.bold { 0.96 } else { 0.92 },
        };
        designed * self.h_scale
    }
}

// Bytes of text measured on this thread, for tests that bound layout work.
#[cfg(test)]
thread_local! {
    pub(crate) static MEASURED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Default line height: XFA's font leading for the standard faces.
pub fn line_height(size: f64) -> f64 {
    size * 1.15
}

/// One positioned piece of text, ready to emit.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub x: f64,
    /// Baseline, top-left based (y grows downwards).
    pub baseline: f64,
    pub text: String,
    pub face: Face,
}

/// A laid-out block of text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Block {
    pub spans: Vec<Span>,
    /// Height used, from the top of the box.
    pub height: f64,
}

#[derive(Clone, Debug)]
struct Word {
    text: String,
    face: Face,
    width: f64,
}

/// The width of `w` without the spaces it ends with: what it takes at the end of a line, where
/// a space is never drawn.
fn ink_width(w: &Word) -> f64 {
    let ink = w.text.trim_end_matches(' ');
    if ink.len() == w.text.len() { w.width } else { w.face.width(ink) }
}

/// Most bytes of embedded field values one text block shows, as much as a text of its own
/// holds: a value embedded over and over is expanded only so far.
pub(crate) const MAX_EMBEDDED: usize = 1 << 20;

/// The longest prefix of `s` at most `max` bytes long that ends on a character boundary.
pub fn prefix(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let end = s.char_indices().map(|(i, _)| i).take_while(|i| *i <= max).last().unwrap_or(0);
    s.get(..end).unwrap_or_default()
}

/// Break the runs of a paragraph into words (spaces stay attached to the word before them).
/// Embedded values take from `embed_left` bytes and are cut when it runs out; `embed` is asked
/// for at most that many bytes of each.
fn words(runs: &[Run], base: &Face, embed: &dyn Fn(&str, usize) -> String, embed_left: &mut usize) -> Vec<Word> {
    let mut out = Vec::new();
    for run in runs {
        let face = base.with_run(run);
        let text: String = match &run.embed {
            Some(_) if *embed_left == 0 => String::new(),
            Some(id) => {
                let mut v = embed(id, *embed_left);
                if v.len() > *embed_left {
                    v = prefix(&v, *embed_left).to_string();
                }
                *embed_left = embed_left.saturating_sub(v.len());
                v
            }
            None => run
                .text
                .chars()
                .filter_map(|c| match c {
                    '\r' | '\n' | '\t' | '\u{2028}' | '\u{2029}' | '\u{2009}' | '\u{202F}' | '\u{3000}' => Some(' '),
                    '\u{00AD}' | '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' => None,
                    '\u{2011}' | '\u{2010}' | '\u{2012}' | '\u{2013}' => Some('-'),
                    '\u{2018}' | '\u{2019}' => Some('\''),
                    _ => Some(c),
                })
                .collect(),
        };
        let mut cur = String::new();
        for ch in text.chars() {
            cur.push(ch);
            if ch == ' ' {
                let width = face.width(&cur);
                out.push(Word { text: std::mem::take(&mut cur), face: face.clone(), width });
            }
        }
        if !cur.is_empty() {
            let width = face.width(&cur);
            out.push(Word { text: cur, face, width });
        }
    }
    out
}

/// Lay `rich` out inside `rect` with `para` and `base` face. `embed` resolves embedded fields to
/// their text. Returns the spans (absolute coordinates) and the height the text takes.
pub fn layout(rich: &RichText, rect: Rect, para: &Para, base: &Face, embed: &dyn Fn(&str, usize) -> String) -> Block {
    struct Line {
        words: Vec<Word>,
        width: f64,
        size: f64,
        first: bool,
    }
    let avail = (rect.w - para.margin_left - para.margin_right).max(1.0);
    let mut lines: Vec<(Line, Option<HAlign>)> = Vec::new();
    let mut total = 0.0;
    let mut embed_left = MAX_EMBEDDED;
    for (pi, p) in rich.paragraphs.iter().enumerate() {
        let ws = words(&p.runs, base, embed, &mut embed_left);
        let mut line = Line { words: Vec::new(), width: 0.0, size: base.size, first: true };
        let mut first_in_para = true;
        let indent = |first: bool| if first { para.text_indent.max(0.0) } else { 0.0 };
        for w in ws {
            // A word fits when what it draws does: the spaces after it would end the line. Spaces
            // alone draw nothing, so they always stay on the line (keeping their width, so a word
            // after them still starts past them).
            let ink = ink_width(&w);
            let blank = w.text.trim_end_matches(' ').is_empty();
            let fits = blank || line.width + ink <= avail - indent(line.first) || line.words.is_empty() && ink <= avail - indent(line.first);
            if !fits && !line.words.is_empty() {
                let done = std::mem::replace(&mut line, Line { words: Vec::new(), width: 0.0, size: base.size, first: false });
                lines.push((done, p.h_align));
            }
            if ink > avail - indent(line.first) && line.words.is_empty() {
                // A word longer than a line: break it by character. The piece's advance grows by
                // each character's (widths add up), so no prefix is measured again.
                let mut piece = String::new();
                let mut advance = 0.0;
                let mut buf = [0u8; 4];
                for ch in w.text.chars() {
                    let ch_width = w.face.width(ch.encode_utf8(&mut buf));
                    piece.push(ch);
                    advance += ch_width;
                    if advance > avail - indent(line.first) && piece.len() > ch.len_utf8() {
                        piece.pop();
                        let width = w.face.width(&piece);
                        line.words.push(Word { text: std::mem::take(&mut piece), face: w.face.clone(), width });
                        line.width += width;
                        line.size = line.size.max(w.face.size);
                        let done = std::mem::replace(&mut line, Line { words: Vec::new(), width: 0.0, size: base.size, first: false });
                        lines.push((done, p.h_align));
                        piece.push(ch);
                        advance = ch_width;
                    }
                }
                if !piece.is_empty() {
                    let width = w.face.width(&piece);
                    line.size = line.size.max(w.face.size);
                    line.width += width;
                    line.words.push(Word { text: piece, face: w.face.clone(), width });
                }
                first_in_para = false;
                continue;
            }
            line.size = line.size.max(w.face.size);
            line.width += w.width;
            line.words.push(w);
            first_in_para = false;
        }
        if !line.words.is_empty() || first_in_para && pi + 1 < rich.paragraphs.len() {
            lines.push((line, p.h_align));
        }
        total += para.space_above + para.space_below;
    }
    let heights: Vec<f64> = lines.iter().map(|(l, _)| para.line_height.unwrap_or_else(|| line_height(l.size))).collect();
    total += heights.iter().sum::<f64>();
    let start_y = match para.v_align {
        crate::model::VAlign::Top => rect.y,
        crate::model::VAlign::Middle => rect.y + ((rect.h - total) / 2.0).max(0.0),
        crate::model::VAlign::Bottom => rect.y + (rect.h - total).max(0.0),
    };
    let mut spans = Vec::new();
    let mut y = start_y + para.space_above;
    for ((line, align), lh) in lines.into_iter().zip(heights) {
        let indent = if line.first { para.text_indent.max(0.0) } else { 0.0 };
        // Trailing spaces don't count for alignment: those of the last word with ink, and any
        // words of spaces alone after it.
        let mut trailing = 0.0;
        for w in line.words.iter().rev() {
            let ink = ink_width(w);
            trailing += w.width - ink;
            if !w.text.trim_end_matches(' ').is_empty() {
                break;
            }
        }
        let used = (line.width - trailing).max(0.0);
        let mut x = rect.x + para.margin_left + indent;
        x += match align.unwrap_or(para.h_align) {
            HAlign::Left | HAlign::Justify => 0.0,
            HAlign::Center => ((avail - indent - used) / 2.0).max(0.0),
            HAlign::Right => (avail - indent - used).max(0.0),
        };
        // Baseline: ascent sits about 0.8 of the size below the line top for these faces.
        let baseline = y + lh - (lh - line.size * 0.8).max(0.0) / 2.0 - line.size * 0.08;
        let mut run: Option<Span> = None;
        for w in line.words {
            match &mut run {
                Some(s) if s.face == w.face => s.text.push_str(&w.text),
                _ => {
                    if let Some(s) = run.take() {
                        x += s.face.width(&s.text);
                        spans.push(s);
                    }
                    run = Some(Span { x, baseline, text: w.text, face: w.face });
                }
            }
        }
        if let Some(s) = run.take() {
            spans.push(s);
        }
        y += lh;
    }
    Block { spans, height: total }
}

/// The width `rich` takes with no line wrapped (its longest paragraph, with the paragraph's
/// margins and first-line indent): how wide a box without an explicit width grows.
pub fn natural_width(rich: &RichText, para: &Para, base: &Face, embed: &dyn Fn(&str, usize) -> String) -> f64 {
    let indent = para.text_indent.max(0.0);
    let mut embed_left = MAX_EMBEDDED;
    let widest =
        rich.paragraphs.iter().map(|p| words(&p.runs, base, embed, &mut embed_left).iter().map(|w| w.width).sum::<f64>()).fold(0.0, f64::max);
    (para.margin_left + indent + widest + para.margin_right).max(0.0)
}

/// Height `rich` needs at `width` (for boxes without an explicit height).
pub fn measure_height(rich: &RichText, width: f64, para: &Para, base: &Face, embed: &dyn Fn(&str, usize) -> String) -> f64 {
    let p = Para { v_align: crate::model::VAlign::Top, ..para.clone() };
    layout(rich, Rect::new(0.0, 0.0, width.max(1.0), 0.0), &p, base, embed).height
}

/// Plain text as rich text (paragraphs split on line breaks; a carriage return and line feed
/// together are one break, as form data often holds them).
pub fn plain(text: &str) -> RichText {
    RichText {
        paragraphs: text
            .replace("\r\n", "\n")
            .split(['\n', '\r', '\u{2028}', '\u{2029}'])
            .map(|l| crate::model::Paragraph { runs: vec![Run { text: l.to_string(), ..Run::default() }], h_align: None })
            .collect(),
    }
}
