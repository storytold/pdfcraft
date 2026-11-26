//! Cell values and Excel's coercion and comparison rules.

use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Excel's error values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CellError {
    Null,
    Div0,
    Value,
    Ref,
    Name,
    Num,
    NA,
    GettingData,
    Spill,
    Calc,
    /// Circular reference that could not be resolved (shown as 0 by Excel; we keep it explicit).
    Circ,
    /// A Python cell's code raised an error, or Python stopped.
    Python,
    /// A Python cell is waiting to run.
    Busy,
    /// A Python cell ran longer than the time limit.
    Timeout,
    /// Python can't run here: the workbook's content isn't enabled, or this build has no Python.
    Blocked,
}

impl CellError {
    pub const ALL: [CellError; 14] = [
        CellError::Null,
        CellError::Div0,
        CellError::Value,
        CellError::Ref,
        CellError::Name,
        CellError::Num,
        CellError::NA,
        CellError::GettingData,
        CellError::Spill,
        CellError::Calc,
        CellError::Python,
        CellError::Busy,
        CellError::Timeout,
        CellError::Blocked,
    ];
    pub fn as_str(&self) -> &'static str {
        match self {
            CellError::Null => "#NULL!",
            CellError::Div0 => "#DIV/0!",
            CellError::Value => "#VALUE!",
            CellError::Ref => "#REF!",
            CellError::Name => "#NAME?",
            CellError::Num => "#NUM!",
            CellError::NA => "#N/A",
            CellError::GettingData => "#GETTING_DATA",
            CellError::Spill => "#SPILL!",
            CellError::Calc => "#CALC!",
            CellError::Circ => "#CIRC!",
            CellError::Python => "#PYTHON!",
            CellError::Busy => "#BUSY!",
            CellError::Timeout => "#TIMEOUT!",
            CellError::Blocked => "#BLOCKED!",
        }
    }
    /// `ERROR.TYPE` number. Microsoft documents 1 to 8; 9 (#SPILL!), 11 (#BLOCKED!) and 14 (#CALC!)
    /// are the numbers Excel returns. Microsoft documents no number for #PYTHON!, #BUSY! or
    /// #TIMEOUT!: 19, 20 and 21 are GridCraft's own choice, after the numbers Excel is known to use,
    /// and may change if Excel's numbers become known.
    pub fn code(&self) -> u32 {
        match self {
            CellError::Null => 1,
            CellError::Div0 => 2,
            CellError::Value => 3,
            CellError::Ref => 4,
            CellError::Name => 5,
            CellError::Num => 6,
            CellError::NA => 7,
            CellError::GettingData => 8,
            CellError::Spill => 9,
            CellError::Calc => 14,
            CellError::Circ => 15,
            CellError::Blocked => 11,
            CellError::Python => 19,
            CellError::Busy => 20,
            CellError::Timeout => 21,
        }
    }
    pub fn parse(s: &str) -> Option<CellError> {
        let u = s.to_ascii_uppercase();
        CellError::ALL.iter().chain([CellError::Circ].iter()).find(|e| e.as_str() == u).copied()
    }
}

impl fmt::Display for CellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A 2-D array of values (row-major). Always at least 1×1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Array {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<Value>,
}

impl Array {
    pub fn new(rows: usize, cols: usize, data: Vec<Value>) -> Option<Array> {
        if rows == 0 || cols == 0 || rows.checked_mul(cols)? != data.len() {
            return None;
        }
        Some(Array { rows, cols, data })
    }
    pub fn filled(rows: usize, cols: usize, v: Value) -> Array {
        let (rows, cols) = (rows.max(1), cols.max(1));
        Array { rows, cols, data: vec![v; rows.saturating_mul(cols)] }
    }
    pub fn scalar(v: Value) -> Array {
        Array { rows: 1, cols: 1, data: vec![v] }
    }
    pub fn column(v: Vec<Value>) -> Array {
        if v.is_empty() {
            return Array::scalar(Value::Error(CellError::Calc));
        }
        Array { rows: v.len(), cols: 1, data: v }
    }
    pub fn row(v: Vec<Value>) -> Array {
        if v.is_empty() {
            return Array::scalar(Value::Error(CellError::Calc));
        }
        Array { rows: 1, cols: v.len(), data: v }
    }
    pub fn get(&self, r: usize, c: usize) -> Option<&Value> {
        if r >= self.rows || c >= self.cols {
            return None;
        }
        self.data.get(r * self.cols + c)
    }
    /// Excel's broadcasting read: a 1-row or 1-column array repeats along that axis; outside the
    /// array is `#N/A`.
    pub fn get_broadcast(&self, r: usize, c: usize) -> Value {
        let rr = if self.rows == 1 { 0 } else { r };
        let cc = if self.cols == 1 { 0 } else { c };
        self.get(rr, cc).cloned().unwrap_or(Value::Error(CellError::NA))
    }
    pub fn transpose(&self) -> Array {
        let mut data = Vec::with_capacity(self.data.len());
        for c in 0..self.cols {
            for r in 0..self.rows {
                data.push(self.get(r, c).cloned().unwrap_or(Value::Empty));
            }
        }
        Array { rows: self.cols, cols: self.rows, data }
    }
    pub fn iter(&self) -> impl Iterator<Item = &Value> {
        self.data.iter()
    }
}

/// A cell value or an intermediate formula result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "t", content = "v")]
pub enum Value {
    #[default]
    Empty,
    Number(f64),
    Text(Arc<str>),
    Bool(bool),
    Error(CellError),
    Array(Arc<Array>),
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::number(v)
    }
}
impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Text(Arc::from(v))
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Text(Arc::from(v))
    }
}
impl From<CellError> for Value {
    fn from(v: CellError) -> Self {
        Value::Error(v)
    }
}
impl From<Array> for Value {
    fn from(a: Array) -> Self {
        Value::Array(Arc::new(a))
    }
}

impl Value {
    /// A number, or `#NUM!` when it is NaN or infinite (Excel never stores those).
    pub fn number(v: f64) -> Value {
        if v.is_finite() { Value::Number(if v == 0.0 { 0.0 } else { v }) } else { Value::Error(CellError::Num) }
    }
    pub fn text(s: impl Into<Arc<str>>) -> Value {
        Value::Text(s.into())
    }
    pub fn is_empty(&self) -> bool {
        matches!(self, Value::Empty)
    }
    pub fn is_error(&self) -> bool {
        matches!(self, Value::Error(_))
    }
    pub fn is_number(&self) -> bool {
        matches!(self, Value::Number(_))
    }
    pub fn is_text(&self) -> bool {
        matches!(self, Value::Text(_))
    }
    pub fn as_error(&self) -> Option<CellError> {
        if let Value::Error(e) = self { Some(*e) } else { None }
    }
    pub fn as_f64(&self) -> Option<f64> {
        if let Value::Number(n) = self { Some(*n) } else { None }
    }
    pub fn as_text(&self) -> Option<&str> {
        if let Value::Text(t) = self { Some(t) } else { None }
    }
    /// Top-left element of an array, the value itself otherwise.
    pub fn scalar(&self) -> Value {
        match self {
            Value::Array(a) => a.data.first().cloned().unwrap_or(Value::Empty),
            v => v.clone(),
        }
    }

    /// Coercion to a number as arithmetic does it: empty = 0, TRUE = 1, numeric text parses
    /// (including dates, times, percentages and currency), other text = `#VALUE!`.
    pub fn to_number(&self) -> Result<f64, CellError> {
        self.to_number_in(crate::DateSystem::D1900)
    }
    /// [`Value::to_number`] with date text read in the given workbook date system.
    pub fn to_number_in(&self, sys: crate::DateSystem) -> Result<f64, CellError> {
        match self {
            Value::Empty => Ok(0.0),
            Value::Number(n) => Ok(*n),
            Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            Value::Text(t) => crate::parse::parse_number_text_in(t, sys).ok_or(CellError::Value),
            Value::Error(e) => Err(*e),
            Value::Array(a) => a.data.first().map(|v| v.to_number_in(sys)).unwrap_or(Ok(0.0)),
        }
    }
    /// Coercion to text as `&` does it.
    pub fn to_text(&self) -> Result<String, CellError> {
        match self {
            Value::Empty => Ok(String::new()),
            Value::Number(n) => Ok(number_to_text(*n)),
            Value::Bool(b) => Ok(if *b { "TRUE".into() } else { "FALSE".into() }),
            Value::Text(t) => Ok(t.to_string()),
            Value::Error(e) => Err(*e),
            Value::Array(a) => a.data.first().map(Value::to_text).unwrap_or(Ok(String::new())),
        }
    }
    /// Coercion to a boolean as `IF` does it.
    pub fn to_bool(&self) -> Result<bool, CellError> {
        match self {
            Value::Empty => Ok(false),
            Value::Number(n) => Ok(*n != 0.0),
            Value::Bool(b) => Ok(*b),
            Value::Text(t) => {
                if t.eq_ignore_ascii_case("TRUE") {
                    Ok(true)
                } else if t.eq_ignore_ascii_case("FALSE") {
                    Ok(false)
                } else {
                    Err(CellError::Value)
                }
            }
            Value::Error(e) => Err(*e),
            Value::Array(a) => a.data.first().map(Value::to_bool).unwrap_or(Ok(false)),
        }
    }
    /// Display text with General formatting (for logs, CSV and tests).
    pub fn display(&self) -> String {
        match self {
            Value::Error(e) => e.as_str().to_string(),
            v => v.to_text().unwrap_or_default(),
        }
    }
    /// Type rank for comparison and sorting: numbers < text < booleans < errors; empty sorts last.
    fn rank(&self) -> u8 {
        match self {
            Value::Number(_) => 0,
            Value::Text(_) => 1,
            Value::Bool(_) => 2,
            Value::Error(_) => 3,
            Value::Empty => 4,
            Value::Array(_) => 5,
        }
    }
}

/// Excel's comparison for `=`, `<`… operators. Text compares case-insensitively; empty acts as
/// 0, "" or FALSE depending on the other side. Errors are handled by the caller.
pub fn compare(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Empty, Value::Empty) => Ordering::Equal,
        (Value::Empty, Value::Number(n)) => compare_numbers(0.0, *n),
        (Value::Number(n), Value::Empty) => compare_numbers(*n, 0.0),
        (Value::Empty, Value::Text(t)) => "".cmp(&t.to_lowercase() as &str),
        (Value::Text(t), Value::Empty) => (t.to_lowercase() as String).as_str().cmp(""),
        (Value::Empty, Value::Bool(b)) => false.cmp(b),
        (Value::Bool(b), Value::Empty) => b.cmp(&false),
        (Value::Number(x), Value::Number(y)) => compare_numbers(*x, *y),
        (Value::Text(x), Value::Text(y)) => compare_text(x, y),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Error(x), Value::Error(y)) => x.code().cmp(&y.code()),
        _ => a.rank().cmp(&b.rank()),
    }
}

/// Compares numbers the way Excel does: both are rounded to 15 significant digits first, so binary
/// noise such as `0.1+0.2` versus `0.3` counts as equal.
pub fn compare_numbers(a: f64, b: f64) -> Ordering {
    fn round15(x: f64) -> f64 {
        if !x.is_finite() || x == 0.0 { x } else { format!("{x:.14e}").parse().unwrap_or(x) }
    }
    if a == b {
        return Ordering::Equal;
    }
    // Numbers further apart than the 15th digit can't round together, and rounding keeps their
    // order: skip the text round trip (this is on the sort and lookup hot path).
    if (a - b).abs() > a.abs().max(b.abs()) * 1e-13 {
        return a.partial_cmp(&b).unwrap_or(Ordering::Equal);
    }
    round15(a).partial_cmp(&round15(b)).unwrap_or(Ordering::Equal)
}

/// Case-insensitive text order in three levels, as Excel's collation sorts: first by letters and
/// digits alone (Arabic marks, tatweel and invisible format characters skipped, Arabic-Indic digits
/// read as digits); then, letter by letter, by the marks on each letter (none first, then in
/// Excel's mark order) and the digit script (ASCII, Arabic-Indic, Extended Arabic-Indic); then, to
/// break a remaining tie, by every character (Unicode lowercase). Only text that is equal at the
/// last level compares equal.
pub fn compare_text(a: &str, b: &str) -> Ordering {
    if !(a.is_ascii() && b.is_ascii()) {
        let primary = primary_chars(a).cmp(primary_chars(b));
        if primary != Ordering::Equal {
            return primary;
        }
        let secondary = secondary_weights(a).cmp(secondary_weights(b));
        if secondary != Ordering::Equal {
            return secondary;
        }
    }
    lowercase_chars(a).cmp(lowercase_chars(b))
}

fn lowercase_chars(s: &str) -> impl Iterator<Item = char> + '_ {
    s.chars().flat_map(char::to_lowercase)
}

/// Characters Excel's text order skips entirely: tatweel and invisible format characters (soft
/// hyphen, combining grapheme joiner, zero-width (non-)joiner, bidi marks, embeddings, overrides
/// and isolates, word joiner, byte order mark). The zero-width space is not one.
fn ignorable(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{0640}'
            | '\u{200C}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

/// Arabic combining marks in the order Excel sorts a letter carrying them; an unmarked letter
/// sorts before all of them. Measured in Excel 16.0 (build 20527, en-US) with one mark on a letter.
/// U+06D6 and U+06D7 sort as letters in Excel, so they are not listed.
const ARABIC_MARK_ORDER: [char; 48] = [
    '\u{06EC}', '\u{0611}', '\u{0612}', '\u{0613}', '\u{0614}', '\u{06DB}', '\u{0656}', '\u{0657}', '\u{0610}', '\u{06EB}', '\u{0615}', '\u{06D9}',
    '\u{06E4}', '\u{06E1}', '\u{06DA}', '\u{06DC}', '\u{06EA}', '\u{06E3}', '\u{06D8}', '\u{0655}', '\u{065F}', '\u{0654}', '\u{06E7}', '\u{0653}',
    '\u{06E8}', '\u{06ED}', '\u{06E2}', '\u{0658}', '\u{064B}', '\u{064C}', '\u{064D}', '\u{064E}', '\u{064F}', '\u{0650}', '\u{0652}', '\u{0651}',
    '\u{0670}', '\u{0659}', '\u{065A}', '\u{065B}', '\u{065C}', '\u{065D}', '\u{065E}', '\u{0616}', '\u{0617}', '\u{0618}', '\u{0619}', '\u{061A}',
];

/// Rank of an Arabic mark that letter order skips (its index in `ARABIC_MARK_ORDER`).
fn arabic_mark_rank(c: char) -> Option<u16> {
    ARABIC_MARK_ORDER.iter().position(|&m| m == c).and_then(|i| u16::try_from(i).ok())
}

/// Digit script of `c` (0 ASCII, 1 Arabic-Indic, 2 Extended Arabic-Indic) and its ASCII digit.
fn digit(c: char) -> Option<(u16, char)> {
    let (script, zero) = match c {
        '0'..='9' => return Some((0, c)),
        '\u{0660}'..='\u{0669}' => (1, 0x0660),
        '\u{06F0}'..='\u{06F9}' => (2, 0x06F0),
        _ => return None,
    };
    char::from_u32(u32::from(c) - zero + u32::from('0')).map(|d| (script, d))
}

/// The characters that decide text order before any tie-break.
fn primary_chars(s: &str) -> impl Iterator<Item = char> + '_ {
    lowercase_chars(s).filter_map(|c| if ignorable(c) || arabic_mark_rank(c).is_some() { None } else { Some(digit(c).map_or(c, |(_, d)| d)) })
}

/// The second level, for text whose primary characters are equal: per letter, its digit script and
/// then its marks, the letters' lists closed by 0 so that an unmarked letter sorts first. Marks
/// before the first letter form a list of their own.
fn secondary_weights(s: &str) -> impl Iterator<Item = u16> + '_ {
    lowercase_chars(s)
        .filter(|&c| !ignorable(c))
        .flat_map(|c| match arabic_mark_rank(c) {
            Some(rank) => [Some(rank.saturating_add(1)), None],
            None => [Some(0), Some(digit(c).map_or(0, |(script, _)| script))],
        })
        .flatten()
        .chain(std::iter::once(0))
}

/// Sorting order used by Sort (ascending): numbers, text, booleans, errors, then blanks last.
pub fn sort_compare(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Empty, Value::Empty) => Ordering::Equal,
        (Value::Empty, _) => Ordering::Greater,
        (_, Value::Empty) => Ordering::Less,
        (Value::Error(_), Value::Error(_)) => Ordering::Equal,
        _ => compare(a, b),
    }
}

/// Number → text the way Excel converts in `&`, CONCAT and TEXTJOIN: at most 15 significant
/// digits, rounded first; scientific from 1E+15, below 1E-9, and wherever the decimal would be
/// longer than 21 characters, not counting the sign (`0.` plus 19 decimals). Excel-verified only
/// for positive 15-digit mantissas: `0.0000692361330972509` stays decimal and
/// `1.28233995888454E-06` does not, as Excel caches them. The limit for shorter mantissas from
/// 1E-6 to 1E-9 (e.g. `0.0000099999999999999` decimal) and for negatives (sign excluded) are
/// inferred.
pub fn number_to_text(n: f64) -> String {
    if n == 0.0 || !n.is_finite() {
        return "0".into();
    }
    // Round to 15 significant digits first, then pick decimal or scientific from the rounded
    // magnitude, so 999999999999999.5 reads `1E+15`, never a 16-digit integer.
    let a = n.abs();
    let sci = format!("{:.14e}", n);
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let mag: i32 = exp.parse().unwrap_or(0);
    let sig = mant.bytes().filter(u8::is_ascii_digit).count() - mant.bytes().rev().take_while(|&b| b == b'0').count();
    // Characters after `0.` for |n| < 1: the leading zeros, then the significant digits.
    let decimals_below_one = -mag - 1 + sig as i32;
    if !(-9..15).contains(&mag) || decimals_below_one > 19 {
        return sci_text(n, 15);
    }
    // A value just below 1e15 can round up to 1e15 at 15 significant digits. Only that narrow
    // band pays for the exact formatting: it runs for every number literal a formula prints.
    if a >= 9.999_999_999_999_9e14 && format!("{:.14e}", a).parse::<f64>().is_ok_and(|r| r >= 1e15) {
        return sci_text(n, 15);
    }
    // 15 significant digits, trailing zeros trimmed.
    let decimals = (14 - mag).clamp(0, 30) as usize;
    let s = format!("{:.*}", decimals, n);
    trim_decimal(&s)
}

/// Removes trailing zeros (and a trailing point) from a decimal string.
pub fn trim_decimal(s: &str) -> String {
    if s.contains('.') {
        let t = s.trim_end_matches('0').trim_end_matches('.');
        if t == "-0" { "0".into() } else { t.to_string() }
    } else {
        s.to_string()
    }
}

/// Scientific text like `1.23456789012346E+15` with up to `sig` significant digits.
pub fn sci_text(n: f64, sig: usize) -> String {
    let s = format!("{:.*e}", sig.saturating_sub(1), n);
    let Some((mant, exp)) = s.split_once('e') else { return s };
    let mant = trim_decimal(mant);
    let e: i32 = exp.parse().unwrap_or(0);
    format!("{}E{}{:02}", mant, if e < 0 { '-' } else { '+' }, e.abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_text() {
        assert_eq!(number_to_text(1.0), "1");
        assert_eq!(number_to_text(-2.5), "-2.5");
        assert_eq!(number_to_text(0.1 + 0.2), "0.3");
        assert_eq!(number_to_text(123456789012345.0), "123456789012345");
        assert_eq!(number_to_text(1e15), "1E+15");
        assert_eq!(number_to_text(1234567890123456.0), "1.23456789012346E+15");
        assert_eq!(number_to_text(0.0001), "0.0001");
        assert_eq!(number_to_text(1e-10), "1E-10");
        assert_eq!(number_to_text(1e15 - 0.5), "1E+15");
        assert_eq!(number_to_text(1e15 - 0.1), "1E+15");
        assert_eq!(number_to_text(-(1e15 - 0.5)), "-1E+15");
        assert_eq!(number_to_text(999999999999999.0), "999999999999999");
        assert_eq!(number_to_text(9.999999999999999e-10), "0.000000001");
        assert_eq!(number_to_text(-9.999999999999999e-10), "-0.000000001");
        assert_eq!(number_to_text(1.0 / 3.0), "0.333333333333333");
    }

    /// Excel switches to scientific when the 15-digit decimal would be longer than 21 characters
    /// (`0.` plus 19 decimals). Excel-verified: IMTAN, IMSECH, IMCSCH and IMEXP results cached by
    /// Excel for Mac in the `formulas` project's test.xlsx (ENGINEERING!S53, R47, S37, V37, V39).
    #[test]
    fn number_text_small_magnitudes() {
        assert_eq!(number_to_text(6.92361330972509e-5), "0.0000692361330972509");
        assert_eq!(number_to_text(2.41377759972715e-4), "0.000241377759972715");
        assert_eq!(number_to_text(1.28233995888454e-6), "1.28233995888454E-06");
        assert_eq!(number_to_text(-4.48088972789593e-6), "-4.48088972789593E-06");
        assert_eq!(number_to_text(5.98317105631764e-7), "5.98317105631764E-07");
        assert_eq!(number_to_text(-2.99158552811404e-7), "-2.99158552811404E-07");
        // Just below powers of ten: the 21-character limit, and the rounded magnitude.
        assert_eq!(number_to_text(9.99999999999999e-5), "0.0000999999999999999");
        assert_eq!(number_to_text(9.99999999999999e-6), "9.99999999999999E-06");
        assert_eq!(number_to_text(9.9999999999999e-6), "0.0000099999999999999");
        assert_eq!(number_to_text(9.99999999999999e-9), "9.99999999999999E-09");
        assert_eq!(number_to_text(-9.99999999999999e-9), "-9.99999999999999E-09");
        assert_eq!(number_to_text(9.99999999999999e-10), "9.99999999999999E-10");
        assert_eq!(number_to_text(9.999999999999999e-6), "0.00001");
        assert_eq!(number_to_text(0.00001), "0.00001");
        assert_eq!(number_to_text(0.000001), "0.000001");
        assert_eq!(number_to_text(1.5e-9), "0.0000000015");
        // Large side: 15 significant digits, scientific from 1E+15.
        assert_eq!(number_to_text(99999999999999.9), "99999999999999.9");
        assert_eq!(number_to_text(999999999999999.4), "999999999999999");
        assert_eq!(number_to_text(9999999999999998.0), "1E+16");
        assert_eq!(number_to_text(9.99999999999999e15), "9.99999999999999E+15");
    }

    #[test]
    fn number_text_rounds_up_to_scientific_boundary() {
        assert_eq!(number_to_text(999999999999999.9), "1E+15");
        assert_eq!(number_to_text(-999999999999999.9), "-1E+15");
        assert_eq!(number_to_text(999999999999999.0), "999999999999999");
        assert_eq!(number_to_text(999999999999999.5), "1E+15");
        assert_eq!(number_to_text(999999999999999.4), "999999999999999");
        assert_eq!(number_to_text(999999999999989.9), "999999999999990");
    }

    #[test]
    fn coercion() {
        assert_eq!(Value::from("12").to_number(), Ok(12.0));
        assert_eq!(Value::from(" 1,234.5 ").to_number(), Ok(1234.5));
        assert_eq!(Value::from("50%").to_number(), Ok(0.5));
        assert_eq!(Value::from("abc").to_number(), Err(CellError::Value));
        assert_eq!(Value::Bool(true).to_number(), Ok(1.0));
        assert_eq!(Value::Empty.to_text(), Ok(String::new()));
        assert_eq!(Value::from("true").to_bool(), Ok(true));
        assert!(Value::number(f64::NAN).is_error());
    }

    #[test]
    fn comparisons() {
        assert_eq!(compare(&Value::from("abc"), &Value::from("ABC")), Ordering::Equal);
        assert_eq!(compare(&Value::Number(5.0), &Value::from("1")), Ordering::Less);
        assert_eq!(compare(&Value::from("z"), &Value::Bool(false)), Ordering::Less);
        assert_eq!(compare(&Value::Empty, &Value::Number(0.0)), Ordering::Equal);
        assert_eq!(sort_compare(&Value::Empty, &Value::Number(1.0)), Ordering::Greater);
    }

    #[test]
    fn arabic_diacritics_sort_with_their_base_letter() {
        assert_eq!(compare_text("بَاب", "بيت"), Ordering::Less);
        assert_eq!(compare_text("بيت", "بَاب"), Ordering::Greater);
        assert_eq!(compare_text("كِتاب", "كتب"), Ordering::Less);
        assert_eq!(compare_text("رحمٰن", "رحمي"), Ordering::Less);
        assert_eq!(compare(&Value::from("بَاب"), &Value::from("بيت")), Ordering::Less);
    }

    #[test]
    fn arabic_indic_digits_sort_with_digits() {
        assert_eq!(compare_text("١٢", "ب"), Ordering::Less);
        assert_eq!(compare_text("۱۲", "ب"), Ordering::Less);
        assert_eq!(compare_text("١", "a"), Ordering::Less);
        assert_eq!(compare_text("١٢", "13"), Ordering::Less);
        assert_eq!(compare_text("۱۴", "13"), Ordering::Greater);
    }

    #[test]
    fn tatweel_and_bidi_marks_do_not_move_sort_order() {
        assert_eq!(compare_text("بـاب", "بت"), Ordering::Less);
        assert_eq!(compare_text("\u{200F}apple", "banana"), Ordering::Less);
        assert_eq!(compare_text("\u{200E}zebra", "apple"), Ordering::Greater);
        assert_eq!(compare_text("\u{061C}a", "b"), Ordering::Less);
        assert_eq!(compare_text("a\u{200C}b", "ac"), Ordering::Less);
        assert_eq!(compare_text("a\u{200D}b", "ac"), Ordering::Less);
    }

    #[test]
    fn primary_ties_keep_equality_unchanged() {
        assert_eq!(compare_text("ABC", "abc"), Ordering::Equal);
        assert_eq!(compare_text("باب", "بَاب"), Ordering::Less);
        assert_eq!(compare_text("12", "١٢"), Ordering::Less);
        assert_eq!(compare_text("a", "\u{200F}a"), Ordering::Less);
        let samples = [
            "",
            "abc",
            "ABC",
            "abd",
            "ab",
            "باب",
            "بَاب",
            "بِاب",
            "بـاب",
            "بيت",
            "١٢",
            "۱۲",
            "12",
            "\u{200F}12",
            "\u{200F}a",
            "a",
            "A\u{200C}",
            "a\u{200F}",
            "b",
            "Straße",
            "STRASSE",
            "ΣΑΣ",
            "σας",
            "İ",
            "i̇",
        ];
        let lowered = |s: &str| s.chars().flat_map(char::to_lowercase).collect::<String>();
        for a in samples {
            for b in samples {
                let order = compare_text(a, b);
                assert_eq!(order.is_eq(), lowered(a) == lowered(b), "{a:?} vs {b:?}");
                assert_eq!(order, compare_text(b, a).reverse(), "{a:?} vs {b:?}");
                for c in samples {
                    if order.is_le() && compare_text(b, c).is_le() {
                        assert!(compare_text(a, c).is_le(), "{a:?} <= {b:?} <= {c:?}");
                    }
                }
            }
        }
    }

    /// Sorts `expected` from reversed and rotated starts and checks each comes back as `expected`.
    #[track_caller]
    fn assert_sorts_to(expected: &[&str]) {
        let mut reversed: Vec<&str> = expected.iter().rev().copied().collect();
        reversed.sort_by(|a, b| compare_text(a, b));
        assert_eq!(reversed, expected);
        let mut rotated: Vec<&str> = expected.to_vec();
        rotated.rotate_left(expected.len() / 2);
        rotated.sort_by(|a, b| compare_text(a, b));
        assert_eq!(rotated, expected);
    }

    // The expected orders below are Excel's, observed with Range.Sort, =SORT() and the `<`
    // operator in Excel 16.0 (build 20527, en-US). Strings Excel ranks equal are left out.

    #[test]
    fn excel_orders_a_letter_by_its_mark() {
        assert_sorts_to(&[
            "باب",
            "ب\u{0611}اب",
            "ب\u{0610}اب",
            "ب\u{0654}اب",
            "ب\u{064B}اب",
            "ب\u{064C}اب",
            "ب\u{064D}اب",
            "ب\u{064E}اب",
            "ب\u{064F}اب",
            "ب\u{0650}اب",
            "ب\u{0652}اب",
            "ب\u{0651}اب",
            "ب\u{0670}اب",
            "ب\u{065E}اب",
            "ب\u{061A}اب",
        ]);
        // Quranic marks are skipped by letter order like harakat (hamza sorts before alef).
        assert_eq!(compare_text("ب\u{0610}ء", "باب"), Ordering::Less);
        assert_eq!(compare_text("ب\u{06E1}ء", "باب"), Ordering::Less);
    }

    #[test]
    fn excel_compares_marks_letter_by_letter() {
        assert_sorts_to(&[
            "بت",
            "بت\u{064E}",
            "بت\u{064F}",
            "بت\u{0650}",
            "ب\u{064E}ت",
            "ب\u{064E}ت\u{064E}",
            "ب\u{064E}ت\u{064F}",
            "ب\u{064F}ت",
            "ب\u{064F}ت\u{064E}",
            "ب\u{0650}ت\u{0650}",
        ]);
        assert_sorts_to(&["مـحـمـد", "محم\u{0651}د", "م\u{064E}حمد", "م\u{064F}ح\u{064E}م\u{064E}\u{0651}د", "محمدي"]);
    }

    #[test]
    fn excel_weighs_marks_and_digit_scripts_by_position() {
        // A mark on an earlier letter outranks the digit script or letter after it, whatever the
        // code point of that next character.
        assert_sorts_to(&[
            "ب",
            "ب\u{064E}",
            "ب1",
            "ب١",
            "ب۱",
            "ب\u{064E}1",
            "ب\u{064E}١",
            "ب\u{064E}۱",
            "بa",
            "ب\u{064E}a",
            "با",
            "ب\u{064E}ا",
            "ب\u{067E}",
            "ب\u{064E}\u{067E}",
            "ب\u{06CC}",
            "ب\u{064E}\u{06CC}",
        ]);
        // And an earlier digit script outranks a later mark.
        assert_sorts_to(&["12", "1٢", "١2", "١٢", "1ب", "1ب\u{064E}", "1ب\u{0650}", "١ب", "١ب\u{064E}", "١ب\u{0650}", "۱ب", "۱ب\u{064E}"]);
        assert_sorts_to(&["ب1ب", "ب1ب\u{064E}", "ب١ب", "ب\u{064E}ب", "بب1", "بب١", "ب\u{064E}ب1"]);
    }

    #[test]
    fn excel_skips_invisible_format_characters() {
        for c in ['\u{00AD}', '\u{034F}', '\u{061C}', '\u{200C}', '\u{200D}', '\u{200E}', '\u{200F}', '\u{202A}', '\u{202B}']
            .into_iter()
            .chain(['\u{202C}', '\u{202D}', '\u{202E}', '\u{2060}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}', '\u{FEFF}'])
        {
            let s = format!("{c}b");
            assert_eq!(compare_text(&s, "a"), Ordering::Greater, "{s:?}");
            assert_eq!(compare_text(&s, "ba"), Ordering::Less, "{s:?}");
            assert_eq!(compare_text(&format!("ب{c}\u{064E}اب"), "باب\u{064B}"), Ordering::Greater, "{c:?}");
        }
        // Not even as a tie-break: a leading mark doesn't push text past marked or other-script forms.
        assert_sorts_to(&["\u{200F}باب", "ب\u{064B}اب", "ب\u{064E}اب", "ب\u{0670}اب"]);
        assert_sorts_to(&["\u{200F}12", "١٢", "۱۲"]);
        assert_sorts_to(&["ب\u{200F}ب\u{064E}", "\u{200F}ب\u{064E}ب"]);
    }

    #[test]
    fn errors() {
        for e in CellError::ALL {
            assert_eq!(CellError::parse(e.as_str()), Some(e));
        }
        assert_eq!(CellError::parse("#div/0!"), Some(CellError::Div0));
    }

    #[test]
    fn numbers_compare_at_15_significant_digits() {
        let num = Value::Number;
        assert_eq!(compare(&num(0.1 + 0.2), &num(0.3)), Ordering::Equal);
        assert_eq!(compare(&num(1.1 * 3.0), &num(3.3)), Ordering::Equal);
        assert_eq!(compare(&num(1.0), &num(1.0 + 1e-16)), Ordering::Equal);
        assert_ne!(compare(&num(0.1 + 0.2), &num(0.3)), Ordering::Greater);
        assert_eq!(compare(&num(1.0), &num(1.00000000000001)), Ordering::Less);
        assert_eq!(compare(&num(1.0), &num(2.0)), Ordering::Less);
    }

    #[test]
    fn arrays() -> Result<(), std::io::Error> {
        let a = Array::new(2, 2, vec![1.0.into(), 2.0.into(), 3.0.into(), 4.0.into()]).ok_or_else(|| std::io::Error::other("valid array"))?;
        assert_eq!(a.transpose().get(0, 1), Some(&Value::Number(3.0)));
        assert!(Array::new(2, 2, vec![]).is_none());
        let row = Array::row(vec![1.0.into(), 2.0.into()]);
        assert_eq!(row.get_broadcast(5, 1), Value::Number(2.0));
        Ok(())
    }
}
