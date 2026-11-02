//! Right-to-left text in Edit text (Hebrew, Arabic).
//!
//! A content stream draws glyphs left to right, so a producer writes a right-to-left line in
//! *visual* order: `בדיוק` is drawn, and read back from the stream, as `קוידב`. The
//! editor works on *logical* (typing) order instead: [`to_logical`] turns a line read from the
//! page into the order a person types it, and [`to_visual`] turns a line about to be written back
//! into the order it must be drawn in (UAX #9 through `unicode-bidi`).
//!
//! Visual order doesn't always determine one logical order (`abc 12 םולש` is both `abc שלום 12`
//! and `abc 12 שלום`), so [`to_logical`] tries a few readings and keeps the first that draws back
//! exactly as the page did. Lines without right-to-left characters are returned unchanged.
//!
//! Combining marks (Hebrew points, Arabic harakat) stay after their base character in both
//! orders. Brackets drawn in a right-to-left run are mirrored, as UAX #9 L4 mirrors them on
//! display.

use unicode_bidi::{BidiInfo, Level};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Rtl,
    Ltr,
    Number,
    Neutral,
}

/// Letters, marks and punctuation of the right-to-left scripts' blocks (digits excepted).
pub(crate) fn is_rtl(c: char) -> bool {
    !is_digit(c) && matches!(u32::from(c), 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFC | 0x1_0800..=0x1_0FFF | 0x1_E800..=0x1_EFFF)
}

/// The Hebrew block and the Hebrew presentation forms.
pub(crate) fn is_hebrew(c: char) -> bool {
    matches!(u32::from(c), 0x0590..=0x05FF | 0xFB1D..=0xFB4F)
}

/// Hebrew points and cantillation marks (niqqud, te'amim): combining, drawn on their letter.
pub(crate) fn is_hebrew_mark(c: char) -> bool {
    matches!(u32::from(c), 0x0591..=0x05BD | 0x05BF | 0x05C1..=0x05C2 | 0x05C4..=0x05C5 | 0x05C7 | 0xFB1E)
}

fn is_digit(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '\u{0660}'..='\u{0669}' | '\u{06F0}'..='\u{06F9}')
}

/// Combining marks that ride on the character before them.
fn is_mark(c: char) -> bool {
    is_hebrew_mark(c)
        || matches!(
            u32::from(c),
            0x0300..=0x036F | 0x0610..=0x061A | 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06DC | 0x06DF..=0x06E4 | 0x06E7..=0x06E8 | 0x06EA..=0x06ED | 0xFE20..=0xFE2F
        )
}

/// Direction marks, embeddings, overrides and isolates: they steer the reordering and are not drawn.
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

fn class(c: char) -> Class {
    if is_digit(c) {
        Class::Number
    } else if is_rtl(c) {
        Class::Rtl
    } else if c.is_alphabetic() {
        Class::Ltr
    } else {
        Class::Neutral
    }
}

/// A base character with the combining marks after it.
#[derive(Clone, Debug)]
struct Cluster {
    text: String,
    class: Class,
}

impl Cluster {
    /// The cluster with its base mirrored (marks stay as they are).
    fn mirror(&self) -> Cluster {
        let mut chars = self.text.chars();
        let mut text: String = chars.next().map(mirrored).into_iter().collect();
        text.extend(chars);
        Cluster { text, class: self.class }
    }
}

fn clusters(s: &str) -> Vec<Cluster> {
    let mut out: Vec<Cluster> = Vec::new();
    for c in s.chars() {
        match out.last_mut() {
            Some(last) if is_mark(c) => last.text.push(c),
            _ => out.push(Cluster { text: c.to_string(), class: class(c) }),
        }
    }
    out
}

/// Whether the line reads right to left as a whole: more right-to-left letters than
/// left-to-right ones. Counting (not the first strong character) gives the same answer for a
/// line in either order, so reading a line and writing it back agree on its direction.
pub(crate) fn rtl_base(text: &str) -> bool {
    let (mut r, mut l) = (0usize, 0usize);
    for c in text.chars() {
        match class(c) {
            Class::Rtl if !is_mark(c) => r = r.saturating_add(1),
            Class::Ltr if !is_mark(c) => l = l.saturating_add(1),
            _ => {}
        }
    }
    r > l
}

/// Maximal ranges that start and end with a `member` and hold only members and `bridge`s.
fn runs(cl: &[Cluster], member: impl Fn(&Cluster) -> bool, bridge: impl Fn(&[Cluster], usize) -> bool) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < cl.len() {
        if !cl.get(i).is_some_and(&member) {
            i += 1;
            continue;
        }
        let mut end = i;
        let mut j = i + 1;
        while let Some(c) = cl.get(j) {
            if member(c) {
                end = j;
            } else if !bridge(cl, j) {
                break;
            }
            j += 1;
        }
        out.push(i..end + 1);
        i = end + 1;
    }
    out
}

/// One separator between two digits (`1,000`, `12:30`, `3.5`, `1/2`, `5-6`) keeps a number whole.
fn number_separator(cl: &[Cluster], j: usize) -> bool {
    let digit = |k: Option<usize>| k.and_then(|k| cl.get(k)).is_some_and(|c| c.class == Class::Number);
    cl.get(j).is_some_and(|c| matches!(c.text.as_str(), "." | "," | ":" | "/" | "-" | "+")) && digit(j.checked_sub(1)) && digit(Some(j + 1))
}

fn reverse_mirrored(cl: &mut [Cluster]) {
    cl.reverse();
    for c in cl.iter_mut() {
        *c = c.mirror();
    }
}

/// One reading of `visual` in logical order. `wide` lets runs of the other direction swallow the
/// spaces and numbers between their words; narrow keeps numbers and words apart.
fn reading(visual: &[Cluster], rtl: bool, wide: bool) -> String {
    let mut cl = visual.to_vec();
    let number = |c: &Cluster| c.class == Class::Number;
    if rtl {
        // Everything reads right to left, except left-to-right words and numbers.
        reverse_mirrored(&mut cl);
        let ltr = |c: &Cluster| matches!(c.class, Class::Ltr | Class::Number);
        let bridge =
            |cl: &[Cluster], j: usize| cl.get(j).is_some_and(|c| c.class == Class::Neutral) && (wide || !neutral_between_word_and_number(cl, j));
        for r in runs(&cl, ltr, bridge) {
            if let Some(run) = cl.get_mut(r) {
                reverse_mirrored(run);
            }
        }
    } else {
        // Left to right, except right-to-left stretches; numbers inside those read left to right.
        let rtl_member = |c: &Cluster| c.class == Class::Rtl;
        let bridge = |cl: &[Cluster], j: usize| cl.get(j).is_some_and(|c| c.class == Class::Neutral || (wide && c.class == Class::Number));
        for r in runs(&cl, rtl_member, bridge) {
            if let Some(run) = cl.get_mut(r) {
                reverse_mirrored(run);
                for n in runs(run, number, number_separator) {
                    if let Some(num) = run.get_mut(n) {
                        reverse_mirrored(num);
                    }
                }
            }
        }
    }
    cl.iter().map(|c| c.text.as_str()).collect()
}

/// Whether the neutral at `j` sits between a left-to-right word on one side and a number on the
/// other (UAX #9 resolves such a space to the line's direction, splitting the run).
fn neutral_between_word_and_number(cl: &[Cluster], j: usize) -> bool {
    let strong = |mut k: usize, step: isize| -> Option<Class> {
        loop {
            k = k.checked_add_signed(step)?;
            match cl.get(k)?.class {
                Class::Neutral => continue,
                other => return Some(other),
            }
        }
    };
    matches!((strong(j, -1), strong(j, 1)), (Some(Class::Ltr), Some(Class::Number)) | (Some(Class::Number), Some(Class::Ltr)))
}

/// A line read from a content stream (drawn left to right), in logical order.
pub fn to_logical(visual: &str) -> String {
    if !visual.chars().any(is_rtl) {
        return visual.to_owned();
    }
    let cl = clusters(visual);
    let rtl = rtl_base(visual);
    let readings = [reading(&cl, rtl, true), reading(&cl, rtl, false)];
    // Readings that draw back exactly as the page did; of those, one whose brackets pair up
    // (`12 (PDF)`, not `PDF) 12)`), else the first.
    let exact: Vec<&String> = readings.iter().filter(|r| to_visual(r) == visual).collect();
    let chosen = exact.iter().find(|r| balanced(r)).or(exact.first()).map_or_else(|| readings[0].clone(), |r| (*r).clone());
    // Some producers (LibreOffice) map a bracket drawn in a right-to-left run to the character it
    // stands for, not the shape drawn, so mirroring it again turns it inside out: `)טיוטה(`.
    let swapped: String = chosen.chars().map(mirrored).collect();
    if !balanced(&chosen) && balanced(&swapped) { swapped } else { chosen }
}

/// Whether every closing bracket closes an open one.
fn balanced(text: &str) -> bool {
    let mut open: Vec<char> = Vec::new();
    for c in text.chars() {
        match c {
            '(' | '[' | '{' => open.push(c),
            ')' | ']' | '}' if open.pop() != Some(mirrored(c)) => return false,
            _ => {}
        }
    }
    open.is_empty()
}

/// A line in logical order, in the order it must be drawn (left to right), without the bidi
/// controls fonts can't draw.
pub fn to_visual(logical: &str) -> String {
    let text: String = logical.chars().filter(|c| !is_bidi_control(*c)).collect();
    if !text.chars().any(is_rtl) {
        return text;
    }
    let level = if rtl_base(&text) { Level::rtl() } else { Level::ltr() };
    let info = BidiInfo::new(&text, Some(level));
    let mut out = String::with_capacity(text.len());
    for para in &info.paragraphs {
        let (levels, runs) = info.visual_runs(para, para.range.clone());
        for run in runs {
            let rtl = levels.get(run.start).is_some_and(|l| l.is_rtl());
            // Runs lie on character boundaries; a missing slice is skipped, never a panic.
            let Some(piece) = text.get(run) else { continue };
            if rtl {
                for c in clusters(piece).iter().rev() {
                    out.push_str(&c.mirror().text);
                }
            } else {
                out.push_str(piece);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a producer writes into the content stream for `logical`.
    fn drawn(logical: &str) -> String {
        to_visual(logical)
    }

    #[test]
    fn the_reported_word_reads_back_in_typing_order() {
        // The report: Edit text showed "בדיוק" (exactly) reversed.
        assert_eq!(drawn("בדיוק"), "קוידב");
        assert_eq!(to_logical("קוידב"), "בדיוק");
    }

    #[test]
    fn hebrew_sentences_numbers_latin_and_brackets_round_trip() {
        for logical in [
            "שלום עולם",
            "עמוד 12 מתוך 40",
            "מחיר: 1,250.50 ₪",
            "השעה 12:30",
            "קובץ PDF חדש",
            "שלום (עולם)",
            "ראו [1] ו-[2]",
            "גרסה 2.0 של PdfCraft נוסחה",
            "hello שלום עולם world",
            "abc שלום def",
            "שלום abc def עולם",
            "Report: דוח שנתי 2026",
            "שלום עולם, עמוד 12 (PDF)",
            "פרק 3 (מבוא)",
            "שָׁלוֹם עוֹלָם",
            "مرحبا بالعالم",
        ] {
            let visual = drawn(logical);
            assert_ne!(visual, logical, "{logical:?} should be reordered");
            assert_eq!(to_visual(&to_logical(&visual)), visual, "{logical:?} drawn as {visual:?}");
            assert_eq!(to_logical(&visual), logical, "{logical:?} drawn as {visual:?}");
        }
    }

    #[test]
    fn ambiguous_lines_still_draw_back_the_same() {
        // "abc 12 םולש" is both "abc שלום 12" and "abc 12 שלום"; either must draw the same.
        for logical in ["abc שלום 12", "שלום 12 abc", "a (ב) c", "דוח Report 2026", "(1) שלום"] {
            let visual = drawn(logical);
            assert_eq!(to_visual(&to_logical(&visual)), visual, "{logical:?}");
        }
    }

    #[test]
    fn brackets_mapped_to_their_logical_character_pair_up() {
        // LibreOffice draws "(טיוטה)" with ToUnicode giving the logical brackets: `)הטויט(`.
        assert_eq!(to_logical(")הטויט("), "(טיוטה)");
        // Brackets mapped to the shape drawn read the same.
        assert_eq!(to_logical("(הטויט)"), "(טיוטה)");
    }

    #[test]
    fn left_to_right_text_is_untouched() {
        for s in ["", "Hello, world (1)", "日本語のテキスト", "Ünïcödé 12:30"] {
            assert_eq!(to_logical(s), s);
            assert_eq!(to_visual(s), s);
        }
    }

    #[test]
    fn points_stay_on_their_letter() {
        // Each point follows its letter in both orders.
        let visual = drawn("שָׁלוֹם");
        let first = visual.chars().next();
        assert_eq!(first, Some('ם'));
        assert!(visual.contains("לוֹ") || visual.contains("וֹל"), "{visual:?}");
        assert_eq!(to_logical(&visual), "שָׁלוֹם");
    }

    #[test]
    fn controls_are_not_drawn_and_hostile_input_does_not_panic() {
        assert_eq!(to_visual("\u{200F}שלום\u{202B}"), "םולש");
        for s in ["\u{05B8}", "\u{05B8}\u{05B8}א", "(((", "א\u{0300}", "1-", "-1א", "א\n\tב", "\u{202E}abc"] {
            let _ = to_visual(&to_logical(s));
        }
        let long = "שלום 12 abc (x) ".repeat(2_000);
        assert_eq!(to_visual(&to_logical(&to_visual(&long))), to_visual(&long));
    }
}
