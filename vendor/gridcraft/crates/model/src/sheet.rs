//! Worksheets.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gridcraft_core::{CellRef, RangeRef, Value};
use serde::{Deserialize, Serialize};

use crate::cell::{Cell, CellPicture};
use crate::features::{
    Anchor, AutoFilter, Chart, Comment, CondFormat, FormControl, Hyperlink, Image, LineInfo, ObjKind, ObjRef, PivotTable, PrintSettings, Shape,
    ShapeKind, SheetProtection, Slicer, Sparkline, Table, Validation,
};
use crate::store::CellStore;
use crate::style::{Color, StyleId};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    #[default]
    Visible,
    Hidden,
    VeryHidden,
}

/// How a sheet is displayed (View › Workbook Views; `<sheetView view="…">` in XLSX).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SheetViewMode {
    #[default]
    Normal,
    /// Pages on paper with rulers, margins and in-place headers and footers.
    PageLayout,
    PageBreakPreview,
}

impl SheetViewMode {
    /// The XLSX / command spelling: `normal`, `pageLayout` or `pageBreakPreview`.
    pub fn as_str(self) -> &'static str {
        match self {
            SheetViewMode::Normal => "normal",
            SheetViewMode::PageLayout => "pageLayout",
            SheetViewMode::PageBreakPreview => "pageBreakPreview",
        }
    }
    pub fn parse(s: &str) -> Option<SheetViewMode> {
        match s {
            "normal" => Some(SheetViewMode::Normal),
            "pageLayout" => Some(SheetViewMode::PageLayout),
            "pageBreakPreview" => Some(SheetViewMode::PageBreakPreview),
            _ => None,
        }
    }
}

/// A sheet's identity while the app runs, for what must follow one sheet (an open cell draft): a
/// sheet made new (added, inserted, copied, read from a file or from JSON) gets a fresh one, and it
/// stays with the sheet through renames, moves and the copies undo keeps. Never saved.
/// It is not content: any two compare equal, so comparing sheets compares what they hold; compare
/// identities with [`SheetUid::get`].
#[derive(Clone, Copy, Debug)]
pub struct SheetUid(u64);

/// The next [`SheetUid`]; 0 is never given out.
static NEXT_SHEET_UID: AtomicU64 = AtomicU64::new(1);

impl SheetUid {
    pub fn fresh() -> SheetUid {
        SheetUid(NEXT_SHEET_UID.fetch_add(1, Ordering::Relaxed))
    }
    pub fn get(self) -> u64 {
        self.0
    }
}

impl Default for SheetUid {
    fn default() -> SheetUid {
        SheetUid::fresh()
    }
}

impl PartialEq for SheetUid {
    fn eq(&self, _: &SheetUid) -> bool {
        true
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sheet {
    pub name: String,
    /// The sheet's identity while the app runs ([`SheetUid`]). Not in JSON or the file.
    #[serde(skip)]
    pub uid: SheetUid,
    /// The `sheetId` the file gave the sheet, written back on save so that what refers to the
    /// sheet by its id keeps finding it; 0 for a sheet the file did not have. Not in JSON.
    #[serde(skip)]
    pub file_id: u32,
    pub cells: CellStore,
    /// Static pictures are sparse rich content. Ordinary cells carry no picture pointer.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty", with = "picture_map")]
    pub cell_pictures: BTreeMap<CellRef, Arc<CellPicture>>,
    /// Values spilled by dynamic-array formulas (maintained by calc), keyed by cell.
    #[serde(skip)]
    pub spill: BTreeMap<CellRef, Value>,
    /// Anchor → spill range.
    #[serde(skip)]
    pub spill_ranges: BTreeMap<CellRef, RangeRef>,
    pub cols: BTreeMap<u32, LineInfo>,
    pub rows: BTreeMap<u32, LineInfo>,
    /// Default column width and row height in pixels at 96 dpi (a file stores row heights in points).
    pub default_col_width: f32,
    pub default_row_height: f32,
    pub merges: Vec<RangeRef>,
    /// Frozen rows and columns (top/left).
    pub freeze: Option<(u32, u32)>,
    pub tab_color: Option<Color>,
    pub visibility: Visibility,
    pub show_gridlines: bool,
    pub show_headings: bool,
    pub show_formulas: bool,
    pub show_zeros: bool,
    /// The outline's level buttons and brackets (Excel's Show Outline Symbols, Ctrl+8); the
    /// outline itself stays.
    #[serde(default = "yes")]
    pub show_outline_symbols: bool,
    /// Outline summary rows sit below their detail, and summary columns right of it (Data ▸
    /// Outline ▸ Settings; off puts them above / left). Saved as `outlinePr summaryBelow` and
    /// `summaryRight`.
    #[serde(default = "yes")]
    pub summary_below: bool,
    #[serde(default = "yes")]
    pub summary_right: bool,
    pub right_to_left: bool,
    pub zoom: u16,
    pub cond_formats: Vec<CondFormat>,
    pub validations: Vec<Validation>,
    pub tables: Vec<Table>,
    pub autofilter: Option<AutoFilter>,
    #[serde(with = "crate::cellmap")]
    pub comments: BTreeMap<CellRef, Comment>,
    #[serde(with = "crate::cellmap")]
    pub hyperlinks: BTreeMap<CellRef, Hyperlink>,
    pub charts: Vec<Chart>,
    pub images: Vec<Image>,
    pub shapes: Vec<Shape>,
    pub sparklines: Vec<Sparkline>,
    /// On-sheet slicer widgets (filter a table column by clicking value tiles).
    #[serde(default)]
    pub slicers: Vec<Slicer>,
    /// Form controls (buttons, sliders, spin buttons). They paint above every other object and
    /// take clicks first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controls: Vec<FormControl>,
    /// PivotTables whose output lives on this sheet.
    #[serde(default)]
    pub pivots: Vec<PivotTable>,
    pub protection: Option<SheetProtection>,
    /// Allow Edit Ranges (meaningful while the sheet is protected).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edit_ranges: Vec<crate::EditRange>,
    pub print: PrintSettings,
    /// Last selection and scroll position, saved with the file.
    pub view_active: CellRef,
    pub view_top_left: CellRef,
    /// Normal, Page Layout or Page Break Preview.
    #[serde(default)]
    pub view: SheetViewMode,
    /// Unmodelled file content of this sheet kept for saving (form controls, OLE objects, query
    /// tables). Goes away with the sheet; not in JSON.
    #[serde(skip)]
    pub preserved: Option<Arc<crate::preserved::Preserved>>,
}

fn yes() -> bool {
    true
}

/// Default width of a column and height of a row (pixels at 96 dpi) for the default font.
pub const DEFAULT_COL_WIDTH: f32 = 64.0;
pub const DEFAULT_ROW_HEIGHT: f32 = 20.0;

/// Column width (points) → characters of the default font's digit width (7 px), without the
/// cell padding: the value the Column Width dialog shows.
pub fn points_to_chars(px: f64) -> f64 {
    if px <= 12.0 { (px / 12.0 * 100.0).round() / 100.0 } else { ((px - 5.0) / 7.0 * 100.0 + 0.5).trunc() / 100.0 }
}

impl Sheet {
    pub fn new(name: impl Into<String>) -> Sheet {
        Sheet {
            name: name.into(),
            uid: SheetUid::fresh(),
            file_id: 0,
            cells: CellStore::new(),
            cell_pictures: BTreeMap::new(),
            spill: BTreeMap::new(),
            spill_ranges: BTreeMap::new(),
            cols: BTreeMap::new(),
            rows: BTreeMap::new(),
            default_col_width: DEFAULT_COL_WIDTH,
            default_row_height: DEFAULT_ROW_HEIGHT,
            merges: vec![],
            freeze: None,
            tab_color: None,
            visibility: Visibility::Visible,
            show_gridlines: true,
            show_outline_symbols: true,
            summary_below: true,
            summary_right: true,
            show_headings: true,
            show_formulas: false,
            show_zeros: true,
            right_to_left: false,
            zoom: 100,
            cond_formats: vec![],
            validations: vec![],
            tables: vec![],
            autofilter: None,
            comments: BTreeMap::new(),
            hyperlinks: BTreeMap::new(),
            charts: vec![],
            images: vec![],
            shapes: vec![],
            sparklines: vec![],
            slicers: vec![],
            controls: vec![],
            pivots: vec![],
            protection: None,
            edit_ranges: vec![],
            print: PrintSettings::default(),
            view_active: CellRef::default(),
            view_top_left: CellRef::default(),
            view: SheetViewMode::Normal,
            preserved: None,
        }
    }

    pub fn cell(&self, c: CellRef) -> Option<&Cell> {
        self.cells.get(c)
    }
    /// The source of a non-anchor dynamic-array spill cell, from the current calc state.
    /// Anchors, ordinary cells and cells outside successfully generated spills return `None`.
    pub fn spill_anchor(&self, c: CellRef) -> Option<CellRef> {
        self.spill_ranges.iter().find(|(anchor, range)| **anchor != c && range.contains(c)).map(|(anchor, _)| *anchor)
    }

    /// Removes the spilled values inside `r`. The work is bounded by the values stored, not by
    /// the size of `r`, so a range from a file that claims the whole sheet costs no more than the
    /// values it holds.
    pub fn clear_spill(&mut self, r: RangeRef) {
        if r.count() <= self.spill.len() as u64 {
            for c in r.iter() {
                self.spill.remove(&c);
            }
        } else {
            self.spill.retain(|c, _| !r.contains(*c));
        }
    }

    /// Turns the spill anchored at `anchor` into plain values, for a Python cell that keeps only
    /// its value: the range is forgotten, and each spilled value in it becomes its cell's value,
    /// unless the cell holds content of its own (its format is kept either way). The work is
    /// bounded by the spilled values stored, not by the size of the range.
    pub fn settle_spill(&mut self, anchor: CellRef) {
        let Some(r) = self.spill_ranges.remove(&anchor) else { return };
        let inside: Vec<CellRef> = self.spill.range(r.start..=r.end).map(|(c, _)| *c).filter(|c| r.contains(*c)).collect();
        for c in inside {
            let Some(v) = self.spill.remove(&c) else { continue };
            if v.is_empty() || c == anchor {
                continue;
            }
            match self.cells.get_mut(c) {
                Some(cell) if cell.has_content() => {}
                Some(cell) => cell.value = v,
                None => {
                    let style = self.style_id(c);
                    self.cells.set(c, Cell::styled(v, style));
                }
            }
        }
    }

    /// The floating object with workbook-unique id `id` on this sheet.
    pub fn object_ref(&self, id: u32) -> Option<ObjRef> {
        let kind = if self.charts.iter().any(|o| o.id == id) {
            ObjKind::Chart
        } else if self.images.iter().any(|o| o.id == id) {
            ObjKind::Image
        } else if let Some(s) = self.shapes.iter().find(|o| o.id == id) {
            if s.kind == ShapeKind::Equation { ObjKind::Equation } else { ObjKind::Shape }
        } else if self.slicers.iter().any(|o| o.id == id) {
            ObjKind::Slicer
        } else if self.controls.iter().any(|o| o.id == id) {
            ObjKind::Control
        } else {
            return None;
        };
        Some(ObjRef::new(kind, id))
    }

    /// Whether the floating object `o` exists on this sheet. `Shape` accepts any shape (an
    /// equation included); `Equation` only an equation.
    pub fn has_object(&self, o: ObjRef) -> bool {
        self.object_anchor(o).is_some()
    }

    /// The anchor of the floating object `o`.
    pub fn object_anchor(&self, o: ObjRef) -> Option<Anchor> {
        match o.kind {
            ObjKind::Chart => self.charts.iter().find(|c| c.id == o.id).map(|c| c.anchor),
            ObjKind::Image => self.images.iter().find(|c| c.id == o.id).map(|c| c.anchor),
            ObjKind::Shape => self.shapes.iter().find(|c| c.id == o.id).map(|c| c.anchor),
            ObjKind::Slicer => self.slicers.iter().find(|c| c.id == o.id).map(|c| c.anchor),
            ObjKind::Equation => self.shapes.iter().find(|c| c.id == o.id && c.kind == ShapeKind::Equation).map(|c| c.anchor),
            ObjKind::Control => self.controls.iter().find(|c| c.id == o.id).map(|c| c.anchor),
        }
    }

    pub fn object_anchor_mut(&mut self, o: ObjRef) -> Option<&mut Anchor> {
        match o.kind {
            ObjKind::Chart => self.charts.iter_mut().find(|c| c.id == o.id).map(|c| &mut c.anchor),
            ObjKind::Image => self.images.iter_mut().find(|c| c.id == o.id).map(|c| &mut c.anchor),
            ObjKind::Shape => self.shapes.iter_mut().find(|c| c.id == o.id).map(|c| &mut c.anchor),
            ObjKind::Slicer => self.slicers.iter_mut().find(|c| c.id == o.id).map(|c| &mut c.anchor),
            ObjKind::Equation => self.shapes.iter_mut().find(|c| c.id == o.id && c.kind == ShapeKind::Equation).map(|c| &mut c.anchor),
            ObjKind::Control => self.controls.iter_mut().find(|c| c.id == o.id).map(|c| &mut c.anchor),
        }
    }

    /// Removes the floating object `o`; false when there is no such object.
    pub fn remove_object(&mut self, o: ObjRef) -> bool {
        fn take<T>(v: &mut Vec<T>, f: impl Fn(&T) -> bool) -> bool {
            let n = v.len();
            v.retain(|x| !f(x));
            v.len() != n
        }
        match o.kind {
            ObjKind::Chart => take(&mut self.charts, |c| c.id == o.id),
            ObjKind::Image => take(&mut self.images, |c| c.id == o.id),
            ObjKind::Shape => take(&mut self.shapes, |c| c.id == o.id),
            ObjKind::Slicer => take(&mut self.slicers, |c| c.id == o.id),
            ObjKind::Equation => take(&mut self.shapes, |c| c.id == o.id && c.kind == ShapeKind::Equation),
            ObjKind::Control => take(&mut self.controls, |c| c.id == o.id),
        }
    }

    /// Every floating object's anchor in a fixed order (charts, images, shapes, slicers, controls)
    /// that [`Sheet::object_anchors_mut`] repeats, so a pass can compute new anchors from the old
    /// ones and write them back in step.
    pub fn object_anchors(&self) -> impl Iterator<Item = &Anchor> {
        self.charts
            .iter()
            .map(|o| &o.anchor)
            .chain(self.images.iter().map(|o| &o.anchor))
            .chain(self.shapes.iter().map(|o| &o.anchor))
            .chain(self.slicers.iter().map(|o| &o.anchor))
            .chain(self.controls.iter().map(|o| &o.anchor))
    }

    pub fn object_anchors_mut(&mut self) -> impl Iterator<Item = &mut Anchor> {
        self.charts
            .iter_mut()
            .map(|o| &mut o.anchor)
            .chain(self.images.iter_mut().map(|o| &mut o.anchor))
            .chain(self.shapes.iter_mut().map(|o| &mut o.anchor))
            .chain(self.slicers.iter_mut().map(|o| &mut o.anchor))
            .chain(self.controls.iter_mut().map(|o| &mut o.anchor))
    }
    /// Displayed value of a cell: its own value, or a spilled value.
    pub fn value(&self, c: CellRef) -> Value {
        match self.cells.get(c) {
            Some(cell) if !cell.value.is_empty() || cell.formula.is_some() => cell.value.clone(),
            _ => self.spill.get(&c).cloned().unwrap_or(Value::Empty),
        }
    }
    pub fn value_ref(&self, c: CellRef) -> Option<&Value> {
        match self.cells.get(c) {
            Some(cell) if !cell.value.is_empty() || cell.formula.is_some() => Some(&cell.value),
            _ => self.spill.get(&c),
        }
    }
    /// Last row (inclusive) of the maximal contiguous run of non-empty values in `col`, starting
    /// at `from_row` and capped at `cap` (inclusive). `from_row - 1` when `from_row` itself is
    /// empty (or `from_row > cap`), same as a cell-by-cell `value_ref` scan would find, but
    /// without calling `value_ref` once per row: `CellStore::contiguous_value_run` walks each
    /// band's row map directly for the common case (a run of stored cells), only falling back to
    /// a per-row `value_ref` check where the stored run stops short of `cap`. Used by the fill-handle
    /// double-click, where `col` can be densely populated for very long stretches.
    pub fn last_contiguous_nonempty(&self, col: u32, from_row: u32, cap: u32) -> u32 {
        if from_row > cap {
            return from_row.saturating_sub(1);
        }
        let mut row = from_row;
        loop {
            if row > cap {
                return cap;
            }
            let c = CellRef::new(row, col);
            match self.cells.contiguous_value_run(c, cap) {
                Some(last) => row = last + 1,
                None => {
                    // The run stopped short of `row`: ask `value_ref` for this one row, which also
                    // covers a stored cell with no value and no formula (a formatted spill child)
                    // falling through to `spill`.
                    let qualifies = self.value_ref(c).is_some_and(|v| !v.is_empty());
                    if qualifies {
                        row += 1;
                    } else {
                        return row.saturating_sub(1);
                    }
                }
            }
        }
    }
    pub fn style_id(&self, c: CellRef) -> StyleId {
        if let Some(cell) = self.cells.get(c) {
            return cell.style;
        }
        if let Some(s) = self.rows.get(&c.row).and_then(|r| r.style) {
            return s;
        }
        self.cols.get(&c.col).and_then(|r| r.style).unwrap_or_default()
    }
    pub fn set_value(&mut self, c: CellRef, v: Value) {
        let style = self.style_id(c);
        self.set_cell(c, Cell { value: v, formula: None, style, py: None });
    }
    /// Replaces content, removing any picture at this address. Style-only edits use set_style.
    pub fn set_cell(&mut self, c: CellRef, cell: Cell) {
        self.cell_pictures.remove(&c);
        self.cells.set(c, cell);
    }
    pub fn remove_cell(&mut self, c: CellRef) -> Option<Cell> {
        self.cell_pictures.remove(&c);
        self.cells.remove(c)
    }
    /// Removes content in a range; returns scalar cells, with pictures removed from the side map.
    pub fn take_cells(&mut self, range: RangeRef) -> Vec<(CellRef, Cell)> {
        self.cell_pictures.retain(|c, _| !range.contains(*c));
        self.cells.take_range(range)
    }
    /// Pictures retain a scalar fallback for calculation and unsupported consumers.
    pub fn set_picture(&mut self, c: CellRef, picture: Arc<CellPicture>) {
        let style = self.style_id(c);
        self.cells.set(c, Cell { value: Value::Error(gridcraft_core::CellError::Value), formula: None, style, py: None });
        self.cell_pictures.insert(c, picture);
    }
    pub fn input_text(&self, c: CellRef) -> String {
        if self.cell_pictures.contains_key(&c) { String::new() } else { self.cell(c).map(Cell::input_text).unwrap_or_default() }
    }
    pub fn set_style(&mut self, c: CellRef, style: StyleId) {
        let mut cell = self.cells.get(c).cloned().unwrap_or_default();
        cell.style = style;
        self.cells.set(c, cell);
    }

    // ---- geometry

    /// Whether this cell has a table or ordinary AutoFilter dropdown.
    pub fn has_filter_button(&self, c: CellRef) -> bool {
        self.autofilter.as_ref().is_some_and(|af| af.range.start.row == c.row && af.range.contains(c))
            || self.tables.iter().any(|t| t.filter_button && t.header_row && t.range.start.row == c.row && t.range.contains(c))
    }

    /// Horizontal header space in unzoomed grid units (96 units per inch).
    /// Observed in Windows Excel 16.0 build 20326: AutoFit reserves 12.5 pt at
    /// 100-200% zoom, 25 pt at 50%, regardless of header row height. Below 100%
    /// the dropdown keeps its screen width. This is geometry, not font padding.
    pub fn filter_button_space(&self, c: CellRef) -> f32 {
        if self.has_filter_button(c) { (12.5 * 96.0 / 72.0) / (self.zoom as f32 / 100.0).clamp(0.1, 1.0) } else { 0.0 }
    }

    pub fn col_width(&self, col: u32) -> f32 {
        match self.cols.get(&col) {
            Some(i) if i.hidden => 0.0,
            Some(LineInfo { size: Some(w), .. }) => *w,
            _ => self.default_col_width,
        }
    }
    pub fn row_height(&self, row: u32) -> f32 {
        match self.rows.get(&row) {
            Some(i) if i.hidden => 0.0,
            Some(LineInfo { size: Some(h), .. }) => *h,
            _ => self.default_row_height,
        }
    }
    pub fn is_row_hidden(&self, row: u32) -> bool {
        self.rows.get(&row).is_some_and(|r| r.hidden)
    }
    /// Whether `row` is hidden by an applied filter: a hidden row below the header of the
    /// sheet AutoFilter while it has criteria, or a hidden data row of a filtered table.
    /// Other hidden rows were hidden by hand.
    pub fn is_row_filtered(&self, row: u32) -> bool {
        self.is_row_hidden(row) && self.filter_row_spans().iter().any(|&(first, last)| row >= first && row <= last)
    }
    /// The row spans (first, last) an applied filter can hide: the data rows of the sheet
    /// AutoFilter while it has criteria, and of each filtered table.
    pub fn filter_row_spans(&self) -> Vec<(u32, u32)> {
        let sheet = self.autofilter.as_ref().filter(|af| !af.criteria.is_empty()).map(|af| (af.range.start.row.saturating_add(1), af.range.end.row));
        let tables = self.tables.iter().filter(|t| !t.filter.is_empty()).filter_map(|t| t.data_range()).map(|r| (r.start.row, r.end.row));
        sheet.into_iter().chain(tables).collect()
    }
    pub fn is_col_hidden(&self, col: u32) -> bool {
        self.cols.get(&col).is_some_and(|r| r.hidden)
    }
    /// Y offset (points) of the top of `row`.
    pub fn row_top(&self, row: u32) -> f64 {
        let mut y = row as f64 * self.default_row_height as f64;
        for (r, info) in self.rows.range(..row) {
            let _ = r;
            let h = if info.hidden { 0.0 } else { info.size.unwrap_or(self.default_row_height) };
            y += h as f64 - self.default_row_height as f64;
        }
        y
    }
    pub fn col_left(&self, col: u32) -> f64 {
        let mut x = col as f64 * self.default_col_width as f64;
        for (_, info) in self.cols.range(..col) {
            let w = if info.hidden { 0.0 } else { info.size.unwrap_or(self.default_col_width) };
            x += w as f64 - self.default_col_width as f64;
        }
        x
    }
    /// Row containing y (points from the top), clamped.
    pub fn row_at(&self, y: f64) -> u32 {
        line_at(y, self.default_row_height as f64, &self.rows, gridcraft_core::MAX_ROWS)
    }
    pub fn col_at(&self, x: f64) -> u32 {
        line_at(x, self.default_col_width as f64, &self.cols, gridcraft_core::MAX_COLS)
    }

    /// The merged range containing `c`, if any.
    pub fn merge_at(&self, c: CellRef) -> Option<RangeRef> {
        self.merges.iter().find(|m| m.contains(c)).copied()
    }
    pub fn table_at(&self, c: CellRef) -> Option<&Table> {
        self.tables.iter().find(|t| t.range.contains(c))
    }
    pub fn used_range(&self) -> Option<RangeRef> {
        let mut r = self.cells.used_range();
        for sr in self.spill_ranges.values() {
            r = Some(r.map_or(*sr, |x| x.union(sr)));
        }
        r
    }
    pub fn is_protected(&self) -> bool {
        self.protection.is_some()
    }
}

/// Prefix sums over the custom-sized lines of an axis: O(log n) position ↔ line lookups even
/// when every row has an explicit height (common in imported files).
#[derive(Clone, Debug, Default)]
pub struct LineIndex {
    default: f64,
    max: u32,
    /// (line, start position of that line, its size), sorted by line.
    lines: Vec<(u32, f64, f64)>,
}

impl LineIndex {
    pub fn new(custom: &BTreeMap<u32, LineInfo>, default: f32, max: u32) -> LineIndex {
        let default = default as f64;
        let mut lines = Vec::with_capacity(custom.len());
        let mut pos = 0.0;
        let mut prev = 0u32;
        for (&line, info) in custom {
            pos += (line - prev) as f64 * default;
            // A negative size (only a malformed file has one) counts as zero, so positions only
            // grow and the searches below stay valid.
            let size = if info.hidden { 0.0 } else { info.size.map_or(default, |s| s as f64).max(0.0) };
            lines.push((line, pos, size));
            pos += size;
            prev = line + 1;
        }
        LineIndex { default, max, lines }
    }
    /// Start position of `line`.
    pub fn start(&self, line: u32) -> f64 {
        match self.lines.binary_search_by(|e| e.0.cmp(&line)) {
            Ok(i) => self.lines.get(i).map_or(0.0, |e| e.1),
            Err(0) => line as f64 * self.default,
            Err(i) => match self.lines.get(i - 1) {
                Some(&(l, p, s)) => p + s + (line - l - 1) as f64 * self.default,
                None => line as f64 * self.default,
            },
        }
    }
    /// The first line at or after `line` (and before the axis end) with a size above zero, or
    /// `None` when every line from there on is hidden or zero-sized. O(log n): a run of hidden
    /// lines all start where the next shown line starts, so one search steps over the whole run.
    pub fn next_visible(&self, line: u32) -> Option<u32> {
        if line >= self.max {
            return None;
        }
        let i = self.lines.partition_point(|e| e.0 < line);
        let custom = self.lines.get(i).is_some_and(|e| e.0 == line);
        if !custom && self.default > 0.0 {
            return Some(line);
        }
        // Every line from `line` up to the answer is zero-sized, so they all start at `p`; the
        // last custom line starting at `p` either has a size (the answer) or is the end of the run.
        let p = self.start(line);
        let k = self.lines.partition_point(|e| e.1 <= p);
        let &(last, _, size) = self.lines.get(k.checked_sub(1)?)?;
        if last < line {
            return None;
        }
        if size > 0.0 {
            return Some(last).filter(|l| *l < self.max);
        }
        // The line after the run has no custom line (a custom one would start at `p` too and be
        // part of the run): it is shown when the default size is.
        let next = last.checked_add(1).filter(|n| *n < self.max)?;
        (self.default > 0.0).then_some(next)
    }
    /// The lines from `first` to `last` (inclusive) with a size above zero, in order. Runs of
    /// hidden lines cost one O(log n) step each, not one per line.
    pub fn visible(&self, first: u32, last: u32) -> impl Iterator<Item = u32> + '_ {
        let mut next = self.next_visible(first);
        std::iter::from_fn(move || {
            let line = next.filter(|l| *l <= last)?;
            next = line.checked_add(1).and_then(|n| self.next_visible(n));
            Some(line)
        })
    }
    /// The line containing `pos`.
    pub fn at(&self, pos: f64) -> u32 {
        if pos <= 0.0 {
            return 0;
        }
        // Last custom line starting at or before pos.
        let i = self.lines.partition_point(|e| e.1 <= pos);
        let (base_line, base_pos) = match i.checked_sub(1).and_then(|k| self.lines.get(k)) {
            Some(&(l, p, s)) => {
                if pos < p + s {
                    return l.min(self.max - 1);
                }
                (l + 1, p + s)
            }
            None => (0, 0.0),
        };
        if self.default <= 0.0 {
            return base_line.min(self.max - 1);
        }
        let k = ((pos - base_pos) / self.default).floor();
        let r = base_line as f64 + k;
        if r >= self.max as f64 { self.max - 1 } else { r as u32 }
    }
}

/// Inverse of the cumulative size along an axis with sparse custom sizes.
fn line_at(pos: f64, default: f64, custom: &BTreeMap<u32, LineInfo>, max: u32) -> u32 {
    if pos <= 0.0 {
        return 0;
    }
    let mut start_pos = 0.0; // position of line `idx`
    let mut idx: u32 = 0;
    for (&line, info) in custom.iter() {
        // Lines idx..line have default size.
        let span = (line - idx) as f64 * default;
        if pos < start_pos + span {
            let k = ((pos - start_pos) / default).floor() as u32;
            return (idx + k).min(max - 1);
        }
        start_pos += span;
        let size = if info.hidden { 0.0 } else { info.size.map_or(default, |s| s as f64) };
        if pos < start_pos + size {
            return line.min(max - 1);
        }
        start_pos += size;
        idx = line + 1;
    }
    if default <= 0.0 {
        return idx.min(max - 1);
    }
    let k = ((pos - start_pos) / default).floor();
    let r = idx as f64 + k;
    if r >= max as f64 { max - 1 } else { r as u32 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Formula;

    /// `LineIndex::visible` lists exactly the lines a line-by-line size check shows, for hidden
    /// runs at the start, in the middle and up to the end of the axis, custom sizes (zero and
    /// negative included), lines with only a style, and a zero default size.
    #[test]
    fn visible_lines_match_a_line_by_line_check() {
        let max = 400u32;
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        for case in 0..300 {
            let mut custom = BTreeMap::new();
            let runs = rnd(6);
            for _ in 0..runs {
                let start = rnd(u64::from(max)) as u32;
                let len = rnd(if case % 3 == 0 { 400 } else { 30 }) as u32;
                for l in start..(start + len).min(max) {
                    custom.insert(l, LineInfo { hidden: true, ..Default::default() });
                }
            }
            // Runs that start at the first line or end at the last one.
            if case % 5 == 0 {
                for l in 0..rnd(20) as u32 {
                    custom.insert(l, LineInfo { hidden: true, ..Default::default() });
                }
            }
            if case % 5 == 1 {
                for l in max - rnd(20) as u32 - 1..max {
                    custom.insert(l, LineInfo { hidden: true, ..Default::default() });
                }
            }
            for _ in 0..rnd(8) {
                let size = [Some(0.0), Some(3.0), Some(20.0), Some(-5.0), None][rnd(5) as usize];
                custom.insert(rnd(u64::from(max)) as u32, LineInfo { size, ..Default::default() });
            }
            let default = if case % 7 == 0 { 0.0 } else { 15.0 };
            let size = |l: u32| match custom.get(&l) {
                Some(i) if i.hidden => 0.0,
                Some(LineInfo { size: Some(s), .. }) => s.max(0.0),
                _ => default,
            };
            let index = LineIndex::new(&custom, default, max);
            for (first, last) in [(0, max - 1), (rnd(u64::from(max)) as u32, max - 1), (5, 5), (max - 1, max - 1)] {
                let want: Vec<u32> = (first..=last).filter(|&l| size(l) > 0.0).collect();
                assert_eq!(index.visible(first, last).collect::<Vec<_>>(), want, "case {case}, {first}..={last}");
            }
            assert_eq!(index.next_visible(max), None);
        }
        // The row after a million hidden rows comes straight after the last shown one.
        let custom: BTreeMap<u32, LineInfo> = (47..1_048_422).map(|r| (r, LineInfo { hidden: true, ..Default::default() })).collect();
        let index = LineIndex::new(&custom, 15.0, 1_048_576);
        assert_eq!(index.visible(40, 1_048_424).collect::<Vec<_>>(), [40, 41, 42, 43, 44, 45, 46, 1_048_422, 1_048_423, 1_048_424]);
    }

    #[test]
    fn a_sheet_keeps_its_identity_through_clones_and_a_new_sheet_gets_its_own() {
        let a = Sheet::new("A");
        let b = Sheet::new("A");
        assert_ne!(a.uid.get(), b.uid.get(), "two new sheets");
        assert_ne!(a.uid.get(), 0);
        // A clone (an undo snapshot, a rename through Arc::make_mut) is the same sheet.
        let mut renamed = a.clone();
        renamed.name = "B".into();
        assert_eq!(renamed.uid.get(), a.uid.get());
        // Not content: equal sheets compare equal whatever their identity.
        assert_eq!(a, b);
        // Not in JSON: a sheet read back is a new one.
        let json = serde_json::to_string(&a).unwrap();
        assert!(!json.contains("uid"), "{json}");
        let read: Sheet = serde_json::from_str(&json).unwrap();
        assert_ne!(read.uid.get(), a.uid.get());
        assert_eq!(read, a);
    }

    #[test]
    fn line_index_matches_linear() {
        let mut s = Sheet::new("S");
        for r in (0..5000).step_by(3) {
            s.rows.insert(r, LineInfo { size: Some(10.0 + (r % 7) as f32), hidden: r % 11 == 0, ..Default::default() });
        }
        let idx = LineIndex::new(&s.rows, s.default_row_height, gridcraft_core::MAX_ROWS);
        for r in [0u32, 1, 2, 3, 4, 100, 999, 4998, 5000, 6000] {
            assert!((idx.start(r) - s.row_top(r)).abs() < 1e-6, "start {r}");
        }
        for y in [0.0, 5.0, 33.3, 1000.0, 47_000.0, 90_000.0] {
            assert_eq!(idx.at(y), s.row_at(y), "at {y}");
        }
    }

    /// Reference implementation: one `value_ref` call per row, exactly the loop
    /// `last_contiguous_nonempty` replaces (from `grid.rs`'s fill-handle double-click).
    fn last_contiguous_nonempty_naive(s: &Sheet, col: u32, from_row: u32, cap: u32) -> u32 {
        let mut row = from_row;
        while row <= cap && s.value_ref(CellRef::new(row, col)).is_some_and(|v| !v.is_empty()) {
            row += 1;
        }
        row.saturating_sub(1).max(from_row.saturating_sub(1))
    }

    /// Deterministic xorshift32 so failures reproduce without pulling in a `rand` dependency
    /// just for this test.
    struct Xorshift32(u32);
    impl Xorshift32 {
        fn next(&mut self) -> u32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            self.0
        }
        fn below(&mut self, n: u32) -> u32 {
            self.next() % n.max(1)
        }
    }

    #[test]
    fn last_contiguous_nonempty_matches_naive() {
        let mut rng = Xorshift32(0x9e3779b9);
        for trial in 0..300u32 {
            let mut s = Sheet::new("S");
            let rows = rng.below(400);
            // A mix of directly-stored values, formulas (value empty but formula present),
            // spill-only cells, blank-but-stored cells, and gaps, like a real sheet.
            for r in 0..rows {
                match rng.below(5) {
                    0 => s.set_value(CellRef::new(r, 0), Value::Number(r as f64)),
                    1 => s.set_cell(CellRef::new(r, 0), Cell { formula: Some(Arc::new(Formula::new("1"))), ..Default::default() }),
                    2 => {
                        s.spill.insert(CellRef::new(r, 0), Value::Number(r as f64));
                    }
                    3 => {
                        // Stored but blank: does not count (matches `has_value`/`value_ref`).
                        s.set_value(CellRef::new(r, 0), Value::Text("".into()));
                    }
                    4 if rng.below(2) == 0 => {
                        // A style-only stored cell over a spill value: `value_ref` falls through
                        // to the spill value, so the row counts.
                        s.spill.insert(CellRef::new(r, 0), Value::Number(r as f64));
                        s.set_cell(CellRef::new(r, 0), Cell { style: StyleId(1), ..Default::default() });
                    }
                    _ => {} // gap
                }
            }
            for _ in 0..5 {
                let from_row = rng.below(420);
                let cap = from_row.saturating_add(rng.below(450));
                let got = s.last_contiguous_nonempty(0, from_row, cap);
                let want = last_contiguous_nonempty_naive(&s, 0, from_row, cap);
                assert_eq!(got, want, "trial {trial}: col 0, from_row {from_row}, cap {cap}, rows up to {rows}");
            }
        }
    }

    #[test]
    fn last_contiguous_nonempty_dense_run() {
        let mut s = Sheet::new("S");
        for r in 0..2000u32 {
            s.set_value(CellRef::new(r, 2), Value::Number(r as f64));
        }
        assert_eq!(s.last_contiguous_nonempty(2, 0, 1_999), 1999);
        assert_eq!(s.last_contiguous_nonempty(2, 0, 1_000), 1000); // capped
        assert_eq!(s.last_contiguous_nonempty(2, 500, 1_999), 1999);
        assert_eq!(s.last_contiguous_nonempty(2, 2000, 3000), 1999); // nothing at from_row
        assert_eq!(s.last_contiguous_nonempty(2, 2001, 3000), 2000); // from_row > cap: from_row - 1
    }

    #[test]
    fn last_contiguous_nonempty_formula_with_empty_value_ends_the_run() {
        let mut s = Sheet::new("S");
        s.set_value(CellRef::new(0, 1), Value::Number(1.0));
        s.set_value(CellRef::new(1, 1), Value::Number(2.0));
        // row 2: a formula (e.g. `=""`) whose cached value is empty. It does not count, same as
        // `value_ref(...).is_some_and(|v| !v.is_empty())` says for it, even though it has a
        // formula (unlike `CellStore::has`, used for Ctrl+arrow navigation, which counts it).
        s.set_cell(CellRef::new(2, 1), Cell { formula: Some(Arc::new(Formula::new("\"\""))), ..Default::default() });
        s.set_value(CellRef::new(3, 1), Value::Number(4.0)); // would extend the run if row 2 didn't stop it
        assert_eq!(s.last_contiguous_nonempty(1, 0, 100), 1);
        assert_eq!(s.last_contiguous_nonempty(1, 2, 100), 1); // from_row itself is the stop
    }

    #[test]
    fn last_contiguous_nonempty_spill_extends_a_style_only_stored_row() {
        let mut s = Sheet::new("S");
        s.set_value(CellRef::new(0, 1), Value::Number(1.0));
        // row 1: a formatted spill child: stored with a style only, value from `spill`.
        s.spill.insert(CellRef::new(1, 1), Value::Number(2.0));
        s.set_cell(CellRef::new(1, 1), Cell { style: StyleId(1), ..Default::default() });
        s.set_value(CellRef::new(2, 1), Value::Number(3.0));
        assert_eq!(s.last_contiguous_nonempty(1, 0, 100), 2);
        assert_eq!(s.last_contiguous_nonempty(1, 1, 100), 2);
    }

    #[test]
    fn last_contiguous_nonempty_spill_extends_an_unstored_row() {
        let mut s = Sheet::new("S");
        s.set_value(CellRef::new(0, 1), Value::Number(1.0));
        // row 1: nothing stored, but a spill value from an array formula anchored elsewhere.
        s.spill.insert(CellRef::new(1, 1), Value::Number(2.0));
        s.set_value(CellRef::new(2, 1), Value::Number(3.0));
        // row 3: a genuine gap (nothing stored, no spill).
        assert_eq!(s.last_contiguous_nonempty(1, 0, 100), 2);
        assert_eq!(s.last_contiguous_nonempty(1, 3, 100), 2); // from_row itself is the gap
    }

    #[test]
    fn geometry() {
        let mut s = Sheet::new("S");
        assert_eq!(s.row_top(3), 60.0);
        s.rows.insert(1, LineInfo { size: Some(40.0), ..Default::default() });
        s.rows.insert(2, LineInfo { hidden: true, ..Default::default() });
        assert_eq!(s.row_top(1), 20.0);
        assert_eq!(s.row_top(2), 60.0);
        assert_eq!(s.row_top(3), 60.0);
        assert_eq!(s.row_top(4), 80.0);
        assert_eq!(s.row_at(10.0), 0);
        assert_eq!(s.row_at(30.0), 1);
        assert_eq!(s.row_at(59.0), 1);
        assert_eq!(s.row_at(61.0), 3);
        assert_eq!(s.row_at(85.0), 4);
        assert_eq!(s.col_at(1e12), gridcraft_core::MAX_COLS - 1);
        assert_eq!(s.col_at(-5.0), 0);
        assert_eq!(s.col_left(2), 128.0);
    }

    use crate::features::{ChartKind, EquationCache, LegendPos, ShapeFormat, ShapeKind};

    /// Adds a floating object of `kind` with id `id` at `anchor`. The match is exhaustive on
    /// purpose: a new kind of floating object does not compile here until this test covers it,
    /// and the counts in `object_anchor_accessors_visit_the_same_objects_in_the_same_order`
    /// then catch an accessor that was not taught about it. An equation is a shape of
    /// `ShapeKind::Equation`, so it lives in `shapes` and is visited among them.
    fn push_object(s: &mut Sheet, kind: ObjKind, id: u32, anchor: Anchor) {
        match kind {
            ObjKind::Chart => s.charts.push(Chart {
                id,
                kind: ChartKind::default(),
                anchor,
                title: None,
                series: vec![],
                legend: LegendPos::default(),
                data_labels: false,
                data_table: None,
                gridlines: false,
                style: 0,
                hole_size: 75,
                first_slice_angle: 0,
                x_title: None,
                y_title: None,
                source: None,
                by_rows: false,
                pivot: None,
            }),
            ObjKind::Image => s.images.push(Image { id, anchor, data: Arc::new(vec![]), mime: String::new(), alt: String::new() }),
            ObjKind::Shape | ObjKind::Equation => s.shapes.push(Shape {
                id,
                kind: if kind == ObjKind::Equation { ShapeKind::Equation } else { ShapeKind::default() },
                anchor,
                fill: Color::default(),
                line: Color::default(),
                text: String::new(),
                rotation: 0.0,
                format: ShapeFormat::default(),
                math: EquationCache::default(),
            }),
            ObjKind::Slicer => {
                s.slicers.push(Slicer { id, table: String::new(), column: String::new(), caption: String::new(), anchor, selected: None, columns: 1 })
            }
            ObjKind::Control => s.controls.push(FormControl { id, anchor, ..Default::default() }),
        }
    }

    /// Sorting, and inserting or deleting rows and columns, read every anchor through
    /// `object_anchors`, compute the new ones and write them back through `object_anchors_mut`
    /// in step. The two must visit the same objects in the same order, or an object receives
    /// another object's anchor; a kind that reaches only one of them would break that silently.
    #[test]
    fn object_anchor_accessors_visit_the_same_objects_in_the_same_order() {
        let mut s = Sheet::new("S");
        // Two objects of every kind, inserted in a mixed order, with ids unrelated to the kind.
        // Each anchor cell's row carries the id, so an anchor read back names its object.
        let inserted = [
            (ObjKind::Slicer, 7),
            (ObjKind::Chart, 3),
            (ObjKind::Image, 11),
            (ObjKind::Shape, 2),
            (ObjKind::Chart, 9),
            (ObjKind::Slicer, 1),
            (ObjKind::Image, 4),
            (ObjKind::Equation, 6),
            (ObjKind::Control, 13),
            (ObjKind::Shape, 12),
            (ObjKind::Equation, 8),
            (ObjKind::Control, 15),
        ];
        for &(kind, id) in &inserted {
            push_object(&mut s, kind, id, Anchor { cell: CellRef::new(id, 0), ..Default::default() });
        }
        let per_kind = s.charts.len() + s.images.len() + s.shapes.len() + s.slicers.len() + s.controls.len();
        assert_eq!(per_kind, inserted.len());
        assert_eq!(s.object_anchors().count(), per_kind, "object_anchors visits every collection");
        assert_eq!(s.object_anchors_mut().count(), per_kind, "object_anchors_mut visits every collection");
        // Mark each anchor with its position through the mutable accessor; the shared accessor
        // reads the marks back in the same sequence.
        for (i, a) in s.object_anchors_mut().enumerate() {
            a.dx = i as f32;
        }
        let marks: Vec<f32> = s.object_anchors().map(|a| a.dx).collect();
        assert_eq!(marks, (0..per_kind).map(|i| i as f32).collect::<Vec<f32>>());
        // The sequence is the documented one (charts, images, shapes, slicers, controls, each in
        // insertion order; equations are shapes) and every object appears in it once.
        let collection = |k: ObjKind| if k == ObjKind::Equation { ObjKind::Shape } else { k };
        let documented: Vec<u32> = [ObjKind::Chart, ObjKind::Image, ObjKind::Shape, ObjKind::Slicer, ObjKind::Control]
            .iter()
            .flat_map(|&k| inserted.iter().filter(move |(kind, _)| collection(*kind) == k).map(|&(_, id)| id))
            .collect();
        let visited: Vec<u32> = s.object_anchors().map(|a| a.cell.row).collect();
        assert_eq!(visited, documented);
        // The mark written through the bulk accessor landed on the object `object_anchor` finds
        // by kind and id.
        for &(kind, id) in &inserted {
            let position = documented.iter().position(|&x| x == id).map(|p| p as f32);
            assert_eq!(s.object_anchor(ObjRef::new(kind, id)).map(|a| a.dx), position, "{kind:?} {id}");
        }
    }
}

// CellRef is a struct, so encode the sparse map as entries for JSON as well as other formats.
mod picture_map {
    use crate::cell::CellPicture;
    use gridcraft_core::CellRef;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    pub fn serialize<S: Serializer>(map: &BTreeMap<CellRef, Arc<CellPicture>>, serializer: S) -> Result<S::Ok, S::Error> {
        map.iter().collect::<Vec<_>>().serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<BTreeMap<CellRef, Arc<CellPicture>>, D::Error> {
        Ok(Vec::<(CellRef, Arc<CellPicture>)>::deserialize(deserializer)?.into_iter().collect())
    }
}
