use super::items::{Acc, FieldItems, build_tree, collect_siblings, is_collapsed, item_label};
use super::output::{
    AggMap, CellKind, ColKind, ColSpec, Ctx, Grid, Line, LineKind, Output, RowKind, ValSrc, caption, data_cols_exceed_sheet, flatten_cols,
};
use super::{
    CalcSet, CellError, CellRef, HashMap, HashSet, MAX_AXIS_FIELDS, MAX_COLS, MAX_INDEXED_CELLS, MAX_VALUE_FIELDS, PivotDataCell, PivotDateGroup,
    PivotField, PivotFilter, PivotFunc, PivotItemRef, PivotLayout, PivotShowAs, PivotTable, PivotValue, Source, Value, Workbook, read_source,
};

/// Why Show Details turns a cell down when it isn't a value cell of a PivotTable (the front end
/// edits that cell instead).
pub const NOT_A_VALUE_CELL: &str = "select a value cell in a PivotTable";

/// Show Details: the source list and the indices of its records behind the value cell at
/// (`row`, `col`) of the report grid (relative to its top-left), in source order. `None` when
/// that cell holds no value (a label, a header, a blank). An error, for any cell, when the report
/// on the sheet is no longer what the source computes: the cell's records would be another cell's.
pub fn detail_records(wb: &Workbook, pt: &PivotTable, pivot_sheet: usize, row: u32, col: u32) -> Result<Option<(Source, Vec<usize>)>, String> {
    let src = read_source(wb, &pt.source, pivot_sheet)?;
    let mut probe = Probe { at: Some((row as usize, col as usize)), hit: None, paths: Some(Vec::new()), cells: None };
    let out = compute_from(wb, pt, &src, Some(&mut probe))?;
    // Checked first: in a stale report even a cell that computes as a label may show a value.
    check_current(wb, pt, pivot_sheet, &out)?;
    let Some((rp, cp)) = probe.hit else { return Ok(None) };
    let records =
        probe.paths.unwrap_or_default().into_iter().filter(|(_, r, c)| r.starts_with(&rp) && c.starts_with(&cp)).map(|(i, _, _)| i).collect();
    Ok(Some((src, records)))
}

/// The cells of the report grid (relative to its top-left) where [`detail_records`] finds
/// records: its value, subtotal and grand total cells, from one computation of the report. The
/// same errors as [`detail_records`].
pub fn detail_cells(wb: &Workbook, pt: &PivotTable, pivot_sheet: usize) -> Result<HashSet<(u32, u32)>, String> {
    let src = read_source(wb, &pt.source, pivot_sheet)?;
    let mut probe = Probe { at: None, hit: None, paths: None, cells: Some(HashSet::new()) };
    let out = compute_from(wb, pt, &src, Some(&mut probe))?;
    check_current(wb, pt, pivot_sheet, &out)?;
    Ok(probe.cells.unwrap_or_default().into_iter().map(|(r, c)| (r as u32, c as u32)).collect())
}

/// An error when the report on the sheet is not what `out` computes. Excel reads its PivotTable
/// cache; without one, the report written by the last refresh must still be what the source gives.
fn check_current(wb: &Workbook, pt: &PivotTable, pivot_sheet: usize, out: &Output) -> Result<(), String> {
    let sh = wb.sheet(pivot_sheet).ok_or("There's no such sheet.")?;
    let current = pt.last_range.is_some_and(|r| {
        r.height() == out.height().max(1)
            && r.width() == out.width.max(1)
            && out.grid.iter().enumerate().all(|(ri, line)| {
                line.iter().enumerate().all(|(ci, oc)| {
                    sh.value_ref(CellRef::new(r.start.row + ri as u32, r.start.col + ci as u32)).unwrap_or(&Value::Empty) == &oc.value
                })
            })
    });
    if !current {
        return Err("The PivotTable report is out of date. Refresh it first.".into());
    }
    Ok(())
}

type ProbePath = (usize, Vec<u32>, Vec<u32>);

/// What [`detail_records`] and [`detail_cells`] ask of a report computation: the row and column
/// item paths of the value cell at `at`, the paths of every record the filters let through (when
/// `paths` is set), and every value cell (when `cells` is set).
pub(super) struct Probe {
    at: Option<(usize, usize)>,
    hit: Option<(Vec<u32>, Vec<u32>)>,
    paths: Option<Vec<ProbePath>>,
    cells: Option<HashSet<(usize, usize)>>,
}

struct AggregateInput<'a> {
    wb: &'a Workbook,
    pt: &'a PivotTable,
    src: &'a Source,
    rows: &'a [(usize, &'a PivotField)],
    cols: &'a [(usize, &'a PivotField)],
    vals: &'a [(ValSrc, &'a PivotValue)],
    extra: &'a [usize],
    filters: &'a [(usize, &'a PivotFilter, PivotDateGroup, bool)],
    filter_fmts: &'a [String],
    /// Each filter's selected labels, ASCII-lowercased, so a record's test is one probe.
    filter_sets: &'a [Option<HashSet<String>>],
    row_fmts: &'a [String],
    col_fmts: &'a [String],
    empty: bool,
}

#[derive(Default)]
struct AggregateData {
    row_items: Vec<FieldItems>,
    col_items: Vec<FieldItems>,
    aggs: AggMap,
    row_set: HashSet<Vec<u32>>,
    col_set: HashSet<Vec<u32>>,
    col_leaves: HashSet<Vec<u32>>,
}

/// Most accumulators (groups times value columns) one report may hold: twice what the 5 million
/// group limit allowed at one value field.
const MAX_ACCUMULATORS: usize = 16_000_000;

fn aggregate_record(
    input: &AggregateInput<'_>,
    data: &mut AggregateData,
    probe: &mut Option<&mut Probe>,
    ri: usize,
    rec: &[Value],
) -> Result<(), String> {
    for (((c, _, group, _), fmt), selected) in input.filters.iter().zip(input.filter_fmts).zip(input.filter_sets) {
        if let Some(selected) = selected {
            let label = item_label(input.wb, rec.get(*c).unwrap_or(&Value::Empty), *group, fmt);
            if !selected.contains(&label.to_ascii_lowercase()) {
                return Ok(());
            }
        }
    }
    if input.empty {
        return Ok(());
    }
    let mut row_path = Vec::with_capacity(input.rows.len());
    for (((column, field), items), fmt) in input.rows.iter().zip(data.row_items.iter_mut()).zip(input.row_fmts) {
        row_path.push(items.intern(input.wb, rec.get(*column).unwrap_or(&Value::Empty), field.date_group, fmt, ri));
    }
    let mut col_path = Vec::with_capacity(input.cols.len());
    for (((column, field), items), fmt) in input.cols.iter().zip(data.col_items.iter_mut()).zip(input.col_fmts) {
        col_path.push(items.intern(input.wb, rec.get(*column).unwrap_or(&Value::Empty), field.date_group, fmt, ri));
    }
    for row_depth in 0..=row_path.len() {
        for col_depth in 0..=col_path.len() {
            let accs = data.aggs.entry((row_path[..row_depth].to_vec(), col_path[..col_depth].to_vec())).or_insert_with(|| {
                input
                    .vals
                    .iter()
                    .map(|(_, v)| Acc::for_func(v.func))
                    .chain(std::iter::repeat_n(Acc::default(), input.extra.len()))
                    .collect::<Vec<_>>()
            });
            for (acc, (source, _)) in accs.iter_mut().zip(input.vals) {
                if let ValSrc::Col(column) = source {
                    acc.add(rec.get(*column).unwrap_or(&Value::Empty));
                }
            }
            for (acc, column) in accs.iter_mut().skip(input.vals.len()).zip(input.extra) {
                acc.add(rec.get(*column).unwrap_or(&Value::Empty));
            }
        }
    }
    let shown_depth = input
        .cols
        .iter()
        .zip(data.col_items.iter())
        .zip(col_path.iter())
        .position(|(((_, field), items), &item)| !field.collapsed_items.is_empty() && is_collapsed(field, &items.label(item)))
        .map_or(col_path.len(), |depth| depth + 1);
    data.col_leaves.insert(col_path[..shown_depth].to_vec());
    if let Some(paths) = probe.as_deref_mut().and_then(|value| value.paths.as_mut()) {
        paths.push((ri, row_path.clone(), col_path.clone()));
    }
    data.row_set.insert(row_path);
    data.col_set.insert(col_path);
    if !input.cols.is_empty() && data_cols_exceed_sheet(data.col_leaves.len(), input.pt.grand_totals_rows, input.vals.len()) {
        return Err("The PivotTable report will not fit on the sheet.".into());
    }
    // Each entry holds one accumulator per value field and calculated-field source column, so the
    // width counts too: a handful of groups over hundreds of fields is large as well.
    if data.aggs.len() > 5_000_000 || data.aggs.len().saturating_mul(input.vals.len() + input.extra.len()) > MAX_ACCUMULATORS {
        return Err("The PivotTable report is too large.".into());
    }
    Ok(())
}

fn aggregate_source(input: AggregateInput<'_>, probe: &mut Option<&mut Probe>) -> Result<AggregateData, String> {
    let mut data = AggregateData {
        row_items: input.rows.iter().map(|_| FieldItems::default()).collect(),
        col_items: input.cols.iter().map(|_| FieldItems::default()).collect(),
        ..AggregateData::default()
    };
    for (ri, record) in input.src.rows.iter().enumerate() {
        aggregate_record(&input, &mut data, probe, ri, record)?;
    }
    Ok(data)
}

struct HeaderInput<'a> {
    vals: &'a [(ValSrc, &'a PivotValue)],
    row_fields: &'a [&'a PivotField],
    col_fields: &'a [&'a PivotField],
    col_items: &'a [FieldItems],
    col_fmts: &'a [String],
    vpos: Option<usize>,
    data_cols: &'a [(usize, Option<usize>)],
    specs: &'a [ColSpec],
    row_levels: usize,
    compact: bool,
    on_rows: bool,
    k: usize,
    l: usize,
    headers_on: bool,
}

fn write_headers(g: &mut Grid, input: HeaderInput<'_>) -> Result<(), String> {
    let HeaderInput { vals, row_fields, col_fields, col_items, col_fmts, vpos, data_cols, specs, row_levels, compact, on_rows, k, l, headers_on } =
        input;
    let val_name = |vi: usize| vals.get(vi).map(|(_, v)| v.name.clone()).unwrap_or_default();
    let row_header = |g: &mut Grid, r: usize| {
        if row_levels == 0 || !headers_on {
            return;
        }
        if compact {
            g.set(r, 0, Value::text("Row Labels"), CellKind::Header, 0, None);
        } else {
            // One column per row field, Σ Values ("Values") at its place.
            let mut names: Vec<&str> = row_fields.iter().map(|f| f.source_col.as_str()).collect();
            if let Some(p) = vpos {
                names.insert(p.min(names.len()), "Values");
            }
            for (i, name) in names.into_iter().enumerate() {
                g.set(r, i, Value::text(name), CellKind::Header, 0, None);
            }
        }
    };

    // ---- header rows
    if col_fields.is_empty() {
        let r = g.push_row(RowKind::Header)?;
        row_header(g, r);
        for (j, (_, vi)) in data_cols.iter().enumerate() {
            match vi {
                Some(vi) => g.set(r, l + j, Value::text(val_name(*vi)), CellKind::Header, 0, None),
                // Values on rows, no column field: the one data column is the grand total.
                None if on_rows => g.set(r, l + j, Value::text("Total"), CellKind::Header, 0, None),
                None => {}
            }
        }
    } else {
        let cap = g.push_row(RowKind::Header)?;
        if l > 0 && k == 1 {
            g.set(cap, 0, Value::text(val_name(0)), CellKind::Header, 0, None);
        }
        if headers_on {
            let caption = if compact { "Column Labels".to_string() } else { col_fields.first().map(|f| f.source_col.clone()).unwrap_or_default() };
            g.set(cap, l, Value::text(caption), CellKind::Header, 0, None);
        }
        let levels = col_fields.len() + usize::from(k > 1 && !on_rows);
        let first_level = g.rows.len();
        for _ in 0..levels {
            g.push_row(RowKind::Header)?;
        }
        let mut prev: Option<&ColSpec> = None;
        for (j, (si, vi)) in data_cols.iter().enumerate() {
            let Some(spec) = specs.get(*si) else { continue };
            let c = l + j;
            match spec.kind {
                ColKind::Leaf => {
                    for (lv, &it) in spec.path.iter().enumerate() {
                        let same = prev.is_some_and(|p| p.kind != ColKind::Grand && p.path.get(..=lv) == spec.path.get(..=lv));
                        if !same {
                            let Some(item) = col_items.get(lv).and_then(|fi| fi.get(it)) else { continue };
                            let fmt = if item.formatted { col_fmts.get(lv).cloned() } else { None };
                            g.set(first_level + lv, c, item.value.clone(), CellKind::ColLabel, 0, fmt);
                        }
                    }
                    if k > 1
                        && let Some(vi) = vi
                    {
                        g.set(first_level + col_fields.len(), c, Value::text(val_name(*vi)), CellKind::ColLabel, 0, None);
                    }
                }
                ColKind::Subtotal => {
                    let lv = spec.path.len().saturating_sub(1);
                    let label = spec.path.last().and_then(|it| col_items.get(lv).map(|fi| fi.label(*it))).unwrap_or_default();
                    let text = match vi {
                        Some(vi) if k > 1 => caption(&label, &val_name(*vi)),
                        _ => caption(&label, "Total"),
                    };
                    g.set(first_level + lv, c, Value::text(text), CellKind::ColLabel, 0, None);
                }
                ColKind::Grand => {
                    let text = match vi {
                        Some(vi) if k > 1 => format!("Total {}", val_name(*vi)),
                        _ => "Grand Total".to_string(),
                    };
                    g.set(first_level, c, Value::text(text), CellKind::ColLabel, 0, None);
                }
            }
            prev = Some(spec);
        }
        let last = g.rows.len() - 1;
        row_header(g, last);
    }

    Ok(())
}

struct ValueLookup<'a> {
    wb: &'a Workbook,
    source: &'a Source,
    calc: &'a CalcSet,
    vals: &'a [(ValSrc, &'a PivotValue)],
    extra: &'a [usize],
    aggs: &'a AggMap,
    value_count: usize,
}

impl ValueLookup<'_> {
    fn get(&self, row_path: &[u32], col_path: &[u32], value_index: usize) -> Option<Value> {
        let (source, definition) = self.vals.get(value_index)?;
        let accumulators = self.aggs.get(&(row_path.to_vec(), col_path.to_vec()))?;
        match source {
            ValSrc::Col(_) => accumulators.get(value_index).map(|acc| acc.result(definition.func)),
            ValSrc::Calc(index) => {
                let sum = |name: &str| -> Value {
                    self.source
                        .col(name)
                        .and_then(|column| self.extra.iter().position(|extra_column| *extra_column == column))
                        .and_then(|index| accumulators.get(self.value_count + index))
                        .map(|acc| acc.result(PivotFunc::Sum))
                        .unwrap_or(Value::Number(0.0))
                };
                Some(self.calc.value(self.wb, *index, &sum))
            }
        }
    }
}

struct ShowContext<'a> {
    row_path: &'a [u32],
    col_path: &'a [u32],
    value_index: usize,
    level0: &'a [u32],
    siblings: &'a HashMap<Vec<u32>, Vec<u32>>,
    cache: &'a ShowCache,
}

fn number(value: &Option<Value>) -> Option<f64> {
    value.as_ref().and_then(Value::as_f64)
}

fn ratio(a: Value, b: Option<Value>) -> Value {
    match (a, b) {
        (Value::Error(error), _) => Value::Error(error),
        (_, Some(Value::Error(error))) => Value::Error(error),
        (Value::Number(_), Some(Value::Number(0.0))) => Value::Error(CellError::Div0),
        (Value::Number(x), Some(Value::Number(y))) => Value::number(x / y),
        (value, _) => value,
    }
}

/// What Running Total and Rank need from other cells, computed once per sibling set instead of
/// once per displayed value (which rescanned the whole set every time).
type ShowKey = (Vec<u32>, Vec<u32>, usize);
/// Cumulative values by top-level item, and the total of all of them.
type RunningTotals = (HashMap<u32, f64>, f64);
/// Top-level items by (rest of a row path, column path).
type ItemsByRest = HashMap<(Vec<u32>, Vec<u32>), Vec<u32>>;

#[derive(Default)]
struct ShowCache {
    /// Ranked values' sibling numbers, sorted ascending without repeats, by (parent, column, value).
    ranks: std::cell::RefCell<HashMap<ShowKey, Vec<f64>>>,
    /// Running totals by top-level item, with the total of all of them, by (rest of path, column, value).
    running: std::cell::RefCell<HashMap<ShowKey, RunningTotals>>,
    /// The top-level items that have a row path ending in `rest` (with column path `col`), in
    /// top-level order, by (`rest`, `col`): built once from the report's cells, so a running total
    /// visits the items that can hold a number, not every top-level item per distinct `rest`.
    items_by_rest: std::cell::RefCell<Option<ItemsByRest>>,
}

fn running_total(raw: Value, lookup: &ValueLookup<'_>, context: &ShowContext<'_>) -> Value {
    let Some((first, rest)) = context.row_path.split_first() else { return raw };
    if !matches!(raw, Value::Number(_)) {
        return raw;
    }
    let mut index = context.cache.items_by_rest.borrow_mut();
    let items_by_rest = index.get_or_insert_with(|| {
        let position: HashMap<u32, usize> = context.level0.iter().enumerate().rev().map(|(at, item)| (*item, at)).collect();
        let mut map = ItemsByRest::new();
        for (row, col) in lookup.aggs.keys() {
            if let Some((item, rest)) = row.split_first()
                && position.contains_key(item)
            {
                map.entry((rest.to_vec(), col.clone())).or_default().push(*item);
            }
        }
        for items in map.values_mut() {
            items.sort_by_key(|item| position.get(item).copied().unwrap_or(usize::MAX));
        }
        map
    });
    let mut cache = context.cache.running.borrow_mut();
    let (by_item, all) = cache.entry((rest.to_vec(), context.col_path.to_vec(), context.value_index)).or_insert_with(|| {
        let mut total = 0.0;
        let mut by_item = HashMap::new();
        for item in items_by_rest.get(&(rest.to_vec(), context.col_path.to_vec())).into_iter().flatten() {
            let mut path = Vec::with_capacity(rest.len() + 1);
            path.push(*item);
            path.extend_from_slice(rest);
            // An item with no number here adds nothing and is not stored (the shown cell always has one).
            let Some(value) = number(&lookup.get(&path, context.col_path, context.value_index)) else { continue };
            total += value;
            by_item.entry(*item).or_insert(total);
        }
        (by_item, total)
    });
    Value::number(by_item.get(first).copied().unwrap_or(*all))
}

fn ranked_value(raw: Value, lookup: &ValueLookup<'_>, context: &ShowContext<'_>) -> Value {
    let Some((_, parent)) = context.row_path.split_last() else { return Value::Empty };
    let Some(me) = raw.as_f64() else { return raw };
    let mut cache = context.cache.ranks.borrow_mut();
    let sorted = cache.entry((parent.to_vec(), context.col_path.to_vec(), context.value_index)).or_insert_with(|| {
        let mut all: Vec<f64> = context
            .siblings
            .get(parent)
            .into_iter()
            .flatten()
            .filter_map(|sibling| {
                let mut path = parent.to_vec();
                path.push(*sibling);
                number(&lookup.get(&path, context.col_path, context.value_index))
            })
            .collect();
        all.sort_by(f64::total_cmp);
        all.dedup();
        all
    });
    // Excel's ranks are dense: ties share a rank and the next value takes the next one
    // (5, 4, 3, 3, 4 rank 1, 2, 3, 3, 2), so count the distinct larger values.
    let larger = sorted.len() - sorted.partition_point(|value| *value <= me);
    Value::number(larger as f64 + 1.0)
}

fn show_value(definition: &PivotValue, raw: Value, lookup: &ValueLookup<'_>, context: &ShowContext<'_>) -> Value {
    match definition.show_as {
        PivotShowAs::Normal => raw,
        PivotShowAs::PercentOfGrandTotal => ratio(raw, lookup.get(&[], &[], context.value_index)),
        PivotShowAs::PercentOfColumnTotal => ratio(raw, lookup.get(&[], context.col_path, context.value_index)),
        PivotShowAs::PercentOfRowTotal => ratio(raw, lookup.get(context.row_path, &[], context.value_index)),
        PivotShowAs::RunningTotal => running_total(raw, lookup, context),
        PivotShowAs::Rank => ranked_value(raw, lookup, context),
    }
}

type ReportFilter<'a> = (usize, &'a PivotFilter, PivotDateGroup, bool);

fn report_filters<'a>(pt: &'a PivotTable, src: &Source) -> (Vec<ReportFilter<'a>>, Vec<ReportFilter<'a>>) {
    let axis_group =
        |name: &str| pt.rows.iter().chain(pt.columns.iter()).find(|field| field.source_col.eq_ignore_ascii_case(name)).map(|field| field.date_group);
    let filters: Vec<_> = pt
        .filters
        .iter()
        .filter_map(|filter| {
            let column = src.col(&filter.source_col)?;
            let group = axis_group(&filter.source_col);
            Some((column, filter, group.unwrap_or_default(), group.is_none()))
        })
        .collect();
    let shown = filters.iter().copied().filter(|filter| filter.3).collect();
    (filters, shown)
}

fn values_position(pt: &PivotTable, src: &Source, value_count: usize) -> Option<usize> {
    if !pt.values_on_rows || value_count <= 1 {
        return None;
    }
    let position = pt.values_position.unwrap_or(pt.rows.len());
    Some(pt.rows.iter().take(position).filter(|field| src.col(&field.source_col).is_some()).count())
}

fn calculated_source_columns(vals: &[(ValSrc, &PivotValue)], calc: &CalcSet, src: &Source) -> Vec<usize> {
    let mut columns = Vec::new();
    for (source, _) in vals {
        let ValSrc::Calc(index) = source else { continue };
        for name in calc.source_refs(*index) {
            if let Some(column) = src.col(&name)
                && !columns.contains(&column)
            {
                columns.push(column);
            }
        }
    }
    columns
}

fn data_columns(specs: &[ColSpec], has_column_fields: bool, on_rows: bool, value_count: usize) -> Result<Vec<(usize, Option<usize>)>, String> {
    let mut columns = Vec::new();
    for (spec_index, _) in specs.iter().enumerate() {
        if on_rows {
            columns.push((spec_index, None));
        } else if value_count == 0 {
            if has_column_fields {
                columns.push((spec_index, None));
            }
        } else {
            columns.extend((0..value_count).map(|value_index| (spec_index, Some(value_index))));
        }
        if columns.len() > MAX_COLS as usize {
            return Err("The PivotTable report will not fit on the sheet.".into());
        }
    }
    Ok(columns)
}

pub(super) fn compute_from(wb: &Workbook, pt: &PivotTable, src: &Source, mut probe: Option<&mut Probe>) -> Result<Output, String> {
    let fmt_of = |c: usize| src.formats.get(c).cloned().unwrap_or_else(|| "General".into());
    let rows: Vec<(usize, &PivotField)> = pt.rows.iter().filter_map(|f| src.col(&f.source_col).map(|c| (c, f))).collect();
    let cols: Vec<(usize, &PivotField)> = pt.columns.iter().filter_map(|f| src.col(&f.source_col).map(|c| (c, f))).collect();
    // Value fields: a source column, or a calculated field evaluated on sums (see `pivot_calc`).
    let calc = CalcSet::new(pt, &src.headers);
    let vals: Vec<(ValSrc, &PivotValue)> = pt
        .values
        .iter()
        .filter_map(|v| match src.col(&v.source_col) {
            Some(c) => Some((ValSrc::Col(c), v)),
            None => calc.index(&v.source_col).map(|i| (ValSrc::Calc(i), v)),
        })
        .collect();
    // Source columns summed for the calculated fields, accumulated after the value fields.
    let extra = calculated_source_columns(&vals, &calc, src);
    if rows.len() + cols.len() > MAX_AXIS_FIELDS {
        return Err(format!("A PivotTable can have at most {MAX_AXIS_FIELDS} row and column fields."));
    }
    if vals.len() > MAX_VALUE_FIELDS {
        return Err(format!("A PivotTable can have at most {MAX_VALUE_FIELDS} value fields."));
    }
    let (filters, shown_filters) = report_filters(pt, src);

    let k = vals.len();
    let empty = rows.is_empty() && cols.is_empty() && vals.is_empty();
    // Σ Values in Rows (it exists with two or more value fields): its depth among the row fields
    // found in the source.
    let vpos = values_position(pt, src, k);
    let on_rows = vpos.is_some();
    let row_levels = rows.len() + usize::from(on_rows);

    // ---- aggregate
    let filter_fmts: Vec<String> = filters.iter().map(|f| fmt_of(f.0)).collect();
    let filter_sets: Vec<Option<HashSet<String>>> =
        filters.iter().map(|(_, field, _, _)| field.selected.as_ref().map(|s| s.iter().map(|v| v.to_ascii_lowercase()).collect())).collect();
    let row_fmts: Vec<String> = rows.iter().map(|(c, _)| fmt_of(*c)).collect();
    let col_fmts: Vec<String> = cols.iter().map(|(c, _)| fmt_of(*c)).collect();
    let aggregate = aggregate_source(
        AggregateInput {
            wb,
            pt,
            src,
            rows: &rows,
            cols: &cols,
            vals: &vals,
            extra: &extra,
            filters: &filters,
            filter_fmts: &filter_fmts,
            filter_sets: &filter_sets,
            row_fmts: &row_fmts,
            col_fmts: &col_fmts,
            empty,
        },
        &mut probe,
    )?;
    let AggregateData { row_items, col_items, aggs, row_set, col_set, col_leaves: _ } = aggregate;

    // ---- axes
    let row_fields: Vec<&PivotField> = rows.iter().map(|(_, f)| *f).collect();
    let col_fields: Vec<&PivotField> = cols.iter().map(|(_, f)| *f).collect();
    let row_paths: Vec<&[u32]> = row_set.iter().map(Vec::as_slice).collect();
    let col_paths: Vec<&[u32]> = col_set.iter().map(Vec::as_slice).collect();
    let row_tree = build_tree(&row_paths, 0, &row_fields, &row_items);
    let col_tree = build_tree(&col_paths, 0, &col_fields, &col_items);
    let mut siblings: HashMap<Vec<u32>, Vec<u32>> = HashMap::new();
    collect_siblings(&row_tree, &mut Vec::new(), &mut siblings);
    let level0: Vec<u32> = row_tree.iter().map(|n| n.item).collect();

    let val_names: Vec<String> = vals.iter().map(|(_, v)| v.name.clone()).collect();
    let ctx = Ctx { pt, rows: row_fields.clone(), row_items, row_fmts: row_fmts.clone(), vpos, val_names: val_names.clone() };
    let mut lines: Vec<Line> = Vec::new();
    if row_levels == 0 {
        lines.push(Line { kind: LineKind::Item(false), leaf: true, path: vec![], values: true, labels: vec![], vi: None });
    } else {
        if pt.layout == PivotLayout::Tabular {
            ctx.flatten_tabular(&row_tree, 0, None, &mut Vec::new(), &[], &mut lines);
        } else {
            ctx.flatten(&row_tree, 0, None, &mut Vec::new(), &mut lines);
        }
        // With only Σ Values in Rows its items are the totals already.
        if pt.grand_totals_cols && !rows.is_empty() {
            let grand = |text: String, vi: Option<usize>| Line {
                kind: LineKind::Grand,
                leaf: false,
                path: vec![],
                values: true,
                labels: vec![(0, Value::text(text), 0, None)],
                vi,
            };
            if on_rows {
                lines.extend(val_names.iter().enumerate().map(|(vi, name)| grand(format!("Total {name}"), Some(vi))));
            } else {
                lines.push(grand("Grand Total".into(), None));
            }
        }
    }
    let mut specs: Vec<ColSpec> = Vec::new();
    if cols.is_empty() {
        specs.push(ColSpec { kind: ColKind::Leaf, path: vec![] });
    } else {
        flatten_cols(&col_tree, 0, &col_fields, &col_items, pt.subtotals, &mut Vec::new(), &mut specs);
        if pt.grand_totals_rows {
            specs.push(ColSpec { kind: ColKind::Grand, path: vec![] });
        }
    }
    let data_cols = data_columns(&specs, !cols.is_empty(), on_rows, k)?;

    let compact = pt.layout == PivotLayout::Compact;
    let l = if row_levels == 0 && cols.is_empty() {
        0
    } else if compact {
        1
    } else {
        row_levels.max(1)
    };
    let mut width = l + data_cols.len();
    if !shown_filters.is_empty() {
        width = width.max(2);
    }
    if empty {
        width = width.max(1);
    }
    let width = width.max(1);
    let mut g = Grid { rows: vec![], kinds: vec![], width };

    // ---- report filters
    for (c, f, _, _) in &shown_filters {
        let r = g.push_row(RowKind::Filter)?;
        let name = src.headers.get(*c).cloned().unwrap_or_default();
        let shown = match &f.selected {
            None => "(All)".to_string(),
            Some(v) if v.len() == 1 => v.first().cloned().unwrap_or_default(),
            Some(_) => "(Multiple Items)".to_string(),
        };
        g.set(r, 0, Value::text(name), CellKind::Filter, 0, None);
        g.set(r, 1, Value::text(shown), CellKind::Filter, 0, None);
    }
    if !shown_filters.is_empty() {
        g.push_row(RowKind::Blank)?;
    }
    let body_offset = g.rows.len() as u32;

    if empty {
        let r = g.push_row(RowKind::Placeholder)?;
        g.set(r, 0, Value::text(format!("{} — choose fields", pt.name)), CellKind::Placeholder, 0, None);
        return Ok(Output {
            width: g.width as u32,
            grid: g.rows,
            row_kinds: g.kinds,
            body_offset,
            chart_rows: vec![],
            chart_cols: vec![],
            label_col: None,
            data: vec![],
        });
    }

    write_headers(
        &mut g,
        HeaderInput {
            vals: &vals,
            row_fields: &row_fields,
            col_fields: &col_fields,
            col_items: &col_items,
            col_fmts: &col_fmts,
            vpos,
            data_cols: &data_cols,
            specs: &specs,
            row_levels,
            compact,
            on_rows,
            k,
            l,
            headers_on: pt.show_headers,
        },
    )?;
    let val_name = |vi: usize| vals.get(vi).map(|(_, value)| value.name.clone()).unwrap_or_default();

    // ---- body
    let data_fmt: Vec<Option<String>> = vals
        .iter()
        .map(|(vs, v)| match v.show_as {
            PivotShowAs::PercentOfGrandTotal | PivotShowAs::PercentOfColumnTotal | PivotShowAs::PercentOfRowTotal => {
                Some(v.number_format.clone().unwrap_or_else(|| "0.00%".into()))
            }
            PivotShowAs::Rank => v.number_format.clone(),
            _ => v.number_format.clone().or_else(|| match v.func {
                PivotFunc::Count | PivotFunc::CountNumbers | PivotFunc::DistinctCount => None,
                _ => {
                    let f = match vs {
                        ValSrc::Col(c) => fmt_of(*c),
                        ValSrc::Calc(_) => "General".to_string(),
                    };
                    (f != "General").then_some(f)
                }
            }),
        })
        .collect();
    let lookup = ValueLookup { wb, source: src, calc: &calc, vals: &vals, extra: &extra, aggs: &aggs, value_count: k };
    let show_cache = ShowCache::default();
    let mut chart_rows: Vec<u32> = Vec::new();
    let chart_cols: Vec<(u32, String)> = data_cols
        .iter()
        .enumerate()
        .filter_map(|(j, (si, vi))| {
            let spec = specs.get(*si)?;
            if spec.kind != ColKind::Leaf {
                return None;
            }
            let mut parts: Vec<String> =
                spec.path.iter().enumerate().map(|(lv, it)| col_items.get(lv).map(|fi| fi.label(*it)).unwrap_or_default()).collect();
            if let Some(vi) = vi
                && (k > 1 || parts.is_empty())
            {
                parts.push(val_name(*vi));
            } else if parts.is_empty() {
                parts.push("Total".into());
            }
            Some(((l + j) as u32, parts.join(" - ")))
        })
        .filter(|_| k > 0)
        .collect();
    let label_col = if row_levels == 0 {
        None
    } else if compact {
        Some(0)
    } else {
        Some(row_levels.saturating_sub(1) as u32)
    };
    let item_refs = |fields: &[&PivotField], items: &[FieldItems], path: &[u32]| -> Vec<PivotItemRef> {
        path.iter()
            .enumerate()
            .filter_map(|(d, it)| {
                let (f, item) = (fields.get(d)?, items.get(d)?.get(*it)?);
                Some(PivotItemRef { field: f.source_col.clone(), label: item.label.clone(), value: item.value.clone() })
            })
            .collect()
    };
    let mut data: Vec<PivotDataCell> = Vec::new();
    for line in &lines {
        let rk = match line.kind {
            LineKind::Item(b) => RowKind::Item(b),
            LineKind::Subtotal => RowKind::Subtotal,
            LineKind::Grand => RowKind::Grand,
        };
        let ck = match line.kind {
            LineKind::Item(_) => (CellKind::RowLabel, CellKind::Data),
            LineKind::Subtotal => (CellKind::Subtotal, CellKind::Subtotal),
            LineKind::Grand => (CellKind::GrandTotal, CellKind::GrandTotal),
        };
        let r = g.push_row(rk)?;
        if line.leaf && k > 0 {
            chart_rows.push(r as u32);
        }
        for (c, v, indent, fmt) in &line.labels {
            g.set(r, *c, v.clone(), ck.0, *indent, fmt.clone());
        }
        if !line.values {
            continue;
        }
        let rp = line.path.as_slice();
        for (j, (si, col_vi)) in data_cols.iter().enumerate() {
            let line_vi = col_vi.or(line.vi);
            let (Some(spec), Some(vi)) = (specs.get(*si), &line_vi) else { continue };
            let Some((_, vdef)) = vals.get(*vi) else { continue };
            let cp = spec.path.as_slice();
            let Some(raw) = lookup.get(rp, cp, *vi) else { continue };
            let context = ShowContext { row_path: rp, col_path: cp, value_index: *vi, level0: &level0, siblings: &siblings, cache: &show_cache };
            let shown = show_value(vdef, raw, &lookup, &context);
            // Every value cell of the report is indexed, so a report that needs more cells than
            // the index cap is rejected outright — never written with a truncated index that
            // would answer #REF! for the values the sheet shows.
            if data.len() >= MAX_INDEXED_CELLS {
                return Err(format!("A PivotTable report can have at most {MAX_INDEXED_CELLS} value cells."));
            }
            let mut items = item_refs(&ctx.rows, &ctx.row_items, rp);
            items.extend(item_refs(&col_fields, &col_items, cp));
            data.push(PivotDataCell {
                data_field: vdef.name.clone(),
                source_col: vdef.source_col.clone(),
                items,
                row: u32::try_from(r).unwrap_or(u32::MAX),
                col: u32::try_from(l + j).unwrap_or(u32::MAX),
            });
            if let Some(pr) = probe.as_deref_mut() {
                if pr.at == Some((r, l + j)) {
                    pr.hit = Some((rp.to_vec(), cp.to_vec()));
                }
                if let Some(cells) = pr.cells.as_mut() {
                    cells.insert((r, l + j));
                }
            }
            g.set(r, l + j, shown, ck.1, 0, data_fmt.get(*vi).cloned().flatten());
        }
    }
    Ok(Output { width: g.width as u32, grid: g.rows, row_kinds: g.kinds, body_offset, chart_rows, chart_cols, label_col, data })
}
