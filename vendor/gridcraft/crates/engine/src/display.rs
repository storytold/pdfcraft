//! How cells look as text: number formats applied, for the grid, clipboard, CSV and agents.

use std::collections::HashMap;
use std::sync::Mutex;

use gridcraft_core::{CellRef, RangeRef, Value};
use gridcraft_model::{Sheet, Workbook};
use gridcraft_numfmt::{Formatted, NumberFormat, format_value, format_value_full};

/// Parsed number formats are cached by code.
pub(crate) fn parsed(code: &str) -> NumberFormat {
    static CACHE: std::sync::LazyLock<Mutex<HashMap<String, NumberFormat>>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
    // A code of more bytes than 4 × the character limit can't be within it: it shows as General
    // without being hashed or scanned again for every cell that shares it.
    if code.len() > 4 * gridcraft_numfmt::MAX_FORMAT_CODE_CHARS {
        return NumberFormat::parse("General");
    }
    let lock = || CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(f) = lock().get(code) {
        return f.clone();
    }
    // Parse outside the lock, so a slow code never blocks the other threads' lookups.
    let f = NumberFormat::parse(code);
    let mut m = lock();
    if m.len() > 4096 {
        m.clear();
    }
    m.insert(code.to_string(), f.clone());
    f
}

/// The parsed format of `code`; one longer than the character limit is a General format that keeps
/// the code it was made from. The engine itself formats through `parsed`, which doesn't copy it.
pub fn number_format(code: &str) -> NumberFormat {
    if code.len() > 4 * gridcraft_numfmt::MAX_FORMAT_CODE_CHARS {
        return NumberFormat::parse(code);
    }
    parsed(code)
}

/// Formats a value with a format code.
pub fn format(v: &Value, code: &str, wb: &Workbook) -> Formatted {
    format_value(v, &parsed(code), wb.date_system)
}

/// Whether typing `t` back into a General cell would not give the text `t` again (it would parse
/// as a number, boolean, error, date or formula, or lose a leading apostrophe). Such text is shown
/// for editing with a leading `'`, as Excel does for text carrying a quote prefix, so that
/// committing the editor unchanged keeps the cell as text.
fn text_needs_quote_prefix(t: &str, sys: gridcraft_core::date::DateSystem) -> bool {
    if t.is_empty() {
        return false;
    }
    let mut chars = t.chars();
    let (first, second) = (chars.next(), chars.next());
    if matches!(first, Some('=' | '\'')) {
        return true;
    }
    if matches!(first, Some('+' | '-')) && second.is_some_and(|c| c.is_ascii_alphabetic() || c == '(') {
        return true;
    }
    !matches!(gridcraft_core::parse::parse_input(t, sys).value, Value::Text(ref x) if x.as_ref() == t)
}

/// What the cell editor and the formula bar start from. Same as `Cell::input_text`, except that a
/// constant number in a date or time format is shown as a date (as the grid shows it), not as its
/// serial. When the formatted text would not read back as the same date (for example
/// `dddd, mmmm d`), an ISO `yyyy-mm-dd` (with `hh:mm:ss` for a time of day; just `hh:mm:ss`
/// for a time alone) is used instead, and if even that would not read back, the serial.
pub fn edit_text(wb: &Workbook, sheet: &Sheet, c: CellRef) -> String {
    let raw = sheet.input_text(c);
    let Some(cell) = sheet.cell(c) else { return raw };
    if let Value::Text(t) = &cell.value
        && cell.formula.is_none()
        && wb.styles.get(cell.style).num_fmt.as_str() != "@"
        && text_needs_quote_prefix(t, wb.date_system)
    {
        return format!("'{raw}");
    }
    let Value::Number(n) = &cell.value else { return raw };
    let code = wb.styles.get(cell.style).num_fmt.as_str();
    if cell.formula.is_some() || !parsed(code).is_date() {
        return raw;
    }
    let reads_back = |s: &str| matches!(gridcraft_core::parse::parse_input(s, wb.date_system).value, Value::Number(m) if (m - n).abs() < 1e-9);
    let shown = format(&cell.value, code, wb).text;
    if reads_back(&shown) {
        return shown;
    }
    let Some(dt) = gridcraft_core::date::datetime_from_serial(wb.date_system, *n) else { return raw };
    let time = format!("{:02}:{:02}:{:02}", dt.hour, dt.minute, dt.second);
    let date = format!("{:04}-{:02}-{:02}", dt.year, dt.month, dt.day);
    let iso = if *n < 1.0 {
        time
    } else if n.fract() == 0.0 {
        date
    } else {
        format!("{date} {time}")
    };
    if reads_back(&iso) { iso } else { raw }
}

/// Whether `c` has a formula or Python code that must not be shown: its cell is formatted
/// Hidden on a protected sheet. A spilled cell follows its anchor.
pub fn formula_hidden(wb: &Workbook, sheet: &Sheet, c: CellRef) -> bool {
    if !sheet.is_protected() {
        return false;
    }
    let at = match sheet.cell(c) {
        Some(x) if x.formula.is_some() || x.py.is_some() => c,
        _ => match sheet.spill_ranges.iter().find(|(_, r)| r.contains(c)) {
            Some((anchor, _)) => *anchor,
            None => return false,
        },
    };
    wb.styles.get(sheet.style_id(at)).protection.hidden
}

/// The text a cell shows (General up to the 11-character cell width; the grid narrows General
/// further to fit the column).
pub fn cell_text(wb: &Workbook, sheet: &Sheet, c: CellRef) -> String {
    if sheet.show_formulas
        && let Some(f) = sheet.cell(c).and_then(|x| x.formula.as_ref())
        && !formula_hidden(wb, sheet, c)
    {
        return format!("={}", f.text);
    }
    shown_text(wb, sheet, c)
}

/// A cell's value with its number format applied, independent of column width: no `#####`
/// and no General narrowing (Save As CSV/Text writes this). Formulas give their result. A number
/// shown as General (also through a coloured or sectioned General, or a Text format) is written
/// as [`format_value_full`] writes it, never with an exponent: `&`'s 15 significant digits below
/// 1E+15 (`124412140710`, not `1.24412E+11`), the shortest round-trip digits from 1E+15 (every
/// integer below 1e16 exactly). Excel writes the cell display.
pub fn value_text(wb: &Workbook, sheet: &Sheet, c: CellRef) -> String {
    let style = wb.styles.get(sheet.style_id(c));
    with_fill_space(format_value_full(&sheet.value(c), &parsed(style.num_fmt.as_str()), wb.date_system))
}

/// A cell's value through its number format, General at the 11-character cell width.
fn shown_text(wb: &Workbook, sheet: &Sheet, c: CellRef) -> String {
    let style = wb.styles.get(sheet.style_id(c));
    value_text_as(wb, &sheet.value(c), style.num_fmt.as_str())
}

/// A value through a format code given apart from a cell, as a cell shows it (General at the
/// 11-character cell width). Print takes the code from the cell's effective look, which a
/// conditional format can override.
pub fn value_text_as(wb: &Workbook, v: &Value, code: &str) -> String {
    with_fill_space(format(v, code, wb))
}

/// The formatted text with a single space where a `*x` repeat fill would expand.
fn with_fill_space(f: Formatted) -> String {
    match f.fill {
        Some((_, pos)) => {
            let mut t = f.text;
            if pos <= t.len() && t.is_char_boundary(pos) {
                t.insert(pos, ' ');
            }
            t
        }
        None => f.text,
    }
}

/// Tab-separated text of a range, as the system clipboard gets it.
pub fn range_text(wb: &Workbook, sheet: usize, r: RangeRef) -> String {
    let Some(sh) = wb.sheet(sheet) else { return String::new() };
    let mut out = String::new();
    let r = clipboard_range(r);
    for row in r.start.row..=r.end.row {
        if sh.is_row_hidden(row) {
            continue;
        }
        let mut first = true;
        for col in r.start.col..=r.end.col {
            if !first {
                out.push('\t');
            }
            first = false;
            let t = cell_text(wb, sh, CellRef::new(row, col));
            // Like Excel, only text with a tab or line break is quoted (its quotes doubled);
            // `say "hi"` is copied as it is (#2225).
            if t.contains(['\t', '\n', '\r']) {
                out.push('"');
                out.push_str(&t.replace('"', "\"\""));
                out.push('"');
            } else {
                out.push_str(&t);
            }
        }
        out.push('\n');
    }
    out
}

/// The existing plain-text clipboard limit, shared with its HTML representation.
pub(crate) fn clipboard_range(r: RangeRef) -> RangeRef {
    if r.count() > 2_000_000 { RangeRef::new(r.start, r.start.offset_clamped(1999, 999)) } else { r }
}

/// Status-bar statistics for a selection: average, count (non-empty), numerical count, min,
/// max, sum.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Stats {
    pub count: u64,
    pub numeric: u64,
    pub sum: f64,
    pub min: f64,
    pub max: f64,
    pub average: Option<f64>,
}

pub fn stats(sheet: &Sheet, ranges: &[RangeRef]) -> Stats {
    let mut s = Stats { min: f64::INFINITY, max: f64::NEG_INFINITY, ..Default::default() };
    let mut seen = std::collections::HashSet::new();
    for r in ranges {
        let r = match sheet.used_range().and_then(|u| r.intersection(&u)) {
            Some(x) => x,
            None => continue,
        };
        let mut visit = |c: CellRef, v: &Value| {
            if !seen.insert(c) || sheet.is_row_hidden(c.row) {
                return;
            }
            if v.is_empty() {
                return;
            }
            s.count += 1;
            if let Value::Number(n) = v {
                s.numeric += 1;
                s.sum += n;
                s.min = s.min.min(*n);
                s.max = s.max.max(*n);
            }
        };
        for (c, cell) in sheet.cells.iter_range(r) {
            visit(c, &cell.value);
        }
        for (c, v) in sheet.spill.range(r.start..=r.end) {
            if r.contains(*c) {
                visit(*c, v);
            }
        }
    }
    if s.numeric > 0 {
        s.average = Some(s.sum / s.numeric as f64);
    } else {
        s.min = 0.0;
        s.max = 0.0;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A format code far past Excel's 255 characters shows its values as General, for every cell
    /// that shares it, without being hashed into the cache each time.
    #[test]
    fn an_overlong_code_formats_as_general() {
        let wb = Workbook::new();
        let code = format!("0.00{}", "\"x\"".repeat(400_000));
        for _ in 0..1_000 {
            assert_eq!(format(&Value::Number(1.5), &code, &wb).text, "1.5");
        }
        assert_eq!(format(&Value::Number(1.5), "0.00", &wb).text, "1.50");
    }

    /// The public `number_format` keeps the code of an overlong format; it is General, but not the
    /// same format as a real `General`.
    #[test]
    fn number_format_keeps_an_overlong_code() {
        let code = format!("0.00{}", "\"x\"".repeat(2_000));
        let f = number_format(&code);
        assert!(f.is_general());
        assert_eq!(f.code(), code);
        assert_ne!(f, number_format("General"));
    }
}
