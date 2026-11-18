//! Wrapping right-to-left text into logical lines.
//!
//! egui 0.37's epaint shapes text with the Unicode bidirectional algorithm (UAX #9), so
//! production code passes it logical (typing-order) text: [`wrap`] breaks a comment into logical
//! lines and lets epaint reorder each one.

use std::ops::Range;

use unicode_bidi::BidiInfo;

/// One line of wrapped text (see [`wrap`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// The line in logical (reading) order. epaint shapes and reorders it for display.
    pub text: String,
    /// The paragraph reads right to left: the line belongs against the right edge.
    pub rtl: bool,
}

/// Whether `text` holds any right-to-left script, so [`wrap`] is needed.
pub fn has_rtl(text: &str) -> bool {
    text.chars().any(is_rtl_script)
}

/// Multi-line text with right-to-left script (a comment in Persian, Arabic, Hebrew), broken
/// into logical-order lines that `fits` accepts (it measures a line in logical order).
///
/// The lines keep reading order and each line's paragraph direction ([`Line::rtl`]): egui 0.37's
/// epaint already shapes every line with the Unicode bidirectional algorithm (UAX #9), so
/// preordering here would reorder mixed Persian/Latin text a second time. Lines break between
/// words, in reading order, and a word wider than a line between characters. Copying and
/// accessibility use the same logical text.
pub fn wrap(text: &str, mut fits: impl FnMut(&str) -> bool) -> Vec<Line> {
    let info = BidiInfo::new(text, None);
    let mut lines = Vec::new();
    for para in &info.paragraphs {
        let whole = text.get(para.range.clone()).unwrap_or_default();
        // unicode-bidi ends a paragraph at the `\r` of `\r\n`; the `\n` is no new paragraph.
        if whole == "\n" && text.get(..para.range.start).is_some_and(|before| before.ends_with('\r')) {
            continue;
        }
        let content = para.range.start..para.range.start.saturating_add(whole.trim_end_matches(is_paragraph_separator).len());
        let rtl = para.level.is_rtl();
        for line in break_lines(text, content, &mut fits) {
            lines.push(Line { text: text.get(line).unwrap_or_default().to_owned(), rtl });
        }
    }
    lines
}

/// The byte ranges of the lines `range` of `text` breaks into: as many words as `fits` accepts
/// per line, a word too wide for any line split between characters. Always at least one line.
fn break_lines(text: &str, range: Range<usize>, fits: &mut impl FnMut(&str) -> bool) -> Vec<Range<usize>> {
    let mut lines = Vec::new();
    let mut current: Option<Range<usize>> = None;
    for word in words(text, range.clone()) {
        if let Some(line) = current.as_mut() {
            if fits(text.get(line.start..word.end).unwrap_or_default()) {
                line.end = word.end;
                continue;
            }
            lines.push(line.clone());
        }
        // The word starts a line.
        let mut start = word.start;
        loop {
            let end = fitting_prefix(text, start..word.end, fits);
            if end >= word.end {
                current = Some(start..word.end);
                break;
            }
            lines.push(start..end);
            start = end;
        }
    }
    lines.push(current.unwrap_or(range.start..range.start));
    lines
}

/// The end of the longest start of `range` that `fits`, at least one character.
fn fitting_prefix(text: &str, range: Range<usize>, fits: &mut impl FnMut(&str) -> bool) -> usize {
    let piece = text.get(range.clone()).unwrap_or_default();
    let mut end = range.end;
    for (i, (at, c)) in piece.char_indices().enumerate() {
        let next = range.start.saturating_add(at).saturating_add(c.len_utf8());
        if i > 0 && !fits(text.get(range.start..next).unwrap_or_default()) {
            end = range.start.saturating_add(at);
            break;
        }
    }
    end
}

/// The byte ranges of the whitespace-separated words of `range` in `text`.
fn words(text: &str, range: Range<usize>) -> Vec<Range<usize>> {
    let mut words = Vec::new();
    let mut start = None;
    for (at, c) in text.get(range.clone()).unwrap_or_default().char_indices() {
        let at = range.start.saturating_add(at);
        match (c.is_whitespace(), start) {
            (true, Some(s)) => {
                words.push(s..at);
                start = None;
            }
            (false, None) => start = Some(at),
            _ => {}
        }
    }
    if let Some(s) = start {
        words.push(s..range.end);
    }
    words
}

/// Characters that end a paragraph (bidi class B).
fn is_paragraph_separator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{1C}'..='\u{1E}' | '\u{85}' | '\u{2029}')
}

/// Characters of the right-to-left scripts' blocks: letters, marks and the scripts' own
/// punctuation, all drawn by the script's face.
fn is_rtl_script(c: char) -> bool {
    matches!(u32::from(c), 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFC | 0x1_0800..=0x1_0FFF | 0x1_E800..=0x1_EFFF)
}

#[cfg(test)]
mod tests;
