//! Cell and range addresses.
//!
//! Internally rows and columns are 0-based (`A1` = row 0, col 0). The sheet is 1,048,576 rows by
//! 16,384 columns (`XFD1048576`), like Excel.

use std::fmt;

use serde::{Deserialize, Serialize};

pub const MAX_ROWS: u32 = 1_048_576;
pub const MAX_COLS: u32 = 16_384;

/// A cell position (0-based).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct CellRef {
    pub row: u32,
    pub col: u32,
}

impl CellRef {
    pub const fn new(row: u32, col: u32) -> Self {
        CellRef { row, col }
    }
    /// `A1`-style text, e.g. `B7`.
    pub fn a1(&self) -> String {
        format!("{}{}", col_to_letters(self.col), self.row as u64 + 1)
    }
    pub fn is_valid(&self) -> bool {
        self.row < MAX_ROWS && self.col < MAX_COLS
    }
    /// Moved by `(dr, dc)`, `None` when the result leaves the sheet.
    pub fn offset(&self, dr: i64, dc: i64) -> Option<CellRef> {
        let r = (self.row as i64).checked_add(dr)?;
        let c = (self.col as i64).checked_add(dc)?;
        if (0..MAX_ROWS as i64).contains(&r) && (0..MAX_COLS as i64).contains(&c) { Some(CellRef::new(r as u32, c as u32)) } else { None }
    }
    /// Moved by `(dr, dc)` and clamped to the sheet.
    pub fn offset_clamped(&self, dr: i64, dc: i64) -> CellRef {
        let r = (self.row as i64).saturating_add(dr).clamp(0, MAX_ROWS as i64 - 1);
        let c = (self.col as i64).saturating_add(dc).clamp(0, MAX_COLS as i64 - 1);
        CellRef::new(r as u32, c as u32)
    }
    /// Parses `A1`, `$A$1`, `xfd1048576` (case-insensitive). `None` if malformed or out of range.
    pub fn parse(s: &str) -> Option<CellRef> {
        let (c, r, _, _, rest) = parse_a1_prefix(s)?;
        if !rest.is_empty() {
            return None;
        }
        Some(CellRef::new(r, c))
    }
}

impl fmt::Display for CellRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", col_to_letters(self.col), self.row as u64 + 1)
    }
}

/// Parses an A1 reference at the start of `s`: `(col, row, col_abs, row_abs, rest)`.
pub fn parse_a1_prefix(s: &str) -> Option<(u32, u32, bool, bool, &str)> {
    let b = s.as_bytes();
    let mut i = 0;
    let col_abs = b.first() == Some(&b'$');
    if col_abs {
        i += 1;
    }
    let cs = i;
    while i < b.len() && b[i].is_ascii_alphabetic() {
        i += 1;
    }
    if i == cs || i - cs > 3 {
        return None;
    }
    let col = letters_to_col(s.get(cs..i)?)?;
    let row_abs = b.get(i) == Some(&b'$');
    if row_abs {
        i += 1;
    }
    let rs = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == rs || i - rs > 7 {
        return None;
    }
    let row: u32 = s.get(rs..i)?.parse().ok()?;
    if row == 0 || row > MAX_ROWS {
        return None;
    }
    Some((col, row - 1, col_abs, row_abs, s.get(i..)?))
}

/// 0 → `A`, 25 → `Z`, 26 → `AA`, 16383 → `XFD`.
pub fn col_to_letters(col: u32) -> String {
    let mut n = col as u64 + 1;
    let mut out = Vec::with_capacity(3);
    while n > 0 {
        let rem = ((n - 1) % 26) as u8;
        out.push(b'A' + rem);
        n = (n - 1) / 26;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// `A` → 0, `xfd` → 16383. `None` for empty, non-letters or beyond `XFD`.
pub fn letters_to_col(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 3 {
        return None;
    }
    let mut n: u32 = 0;
    for ch in s.bytes() {
        if !ch.is_ascii_alphabetic() {
            return None;
        }
        n = n * 26 + (ch.to_ascii_uppercase() - b'A' + 1) as u32;
    }
    if n == 0 || n > MAX_COLS {
        return None;
    }
    Some(n - 1)
}

/// A rectangular block of cells, inclusive on both ends. Always normalised (`start <= end`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct RangeRef {
    pub start: CellRef,
    pub end: CellRef,
}

impl RangeRef {
    pub fn new(a: CellRef, b: CellRef) -> Self {
        RangeRef { start: CellRef::new(a.row.min(b.row), a.col.min(b.col)), end: CellRef::new(a.row.max(b.row), a.col.max(b.col)) }
    }
    pub fn cell(c: CellRef) -> Self {
        RangeRef { start: c, end: c }
    }
    /// Whole rows `r0..=r1`.
    pub fn rows(r0: u32, r1: u32) -> Self {
        RangeRef::new(CellRef::new(r0, 0), CellRef::new(r1, MAX_COLS - 1))
    }
    /// Whole columns `c0..=c1`.
    pub fn cols(c0: u32, c1: u32) -> Self {
        RangeRef::new(CellRef::new(0, c0), CellRef::new(MAX_ROWS - 1, c1))
    }
    pub fn all() -> Self {
        RangeRef::new(CellRef::new(0, 0), CellRef::new(MAX_ROWS - 1, MAX_COLS - 1))
    }
    pub fn height(&self) -> u32 {
        self.end.row.saturating_sub(self.start.row).saturating_add(1)
    }
    pub fn width(&self) -> u32 {
        self.end.col.saturating_sub(self.start.col).saturating_add(1)
    }
    pub fn count(&self) -> u64 {
        self.height() as u64 * self.width() as u64
    }
    pub fn is_single(&self) -> bool {
        self.start == self.end
    }
    pub fn is_full_rows(&self) -> bool {
        self.start.col == 0 && self.end.col == MAX_COLS - 1
    }
    pub fn is_full_cols(&self) -> bool {
        self.start.row == 0 && self.end.row == MAX_ROWS - 1
    }
    pub fn contains(&self, c: CellRef) -> bool {
        c.row >= self.start.row && c.row <= self.end.row && c.col >= self.start.col && c.col <= self.end.col
    }
    pub fn contains_range(&self, o: &RangeRef) -> bool {
        self.contains(o.start) && self.contains(o.end)
    }
    pub fn intersects(&self, o: &RangeRef) -> bool {
        self.start.row <= o.end.row && o.start.row <= self.end.row && self.start.col <= o.end.col && o.start.col <= self.end.col
    }
    pub fn intersection(&self, o: &RangeRef) -> Option<RangeRef> {
        if !self.intersects(o) {
            return None;
        }
        Some(RangeRef {
            start: CellRef::new(self.start.row.max(o.start.row), self.start.col.max(o.start.col)),
            end: CellRef::new(self.end.row.min(o.end.row), self.end.col.min(o.end.col)),
        })
    }
    pub fn union(&self, o: &RangeRef) -> RangeRef {
        RangeRef {
            start: CellRef::new(self.start.row.min(o.start.row), self.start.col.min(o.start.col)),
            end: CellRef::new(self.end.row.max(o.end.row), self.end.col.max(o.end.col)),
        }
    }
    /// The top-left cell of a list of areas: the lowest row and the lowest column across all of
    /// them, whatever order they are listed in. Excel writes the relative formulas of a
    /// multi-area validation or conditional format for this cell. `None` for no areas.
    pub fn top_left(ranges: &[RangeRef]) -> Option<CellRef> {
        ranges.iter().copied().reduce(|a, b| a.union(&b)).map(|r| r.start)
    }
    /// Row-major iterator over the cells. Callers must bound huge ranges themselves.
    pub fn iter(&self) -> impl Iterator<Item = CellRef> + '_ {
        let (r0, r1, c0, c1) = (self.start.row, self.end.row, self.start.col, self.end.col);
        (r0..=r1).flat_map(move |r| (c0..=c1).map(move |c| CellRef::new(r, c)))
    }
    /// `A1:B2`, `A1` for single cells, `1:3` for full rows, `A:C` for full columns.
    pub fn a1(&self) -> String {
        if self.is_full_rows() && !self.is_full_cols() {
            return format!("{}:{}", self.start.row + 1, self.end.row + 1);
        }
        if self.is_full_cols() && !self.is_full_rows() {
            return format!("{}:{}", col_to_letters(self.start.col), col_to_letters(self.end.col));
        }
        if self.is_single() { self.start.a1() } else { format!("{}:{}", self.start.a1(), self.end.a1()) }
    }
    /// Parses `A1`, `A1:B2`, `$A$1:$B$2`, `A:C`, `3:5` (no sheet prefix).
    pub fn parse(s: &str) -> Option<RangeRef> {
        let s = s.trim();
        if let Some((a, b)) = s.split_once(':') {
            if let (Some(x), Some(y)) = (CellRef::parse(a), CellRef::parse(b)) {
                return Some(RangeRef::new(x, y));
            }
            let strip = |t: &str| t.strip_prefix('$').unwrap_or(t).to_string();
            if let (Some(x), Some(y)) = (letters_to_col(&strip(a)), letters_to_col(&strip(b))) {
                return Some(RangeRef::cols(x.min(y), x.max(y)));
            }
            let row_num = |t: &str| {
                let t = t.strip_prefix('$').unwrap_or(t);
                if t.is_empty() || !t.bytes().all(|c| c.is_ascii_digit()) {
                    return None;
                }
                t.parse::<u32>().ok()
            };
            let (pa, pb) = (row_num(a)?, row_num(b)?);
            if pa == 0 || pb == 0 || pa > MAX_ROWS || pb > MAX_ROWS {
                return None;
            }
            return Some(RangeRef::rows(pa.min(pb) - 1, pa.max(pb) - 1));
        }
        CellRef::parse(s).map(RangeRef::cell)
    }
}

impl fmt::Display for RangeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.a1())
    }
}

/// Whether `name` reads as an R1C1 reference: `R`, `C`, `RC`, `R1`, `C1`, `R1C`, `RC1`, `R1C1`
/// in any case.
fn is_r1c1_token(name: &str) -> bool {
    let mut chars = name.chars().peekable();
    let mut letters = 0;
    for letter in ['r', 'c'] {
        if chars.peek().is_some_and(|c| c.to_ascii_lowercase() == letter) {
            chars.next();
            letters += 1;
            while chars.peek().is_some_and(char::is_ascii_digit) {
                chars.next();
            }
        }
    }
    letters > 0 && chars.next().is_none()
}

/// Whether Excel quotes a sheet name in a reference (`'My Sheet'!A1`): it starts with a digit or
/// `.`, holds anything but letters (any script), digits, `_` and `.`, reads as an A1 cell within
/// the sheet or as an R1C1 reference, or is a boolean.
pub fn sheet_needs_quotes(name: &str) -> bool {
    let Some(first) = name.chars().next() else {
        return true;
    };
    if first.is_ascii_digit() || first == '.' || name.chars().any(|c| !(c.is_alphanumeric() || c == '_' || c == '.')) {
        return true;
    }
    CellRef::parse(name).is_some() || is_r1c1_token(name) || name.eq_ignore_ascii_case("TRUE") || name.eq_ignore_ascii_case("FALSE")
}

/// A sheet name as it is written in a reference: quoted, with `'` doubled, when Excel quotes it.
pub fn quote_sheet(name: &str) -> String {
    if sheet_needs_quotes(name) { format!("'{}'", name.replace('\'', "''")) } else { name.to_string() }
}

/// The inverse of [`quote_sheet`]: takes the quotes off a sheet name and reads `''` as `'`.
/// Text that isn't quoted is returned as it is.
pub fn unquote_sheet(text: &str) -> String {
    match text.trim().strip_prefix('\'').and_then(|t| t.strip_suffix('\'')) {
        Some(inner) => inner.replace("''", "'"),
        None => text.trim().to_string(),
    }
}

/// `'My Sheet'!A1` → (`Some("My Sheet")`, `A1`). Splits at the last `!`, so a quoted name may
/// hold one (`'a!b'!A1`).
pub fn split_sheet(reference: &str) -> (Option<String>, &str) {
    match reference.rsplit_once('!') {
        Some((sheet, body)) => (Some(unquote_sheet(sheet)), body),
        None => (None, reference),
    }
}

/// The areas of a reference list, split at the commas between them. A comma inside a quoted
/// sheet name (`'Sales, West'!A1`) is part of the name, and a doubled quote inside the quotes
/// (`'O''Brien, Jr'!A1`) is an escaped quote, not the end of the name: a quote toggles the
/// quoted state, so a doubled one toggles it twice.
pub fn split_areas(reference: &str) -> Vec<&str> {
    let mut areas = Vec::new();
    let (mut start, mut quoted) = (0, false);
    for (i, c) in reference.char_indices() {
        match c {
            '\'' => quoted = !quoted,
            ',' if !quoted => {
                areas.push(reference.get(start..i).unwrap_or_default());
                start = i.saturating_add(1);
            }
            _ => {}
        }
    }
    areas.push(reference.get(start..).unwrap_or_default());
    areas
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_range_dimensions_saturate_instead_of_wrapping() {
        let r = RangeRef { start: CellRef::new(0, 0), end: CellRef::new(u32::MAX, u32::MAX) };
        assert_eq!((r.height(), r.width()), (u32::MAX, u32::MAX));
        assert_eq!(r.count(), u64::from(u32::MAX) * u64::from(u32::MAX));
    }

    #[test]
    fn areas_split_only_at_commas_outside_quoted_sheet_names() {
        assert_eq!(split_areas("B2:B4,D1"), ["B2:B4", "D1"]);
        assert_eq!(split_areas("'Sales, West'!$A$1"), ["'Sales, West'!$A$1"]);
        assert_eq!(split_areas("'O''Brien, Jr'!A1:B2,D4"), ["'O''Brien, Jr'!A1:B2", "D4"]);
        assert_eq!(split_areas("'Q1, Q2'!A1,'Q3, Q4'!B2"), ["'Q1, Q2'!A1", "'Q3, Q4'!B2"]);
        assert_eq!(split_areas(""), [""]);
    }

    #[test]
    fn range_parse_rejects_signed_rows() -> Result<(), std::io::Error> {
        assert!(RangeRef::parse("+1:+3").is_none());
        assert!(RangeRef::parse("-1:3").is_none());
        assert!(RangeRef::parse("$$1:3").is_none());
        assert!(RangeRef::parse(":3").is_none());
        assert!(RangeRef::parse("$$A:$$C").is_none());
        assert!(RangeRef::parse("$A:$C").is_some());
        let r = RangeRef::parse("$1:$3").ok_or_else(|| std::io::Error::other("valid row range"))?;
        assert_eq!((r.start.row, r.end.row), (0, 2));
        Ok(())
    }

    #[test]
    fn top_left_spans_every_area_in_any_order() {
        let areas: Vec<RangeRef> = ["H62:H68", "H26:H27", "H93:H95"].iter().filter_map(|a| RangeRef::parse(a)).collect();
        assert_eq!(RangeRef::top_left(&areas), CellRef::parse("H26"));
        // Lowest row and lowest column, even when they come from different areas.
        let areas: Vec<RangeRef> = ["C5:D6", "E2", "B9"].iter().filter_map(|a| RangeRef::parse(a)).collect();
        assert_eq!(RangeRef::top_left(&areas), CellRef::parse("B2"));
        assert_eq!(RangeRef::top_left(&[]), None);
    }

    #[test]
    fn sheet_quoting_follows_excel() {
        for quoted in [
            "A1",
            "XFD1",
            "AB1048576",
            "R1",
            "C1",
            "RC1",
            "R",
            "C",
            "RC",
            "R1C",
            "R1C1",
            "r2c3",
            "CR1",
            ".x",
            "TRUE",
            "false",
            "1abc",
            "My Sheet",
            "Bob's",
            "a-b",
            "",
        ] {
            assert!(sheet_needs_quotes(quoted), "{quoted:?} should be quoted");
        }
        for bare in ["Sheet1", "XFE1", "AB1048577", "Ärger", "日本", "Σ", "Sheet.1", "_x", "A1A", "Data", "RR", "CRX"] {
            assert!(!sheet_needs_quotes(bare), "{bare:?} should stay bare");
        }
    }

    #[test]
    fn quote_unquote_and_split_sheet() {
        assert_eq!(quote_sheet("Bob's"), "'Bob''s'");
        assert_eq!(quote_sheet("Data"), "Data");
        assert_eq!(unquote_sheet("'Bob''s'"), "Bob's");
        assert_eq!(unquote_sheet("Data"), "Data");
        assert_eq!(unquote_sheet("'"), "'");
        assert_eq!(split_sheet("'a!b'!A1"), (Some("a!b".to_string()), "A1"));
        assert_eq!(split_sheet("'Bob''s'!$A$1:$B$2"), (Some("Bob's".to_string()), "$A$1:$B$2"));
        assert_eq!(split_sheet("A1"), (None, "A1"));
    }

    /// #2312: device/backslash garbage in front of `!` is cosmetic at worst — `split_sheet` only
    /// splits at `!`, and every consumer matches the sheet part against workbook sheet names (it
    /// never reaches a filesystem path or a shell); the body half always stays a pure cell/range
    /// reference.
    #[test]
    fn split_sheet_leaves_device_names_inert_in_the_body() {
        for s in ["C:\\a\\file!A1", "CON!A1", "NUL!A1", "a[b]!A1"] {
            let (sheet, rest) = split_sheet(s);
            assert_eq!(CellRef::parse(rest), Some(CellRef::new(0, 0)), "{s:?}: body must stay a bare reference");
            let _ = sheet;
        }
        // A plain single-letter sheet is still split, not mistaken for a body.
        assert_eq!(split_sheet("C!A1").0, Some("C".to_string()));
    }

    #[test]
    fn letters_roundtrip() {
        for (c, s) in [(0, "A"), (25, "Z"), (26, "AA"), (51, "AZ"), (52, "BA"), (701, "ZZ"), (702, "AAA"), (16383, "XFD")] {
            assert_eq!(col_to_letters(c), s);
            assert_eq!(letters_to_col(s), Some(c));
        }
        assert_eq!(letters_to_col("XFE"), None);
        assert_eq!(letters_to_col(""), None);
        assert_eq!(letters_to_col("A1"), None);
    }

    #[test]
    fn parse_cells() {
        assert_eq!(CellRef::parse("B7"), Some(CellRef::new(6, 1)));
        assert_eq!(CellRef::parse("$b$7"), Some(CellRef::new(6, 1)));
        assert_eq!(CellRef::parse("XFD1048576"), Some(CellRef::new(MAX_ROWS - 1, MAX_COLS - 1)));
        assert_eq!(CellRef::parse("A0"), None);
        assert_eq!(CellRef::parse("A1048577"), None);
        assert_eq!(CellRef::parse("A1x"), None);
        assert_eq!(CellRef::parse(""), None);
        assert_eq!(CellRef::parse("$"), None);
    }

    #[test]
    fn ranges() -> Result<(), std::io::Error> {
        let parse = |s| RangeRef::parse(s).ok_or_else(|| std::io::Error::other("valid range"));
        let r = parse("C3:A1")?;
        assert_eq!(r.a1(), "A1:C3");
        assert_eq!(r.count(), 9);
        assert_eq!(parse("A:C")?.a1(), "A:C");
        assert_eq!(parse("3:5")?.a1(), "3:5");
        assert!(RangeRef::parse("0:5").is_none());
        let a = parse("A1:C3")?;
        let b = parse("B2:D4")?;
        assert_eq!(a.intersection(&b).ok_or_else(|| std::io::Error::other("overlap expected"))?.a1(), "B2:C3");
        assert_eq!(a.union(&b).a1(), "A1:D4");
        assert_eq!(a.iter().count(), 9);
        Ok(())
    }

    #[test]
    fn extreme_offsets() {
        let cell = CellRef::new(1, 1);
        for delta in [i64::MIN, i64::MAX] {
            assert_eq!(cell.offset(delta, 0), None);
            assert_eq!(cell.offset(0, delta), None);
            assert_eq!(cell.offset(delta, delta), None);
        }
        assert_eq!(cell.offset_clamped(i64::MAX, i64::MAX), CellRef::new(MAX_ROWS - 1, MAX_COLS - 1));
        assert_eq!(cell.offset_clamped(i64::MIN, i64::MIN), CellRef::new(0, 0));
        assert_eq!(cell.offset_clamped(i64::MAX, i64::MIN), CellRef::new(MAX_ROWS - 1, 0));
        assert_eq!(cell.offset_clamped(i64::MIN, i64::MAX), CellRef::new(0, MAX_COLS - 1));
        assert_eq!(cell.offset(2, -1), Some(CellRef::new(3, 0)));
    }

    #[test]
    fn offsets() {
        assert_eq!(CellRef::new(0, 0).offset(-1, 0), None);
        assert_eq!(CellRef::new(0, 0).offset_clamped(-5, 3), CellRef::new(0, 3));
    }
}

/// The shape of a chart's category range beside its values: a plain list, or a multi-level
/// axis (Excel: a category range two or more columns wide beside a column of values is a
/// two-level axis, year beside month; one two or more rows tall above a row of values likewise).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CategoryLevels {
    /// The levels are columns and the points run down the rows (else the levels are rows and the
    /// points run along the columns).
    pub by_columns: bool,
    /// Outer levels beyond the innermost labels (the column or row nearest the values): 1 for a
    /// two-level axis. At most 3 are kept.
    pub outer: u32,
}

/// How `categories` lays its labels out beside `values` (`None` when the values aren't a
/// reference: the categories' own shape decides). `None` for a plain list of labels.
pub fn category_levels(categories: &RangeRef, values: Option<&RangeRef>) -> Option<CategoryLevels> {
    if categories.is_full_rows() || categories.is_full_cols() {
        return None;
    }
    let (w, h) = (categories.width(), categories.height());
    let vertical = match values {
        Some(v) => v.height() >= v.width(),
        None => h >= w,
    };
    let outer = if vertical { w.saturating_sub(1) } else { h.saturating_sub(1) };
    (outer > 0).then_some(CategoryLevels { by_columns: vertical, outer: outer.min(3) })
}

#[cfg(test)]
mod category_level_tests {
    use super::*;

    #[test]
    fn a_wide_range_beside_a_column_of_values_is_a_multi_level_axis() -> Result<(), std::io::Error> {
        let r = |s| RangeRef::parse(s).ok_or_else(|| std::io::Error::other("valid test range"));
        assert_eq!(category_levels(&r("A4:B39")?, Some(&r("C4:C39")?)), Some(CategoryLevels { by_columns: true, outer: 1 }));
        assert_eq!(category_levels(&r("A4:D39")?, Some(&r("C4:C39")?)), Some(CategoryLevels { by_columns: true, outer: 3 }));
        assert_eq!(category_levels(&r("A4:F39")?, Some(&r("C4:C39")?)).map(|l| l.outer), Some(3), "capped");
        assert_eq!(category_levels(&r("A4:A39")?, Some(&r("C4:C39")?)), None, "one column: a plain list");
        // A row of values: levels are rows.
        assert_eq!(category_levels(&r("B2:M3")?, Some(&r("B4:M4")?)), Some(CategoryLevels { by_columns: false, outer: 1 }));
        assert_eq!(category_levels(&r("B3:M3")?, Some(&r("B4:M4")?)), None);
        // Literal values: the categories' own shape decides.
        assert_eq!(category_levels(&r("A1:B6")?, None), Some(CategoryLevels { by_columns: true, outer: 1 }));
        assert_eq!(category_levels(&r("A1:F2")?, None), Some(CategoryLevels { by_columns: false, outer: 1 }));
        assert_eq!(category_levels(&r("A:B")?, Some(&r("C:C")?)), None, "whole columns");
        Ok(())
    }
}
