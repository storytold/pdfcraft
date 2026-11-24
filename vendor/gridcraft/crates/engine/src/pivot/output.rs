use super::items::{Acc, FieldItems, Node, is_collapsed};
use super::{
    BorderLine, BorderStyle, Color, Fill, HAlign, HashMap, MAX_COLS, MAX_OUTPUT_CELLS, MAX_ROWS, NumFmt, PivotDataCell, PivotField, PivotLayout,
    PivotTable, Style, Value,
};

// ---------------------------------------------------------------- output

/// What a report cell is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum CellKind {
    Empty,
    Header,
    RowLabel,
    ColLabel,
    Data,
    Subtotal,
    GrandTotal,
    Filter,
    Placeholder,
}

/// What a report row is (for row-wide styling).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum RowKind {
    Filter,
    Blank,
    Header,
    /// Item row; `true` for a parent item (bold).
    Item(bool),
    Subtotal,
    Grand,
    Placeholder,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutCell {
    pub value: Value,
    pub kind: CellKind,
    pub indent: u8,
    pub num_fmt: Option<String>,
}

impl OutCell {
    fn empty() -> OutCell {
        OutCell { value: Value::Empty, kind: CellKind::Empty, indent: 0, num_fmt: None }
    }
}

/// A computed report: a dense grid relative to its top-left, with row kinds.
#[derive(Clone, Debug)]
pub struct Output {
    pub grid: Vec<Vec<OutCell>>,
    pub row_kinds: Vec<RowKind>,
    /// Rows above the body (report filters plus a blank row).
    pub body_offset: u32,
    pub width: u32,
    /// Grid rows of the innermost (or collapsed) items: a PivotChart's categories.
    pub chart_rows: Vec<u32>,
    /// Grid columns of item (not total) values with their series names: a PivotChart's series.
    pub chart_cols: Vec<(u32, String)>,
    /// Grid column holding the innermost row labels (`None` without row fields).
    pub label_col: Option<u32>,
    /// Every value cell with the items that locate it (for GETPIVOTDATA).
    pub data: Vec<PivotDataCell>,
}

impl Output {
    pub fn height(&self) -> u32 {
        self.grid.len() as u32
    }
    /// Value at (row, col) relative to the top-left.
    pub fn value(&self, r: usize, c: usize) -> Value {
        self.grid.get(r).and_then(|row| row.get(c)).map(|x| x.value.clone()).unwrap_or_default()
    }
}

pub(super) struct Grid {
    pub(super) rows: Vec<Vec<OutCell>>,
    pub(super) kinds: Vec<RowKind>,
    pub(super) width: usize,
}

impl Grid {
    pub(super) fn push_row(&mut self, kind: RowKind) -> Result<usize, String> {
        if (self.rows.len() as u64 + 1) * self.width.max(1) as u64 > MAX_OUTPUT_CELLS || self.rows.len() as u64 >= MAX_ROWS as u64 {
            return Err("The PivotTable report is too large.".into());
        }
        self.rows.push(vec![OutCell::empty(); self.width]);
        self.kinds.push(kind);
        Ok(self.rows.len() - 1)
    }
    pub(super) fn set(&mut self, r: usize, c: usize, value: Value, kind: CellKind, indent: u8, fmt: Option<String>) {
        if let Some(cell) = self.rows.get_mut(r).and_then(|row| row.get_mut(c)) {
            *cell = OutCell { value, kind, indent, num_fmt: fmt };
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LineKind {
    Item(bool),
    Subtotal,
    Grand,
}

pub(super) struct Line {
    pub(super) kind: LineKind,
    /// A category of a PivotChart: an innermost (or collapsed) item.
    pub(super) leaf: bool,
    pub(super) path: Vec<u32>,
    pub(super) values: bool,
    /// (column, value, indent, number format)
    pub(super) labels: Vec<(usize, Value, u8, Option<String>)>,
    /// The value field of the line when Σ Values sits in Rows (`None`: one per data column).
    pub(super) vi: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ColKind {
    Leaf,
    Subtotal,
    Grand,
}

/// Longest item caption Excel keeps; a subtotal caption repeats its parent's label for every value
/// field at every level, so a long source label is cut rather than multiplied.
const MAX_CAPTION_LABEL_CHARS: usize = 255;

/// `label` (cut to [`MAX_CAPTION_LABEL_CHARS`]) and `suffix`, as in "East Total".
pub(super) fn caption(label: &str, suffix: &str) -> String {
    let mut out: String = label.chars().take(MAX_CAPTION_LABEL_CHARS).collect();
    out.push(' ');
    out.push_str(suffix);
    out
}

pub(super) struct ColSpec {
    pub(super) kind: ColKind,
    pub(super) path: Vec<u32>,
}

pub(super) struct Ctx<'a> {
    pub(super) pt: &'a PivotTable,
    pub(super) rows: Vec<&'a PivotField>,
    pub(super) row_items: Vec<FieldItems>,
    pub(super) row_fmts: Vec<String>,
    /// Depth of Σ Values among the row fields when it sits in Rows (two or more value fields).
    pub(super) vpos: Option<usize>,
    /// Value field captions.
    pub(super) val_names: Vec<String>,
}

impl Ctx<'_> {
    fn label_cell(&self, depth: usize, item: u32) -> (Value, Option<String>, String) {
        let Some(it) = self.row_items.get(depth).and_then(|fi| fi.get(item)) else { return (Value::Empty, None, String::new()) };
        let fmt = if it.formatted { self.row_fmts.get(depth).cloned() } else { None };
        (it.value.clone(), fmt, it.label.clone())
    }

    /// (column, indent) of a row level (Σ Values counts as a level) in Compact/Outline form.
    fn level_cell(&self, level: usize) -> (usize, u8) {
        if self.pt.layout == PivotLayout::Outline { (level, 0) } else { (0, level.min(250) as u8) }
    }

    /// Σ Values comes right here, at row field depth `depth` (`vi`: the value field already chosen above).
    fn values_here(&self, depth: usize, vi: Option<usize>) -> bool {
        vi.is_none() && self.vpos == Some(depth)
    }

    /// Per-value subtotal lines of an item with Σ Values below it ("East Sum of Sales").
    fn value_subtotals(&self, col: usize, indent: u8, label: &str, prefix: &[u32], out: &mut Vec<Line>) {
        for (vi, name) in self.val_names.iter().enumerate() {
            out.push(Line {
                kind: LineKind::Subtotal,
                leaf: false,
                path: prefix.to_vec(),
                values: true,
                labels: vec![(col, Value::text(caption(label, name)), indent, None)],
                vi: Some(vi),
            });
        }
    }

    /// The Σ Values level at row field depth `depth`: one item per value field, each followed by
    /// the row fields below it (`nodes`) or, innermost, holding that value.
    fn flatten_values(&self, nodes: &[Node], depth: usize, prefix: &mut Vec<u32>, out: &mut Vec<Line>) {
        let (col, indent) = self.level_cell(depth);
        for (vi, name) in self.val_names.iter().enumerate() {
            let labels = vec![(col, Value::text(name.as_str()), indent, None)];
            let leaf = nodes.is_empty();
            out.push(Line { kind: LineKind::Item(!leaf), leaf, path: prefix.clone(), values: leaf, labels, vi: Some(vi) });
            if !leaf {
                self.flatten(nodes, depth, Some(vi), prefix, out);
            }
        }
    }

    pub(super) fn flatten(&self, nodes: &[Node], depth: usize, vi: Option<usize>, prefix: &mut Vec<u32>, out: &mut Vec<Line>) {
        if self.values_here(depth, vi) {
            self.flatten_values(nodes, depth, prefix, out);
            return;
        }
        let n = self.rows.len();
        // Σ Values still to come below this level: totals need one line per value field.
        let values_below = vi.is_none() && self.vpos.is_some();
        let (col, indent) = self.level_cell(depth + usize::from(vi.is_some() && self.vpos.is_some()));
        for node in nodes {
            prefix.push(node.item);
            let (v, fmt, label) = self.label_cell(depth, node.item);
            let collapsed = depth + 1 < n && self.rows.get(depth).is_some_and(|f| is_collapsed(f, &label));
            let leaf = depth + 1 >= n || collapsed || node.children.is_empty();
            if leaf && !values_below {
                out.push(Line {
                    kind: LineKind::Item(depth + 1 < n),
                    leaf: true,
                    path: prefix.clone(),
                    values: true,
                    labels: vec![(col, v, indent, fmt)],
                    vi,
                });
            } else if leaf {
                out.push(Line {
                    kind: LineKind::Item(true),
                    leaf: false,
                    path: prefix.clone(),
                    values: false,
                    labels: vec![(col, v, indent, fmt)],
                    vi,
                });
                // Outline gives each level its own column, so the captions go in the Values
                // column even when the levels between are collapsed away.
                let level = if self.pt.layout == PivotLayout::Outline { self.vpos.unwrap_or(depth + 1) } else { depth + 1 };
                self.flatten_values(&[], level, prefix, out);
            } else if values_below {
                // Subtotals per value field, below the group (one line can't hold them all).
                out.push(Line {
                    kind: LineKind::Item(true),
                    leaf: false,
                    path: prefix.clone(),
                    values: false,
                    labels: vec![(col, v, indent, fmt)],
                    vi,
                });
                self.flatten(&node.children, depth + 1, None, prefix, out);
                if self.pt.subtotals {
                    self.value_subtotals(col, indent, &label, prefix, out);
                }
            } else {
                let top = self.pt.subtotals && self.pt.subtotals_top;
                out.push(Line {
                    kind: LineKind::Item(true),
                    leaf: false,
                    path: prefix.clone(),
                    values: top,
                    labels: vec![(col, v, indent, fmt)],
                    vi,
                });
                self.flatten(&node.children, depth + 1, vi, prefix, out);
                if self.pt.subtotals && !self.pt.subtotals_top {
                    out.push(Line {
                        kind: LineKind::Subtotal,
                        leaf: false,
                        path: prefix.clone(),
                        values: true,
                        labels: vec![(col, Value::text(caption(&label, "Total")), indent, None)],
                        vi,
                    });
                }
            }
            prefix.pop();
        }
    }

    /// Tabular form of [`Self::flatten_values`]: the first line also carries the parents' labels.
    fn flatten_tabular_values(
        &self,
        nodes: &[Node],
        depth: usize,
        prefix: &mut Vec<u32>,
        pending: &[(usize, Value, u8, Option<String>)],
        out: &mut Vec<Line>,
    ) {
        for (vi, name) in self.val_names.iter().enumerate() {
            let mut labels: Vec<(usize, Value, u8, Option<String>)> = if vi == 0 { pending.to_vec() } else { vec![] };
            labels.push((depth, Value::text(name.as_str()), 0, None));
            if nodes.is_empty() {
                out.push(Line { kind: LineKind::Item(false), leaf: true, path: prefix.clone(), values: true, labels, vi: Some(vi) });
            } else {
                self.flatten_tabular(nodes, depth, Some(vi), prefix, &labels, out);
            }
        }
    }

    pub(super) fn flatten_tabular(
        &self,
        nodes: &[Node],
        depth: usize,
        vi: Option<usize>,
        prefix: &mut Vec<u32>,
        pending: &[(usize, Value, u8, Option<String>)],
        out: &mut Vec<Line>,
    ) {
        if self.values_here(depth, vi) {
            self.flatten_tabular_values(nodes, depth, prefix, pending, out);
            return;
        }
        let n = self.rows.len();
        let values_below = vi.is_none() && self.vpos.is_some();
        let col = depth + usize::from(vi.is_some() && self.vpos.is_some());
        for (i, node) in nodes.iter().enumerate() {
            prefix.push(node.item);
            let (v, fmt, label) = self.label_cell(depth, node.item);
            let mut labels: Vec<(usize, Value, u8, Option<String>)> = if i == 0 { pending.to_vec() } else { vec![] };
            labels.push((col, v, 0, fmt));
            let collapsed = depth + 1 < n && self.rows.get(depth).is_some_and(|f| is_collapsed(f, &label));
            if depth + 1 >= n || collapsed || node.children.is_empty() {
                if values_below {
                    self.flatten_tabular_values(&[], self.vpos.unwrap_or(depth + 1), prefix, &labels, out);
                } else {
                    out.push(Line { kind: LineKind::Item(false), leaf: true, path: prefix.clone(), values: true, labels, vi });
                }
            } else {
                self.flatten_tabular(&node.children, depth + 1, vi, prefix, &labels, out);
                if self.pt.subtotals && values_below {
                    self.value_subtotals(col, 0, &label, prefix, out);
                } else if self.pt.subtotals {
                    out.push(Line {
                        kind: LineKind::Subtotal,
                        leaf: false,
                        path: prefix.clone(),
                        values: true,
                        labels: vec![(col, Value::text(caption(&label, "Total")), 0, None)],
                        vi,
                    });
                }
            }
            prefix.pop();
        }
    }
}

pub(super) fn flatten_cols(
    nodes: &[Node],
    depth: usize,
    fields: &[&PivotField],
    items: &[FieldItems],
    subtotals: bool,
    prefix: &mut Vec<u32>,
    out: &mut Vec<ColSpec>,
) {
    let n = fields.len();
    for node in nodes {
        prefix.push(node.item);
        let label = items.get(depth).map(|fi| fi.label(node.item)).unwrap_or_default();
        let collapsed = depth + 1 < n && fields.get(depth).is_some_and(|f| is_collapsed(f, &label));
        if depth + 1 >= n || collapsed || node.children.is_empty() {
            out.push(ColSpec { kind: ColKind::Leaf, path: prefix.clone() });
        } else {
            flatten_cols(&node.children, depth + 1, fields, items, subtotals, prefix, out);
            if subtotals {
                out.push(ColSpec { kind: ColKind::Subtotal, path: prefix.clone() });
            }
        }
        prefix.pop();
    }
}

pub(super) type AggMap = HashMap<(Vec<u32>, Vec<u32>), Vec<Acc>>;

/// Where a value field's numbers come from.
#[derive(Clone, Copy)]
pub(super) enum ValSrc {
    /// A source column.
    Col(usize),
    /// A calculated field (index into the PivotTable's calculated fields).
    Calc(usize),
}

/// True when `col_groups` distinct column leaves (plus the grand-total column) times `values` value fields
/// (at least one data column per group) already exceed the sheet width. This is a lower bound on the final column count.
pub(super) fn data_cols_exceed_sheet(col_groups: usize, grand_total: bool, values: usize) -> bool {
    (col_groups + usize::from(grand_total)).saturating_mul(values.max(1)) > MAX_COLS as usize
}

// ---------------------------------------------------------------- styling and writing

/// Colours for a PivotTable style name (our own palette, keyed by the familiar
/// `PivotStyleLight1…28 / Medium1…28 / Dark1…28` names).
pub(super) struct Look {
    header_fill: Color,
    header_font: Color,
    line: Color,
    filter_fill: Color,
}

pub(super) fn look(style: &str) -> Look {
    let rest = style.strip_prefix("PivotStyle").unwrap_or(style);
    let (fam, n) = ["Light", "Medium", "Dark"]
        .iter()
        .find_map(|f| rest.strip_prefix(f).map(|num| (*f, num.parse::<u32>().unwrap_or(16))))
        .unwrap_or(("Light", 16));
    let k = n.saturating_sub(1) % 7;
    let acc = if k == 0 { None } else { Some(3 + k as u8) };
    let tone = |tint: i16| match acc {
        Some(a) => Color::Theme(a, tint),
        None => Color::Theme(1, tint.clamp(-1000, 1000)),
    };
    match fam {
        "Dark" => Look { header_fill: Color::Theme(1, 0), header_font: Color::Theme(0, 0), line: tone(0), filter_fill: tone(600) },
        "Medium" => Look { header_fill: tone(0), header_font: Color::Theme(0, 0), line: tone(0), filter_fill: tone(800) },
        _ => Look { header_fill: tone(800), header_font: Color::Auto, line: tone(400), filter_fill: tone(800) },
    }
}

pub(super) fn cell_style(lk: &Look, rk: RowKind, cell: &OutCell, last_header: bool) -> Style {
    let mut s = Style::default();
    match rk {
        RowKind::Header => {
            s.font.bold = true;
            s.fill = Fill::solid(lk.header_fill);
            s.font.color = lk.header_font;
            if last_header {
                s.borders.bottom = BorderLine { style: BorderStyle::Thin, color: lk.line };
            }
        }
        RowKind::Filter => {
            s.fill = Fill::solid(lk.filter_fill);
            s.font.bold = last_header;
        }
        RowKind::Item(parent) => s.font.bold = parent,
        RowKind::Subtotal => s.font.bold = true,
        RowKind::Grand => {
            s.font.bold = true;
            s.borders.top = BorderLine { style: BorderStyle::Thin, color: lk.line };
        }
        RowKind::Placeholder => {
            s.font.italic = true;
            s.font.color = Color::Theme(1, 500);
        }
        RowKind::Blank => {}
    }
    if matches!(cell.kind, CellKind::RowLabel | CellKind::ColLabel | CellKind::Subtotal | CellKind::GrandTotal) && !cell.value.is_number() {
        s.align.h = HAlign::Left;
    }
    if matches!(cell.kind, CellKind::RowLabel | CellKind::ColLabel) && cell.value.is_number() {
        s.align.h = HAlign::Left;
    }
    s.align.indent = cell.indent.min(15);
    if let Some(f) = &cell.num_fmt {
        s.num_fmt = NumFmt::new(f);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captions_cut_a_long_parent_label() {
        assert_eq!(caption("East", "Total"), "East Total");
        let long = "é".repeat(100_000);
        let c = caption(&long, "Sum of Sales");
        assert_eq!(c.chars().count(), MAX_CAPTION_LABEL_CHARS + " Sum of Sales".len());
        assert!(c.ends_with(" Sum of Sales"));
    }
}
