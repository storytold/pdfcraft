//! Literal search in normalized page text, retaining the original glyph owners.
//!
//! The page text is read as characters, each owned by the glyph it came from. Words and lines are
//! separated by one space, except where a line of Chinese or Japanese runs on to the next: a break
//! between two such characters is no word break, so it reads as nothing and "検知機構" is found
//! when the line ends after "検知". A Latin compatibility ligature (U+FB00–U+FB06) reads as its
//! letters in the page and in the query, so "office" finds "o\u{FB03}ce". A word divided by a
//! hyphen at the end of a line is also found whole: where a line ends in a hyphen between two
//! letters, the text is searched a second time without the hyphen and the line break ("example"
//! finds "exam-" + "ple") and a third time with the hyphen but without the break ("exam-ple"). A
//! hit in any reading counts; where hits from different readings overlap, the one that starts
//! first is kept (the longer on a tie). A soft hyphen needs no second reading: it draws nothing,
//! and a line that ends in one always runs on.
use super::{PageText, fold_case};
use std::ops::Range;

/// How a line ending in a hyphen between two letters is read.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hyphen {
    /// As written: the hyphen, then a line break (a space).
    Written,
    /// One word: neither the hyphen nor the break.
    Joined,
    /// A hyphenated compound: the hyphen, but no break.
    Kept,
}

/// The letters of a Latin compatibility ligature (U+FB00–U+FB06), their compatibility decomposition.
fn ligature(c: char) -> Option<&'static str> {
    match c {
        '\u{FB00}' => Some("ff"),
        '\u{FB01}' => Some("fi"),
        '\u{FB02}' => Some("fl"),
        '\u{FB03}' => Some("ffi"),
        '\u{FB04}' => Some("ffl"),
        '\u{FB05}' | '\u{FB06}' => Some("st"),
        _ => None,
    }
}

/// `s` with each Latin compatibility ligature written as its letters. Nothing else is normalised.
fn letters(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match ligature(c) {
            Some(l) => out.push_str(l),
            None => out.push(c),
        }
    }
    out
}

/// A visible hyphen that can divide a word at a line end: HYPHEN-MINUS and HYPHEN. A SOFT HYPHEN
/// (ISO 32000-2 §14.8.2.3) is invisible instead: find skips it, and a line that ends in one runs on.
fn is_hyphen(c: char) -> bool {
    matches!(c, '-' | '\u{2010}')
}

/// Whether a line break separates glyph `i` from the glyph before it.
fn breaks_before(page: &PageText, i: usize) -> bool {
    i.checked_sub(1).and_then(|previous| page.line_of.get(previous)).zip(page.line_of.get(i)).is_some_and(|(a, b)| a != b)
}

pub(super) fn find(page: &PageText, query: &str, sensitive: bool, whole_words: bool, limit: usize) -> Vec<Range<usize>> {
    if limit == 0 {
        return Vec::new();
    }
    let fold = |s: &str| if sensitive { letters(s) } else { fold_case(&letters(s)) };
    let query: String = fold(query).chars().filter(|&c| !is_invisible(c)).collect();
    let needle: Vec<char> = query.split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let texts: Vec<String> = page.glyphs.iter().map(|g| fold(&g.text)).collect();
    let divided: Vec<bool> = (0..texts.len()).map(|i| divides(page, &texts, i)).collect();
    let readings: &[Hyphen] = if divided.contains(&true) { &[Hyphen::Written, Hyphen::Joined, Hyphen::Kept] } else { &[Hyphen::Written] };
    // KMP reuses a matched prefix instead of comparing a long query again at every position.
    // The table is bounded by the page's already materialized character count.
    let mut prefix = vec![0; needle.len()];
    let mut matched = 0;
    for (i, &c) in needle.iter().enumerate().skip(1) {
        matched = advance(&needle, &prefix, matched, c);
        if let Some(slot) = prefix.get_mut(i) {
            *slot = matched;
        }
    }
    let mut hits: Vec<(Range<usize>, usize)> = Vec::new();
    for (r, &reading) in readings.iter().enumerate() {
        let (chars, zero_width_space) = characters(page, &texts, &divided, reading);
        let found = search(&chars, &zero_width_space, &needle, &prefix, whole_words, limit);
        hits.extend(found.into_iter().map(|hit| (hit, r)));
    }
    if readings.len() == 1 {
        return hits.into_iter().map(|(hit, _)| hit).collect();
    }
    // Each reading's hits are in order and apart in its characters, though two can share a glyph
    // ("f" twice in an "ffi" glyph); they are all kept. Across readings, the earliest start wins
    // (the longer on a tie): a hit is left out where it overlaps one kept from another reading.
    // The sort is stable, so the earlier reading wins an exact tie.
    hits.sort_by(|a, b| a.0.start.cmp(&b.0.start).then(b.0.end.cmp(&a.0.end)));
    // The furthest end of the hits kept from each reading. Hits come in order of start, so one
    // overlaps an earlier kept hit exactly when it starts before that hit's end.
    let mut ends = [0usize; 3];
    let mut kept: Vec<Range<usize>> = Vec::new();
    for (hit, r) in hits {
        if ends.iter().enumerate().all(|(other, &end)| other == r || hit.start >= end) {
            if let Some(end) = ends.get_mut(r) {
                *end = (*end).max(hit.end);
            }
            kept.push(hit);
            if kept.len() == limit {
                break;
            }
        }
    }
    kept
}

/// Whether glyph `i` ends its line with a hyphen that follows a letter of its word, and the next
/// line starts with a letter. `texts` are the glyphs' folded texts; the invisible characters find
/// skips are passed over on both sides.
fn divides(page: &PageText, texts: &[String], i: usize) -> bool {
    let Some(text) = texts.get(i) else { return false };
    let mut back = text.chars().rev().filter(|&c| !is_invisible(c));
    if !back.next().is_some_and(is_hyphen) {
        return false;
    }
    let before = match back.next() {
        Some(c) => Some(c),
        // A hyphen glyph of its own: the letter is the end of the glyph before, on its line with no word gap.
        None if !page.space_before.get(i).copied().unwrap_or(false) && !breaks_before(page, i) => {
            i.checked_sub(1).and_then(|p| texts.get(p)).and_then(|t| t.chars().rev().find(|&c| !is_invisible(c)))
        }
        None => None,
    };
    let Some(next) = i.checked_add(1) else { return false };
    before.is_some_and(char::is_alphabetic)
        && breaks_before(page, next)
        && texts.get(next).and_then(|t| t.chars().find(|&c| !is_invisible(c))).is_some_and(char::is_alphabetic)
}

/// The page's characters in one `reading`, each with the glyph it came from, and whether a
/// skipped ZERO WIDTH SPACE came right before each one: it draws nothing, but it is a word
/// boundary (UAX #29 breaks on both sides of it), which Thai and Khmer write between words with
/// no visible gap, so whole-word search still sees two words there.
fn characters(page: &PageText, texts: &[String], divided: &[bool], reading: Hyphen) -> (Vec<(char, usize)>, Vec<bool>) {
    let mut chars: Vec<(char, usize)> = Vec::new();
    let mut zero_width_space: Vec<bool> = Vec::new();
    let mut pending_zero_width_space = false;
    // The last character pushed other than a variation selector, kept as characters are pushed so
    // a long run of selectors costs no rescan.
    let mut last_base: Option<char> = None;
    for (i, text) in texts.iter().enumerate() {
        let line_break = breaks_before(page, i);
        // A soft hyphen marks where a word may be divided at a line end (ISO 32000-2 §14.8.2.3), so
        // a line that ends in one runs on to the next with no word break between them.
        // Other invisible characters after the soft hyphen don't hide it.
        let soft_break = line_break && last_kept(page, i) == Some('\u{AD}');
        // So does a line of Chinese or Japanese that runs on to the next. The characters on both
        // sides of the break are the visible ones: invisible characters are skipped below, and a
        // variation selector after the last one only picks its glyph.
        let joined = line_break && last_base.is_some_and(unspaced) && first_visible(page, i).is_some_and(unspaced) && wraps(page, i);
        // And, in the readings that join it, a line that ends in a visible hyphen between letters.
        let after_divide = reading != Hyphen::Written && i.checked_sub(1).and_then(|p| divided.get(p)).copied().unwrap_or(false);
        let gap = page.space_before.get(i).copied().unwrap_or(false) || (line_break && !(joined || soft_break || after_divide));
        if i > 0 && gap && chars.last().is_some_and(|c| c.0 != ' ') {
            chars.push((' ', i));
            last_base = Some(' ');
            zero_width_space.push(false);
            pending_zero_width_space = false;
        }
        // The divided glyph's hyphen is its last visible character, left out of the joined reading.
        let dropped = if reading == Hyphen::Joined && divided.get(i).copied().unwrap_or(false) {
            text.char_indices().rev().find(|&(_, c)| !is_invisible(c)).map(|(at, _)| at)
        } else {
            None
        };
        for (at, c) in text.char_indices() {
            if is_invisible(c) {
                pending_zero_width_space |= c == '\u{200B}';
                continue;
            }
            if Some(at) == dropped {
                continue;
            }
            let c = if c.is_whitespace() { ' ' } else { c };
            if c != ' ' || chars.last().is_none_or(|last| last.0 != ' ') {
                chars.push((c, i));
                if !is_variation_selector(c) {
                    last_base = Some(c);
                }
                zero_width_space.push(pending_zero_width_space && c != ' ');
                pending_zero_width_space = false;
            }
        }
    }
    (chars, zero_width_space)
}

/// Non-overlapping hits of `needle` (with its KMP `prefix` table) in `chars`, as glyph ranges.
/// `zero_width_space` says which characters come right after a skipped zero width space.
fn search(
    chars: &[(char, usize)],
    zero_width_space: &[bool],
    needle: &[char],
    prefix: &[usize],
    whole_words: bool,
    limit: usize,
) -> Vec<Range<usize>> {
    if needle.len() > chars.len() {
        return Vec::new();
    }
    let mut matched = 0;
    // A neighbour bounds a word only in a script that puts spaces between words. Han, kana, Thai
    // and the like run their words together, so a character of theirs next to the match is no
    // reason to reject it: Acrobat finds 台北 inside 台北市 with Whole words only on. Digits still
    // bound, in any script: ๑ is not a whole word inside the number ๑๒.
    let word = |i: Option<usize>| i.and_then(|i| chars.get(i)).is_some_and(|c| c.0.is_alphanumeric() && (c.0.is_numeric() || !unspaced_script(c.0)));
    // A skipped zero width space before the first character or after the last is a boundary.
    let after_break = |i: usize| zero_width_space.get(i).copied().unwrap_or(false);
    let mut hits = Vec::new();
    for (i, &(c, owner)) in chars.iter().enumerate() {
        matched = advance(needle, prefix, matched, c);
        if matched != needle.len() {
            continue;
        }
        let end = i.saturating_add(1);
        let start = end.saturating_sub(needle.len());
        let bounded = !whole_words || ((after_break(start) || !word(start.checked_sub(1))) && (after_break(end) || !word(Some(end))));
        if bounded {
            if let Some((_, first)) = chars.get(start)
                && let Some(last) = owner.checked_add(1)
            {
                hits.push(*first..last);
                if hits.len() == limit {
                    break;
                }
            }
            // Only accepted hits consume their full span: literal search is non-overlapping.
            matched = 0;
        } else {
            // A word-boundary rejection must still allow an overlapping later candidate.
            matched = prefix.last().copied().unwrap_or_default();
        }
    }
    hits
}

/// A character that draws nothing inside a word and only tells layout where a line may break or
/// letters may join: SOFT HYPHEN, ZERO WIDTH SPACE, ZERO WIDTH NON-JOINER and JOINER, WORD JOINER
/// and ZERO WIDTH NO-BREAK SPACE. All six are Unicode default-ignorable format characters, so
/// find skips them in the page text and in the query alike: "Mitarbeiterinnen" matches a word
/// stored with soft hyphens. A visible word gap is its own space in the page text, so skipping a
/// zero width space next to one still keeps the two words apart.
fn is_invisible(c: char) -> bool {
    matches!(c, '\u{00AD}' | '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}')
}

/// A variation selector (VS1-VS16 and VS17-VS256): it picks a variant glyph of the character
/// before it and is no character of its own.
fn is_variation_selector(c: char) -> bool {
    matches!(c as u32, 0xFE00..=0xFE0F | 0xE0100..=0xE01EF)
}

/// The last character before glyph `i` on the line that glyph `i - 1` ends, other than the
/// invisible characters find skips; a soft hyphen counts, since it marks a divided word. Looks
/// back no further than that line's start, so each line is read once.
fn last_kept(page: &PageText, i: usize) -> Option<char> {
    let end = i.checked_sub(1)?;
    let line = page.line_of.get(end)?;
    (0..=end)
        .rev()
        .take_while(|&k| page.line_of.get(k) == Some(line))
        .filter_map(|k| page.glyphs.get(k))
        .flat_map(|g| g.text.chars().rev())
        .find(|&c| c == '\u{AD}' || !is_invisible(c))
}

/// Whether `c` is set solid, with no spaces between words, in text that a line break may divide
/// anywhere: East Asian Width Wide, Fullwidth or Halfwidth (Unicode Standard Annex #11) Han,
/// kana, bopomofo, CJK symbols and punctuation and fullwidth forms other than Latin letters and
/// digits. Hangul is left out: Korean separates its words with spaces, so a line end there is a
/// word break. A line break between two such characters is the CSS segment-break rule's case for removing the break, not turning
/// it into a space (CSS Text Module, segment break transformation).
fn unspaced(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x2FFF // CJK radicals, Kangxi radicals, ideographic description characters
        | 0x3001..=0x303E // CJK symbols and punctuation (U+3000 is a space; U+303F is narrow)
        | 0x3041..=0x30FF // Hiragana, Katakana
        | 0x3105..=0x312F // Bopomofo
        | 0x3190..=0x31FF // Kanbun, Bopomofo extended, CJK strokes, Katakana phonetic extensions
        | 0x3200..=0x4DBF // Enclosed CJK letters, CJK compatibility, Han extension A
        | 0x4E00..=0x9FFF // Han
        | 0xF900..=0xFAFF // CJK compatibility ideographs
        | 0xFE10..=0xFE19 | 0xFE30..=0xFE6F // vertical, compatibility and small forms
        // Fullwidth forms, less the fullwidth digits and Latin letters (U+FF10-U+FF19, U+FF21-U+FF3A,
        // U+FF41-U+FF5A): those are Latin text set wide, whose words still end at a line end.
        | 0xFF01..=0xFF0F | 0xFF1A..=0xFF20 | 0xFF3B..=0xFF40 | 0xFF5B..=0xFF60 | 0xFFE0..=0xFFE6
        | 0xFF61..=0xFF9F // halfwidth CJK punctuation and Katakana (not halfwidth Hangul)
        | 0x16FE0..=0x16FFF | 0x1AFF0..=0x1B2FF // ideographic symbols, kana supplements and extensions
        | 0x20000..=0x3FFFD // Han extensions B and later, compatibility supplement
    ) && !matches!(c as u32, 0x3200..=0x321E | 0x3260..=0x327E) // parenthesised and circled Hangul
}

/// Whether glyph `i` starts the line that the text of glyph `i - 1` runs on to: both run the same
/// way, it sits past that glyph across the lines, by more than half the glyph's line height and
/// at most three, and starts no further along the line than half a line height past it, as a
/// wrapped line does (a line of one character starts level with the one before). Text set side by
/// side on one line (table cells, labels), a cell set lower and further along the row, a column
/// that starts back at the top, a block further down the page and a flow in another direction
/// keep their word break.
fn wraps(page: &PageText, i: usize) -> bool {
    // Measured on the glyphs that own the visible characters either side of the break: an
    // invisible glyph or a variation selector of its own (drawn with no height) says nothing about
    // the lines. Each search stays within one line.
    let Some(end) = i.checked_sub(1) else { return false };
    let visible = |k: &usize| page.glyphs.get(*k).is_some_and(|g| g.text.chars().any(|c| !is_invisible(c) && !is_variation_selector(c)));
    let same_line = |k: &usize, of: usize| page.line_of.get(*k).is_some() && page.line_of.get(*k) == page.line_of.get(of);
    let before = (0..=end).rev().take_while(|k| same_line(k, end)).find(visible).unwrap_or(end);
    let after = (i..page.glyphs.len()).take_while(|k| same_line(k, i)).find(visible).unwrap_or(i);
    let Some((before, after)) = page.glyphs.get(before).zip(page.glyphs.get(after)) else { return false };
    // Unit directions: the same flow has a dot product of 1.
    let same_flow = before.direction[0] * after.direction[0] + before.direction[1] * after.direction[1] > 0.99;
    let centre = |q: &[[f32; 2]; 4]| [q.iter().map(|p| p[0]).sum::<f32>() / 4.0, q.iter().map(|p| p[1]).sum::<f32>() / 4.0];
    let (a, b) = (centre(&before.quad), centre(&after.quad));
    // Towards the next line: the writing direction turned a quarter clockwise in y-down view space.
    let [dx, dy] = before.direction;
    let across = (b[0] - a[0]) * -dy + (b[1] - a[1]) * dx;
    let along = (b[0] - a[0]) * dx + (b[1] - a[1]) * dy;
    let [near, far] = [before.quad[0], before.quad[3]];
    let height = (near[0] - far[0]).hypot(near[1] - far[1]);
    // NaN compares false: no join.
    same_flow && across > height * 0.5 && across <= height * 3.0 && along < height * 0.5
}

/// The first visible character on the line that glyph `i` starts, skipping the invisible characters
/// that find drops, so a line that opens with a zero width character still joins on its first letter.
fn first_visible(page: &PageText, i: usize) -> Option<char> {
    let line = page.line_of.get(i)?;
    page.glyphs.iter().zip(&page.line_of).skip(i).take_while(|(_, l)| *l == line).flat_map(|(g, _)| g.text.chars()).find(|&c| !is_invisible(c))
}

/// Extend a prefix, falling back through strictly shorter proper prefixes on a mismatch.
fn advance(needle: &[char], prefix: &[usize], mut matched: usize, c: char) -> usize {
    while matched > 0 && needle.get(matched) != Some(&c) {
        matched = prefix.get(matched - 1).copied().unwrap_or_default();
    }
    if needle.get(matched) == Some(&c) { matched.saturating_add(1) } else { 0 }
}

/// Whether `c` is a letter of a script written without spaces between words (CJK ideographs,
/// kana, bopomofo, Thai, Lao, Myanmar, Khmer, Tibetan), so that a whole-word search cannot take a
/// neighbouring letter as a word boundary.
fn unspaced_script(c: char) -> bool {
    matches!(
        u32::from(c),
        0x0E00..=0x0EFF      // Thai, Lao
        | 0x0F00..=0x0FFF    // Tibetan
        | 0x1000..=0x109F    // Myanmar
        | 0x1780..=0x17FF    // Khmer
        | 0x2E80..=0x2FDF    // CJK and Kangxi radicals
        | 0x3005..=0x3007    // 々 〆 〇
        | 0x3031..=0x3035    // vertical kana repeat marks
        | 0x303B..=0x303C    // 〻 〼
        | 0x3040..=0x30FF    // hiragana, katakana
        | 0x3100..=0x312F    // bopomofo
        | 0x31A0..=0x31BF    // bopomofo extended
        | 0x31F0..=0x31FF    // katakana phonetic extensions
        | 0x3400..=0x4DBF    // CJK extension A
        | 0x4E00..=0x9FFF    // CJK unified ideographs
        | 0xA9E0..=0xA9FF    // Myanmar extended-B
        | 0xAA60..=0xAA7F    // Myanmar extended-A
        | 0xF900..=0xFAFF    // CJK compatibility ideographs
        | 0xFF66..=0xFF9F    // halfwidth katakana
        | 0x1AFF0..=0x1B16F  // kana extended-B, kana supplement, kana extended-A, small kana
        // The supplementary and tertiary ideographic planes, all CJK ideographs (extensions B
        // to J, compatibility supplement).
        | 0x20000..=0x3FFFF
    )
}

#[cfg(test)]
mod tests {
    use super::letters;

    #[test]
    fn ligatures_read_as_their_letters() {
        assert_eq!(letters("o\u{FB03}ce \u{FB00}\u{FB01}\u{FB02}\u{FB04}\u{FB05}\u{FB06}"), "office fffiflfflstst");
        assert_eq!(letters("\u{FB13}é\u{FF21}\u{2460}"), "\u{FB13}é\u{FF21}\u{2460}", "nothing else is normalised");
    }
}
