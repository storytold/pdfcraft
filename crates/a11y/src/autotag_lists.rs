//! Lists among auto-tagged text objects (#7068).
//!
//! A text object starts with a list marker when its text begins with a bullet, or with a number,
//! letter or roman numeral followed by `.` or `)` or enclosed in `( )`, and then white space or
//! nothing. Two or more consecutive paragraph objects whose markers have the same kind and
//! punctuation, start at the same indent and, when ordered, count up by one, form a list: `L` ›
//! `LI` › `LBody`. An object that shows only a marker, followed by the item's text on the same
//! line, is the item's `Lbl`. A paragraph without a marker right below an item's full line, flush
//! with its marker or at a hanging indent, is the item's wrapped line and joins its `LBody` when its
//! first word would not have fitted at the end of that line. A line that stops short of its column
//! ends the item, so a paragraph after it stays a paragraph, and a list whose column no two lines
//! fill keeps its items to their own objects.
//! Anything else ends the list, and a run of one item stays a paragraph.
//!
//! A marker shown by its own operator, before the item's text in the same text object, is tagged
//! `Lbl` there (see `Evidence::label_op`). A marker in the same show as its item's text gets no `Lbl`
//! of its own: the item is still found, and the marker stays in its `LBody`. Otherwise only whole
//! text objects are grouped. These are not recognised, and end the list or stay paragraphs as they
//! were tagged before:
//! - a list drawn in one text object: an object with a later line that continues its first line's
//!   marker, at the same indent, stays a paragraph;
//! - nested lists.

use crate::autotag_roles::Evidence;

/// Markers this far apart horizontally, in points, are at different indents.
pub(crate) const INDENT_TOLERANCE: f64 = 2.0;
/// A label and its item's text are on one line when their baselines are this many text sizes
/// apart or less.
const SAME_LINE: f64 = 0.5;
/// Decimal markers longer than this are numbers, not list markers.
const MAX_DIGITS: usize = 3;
/// An item's wrapped line starts at most this many text sizes right of its marker.
const MAX_HANG: f64 = 4.0;
/// An item's wrapped line starts at most this many text sizes below the last line before it.
const MAX_LINE_STEP: f64 = 1.6;
/// A line is full, so the text after it may wrap, when it ends at least this fraction of the way
/// across its column; a column's edge is where at least two of its lines end that far.
const FULL_LINE: f64 = 0.75;
/// The width of a space between words, in text sizes, for whether the next line's first word
/// would have fitted on the line before.
const SPACE: f64 = 0.25;
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

    pub(crate) fn ordered(self) -> bool {
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

impl Marker {
    /// Whether `next` could be the marker of the item after this one's, in the same list: the
    /// same bullet, or the same punctuation and numbering (either reading of a letter) counting
    /// up by one.
    pub(crate) fn followed_by(&self, next: &Marker) -> bool {
        let readings = [Some((self.numbering, self.value)), self.alternative];
        let next_readings = [Some((next.numbering, next.value)), next.alternative];
        self.punctuation == next.punctuation
            && readings.into_iter().flatten().any(|(kind, value)| {
                next_readings
                    .into_iter()
                    .flatten()
                    .any(|(next_kind, next_value)| kind == next_kind && (!kind.ordered() || next_value == value.saturating_add(1)))
            })
    }
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Item {
    pub label: Option<usize>,
    pub body: usize,
    /// The item's further lines, drawn as text objects of their own after `body`.
    pub wrapped: Vec<usize>,
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
        return Some((Item { label: None, body: i, wrapped: Vec::new() }, m, 1));
    }
    let body = evidence.get(i + 1).filter(|_| paragraph(i + 1))?;
    let same_line = (body.y - e.y).abs() <= e.size.max(1.0) * SAME_LINE;
    let follows = body.chars > 0 && body.x > e.x && !body.marker.is_some_and(|b| b.alone);
    (same_line && follows).then_some((Item { label: Some(i), body: i + 1, wrapped: Vec::new() }, m, 2))
}

/// The text objects from `j` on that continue `item`: paragraphs without a marker, flush with the
/// marker at `x` or at a hanging indent within [`MAX_HANG`] text sizes right of it, each starting
/// within [`MAX_LINE_STEP`] lines below the line before, which must be full: ending at least
/// [`FULL_LINE`] of the way from `x` to `edge`, the right edge of the column, with no room left
/// there for the object's first word.
fn wraps_after(evidence: &[Evidence], roles: &[&str], item: &Item, x: f64, edge: Option<f64>, mut j: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let Some(edge) = edge else { return out };
    let mut above = evidence.get(item.body).copied();
    while let (Some(prev), Some(e)) = (above, evidence.get(j)) {
        let size = prev.size.max(1.0);
        let step = prev.last_y - e.y;
        let flush = (e.x - x).abs() <= INDENT_TOLERANCE;
        let hanging = e.x > x + INDENT_TOLERANCE && e.x <= x + MAX_HANG * size;
        let full = prev.end >= x + (edge - x) * FULL_LINE && edge > x;
        let fitted = e.word > 0.0 && prev.end + SPACE * size + e.word <= edge;
        let wraps = roles.get(j).is_some_and(|r| *r == "P")
            && e.marker.is_none()
            && e.chars > 0
            && (flush || hanging)
            && full
            && !fitted
            && step > 0.0
            && step <= MAX_LINE_STEP * size;
        if !wraps {
            break;
        }
        out.push(j);
        above = Some(*e);
        j += 1;
    }
    out
}

/// The right edge of the column the list at text object `i` starts in, if the first lines of two
/// of its items reach it: the furthest any line of the block reaches, where the block is the objects from `i` on
/// that start flush with the marker at `x` or at a hanging indent, each within [`MAX_LINE_STEP`]
/// lines below the one before, with what is beside them on their line. Text elsewhere on the page,
/// such as a full-width footer at the same indent, does not count. A block in which only one object
/// comes near its widest line, such as a short list with one long item, has no edge to wrap at.
/// Lines without a marker may be wrapped lines or the paragraph after the list, so they widen the
/// edge but do not vouch for it.
fn column_edge(evidence: &[Evidence], i: usize, x: f64) -> Option<f64> {
    let mut block = Vec::new();
    let mut above: Option<&Evidence> = None;
    for e in evidence.iter().skip(i) {
        let size = e.size.max(1.0);
        let beside = above.is_some_and(|prev| (prev.last_y - e.y).abs() <= prev.size.max(1.0) * SAME_LINE && e.x > prev.x);
        let indented = e.x >= x - INDENT_TOLERANCE && e.x <= x + MAX_HANG * size;
        let below = above.is_none_or(|prev| {
            let step = prev.last_y - e.y;
            step > 0.0 && step <= MAX_LINE_STEP * prev.size.max(1.0)
        });
        if !beside && !(indented && below) {
            break;
        }
        block.push((e.right, e.marker.is_some() || beside));
        above = Some(e);
    }
    let edge = block.iter().map(|&(right, _)| right).fold(x, f64::max);
    // Only the items' own first lines vouch for the edge: a paragraph after the list must not.
    let reached = block.iter().filter(|&&(right, item)| item && right >= x + (edge - x) * FULL_LINE).count();
    (edge > x && reached >= 2).then_some(edge)
}

/// The items of the list that starts at text object `i` with `numbering` and `value`, and how many
/// objects they take.
fn run(evidence: &[Evidence], roles: &[&str], i: usize, numbering: Numbering, value: u32) -> (Vec<Item>, usize) {
    let Some((mut first, marker, mut used)) = item_at(evidence, roles, i) else { return (Vec::new(), 1) };
    let x = evidence.get(i).map_or(0.0, |e| e.x);
    let edge = column_edge(evidence, i, x);
    first.wrapped = wraps_after(evidence, roles, &first, x, edge, i + used);
    used += first.wrapped.len();
    let (mut items, mut previous) = (vec![first], value);
    while let Some((mut item, next, n)) = item_at(evidence, roles, i + used) {
        let reading = [Some((next.numbering, next.value)), next.alternative].into_iter().flatten().find(|(kind, _)| *kind == numbering);
        let counts = |v: u32| !numbering.ordered() || v == previous.saturating_add(1);
        let continues = next.punctuation == marker.punctuation
            && evidence.get(i + used).is_some_and(|e| (e.x - x).abs() <= INDENT_TOLERANCE)
            && reading.is_some_and(|(_, v)| counts(v));
        let Some((_, v)) = reading.filter(|_| continues) else { break };
        used += n;
        item.wrapped = wraps_after(evidence, roles, &item, x, edge, i + used);
        used += item.wrapped.len();
        items.push(item);
        previous = v;
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

    /// One line of 12 pt text at `x`, `y`, 6 pt per character wide.
    fn line(text: &str, x: f64, y: f64) -> Evidence {
        let end = x + 6.0 * text.chars().count() as f64;
        let word = 6.0 * text.split_whitespace().next().map_or(0, |w| w.chars().count()) as f64;
        Evidence {
            size: 12.0,
            lines: 1,
            chars: text.len(),
            letters: true,
            marker: marker(text),
            x,
            y,
            end,
            right: end,
            word,
            last_y: y,
            ..Evidence::default()
        }
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
    fn a_line_below_an_items_full_line_is_its_wrapped_line_and_a_line_below_a_short_one_is_not() {
        let long = "• Print voucher and bring it to a merchant listed on";
        let page = [
            line(long, 20.0, 400.0),
            line("this voucher.", 20.0, 388.0),
            line("• Write your name and the full address on your ID in", 20.0, 376.0),
            line("the space below, at a hanging indent, with its name", 32.0, 364.0),
            line("as the person redeeming it.", 32.0, 352.0),
            line("• Bring photo ID.", 20.0, 340.0),
            line("A paragraph after the list, flush with its bullets.", 20.0, 328.0),
            // Far below, at the same indent but twice as wide: not part of the list's column.
            line(&"A full-width footer. ".repeat(5), 20.0, 60.0),
        ];
        let found = grouped(&page);
        let [Block::List { items, .. }, Block::Text(6), Block::Text(7)] = found.as_slice() else { panic!("{found:?}") };
        let wrapped: Vec<&[usize]> = items.iter().map(|i| i.wrapped.as_slice()).collect();
        assert_eq!(wrapped, [&[1][..], &[3, 4][..], &[][..]]);
        // A full line does not reach across a gap of more than a line and a half.
        let spaced = [line(long, 20.0, 400.0), line("this voucher.", 20.0, 370.0), line(long, 20.0, 358.0)];
        assert_eq!(grouped(&spaced), [Block::Text(0), Block::Text(1), Block::Text(2)]);
        // The next line is measured from an object's last line, not from its first: a paragraph
        // two lines below a three-line item is not its wrapped line.
        let below = |gap: f64| {
            let item = Evidence { lines: 3, last_y: 376.0, ..line(long, 20.0, 400.0) };
            let next = 376.0 - gap;
            grouped(&[item, line("this voucher.", 20.0, next), line(long, 20.0, next - 12.0)])
        };
        assert!(matches!(below(12.0).as_slice(), [Block::List { items, .. }] if items[0].wrapped == [1]), "{:?}", below(12.0));
        assert!(matches!(below(26.0).as_slice(), [Block::Text(0), Block::Text(1), Block::Text(2)]), "{:?}", below(26.0));
        // With bullets drawn apart, the items' text beside them makes the column.
        let apart = [
            line("•", 20.0, 352.0),
            line("Print voucher and bring it to a merchant listed on", 32.0, 352.0),
            line("•", 20.0, 340.0),
            line("Write your name and the address on your ID in the", 32.0, 340.0),
            line("space below.", 32.0, 328.0),
        ];
        let found = grouped(&apart);
        let [Block::List { items, .. }] = found.as_slice() else { panic!("{found:?}") };
        assert_eq!(items.iter().map(|i| i.wrapped.as_slice()).collect::<Vec<_>>(), [&[][..], &[4][..]]);
    }

    #[test]
    fn a_paragraph_after_a_list_stays_a_paragraph_unless_the_line_above_had_no_room_for_it() {
        let wrapped = |found: &[Block]| match found {
            [Block::List { items, .. }, ..] => items.iter().map(|i| i.wrapped.len()).sum::<usize>(),
            _ => panic!("{found:?}"),
        };
        // Bullets drawn apart from their text: the column is the items' text, not the bullets.
        let apart =
            [line("•", 20.0, 352.0), line("Milk", 32.0, 352.0), line("•", 20.0, 340.0), line("Bread", 32.0, 340.0), line("Open daily.", 20.0, 328.0)];
        // One long item in a short list does not make the column, so it is not full.
        let short = [
            line("• A", 20.0, 352.0),
            line("• B", 20.0, 340.0),
            line("• Bring photo ID and your voucher to the desk.", 20.0, 328.0),
            line("Open daily.", 20.0, 316.0),
        ];
        // Lines that fill the column, but the next line's first word would have fitted on the last.
        let room = [
            line("• Print voucher and bring it to a merchant listed on", 20.0, 352.0),
            line("• Write your name and the address on your ID in the", 20.0, 340.0),
            line("• Call ahead to book a class at the front desk", 20.0, 328.0),
            line("Open daily.", 20.0, 316.0),
        ];
        // Nor does a long paragraph after it, whose first word would not have fitted.
        let after = [
            line("• A", 20.0, 352.0),
            line("• B", 20.0, 340.0),
            line("• Bring photo ID and your voucher to the desk.", 20.0, 328.0),
            line("Unfortunately the office is closed on public holidays.", 20.0, 316.0),
        ];
        for page in [&apart[..], &short[..], &room[..], &after[..]] {
            let found = grouped(page);
            assert_eq!((wrapped(&found), found.last()), (0, Some(&Block::Text(page.len() - 1))), "{found:?}");
        }
        // A paragraph between two lists still ends the first one.
        let two = [
            line("• A", 20.0, 352.0),
            line("• B", 20.0, 340.0),
            line("• Bring photo ID and your voucher.", 20.0, 328.0),
            line("Then wait.", 20.0, 316.0),
            line("• X", 20.0, 304.0),
            line("• Y", 20.0, 292.0),
        ];
        assert!(matches!(grouped(&two).as_slice(), [Block::List { .. }, Block::Text(3), Block::List { .. }]), "{:?}", grouped(&two));
    }

    #[test]
    fn a_marker_alone_is_a_label_only_with_its_text_beside_it() {
        // The text is on the next line, not beside the bullet: no label, and a run of one.
        let below = [line("•", 20.0, 180.0), line("Milk", 32.0, 160.0), line("•", 20.0, 140.0), line("Bread", 32.0, 140.0)];
        let found = grouped(&below);
        assert!(found.iter().all(|b| matches!(b, Block::Text(_))), "{found:?}");
    }
}
