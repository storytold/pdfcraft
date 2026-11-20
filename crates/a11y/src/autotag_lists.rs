//! Lists among auto-tagged text objects (#7068).
//!
//! A text object starts with a list marker when its text begins with a bullet, or with a number,
//! letter or roman numeral followed by `.` or `)` or enclosed in `( )`, and then white space or
//! nothing. Two or more consecutive paragraph objects whose markers have the same kind and
//! punctuation, start at the same indent and, when ordered, count up by one, form a list: `L` ›
//! `LI` › `LBody`. An object that shows only a marker, followed by the item's text on the same
//! line, is the item's `Lbl`. Anything else ends the list, and a run of one item stays a paragraph.
//!
//! A marker shown by its own operator, before the item's text in the same text object, is tagged
//! `Lbl` there (see `Evidence::label_op`). Otherwise only whole text objects are grouped: a list
//! drawn in one text object (an object with marked lines after its first stays a paragraph), a marker in the same show as its item's text (it stays in `LBody`),
//! an item's wrapped lines drawn as separate objects, and nested lists are not recognised. Those
//! lines end the list, or stay paragraphs, as they were tagged before.

use crate::autotag_roles::Evidence;

/// Markers this far apart horizontally, in points, are at different indents.
const INDENT_TOLERANCE: f64 = 2.0;
/// A label and its item's text are on one line when their baselines are this many text sizes
/// apart or less.
const SAME_LINE: f64 = 0.5;
/// Decimal markers longer than this are numbers, not list markers.
const MAX_DIGITS: usize = 3;
/// Roman numerals longer than this are words, not list markers.
const MAX_ROMAN: usize = 6;

/// How a list's items are marked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Numbering {
    Bullet(char),
    Decimal,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
}

impl Numbering {
    /// The `/ListNumbering` attribute value (ISO 32000-2 Table 383).
    pub(crate) fn attribute(self) -> &'static str {
        match self {
            Numbering::Bullet('•' | '●' | '·' | '\u{F0B7}') => "Disc",
            Numbering::Bullet('○' | '◦') => "Circle",
            Numbering::Bullet('■' | '▪' | '□' | '▫' | '\u{F0A7}') => "Square",
            Numbering::Bullet(_) => "None",
            Numbering::Decimal => "Decimal",
            Numbering::LowerAlpha => "LowerAlpha",
            Numbering::UpperAlpha => "UpperAlpha",
            Numbering::LowerRoman => "LowerRoman",
            Numbering::UpperRoman => "UpperRoman",
        }
    }

    fn ordered(self) -> bool {
        !matches!(self, Numbering::Bullet(_))
    }
}

/// The punctuation around an ordered marker: `1.`, `1)` or `(1)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Punctuation {
    None,
    Period,
    Paren,
    Enclosed,
}

/// The list marker a text object starts with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Marker {
    pub numbering: Numbering,
    pub punctuation: Punctuation,
    /// The ordinal: 1 for `1.`, `a)` and `(i)`; 0 for a bullet.
    pub value: u32,
    /// The other reading of a letter that is both alphabetic and roman (`i`, `v`, `x`, `l`, `c`):
    /// the list it is in decides.
    pub alternative: Option<(Numbering, u32)>,
    /// The marker is all the object shows.
    pub alone: bool,
}

const BULLETS: &[char] =
    &['•', '◦', '▪', '▫', '‣', '⁃', '●', '○', '■', '□', '►', '▶', '➢', '✓', '✔', '–', '—', '-', '*', '·', '\u{F0B7}', '\u{F0A7}'];

/// The marker `text` starts with, if any.
pub(crate) fn marker(text: &str) -> Option<Marker> {
    let text = text.trim_start();
    let first = text.chars().next()?;
    if BULLETS.contains(&first) {
        let rest = &text[first.len_utf8()..];
        return ends_marker(rest).then(|| Marker {
            numbering: Numbering::Bullet(first),
            punctuation: Punctuation::None,
            value: 0,
            alternative: None,
            alone: rest.trim().is_empty(),
        });
    }
    let (enclosed, body) = match text.strip_prefix('(') {
        Some(body) => (true, body),
        None => (false, text),
    };
    let end = body.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(body.len());
    let (token, after) = body.split_at(end);
    let (punctuation, rest) = match (enclosed, after.chars().next()) {
        (true, Some(')')) => (Punctuation::Enclosed, &after[1..]),
        (false, Some(')')) => (Punctuation::Paren, &after[1..]),
        (false, Some('.')) => (Punctuation::Period, &after[1..]),
        _ => return None,
    };
    if !ends_marker(rest) {
        return None;
    }
    let (numbering, value) = ordinal(token)?;
    let alternative = match numbering {
        Numbering::LowerAlpha | Numbering::UpperAlpha => roman(token).map(|v| (roman_numbering(token), v)),
        Numbering::LowerRoman | Numbering::UpperRoman if token.len() == 1 => alphabetic(token),
        _ => None,
    };
    Some(Marker { numbering, punctuation, value, alternative, alone: rest.trim().is_empty() })
}

/// A marker is followed by white space or by nothing.
fn ends_marker(rest: &str) -> bool {
    rest.chars().next().is_none_or(char::is_whitespace)
}

/// The numbering and value of `1`, `b`, `iv` and so on. A single `i` or `I` is roman; any other
/// single letter is alphabetic.
fn ordinal(token: &str) -> Option<(Numbering, u32)> {
    if !token.is_empty() && token.len() <= MAX_DIGITS && token.bytes().all(|b| b.is_ascii_digit()) {
        return Some((Numbering::Decimal, token.parse().ok()?));
    }
    let lower = token.bytes().all(|b| b.is_ascii_lowercase());
    let upper = token.bytes().all(|b| b.is_ascii_uppercase());
    if !lower && !upper {
        return None;
    }
    if !token.eq_ignore_ascii_case("i")
        && let Some(found) = alphabetic(token)
    {
        return Some(found);
    }
    Some((roman_numbering(token), roman(token)?))
}

/// The reading of a single letter as alphabetic numbering: `a` is 1.
fn alphabetic(token: &str) -> Option<(Numbering, u32)> {
    let mut letters = token.chars();
    let (Some(c), None) = (letters.next(), letters.next()) else { return None };
    let value = u32::from(c.to_ascii_lowercase()).checked_sub(u32::from('a'))?.checked_add(1)?;
    Some((if c.is_ascii_lowercase() { Numbering::LowerAlpha } else { Numbering::UpperAlpha }, value))
}

fn roman_numbering(token: &str) -> Numbering {
    if token.bytes().all(|b| b.is_ascii_lowercase()) { Numbering::LowerRoman } else { Numbering::UpperRoman }
}

/// The value of a roman numeral written the usual way (`iv`, not `iiii`), up to [`MAX_ROMAN`]
/// letters.
fn roman(token: &str) -> Option<u32> {
    if token.len() > MAX_ROMAN {
        return None;
    }
    let digit = |c: u8| match c.to_ascii_lowercase() {
        b'i' => Some(1),
        b'v' => Some(5),
        b'x' => Some(10),
        b'l' => Some(50),
        b'c' => Some(100),
        _ => None,
    };
    let digits: Vec<i64> = token.bytes().map(digit).collect::<Option<_>>()?;
    // A digit before a larger one is subtracted (`iv`); at most six digits of 100 or less.
    let value: i64 = digits.iter().enumerate().map(|(i, d)| if digits.get(i + 1).is_some_and(|next| next > d) { -d } else { *d }).sum();
    let value = u32::try_from(value).ok().filter(|v| *v > 0)?;
    // Only the canonical spelling: `iiii` and `vx` are not numerals.
    to_roman(value).eq_ignore_ascii_case(token).then_some(value)
}

fn to_roman(mut value: u32) -> String {
    const STEPS: [(u32, &str); 9] = [(100, "c"), (90, "xc"), (50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")];
    let mut out = String::new();
    for (step, letters) in STEPS {
        while value >= step {
            out.push_str(letters);
            value -= step;
        }
    }
    out
}

/// A list item: the text object of its label, if drawn apart, and of its body, as indexes into
/// the page's text objects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Item {
    pub label: Option<usize>,
    pub body: usize,
}

/// The page's text objects in order, each alone or grouped into a list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Block {
    Text(usize),
    List { numbering: Numbering, items: Vec<Item> },
}

/// Group a page's text objects, given each one's evidence and role, into lists and lone objects.
/// Only paragraphs become list items.
pub(crate) fn blocks(evidence: &[Evidence], roles: &[&str]) -> Vec<Block> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < evidence.len() {
        let Some((_, marker, _)) = item_at(evidence, roles, i) else {
            out.push(Block::Text(i));
            i += 1;
            continue;
        };
        // A letter that reads two ways starts whichever list runs longer.
        let readings = [Some((marker.numbering, marker.value)), marker.alternative];
        let (numbering, items, used) = readings
            .into_iter()
            .flatten()
            .map(|(numbering, value)| {
                let (items, used) = run(evidence, roles, i, numbering, value);
                (numbering, items, used)
            })
            .max_by_key(|(_, items, _)| items.len())
            .unwrap_or((marker.numbering, Vec::new(), 1));
        if items.len() >= 2 {
            out.push(Block::List { numbering, items });
        } else {
            out.extend((i..i + used.max(1)).map(Block::Text));
        }
        i += used.max(1);
    }
    out
}

/// The item that starts at text object `i`, its marker and how many objects it takes.
fn item_at(evidence: &[Evidence], roles: &[&str], i: usize) -> Option<(Item, Marker, usize)> {
    let paragraph = |i: usize| roles.get(i).is_some_and(|r| *r == "P");
    let e = evidence.get(i).filter(|_| paragraph(i))?;
    let m = e.marker?;
    if !m.alone {
        return Some((Item { label: None, body: i }, m, 1));
    }
    let body = evidence.get(i + 1).filter(|_| paragraph(i + 1))?;
    let same_line = (body.y - e.y).abs() <= e.size.max(1.0) * SAME_LINE;
    let follows = body.chars > 0 && body.x > e.x && !body.marker.is_some_and(|b| b.alone);
    (same_line && follows).then_some((Item { label: Some(i), body: i + 1 }, m, 2))
}

/// The items of the list that starts at text object `i` with `numbering` and `value`, and how many
/// objects they take.
fn run(evidence: &[Evidence], roles: &[&str], i: usize, numbering: Numbering, value: u32) -> (Vec<Item>, usize) {
    let Some((first, marker, mut used)) = item_at(evidence, roles, i) else { return (Vec::new(), 1) };
    let x = evidence.get(i).map_or(0.0, |e| e.x);
    let (mut items, mut previous) = (vec![first], value);
    while let Some((item, next, n)) = item_at(evidence, roles, i + used) {
        let reading = [Some((next.numbering, next.value)), next.alternative].into_iter().flatten().find(|(kind, _)| *kind == numbering);
        let counts = |v: u32| !numbering.ordered() || v == previous.saturating_add(1);
        let continues = next.punctuation == marker.punctuation
            && evidence.get(i + used).is_some_and(|e| (e.x - x).abs() <= INDENT_TOLERANCE)
            && reading.is_some_and(|(_, v)| counts(v));
        let Some((_, v)) = reading.filter(|_| continues) else { break };
        items.push(item);
        previous = v;
        used += n;
    }
    (items, used)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_are_recognised_by_kind_punctuation_and_value() {
        let cases = [
            ("• Milk", Numbering::Bullet('•'), Punctuation::None, 0, false),
            ("–", Numbering::Bullet('–'), Punctuation::None, 0, true),
            ("12. Pears", Numbering::Decimal, Punctuation::Period, 12, false),
            ("b) Second", Numbering::LowerAlpha, Punctuation::Paren, 2, false),
            ("(iv) Fourth", Numbering::LowerRoman, Punctuation::Enclosed, 4, false),
            ("i. First", Numbering::LowerRoman, Punctuation::Period, 1, false),
            ("C.", Numbering::UpperAlpha, Punctuation::Period, 3, true),
            ("XIV)\tFourteenth", Numbering::UpperRoman, Punctuation::Paren, 14, false),
        ];
        for (text, numbering, punctuation, value, alone) in cases {
            let found = marker(text).unwrap_or_else(|| panic!("{text:?}"));
            assert_eq!((found.numbering, found.punctuation, found.value, found.alone), (numbering, punctuation, value, alone), "{text:?}");
        }
    }

    #[test]
    fn text_that_only_resembles_a_marker_is_not_one() {
        for text in ["1.5 mm", "-5 degrees", "2024. A year", "e.g. this", "Mr. Smith", "iiii. No", "vx. No", "(a. unbalanced", "Apples", ""] {
            assert_eq!(marker(text), None, "{text:?}");
        }
    }

    fn line(text: &str, x: f64, y: f64) -> Evidence {
        Evidence { size: 12.0, lines: 1, chars: text.len(), letters: true, marker: marker(text), x, y, ..Evidence::default() }
    }

    fn grouped(lines: &[Evidence]) -> Vec<Block> {
        blocks(lines, &vec!["P"; lines.len()])
    }

    fn column(markers: &[&str]) -> Vec<Evidence> {
        markers.iter().zip(0..).map(|(m, i)| line(&format!("{m} item"), 20.0, 280.0 - 15.0 * f64::from(i))).collect()
    }

    #[test]
    fn a_letter_that_is_also_a_roman_numeral_takes_the_reading_of_its_list() {
        // `(i)` after `(h)` is the ninth letter; `v.` after `iv.` is five.
        let cases: [(&[&str], Numbering); 3] = [
            (&["(g)", "(h)", "(i)", "(j)"], Numbering::LowerAlpha),
            (&["i.", "ii.", "iii.", "iv.", "v.", "vi."], Numbering::LowerRoman),
            (&["V)", "VI)"], Numbering::UpperRoman),
        ];
        for (markers, expected) in cases {
            let found = grouped(&column(markers));
            let [Block::List { numbering, items }] = found.as_slice() else { panic!("{markers:?}: {found:?}") };
            assert_eq!((*numbering, items.len()), (expected, markers.len()), "{markers:?}");
        }
    }

    #[test]
    fn a_list_ends_where_the_kind_indent_count_or_role_changes() {
        // Kind, punctuation, indent and count each split the run.
        let splits = [
            [line("1. a", 20.0, 240.0), line("b) b", 20.0, 225.0)],
            [line("1. a", 20.0, 240.0), line("2) b", 20.0, 225.0)],
            [line("• a", 20.0, 240.0), line("• b", 40.0, 225.0)],
            [line("1. a", 20.0, 240.0), line("3. b", 20.0, 225.0)],
            [line("• a", 20.0, 240.0), line("◦ b", 20.0, 225.0)],
        ];
        for pair in splits {
            assert_eq!(grouped(&pair), [Block::Text(0), Block::Text(1)], "{pair:?}");
        }
        // A heading is never a list item.
        let headed = [line("1. Scope", 20.0, 240.0), line("2. Terms", 20.0, 225.0)];
        assert_eq!(blocks(&headed, &["H1", "P"]), [Block::Text(0), Block::Text(1)]);
    }

    #[test]
    fn a_marker_alone_is_a_label_only_with_its_text_beside_it() {
        // The text is on the next line, not beside the bullet: no label, and a run of one.
        let below = [line("•", 20.0, 180.0), line("Milk", 32.0, 160.0), line("•", 20.0, 140.0), line("Bread", 32.0, 140.0)];
        let found = grouped(&below);
        assert!(found.iter().all(|b| matches!(b, Block::Text(_))), "{found:?}");
    }
}
