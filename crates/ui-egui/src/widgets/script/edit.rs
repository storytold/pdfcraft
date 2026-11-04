//! Text operations of the script editor, on plain strings so they are tested without a UI:
//! keeping the indentation on Enter, Tab and Shift+Tab on lines, and taking a level off a line when
//! its `}` is typed.
//!
//! Positions are character indices (what egui's cursors count) unless a name says bytes. Lines
//! end as JavaScript's do: at `\n`, `\r\n`, a lone `\r` (old Acrobat scripts use it), U+2028 or
//! U+2029.

use std::ops::Range;

/// Most lines looked at to find the script's indentation.
const MAX_LINES_FOR_UNIT: usize = 2000;

/// The text after an edit and the selection to put back (`start == end` for a caret).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Edit {
    pub text: String,
    pub selection: Range<usize>,
}

/// The byte where character `index` starts (the end for an index past it).
pub(super) fn byte_of(text: &str, index: usize) -> usize {
    text.char_indices().nth(index).map_or(text.len(), |(b, _)| b)
}

/// The character index of byte `byte` (rounded down to a character start).
pub(super) fn char_of(text: &str, byte: usize) -> usize {
    text.char_indices().take_while(|(b, _)| *b < byte).count()
}

/// The indentation the script uses: a tab if its first indented line starts with one, else the
/// fewest spaces any line is indented by (2 to 8), else two spaces.
pub(super) fn indent_unit(text: &str) -> String {
    let mut fewest: Option<usize> = None;
    for (line, _) in lines_with_ends(text).take(MAX_LINES_FOR_UNIT) {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with('\t') {
            if fewest.is_none() {
                return "\t".into();
            }
            continue;
        }
        let spaces = line.chars().take_while(|c| *c == ' ').count();
        if spaces >= 2 {
            fewest = Some(fewest.map_or(spaces, |f| f.min(spaces)));
        }
    }
    " ".repeat(fewest.unwrap_or(2).min(8))
}

/// Whether `c` ends a line.
pub(super) fn is_line_end(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// How many lines `text` has (`\r\n` ends one).
pub(super) fn line_count(text: &str) -> usize {
    text.chars().filter(|c| is_line_end(*c)).count().saturating_sub(text.matches("\r\n").count()).saturating_add(1)
}

/// The line end the script uses (its first), for a line the editor adds: `\n` for one with none.
pub(super) fn line_end(text: &str) -> &'static str {
    match text.find(is_line_end).and_then(|i| text.get(i..)) {
        Some(rest) if rest.starts_with("\r\n") => "\r\n",
        Some(rest) if rest.starts_with('\r') => "\r",
        _ => "\n",
    }
}

/// Whether character `index` of `text` is the `\n` of a `\r\n`, with the `\r` before it.
pub(super) fn inside_crlf(text: &str, index: usize) -> bool {
    let at = byte_of(text, index);
    index > 0 && text.get(..at).is_some_and(|b| b.ends_with('\r')) && text.get(at..).is_some_and(|a| a.starts_with('\n'))
}

/// The byte range of the line holding byte `at` (without its line end).
fn line_at(text: &str, at: usize) -> Range<usize> {
    let start = text.get(..at).and_then(|b| b.char_indices().rev().find(|(_, c)| is_line_end(*c))).map_or(0, |(i, c)| i.saturating_add(c.len_utf8()));
    let end = text.get(at..).and_then(|a| a.find(is_line_end)).map_or(text.len(), |i| at.saturating_add(i));
    start..end.max(start)
}

/// `block` cut into lines, each with the line end after it (empty for the last).
fn lines_with_ends(block: &str) -> impl Iterator<Item = (&str, &str)> {
    let mut rest = Some(block);
    std::iter::from_fn(move || {
        let text = rest?;
        let Some(at) = text.find(is_line_end) else {
            rest = None;
            return Some((text, ""));
        };
        let end = if text.get(at..).is_some_and(|r| r.starts_with("\r\n")) { 2 } else { text.get(at..)?.chars().next()?.len_utf8() };
        let after = at.saturating_add(end);
        rest = text.get(after..);
        Some((text.get(..at)?, text.get(at..after)?))
    })
}

fn leading_whitespace(line: &str) -> &str {
    let n = line.len().saturating_sub(line.trim_start_matches([' ', '\t']).len());
    line.get(..n).unwrap_or_default()
}

/// Whether `block` has a line of text that starts with white space: what the script's indentation is
/// worked out from.
pub(super) fn has_indented_line(block: &str) -> bool {
    lines_with_ends(block).any(|(line, _)| !line.trim().is_empty() && !leading_whitespace(line).is_empty())
}

/// `text` with the characters in `selection` replaced by `insert`, the caret after it.
pub(super) fn replace(text: &str, selection: Range<usize>, insert: &str) -> Edit {
    let (from, to) = (byte_of(text, selection.start), byte_of(text, selection.end.max(selection.start)));
    let mut out = String::with_capacity(text.len().saturating_add(insert.len()));
    out.push_str(text.get(..from).unwrap_or_default());
    out.push_str(insert);
    out.push_str(text.get(to..).unwrap_or_default());
    let caret = selection.start.saturating_add(insert.chars().count());
    Edit { text: out, selection: caret..caret }
}

/// Enter: a new line indented as the caret's line (no deeper than the caret, when it is in the
/// indentation), one level more after an opening bracket. Between a bracket pair (`{|}`) the
/// closing one moves to its own line below. The line ends as the script's lines do, except next to
/// a line end it would join into one, which would add no line: then it takes that line end's form.
pub(super) fn newline(text: &str, selection: Range<usize>, unit: &str) -> Edit {
    let at = byte_of(text, selection.start);
    let line = line_at(text, at);
    let current = text.get(line.clone()).unwrap_or_default();
    let lead = leading_whitespace(current);
    // Spaces and tabs are one byte each, so any cut of `lead` is on a character boundary.
    let indent = lead.get(..lead.len().min(at.saturating_sub(line.start))).unwrap_or(lead).to_string();
    let end = line_end(text);
    let before = text.get(line.start..at).unwrap_or_default().trim_end();
    let opens = before.ends_with(['{', '(', '[']);
    let after = text.get(byte_of(text, selection.end.max(selection.start))..).unwrap_or_default();
    let closes = after.trim_start_matches([' ', '\t']).starts_with(['}', ')', ']']);
    if opens {
        let inner = format!("{end}{indent}{unit}");
        if closes {
            let edit = replace(text, selection, &format!("{inner}{end}{indent}"));
            let caret = edit.selection.start.saturating_sub(indent.chars().count().saturating_add(end.chars().count()));
            return Edit { selection: caret..caret, ..edit };
        }
        return replace(text, selection, &inner);
    }
    let after_cr = text.get(..at).is_some_and(|b| b.ends_with('\r'));
    let before_lf = indent.is_empty() && after.starts_with('\n');
    let end = match end {
        "\r" if before_lf => "\n",
        "\n" if after_cr && !before_lf => "\r",
        end => end,
    };
    replace(text, selection, &format!("{end}{indent}"))
}

/// The character range of the lines `selection` covers. A selection that ends at the start of
/// a line does not take that line.
fn lines_of(text: &str, selection: &Range<usize>) -> Range<usize> {
    let (from, mut to) = (byte_of(text, selection.start), byte_of(text, selection.end.max(selection.start)));
    if to > from
        && let Some(before) = text.get(..to)
    {
        let end = if before.ends_with("\r\n") { 2 } else { before.chars().next_back().filter(|c| is_line_end(*c)).map_or(0, char::len_utf8) };
        // Never back past the selection's start (a selection of only the `\n` of a `\r\n`).
        to = to.saturating_sub(end).max(from);
    }
    let first = line_at(text, from);
    let last = line_at(text, to);
    char_of(text, first.start)..char_of(text, last.end)
}

/// Whether Tab should indent lines rather than type over the selection: it takes in a line end,
/// or a whole line.
pub(super) fn indents_lines(text: &str, selection: &Range<usize>) -> bool {
    let (from, to) = (byte_of(text, selection.start), byte_of(text, selection.end.max(selection.start)));
    let Some(selected) = text.get(from..to).filter(|s| !s.is_empty()) else { return false };
    let line = line_at(text, from);
    selected.contains(is_line_end) || (from == line.start && to == line.end)
}

/// Tab: indents the lines of a selection that takes them in ([`indents_lines`]), and types `unit`
/// over anything else.
pub(super) fn tab(text: &str, selection: Range<usize>, unit: &str) -> Edit {
    if indents_lines(text, &selection) { indent_lines(text, selection, false, unit) } else { replace(text, selection, unit) }
}

/// Shift+Tab: gives back what [`tab`] did. A caret right after one `unit`, where Tab leaves it, loses
/// that one, in a line's text too. Otherwise the lines the selection covers lose one each.
pub(super) fn shift_tab(text: &str, selection: Range<usize>, unit: &str) -> Edit {
    let caret = selection.start;
    if selection.is_empty() && !unit.is_empty() && text.get(..byte_of(text, caret)).is_some_and(|b| b.ends_with(unit)) {
        return replace(text, caret.saturating_sub(unit.chars().count())..caret, "");
    }
    indent_lines(text, selection, true, unit)
}

/// Tab (or Shift+Tab, `outdent`) on the lines the selection covers: each non-empty line gains
/// (or loses) one `unit`. The selection keeps covering the same text, less any indentation removed.
pub(super) fn indent_lines(text: &str, selection: Range<usize>, outdent: bool, unit: &str) -> Edit {
    let lines = lines_of(text, &selection);
    let (from, to) = (byte_of(text, lines.start), byte_of(text, lines.end));
    let (from, to) = (from.min(to), to.max(from));
    let block = text.get(from..to).unwrap_or_default();
    let mut out = String::with_capacity(block.len().saturating_add(unit.len().saturating_mul(16)));
    // Characters added (positive) or removed on the first line, and in all.
    let (mut first_delta, mut total_delta, mut first) = (0isize, 0isize, true);
    // Where the line being written starts in the result, and so the last one when the loop ends.
    let (mut written, mut last_line) = (lines.start, lines.start);
    for (line, end) in lines_with_ends(block) {
        last_line = written;
        let delta: isize = if line.trim().is_empty() {
            out.push_str(line);
            0
        } else if outdent {
            let lead = leading_whitespace(line);
            let take = if lead.starts_with('\t') { 1 } else { lead.chars().take_while(|c| *c == ' ').count().min(unit.chars().count().max(1)) };
            out.push_str(line.get(take..).unwrap_or(line));
            -isize::try_from(take).unwrap_or(0)
        } else {
            out.push_str(unit);
            out.push_str(line);
            isize::try_from(unit.chars().count()).unwrap_or(0)
        };
        out.push_str(end);
        if first {
            first_delta = delta;
            first = false;
        }
        total_delta = total_delta.saturating_add(delta);
        written = written.saturating_add(line.chars().count()).saturating_add(end.chars().count()).saturating_add_signed(delta);
    }
    let mut text_out = String::with_capacity(text.len().saturating_add(out.len()));
    text_out.push_str(text.get(..from).unwrap_or_default());
    text_out.push_str(&out);
    text_out.push_str(text.get(to..).unwrap_or_default());
    let shift = |at: usize, delta: isize, floor: usize| at.saturating_add_signed(delta).max(floor);
    let start = if selection.start == lines.start { selection.start } else { shift(selection.start, first_delta, lines.start) };
    // A selection can end in the indentation its last line lost: no further back than that line's start.
    let end = shift(selection.end.max(selection.start), total_delta, start.max(last_line));
    Edit { text: text_out, selection: start..end }
}

/// Typing `}` on a line with nothing but indentation before the caret: the line loses one level.
pub(super) fn close_brace(text: &str, selection: Range<usize>, unit: &str) -> Option<Edit> {
    if selection.start != selection.end {
        return None;
    }
    let at = byte_of(text, selection.start);
    let line = line_at(text, at);
    let before = text.get(line.start..at)?;
    if before.is_empty() || !before.chars().all(|c| c == ' ' || c == '\t') {
        return None;
    }
    // One tab, or up to one unit of spaces.
    let removed = if before.ends_with('\t') { 1 } else { before.chars().rev().take_while(|c| *c == ' ').count().min(unit.chars().count()) };
    Some(replace(text, selection.start.saturating_sub(removed)..selection.start, "}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(marked: &str) -> (String, Range<usize>) {
        // `[` … `]` marks a selection, `|` a caret.
        if let Some(caret) = marked.find('|') {
            let i = marked.get(..caret).unwrap_or_default().chars().count();
            return (marked.replacen('|', "", 1), i..i);
        }
        let s = marked.find('«').expect("a selection");
        let text = marked.replacen('«', "", 1);
        let e = text.find('»').expect("its end");
        let text = text.replacen('»', "", 1);
        (text.clone(), char_of(&text, s)..char_of(&text, e))
    }

    fn show(edit: &Edit) -> String {
        let mut t = edit.text.clone();
        let (s, e) = (byte_of(&t, edit.selection.start), byte_of(&t, edit.selection.end));
        if s == e {
            t.insert(s, '|');
        } else {
            t.insert(e, '»');
            t.insert(s, '«');
        }
        t
    }

    #[test]
    fn the_unit_follows_the_script() {
        assert_eq!(indent_unit("a\n    b\n        c"), "    ");
        assert_eq!(indent_unit("a\n\tb"), "\t");
        assert_eq!(indent_unit("a\n  b"), "  ");
        assert_eq!(indent_unit("plain"), "  ");
        assert_eq!(indent_unit("a\n                          b"), "        ", "at most eight");
        assert_eq!(indent_unit("if (a) {\r\tx();\r}"), "\t", "lines ended by a lone \\r");
    }

    #[test]
    fn enter_keeps_the_indentation_and_opens_blocks() {
        let (t, s) = at("if (a) {\n  x = 1;|\n}");
        assert_eq!(show(&newline(&t, s, "  ")), "if (a) {\n  x = 1;\n  |\n}");
        let (t, s) = at("if (a) {|");
        assert_eq!(show(&newline(&t, s, "  ")), "if (a) {\n  |");
        let (t, s) = at("  f({|})");
        assert_eq!(show(&newline(&t, s, "  ")), "  f({\n    |\n  })");
        let (t, s) = at("é|");
        assert_eq!(show(&newline(&t, s, "\t")), "é\n|");
        // In the indentation: the new line gets no more of it than is before the caret.
        let (t, s) = at("|  x = 1;");
        assert_eq!(show(&newline(&t, s, "  ")), "\n|  x = 1;");
        let (t, s) = at("  |  x");
        assert_eq!(show(&newline(&t, s, "  ")), "  \n  |  x");
        // The script's own line ends.
        let (t, s) = at("a;\r  b;|");
        assert_eq!(show(&newline(&t, s, "  ")), "a;\r  b;\r  |");
        let (t, s) = at("a;\r\nif (a) {|}");
        assert_eq!(show(&newline(&t, s, "  ")), "a;\r\nif (a) {\r\n  |\r\n}");
    }

    #[test]
    fn tab_and_shift_tab_move_whole_lines() {
        let (t, s) = at("«a\nb»\n\nc");
        assert_eq!(show(&indent_lines(&t, s, false, "  ")), "«  a\n  b»\n\nc");
        let (t, s) = at("    «a\n  b\nc»");
        assert_eq!(show(&indent_lines(&t, s, true, "  ")), "  «a\nb\nc»");
        // A selection ending at the start of a line leaves that line alone.
        let (t, s) = at("«a\n»b");
        assert_eq!(show(&indent_lines(&t, s, false, "\t")), "«\ta\n»b");
        // Tab indents a selection that takes in a line end or a whole line, and types over one
        // inside a line.
        assert!(indents_lines("a\nb", &(0..3)) && indents_lines("a\nb", &(0..2)) && indents_lines("  x;", &(0..4)));
        assert!(!indents_lines("ab\n", &(0..1)) && !indents_lines("ab", &(1..1)));
        // Lines ended by a lone `\r` or U+2028 are lines too.
        let (t, s) = at("«a\rb\u{2028}c»");
        assert_eq!(show(&indent_lines(&t, s, false, "  ")), "«  a\r  b\u{2028}  c»");
        let (t, s) = at("«a\r\n»b");
        assert_eq!(show(&indent_lines(&t, s, false, "  ")), "«  a\r\n»b");
        assert_eq!((line_count("a\rb\r\nc\nd"), line_count("")), (4, 1));
    }

    #[test]
    fn a_closing_brace_takes_a_level_off_its_line() {
        let (t, s) = at("if (a) {\n    |");
        assert_eq!(close_brace(&t, s, "  ").map(|e| show(&e)).as_deref(), Some("if (a) {\n  }|"));
        let (t, s) = at("x = {|");
        assert_eq!(close_brace(&t, s, "  "), None, "not after other text");
        let (t, s) = at("\t\t|");
        assert_eq!(close_brace(&t, s, "\t").map(|e| show(&e)).as_deref(), Some("\t}|"));
    }

    #[test]
    fn enter_between_different_line_ends_adds_a_line() {
        // Before a `\n`, a `\r` would join it: the line end there is the one to use.
        let (t, s) = at("a;\rb;|\nc;");
        assert_eq!(show(&newline(&t, s, "  ")), "a;\rb;\n|\nc;");
        // And after a lone `\r`, a `\n` would.
        let (t, s) = at("a;\nb;\r|c;");
        assert_eq!(show(&newline(&t, s, "  ")), "a;\nb;\r\r|c;");
        // With indentation between them they stay two line ends, in the script's own form.
        let (t, s) = at("  a;\r  b;|\nc;");
        assert_eq!(show(&newline(&t, s, "  ")), "  a;\r  b;\r  |\nc;");
        // Over a selection between the two: the caret must not end up inside a `\r\n`.
        let (t, s) = at("a;\r«b;»\nc;");
        assert_eq!(show(&newline(&t, s, "  ")), "a;\r\n|\nc;");
        let (t, s) = at("z\na;\r«b;»\nc;");
        assert_eq!(show(&newline(&t, s, "  ")), "z\na;\r\n|\nc;");
    }

    #[test]
    fn shift_tab_keeps_a_selection_that_ends_in_the_indentation_it_removes() {
        let (t, s) = at("  «  a\n  »  b");
        assert_eq!(show(&indent_lines(&t, s, true, "    ")), "«a\n»b");
        // Ending inside the last line's text: it moves with that text.
        let (t, s) = at("  «  a\n    b»c");
        assert_eq!(show(&indent_lines(&t, s, true, "    ")), "«a\nb»c");
        // Of three lines, the end is on the last, past what the others lost.
        let (t, s) = at("  «a\n  b\n »  c");
        assert_eq!(show(&indent_lines(&t, s, true, "  ")), "«a\nb\n» c");
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(2000))]

        /// The selection's ends never inside a `\r\n`: the editor moves them off before an edit.
        #[test]
        fn enter_adds_a_line_and_keeps_the_rest((text, selection) in script_and_selection(), unit in units()) {
            let edit = newline(&text, selection.clone(), unit);
            let without = replace(&text, selection.clone(), "").text;
            proptest::prop_assert!(line_count(&edit.text) > line_count(&without), "{:?} became {:?}", text, edit.text);
            proptest::prop_assert!(!inside_crlf(&edit.text, edit.selection.start), "{:?}", edit);
            let (from, to) = (byte_of(&text, selection.start), byte_of(&text, selection.end));
            proptest::prop_assert!(edit.text.starts_with(text.get(..from).unwrap_or_default()));
            proptest::prop_assert!(edit.text.ends_with(text.get(to..).unwrap_or_default()));
        }

        /// Whatever is selected stays selected, less the white space removed.
        #[test]
        fn shift_tab_keeps_covering_the_same_text((text, selection) in script_and_selection(), unit in units()) {
            let edit = indent_lines(&text, selection.clone(), true, unit);
            let inked = |t: &str, s: &Range<usize>| -> String {
                t.chars().skip(s.start).take(s.end.saturating_sub(s.start)).filter(|c| *c != ' ' && *c != '\t').collect()
            };
            proptest::prop_assert_eq!(inked(&edit.text, &edit.selection), inked(&text, &selection), "{:?}", edit);
        }

        /// Tab over a selection inside a line types over it, which only Undo gives back, so that one
        /// is taken as a caret.
        #[test]
        fn shift_tab_gives_back_what_tab_did((text, selection) in script_and_selection(), unit in units()) {
            let selection = if indents_lines(&text, &selection) { selection } else { selection.start..selection.start };
            let after = tab(&text, selection.clone(), unit);
            let back = shift_tab(&after.text, after.selection.clone(), unit);
            proptest::prop_assert_eq!(&back.text, &text, "through {:?}", after);
            proptest::prop_assert_eq!(back.selection, selection, "through {:?}", after);
        }
    }

    /// Some script, and a selection in it whose ends are not inside a `\r\n`.
    fn script_and_selection() -> impl proptest::strategy::Strategy<Value = (String, Range<usize>)> {
        use proptest::prelude::*;
        (proptest::collection::vec(proptest::sample::select(ATOMS), 0..9), any::<proptest::sample::Index>(), any::<proptest::sample::Index>())
            .prop_map(|(parts, from, to)| {
                let text = parts.concat();
                let positions = text.chars().count() + 1;
                let snapped = |i: usize| if inside_crlf(&text, i) { i - 1 } else { i };
                let (a, b) = (snapped(from.index(positions)), snapped(to.index(positions)));
                let selection = a.min(b)..a.max(b);
                (text, selection)
            })
    }

    fn units() -> impl proptest::strategy::Strategy<Value = &'static str> {
        proptest::sample::select(&["  ", "    ", "\t"][..])
    }

    /// Pieces of scripts, for generating them: every kind of line end and of indentation.
    const ATOMS: &[&str] = &["a", "b;", "{", "}", "(", ")", " ", "  ", "    ", "\t", "\n", "\n", "\r", "\r\n", "\u{2028}", "é", "x = 1;"];
}
