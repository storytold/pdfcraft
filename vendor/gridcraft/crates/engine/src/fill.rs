//! Fill: copy, AutoFill series (numbers, dates, weekdays, months, text with numbers, custom
//! lists), Series dialog and Flash Fill.

use std::sync::Arc;

use gridcraft_core::date::{MONTHS, WEEKDAYS, datetime_from_serial, days_in_month, serial_from_ymd};
use gridcraft_core::{CellRef, RangeRef, Value};
use gridcraft_model::{Cell, Formula, Workbook};
use gridcraft_numfmt::{FormatKind, NumberFormat};

use crate::Result;
use crate::cmd::Ctx;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillMode {
    Series,
    /// The Ctrl+drag fill: the opposite of a plain drag. A lone number fills a
    /// series stepping by 1; anything a plain drag would extend is copied.
    Alternate,
    Copy,
    Formats,
    /// Fill Without Formatting: extends like a plain drag, the targets keep their formats.
    ValuesOnly,
    /// Fill Series from the right-drag menu: extends like a plain drag, and a lone number counts up.
    Extend,
    /// Fill Days, Fill Weekdays, Fill Months or Fill Years: a source of dates steps by that unit.
    /// A lone date steps by one unit; two or more step by the difference between the first two,
    /// counted in that unit (one unit when they fall in the same one). A source that isn't dates
    /// extends like a plain drag.
    Dates(DateUnit),
}

/// The step of a [`FillMode::Dates`] fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateUnit {
    Day,
    /// Monday to Friday: Saturday and Sunday are skipped.
    Weekday,
    Month,
    Year,
}

impl FillMode {
    /// The modes that continue the source's series rather than copy it.
    fn extends_series(self) -> bool {
        matches!(self, FillMode::Series | FillMode::Extend | FillMode::ValuesOnly | FillMode::Dates(_))
    }
}

/// Whether a number format shows a calendar date (`YYYY-MM-DD` and `m/d/yyyy h:mm` do; `0 "days"`
/// and `h:mm` don't).
fn is_date_format(fmt: &str) -> bool {
    NumberFormat::parse(fmt).kind() == FormatKind::Date
}

/// Whether the first cell of `src` holds a date, so a fill from it can step by days, weekdays,
/// months or years: a number (not a formula) shown in a date format.
pub fn starts_with_date(wb: &Workbook, sheet: usize, src: RangeRef) -> bool {
    let Some(cell) = wb.sheet(sheet).and_then(|sh| sh.cell(src.start)) else { return false };
    cell.formula.is_none() && cell.value.is_number() && is_date_format(wb.styles.get(cell.style).num_fmt.as_str())
}

/// Built-in custom lists.
fn lists() -> Vec<Vec<String>> {
    let short_days: Vec<String> = WEEKDAYS.iter().map(|d| d.get(..3).unwrap_or(d).to_string()).collect();
    let short_months: Vec<String> = MONTHS.iter().map(|m| m.get(..3).unwrap_or(m).to_string()).collect();
    vec![
        short_days,
        WEEKDAYS.iter().map(|s| s.to_string()).collect(),
        short_months,
        MONTHS.iter().map(|s| s.to_string()).collect(),
        vec!["Q1".into(), "Q2".into(), "Q3".into(), "Q4".into()],
    ]
}

/// The most cells one fill may write: the size cap the fill commands put on their ranges.
pub const MAX_FILL_CELLS: u64 = 5_000_000;

/// Whether `target` is a shape Excel's AutoFill accepts for `src`: it contains the source and the
/// source sits in one of its corners. Excel refuses a target that doesn't contain the source
/// (`2:3` into `A:A`, `H4:H9` into `D:F`) or holds it away from every corner (`H4:H9` into `H:H`).
pub fn holds_source_in_corner(src: RangeRef, target: RangeRef) -> bool {
    target.contains_range(&src)
        && (target.start.row == src.start.row || target.end.row == src.end.row)
        && (target.start.col == src.start.col || target.end.col == src.end.col)
}

/// Fills `target` (which contains `src` at one edge) from the pattern in `src`.
pub fn fill(cx: &mut Ctx, sheet: usize, src: RangeRef, target: RangeRef, mode: FillMode) -> Result<()> {
    fill_except(cx, sheet, src, target, mode, &std::collections::BTreeSet::new())
}

/// [`fill`], leaving the rows in `skip_rows` untouched (Fill Down over a filtered range).
pub fn fill_except(
    cx: &mut Ctx,
    sheet: usize,
    src: RangeRef,
    target: RangeRef,
    mode: FillMode,
    skip_rows: &std::collections::BTreeSet<u32>,
) -> Result<()> {
    let custom = cx.wb.custom_lists.clone();
    let Some(sh) = cx.wb.sheet(sheet) else { return Ok(()) };
    // Direction: down/up if target extends rows, else right/left.
    let vertical = target.height() != src.height() || target.width() == src.width();
    let forward = if vertical {
        target.end.row > src.end.row || target.start.row == src.start.row
    } else {
        target.end.col > src.end.col || target.start.col == src.start.col
    };
    // Every source lane writes one cell per step past the source: bound that product, not the
    // source and target separately (whole rows into whole columns was ~17 billion writes).
    let (lane_count, steps) = if vertical {
        let steps = if forward { target.end.row.saturating_sub(src.end.row) } else { src.start.row.saturating_sub(target.start.row) };
        (src.width(), steps)
    } else {
        let steps = if forward { target.end.col.saturating_sub(src.end.col) } else { src.start.col.saturating_sub(target.start.col) };
        (src.height(), steps)
    };
    if u64::from(lane_count).saturating_mul(u64::from(steps)) > MAX_FILL_CELLS {
        return Err(crate::EngineError::Other("The fill is too large.".into()));
    }
    let lanes: Vec<u32> = if vertical { (src.start.col..=src.end.col).collect() } else { (src.start.row..=src.end.row).collect() };
    let src_len = if vertical { src.height() } else { src.width() };
    let mut writes: Vec<(CellRef, Option<Cell>)> = Vec::new();
    let mut pictures = Vec::new();
    for lane in lanes {
        let src_cells: Vec<(CellRef, Option<Cell>)> = (0..src_len)
            .map(|i| {
                let c = if vertical { CellRef::new(src.start.row + i, lane) } else { CellRef::new(lane, src.start.col + i) };
                (c, sh.cell(c).cloned())
            })
            .collect();
        let mut targets: Vec<CellRef> = if vertical {
            let rows: Vec<u32> =
                if forward { (src.end.row + 1..=target.end.row).collect() } else { (target.start.row..src.start.row).rev().collect() };
            rows.into_iter().map(|r| CellRef::new(r, lane)).collect()
        } else {
            let cols: Vec<u32> =
                if forward { (src.end.col + 1..=target.end.col).collect() } else { (target.start.col..src.start.col).rev().collect() };
            cols.into_iter().map(|c| CellRef::new(lane, c)).collect()
        };
        targets.retain(|t| !skip_rows.contains(&t.row));
        let fmt = |c: &(CellRef, Option<Cell>)| cx.wb.styles.get(c.1.as_ref().map(|c| c.style).unwrap_or_default()).num_fmt.as_str();
        let pattern = if mode.extends_series() || mode == FillMode::Alternate {
            let first_fmt = src_cells.first().map(fmt).unwrap_or_default();
            let dates = match mode {
                FillMode::Dates(unit) => Pattern::dates(&src_cells, unit, cx.wb.date_system, first_fmt),
                _ => None,
            };
            let detected = dates.unwrap_or_else(|| Pattern::detect(&src_cells, &custom, cx.wb.date_system, first_fmt));
            match (mode, detected) {
                (FillMode::Alternate, p) | (FillMode::Extend, p @ Pattern::Copy) => p.alternate(&src_cells),
                (_, p) => p,
            }
        } else {
            Pattern::Copy
        };
        // A source with no common pattern (`Product 1`, `On backorder`): each cell continues its
        // own series, one step per repeat of the source.
        let per_cell: Vec<Pattern> = if mode.extends_series() && matches!(pattern, Pattern::Copy) && src_cells.len() > 1 {
            src_cells.iter().map(|c| Pattern::detect(std::slice::from_ref(c), &custom, cx.wb.date_system, fmt(c))).collect()
        } else {
            Vec::new()
        };
        for (k, t) in targets.iter().enumerate() {
            let step = k as i64 + 1;
            let idx = k % src_cells.len().max(1);
            let src_i = if forward { idx } else { src_cells.len() - 1 - idx };
            let (sc, scell) = src_cells.get(src_i).cloned().unwrap_or((src.start, None));
            let mut cell = scell.clone().unwrap_or_default();
            match mode {
                FillMode::Formats => {
                    let mut existing = sh.cell(*t).cloned().unwrap_or_default();
                    existing.style = cell.style;
                    writes.push((*t, Some(existing)));
                    continue;
                }
                FillMode::ValuesOnly | FillMode::Series | FillMode::Alternate | FillMode::Copy | FillMode::Extend | FillMode::Dates(_) => {
                    let style = if mode == FillMode::ValuesOnly { sh.cell(*t).map(|c| c.style).unwrap_or_default() } else { cell.style };
                    if let Some(f) = &cell.formula {
                        if let Some(e) = f.expr() {
                            let shifted = gridcraft_formula::adjust::shift_relative(e, t.row as i64 - sc.row as i64, t.col as i64 - sc.col as i64);
                            cell.formula = Some(Arc::new(Formula::from_expr(shifted)));
                            cell.value = Value::Empty;
                        }
                    } else if cell.py.is_none()
                        && let Some(v) = match per_cell.get(src_i) {
                            Some(p) => p.value(k as i64 / src_cells.len() as i64 + 1, forward, 1),
                            None => pattern.value(step, forward, src_cells.len()),
                        }
                    {
                        cell.value = v;
                    }
                    cell.style = style;
                }
            }
            if let Some(picture) = sh.cell_pictures.get(&sc) {
                pictures.push((*t, picture.clone()));
            }
            writes.push((*t, if cell.is_blank() { None } else { Some(cell) }));
        }
    }
    let sh = cx.sheet_mut(sheet)?;
    for (c, cell) in &writes {
        match cell {
            Some(x) if mode == FillMode::Formats => sh.cells.set(*c, x.clone()),
            Some(x) => sh.set_cell(*c, x.clone()),
            None => {
                sh.remove_cell(*c);
            }
        }
    }
    sh.cell_pictures.extend(pictures);
    for (c, _) in writes {
        cx.touch(sheet, c);
    }
    Ok(())
}

#[derive(Clone, Debug)]
enum Pattern {
    Copy,
    /// Linear numeric series: last value and step.
    Linear {
        last: f64,
        first: f64,
        step: f64,
    },
    /// Text with a trailing (or leading) number: `Item 1`, `Q1`.
    TextNum {
        prefix: String,
        suffix: String,
        last: i64,
        first: i64,
        step: i64,
        width: usize,
    },
    /// A position in a list (days, months, custom).
    List {
        list: Vec<String>,
        last: usize,
        first: usize,
        step: i64,
        upper: bool,
        lower: bool,
    },
    /// Dates stepping by months (same day each month) or years.
    Months {
        last: f64,
        first: f64,
        months: i64,
        sys: gridcraft_core::DateSystem,
    },
    /// Dates stepping by weekdays, skipping Saturday and Sunday.
    Weekdays {
        last: f64,
        first: f64,
        step: i64,
        sys: gridcraft_core::DateSystem,
    },
}

impl Pattern {
    /// The pattern of Fill Days, Fill Weekdays, Fill Months or Fill Years, or `None` when the
    /// source isn't all dates (numbers, not formulas, under a date format in its first cell).
    fn dates(cells: &[(CellRef, Option<Cell>)], unit: DateUnit, sys: gridcraft_core::DateSystem, fmt: &str) -> Option<Pattern> {
        if !is_date_format(fmt) {
            return None;
        }
        let mut nums = Vec::with_capacity(cells.len());
        for (_, cell) in cells {
            let cell = cell.as_ref()?;
            if cell.formula.is_some() || !cell.value.is_number() {
                return None;
            }
            nums.push(cell.value.as_f64()?);
        }
        let first = *nums.first()?;
        let last = *nums.last()?;
        let second = nums.get(1).copied();
        // The difference between the first two dates in the unit, and one unit when they fall in
        // the same one.
        let at_least_one = |n: i64| if n == 0 { 1 } else { n };
        Some(match unit {
            DateUnit::Day => {
                let days = second.map_or(1.0, |b| b.floor() - first.floor());
                Pattern::Linear { last, first, step: if days == 0.0 { 1.0 } else { days } }
            }
            DateUnit::Weekday => {
                let step = match second {
                    Some(b) => at_least_one(weekdays_between(sys, first, b)?),
                    None => 1,
                };
                Pattern::Weekdays { last, first, step, sys }
            }
            DateUnit::Month | DateUnit::Year => {
                let months = match second {
                    Some(b) => {
                        let (a, b) = (datetime_from_serial(sys, first)?, datetime_from_serial(sys, b)?);
                        let years = b.year as i64 - a.year as i64;
                        if unit == DateUnit::Year { at_least_one(years) * 12 } else { at_least_one(years * 12 + b.month as i64 - a.month as i64) }
                    }
                    None if unit == DateUnit::Year => 12,
                    None => 1,
                };
                Pattern::Months { last, first, months, sys }
            }
        })
    }

    fn detect(cells: &[(CellRef, Option<Cell>)], custom: &[Vec<String>], sys: gridcraft_core::DateSystem, fmt: &str) -> Pattern {
        let vals: Vec<Value> = cells.iter().map(|(_, c)| c.as_ref().map(|c| c.value.clone()).unwrap_or_default()).collect();
        // Formulas and Python cells copy.
        if cells.iter().any(|(_, c)| c.as_ref().is_some_and(|c| c.formula.is_some() || c.py.is_some())) {
            return Pattern::Copy;
        }
        // Numbers.
        if !vals.is_empty() && vals.iter().all(|v| v.is_number()) {
            let nums: Vec<f64> = vals.iter().filter_map(Value::as_f64).collect();
            let first = nums.first().copied().unwrap_or(0.0);
            let last = nums.last().copied().unwrap_or(0.0);
            let is_date = fmt.contains('d') || fmt.contains('y') || (fmt.contains('m') && !fmt.contains('h'));
            if nums.len() == 1 {
                // A single number copies; a single date increments by a day, a single time by an hour.
                return if is_date {
                    Pattern::Linear { last, first, step: 1.0 }
                } else if NumberFormat::parse(fmt).kind() == FormatKind::Time {
                    Pattern::Linear { last, first, step: 1.0 / 24.0 }
                } else {
                    Pattern::Copy
                };
            }
            // Dates that differ by whole months.
            if is_date && nums.len() >= 2 {
                let d0 = datetime_from_serial(sys, first);
                let d1 = datetime_from_serial(sys, nums.get(1).copied().unwrap_or(first));
                if let (Some(a), Some(b)) = (d0, d1)
                    && a.day == b.day
                    && (a.month != b.month || a.year != b.year)
                {
                    let months = (b.year as i64 - a.year as i64) * 12 + b.month as i64 - a.month as i64;
                    return Pattern::Months { last, first, months, sys };
                }
            }
            // Least-squares step for 2+ values (Excel uses a linear trend).
            let n = nums.len() as f64;
            let mean_x = (n - 1.0) / 2.0;
            let mean_y = nums.iter().sum::<f64>() / n;
            let mut num = 0.0;
            let mut den = 0.0;
            for (i, y) in nums.iter().enumerate() {
                num += (i as f64 - mean_x) * (y - mean_y);
                den += (i as f64 - mean_x).powi(2);
            }
            let step = if den == 0.0 { 0.0 } else { num / den };
            let fitted_last = mean_y + step * (n - 1.0 - mean_x);
            let fitted_first = mean_y - step * mean_x;
            return Pattern::Linear { last: fitted_last, first: fitted_first, step };
        }
        // Text.
        if !vals.is_empty() && vals.iter().all(|v| v.is_text()) {
            let texts: Vec<String> = vals.iter().filter_map(|v| v.as_text().map(str::to_string)).collect();
            let all_lists: Vec<Vec<String>> = custom.iter().cloned().chain(lists()).collect();
            for list in &all_lists {
                let idx: Vec<Option<usize>> = texts.iter().map(|t| list.iter().position(|l| l.eq_ignore_ascii_case(t))).collect();
                if idx.iter().all(Option::is_some) {
                    let idx: Vec<usize> = idx.into_iter().flatten().collect();
                    let first = idx.first().copied().unwrap_or(0);
                    let last = idx.last().copied().unwrap_or(0);
                    let step = if idx.len() >= 2 { idx[1] as i64 - idx[0] as i64 } else { 1 };
                    let t0 = texts.first().cloned().unwrap_or_default();
                    let upper = t0.chars().all(|c| !c.is_lowercase()) && t0.chars().any(char::is_alphabetic) && t0.len() > 1;
                    let lower = t0.chars().all(|c| !c.is_uppercase());
                    return Pattern::List { list: list.clone(), last, first, step, upper, lower };
                }
            }
            // Text with a number at the end (or start).
            let split: Vec<Option<(String, i64, String, usize)>> = texts.iter().map(|t| split_num(t)).collect();
            if split.iter().all(Option::is_some) {
                let parts: Vec<(String, i64, String, usize)> = split.into_iter().flatten().collect();
                if parts.iter().all(|p| p.0 == parts[0].0 && p.2 == parts[0].2) {
                    let first = parts.first().map(|p| p.1).unwrap_or(0);
                    let last = parts.last().map(|p| p.1).unwrap_or(0);
                    let step = if parts.len() >= 2 { parts[1].1 - parts[0].1 } else { 1 };
                    return Pattern::TextNum { prefix: parts[0].0.clone(), suffix: parts[0].2.clone(), last, first, step, width: parts[0].3 };
                }
            }
        }
        Pattern::Copy
    }

    /// The pattern of a Ctrl+drag, which swaps copying and extending: a single
    /// number (which a plain drag copies) counts up by 1, and every series a
    /// plain drag would extend (numbers, dates, lists, `Item 1`) is copied.
    fn alternate(self, cells: &[(CellRef, Option<Cell>)]) -> Pattern {
        match (self, cells) {
            (Pattern::Copy, [(_, Some(cell))]) if cell.formula.is_none() => match cell.value.as_f64() {
                Some(n) if cell.value.is_number() => Pattern::Linear { last: n, first: n, step: 1.0 },
                _ => Pattern::Copy,
            },
            _ => Pattern::Copy,
        }
    }

    /// The value `k` steps beyond the source (forward) or before it (backward).
    fn value(&self, k: i64, forward: bool, _len: usize) -> Option<Value> {
        let sign = if forward { 1 } else { -1 };
        match self {
            Pattern::Copy => None,
            Pattern::Linear { last, first, step } => {
                let base = if forward { *last } else { *first };
                Some(Value::number(round15(base + step * (k * sign) as f64)))
            }
            Pattern::TextNum { prefix, suffix, last, first, step, width } => {
                let base = if forward { *last } else { *first };
                // Text numbers come from input and can sit near i64::MAX (`X 9223372036854775807`):
                // an overflowing series yields nothing — the cell keeps its source value — rather
                // than wrapping in release or panicking in debug.
                let n = step.checked_mul(k)?.checked_mul(sign)?.checked_add(base)?;
                // `Qtr3`, `Quarter 3` wrap after 4 like `Q3`.
                let quarter = suffix.is_empty()
                    && (1..=4).contains(&base)
                    && ["q", "qtr", "quarter"].contains(&prefix.trim_end().to_ascii_lowercase().as_str());
                let n = if quarter { n.checked_sub(1)?.rem_euclid(4) + 1 } else { n.checked_abs()? };
                Some(Value::text(format!("{prefix}{n:0width$}{}", ordinal_suffix(n, suffix), width = *width)))
            }
            Pattern::List { list, last, first, step, upper, lower } => {
                let len = list.len() as i64;
                if len == 0 {
                    return None;
                }
                let base = if forward { *last } else { *first } as i64;
                let i = (base + step * k * sign).rem_euclid(len) as usize;
                let s = list.get(i)?.clone();
                Some(Value::text(if *upper {
                    s.to_uppercase()
                } else if *lower && !s.chars().next().is_some_and(char::is_uppercase) {
                    s.to_lowercase()
                } else {
                    s
                }))
            }
            Pattern::Months { last, first, months, sys } => {
                let base = if forward { *last } else { *first };
                let s = add_months_clamped(*sys, base, months.checked_mul(k * sign)?)?;
                Some(Value::number(s))
            }
            Pattern::Weekdays { last, first, step, sys } => {
                let base = if forward { *last } else { *first };
                Some(Value::number(add_weekdays(*sys, base, step.checked_mul(k)?.checked_mul(sign)?)?))
            }
        }
    }
}

/// Whether `serial` falls on Monday to Friday; `None` outside the supported date range.
fn is_weekday(sys: gridcraft_core::DateSystem, serial: f64) -> Option<bool> {
    datetime_from_serial(sys, serial).map(|d| d.weekday != 0 && d.weekday != 6)
}

/// `serial` moved by `n` weekdays (back when `n` is negative), keeping its time of day. A start on
/// a Saturday or Sunday moves to the next (or previous) weekday for the first step.
fn add_weekdays(sys: gridcraft_core::DateSystem, serial: f64, n: i64) -> Option<f64> {
    if n == 0 {
        return Some(serial);
    }
    let dir = n.signum() as f64;
    let mut d = serial;
    let next_weekday = |d: &mut f64| -> Option<()> {
        loop {
            *d += dir;
            if is_weekday(sys, *d)? {
                return Some(());
            }
        }
    };
    next_weekday(&mut d)?;
    // From a weekday, every seven days hold five weekdays.
    let rest = n.unsigned_abs() - 1;
    d += dir * (rest / 5 * 7) as f64;
    for _ in 0..rest % 5 {
        next_weekday(&mut d)?;
    }
    datetime_from_serial(sys, d)?;
    Some(d)
}

/// How many weekdays lie after `a` up to and including `b` (negative when `b` comes first),
/// counting whole days; `None` outside the supported date range.
fn weekdays_between(sys: gridcraft_core::DateSystem, a: f64, b: f64) -> Option<i64> {
    let (lo, hi, sign) = if b >= a { (a.floor(), b.floor(), 1) } else { (b.floor(), a.floor(), -1) };
    let days = (hi - lo) as i64;
    let mut count = days / 7 * 5;
    let mut d = lo + (days / 7 * 7) as f64;
    for _ in 0..days % 7 {
        d += 1.0;
        if is_weekday(sys, d)? {
            count += 1;
        }
    }
    Some(count * sign)
}

/// `serial` shifted by `months` calendar months, keeping the day of month but clamping it to the
/// target month's last day (Jan 31 + 1 month = Feb 28/29). Always measured from the original date,
/// so a series does not drift: Jan 31, Feb 28, Mar 31, Apr 30. The time of day is kept.
fn add_months_clamped(sys: gridcraft_core::DateSystem, serial: f64, months: i64) -> Option<f64> {
    let d = datetime_from_serial(sys, serial)?;
    let total = (d.year as i64).checked_mul(12)?.checked_add(d.month as i64 - 1)?.checked_add(months)?;
    let year = total.div_euclid(12);
    let month = total.rem_euclid(12) + 1;
    let last = days_in_month(i32::try_from(year).ok()?, month as u32);
    serial_from_ymd(sys, year, month, (d.day as i64).min(last as i64)).map(|day| day + serial - serial.floor())
}

fn round15(x: f64) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let mag = x.abs().log10().floor() as i32;
    let p = 10f64.powi(14 - mag);
    if !p.is_finite() || p == 0.0 { x } else { (x * p).round() / p }
}

/// `1st Period` → `2nd Period`: an English ordinal ending is rewritten to suit the new number.
fn ordinal_suffix(n: i64, suffix: &str) -> String {
    let (Some(end), Some(rest)) = (suffix.get(..2), suffix.get(2..)) else { return suffix.to_string() };
    if !["st", "nd", "rd", "th"].contains(&end.to_ascii_lowercase().as_str()) || rest.starts_with(char::is_alphanumeric) {
        return suffix.to_string();
    }
    let new = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    let new = if end.chars().all(|c| c.is_ascii_uppercase()) { new.to_ascii_uppercase() } else { new.to_string() };
    format!("{new}{rest}")
}

/// `Item 007` → ("Item ", 7, "", 3); `3rd` style leading numbers → ("", 3, "rd", 1).
fn split_num(t: &str) -> Option<(String, i64, String, usize)> {
    let chars: Vec<char> = t.chars().collect();
    let end = chars.iter().rposition(char::is_ascii_digit)?;
    let mut start = end;
    while start > 0 && chars.get(start - 1).is_some_and(char::is_ascii_digit) {
        start -= 1;
    }
    let digits: String = chars.get(start..=end)?.iter().collect();
    let n: i64 = digits.parse().ok()?;
    let prefix: String = chars.get(..start)?.iter().collect();
    let suffix: String = chars.get(end + 1..)?.iter().collect();
    let width = if digits.starts_with('0') && digits.len() > 1 { digits.len() } else { 1 };
    Some((prefix, n, suffix, width))
}

/// Options for filling a numeric or date series.
pub struct SeriesOptions<'a> {
    pub range: RangeRef,
    pub rows: bool,
    pub kind: &'a str,
    pub step: f64,
    pub stop: Option<f64>,
    pub unit: &'a str,
}

/// Home › Fill › Series.
pub fn series(cx: &mut Ctx, sheet: usize, options: SeriesOptions<'_>) -> Result<()> {
    let SeriesOptions { range: r, rows, kind, step, stop, unit } = options;
    let sys = cx.wb.date_system;
    let Some(sh) = cx.wb.sheet(sheet) else { return Ok(()) };
    let lanes: Vec<u32> = if rows { (r.start.row..=r.end.row).collect() } else { (r.start.col..=r.end.col).collect() };
    let len = if rows { r.width() } else { r.height() };
    let mut writes = Vec::new();
    for lane in lanes {
        let first_cell = if rows { CellRef::new(lane, r.start.col) } else { CellRef::new(r.start.row, lane) };
        let Some(start) = sh.value(first_cell).as_f64() else { continue };
        let style = sh.style_id(first_cell);
        let mut v = start;
        for i in 1..len {
            let prev = v;
            v = match kind {
                "growth" => v * step,
                "date" => {
                    let d = datetime_from_serial(sys, v);
                    match (unit, d) {
                        ("month", Some(_)) => (step as i64).checked_mul(i as i64).and_then(|m| add_months_clamped(sys, start, m)).unwrap_or(v),
                        ("year", Some(_)) => (step as i64).checked_mul(i as i64 * 12).and_then(|m| add_months_clamped(sys, start, m)).unwrap_or(v),
                        // Whole weeks at a time: a huge step costs no more than a small one.
                        ("weekday", Some(_)) => add_weekdays(sys, v, step as i64)
                            .ok_or_else(|| crate::EngineError::Other("The weekday series runs past the supported date range.".into()))?,
                        _ => v + step,
                    }
                }
                _ => v + step,
            };
            // The stop value bounds the series in the direction it runs (a growth by 0.5 falls).
            if let Some(s) = stop
                && ((v >= prev && round15(v) > s) || (v < prev && round15(v) < s))
            {
                break;
            }
            let c = if rows { CellRef::new(lane, r.start.col + i) } else { CellRef::new(r.start.row + i, lane) };
            writes.push((c, Cell { value: Value::number(round15(v)), formula: None, style, py: None }));
        }
    }
    let sh = cx.sheet_mut(sheet)?;
    for (c, cell) in &writes {
        sh.set_cell(*c, cell.clone());
    }
    for (c, _) in writes {
        cx.touch(sheet, c);
    }
    Ok(())
}

/// Flash Fill: learns a transformation from the examples typed in the column of `at` (rows
/// above it with values) from the column(s) to the left, and fills the remaining rows.
/// Supported transformations: a token (word) of the source by index, concatenations of tokens
/// with literal separators, upper/lower/proper case, and fixed-position substrings.
pub fn flash_fill(cx: &mut Ctx, sheet: usize, at: CellRef) -> Result<usize> {
    let Some(sh) = cx.wb.sheet(sheet) else { return Ok(0) };
    if at.col == 0 {
        return Ok(0);
    }
    let src_col = at.col - 1;
    // Rows of the data region in the source column.
    let mut top = at.row;
    while top > 0 && !sh.value(CellRef::new(top - 1, src_col)).is_empty() {
        top -= 1;
    }
    let mut bottom = at.row;
    while bottom + 1 < gridcraft_core::MAX_ROWS && !sh.value(CellRef::new(bottom + 1, src_col)).is_empty() && bottom - top < 1_000_000 {
        bottom += 1;
    }
    // Header row: skip a first row whose target is text and source looks like a header? Keep simple.
    let mut examples = Vec::new();
    let mut todo = Vec::new();
    // Flash Fill works on the text cells show (a date as formatted, not its serial number).
    for r in top..=bottom {
        let src = crate::display::cell_text(&cx.wb, sh, CellRef::new(r, src_col));
        if sh.value(CellRef::new(r, at.col)).is_empty() {
            todo.push((r, src));
        } else {
            examples.push((src, crate::display::cell_text(&cx.wb, sh, CellRef::new(r, at.col))));
        }
    }
    if examples.is_empty() {
        return Ok(0);
    }
    let Some(program) = learn(&examples) else { return Ok(0) };
    let mut n = 0;
    let style = sh.style_id(CellRef::new(top, at.col));
    let mut writes = Vec::new();
    for (r, src) in todo {
        if let Some(out) = program.apply(&src) {
            writes.push((CellRef::new(r, at.col), out));
            n += 1;
        }
    }
    let sh = cx.sheet_mut(sheet)?;
    for (c, out) in &writes {
        sh.set_cell(*c, Cell { value: Value::text(out.as_str()), formula: None, style, py: None });
    }
    for (c, _) in writes {
        cx.touch(sheet, c);
    }
    Ok(n)
}

#[derive(Clone, Debug, PartialEq)]
enum Piece {
    Lit(String),
    /// Token index (negative from the end) with a case transform.
    Tok(i32, Case),
    /// First character of a token.
    Initial(i32, Case),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Case {
    Same,
    Upper,
    Lower,
    Proper,
}

#[derive(Clone, Debug)]
struct Program(Vec<Piece>);

fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|t| !t.is_empty()).map(str::to_string).collect()
}

fn apply_case(s: &str, c: Case) -> String {
    match c {
        Case::Same => s.to_string(),
        Case::Upper => s.to_uppercase(),
        Case::Lower => s.to_lowercase(),
        Case::Proper => {
            let mut out = String::new();
            for (i, ch) in s.chars().enumerate() {
                if i == 0 {
                    out.extend(ch.to_uppercase());
                } else {
                    out.extend(ch.to_lowercase());
                }
            }
            out
        }
    }
}

impl Program {
    fn apply(&self, src: &str) -> Option<String> {
        let toks = tokens(src);
        let get = |i: i32| -> Option<&String> {
            if i >= 0 { toks.get(i as usize) } else { toks.len().checked_sub(i.unsigned_abs() as usize).and_then(|j| toks.get(j)) }
        };
        let mut out = String::new();
        for p in &self.0 {
            match p {
                Piece::Lit(l) => out.push_str(l),
                Piece::Tok(i, c) => out.push_str(&apply_case(get(*i)?, *c)),
                Piece::Initial(i, c) => out.push_str(&apply_case(&get(*i)?.chars().next()?.to_string(), *c)),
            }
        }
        Some(out)
    }
}

/// Finds a program consistent with every example.
fn learn(examples: &[(String, String)]) -> Option<Program> {
    let (src, dst) = examples.first()?;
    let candidates = synthesize(src, dst);
    candidates.into_iter().find(|p| examples.iter().all(|(s, d)| p.apply(s).as_deref() == Some(d.as_str())))
}

/// Candidate programs for one example, by greedy segmentation of the output into token matches
/// and literals (tries several index conventions).
fn synthesize(src: &str, dst: &str) -> Vec<Program> {
    let toks = tokens(src);
    let n = toks.len() as i32;
    let mut out = Vec::new();
    for prefer_neg in [false, true] {
        let mut pieces = Vec::new();
        let mut rest = dst;
        let mut guard = 0;
        while !rest.is_empty() && guard < 64 {
            guard += 1;
            // Longest token (with any case) that prefixes `rest`.
            let mut best: Option<(usize, Piece)> = None;
            for (i, t) in toks.iter().enumerate() {
                let idx = if prefer_neg { i as i32 - n } else { i as i32 };
                for c in [Case::Same, Case::Upper, Case::Lower, Case::Proper] {
                    let cand = apply_case(t, c);
                    if !cand.is_empty() && rest.starts_with(&cand) && best.as_ref().is_none_or(|(l, _)| cand.len() > *l) {
                        best = Some((cand.len(), Piece::Tok(idx, c)));
                    }
                }
            }
            if best.as_ref().is_none_or(|(l, _)| *l <= 1) {
                // Initial letters (e.g. "J. Smith").
                for (i, t) in toks.iter().enumerate() {
                    let idx = if prefer_neg { i as i32 - n } else { i as i32 };
                    if let Some(ch) = t.chars().next() {
                        for c in [Case::Same, Case::Upper, Case::Lower] {
                            let cand = apply_case(&ch.to_string(), c);
                            if rest.starts_with(&cand) && best.is_none() {
                                best = Some((cand.len(), Piece::Initial(idx, c)));
                            }
                        }
                    }
                }
            }
            match best {
                Some((len, piece)) => {
                    pieces.push(piece);
                    rest = rest.get(len..).unwrap_or("");
                }
                None => {
                    let ch = rest.chars().next().unwrap_or(' ');
                    match pieces.last_mut() {
                        Some(Piece::Lit(l)) => l.push(ch),
                        _ => pieces.push(Piece::Lit(ch.to_string())),
                    }
                    rest = rest.get(ch.len_utf8()..).unwrap_or("");
                }
            }
        }
        out.push(Program(pieces));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flash_fill_reads_shown_text() {
        let mut s = crate::Session::new();
        s.new_workbook();
        let values = serde_json::json!([["2024-01-15", "15/01/2024"], ["2023-12-03", null]]);
        s.execute("range.setValues", serde_json::json!({"range": "A1", "values": values})).unwrap();
        s.execute("selection.set", serde_json::json!({"cell": "B2"})).unwrap();
        s.execute("data.flashFill", serde_json::json!({})).unwrap();
        let sh = s.doc().unwrap().wb.active().unwrap();
        assert_eq!(sh.value(CellRef::new(1, 1)), Value::from("03/12/2023"));
    }

    #[test]
    fn series_keeps_value_equal_to_stop_after_float_drift() {
        let mut s = crate::Session::new();
        s.new_workbook();
        s.execute("cell.set", serde_json::json!({"cell": "A1", "input": "1"})).unwrap();
        s.execute("edit.fillSeries", serde_json::json!({"range": "A1:A6", "type": "linear", "step": 0.1, "stop": 1.3})).unwrap();
        s.execute("cell.set", serde_json::json!({"cell": "B1", "input": "0.1"})).unwrap();
        s.execute("edit.fillSeries", serde_json::json!({"range": "B1:B4", "type": "growth", "step": 3, "stop": 0.3})).unwrap();
        let sh = s.doc().unwrap().wb.active().unwrap();
        let col = |c: u32, n: u32| (0..n).map(|r| sh.value(CellRef::new(r, c))).collect::<Vec<_>>();
        let n = Value::number;
        assert_eq!(col(0, 6), vec![n(1.0), n(1.1), n(1.2), n(1.3), Value::Empty, Value::Empty]);
        assert_eq!(col(1, 4), vec![n(0.1), n(0.3), Value::Empty, Value::Empty]);
    }

    #[test]
    fn flash_programs() {
        let p = learn(&[("John Smith".into(), "Smith, John".into())]).unwrap();
        assert_eq!(p.apply("Ada Lovelace").as_deref(), Some("Lovelace, Ada"));
        let p = learn(&[("jane doe".into(), "JD".into())]).unwrap();
        assert_eq!(p.apply("alan turing").as_deref(), Some("AT"));
        let p = learn(&[("alice@example.com".into(), "alice".into())]).unwrap();
        assert_eq!(p.apply("bob@example.com").as_deref(), Some("bob"));
    }

    #[test]
    fn split_numbers() {
        assert_eq!(split_num("Item 7"), Some(("Item ".into(), 7, "".into(), 1)));
        assert_eq!(split_num("Q003x"), Some(("Q".into(), 3, "x".into(), 3)));
        assert_eq!(split_num("abc"), None);
    }

    #[test]
    fn times_quarters_ordinals_and_mixed_sources() {
        let mut s = crate::Session::new();
        s.new_workbook();
        let mut fill = |values: serde_json::Value, source: &str, target: &str| {
            s.execute("range.setValues", serde_json::json!({"range": source.get(..2).unwrap(), "values": values})).unwrap();
            s.execute("edit.autoFill", serde_json::json!({"source": source, "target": target})).unwrap();
            let sh = s.doc().unwrap().wb.active().unwrap();
            let r = RangeRef::parse(target).unwrap();
            r.iter().map(|c| crate::display::cell_text(&s.doc().unwrap().wb, sh, c)).collect::<Vec<_>>()
        };
        assert_eq!(fill(serde_json::json!([["9:00"]]), "A1", "A1:A3"), ["9:00", "10:00", "11:00"]);
        assert_eq!(fill(serde_json::json!([["Qtr3"]]), "B1", "B1:B3"), ["Qtr3", "Qtr4", "Qtr1"]);
        assert_eq!(fill(serde_json::json!([["Quarter 4"]]), "C1", "C1:C2"), ["Quarter 4", "Quarter 1"]);
        assert_eq!(fill(serde_json::json!([["1st Period"]]), "D1", "D1:D4"), ["1st Period", "2nd Period", "3rd Period", "4th Period"]);
        assert_eq!(
            fill(serde_json::json!([["Product 1"], ["On backorder"]]), "E1:E2", "E1:E6"),
            ["Product 1", "On backorder", "Product 2", "On backorder", "Product 3", "On backorder"]
        );
    }

    #[test]
    fn falling_growth_series_stops_at_its_stop_value() {
        let mut s = crate::Session::new();
        s.new_workbook();
        s.execute("cell.set", serde_json::json!({"cell": "A1", "input": "5"})).unwrap();
        s.execute("edit.fillSeries", serde_json::json!({"range": "A1:A6", "type": "growth", "step": 0.5, "stop": 1})).unwrap();
        let d = s.doc().unwrap();
        let sh = d.wb.active().unwrap();
        let growth: Vec<String> = (1..6).map(|r| crate::display::cell_text(&d.wb, sh, CellRef::new(r, 0))).collect();
        assert_eq!(growth, ["2.5", "1.25", "", "", ""]);
    }

    #[test]
    fn text_series_near_i64_max_neither_wraps_nor_panics() {
        let mut s = crate::Session::new();
        s.new_workbook();
        s.execute("cell.set", serde_json::json!({"cell": "A1", "input": "X 0"})).unwrap();
        s.execute("cell.set", serde_json::json!({"cell": "A2", "input": "X 9223372036854775807"})).unwrap();
        // The detected step is i64::MAX, so the next value overflows i64. The cell keeps its
        // source value instead of panicking in debug or wrapping in release.
        s.execute("edit.autoFill", serde_json::json!({"source": "A1:A2", "target": "A1:A3"})).unwrap();
        let d = s.doc().unwrap();
        let sh = d.wb.active().unwrap();
        assert_eq!(crate::display::cell_text(&d.wb, sh, CellRef::new(2, 0)), "X 0");
    }

    #[test]
    fn autofill_refuses_targets_that_do_not_extend_the_source() {
        // Whole rows into whole columns once filled every source column down the whole sheet
        // (~17 billion writes) and aborted the process on a 24-51 GB allocation (fuzz seeds 1064,
        // 1114, 1775). Excel refuses every one of these shapes.
        let mut s = crate::Session::new();
        s.new_workbook();
        s.execute("range.setValues", serde_json::json!({"range": "A2", "values": [[1, "Item 1"], [2, "Item 2"]]})).unwrap();
        s.execute("range.setValues", serde_json::json!({"range": "H4", "values": [[10], [20]]})).unwrap();
        let before = s.doc().unwrap().wb.active().unwrap().cells.len();
        for (mode, source, target) in [
            ("series", "2:3", "A:A"),
            ("alternate", "2:3", "A:A"),
            ("series", "8:10", "C:E"),
            ("copy", "24:24", "B:D"),
            ("values", "H4:H9", "D:F"),
            ("series", "H4:H9", "H:H"),
            ("series", "A2:A3", "A1:A5"),
            ("series", "A2:A6", "A2:A4"),
            ("series", "B2:B3", "A2:A3"),
        ] {
            let r = s.execute("edit.autoFill", serde_json::json!({"mode": mode, "source": source, "target": target}));
            assert!(r.is_err(), "{source} -> {target} should be refused");
        }
        assert_eq!(s.doc().unwrap().wb.active().unwrap().cells.len(), before, "a refused fill writes nothing");
        // Targets holding the source in a corner still fill, whole rows and diagonal drags included.
        s.execute("edit.autoFill", serde_json::json!({"source": "2:3", "target": "2:6"})).unwrap();
        s.execute("edit.autoFill", serde_json::json!({"source": "A2:A3", "target": "A1:A3"})).unwrap();
        let sh = s.doc().unwrap().wb.active().unwrap();
        let text = |a: &str| crate::display::cell_text(&s.doc().unwrap().wb, sh, CellRef::parse(a).unwrap());
        assert_eq!([text("A1"), text("A6"), text("B6")], ["0", "5", "Item 5"]);
        assert!(holds_source_in_corner(RangeRef::parse("B5").unwrap(), RangeRef::parse("B3:B5").unwrap()));
        assert!(holds_source_in_corner(RangeRef::parse("A:A").unwrap(), RangeRef::parse("A:C").unwrap()));
        assert!(holds_source_in_corner(RangeRef::parse("B5:C9").unwrap(), RangeRef::parse("B5:D13").unwrap()));
        assert!(holds_source_in_corner(RangeRef::parse("B1:B2").unwrap(), RangeRef::parse("A1:B4").unwrap()));
        assert!(!holds_source_in_corner(RangeRef::parse("A:A").unwrap(), RangeRef::parse("1:1").unwrap()));
    }
}
