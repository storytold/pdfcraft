//! Literal search in normalized page text, retaining the original glyph owners.
//!
//! The page text is read as characters, each owned by the glyph it came from. Words and lines are
//! separated by one space, except where a line of Chinese or Japanese runs on to the next: a break
//! between two such characters is no word break, so it reads as nothing and "検知機構" is found
//! when the line ends after "検知". A Latin compatibility ligature (U+FB00–U+FB06) reads as its
//! letters in the page and in the query, so "office" finds "o\u{FB03}ce". A character of the
//! Halfwidth and Fullwidth Forms block (U+FF00–U+FFEF) reads as the character its `<wide>` or
//! `<narrow>` compatibility decomposition names, and a voiced or semi-voiced katakana as its base
//! and combining sound mark, in the page and in the query, so "ABC" finds "ＡＢＣ", "カナ" finds
//! "ｶﾅ" and "ガ" finds "ｶﾞ" drawn as two glyphs, each way round; a hit never ends between a kana
//! and its sound mark, so "カ" finds neither. This width folding is on unless the search asks
//! for width to count ([`FindOptions::ignore_width`]). An ideographic space (U+3000) is whitespace and
//! reads as one space like any other. A word divided by a hyphen at the end of a line is also
//! found whole: where a line ends in a hyphen between two letters, the text is searched a second
//! time without the hyphen and the line break ("example" finds "exam-" + "ple") and a third time
//! with the hyphen but without the break ("exam-ple"). A hit in any reading counts; where hits
//! from different readings overlap, the one that starts first is kept (the longer on a tie). A
//! soft hyphen needs no second reading: it draws nothing, and a line that ends in one always runs
//! on.
use super::device::ligature_letters;
use super::{FindOptions, PageText, fold_case};
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

/// A visible hyphen that can divide a word at a line end: HYPHEN-MINUS and HYPHEN. A SOFT HYPHEN
/// (ISO 32000-2 §14.8.2.3) is invisible instead: find skips it, and a line that ends in one runs on.
fn is_hyphen(c: char) -> bool {
    matches!(c, '-' | '\u{2010}')
}

/// Whether a line break separates glyph `i` from the glyph before it.
fn breaks_before(page: &PageText, i: usize) -> bool {
    i.checked_sub(1).and_then(|previous| page.line_of.get(previous)).zip(page.line_of.get(i)).is_some_and(|(a, b)| a != b)
}

/// `s` as find compares it, in the page and in the query alike: ligatures as their letters, with
/// `ignore_width` the halfwidth and fullwidth forms as their ordinary width ([`fold_width`]), and,
/// unless `case_sensitive`, case folded. Each step works per character, so a glyph's text folds
/// on its own and every folded character keeps the glyph it came from.
fn search_fold(s: &str, opts: FindOptions) -> String {
    let s = ligature_letters(s.to_owned());
    let s = if opts.ignore_width { fold_width(s) } else { s };
    if opts.case_sensitive { s } else { fold_case(&s) }
}

/// The full-width katakana of the halfwidth katakana U+FF65..=U+FF9D, in order: each one's
/// `<narrow>` decomposition in UnicodeData.txt.
const HALFWIDTH_KATAKANA: &str = "・ヲァィゥェォャュョッーアイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワン";

/// The character a halfwidth or fullwidth form stands for: the target of its `<wide>` or
/// `<narrow>` compatibility decomposition (UnicodeData.txt, field 5; UAX #44), which every
/// assigned character of the Halfwidth and Fullwidth Forms block (U+FF00–U+FFEF) has. Fullwidth
/// ASCII becomes ASCII, halfwidth katakana and Hangul become their ordinary forms, and the
/// halfwidth voiced and semi-voiced sound marks become the combining marks U+3099 and U+309A.
/// Only width differs: unlike NFKC this leaves superscripts, circled numbers, compatibility
/// ideographs and the like alone, and goes one level down, so U+FFE3 FULLWIDTH MACRON stays a
/// macron rather than a space and a combining mark.
fn narrow_or_wide(c: char) -> Option<char> {
    let cp = u32::from(c);
    let to = match cp {
        // Fullwidth ASCII, ! to ~, sits 0xFEE0 above it.
        0xFF01..=0xFF5E => cp.checked_sub(0xFEE0)?,
        0xFF5F => 0x2985,
        0xFF60 => 0x2986,
        0xFF61 => 0x3002,
        0xFF62 => 0x300C,
        0xFF63 => 0x300D,
        0xFF64 => 0x3001,
        0xFF65..=0xFF9D => return HALFWIDTH_KATAKANA.chars().nth(usize::try_from(cp.checked_sub(0xFF65)?).ok()?),
        0xFF9E => 0x3099,
        0xFF9F => 0x309A,
        // Halfwidth Hangul: the filler, then the compatibility jamo in runs.
        0xFFA0 => 0x3164,
        0xFFA1..=0xFFBE => cp.checked_sub(0xFFA1 - 0x3131)?,
        0xFFC2..=0xFFC7 => cp.checked_sub(0xFFC2 - 0x314F)?,
        0xFFCA..=0xFFCF => cp.checked_sub(0xFFCA - 0x3155)?,
        0xFFD2..=0xFFD7 => cp.checked_sub(0xFFD2 - 0x315B)?,
        0xFFDA..=0xFFDC => cp.checked_sub(0xFFDA - 0x3161)?,
        0xFFE0 => 0xA2,
        0xFFE1 => 0xA3,
        0xFFE2 => 0xAC,
        0xFFE3 => 0xAF,
        0xFFE4 => 0xA6,
        0xFFE5 => 0xA5,
        0xFFE6 => 0x20A9,
        0xFFE8 => 0x2502,
        0xFFE9..=0xFFEC => cp.checked_sub(0xFFE9 - 0x2190)?,
        0xFFED => 0x25A0,
        0xFFEE => 0x25CB,
        _ => return None,
    };
    char::from_u32(to)
}

/// A voiced or semi-voiced katakana as its canonical decomposition (UnicodeData.txt): the base
/// katakana and U+3099 COMBINING KATAKANA-HIRAGANA VOICED SOUND MARK or U+309A SEMI-VOICED. A
/// halfwidth katakana takes its sound mark as a character of its own (ｶﾞ), often drawn as a
/// glyph of its own, so the full-width ガ has to read as two characters for the two to match.
fn decompose_katakana(c: char) -> Option<[char; 2]> {
    let cp = u32::from(c);
    let (base, mark) = match cp {
        0x30AC..=0x30C2 if cp % 2 == 0 => (cp.checked_sub(1)?, 0x3099),
        0x30C5 | 0x30C7 | 0x30C9 | 0x30D0 | 0x30D3 | 0x30D6 | 0x30D9 | 0x30DC | 0x30FE => (cp.checked_sub(1)?, 0x3099),
        0x30D1 | 0x30D4 | 0x30D7 | 0x30DA | 0x30DD => (cp.checked_sub(2)?, 0x309A),
        0x30F4 => (0x30A6, 0x3099),
        0x30F7..=0x30FA => (cp.checked_sub(8)?, 0x3099),
        _ => return None,
    };
    Some([char::from_u32(base)?, char::from_u32(mark)?])
}

/// `text` with every halfwidth or fullwidth form at its ordinary width ([`narrow_or_wide`]) and
/// every voiced or semi-voiced katakana decomposed ([`decompose_katakana`]).
fn fold_width(text: String) -> String {
    let folds = |c: char| narrow_or_wide(c).is_some() || decompose_katakana(c).is_some();
    if !text.chars().any(folds) {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let c = narrow_or_wide(c).unwrap_or(c);
        match decompose_katakana(c) {
            Some(parts) => out.extend(parts),
            None => out.push(c),
        }
    }
    out
}

pub(super) fn find(page: &PageText, query: &str, opts: FindOptions, limit: usize) -> Vec<Range<usize>> {
    if limit == 0 {
        return Vec::new();
    }
    let fold = |s: &str| search_fold(s, opts);
    let query: String = fold(query).chars().filter(|&c| !is_invisible(c)).collect();
    let needle: Vec<char> = query.split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let texts: Vec<String> = page.glyphs.iter().map(|g| fold(&g.text)).collect();
    let divided: Vec<bool> = (0..texts.len()).map(|i| divides(page, &texts, i)).collect();
    let readings: &[Hyphen] = if divided.contains(&true) { &[Hyphen::Written, Hyphen::Joined, Hyphen::Kept] } else { &[Hyphen::Written] };
    let prefix = prefix_table(&needle);
    let mut hits: Vec<(Range<usize>, usize)> = Vec::new();
    for (r, &reading) in readings.iter().enumerate() {
        let (chars, zero_width_space) = characters(page, &texts, &divided, reading);
        let found = search(&chars, &zero_width_space, &needle, &prefix, opts, limit);
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

/// [`super::find_in_text`]: the same search over a string (find and replace matches a paragraph's
/// text with it), as the byte ranges of the characters each hit covers. Hyphenated line ends do
/// not arise in a string, so there is the one reading.
pub(super) fn find_in_text(text: &str, query: &str, opts: FindOptions, limit: usize) -> Vec<Range<usize>> {
    if limit == 0 {
        return Vec::new();
    }
    let fold = |s: &str| search_fold(s, opts);
    let query: String = fold(query).chars().filter(|&c| !is_invisible(c)).collect();
    let needle: Vec<char> = query.split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    if needle.is_empty() {
        return Vec::new();
    }
    // Each character is owned by its index in the string, turned back into bytes at the end.
    let mut chars: Vec<(char, usize)> = Vec::new();
    let mut zero_width_space: Vec<bool> = Vec::new();
    let mut pending_zero_width_space = false;
    for (index, (_, c)) in text.char_indices().enumerate() {
        for c in fold(c.encode_utf8(&mut [0; 4])).chars() {
            if is_invisible(c) {
                pending_zero_width_space |= c == '\u{200B}';
                continue;
            }
            let c = if c.is_whitespace() { ' ' } else { c };
            if c != ' ' || chars.last().is_none_or(|last| last.0 != ' ') {
                chars.push((c, index));
                zero_width_space.push(pending_zero_width_space && c != ' ');
                pending_zero_width_space = false;
            }
        }
    }
    let starts: Vec<usize> = text.char_indices().map(|(at, _)| at).chain(std::iter::once(text.len())).collect();
    search(&chars, &zero_width_space, &needle, &prefix_table(&needle), opts, limit)
        .into_iter()
        .filter_map(|hit| Some(*starts.get(hit.start)?..*starts.get(hit.end)?))
        .collect()
}

/// KMP reuses a matched prefix instead of comparing a long query again at every position. The
/// table is bounded by the page's already materialized character count.
fn prefix_table(needle: &[char]) -> Vec<usize> {
    let mut prefix = vec![0; needle.len()];
    let mut matched = 0;
    for (i, &c) in needle.iter().enumerate().skip(1) {
        matched = advance(needle, &prefix, matched, c);
        if let Some(slot) = prefix.get_mut(i) {
            *slot = matched;
        }
    }
    prefix
}

/// Whether `c`, the character before a line-end hyphen, ends a word: a letter, or the voiced or
/// semi-voiced sound mark U+3099 or U+309A after one. Ignoring width writes ガ as カ and U+3099,
/// and the halfwidth mark ﾞ as U+3099; the marks are not letters, so without them a word divided
/// after ガ or ｶﾞ would match whole only where width counts.
fn ends_a_word(c: char) -> bool {
    c.is_alphabetic() || matches!(c, '\u{3099}' | '\u{309A}')
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
    before.is_some_and(ends_a_word)
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
    // The page's own character behind the last one pushed other than a variation selector, kept
    // as characters are pushed so a long run of selectors costs no rescan. It is the page's
    // character, not the folded one: a FULLWIDTH COMMA ends a line of Chinese that runs on,
    // though find reads it as an ASCII comma.
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
        let mut pushed_base = false;
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
                pushed_base |= !is_variation_selector(c);
                zero_width_space.push(pending_zero_width_space && c != ' ');
                pending_zero_width_space = false;
            }
        }
        // The glyph's last visible character other than a variation selector, as the page has it.
        // The hyphen the joined reading drops and the letters a ligature or case folds to are no
        // characters `unspaced` lists, so reading the page's text tells only the width forms apart.
        if pushed_base {
            let raw = page.glyphs.get(i).and_then(|g| g.text.chars().rev().find(|&c| !is_invisible(c) && !is_variation_selector(c)));
            last_base = raw.map(|c| if c.is_whitespace() { ' ' } else { c }).or(last_base);
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
    opts: FindOptions,
    limit: usize,
) -> Vec<Range<usize>> {
    let whole_words = opts.whole_words;
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
    // A hiragana or katakana letter, which a following U+3099 or U+309A voices.
    let kana = |i: Option<usize>| i.and_then(|i| chars.get(i)).is_some_and(|&(c, _)| matches!(c, '\u{3041}'..='\u{3096}' | '\u{309D}'..='\u{30FF}'));
    let mut hits = Vec::new();
    for (i, &(c, owner)) in chars.iter().enumerate() {
        matched = advance(needle, prefix, matched, c);
        if matched != needle.len() {
            continue;
        }
        let end = i.saturating_add(1);
        let start = end.saturating_sub(needle.len());
        let bounded = !whole_words || ((after_break(start) || !word(start.checked_sub(1))) && (after_break(end) || !word(Some(end))));
        // Nor does a hit end on a kana whose sound mark follows: カ is not found in ガ, which find
        // reads as カ and U+3099, any more than in ｶﾞ. A mark after anything else marks no kana.
        // Where width counts, ガ stays one character and find matches characters as written.
        let sound_mark = opts.ignore_width && chars.get(end).is_some_and(|&(next, _)| matches!(next, '\u{3099}' | '\u{309A}'));
        let whole_kana = !(sound_mark && kana(end.checked_sub(1)));
        if bounded && whole_kana {
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
            // A word-boundary or sound-mark rejection must still allow an overlapping later candidate.
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
    use super::{decompose_katakana, fold_width, narrow_or_wide};

    /// Characters of the Halfwidth and Fullwidth Forms block and the target of their `<wide>` or
    /// `<narrow>` decomposition in UnicodeData.txt (Unicode 15.1), in runs that step by one on
    /// both sides: (first, last, first's target). The halfwidth katakana these leave out are
    /// checked one by one below, and the count checks that none of the block's 225 is missed.
    const RUNS: &[(u32, u32, u32)] = &[
        (0xFF01, 0xFF5E, 0x21),
        (0xFF5F, 0xFF60, 0x2985),
        (0xFF61, 0xFF61, 0x3002),
        (0xFF62, 0xFF63, 0x300C),
        (0xFF64, 0xFF64, 0x3001),
        (0xFF65, 0xFF65, 0x30FB),
        (0xFF66, 0xFF66, 0x30F2),
        (0xFF67, 0xFF67, 0x30A1),
        (0xFF6F, 0xFF6F, 0x30C3),
        (0xFF70, 0xFF70, 0x30FC),
        (0xFF71, 0xFF71, 0x30A2),
        (0xFF75, 0xFF76, 0x30AA),
        (0xFF82, 0xFF82, 0x30C4),
        (0xFF85, 0xFF8A, 0x30CA),
        (0xFF8F, 0xFF93, 0x30DE),
        (0xFF96, 0xFF9B, 0x30E8),
        (0xFF9C, 0xFF9C, 0x30EF),
        (0xFF9D, 0xFF9D, 0x30F3),
        (0xFF9E, 0xFF9F, 0x3099),
        (0xFFA0, 0xFFA0, 0x3164),
        (0xFFA1, 0xFFBE, 0x3131),
        (0xFFC2, 0xFFC7, 0x314F),
        (0xFFCA, 0xFFCF, 0x3155),
        (0xFFD2, 0xFFD7, 0x315B),
        (0xFFDA, 0xFFDC, 0x3161),
        (0xFFE0, 0xFFE1, 0xA2),
        (0xFFE2, 0xFFE2, 0xAC),
        (0xFFE3, 0xFFE3, 0xAF),
        (0xFFE4, 0xFFE4, 0xA6),
        (0xFFE5, 0xFFE5, 0xA5),
        (0xFFE6, 0xFFE6, 0x20A9),
        (0xFFE8, 0xFFE8, 0x2502),
        (0xFFE9, 0xFFEC, 0x2190),
        (0xFFED, 0xFFED, 0x25A0),
        (0xFFEE, 0xFFEE, 0x25CB),
    ];

    #[test]
    fn every_width_form_maps_to_its_unicode_decomposition_and_nothing_else_moves() {
        for &(first, last, to) in RUNS {
            for cp in first..=last {
                let c = char::from_u32(cp).unwrap();
                assert_eq!(narrow_or_wide(c), char::from_u32(to + (cp - first)), "U+{cp:04X}");
            }
        }
        let mapped = (0xFF00..=0xFFEF).filter_map(char::from_u32).filter(|&c| narrow_or_wide(c).is_some()).count();
        assert_eq!(mapped, 225, "the block's <wide> and <narrow> characters, no more");
        // Outside the block nothing folds, the targets included: folding is one way and one level.
        for cp in (0..0xFF00).chain(0xFFF0..0x11000) {
            if let Some(c) = char::from_u32(cp) {
                assert_eq!(narrow_or_wide(c), None, "U+{cp:04X}");
            }
        }
        // The halfwidth katakana between the runs above, one by one.
        assert_eq!(
            fold_width("ｦｧｨｩｪｫｬｭｮｯｰｱｲｳｴｵｶｷｸｹｺｻｼｽｾｿﾀﾁﾂﾃﾄﾅﾆﾇﾈﾉﾊﾋﾌﾍﾎﾏﾐﾑﾒﾓﾔﾕﾖﾗﾘﾙﾚﾛﾜﾝ".into()),
            "ヲァィゥェォャュョッーアイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワン"
        );
    }

    #[test]
    fn voiced_katakana_decompose_as_unicode_data_gives_them() {
        // Every katakana with a canonical decomposition to a base and U+3099 or U+309A.
        let voiced = "ガギグゲゴザジズゼゾダヂヅデドバビブベボヴヷヸヹヺヾ";
        let bases = "カキクケコサシスセソタチツテトハヒフヘホウワヰヱヲヽ";
        for (v, b) in voiced.chars().zip(bases.chars()) {
            assert_eq!(decompose_katakana(v), Some([b, '\u{3099}']), "{v}");
        }
        for (v, b) in "パピプペポ".chars().zip("ハヒフヘホ".chars()) {
            assert_eq!(decompose_katakana(v), Some([b, '\u{309A}']), "{v}");
        }
        let decomposed = (0x3000..=0x30FF).filter_map(char::from_u32).filter(|&c| decompose_katakana(c).is_some()).count();
        assert_eq!(decomposed, 31, "no other character decomposes");
        assert_eq!(fold_width("ｶﾞｲﾄﾞ ﾊﾟﾝ".into()), fold_width("ガイド パン".into()));
        assert_eq!(fold_width("plain text".into()), "plain text");
    }
}
