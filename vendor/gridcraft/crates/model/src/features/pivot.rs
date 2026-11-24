use gridcraft_core::Value;

use super::{CellRef, RangeRef};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------- pivot tables

/// How a PivotTable lays out its row fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PivotLayout {
    /// All row fields in one column, inner items indented ("Row Labels" header).
    #[default]
    Compact,
    /// One column per row field; each item on its own row.
    Outline,
    /// One column per row field; inner items start on their parent's row, totals below.
    Tabular,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PivotSort {
    #[default]
    Asc,
    Desc,
    /// Source order (first appearance).
    None,
}

/// Grouping of a date field (values are date serials).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PivotDateGroup {
    #[default]
    None,
    Years,
    Quarters,
    Months,
    Days,
}

/// A field placed in the Rows or Columns area.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotField {
    /// Source column header name.
    pub source_col: String,
    pub sort: PivotSort,
    pub date_group: PivotDateGroup,
    /// Item labels whose detail is hidden (collapsed).
    pub collapsed_items: Vec<String>,
}

/// Summarize-values-by function.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PivotFunc {
    #[default]
    Sum,
    Count,
    Average,
    Max,
    Min,
    Product,
    CountNumbers,
    /// Number of different non-empty values (Excel's Distinct Count, a Data Model summary).
    /// A workbook file has no field for it: it is saved as Count.
    DistinctCount,
    StdDev,
    StdDevP,
    Var,
    VarP,
}

/// Show-values-as transform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PivotShowAs {
    #[default]
    Normal,
    PercentOfGrandTotal,
    PercentOfColumnTotal,
    PercentOfRowTotal,
    /// Running total in the first row field.
    RunningTotal,
    /// Rank largest to smallest among sibling row items.
    Rank,
}

/// A field in the Values area.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotValue {
    pub source_col: String,
    pub func: PivotFunc,
    /// Caption, e.g. "Sum of Sales".
    pub name: String,
    pub show_as: PivotShowAs,
    pub number_format: Option<String>,
}

/// A report filter (or, for a field also in Rows/Columns, its hidden-items filter).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotFilter {
    pub source_col: String,
    /// Item labels shown; `None` = all.
    pub selected: Option<Vec<String>>,
}

/// A PivotTable: its definition plus where its output was last written. The output itself is
/// static cell values and styles on the sheet, rewritten on every refresh.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotTable {
    pub id: u32,
    pub name: String,
    /// Source: range formula text (`Sheet1!$A$1:$E$200`), a table name or a defined name.
    pub source: String,
    /// Top-left of the output body on the pivot's sheet.
    pub anchor: CellRef,
    pub rows: Vec<PivotField>,
    pub columns: Vec<PivotField>,
    pub values: Vec<PivotValue>,
    pub filters: Vec<PivotFilter>,
    pub layout: PivotLayout,
    /// PivotTable Options › Data › Enable show details: Show Details lists a value cell's
    /// source records only while this is on.
    pub enable_show_details: bool,
    /// Grand total for each row (the "Grand Total" column), like Excel's "On for Rows".
    pub grand_totals_rows: bool,
    /// Grand total for each column (the "Grand Total" row), like Excel's "On for Columns".
    pub grand_totals_cols: bool,
    pub subtotals: bool,
    /// Subtotals at the top of each group (Compact/Outline); otherwise below it.
    pub subtotals_top: bool,
    pub style: String,
    pub show_headers: bool,
    /// Area written by the last refresh.
    pub last_range: Option<RangeRef>,
    /// Calculated fields, usable in the Values area by name.
    pub calculated_fields: Vec<PivotCalcField>,
    /// Where GETPIVOTDATA finds the report's value cells, as of the last refresh. Shared between
    /// workbook clones (so editing the pivot's sheet doesn't copy it), set on refresh and saved
    /// with the report, so a reopened unrefreshed pivot still reads the report it stored. Only a
    /// pivot loaded without one (XLSX) builds it on first use, checked against the stored report.
    pub data_index: PivotLookup,
    /// The Σ Values pseudo-field (it exists with two or more value fields) sits in Rows rather
    /// than Columns (Excel's "values on rows").
    pub values_on_rows: bool,
    /// Position of Σ Values among the row fields (`None` = after the last). Only used with
    /// `values_on_rows`: in Columns Σ Values always comes after the last column field.
    pub values_position: Option<usize>,
}

/// A calculated field: a formula over other fields (`=Sales*0.1`, `='Unit Price'*Units`). As in
/// Excel, it is evaluated on the sums of the fields it names for each report cell, not per record.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotCalcField {
    pub name: String,
    /// Formula text, starting with `=`.
    pub formula: String,
}

/// One value cell of a computed PivotTable report: the value field, the row and column items
/// that locate it (fewer for subtotals, none for the grand total) and where the report shows it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PivotDataCell {
    /// The value field's caption ("Sum of Sales").
    pub data_field: String,
    /// The value field's source column ("Sales").
    pub source_col: String,
    pub items: Vec<PivotItemRef>,
    /// Row and column of the cell, counted from the report's top-left cell.
    pub row: u32,
    pub col: u32,
}

/// Builds a PivotTable's value cells from its source (`sheet` is the pivot's sheet). The engine
/// sets it; the model can't compute reports.
pub type PivotIndexer = fn(&crate::Workbook, usize, &PivotTable) -> Result<Vec<PivotDataCell>, String>;
pub static PIVOT_INDEXER: std::sync::OnceLock<PivotIndexer> = std::sync::OnceLock::new();

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

/// One part of a lookup key: length-prefixed, so no label text can imitate a separator.
fn key_part(out: &mut String, s: &str) {
    out.push_str(&s.len().to_string());
    out.push(':');
    out.push_str(s);
}

/// The lookup key of a value field with field/item labels (names ignore case and outer blanks).
/// Each item is `(field, label, is_bool)`: a Boolean item and a text item with the same label
/// are different items.
pub fn pivot_key(data_field: &str, items: &[(&str, &str, bool)]) -> String {
    let mut parts: Vec<String> = items
        .iter()
        .map(|(f, l, b)| {
            let mut p = String::new();
            key_part(&mut p, &norm(f));
            key_part(&mut p, &norm(l));
            p.push(if *b { 'b' } else { 't' });
            p
        })
        .collect();
    parts.sort_unstable();
    let mut key = String::new();
    key_part(&mut key, &norm(data_field));
    key.push_str(&parts.len().to_string());
    key.push('|');
    for p in parts {
        key.push_str(&p);
    }
    key
}

#[derive(Default)]
struct LookupInner {
    cells: std::sync::OnceLock<Vec<PivotDataCell>>,
    keys: std::sync::OnceLock<std::collections::HashMap<String, usize>>,
}

/// A PivotTable's GETPIVOTDATA index. Clones share it, so `Workbook` clones per command stay
/// cheap however large the report is. The cells are set on refresh or on first use; the hash of
/// them is built on the first lookup.
#[derive(Clone, Default)]
pub struct PivotLookup(std::sync::Arc<LookupInner>);

impl PivotLookup {
    /// The value cells, if they were set.
    pub fn cells(&self) -> Option<&[PivotDataCell]> {
        self.0.cells.get().map(Vec::as_slice)
    }

    /// Sets the value cells (the first set wins) and returns them.
    pub fn set_cells(&self, cells: Vec<PivotDataCell>) -> &[PivotDataCell] {
        self.0.cells.get_or_init(|| cells)
    }

    /// The cell with this value field and exactly these items, by hash.
    pub fn find(&self, data_field: &str, items: &[(&str, &str, bool)]) -> Option<&PivotDataCell> {
        let cells = self.0.cells.get()?;
        let keys = self.0.keys.get_or_init(|| {
            let mut m = std::collections::HashMap::with_capacity(cells.len());
            for (i, c) in cells.iter().enumerate() {
                let items: Vec<(&str, &str, bool)> =
                    c.items.iter().map(|it| (it.field.as_str(), it.label.as_str(), matches!(it.value, Value::Bool(_)))).collect();
                m.entry(pivot_key(&c.data_field, &items)).or_insert(i);
                m.entry(pivot_key(&c.source_col, &items)).or_insert(i);
            }
            m
        });
        keys.get(&pivot_key(data_field, items)).and_then(|i| cells.get(*i))
    }
}

impl std::fmt::Debug for PivotLookup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PivotLookup({} cells)", self.cells().map_or(0, <[PivotDataCell]>::len))
    }
}

/// Saved as the list of value cells (or nothing before the first refresh); the hash is rebuilt.
/// An empty list is never saved: it is no index, and restoring one would lock the shared lookup
/// against the lazy rebuild that a reopened workbook needs.
impl Serialize for PivotLookup {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.cells().filter(|cells| !cells.is_empty()).serialize(s)
    }
}

impl<'de> Deserialize<'de> for PivotLookup {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let l = PivotLookup::default();
        if let Some(cells) = Option::<Vec<PivotDataCell>>::deserialize(d)?
            && !cells.is_empty()
        {
            l.set_cells(cells);
        }
        Ok(l)
    }
}

/// The index is a cache, not part of a PivotTable's identity.
impl PartialEq for PivotLookup {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// A row or column field item: the field's source column, the item's label and its value.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PivotItemRef {
    pub field: String,
    pub label: String,
    pub value: Value,
}

impl Default for PivotTable {
    fn default() -> Self {
        PivotTable {
            id: 0,
            name: "PivotTable1".into(),
            source: String::new(),
            anchor: CellRef::default(),
            rows: vec![],
            columns: vec![],
            values: vec![],
            filters: vec![],
            layout: PivotLayout::Compact,
            enable_show_details: true,
            grand_totals_rows: true,
            grand_totals_cols: true,
            subtotals: true,
            subtotals_top: true,
            style: "PivotStyleLight16".into(),
            show_headers: true,
            last_range: None,
            calculated_fields: vec![],
            data_index: PivotLookup::default(),
            values_on_rows: false,
            values_position: None,
        }
    }
}

#[cfg(test)]
mod pivot_lookup_tests {
    use super::*;

    fn cell(field: &str, label: &str, value: Value, row: u32) -> PivotDataCell {
        PivotDataCell {
            data_field: "Sum of Sales".into(),
            source_col: "Sales".into(),
            items: vec![PivotItemRef { field: field.into(), label: label.into(), value }],
            row,
            col: 1,
        }
    }

    #[test]
    fn labels_with_separator_characters_do_not_collide() {
        let l = PivotLookup::default();
        // Two different (field, label) pairs that a separator-joined key would make identical.
        l.set_cells(vec![cell("A\u{2}B", "C", Value::Text("C".into()), 1), cell("A", "B\u{2}C", Value::Text("B\u{2}C".into()), 2)]);
        assert_eq!(l.find("Sales", &[("A\u{2}B", "C", false)]).unwrap().row, 1);
        assert_eq!(l.find("Sales", &[("A", "B\u{2}C", false)]).unwrap().row, 2);
        // A label carrying the old pair separator cannot stand for two items.
        assert!(l.find("Sales", &[("A", "B", false), ("C", "D", false)]).is_none());
    }

    #[test]
    fn boolean_and_text_items_with_one_label_are_different_items() {
        let l = PivotLookup::default();
        l.set_cells(vec![cell("Flag", "TRUE", Value::Text("TRUE".into()), 1), cell("Flag", "TRUE", Value::Bool(true), 2)]);
        assert_eq!(l.find("Sales", &[("Flag", "TRUE", true)]).unwrap().row, 2);
        assert_eq!(l.find("Sales", &[("Flag", "TRUE", false)]).unwrap().row, 1);
    }

    #[test]
    fn index_round_trips_through_serde() {
        let l = PivotLookup::default();
        l.set_cells(vec![cell("Region", "East", Value::Text("East".into()), 1)]);
        let back: PivotLookup = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        assert_eq!(back.find("Sales", &[("Region", "East", false)]).unwrap().row, 1);
        let none: PivotLookup = serde_json::from_str(&serde_json::to_string(&PivotLookup::default()).unwrap()).unwrap();
        assert!(none.cells().is_none());
    }

    #[test]
    fn an_empty_lookup_is_saved_as_no_index() {
        // An empty cell list is no index, so it must not be saved as an empty one: restoring an
        // empty index counts as an initialized lookup, which blocks the lazy rebuild.
        let l = PivotLookup::default();
        l.set_cells(vec![]);
        let saved = serde_json::to_string(&l).unwrap();
        assert_eq!(saved, "null", "an empty lookup saves no index: {saved}");
        let back: PivotLookup = serde_json::from_str("[]").unwrap();
        assert!(back.cells().is_none(), "an empty saved list restores no index");
    }
}
