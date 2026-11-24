//! Excel criteria (COUNTIF, SUMIFS, DCOUNT…) and wildcard matching.

use std::cmp::Ordering;

use gridcraft_core::parse::parse_number_text;
use gridcraft_core::{CellError, Value, compare_numbers, compare_text};

/// Longest text criterion COUNTIF and its relatives accept, in UTF-16 code units as Excel
/// counts them (😀 is two units); a longer one is `#VALUE!`.
const MAX_CRITERION_UTF16_UNITS: usize = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug)]
enum Operand {
    Number(f64),
    Bool(bool),
    Error(CellError),
    /// Text (lower-cased chars kept for wildcard matching).
    Text(String),
    /// A criterion cut to its first 255 UTF-16 units in the middle of a surrogate pair: the
    /// complete characters before the cut, followed (in Excel) by a lone high surrogate.
    ///
    /// No cell text holds a lone surrogate, so `=` matches nothing and `<>` everything, as
    /// Excel gives 0 for SUMIFS with a criterion of 128 emoji. For `<`, `<=`, `>` and `>=` the
    /// criterion sorts just above the kept text: a cell matches `>`/`>=` when it sorts above
    /// that text, and `<`/`<=` otherwise. This is an approximation (Excel wasn't checked) and
    /// only matters for comparison criteria over 255 units.
    CutText(String),
    /// Nothing after the operator (`"="`, `"<>"`) or an empty criterion.
    Blank,
}

/// A parsed criterion.
#[derive(Clone, Debug)]
pub(crate) struct Criterion {
    op: Op,
    operand: Operand,
    /// The criterion was a bare `""` (or empty cell text): matches blanks and empty strings.
    bare_empty: bool,
}

impl Criterion {
    /// Parses a criterion value the way COUNTIF does.
    pub(crate) fn parse(v: &Value) -> Criterion {
        match v {
            Value::Number(n) => Criterion { op: Op::Eq, operand: Operand::Number(*n), bare_empty: false },
            Value::Bool(b) => Criterion { op: Op::Eq, operand: Operand::Bool(*b), bare_empty: false },
            Value::Error(e) => Criterion { op: Op::Eq, operand: Operand::Error(*e), bare_empty: false },
            Value::Empty => Criterion { op: Op::Eq, operand: Operand::Number(0.0), bare_empty: false },
            Value::Array(a) => Criterion::parse(a.data.first().unwrap_or(&Value::Empty)),
            Value::Text(t) => Criterion::parse_text(t),
        }
    }

    /// Parses the criteria argument of COUNTIF, SUMIF, AVERAGEIF and the single-pair *IFS
    /// functions: like `parse`, but text longer than 255 UTF-16 code units is `#VALUE!`, as in
    /// Excel (checked on Excel 2016 and Microsoft 365: `"a"×254 & "😀"` is 255 characters but
    /// 256 units, and is `#VALUE!`).
    pub(crate) fn parse_arg(v: &Value) -> Result<Criterion, CellError> {
        match v {
            Value::Array(a) => Criterion::parse_arg(a.data.first().unwrap_or(&Value::Empty)),
            // A UTF-8 string has at least as many bytes as UTF-16 units, so short text skips the count.
            Value::Text(t) if t.len() > MAX_CRITERION_UTF16_UNITS && t.encode_utf16().nth(MAX_CRITERION_UTF16_UNITS).is_some() => {
                Err(CellError::Value)
            }
            other => Ok(Criterion::parse(other)),
        }
    }

    /// Parses a criteria argument of an *IFS function with two or more criteria pairs. Excel
    /// doesn't reject a longer text criterion there: it matches its first 255 UTF-16 code units
    /// (checked on Excel 2016 and Microsoft 365 with COUNTIFS, SUMIFS, AVERAGEIFS, MAXIFS and
    /// MINIFS). A cut through a surrogate pair matches nothing; see `Operand::CutText`.
    pub(crate) fn parse_arg_truncated(v: &Value) -> Criterion {
        match v {
            Value::Array(a) => Criterion::parse_arg_truncated(a.data.first().unwrap_or(&Value::Empty)),
            Value::Text(t) if t.len() > MAX_CRITERION_UTF16_UNITS => {
                let mut units = 0;
                for (i, c) in t.char_indices() {
                    if units + c.len_utf16() > MAX_CRITERION_UTF16_UNITS {
                        if units == MAX_CRITERION_UTF16_UNITS {
                            return Criterion::parse_text(&t[..i]);
                        }
                        // The cut falls between the two halves of `c`.
                        let (op, rest) = split_op(&t[..i]);
                        return Criterion { op, operand: Operand::CutText(rest.to_string()), bare_empty: false };
                    }
                    units += c.len_utf16();
                }
                Criterion::parse_text(t)
            }
            other => Criterion::parse(other),
        }
    }

    fn parse_text(t: &str) -> Criterion {
        if t.is_empty() {
            return Criterion { op: Op::Eq, operand: Operand::Blank, bare_empty: true };
        }
        let (op, rest) = split_op(t);
        let operand = if rest.is_empty() {
            Operand::Blank
        } else if let Some(n) = parse_number_text(rest) {
            Operand::Number(n)
        } else if rest.eq_ignore_ascii_case("TRUE") {
            Operand::Bool(true)
        } else if rest.eq_ignore_ascii_case("FALSE") {
            Operand::Bool(false)
        } else if let Some(e) = CellError::parse(rest) {
            Operand::Error(e)
        } else {
            Operand::Text(rest.to_string())
        };
        Criterion { op, operand, bare_empty: false }
    }

    fn matches_blank(&self, v: &Value) -> bool {
        let blank = matches!(v, Value::Empty);
        match self.op {
            Op::Eq => blank,
            Op::Ne => !blank,
            // "<" / ">" with nothing: Excel compares against "" for text cells, but an
            // empty string never matches ("<=" and ">=" count it in neither).
            _ => match v {
                Value::Text(t) if !t.is_empty() => cmp_op(self.op, compare_text(t, "")),
                _ => false,
            },
        }
    }

    fn matches_number(&self, v: &Value, n: f64) -> bool {
        let cell = match v {
            Value::Number(x) => Some(*x),
            // Numeric text matches only for equality (COUNTIF("1") counts text "1").
            Value::Text(t) if matches!(self.op, Op::Eq | Op::Ne) => parse_number_text(t),
            _ => None,
        };
        match cell {
            Some(x) => cmp_op(self.op, compare_numbers(x, n)),
            None => self.op == Op::Ne,
        }
    }

    fn matches_text(&self, v: &Value, pat: &str) -> bool {
        match self.op {
            Op::Eq | Op::Ne => {
                // Blank cells hold no text: `"*"` skips them and `"<>*"` counts them.
                let cell_text = match v {
                    Value::Text(t) => Some(t.to_string()),
                    _ => None,
                };
                let m = cell_text.is_some_and(|t| wildcard_match(pat, &t));
                if self.op == Op::Eq { m } else { !m }
            }
            _ => match v {
                Value::Text(t) => cmp_op(self.op, compare_text(t, pat)),
                _ => false,
            },
        }
    }

    fn matches_cut_text(&self, v: &Value, kept: &str) -> bool {
        match self.op {
            Op::Eq => false,
            Op::Ne => true,
            Op::Lt | Op::Le => matches!(v, Value::Text(t) if compare_text(t, kept) != Ordering::Greater),
            Op::Gt | Op::Ge => matches!(v, Value::Text(t) if compare_text(t, kept) == Ordering::Greater),
        }
    }

    /// Whether a cell value satisfies the criterion.
    pub(crate) fn matches(&self, v: &Value) -> bool {
        let v = match v {
            Value::Array(a) => a.data.first().cloned().unwrap_or(Value::Empty),
            other => other.clone(),
        };
        if self.bare_empty {
            return matches!(&v, Value::Empty) || matches!(&v, Value::Text(t) if t.is_empty());
        }
        match &self.operand {
            Operand::Blank => self.matches_blank(&v),
            Operand::Number(n) => self.matches_number(&v, *n),
            Operand::Bool(b) => match &v {
                Value::Bool(x) => cmp_op(self.op, x.cmp(b)),
                _ => self.op == Op::Ne,
            },
            Operand::Error(e) => match &v {
                Value::Error(x) => cmp_op(self.op, x.code().cmp(&e.code())),
                _ => self.op == Op::Ne,
            },
            Operand::Text(pat) => self.matches_text(&v, pat),
            Operand::CutText(kept) => self.matches_cut_text(&v, kept),
        }
    }
}

/// Splits a leading comparison operator (`=` when there is none) off a text criterion.
fn split_op(t: &str) -> (Op, &str) {
    if let Some(r) = t.strip_prefix(">=") {
        (Op::Ge, r)
    } else if let Some(r) = t.strip_prefix("<=") {
        (Op::Le, r)
    } else if let Some(r) = t.strip_prefix("<>") {
        (Op::Ne, r)
    } else if let Some(r) = t.strip_prefix('=') {
        (Op::Eq, r)
    } else if let Some(r) = t.strip_prefix('>') {
        (Op::Gt, r)
    } else if let Some(r) = t.strip_prefix('<') {
        (Op::Lt, r)
    } else {
        (Op::Eq, t)
    }
}

fn cmp_op(op: Op, o: Ordering) -> bool {
    match op {
        Op::Eq => o == Ordering::Equal,
        Op::Ne => o != Ordering::Equal,
        Op::Lt => o == Ordering::Less,
        Op::Le => o != Ordering::Greater,
        Op::Gt => o == Ordering::Greater,
        Op::Ge => o != Ordering::Less,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tok {
    Char(char),
    Any,
    Star,
}

fn tokenize(pattern: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut it = pattern.chars();
    while let Some(c) = it.next() {
        match c {
            '~' => match it.next() {
                Some(n) => out.extend(std::iter::once(lower_char(n)).map(Tok::Char)),
                None => out.push(Tok::Char('~')),
            },
            '*' => out.push(Tok::Star),
            '?' => out.push(Tok::Any),
            c => out.extend(std::iter::once(lower_char(c)).map(Tok::Char)),
        }
    }
    out
}

/// Lowercase one char to exactly one char, so positions in the lowered text match the original
/// (U+0130 `İ` folds to `i`, like `fold` in text.rs).
fn lower_char(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn lower_chars(s: &str) -> Vec<char> {
    s.chars().map(lower_char).collect()
}

/// Whether the pattern contains an unescaped wildcard.
pub(crate) fn has_wildcards(pattern: &str) -> bool {
    tokenize(pattern).iter().any(|t| !matches!(t, Tok::Char(_))) || pattern.contains('~')
}

/// Case-insensitive whole-text match with `*`, `?` and `~` escapes.
pub(crate) fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p = tokenize(pattern);
    let t = lower_chars(text);
    let mut budget = MAX_STEPS;
    match_from(&p, &t, true, &mut budget).is_some()
}

/// Steps one `match_from` call may take. An honest match against Excel's 32,767-character
/// cell ceiling costs a few hundred thousand; a hostile one (star churn) is capped instead
/// of quadratic — a call that spends more gives up and reports no match, quiet degradation
/// like the other work caps on far-past-realistic input.
const MAX_STEPS: u64 = 1 << 26;

/// Greedy-free matcher: returns the matched length of `t` when `p` matches a prefix of `t`
/// (or all of `t` when `whole`). Iterative with backtracking on the last star (linear-ish,
/// quadratic at worst). `(budget, whole)`: the budget is spent one per loop step; a call that
/// would spend more than it has gives up and reports no match — quiet degradation, as with the
/// other work caps on far-past-realistic input (an honest match against Excel's 32,767-char
/// cell ceiling costs a few hundred thousand steps).
fn match_from(p: &[Tok], t: &[char], whole: bool, budget: &mut u64) -> Option<usize> {
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    loop {
        *budget = budget.checked_sub(1)?;
        if pi == p.len() && (!whole || ti == t.len()) {
            return Some(ti);
        }
        let step = match (p.get(pi), t.get(ti)) {
            (Some(Tok::Star), _) => {
                star = Some((pi, ti));
                pi += 1;
                continue;
            }
            (Some(Tok::Any), Some(_)) => true,
            (Some(Tok::Char(c)), Some(x)) => c == x,
            _ => false,
        };
        if step {
            pi += 1;
            ti += 1;
            continue;
        }
        match star {
            Some((sp, st)) if st < t.len() => {
                star = Some((sp, st + 1));
                pi = sp + 1;
                ti = st + 1;
            }
            _ => return None,
        }
    }
}

/// SEARCH-style find: the char index (0-based) of the first position at or after `start` where
/// the wildcard pattern matches, case-insensitively.
pub(crate) fn wildcard_find(pattern: &str, text: &str, start: usize) -> Option<usize> {
    let p = tokenize(pattern);
    let t = lower_chars(text);
    // A leading run of stars never matters for a prefix match (it consumes nothing), so
    // dropping it lets the anchor below skip linearly through the text (#2382).
    let skip = p.iter().take_while(|tk| matches!(tk, Tok::Star)).count();
    let p = &p[skip.min(p.len())..];
    if p.is_empty() {
        return if start <= t.len() { Some(start) } else { None };
    }
    // The first token that consumes a character anchors the scan: a literal skips straight
    // to its occurrences, and after leading `?`s each attempt only pays for the `?` run and
    // one suffix try. Every attempt shares one budget, so the total work stays flat.
    let anys = p.iter().take_while(|tk| matches!(tk, Tok::Any)).count();
    let anchor = p.get(anys);
    let mut budget = MAX_STEPS;
    let mut i = start;
    while i < t.len() {
        let hit = match (anchor, t.get(i + anys)) {
            (Some(Tok::Char(c)), Some(rest)) => *rest == *c && match_from(p, &t[i..], false, &mut budget).is_some(),
            (Some(Tok::Char(_)), None) => break, // the anchor char no longer exists ahead
            _ => match_from(p, &t[i..], false, &mut budget).is_some(),
        };
        if hit {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Exact-or-wildcard equality used by MATCH/VLOOKUP/XLOOKUP exact modes: text with wildcards
/// matches by pattern (only when `wild`), otherwise values compare case-insensitively. Numbers
/// compare exactly, not at the 15 digits of `=` and the criteria functions (Excel 365:
/// `MATCH(0.1+0.2,{0.3},0)` is #N/A while `0.1+0.2=0.3` is TRUE).
pub(crate) fn lookup_equal(needle: &Value, hay: &Value, wild: bool) -> bool {
    match (needle, hay) {
        (Value::Text(p), Value::Text(t)) => {
            if wild {
                wildcard_match(p, t)
            } else {
                compare_text(p, t) == Ordering::Equal
            }
        }
        (Value::Number(a), Value::Number(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Error(a), Value::Error(b)) => a == b,
        (Value::Empty, Value::Empty) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(c: &str, v: Value) -> bool {
        Criterion::parse(&Value::from(c)).matches(&v)
    }

    #[test]
    fn text_criteria() {
        assert!(m(">5", Value::Number(6.0)));
        assert!(!m(">5", Value::Number(5.0)));
        assert!(m(">=5", Value::Number(5.0)));
        assert!(m("<>5", Value::from("x")));
        assert!(m("<>5", Value::Empty));
        assert!(!m(">5", Value::from("9")));
        assert!(m("5", Value::from("5")));
        assert!(m("a*", Value::from("Apple")));
        assert!(!m("a*", Value::from("banana")));
        assert!(m("?pple", Value::from("apple")));
        assert!(m("~*", Value::from("*")));
        assert!(!m("~*", Value::from("x")));
    }

    #[test]
    fn blank_criteria() {
        assert!(m("", Value::Empty));
        assert!(m("", Value::from("")));
        assert!(!m("", Value::Number(0.0)));
        assert!(m("=", Value::Empty));
        assert!(!m("=", Value::from("a")));
        assert!(m("<>", Value::from("a")));
        assert!(!m("<>", Value::Empty));
        assert!(!m("<=", Value::from("")));
        assert!(!m(">=", Value::from("")));
        // A bare "<=" or ">=" never matches an empty string (Excel 365: COUNTIF(="", "<=") is 0).
    }

    #[test]
    fn typed_criteria() {
        assert!(m("TRUE", Value::Bool(true)));
        assert!(m("<b", Value::from("apple")));
        assert!(!m("<b", Value::Number(1.0)));
        assert!(m("#N/A", Value::Error(CellError::NA)));
        assert!(Criterion::parse(&Value::Number(3.0)).matches(&Value::Number(3.0)));
        assert!(Criterion::parse(&Value::Number(0.3)).matches(&Value::Number(0.1 + 0.2)));
        assert!(!lookup_equal(&Value::Number(0.3), &Value::Number(0.1 + 0.2), false));
        assert!(lookup_equal(&Value::Number(0.0), &Value::Number(-0.0), false));
        assert!(m("<>a*", Value::from("bcd")));
        assert!(m(">1/1/2020", Value::Number(44000.0)));
        // Text patterns never match blank cells, but an empty string is text.
        assert!(!m("*", Value::Empty));
        assert!(m("*", Value::from("")));
        assert!(m("<>*", Value::Empty));
        assert!(!m("<>*", Value::from("")));
        assert!(!m("?*", Value::Empty));
        assert!(m("<>a*", Value::Empty));
    }

    #[test]
    fn wildcards() {
        assert!(wildcard_match("*", ""));
        assert!(wildcard_match("a*c", "abbbc"));
        assert!(!wildcard_match("a*c", "abbbd"));
        assert!(wildcard_match("*~?", "what?"));
        assert_eq!(wildcard_find("b?d", "abcd abd", 0), Some(1));
        assert_eq!(wildcard_find("b?d", "abxxd bcd", 0), Some(6));
        assert_eq!(wildcard_find("c*", "abcd", 0), Some(2));
        assert_eq!(wildcard_find("z", "abcd", 0), None);
        assert_eq!(wildcard_find("b", "a\u{130}bc", 0), Some(2));
        assert!(wildcard_match("i*", "\u{130}stanbul"));
        assert!(has_wildcards("a*"));
        assert!(!has_wildcards("abc"));
    }
}
