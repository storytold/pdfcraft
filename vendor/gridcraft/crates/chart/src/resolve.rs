//! Pulls chart data (names, categories, values, formats, colours) out of a workbook.

use gridcraft_calc::evaluate;
use gridcraft_core::{CellRef, MAX_COLS, MAX_ROWS, RangeRef, Value, category_levels, number_to_text};
use gridcraft_model::{Chart, ChartKind, Theme, Workbook, style::apply_tint};
use gridcraft_numfmt::{NumberFormat, format_value};

use crate::{ChartData, Rgba, SeriesData, TrendlineData};

/// Cap on points per series (keeps whole-column references from exploding).
pub(crate) const MAX_POINTS: usize = 100_000;
/// Cap on series per chart.
pub(crate) const MAX_SERIES: usize = 255;
/// Cap on trendlines per chart (not per series: each one is a fit over a series' points).
pub(crate) const MAX_TRENDLINES: usize = 16;
/// Budget for series x points in one chart. When a chart would hold more, every series (and the
/// category list) keeps its first `MAX_TOTAL_POINTS / series` points; no series is dropped.
pub(crate) const MAX_TOTAL_POINTS: usize = 2_000_000;

/// Points kept per series (and categories) in a chart of `series` series: `MAX_POINTS`, or less
/// when `series x MAX_POINTS` would pass `MAX_TOTAL_POINTS`.
pub(crate) fn points_per_series(series: usize) -> usize {
    (MAX_TOTAL_POINTS / series.clamp(1, MAX_SERIES)).clamp(1, MAX_POINTS)
}

/// Tints used for series beyond the six theme accents: lighter, darker, lighter still…
const CYCLE_TINTS: [f64; 6] = [0.0, 0.4, -0.25, 0.6, -0.5, 0.2];
/// `series_color` repeats after this many colours (six accents × the tints).
pub(crate) const PALETTE_CYCLE: usize = 6 * CYCLE_TINTS.len();

/// Colour of the `i`-th series: theme accents 1–6, then tinted variations of them.
pub fn series_color(theme: &Theme, i: usize) -> Rgba {
    let accent = i % 6;
    let cycle = (i / 6) % CYCLE_TINTS.len();
    let base = theme.colors.get(4 + accent).copied().unwrap_or(0x156082);
    let rgb = [(base >> 16) as u8, (base >> 8) as u8, base as u8];
    let tint = CYCLE_TINTS.get(cycle).copied().unwrap_or(0.0);
    let [r, g, b] = apply_tint(rgb, tint);
    [r, g, b, 0xFF]
}

/// The first `n` series colours for `theme`.
pub fn default_palette(theme: &Theme, n: usize) -> Vec<Rgba> {
    (0..n.min(4096)).map(|i| series_color(theme, i)).collect()
}

/// Splits `Sheet!A1:B2` / `'My Sheet'!$A$1` / `A1:B2` into a sheet index and range.
fn parse_ref(wb: &Workbook, sheet: usize, formula: &str) -> Option<(usize, RangeRef)> {
    let f = formula.trim().trim_start_matches('=').trim();
    let (sh, addr) = match f.rfind('!') {
        Some(i) => {
            let name = f.get(..i)?.trim();
            let name = gridcraft_core::unquote_sheet(name);
            (wb.sheet_index(&name)?, f.get(i + 1..)?)
        }
        None => (sheet, f),
    };
    let r = RangeRef::parse(addr)?;
    Some((sh, r))
}

/// The parts of a union reference `(Sheet1!$A$1:$A$3,Sheet1!$A$5)`, or `None`.
fn union_parts(f: &str) -> Option<Vec<&str>> {
    let inner = f.strip_prefix('(')?.strip_suffix(')')?;
    let mut parts = Vec::new();
    let (mut depth, mut quoted, mut start) = (0i32, false, 0usize);
    for (i, c) in inner.char_indices() {
        match c {
            '\'' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => depth -= 1,
            ',' if !quoted && depth == 0 => {
                parts.push(inner.get(start..i)?);
                start = i + 1;
            }
            _ => {}
        }
        if depth < 0 {
            return None;
        }
    }
    parts.push(inner.get(start..)?);
    (parts.len() > 1).then_some(parts)
}

/// Evaluates a series formula to a flat list of values (row-major), capped. A union of
/// references (`(A1:A3,A5)`, as PivotCharts use) is read part by part.
fn eval_list(wb: &Workbook, sheet: usize, formula: &str) -> Vec<Value> {
    eval_list_in(wb, sheet, formula, MAX_POINTS, 0)
}

/// calc reads a reference of up to this many cells at its full size and trims a larger one to the
/// sheet's used range (`FULL_SIZE_CELLS` in `gridcraft_calc::eval`). A clipped reference must stay
/// above it, or the clip would turn a trimmed read into a full-size one with trailing blanks.
const CALC_FULL_SIZE_CELLS: u64 = 16_384;
/// Bounded references up to this many cells are read at their full size too
/// (`FULL_SIZE_BOUNDED_CELLS` in `gridcraft_calc::eval`).
const CALC_FULL_SIZE_BOUNDED_CELLS: u64 = 1_048_576;

/// `formula` as a plain reference (`Sheet!A1:A1000000`) cut after the rows that can still hold
/// one of the first `limit` points, so that calc never reads the rest; `None` when `formula` is
/// no reference or already small enough. Values are read row by row, and calc trims only the
/// bottom and right of a reference, so keeping `limit` rows keeps the first `limit` values.
fn clipped_reference(wb: &Workbook, sheet: usize, formula: &str, limit: usize) -> Option<String> {
    let (sh, mut r) = parse_ref(wb, sheet, formula)?;
    let cells = u64::from(r.height()).saturating_mul(u64::from(r.width()));
    // calc trims a block that runs to the sheet's edge, and any block over a column's worth, to
    // the used range. Do it here, or the clipped block below would be read at its full size.
    let to_edge = r.end.row == MAX_ROWS - 1 || r.end.col == MAX_COLS - 1;
    let trimmed = cells > CALC_FULL_SIZE_CELLS && (to_edge || cells > CALC_FULL_SIZE_BOUNDED_CELLS);
    let mut changed = false;
    if trimmed {
        let end = match wb.sheet(sh).and_then(|s| s.used_range()) {
            Some(u) => CellRef::new(r.end.row.min(u.end.row.max(r.start.row)), r.end.col.min(u.end.col.max(r.start.col))),
            None => r.start,
        };
        changed = end != r.end;
        r = RangeRef::new(r.start, end);
    }
    let prefix = formula.rfind('!').and_then(|i| formula.get(..=i)).unwrap_or("");
    let (h, w) = (u64::from(r.height()), u64::from(r.width()));
    if h.saturating_mul(w) > CALC_FULL_SIZE_CELLS {
        let keep = (limit as u64).max(CALC_FULL_SIZE_CELLS / w + 1);
        if h > keep {
            let end = CellRef::new(r.start.row.saturating_add(u32::try_from(keep).ok()?).saturating_sub(1), r.end.col);
            r = RangeRef::new(r.start, end);
            changed = true;
        }
    }
    changed.then(|| format!("{prefix}{r}"))
}

fn eval_list_in(wb: &Workbook, sheet: usize, formula: &str, limit: usize, depth: u32) -> Vec<Value> {
    let f = formula.trim();
    if f.is_empty() {
        return vec![];
    }
    if depth < 4
        && let Some(parts) = union_parts(f)
    {
        let mut out = Vec::new();
        for p in parts {
            if out.len() >= limit {
                break;
            }
            let rest = limit - out.len();
            out.extend(eval_list_in(wb, sheet, p, rest, depth + 1).into_iter().take(rest));
        }
        return out;
    }
    let clipped = clipped_reference(wb, sheet, f, limit);
    match evaluate(wb, sheet, CellRef::default(), clipped.as_deref().unwrap_or(f)) {
        Value::Array(a) => a.iter().take(limit).cloned().collect(),
        v => vec![v],
    }
}

/// Whether a series formula of `ch` (chart of `sheet`) calls a function whose result changes
/// between evaluations (`NOW`, `TODAY`, `RAND`, `RANDBETWEEN`, `RANDARRAY`), so a result resolved
/// earlier is stale. Defined names the formulas use (`=FreshClock`, `Sheet1!Rates`, a LAMBDA name)
/// are followed into their own formulas, the way the evaluator looks them up; each name once per
/// chart, so a cycle ends. Names nested deeper than [`MAX_NAME_DEPTH`], or more than
/// [`MAX_NAMES_FOLLOWED`] distinct names, count as volatile: a chart whose stability isn't
/// established is not cached.
pub fn has_volatile_series(wb: &Workbook, sheet: usize, ch: &Chart) -> bool {
    let mut seen = Seen::new();
    ch.series.iter().any(|s| {
        [s.name.as_deref(), s.categories.as_deref(), Some(s.values.as_str()), s.bubble_sizes.as_deref()]
            .into_iter()
            .flatten()
            .any(|f| formula_volatile(wb, sheet, f, 0, &mut seen))
    })
}

/// Names followed inside names before a formula counts as volatile.
const MAX_NAME_DEPTH: u32 = 8;
/// Distinct defined names followed per chart before the chart counts as volatile.
const MAX_NAMES_FOLLOWED: usize = 64;

/// Defined names already followed for one chart, each with the sheet its formula is read on.
type Seen<'a> = Vec<(&'a gridcraft_model::DefinedName, usize)>;

fn formula_volatile<'a>(wb: &'a Workbook, sheet: usize, f: &str, depth: u32, seen: &mut Seen<'a>) -> bool {
    if calls_volatile(f) {
        return true;
    }
    name_tokens(f).into_iter().any(|t| {
        let Some((n, at)) = defined_name(wb, sheet, t) else { return false };
        // Followed already, or being followed (a cycle): a volatile call in it is found there.
        if seen.iter().any(|&(m, s)| std::ptr::eq(m, n) && s == at) {
            return false;
        }
        if depth >= MAX_NAME_DEPTH || seen.len() >= MAX_NAMES_FOLLOWED {
            return true;
        }
        seen.push((n, at));
        formula_volatile(wb, at, &n.formula, depth + 1, seen)
    })
}

/// Whether formula text `f` calls one of the volatile functions by name.
fn calls_volatile(f: &str) -> bool {
    const VOLATILE: [&str; 5] = ["NOW", "TODAY", "RAND", "RANDBETWEEN", "RANDARRAY"];
    // Formulas read from a file may carry the `_xlfn.` prefix of newer functions.
    let up = f.to_ascii_uppercase().replace("_XLFN.", "");
    VOLATILE.iter().any(|name| {
        up.match_indices(name).any(|(i, _)| {
            let before = up[..i].chars().next_back();
            let after = up[i + name.len()..].trim_start();
            before.is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '.')) && after.starts_with('(')
        })
    })
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.' || c == '\\'
}

/// End of the identifier starting at byte `from` of `f`.
fn ident_end(f: &str, from: usize) -> usize {
    f[from..].char_indices().find(|&(_, c)| !is_ident_char(c)).map_or(f.len(), |(j, _)| from + j)
}

/// End (past the closing quote) of the `q`-quoted text opening at byte `from`; a doubled quote
/// is an escaped one. `None` when it is not closed.
fn quoted_end(f: &str, from: usize, q: char) -> Option<usize> {
    let mut i = from + q.len_utf8();
    loop {
        let j = i + f[i..].find(q)?;
        if f[j + q.len_utf8()..].starts_with(q) {
            i = j + 2 * q.len_utf8();
        } else {
            return Some(j + q.len_utf8());
        }
    }
}

/// The words of formula text `f` that may be defined names: `Name`, `Sheet1!Name`,
/// `'Bob''s'!Name`. String literals and numbers are skipped; function names, column letters and
/// the like come along too and simply aren't found as names.
fn name_tokens(f: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(c) = f[i..].chars().next() {
        let start = i;
        let end = match c {
            '"' => {
                i = quoted_end(f, i, '"').unwrap_or(f.len());
                continue;
            }
            '\'' => quoted_end(f, i, '\'').unwrap_or(f.len()),
            c if is_ident_char(c) => ident_end(f, i),
            c => {
                i += c.len_utf8();
                continue;
            }
        };
        i = end;
        if f[end..].starts_with('!') {
            let e = ident_end(f, end + 1);
            if e > end + 1 {
                out.push(&f[start..e]);
            }
            i = e;
        } else if c != '\'' && !c.is_ascii_digit() {
            out.push(&f[start..end]);
        }
    }
    out
}

/// The defined name `t` (`Name` or `Sheet1!Name`) seen from `sheet`, and the sheet its own
/// formula is read on.
fn defined_name<'a>(wb: &'a Workbook, sheet: usize, t: &str) -> Option<(&'a gridcraft_model::DefinedName, usize)> {
    // `'Bob''s'!Name`: the sheet is unquoted as everywhere else (#2012).
    let (at, bare) = match gridcraft_core::split_sheet(t) {
        (Some(s), bare) => (wb.sheet_index(&s)?, bare),
        (None, bare) => (sheet, bare),
    };
    wb.name(bare, at).map(|n| (n, n.scope.unwrap_or(at)))
}

fn to_num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) if n.is_finite() => Some(*n),
        _ => None,
    }
}

fn nums(wb: &Workbook, sheet: usize, formula: &str, limit: usize) -> Vec<Option<f64>> {
    eval_list_in(wb, sheet, formula, limit, 0).iter().map(to_num).collect()
}

/// Number format code of the first cell of a reference ("General" when not a reference).
fn ref_format(wb: &Workbook, sheet: usize, formula: &str) -> String {
    parse_ref(wb, sheet, formula)
        .and_then(|(sh, r)| wb.sheet(sh).map(|s| wb.styles.get(s.style_id(r.start)).num_fmt.0.to_string()))
        .unwrap_or_else(|| "General".into())
}

fn value_text(v: &Value) -> String {
    match v {
        Value::Empty => String::new(),
        Value::Number(n) => number_to_text(*n),
        Value::Text(t) => t.to_string(),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        Value::Error(e) => e.as_str().into(),
        Value::Array(a) => a.get(0, 0).map(value_text).unwrap_or_default(),
    }
}

/// Characters kept of one category label: an axis label never shows more (labels are fitted to
/// the axis), and copying more only costs memory.
const MAX_LABEL_CHARS: usize = 255;
/// Bytes of category label text kept per series. One long text broadcast by a short formula
/// (`IF(SEQUENCE(100000), long_text)`) is a single shared value, but turning every element into
/// its own label copied it 100,000 times (gigabytes). Labels past the budget are empty.
const MAX_CATEGORY_BYTES: usize = 8 << 20;

/// At most [`MAX_LABEL_CHARS`] characters and `room` bytes of `s`, cut at a character boundary.
fn clip_label(s: &str, room: usize) -> String {
    let mut end = 0;
    for (n, (i, c)) in s.char_indices().enumerate() {
        if n >= MAX_LABEL_CHARS || i + c.len_utf8() > room {
            break;
        }
        end = i + c.len_utf8();
    }
    s.get(..end).unwrap_or_default().to_string()
}

/// The label of category `i` (value `v`) in at most `room` bytes: formatted with its source
/// cell's number format when it is a number.
fn category_label(wb: &Workbook, src: Option<(&gridcraft_model::Sheet, RangeRef)>, i: usize, v: &Value, room: usize) -> String {
    if room == 0 {
        return String::new(); // nothing fits: skip the number formatting
    }
    if let (Some((s, r)), Value::Number(_)) = (src, v) {
        let w = r.width().max(1) as usize;
        let at = r.start.offset((i / w) as i64, (i % w) as i64);
        if let Some(c) = at {
            let code = &wb.styles.get(s.style_id(c)).num_fmt.0;
            if !code.eq_ignore_ascii_case("general") {
                return clip_label(&format_value(v, &NumberFormat::parse_cached(code), wb.date_system).text, room);
            }
        }
    }
    // Text is clipped before it is copied: the source may be one long shared string.
    let mut v = v;
    while let Value::Array(a) = v {
        v = a.get(0, 0).unwrap_or(&Value::Empty);
    }
    match v {
        Value::Text(t) => clip_label(t, room),
        _ => clip_label(&value_text(v), room),
    }
}

/// Category labels, formatted with each source cell's number format (dates, currency…), and
/// the outer levels of a multi-level axis: a category range wider than a column of values (or
/// taller than a row of values) groups its innermost labels, year beside month. Each outer level
/// has one entry per category, non-empty where a group starts, as the cells are.
fn category_table(wb: &Workbook, sheet: usize, formula: &str, values: &str, limit: usize) -> (Vec<String>, Vec<Vec<String>>) {
    let cells: Option<(usize, RangeRef)> = parse_ref(wb, sheet, formula);
    let value_range = parse_ref(wb, sheet, values).map(|(_, r)| r);
    // Bound the category dimension by `limit` and read only the cells the levels use (the
    // innermost label line and up to three outer ones), so that every kept category keeps its
    // cells in every level. Counting raw cells would cut a two-level axis in the middle.
    let levels = cells.and_then(|(_, r)| category_levels(&r, value_range.as_ref()));
    let (cells, read) = match (cells, levels) {
        (Some((sh, r)), Some(lv)) => {
            let cut = u32::try_from(limit.saturating_sub(1)).unwrap_or(u32::MAX);
            let outer = lv.outer;
            let end = if lv.by_columns {
                CellRef::new(r.end.row.min(r.start.row.saturating_add(cut)), r.end.col)
            } else {
                CellRef::new(r.end.row, r.end.col.min(r.start.col.saturating_add(cut)))
            };
            let start = if lv.by_columns {
                CellRef::new(r.start.row, r.start.col.max(end.col.saturating_sub(outer)))
            } else {
                CellRef::new(r.start.row.max(end.row.saturating_sub(outer)), r.start.col)
            };
            let r = RangeRef::new(start, end);
            let prefix = formula.rfind('!').and_then(|i| formula.get(..=i)).unwrap_or("");
            let cap = (r.width() as usize).saturating_mul(r.height() as usize);
            (Some((sh, r)), Some((format!("{prefix}{r}"), cap)))
        }
        (c, _) => (c, None),
    };
    let vals = match &read {
        Some((f, cap)) => eval_list_in(wb, sheet, f, *cap, 0),
        None => eval_list_in(wb, sheet, formula, limit, 0),
    };
    let src = cells.and_then(|(sh, r)| wb.sheet(sh).map(|s| (s, r)));
    // The retained levels are known before any label is charged: a cell on a discarded level
    // gets no label and spends none of the budget. `None`: a flat list (every cell is a label).
    let layout = src.and_then(|(_, r)| {
        let levels = levels?;
        let (w, h) = (r.width().max(1) as usize, r.height().max(1) as usize);
        (vals.len() == w.saturating_mul(h)).then_some((levels, w, h)) // else truncated or not a plain block
    });
    let kept =
        |i: usize| layout.is_none_or(|(l, w, h)| if l.by_columns { i % w + 1 + l.outer as usize >= w } else { i / w + 1 + l.outer as usize >= h });
    let mut room = MAX_CATEGORY_BYTES;
    let texts: Vec<String> = vals
        .iter()
        .enumerate()
        .map(|(i, v)| {
            if !kept(i) {
                return String::new();
            }
            let label = category_label(wb, src, i, v, room);
            room = room.saturating_sub(label.len());
            label
        })
        .collect();
    let Some((levels, w, h)) = layout else { return (texts, vec![]) };
    let at = |row: usize, col: usize| texts.get(row * w + col).cloned().unwrap_or_default();
    let column = |c: usize| (0..h).map(|row| at(row, c)).collect::<Vec<String>>();
    let row = |rr: usize| (0..w).map(|c| at(rr, c)).collect::<Vec<String>>();
    let outer = levels.outer as usize;
    if levels.by_columns {
        (column(w - 1), (1..=outer).filter_map(|k| (w - 1).checked_sub(k)).map(column).collect())
    } else {
        (row(h - 1), (1..=outer).filter_map(|k| (h - 1).checked_sub(k)).map(row).collect())
    }
}

/// Whether `t` names a defined name seen from `sheet` (`Name`, or `Sheet1!Name` looked up on
/// that sheet), the way the evaluator looks names up.
fn is_defined_name(wb: &Workbook, sheet: usize, t: &str) -> bool {
    defined_name(wb, sheet, t).is_some()
}

/// A series name: a reference/formula or a defined name (`[0]!Title` in a file) is evaluated,
/// a quoted or other bare literal is used as is.
fn series_name(wb: &Workbook, sheet: usize, name: &str) -> String {
    let t = name.trim();
    if let Some(q) = t.strip_prefix('"').and_then(|q| q.strip_suffix('"')) {
        return q.replace("\"\"", "\"");
    }
    if t.starts_with('=') || parse_ref(wb, sheet, t).is_some() && t.contains('!') || is_defined_name(wb, sheet, t) {
        let vals = eval_list(wb, sheet, t);
        let parts: Vec<String> = vals.iter().map(value_text).filter(|s| !s.is_empty()).take(8).collect();
        return parts.join(" ");
    }
    t.to_string()
}

/// Evaluates a chart's series formulas: names, values, categories (default "1", "2", …), X
/// values (scatter/bubble), bubble sizes, number formats and colours (explicit, else the
/// default palette derived from the workbook theme accents).
pub fn resolve(wb: &Workbook, sheet: usize, chart: &Chart) -> ChartData {
    let numeric_x = matches!(chart.kind, ChartKind::Scatter | ChartKind::ScatterLines | ChartKind::Bubble);
    let mut series = Vec::new();
    let mut categories: Option<Vec<String>> = None;
    let limit = points_per_series(chart.series.len());
    let mut trend_left = MAX_TRENDLINES;
    let mut category_levels = Vec::new();
    for (i, s) in chart.series.iter().take(MAX_SERIES).enumerate() {
        let values = nums(wb, sheet, &s.values, limit);
        let name = s.name.as_deref().map(|n| series_name(wb, sheet, n)).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("Series{}", i + 1));
        let color =
            s.color.as_ref().and_then(|c| c.resolve(&wb.theme)).map(|[r, g, b]| [r, g, b, 0xFF]).unwrap_or_else(|| series_color(&wb.theme, i));
        let kind = chart.series_kind(i);
        let x = match (&s.categories, numeric_x || matches!(kind, ChartKind::Scatter | ChartKind::ScatterLines | ChartKind::Bubble)) {
            // X values with text in them (labels) are ignored: the points are numbered 1..n.
            (Some(f), true) => {
                Some(eval_list_in(wb, sheet, f, limit, 0)).filter(|xs| !xs.iter().any(Value::is_text)).map(|xs| xs.iter().map(to_num).collect())
            }
            _ => None,
        };
        if categories.is_none()
            && let Some(f) = &s.categories
        {
            let (mut c, outer) = category_table(wb, sheet, f, &s.values, limit);
            c.truncate(limit);
            if !c.is_empty() {
                categories = Some(c);
                category_levels = outer;
            }
        }
        let sizes = s.bubble_sizes.as_deref().map(|f| nums(wb, sheet, f, limit));
        let trendlines = s
            .trendlines
            .iter()
            .take(trend_left)
            .map(|t| TrendlineData {
                spec: t.clone(),
                color: t.color.as_ref().and_then(|c| c.resolve(&wb.theme)).map(|[r, g, b]| [r, g, b, 0xFF]).unwrap_or(color),
            })
            .collect::<Vec<_>>();
        trend_left = trend_left.saturating_sub(trendlines.len());
        series.push(SeriesData {
            name,
            values,
            x,
            sizes,
            color,
            kind,
            secondary: s.secondary,
            smooth: s.smooth,
            trendlines,
            number_format: ref_format(wb, sheet, &s.values),
        });
    }
    let n = series.iter().map(|s| s.values.len()).max().unwrap_or(0);
    let categories = categories.unwrap_or_else(|| (1..=n).map(|i| i.to_string()).collect());
    let palette = default_palette(&wb.theme, n.min(PALETTE_CYCLE));
    ChartData { categories, category_levels, series, palette }
}

#[cfg(test)]
mod label_budget_tests {
    use gridcraft_model::Workbook;

    /// Security: category labels are clipped before they are copied, with one byte budget per
    /// series. A formula broadcasting one long text to 100,000 categories used to copy it
    /// 100,000 times (3.3 GB).
    #[test]
    fn category_labels_share_one_byte_budget() {
        let wb = Workbook::new();
        let (texts, _) = super::category_table(&wb, 0, r#"=IF(SEQUENCE(100000),REPT("x",32767))"#, "", super::MAX_POINTS);
        assert_eq!(texts.len(), 100_000);
        assert_eq!(texts.first().map(String::len), Some(super::MAX_LABEL_CHARS));
        assert!(texts.iter().map(String::len).sum::<usize>() <= super::MAX_CATEGORY_BYTES);
        // Labels are cut at character boundaries.
        assert_eq!(super::clip_label("aé", 2), "a");
        assert_eq!(super::clip_label("aé", 3), "aé");
    }

    /// Cells on discarded outer levels (only the innermost labels and three outer levels are
    /// kept) spend none of the byte budget.
    #[test]
    fn discarded_levels_do_not_spend_the_label_budget() {
        let mut wb = Workbook::new();
        let rows = 8000u32;
        if let Some(sh) = wb.sheet_mut(0) {
            for r in 0..rows {
                for c in 0..5 {
                    sh.set_value(gridcraft_core::CellRef::new(r, c), gridcraft_core::Value::text("x".repeat(255)));
                }
            }
        }
        // Five columns: column A is a fourth outer level and is discarded.
        let (inner, outer) = super::category_table(&wb, 0, "=A1:E8000", "", super::MAX_POINTS);
        assert_eq!(outer.len(), 3);
        assert_eq!(inner.last().map(String::len), Some(super::MAX_LABEL_CHARS));
        assert_eq!(outer.first().and_then(|l| l.last()).map(String::len), Some(super::MAX_LABEL_CHARS));
    }
}

#[cfg(test)]
mod tests {
    use gridcraft_model::Workbook;

    use super::*;

    fn book(rows: u32, cols: u32) -> Workbook {
        let mut wb = Workbook::new();
        if let Some(s) = wb.sheet_mut(0) {
            s.name = "My Sheet".into();
            for r in 0..rows {
                for c in 0..cols {
                    s.set_value(CellRef::new(r, c), Value::Number(f64::from(r * 10 + c)));
                }
            }
        }
        wb
    }

    #[test]
    fn a_long_reference_is_clipped_before_it_is_evaluated() {
        let wb = book(120_000, 3);
        let rows = |f: &str, limit| {
            let g = clipped_reference(&wb, 0, f, limit)?;
            let (_, r) = parse_ref(&wb, 0, &g)?;
            Some((g, u64::from(r.height())))
        };
        // 1,000,000 rows: at most `MAX_POINTS` of them are asked for.
        assert_eq!(rows("'My Sheet'!$A$1:$A$1000000", MAX_POINTS), Some(("'My Sheet'!A1:A100000".to_string(), 100_000)));
        assert_eq!(rows("A5:A1048576", MAX_POINTS).map(|r| r.1), Some(100_000));
        assert_eq!(rows("A:A", 50_000).map(|r| r.1), Some(50_000));
        // Never below what keeps calc trimming the clipped reference like the whole one.
        assert_eq!(rows("A1:A1000000", 100).map(|r| r.1), Some(16_385));
        // Wide references keep whole rows.
        assert_eq!(rows("A1:C1000000", MAX_POINTS).map(|r| r.1), Some(100_000));
        // Small references, single cells and anything that isn't a plain reference are left alone.
        assert_eq!(rows("A1:A9000", MAX_POINTS), None);
        assert_eq!(rows("A1:A20000", MAX_POINTS), None);
        assert_eq!(rows("B2", MAX_POINTS), None);
        assert_eq!(rows("SUM(A1:A1000000)", MAX_POINTS), None);
        assert_eq!(rows("{1,2,3}", MAX_POINTS), None);
    }

    #[test]
    fn clipping_does_not_change_the_points() {
        for (rows, cols) in [(10, 1), (30_000, 1), (30_000, 2), (3, 4)] {
            let wb = book(rows, cols);
            for f in ["'My Sheet'!A1:A1000000", "'My Sheet'!$A$3:$B$1048576", "A:A", "A1:C1000000", "(A1:A1000000,B1:B1000000)", "A2:A20000"] {
                for limit in [100, 7_843, 25_000, MAX_POINTS] {
                    let clipped = eval_list_in(&wb, 0, f, limit, 0);
                    let whole: Vec<Value> = match evaluate(&wb, 0, CellRef::default(), f) {
                        Value::Array(a) => a.iter().take(limit).cloned().collect(),
                        v => vec![v],
                    };
                    if f.starts_with('(') {
                        // A union is read part by part; each part keeps its own order.
                        assert!(clipped.len() <= limit);
                        continue;
                    }
                    assert_eq!(clipped, whole, "{f} limit {limit} on {rows}x{cols}");
                }
            }
        }
    }
}
