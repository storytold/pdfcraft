//! CSV import and export (RFC 4180 with the usual real-world leniency). Decoding, delimiter
//! sniffing, the record parser and field quoting are `craft-csv`'s; this module turns records into
//! typed cells (with each column's date order) and writes a sheet.

use std::sync::Arc;

use gridcraft_core::date::{days_in_month_in, serial_from_ymd};
use gridcraft_core::parse::{Parsed, parse_input};
use gridcraft_core::{CellRef, DateSystem, MAX_COLS, MAX_ROWS, Value, col_to_letters};
use gridcraft_model::{Cell, NumFmt, Sheet, StyleId, Workbook};

use crate::{CsvOptions, IoError, ReadReport};

/// The largest CSV or tab-separated file written (bytes).
pub const MAX_CSV_BYTES: usize = 512 << 20;

pub use craft_csv::{decode_text, sniff_delimiter};

/// [`craft_csv::parse_records`] with a record kept to the grid's [`MAX_COLS`] fields.
fn parse_records(text: &str, delim: char, quote: char, on_record: impl FnMut(Vec<String>) -> bool) {
    craft_csv::parse_records(text, delim, quote, MAX_COLS as usize, on_record);
}

/// How many rows and bytes of a text file are read to decide each column's date order.
const DATE_SAMPLE_ROWS: usize = 200;
const DATE_SAMPLE_BYTES: usize = 1 << 20;

/// What a column's numeric dates say about the order of day and month.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DateRule {
    /// No `d/m/y` or `d-m-y` value in the sample: read as typed input does.
    NoDates,
    /// A value reads only month first (`12/31/2026`), none reads only day first.
    MonthFirst,
    /// A value reads only day first (`31/05/2026`), none reads only month first.
    DayFirst,
    /// Every date is ambiguous (`03/04/2026`): keep the input parser's month-first reading.
    Ambiguous,
    /// Values read only day first and values read only month first: the dates stay text.
    Conflict,
}

impl DateRule {
    fn of(day_first: usize, month_first: usize, ambiguous: usize) -> Self {
        match (day_first > 0, month_first > 0, ambiguous > 0) {
            (true, true, _) => Self::Conflict,
            (true, false, _) => Self::DayFirst,
            (false, true, _) => Self::MonthFirst,
            (false, false, true) => Self::Ambiguous,
            (false, false, false) => Self::NoDates,
        }
    }
}

/// The evidence a column's sample gives for its date order.
#[derive(Clone, Default)]
struct DateEvidence {
    day_first: usize,
    month_first: usize,
    ambiguous: usize,
    day_first_example: Option<String>,
    month_first_example: Option<String>,
    ambiguous_example: Option<String>,
}

/// A numeric `d/m/y` or `d-m-y` field, with the serial it has read each way and its time of day.
struct NumericDate {
    day_first: Option<f64>,
    month_first: Option<f64>,
    time: Option<f64>,
}

impl NumericDate {
    /// The field under the column's rule: a date when that reading is valid, else text.
    fn read(&self, field: &str, rule: DateRule) -> Parsed {
        let serial = match rule {
            DateRule::DayFirst => self.day_first,
            DateRule::MonthFirst | DateRule::Ambiguous => self.month_first,
            DateRule::Conflict | DateRule::NoDates => None,
        };
        match serial {
            Some(serial) => Parsed {
                value: Value::number(serial + self.time.unwrap_or(0.0)),
                format: Some(if self.time.is_some() { "m/d/yyyy h:mm" } else { "m/d/yyyy" }),
            },
            None => Parsed { value: Value::text(field), format: None },
        }
    }
}

/// A numeric date written with `/` or `-`, with the serial it has read day first and month first.
/// `None` when the field isn't such a date (including ISO `yyyy-mm-dd`, month names and dots,
/// which read one way and keep the input parser).
fn numeric_date(field: &str, sys: DateSystem) -> Option<NumericDate> {
    let text = field.trim();
    let (date, time) = split_time(text, sys);
    // Typed input reads a trailing comma after a date (`3/4/2026,`); do the same here.
    let date = date.trim_end_matches(',');
    let mut parts = date.split(['/', '-']);
    let (a, b, c) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let (a, b, c) = (a.trim(), b.trim(), c.trim());
    // ISO dates are unambiguous; the input parser reads them. A three-digit year is not a
    // date to typed input either.
    if a.len() == 4 || c.len() == 3 {
        return None;
    }
    let a = parse_digits(a)?;
    let b = parse_digits(b)?;
    let year = expand_year(c, parse_digits(c)?);
    let day_first = valid_date(sys, year, b, a);
    let month_first = valid_date(sys, year, a, b);
    if day_first.is_none() && month_first.is_none() {
        return None;
    }
    Some(NumericDate { day_first, month_first, time })
}

/// Splits a field into its date and a time of day, when the tail reads as a time. Both the first
/// and last space are tried, so `03/04/2026 10:30 AM` and `03 / 04 / 2026 10:30` both split.
fn split_time(text: &str, sys: DateSystem) -> (&str, Option<f64>) {
    for at in [text.find(' '), text.rfind(' ')].into_iter().flatten() {
        let (date, time) = text.split_at(at);
        let time = time.trim();
        // Times are short; a long tail is not one, and parsing it would be wasted work.
        if time.len() > 40 {
            continue;
        }
        if let Some(fraction) = time_of_day(time, sys) {
            return (date.trim(), Some(fraction));
        }
    }
    (text, None)
}

fn time_of_day(text: &str, sys: DateSystem) -> Option<f64> {
    match parse_input(text, sys) {
        Parsed { value: Value::Number(fraction), format: Some(code) } if code.contains('h') => Some(fraction),
        _ => None,
    }
}

fn parse_digits(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Expands a one- or two-digit year the way typed input does (`26` is 2026, `30` is 1930).
fn expand_year(text: &str, year: i64) -> i64 {
    if text.len() <= 2 { if year < 30 { 2000 + year } else { 1900 + year } } else { year }
}

/// The serial for a date the input parser accepts, or `None`.
fn valid_date(sys: DateSystem, year: i64, month: i64, day: i64) -> Option<f64> {
    if !(1..=12).contains(&month) || day < 1 || year < 1900 {
        return None;
    }
    let year = i32::try_from(year).ok()?;
    let month = u32::try_from(month).ok()?;
    if day > i64::from(days_in_month_in(sys, year, month)) {
        return None;
    }
    serial_from_ymd(sys, i64::from(year), i64::from(month), day)
}

/// The first records of the file, bounded by rows and bytes, for date-order inference.
fn sample_records(text: &str, delim: char, quote: char) -> Vec<Vec<String>> {
    let mut cut = text.len().min(DATE_SAMPLE_BYTES);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let truncated = cut < text.len();
    let sample = text.get(..cut).unwrap_or_default();
    let mut rows = Vec::new();
    parse_records(sample, delim, quote, |record| {
        rows.push(record);
        rows.len() < DATE_SAMPLE_ROWS
    });
    // A cut through the last record leaves a partial field; it says nothing reliable.
    if truncated {
        rows.pop();
    }
    rows
}

/// The date rule for each column, from the sample.
fn infer_date_rules(rows: &[Vec<String>], sys: DateSystem) -> Vec<(DateRule, DateEvidence)> {
    let width = rows.iter().map(Vec::len).max().unwrap_or(0).min(MAX_COLS as usize);
    let mut columns = vec![DateEvidence::default(); width];
    for row in rows {
        for (col, field) in row.iter().enumerate() {
            let Some(column) = columns.get_mut(col) else { continue };
            let Some(date) = numeric_date(field, sys) else { continue };
            let example = || field.trim().chars().take(40).collect::<String>();
            match (date.day_first.is_some(), date.month_first.is_some()) {
                (true, true) => {
                    column.ambiguous += 1;
                    column.ambiguous_example.get_or_insert_with(example);
                }
                (true, false) => {
                    column.day_first += 1;
                    column.day_first_example.get_or_insert_with(example);
                }
                (false, true) => {
                    column.month_first += 1;
                    column.month_first_example.get_or_insert_with(example);
                }
                (false, false) => {}
            }
        }
    }
    columns.into_iter().map(|c| (DateRule::of(c.day_first, c.month_first, c.ambiguous), c)).collect()
}

/// Warns when the sample could not choose a date order: an all-ambiguous column keeps the
/// month-first reading, and a column with both kinds keeps its dates as text.
fn date_warnings(columns: &[(DateRule, DateEvidence)], report: &mut ReadReport) {
    for (col, (rule, evidence)) in columns.iter().enumerate() {
        match rule {
            DateRule::Ambiguous => report.warn(format!(
                "column {}: every date is ambiguous, so it was read month first ({})",
                col_to_letters(col as u32),
                evidence.ambiguous_example.as_deref().unwrap_or("a date")
            )),
            DateRule::Conflict => report.warn(format!(
                "column {}: it holds both day-first and month-first dates ({} and {}), so its dates stay text",
                col_to_letters(col as u32),
                evidence.day_first_example.as_deref().unwrap_or("a day-first date"),
                evidence.month_first_example.as_deref().unwrap_or("a month-first date")
            )),
            DateRule::NoDates | DateRule::MonthFirst | DateRule::DayFirst => {}
        }
    }
}

/// Reads one field with the column's date rule; other values go through typed input.
fn read_field(field: &str, sys: DateSystem, rule: DateRule) -> Parsed {
    match numeric_date(field, sys) {
        Some(date) if rule != DateRule::NoDates => date.read(field, rule),
        _ => parse_input(field, sys),
    }
}

pub fn read_csv(bytes: &[u8], opts: &CsvOptions) -> Result<Workbook, IoError> {
    read_csv_with_report(bytes, opts).map(|(wb, _)| wb)
}

/// [`read_csv`], with the warnings an open shows: a column whose date order had to be guessed,
/// or whose dates stayed text because it holds both orders.
pub fn read_csv_with_report(bytes: &[u8], opts: &CsvOptions) -> Result<(Workbook, ReadReport), IoError> {
    let text = decode_text(bytes);
    let quote = if opts.quote == 0 { '"' } else { opts.quote as char };
    let delim = if opts.delimiter == 0 { sniff_delimiter(&text, quote) } else { opts.delimiter } as char;
    if delim == quote {
        return Err(IoError::Format("the delimiter and quote character must differ".into()));
    }
    let date_columns = if opts.parse_values { infer_date_rules(&sample_records(&text, delim, quote), opts.date_system) } else { vec![] };
    let mut report = ReadReport::default();
    date_warnings(&date_columns, &mut report);
    let mut wb = Workbook::new();
    wb.date_system = opts.date_system;
    wb.csv_delimiter = u8::try_from(delim).ok();
    let mut sheet = Sheet::new("Sheet1");
    let mut styles = std::mem::take(&mut wb.styles);
    let mut fmt_cache: Vec<(&'static str, StyleId)> = vec![];
    let mut row: u32 = 0;
    // The bounding box of the non-empty cells, which is the sheet's used range.
    let mut used: Option<(u32, u32, u32, u32)> = None;
    let mut too_many = false;
    parse_records(&text, delim, quote, |rec| {
        if row >= MAX_ROWS {
            return false;
        }
        // A blank line is an empty row.
        for (col, f) in rec.into_iter().enumerate() {
            if col as u32 >= MAX_COLS {
                break;
            }
            if f.is_empty() {
                continue;
            }
            let pos = CellRef::new(row, col as u32);
            let (r0, c0, c1) = used.map_or((row, col as u32, col as u32), |(r0, c0, _, c1)| (r0, c0.min(col as u32), c1.max(col as u32)));
            used = Some((r0, c0, row, c1));
            let cell = if opts.parse_values {
                let rule = date_columns.get(col).map_or(DateRule::NoDates, |(rule, _)| *rule);
                let p = read_field(&f, opts.date_system, rule);
                let style = match p.format {
                    Some(code) => match fmt_cache.iter().find(|(c, _)| *c == code) {
                        Some((_, id)) => *id,
                        None => {
                            let id = styles.derive(StyleId::DEFAULT, |s| s.num_fmt = NumFmt::new(code));
                            fmt_cache.push((code, id));
                            id
                        }
                    },
                    None => StyleId::DEFAULT,
                };
                Cell { value: p.value, formula: None, style, py: None }
            } else {
                Cell::value(Value::Text(Arc::from(f)))
            };
            sheet.cells.set(pos, cell);
        }
        // Stop at the first record that takes the used range past the limit, before more is stored.
        if let (Some(max), Some((r0, c0, r1, c1))) = (opts.max_used_cells, used)
            && u64::from(r1 - r0 + 1) * u64::from(c1 - c0 + 1) > max
        {
            too_many = true;
            return false;
        }
        row += 1;
        true
    });
    if too_many {
        return Err(IoError::TooLarge(format!("the CSV's used range is over the limit of {} cells", opts.max_used_cells.unwrap_or(0))));
    }
    wb.styles = styles;
    wb.sheets = vec![Arc::new(sheet)];
    Ok((wb, report))
}

/// Writes the sheet as CSV from A1 to the end of its used range, so leading empty columns and rows
/// stay as empty fields and every cell keeps its position (CRLF line ends, UTF-8 without BOM). `cell_text` gives
/// each cell's field text: the caller decides how values look (Save As CSV writes them as
/// displayed, number formats applied); quoting is done here.
///
/// The output is capped at [`MAX_CSV_BYTES`]: a cell far from A1 makes the file as big as the whole
/// rectangle between them (`A1` and `XFD1048576` are 16 GiB of commas).
/// An export that would pass the cap is refused before anything is allocated, or at the first
/// field or line end that takes it past the cap (a field is measured before it is written), with
/// [`IoError::TooLarge`]; accepted exports keep every field in its place.
pub fn write_csv(sheet: &Sheet, delimiter: u8, cell_text: impl FnMut(CellRef) -> String) -> Result<Vec<u8>, IoError> {
    write_csv_limited(sheet, delimiter, MAX_CSV_BYTES, cell_text)
}

/// [`write_csv`] with the output cap `limit` (bytes) given.
pub fn write_csv_limited(sheet: &Sheet, delimiter: u8, limit: usize, mut cell_text: impl FnMut(CellRef) -> String) -> Result<Vec<u8>, IoError> {
    let delim = if delimiter == 0 { ',' } else { delimiter as char };
    let mut out = String::new();
    let Some(r) = sheet.used_range() else { return Ok(Vec::new()) };
    let too_large =
        |what: String| IoError::TooLarge(format!("the CSV would be {what}, over the limit of {} MiB; export a smaller range", limit >> 20));
    // Every row has a delimiter between its fields and a CRLF, whatever the cells hold: a lower
    // bound on the output that needs no cell text.
    let (rows, cols) = (u64::from(r.end.row) + 1, u64::from(r.end.col) + 1);
    let per_row = (cols - 1).saturating_mul(delim.len_utf8() as u64).saturating_add(2);
    let least = rows.saturating_mul(per_row);
    if least > limit as u64 {
        return Err(too_large(format!("at least {} MiB ({rows} rows by {cols} columns)", least >> 20)));
    }
    for row in 0..=r.end.row {
        for col in 0..=r.end.col {
            if col > 0 {
                out.push(delim);
            }
            let t = cell_text(CellRef::new(row, col));
            // The exact size of the field as written (the delimiter was pushed above), so a row
            // never grows past the cap unseen.
            if out.len().saturating_add(craft_csv::field_len(&t, delim)) > limit {
                return Err(too_large(format!("more than {} MiB (stopped at row {})", limit >> 20, row + 1)));
            }
            craft_csv::push_field(&mut out, &t, delim);
        }
        out.push_str("\r\n");
        if out.len() > limit {
            return Err(too_large(format!("more than {} MiB (stopped at row {})", limit >> 20, row + 1)));
        }
    }
    Ok(out.into_bytes())
}
