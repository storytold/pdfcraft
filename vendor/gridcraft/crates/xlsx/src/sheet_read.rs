//! Reading worksheet parts.

use std::collections::HashMap;
use std::sync::Arc;

use craft_xlsx::{
    CellFormula as CraftFormula, CellValue as CraftValue, FormulaKind as CraftFormulaKind, Row as CraftRow, SheetEntry as CraftSheetEntry,
    SheetKind as CraftSheetKind, SheetPart as CraftSheetPart, Visibility as CraftVisibility,
};
use gridcraft_core::date::{serial_from_ymd, time_fraction};
use gridcraft_core::{CellError, CellRef, DateSystem, MAX_COLS, MAX_ROWS, RangeRef, Value};
use gridcraft_formula::Expr;
use gridcraft_model::{
    AutoFilter, Cell, CfOperator, CfRule, CfValueKind, Comment, CondFormat, EditRange, ErrorStyle, FilterCriterion, Formula, Hyperlink, LineInfo,
    NoteBox, NoteCorner, Orientation, PageOrder, PasswordHash, Sheet, SheetProtection, Sparkline, SparklineKind, Style, Table, TableColumn, TotalsFn,
    Validation, ValidationKind, XmlColumnPr,
};

use crate::IoError;
use crate::fmla::{from_file, unwrap_google_sheets};
use crate::package::{Rel, craft_error};
use crate::read::Ctx;
use crate::styles::read_color;
use crate::tables::{col_width_to_px, paper_name, pt_to_px};
use crate::xml::{self, El};

mod parts;
mod rules;

pub use parts::read_autofilter;
use parts::read_threaded;
use rules::{read_cf, read_validation, read_x14_cf};

struct Shared {
    anchor: CellRef,
    expr: Option<Expr>,
    text: String,
    unwrapped: bool,
    /// Approximate bytes one dependent cell costs (its printed text plus its expression tree);
    /// what exceeds [`SHARED_FORMULA_FREE_COST`] is charged against [`MAX_SHARED_FORMULA_BYTES`].
    cost: u64,
}

/// What one sheet's hyperlinks have copied so far, across all its `<hyperlinks>` blocks.
#[derive(Default)]
struct LinkBudget {
    cells: u64,
    bytes: u64,
}

#[derive(Default)]
struct RowState {
    shared: HashMap<u32, Shared>,
    /// Dynamic-array spill ranges and legacy array ranges: cached values inside them (other than
    /// the anchor) are not stored, so the formula can fill them again.
    dynamic: Vec<RangeRef>,
    /// Cells inside the ranges above, for a cap before a pathological sheet proves it itself.
    dynamic_cells: u64,
    /// Row-membership checks served so far, for the work cap above.
    dynamic_checks: u64,
    /// The cell being read lost its shared formula to a copy limit: its cached value is kept, even
    /// inside an array range (where a formula's cached values otherwise give way to the spill).
    shared_copy_refused: bool,
}

/// The known dynamic range `pos` lies in, when it isn't the range's anchor: the cell's cached
/// value gives way to the formula that will fill it (or, for a Python cell, becomes its
/// result). Charges every scanned range against one work budget, then gives up consulting the
/// ranges at all: the worst kind of file may then import a few cached spill values, but it
/// never stalls the whole import.
fn dynamic_range(st: &mut RowState, pos: CellRef) -> Option<RangeRef> {
    if st.dynamic_checks > MAX_DYNAMIC_CHECKS || st.dynamic.is_empty() {
        return None;
    }
    let ranges = st.dynamic.len();
    let hit = st.dynamic.iter().find(|r| r.contains(pos) && r.start != pos).copied();
    st.dynamic_checks = st.dynamic_checks.saturating_add(ranges as u64);
    hit
}

/// Records a dynamic-array range, so its cells outside the anchor import as empty and the
/// formula fills them again. Repeated identical anchors (a damaged or hostile sheet may repeat
/// the same record) are recorded once, and count and area are capped: every later cell inside
/// known range territory pays a scan proportional to what is recorded here.
fn push_dynamic(st: &mut RowState, range: RangeRef) {
    if st.dynamic.contains(&range) {
        return;
    }
    let cells = range.count();
    if st.dynamic.len() >= MAX_DYNAMIC_RANGES || st.dynamic_cells.saturating_add(cells) > MAX_DYNAMIC_CELLS {
        return;
    }
    st.dynamic_cells = st.dynamic_cells.saturating_add(cells);
    st.dynamic.push(range);
}

/// Forgets the range anchored at `pos` (a Python cell whose code the file doesn't hold, which
/// keeps only its cached value): the cached values over the rest of its spill are kept as plain
/// values too, as the file shows them, rather than left for a result that won't come. Charged
/// like [`dynamic_range`]'s scans.
fn forget_dynamic(st: &mut RowState, pos: CellRef) {
    if st.dynamic_checks > MAX_DYNAMIC_CHECKS {
        return;
    }
    st.dynamic_checks = st.dynamic_checks.saturating_add(st.dynamic.len() as u64);
    if let Some(i) = st.dynamic.iter().rposition(|r| r.start == pos) {
        st.dynamic.remove(i);
    }
}

/// Records the spill range of the Python cell read at `pos`, its anchor, when the file declares
/// one that a result can fill: the spill then exists even when the file holds no records for its
/// blank cells. Charged like [`dynamic_range`]'s scans.
fn record_python_spill(st: &mut RowState, sheet: &mut Sheet, pos: CellRef) {
    if st.dynamic_checks > MAX_DYNAMIC_CHECKS {
        return;
    }
    st.dynamic_checks = st.dynamic_checks.saturating_add(st.dynamic.len() as u64);
    let Some(r) = st.dynamic.iter().rev().find(|r| r.start == pos).copied() else { return };
    if r.count() > 1 && r.count() <= gridcraft_model::python::MAX_RESULT_CELLS as u64 {
        sheet.spill_ranges.insert(pos, r);
    }
}

/// Past the scan budget, the cells of a Python spill read afterwards were kept as plain cells, not
/// as its spilled values, and a result rebuilt over them would block itself. Every Python spill of
/// such a sheet keeps its values as plain values instead, with no live spill.
fn settle_python_spills_past_budget(st: &RowState, sheet: &mut Sheet) {
    if st.dynamic_checks <= MAX_DYNAMIC_CHECKS {
        return;
    }
    let anchors: Vec<CellRef> = sheet.spill_ranges.keys().copied().filter(|a| sheet.cells.get(*a).is_some_and(|c| c.py.is_some())).collect();
    for anchor in anchors {
        sheet.settle_spill(anchor);
    }
}

/// The most cell elements read from one workbook, across its sheets: the reader's own cap.
/// (`calc::eval::MAX_CELLS` has the same value but limits one array result, not a workbook.)
pub(crate) const MAX_CELLS: u64 = 16_000_000;
/// Target and tooltip bytes kept for hyperlinks of one sheet, over all cells: the same URL is
/// copied for every cell of its range, so length × cells must stay bounded whatever the file.
const MAX_HYPERLINK_BYTES: u64 = 8 * 1024 * 1024;
/// Most cells one sheet's hyperlinks may cover (each cell gets its own copy of the link).
const MAX_HYPERLINK_CELLS: u64 = 10_000;
/// Longest hyperlink target kept, in bytes (Excel's own limit is about 2,079 characters).
const MAX_HYPERLINK_TARGET: usize = 8_192;
/// Longest hyperlink ScreenTip kept, in characters (Excel's limit).
const MAX_TOOLTIP_CHARS: usize = 255;
/// What a dependent of a shared formula may cost for free: an ordinary formula cell's worth (by
/// [`shared_cost`], a formula of about 40 operators and operands), so a sheet of many ordinary
/// shared formulas is never cut short however many cells share them.
const SHARED_FORMULA_FREE_COST: u64 = 4096;
/// Budget, per workbook, for what shared formulas cost beyond [`SHARED_FORMULA_FREE_COST`] per
/// dependent cell: a long master copied into many cells is where the expansion multiplies the
/// file's bytes, and this bounds it to well within a web build's memory.
pub const MAX_SHARED_FORMULA_BYTES: u64 = 256 << 20;
const MAX_COLUMN_ASSIGNMENTS: u64 = 1_000_000;
/// Most dynamic-array and legacy-array ranges of one sheet.
const MAX_DYNAMIC_RANGES: usize = 65_536;
/// Worst-case cells inside the dynamic ranges the sheet declares. Beyond it further ranges are
/// dropped: they would cost their storage in every membership lookup that follows.
const MAX_DYNAMIC_CELLS: u64 = 16_000_000;
/// Most row-membership checks the sheet's ranges may serve. Beyond it the remaining cells are
/// imported with their cached values instead of being blanked for a spill that may still come,
/// so adversarial range counts can't make each cell scan a long list (a real workbook has a
/// handful of ranges).
const MAX_DYNAMIC_CHECKS: u64 = 16_000_000;
/// Most conditional-format range cells one sheet keeps, over all rules of an element: every
/// rule copies its element's whole range list, so rules × ranges must stay bounded.
const MAX_COND_CELLS: u64 = 4_000_000;

fn from_craft_el(e: &craft_xml::El) -> El {
    let name = e.name.split_once(':').map_or(e.name.as_str(), |(_, local)| local).to_string();
    let attrs = e.attrs.iter().map(|(k, v)| (k.split_once(':').map_or(k.as_str(), |(_, local)| local).to_string(), v.clone())).collect();
    let mut children = Vec::new();
    let mut text = String::new();
    for node in &e.children {
        match node {
            craft_xml::Node::El(c) => children.push(from_craft_el(c)),
            craft_xml::Node::Text(t) | craft_xml::Node::CData(t) => text.push_str(t),
            craft_xml::Node::Pi { .. } | craft_xml::Node::Comment(_) => {}
        }
    }
    El { name, attrs, children, text }
}

fn to_range_ref(r: craft_xlsx::Range) -> RangeRef {
    RangeRef::new(CellRef::new(r.first.row, r.first.col), CellRef::new(r.last.row, r.last.col))
}

fn map_craft_error(e: craft_xlsx::CellError) -> CellError {
    match e {
        craft_xlsx::CellError::Null => CellError::Null,
        craft_xlsx::CellError::Div0 => CellError::Div0,
        craft_xlsx::CellError::Value => CellError::Value,
        craft_xlsx::CellError::Ref => CellError::Ref,
        craft_xlsx::CellError::Name => CellError::Name,
        craft_xlsx::CellError::Num => CellError::Num,
        craft_xlsx::CellError::NA => CellError::NA,
        craft_xlsx::CellError::GettingData => CellError::GettingData,
        craft_xlsx::CellError::Spill => CellError::Spill,
        craft_xlsx::CellError::Calc => CellError::Calc,
        _ => CellError::Value,
    }
}

/// Reads the sheet part `part`, with the VML shape numbers of the form controls read into the
/// model (pass-through keeps the other controls).
pub fn read_sheet(cx: &mut Ctx<'_>, part: &str, name: &str) -> Result<(Sheet, crate::controls::ReadControls), IoError> {
    if !cx.pkg.has(part) {
        return Err(IoError::Format(format!("missing part {part}")));
    }
    let rels = cx.pkg.rels(part)?;
    let mut sheet = Sheet::new(name);
    let mut st = RowState::default();
    let entry = CraftSheetEntry {
        name: name.to_string(),
        sheet_id: 0,
        rel_id: String::new(),
        kind: CraftSheetKind::Worksheet,
        visibility: CraftVisibility::Visible,
        part: Some(part.to_string()),
    };
    let prev_warnings = cx.pkg.craft_mut().warnings().len();
    let mut remaining_cells = MAX_CELLS.saturating_sub(cx.cells);
    let mut rows = Vec::new();
    let sheet_part = cx
        .pkg
        .craft_mut()
        .sheet(&entry, |row| {
            let n = row.cells.len() as u64;
            if n > remaining_cells {
                return Err(craft_xlsx::Error::TooLarge(format!("the workbook has more than {MAX_CELLS} cells")));
            }
            remaining_cells -= n;
            rows.push(row);
            Ok(())
        })
        .map_err(|e| match e {
            craft_xlsx::Error::TooLarge(m) if m.contains("XML") || m.contains("node") || m.contains("nested") => {
                IoError::Format(format!("sheet \"{name}\" is damaged ({m})"))
            }
            other => craft_error(other),
        })?;
    let total_cells: u64 = rows.iter().map(|r| r.cells.len() as u64).sum();
    if sheet_part.dropped_cells > 0 && sheet_part.dropped_cells.saturating_add(total_cells) > crate::xml::MAX_XML_NODES as u64 {
        cx.warn(format!("sheet \"{name}\" is damaged: more than {} cells", crate::xml::MAX_XML_NODES));
        return Ok((Sheet::new(name), Default::default()));
    }
    let too_many_merges = sheet_part.merges.len() > crate::xml::MAX_XML_NODES;
    if too_many_merges {
        cx.warn(format!("sheet \"{name}\" is damaged: more than {} merge cells", crate::xml::MAX_XML_NODES));
    }
    for row in &rows {
        read_craft_row(cx, &mut sheet, &mut st, row).map_err(craft_error)?;
    }
    charge_cells(cx, sheet_part.dropped_cells)?;
    let new_warnings: Vec<String> = cx.pkg.craft_mut().warnings().get(prev_warnings..).unwrap_or_default().to_vec();
    for w in new_warnings {
        cx.warn(w);
    }
    settle_python_spills_past_budget(&st, &mut sheet);

    apply_craft_sheet_part(cx, &sheet_part, &rels, name, too_many_merges, &mut sheet)?;
    let root = from_craft_el(&sheet_part.root);
    let read_controls = read_form_controls(cx, &root, &rels, &mut sheet);
    read_sheet_notes_and_tables(cx, &sheet_part, &rels, &mut sheet)?;
    Ok((sheet, read_controls))
}

fn apply_craft_sheet_part(
    cx: &mut Ctx<'_>,
    sp: &CraftSheetPart,
    rels: &[Rel],
    name: &str,
    skip_merges: bool,
    sheet: &mut Sheet,
) -> Result<(), IoError> {
    let mut rel_by_id: HashMap<&str, &Rel> = HashMap::with_capacity(rels.len());
    for r in rels {
        rel_by_id.entry(r.id.as_str()).or_insert(r);
    }
    if let Some(fmt) = &sp.features.format {
        if let Some(h) = fmt.default_row_height {
            let px = pt_to_px(h);
            if px > 0.0 {
                sheet.default_row_height = px;
            }
        }
        if let Some(w) = fmt.default_col_width {
            let px = col_width_to_px(w);
            if px > 0.0 {
                sheet.default_col_width = px;
            }
        } else if let Some(b) = fmt.base_col_width {
            let px = ((f64::from(b.min(255)) * 7.0 + 5.0) / 8.0).ceil() * 8.0;
            sheet.default_col_width = px as f32;
        }
    }
    let cols_blocks = sp.root.children.iter().filter(|n| matches!(n, craft_xml::Node::El(e) if e.name == "cols")).count();
    if cols_blocks > 1 {
        apply_xml_cols(cx, sp, sheet)?;
    } else {
        apply_craft_cols(cx, &sp.columns, sheet)?;
    }
    if !skip_merges {
        for m in &sp.merges {
            if !m.is_single() {
                sheet.merges.push(to_range_ref(*m));
            }
        }
    }
    if let Some(v) = sp.features.views.first() {
        apply_craft_view(v, sheet);
    }
    let mut fit_to_page = false;
    let mut cf_kept_cells = 0u64;
    cx.cf_ext_ids.clear();
    for cf in &sp.features.conditional_formats {
        read_cf(cx, &from_craft_el(&cf.to_el()), sheet, &mut cf_kept_cells);
    }
    if let Some(vals) = &sp.features.validations {
        for v in &vals.items {
            if let Some(val) = read_validation(&from_craft_el(&v.to_el())) {
                sheet.validations.push(val);
            }
        }
    }
    for node in &sp.root.children {
        let craft_xml::Node::El(child) = node else { continue };
        match child.name.as_str() {
            "sheetPr" => {
                let e = from_craft_el(child);
                if let Some(t) = e.child("tabColor") {
                    sheet.tab_color = Some(read_color(t, &cx.palette));
                }
                if let Some(o) = e.child("outlinePr") {
                    sheet.summary_below = o.flag("summaryBelow", true);
                    sheet.summary_right = o.flag("summaryRight", true);
                }
                fit_to_page = e.child("pageSetUpPr").is_some_and(|p| p.flag("fitToPage", false));
            }
            "sheetProtection" => {
                let e = from_craft_el(child);
                if e.flag("sheet", false) {
                    sheet.protection = Some(SheetProtection {
                        password: read_password(&e, ""),
                        legacy_password_hash: None,
                        select_locked: !e.flag("selectLockedCells", false),
                        select_unlocked: !e.flag("selectUnlockedCells", false),
                        format_cells: !e.flag("formatCells", true),
                        format_columns: !e.flag("formatColumns", true),
                        format_rows: !e.flag("formatRows", true),
                        insert_columns: !e.flag("insertColumns", true),
                        insert_rows: !e.flag("insertRows", true),
                        delete_columns: !e.flag("deleteColumns", true),
                        delete_rows: !e.flag("deleteRows", true),
                        sort: !e.flag("sort", true),
                        autofilter: !e.flag("autoFilter", true),
                    });
                }
            }
            "protectedRanges" => read_protected_ranges(&from_craft_el(child), sheet),
            "autoFilter" => sheet.autofilter = read_autofilter(cx, &from_craft_el(child)),
            "extLst" => read_ext(cx, &from_craft_el(child), sheet, &mut cf_kept_cells),
            _ => {}
        }
    }
    let mut link_budget = LinkBudget::default();
    let hyperlink_blocks = sp.root.children.iter().filter(|n| matches!(n, craft_xml::Node::El(e) if e.name == "hyperlinks")).count();
    if hyperlink_blocks > 1 {
        for node in &sp.root.children {
            if let craft_xml::Node::El(hl) = node
                && hl.name == "hyperlinks"
            {
                let hl_el = from_craft_el(hl);
                for h in hl_el.kids("hyperlink") {
                    let Some(range) = h.attr("ref").and_then(RangeRef::parse) else { continue };
                    let raw = RawHyperlink { range, rel_id: h.attr("id"), location: h.attr("location"), tooltip: h.attr("tooltip") };
                    if !push_hyperlink(cx, name, raw, &rel_by_id, sheet, &mut link_budget) {
                        break;
                    }
                }
            }
        }
    } else {
        read_craft_hyperlinks(cx, name, &sp.features.hyperlinks, &rel_by_id, sheet, &mut link_budget);
    }
    apply_craft_print(&sp.features.print, fit_to_page, sheet);
    if let Some(id) = &sp.features.drawing
        && let Some(rel) = rel_by_id.get(id.as_str()).copied()
    {
        let target = rel.target.clone();
        crate::drawing::read_drawing(cx, &target, sheet)?;
    }
    Ok(())
}

fn read_sheet_notes_and_tables(cx: &mut Ctx<'_>, sp: &CraftSheetPart, rels: &[Rel], sheet: &mut Sheet) -> Result<(), IoError> {
    let tables = cx.pkg.craft_mut().tables(sp).map_err(craft_error)?;
    for (_, t) in tables {
        let mut model = parts::from_craft_table(cx, &t);
        model.id = cx.next_object_id();
        sheet.tables.push(model);
    }
    if let Some(comments) = cx.pkg.craft_mut().comments(sp).map_err(craft_error)? {
        parts::apply_craft_comments(&comments, sheet);
    }
    for rel in rels.iter().filter(|r| r.kind == "threadedComment") {
        if let Some(x) = cx.optional_xml(&rel.target)? {
            read_threaded(cx, &x, sheet);
        }
    }
    if !sheet.comments.is_empty() {
        match cx.pkg.craft_mut().note_shapes(sp) {
            Ok(shapes) => parts::apply_craft_note_shapes(&shapes, sheet),
            Err(e) => cx.warn(format!("sheet \"{}\": note drawing could not be read ({e})", sheet.name)),
        }
    }
    Ok(())
}

struct RawCol {
    min: u32,
    max: u32,
    width: Option<f64>,
    custom_width: bool,
    hidden: bool,
    outline_level: u8,
    collapsed: bool,
    style: Option<u32>,
}

fn apply_col_entry(cx: &Ctx<'_>, c: RawCol, remaining: &mut u64, sheet: &mut Sheet) -> Result<(), IoError> {
    let min = c.min;
    let max = c.max.min(MAX_COLS);
    if min == 0 || min > MAX_COLS || max < min {
        return Ok(());
    }
    let mut info = LineInfo::default();
    if let Some(w) = c.width {
        let px = col_width_to_px(w);
        if c.custom_width || px != sheet.default_col_width {
            info.size = Some(px);
        }
    }
    info.hidden = c.hidden;
    info.outline = c.outline_level.min(7);
    info.collapsed = c.collapsed;
    if let Some(s) = c.style {
        let id = cx.style(Some(s));
        if id != gridcraft_model::StyleId::DEFAULT {
            info.style = Some(id);
        }
    }
    if info == LineInfo::default() {
        return Ok(());
    }
    *remaining = remaining
        .checked_sub(u64::from(max - min + 1))
        .ok_or_else(|| IoError::TooLarge(format!("sheet \"{}\": more than {MAX_COLUMN_ASSIGNMENTS} expanded column assignments", sheet.name)))?;
    for col in (min - 1)..max {
        sheet.cols.insert(col, info);
    }
    Ok(())
}

fn apply_craft_cols(cx: &Ctx<'_>, cols: &[craft_xlsx::Column], sheet: &mut Sheet) -> Result<(), IoError> {
    let mut remaining = MAX_COLUMN_ASSIGNMENTS;
    for c in cols {
        let raw = RawCol {
            min: c.first + 1,
            max: c.last + 1,
            width: c.width,
            custom_width: c.custom_width,
            hidden: c.hidden,
            outline_level: c.outline_level,
            collapsed: c.collapsed,
            style: c.style,
        };
        apply_col_entry(cx, raw, &mut remaining, sheet)?;
    }
    Ok(())
}

fn apply_xml_cols(cx: &Ctx<'_>, sp: &CraftSheetPart, sheet: &mut Sheet) -> Result<(), IoError> {
    let mut remaining = MAX_COLUMN_ASSIGNMENTS;
    for node in &sp.root.children {
        if let craft_xml::Node::El(cols_el) = node
            && cols_el.name == "cols"
        {
            let cols = from_craft_el(cols_el);
            for c in cols.kids("col") {
                let raw = RawCol {
                    min: c.attr_u32("min").unwrap_or(0),
                    max: c.attr_u32("max").unwrap_or(0),
                    width: c.attr_f64("width"),
                    custom_width: c.flag("customWidth", false),
                    hidden: c.flag("hidden", false),
                    outline_level: c.attr_u32("outlineLevel").unwrap_or(0).min(7) as u8,
                    collapsed: c.flag("collapsed", false),
                    style: c.attr_u32("style"),
                };
                apply_col_entry(cx, raw, &mut remaining, sheet)?;
            }
        }
    }
    Ok(())
}

fn apply_craft_view(v: &craft_xlsx::SheetView, sheet: &mut Sheet) {
    sheet.show_gridlines = v.show_grid_lines.unwrap_or(true);
    sheet.show_headings = v.show_row_col_headers.unwrap_or(true);
    sheet.show_formulas = v.show_formulas.unwrap_or(false);
    sheet.show_zeros = v.show_zeros.unwrap_or(true);
    sheet.show_outline_symbols = v.show_outline_symbols.unwrap_or(true);
    sheet.right_to_left = v.right_to_left.unwrap_or(false);
    sheet.view = v.view.as_deref().and_then(gridcraft_model::SheetViewMode::parse).unwrap_or_default();
    if let Some(z) = v.zoom_scale {
        sheet.zoom = z.clamp(10, 400) as u16;
    }
    if let Some(c) = v.top_left_cell.as_deref().and_then(CellRef::parse) {
        sheet.view_top_left = c;
    }
    let mut active_pane = None;
    if let Some(p) = &v.pane {
        if matches!(p.state.as_deref(), Some("frozen" | "frozenSplit")) {
            let cols = p.x_split.unwrap_or(0.0).clamp(0.0, (MAX_COLS - 1) as f64) as u32;
            let rows = p.y_split.unwrap_or(0.0).clamp(0.0, (MAX_ROWS - 1) as f64) as u32;
            if rows > 0 || cols > 0 {
                sheet.freeze = Some((rows, cols));
                if let Some(c) = p.top_left_cell.as_deref().and_then(CellRef::parse) {
                    sheet.view_top_left = c;
                }
            }
        }
        active_pane = p.active_pane.as_deref();
    }
    let sel = v.selections.iter().find(|s| s.pane.as_deref() == active_pane).or_else(|| v.selections.last());
    if let Some(c) = sel.and_then(|s| s.active_cell.as_deref()).and_then(CellRef::parse) {
        sheet.view_active = c;
    }
}

fn apply_craft_print(print: &craft_xlsx::Print, fit_to_page: bool, sheet: &mut Sheet) {
    if let Some(o) = &print.options {
        sheet.print.gridlines = o.grid_lines.unwrap_or(false);
        sheet.print.headings = o.headings.unwrap_or(false);
        sheet.print.center_h = o.horizontal_centered.unwrap_or(false);
        sheet.print.center_v = o.vertical_centered.unwrap_or(false);
    }
    if let Some(pm) = &print.margins {
        for (i, v) in [pm.left, pm.right, pm.top, pm.bottom, pm.header, pm.footer].into_iter().enumerate() {
            if let (Some(v), Some(slot)) = (v, sheet.print.margins.get_mut(i)) {
                *slot = v.clamp(0.0, 50.0) as f32;
            }
        }
    }
    if let Some(ps) = &print.setup {
        if ps.orientation.as_deref() == Some("landscape") {
            sheet.print.orientation = Orientation::Landscape;
        }
        sheet.print.page_order = ps.page_order.as_deref().and_then(PageOrder::parse).unwrap_or_default();
        sheet.print.first_page_number = if ps.use_first_page_number.unwrap_or(false) { ps.first_page_number } else { None };
        if let Some(p) = ps.paper_size.and_then(paper_name) {
            sheet.print.paper = p.to_string();
        }
        if let Some(s) = ps.scale {
            sheet.print.scale = s.clamp(10, 400) as u16;
        }
        if fit_to_page {
            let w = ps.fit_to_width.unwrap_or(1).min(32767) as u16;
            let h = ps.fit_to_height.unwrap_or(1).min(32767) as u16;
            sheet.print.fit_to = Some((w, h));
        }
    }
    if let Some(hf) = &print.header_footer {
        let pr = &mut sheet.print;
        for (src, slot) in [
            (&hf.odd_header, &mut pr.header),
            (&hf.odd_footer, &mut pr.footer),
            (&hf.even_header, &mut pr.even_header),
            (&hf.even_footer, &mut pr.even_footer),
            (&hf.first_header, &mut pr.first_header),
            (&hf.first_footer, &mut pr.first_footer),
        ] {
            *slot = src.as_deref().unwrap_or("").chars().take(255).collect();
        }
        pr.different_first = hf.different_first.unwrap_or(false);
        pr.different_odd_even = hf.different_odd_even.unwrap_or(false);
        pr.hf_scale_with_doc = hf.scale_with_doc.unwrap_or(true);
        pr.hf_align_with_margins = hf.align_with_margins.unwrap_or(true);
    }
    sheet.print.row_breaks = print.row_breaks.iter().filter_map(|b| b.id).filter(|&r| r > 0 && r < MAX_ROWS).collect();
    sheet.print.col_breaks = print.col_breaks.iter().filter_map(|b| b.id).filter(|&c| c > 0 && c < MAX_COLS).collect();
}

struct RawHyperlink<'a> {
    range: RangeRef,
    rel_id: Option<&'a str>,
    location: Option<&'a str>,
    tooltip: Option<&'a str>,
}

fn push_hyperlink(cx: &mut Ctx<'_>, name: &str, h: RawHyperlink<'_>, rels: &HashMap<&str, &Rel>, sheet: &mut Sheet, budget: &mut LinkBudget) -> bool {
    let rel_target = h.rel_id.and_then(|id| rels.get(id)).map(|r| r.target.as_str()).unwrap_or("");
    let location = h.location.filter(|l| !l.is_empty());
    if rel_target.len().saturating_add(location.map_or(0, str::len)) > MAX_HYPERLINK_TARGET {
        cx.warn(format!("sheet \"{name}\": hyperlinks longer than {MAX_HYPERLINK_TARGET} characters were dropped"));
        return true;
    }
    let mut target = rel_target.to_string();
    let external = !target.is_empty();
    if let Some(loc) = location {
        if target.is_empty() {
            target = loc.trim_start_matches('#').to_string();
        } else {
            target = format!("{target}#{loc}");
        }
    }
    if target.is_empty() {
        return true;
    }
    let tooltip = h.tooltip.map(|t| truncate_chars(t, MAX_TOOLTIP_CHARS));
    let link_bytes = (target.len() + tooltip.as_ref().map_or(0, String::len)) as u64;
    let link = Hyperlink { target, tooltip, external };
    for c in h.range.iter() {
        if budget.cells >= MAX_HYPERLINK_CELLS {
            cx.warn(format!("sheet \"{name}\": hyperlinks past the first {MAX_HYPERLINK_CELLS} cells were dropped"));
            return false;
        }
        if budget.bytes.saturating_add(link_bytes) > MAX_HYPERLINK_BYTES {
            cx.warn(format!("sheet \"{name}\": hyperlink targets past {MAX_HYPERLINK_BYTES} bytes were dropped"));
            return false;
        }
        budget.cells += 1;
        budget.bytes += link_bytes;
        sheet.hyperlinks.insert(c, link.clone());
    }
    true
}

fn read_craft_hyperlinks(
    cx: &mut Ctx<'_>,
    name: &str,
    links: &[craft_xlsx::Hyperlink],
    rels: &HashMap<&str, &Rel>,
    sheet: &mut Sheet,
    budget: &mut LinkBudget,
) {
    for h in links {
        let raw = RawHyperlink {
            range: to_range_ref(h.range),
            rel_id: h.rel_id.as_deref(),
            location: h.location.as_deref(),
            tooltip: h.tooltip.as_deref(),
        };
        if !push_hyperlink(cx, name, raw, rels, sheet, budget) {
            return;
        }
    }
}

/// Buttons, scroll bars and spin buttons: the `<controls>` list (also inside
/// `mc:AlternateContent`) with its properties parts, else the shapes of the VML drawing that
/// `<legacyDrawing r:id>` names (not the header and footer drawing of `<legacyDrawingHF>`).
/// Returns the VML shape numbers read, so pass-through keeps only the others.
fn read_form_controls(cx: &mut Ctx<'_>, root: &El, rels: &[Rel], sheet: &mut Sheet) -> crate::controls::ReadControls {
    let controls = root.children.iter().filter_map(crate::controls::controls_of).next_back();
    let vml = root.child("legacyDrawing").and_then(|e| e.attr("id"));
    if controls.is_some() || vml.is_some() { crate::controls::read(cx, controls, vml, rels, sheet) } else { Default::default() }
}

/// A protection password hash: the iterated attributes (`algorithmName`, `hashValue`,
/// `saltValue`, `spinCount`) or the legacy 16-bit `password`, each name preceded by `prefix`
/// (`workbook` gives `workbookHashValue`…). One that can't be read keeps a hash that never
/// matches, so what it protects stays locked rather than open.
pub(crate) fn read_password(e: &El, prefix: &str) -> Option<PasswordHash> {
    let attr = |base: &str| e.attr(&password_attr(prefix, base));
    if attr("algorithmName").is_some() || attr("hashValue").is_some() {
        return Some(PasswordHash::Iterated {
            algorithm: attr("algorithmName").unwrap_or("").to_string(),
            hash: attr("hashValue").unwrap_or("").to_string(),
            salt: attr("saltValue").unwrap_or("").to_string(),
            spin_count: e.attr_u32(&password_attr(prefix, "spinCount")).unwrap_or(0),
        });
    }
    attr("password").map(|h| match u16::from_str_radix(h.trim(), 16) {
        Ok(h) => PasswordHash::Legacy(h),
        Err(_) => PasswordHash::Iterated { algorithm: String::new(), hash: String::new(), salt: String::new(), spin_count: 0 },
    })
}

/// `base` preceded by `prefix` in camel case: ("workbook", "hashValue") → `workbookHashValue`.
pub(crate) fn password_attr(prefix: &str, base: &str) -> String {
    let mut chars = base.chars();
    match chars.next() {
        Some(c) if !prefix.is_empty() => format!("{prefix}{}{}", c.to_ascii_uppercase(), chars.as_str()),
        _ => base.to_string(),
    }
}

/// Allow Edit Ranges. A range whose password can't be read keeps a hash that never matches
/// (or an unsupported algorithm), so it stays locked rather than open.
fn read_protected_ranges(e: &El, sheet: &mut Sheet) {
    for p in e.kids("protectedRange") {
        let ranges: Vec<RangeRef> = p.attr("sqref").unwrap_or("").split_whitespace().filter_map(RangeRef::parse).collect();
        if ranges.is_empty() {
            continue;
        }
        let password = read_password(p, "");
        let title = p.attr("name").unwrap_or("").to_string();
        sheet.edit_ranges.push(EditRange { title, ranges, password, security_descriptor: p.attr("securityDescriptor").map(str::to_string) });
    }
}

/// Charges `n` cell elements against the workbook's [`MAX_CELLS`].
fn charge_cells(cx: &mut Ctx<'_>, n: u64) -> Result<(), IoError> {
    cx.cells = cx.cells.saturating_add(n);
    if cx.cells > MAX_CELLS {
        return Err(IoError::TooLarge(format!("the workbook has more than {MAX_CELLS} cells")));
    }
    Ok(())
}

fn read_craft_row(cx: &mut Ctx<'_>, sheet: &mut Sheet, st: &mut RowState, row: &CraftRow) -> Result<(), craft_xlsx::Error> {
    let r = row.index;
    let mut info = LineInfo::default();
    if let Some(ht) = row.height {
        info.custom = row.custom_height;
        if info.custom || (pt_to_px(ht) - sheet.default_row_height).abs() > 0.5 {
            info.size = Some(pt_to_px(ht));
        }
    }
    info.hidden = row.hidden;
    info.outline = row.outline_level.min(7);
    info.collapsed = row.collapsed;
    if let Some(s) = row.style {
        info.style = Some(cx.style(Some(s)));
    }
    if info != LineInfo::default() {
        sheet.rows.insert(r, info);
    }
    for c in &row.cells {
        charge_cells(cx, 1).map_err(|e| craft_xlsx::Error::TooLarge(e.to_string()))?;
        let pos = CellRef::new(c.at.row, c.at.col);
        read_craft_cell(cx, sheet, st, c, pos);
    }
    Ok(())
}

fn read_craft_cell(cx: &mut Ctx<'_>, sheet: &mut Sheet, st: &mut RowState, c: &craft_xlsx::Cell, pos: CellRef) {
    let style = cx.style((c.style != 0).then_some(c.style));
    let value = match &c.value {
        CraftValue::Empty => Value::Empty,
        CraftValue::Shared(i) => match cx.sst.get(*i as usize) {
            Some(s) => Value::Text(s.clone()),
            None => {
                cx.warn("some cells refer to missing shared strings");
                Value::Empty
            }
        },
        CraftValue::Text(t) => Value::Text(t.as_ref().into()),
        CraftValue::Inline(s) => Value::Text(s.text.as_ref().into()),
        CraftValue::Bool(b) => Value::Bool(*b),
        CraftValue::Error(e) => Value::Error(map_craft_error(*e)),
        CraftValue::Date(d) => match parse_iso_datetime(d, cx.date_system) {
            Some(n) => Value::number(n),
            None => {
                cx.warn("some date cells could not be read");
                Value::Empty
            }
        },
        CraftValue::Number(n) => Value::Number(*n),
        CraftValue::Invalid(_) => {
            cx.warn("some numeric cells could not be read");
            Value::Empty
        }
    };
    let picture = c.value_metadata.and_then(|vm| {
        if vm == 0 {
            return None;
        }
        let picture = (vm as usize).checked_sub(1).and_then(|n| cx.cell_pictures.get(n)).cloned().flatten();
        if picture.is_none() {
            cx.warn("some cells have unsupported or missing value metadata; kept their scalar fallback (cell pictures may be missing)");
        }
        picture
    });
    if let Some(picture) = picture {
        if c.formula.is_some() {
            cx.warn("formula-generated cell pictures were imported as static pictures; their formulas were not retained");
        }
        sheet.set_picture(pos, picture);
        sheet.set_style(pos, style);
        return;
    }
    let dynamic = c.cell_metadata.is_some();
    st.shared_copy_refused = false;
    let formula = c.formula.as_ref().and_then(|f| read_craft_formula(cx, st, f, pos, dynamic));
    let py = match (&formula, &c.formula) {
        (Some(fm), Some(f)) => crate::python_part::cell_code(&mut cx.python, fm, &f.text),
        _ => crate::python_part::PyFormula::Not,
    };
    let (formula, py) = match py {
        crate::python_part::PyFormula::Not => (formula, None),
        crate::python_part::PyFormula::Code(py) => (None, Some(Arc::new(py))),
        crate::python_part::PyFormula::Missing => {
            forget_dynamic(st, pos);
            (None, None)
        }
    };
    if py.is_some() {
        record_python_spill(st, sheet, pos);
    }
    if formula.as_ref().is_some_and(|f| !f.dynamic && f.array.is_none() && f.expr_arc().is_some()) {
        cx.legacy_formulas.push(pos);
    }
    let spilled_from = if formula.is_none() && py.is_none() && !st.shared_copy_refused { dynamic_range(st, pos) } else { None };
    let python_anchor = spilled_from.is_some_and(|r| sheet.cells.get(r.start).is_some_and(|a| a.py.is_some()));
    let value = match spilled_from {
        Some(r) if python_anchor && r.count() <= gridcraft_model::python::MAX_RESULT_CELLS as u64 => {
            if !value.is_empty() {
                sheet.spill.insert(pos, value);
            }
            sheet.spill_ranges.insert(r.start, r);
            Value::Empty
        }
        Some(_) if python_anchor => value,
        Some(_) => Value::Empty,
        None => value,
    };
    if dynamic
        && formula.as_ref().is_some_and(|f| f.array.is_none())
        && let Some(r) = st.dynamic.last().filter(|r| r.start == pos)
    {
        sheet.spill_ranges.insert(pos, *r);
    }
    sheet.cells.set(pos, Cell { value, formula: formula.map(Arc::new), style, py });
}

fn read_craft_formula(cx: &mut Ctx<'_>, st: &mut RowState, f: &CraftFormula, pos: CellRef, dynamic: bool) -> Option<Formula> {
    let mut unwrapped = false;
    let fm = read_craft_formula_text(cx, st, f, pos, dynamic, &mut unwrapped)?;
    let dynamic = dynamic || unwrapped;
    Some(Formula { dynamic, ..fm })
}

fn read_craft_formula_text(
    cx: &mut Ctx<'_>,
    st: &mut RowState,
    f: &CraftFormula,
    pos: CellRef,
    dynamic: bool,
    unwrapped: &mut bool,
) -> Option<Formula> {
    let mut model = from_file(f.text.trim());
    if let Some(known) = cx.known_function
        && let Some(u) = unwrap_google_sheets(&model, known)
    {
        model = u;
        *unwrapped = true;
    }
    let text = model.as_str();
    match f.kind {
        CraftFormulaKind::Shared => {
            let si = f.shared_index?;
            if !text.is_empty() {
                let expr = gridcraft_formula::parse(text).ok();
                let cost = shared_cost(text, expr.as_ref());
                st.shared.insert(si, Shared { anchor: pos, expr, text: text.to_string(), unwrapped: *unwrapped, cost });
                Some(Formula::new(text))
            } else {
                let Some(sh) = st.shared.get(&si) else {
                    cx.warn("a shared formula refers to a missing master cell; kept the cached value");
                    return None;
                };
                if sh.expr.is_none()
                    && sh.text.len() > gridcraft_formula::MAX_FORMULA_LEN
                    && sh.text.chars().count() > gridcraft_formula::MAX_FORMULA_LEN
                {
                    cx.warn(format!(
                        "a shared formula is longer than {} characters; kept the cached values of the cells that share it",
                        gridcraft_formula::MAX_FORMULA_LEN
                    ));
                    st.shared_copy_refused = true;
                    return None;
                }
                if !charge(&mut cx.shared_formula_bytes, extra_shared_cost(sh.cost), MAX_SHARED_FORMULA_BYTES) {
                    cx.warn("shared formulas expand to too much data; kept the cached values of the remaining cells that share them");
                    st.shared_copy_refused = true;
                    return None;
                }
                *unwrapped |= sh.unwrapped;
                Some(match &sh.expr {
                    Some(e) => {
                        let dr = pos.row as i64 - sh.anchor.row as i64;
                        let dc = pos.col as i64 - sh.anchor.col as i64;
                        Formula::from_expr(gridcraft_formula::adjust::shift_relative(e.clone(), dr, dc))
                    }
                    None => Formula::new(&sh.text),
                })
            }
        }
        CraftFormulaKind::Array => {
            if text.is_empty() {
                return None;
            }
            let mut fm = Formula::new(text);
            let range = f.range.map(to_range_ref).unwrap_or(RangeRef::cell(pos));
            if !range.is_single() {
                push_dynamic(st, range);
            }
            if !dynamic {
                fm.array = Some(range);
            }
            Some(fm)
        }
        CraftFormulaKind::DataTable => {
            cx.warn("what-if data tables are not supported; kept their values");
            None
        }
        CraftFormulaKind::Normal => {
            if text.is_empty() {
                None
            } else {
                Some(Formula::new(text))
            }
        }
    }
}

/// `2024-03-15`, `2024-03-15T10:30:00Z`, `10:30:00` → serial.
pub fn parse_iso_datetime(s: &str, sys: DateSystem) -> Option<f64> {
    let s = s.trim_end_matches('Z');
    let (date, time) = match s.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None if s.contains(':') => ("", Some(s)),
        None => (s, None),
    };
    let mut serial = 0.0;
    if !date.is_empty() {
        let mut it = date.split('-');
        let y: i64 = it.next()?.parse().ok()?;
        let m: i64 = it.next()?.parse().ok()?;
        let d: i64 = it.next()?.parse().ok()?;
        if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
            return None;
        }
        serial = serial_from_ymd(sys, y, m, d)?;
    }
    if let Some(t) = time {
        // Drop a time-zone offset.
        let t = t.split(['+']).next().unwrap_or(t);
        let mut it = t.split(':');
        let h: f64 = it.next()?.parse().ok()?;
        let mi: f64 = it.next().unwrap_or("0").parse().ok()?;
        let se: f64 = it.next().unwrap_or("0").parse().ok()?;
        if !(0.0..24.0).contains(&h) || !(0.0..60.0).contains(&mi) || !(0.0..61.0).contains(&se) {
            return None;
        }
        serial += time_fraction(h, mi, se);
    }
    Some(serial)
}

/// What one dependent of a shared formula costs: its printed text plus its expression tree.
fn shared_cost(text: &str, expr: Option<&Expr>) -> u64 {
    let mut nodes = 0u64;
    if let Some(e) = expr {
        e.walk(&mut |_| nodes = nodes.saturating_add(1));
    }
    let node = std::mem::size_of::<Expr>() as u64;
    (text.len() as u64).saturating_mul(2).saturating_add(nodes.saturating_mul(node)).saturating_add(64)
}

/// What one dependent of a shared formula costing `cost` is charged: its cost beyond an ordinary
/// formula cell's.
fn extra_shared_cost(cost: u64) -> u64 {
    cost.saturating_sub(SHARED_FORMULA_FREE_COST)
}

/// Adds `cost` to `used` when that stays within `cap`; `false` (and `used` unchanged) otherwise.
fn charge(used: &mut u64, cost: u64, cap: u64) -> bool {
    match used.checked_add(cost) {
        Some(total) if total <= cap => {
            *used = total;
            true
        }
        _ => false,
    }
}

/// The first `max` characters of `s`.
fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((at, _)) => s.get(..at).unwrap_or(s).to_string(),
        None => s.to_string(),
    }
}

pub fn parse_sqref(s: Option<&str>) -> Vec<RangeRef> {
    s.unwrap_or("").split_whitespace().filter_map(RangeRef::parse).collect()
}

/// Worksheet extensions: x14 data validations and sparklines.
fn read_ext(cx: &mut Ctx<'_>, e: &El, sheet: &mut Sheet, cf_kept_cells: &mut u64) {
    for ext in e.kids("ext") {
        for c in &ext.children {
            match c.name.as_str() {
                "dataValidations" => {
                    for v in c.kids("dataValidation") {
                        if let Some(v) = read_validation(v) {
                            sheet.validations.push(v);
                        }
                    }
                }
                "sparklineGroups" => {
                    for g in c.kids("sparklineGroup") {
                        let kind = match g.attr("type") {
                            Some("column") => SparklineKind::Column,
                            Some("stacked") => SparklineKind::WinLoss,
                            _ => SparklineKind::Line,
                        };
                        let color = g.child("colorSeries").map(|c| read_color(c, &cx.palette)).unwrap_or_default();
                        let markers = g.flag("markers", false);
                        for s in g.child("sparklines").map(|s| s.kids("sparkline").collect::<Vec<_>>()).unwrap_or_default() {
                            let source = s.child("f").map(|f| f.text.trim().to_string()).unwrap_or_default();
                            if let Some(cell) = s.child("sqref").and_then(|q| CellRef::parse(q.text.trim())) {
                                sheet.sparklines.push(Sparkline { cell, source, kind, color, markers });
                            }
                        }
                    }
                }
                "conditionalFormattings" => read_x14_cf(cx, c, sheet, cf_kept_cells),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_formula_budget_stops_expansion() {
        let expr = gridcraft_formula::parse("SUM(1,2,3)").ok();
        let cost = shared_cost("SUM(1,2,3)", expr.as_ref());
        assert!(cost > 64 + 4 * std::mem::size_of::<Expr>() as u64);
        let cap = cost * 3;
        let mut used = 0;
        assert!((0..3).all(|_| charge(&mut used, cost, cap)));
        assert!(!charge(&mut used, cost, cap));
        assert_eq!(used, cap);
        assert!(!charge(&mut used, u64::MAX, u64::MAX));
    }

    #[test]
    fn ordinary_shared_formulas_are_free_and_long_ones_pay_their_excess() {
        let text = r#"IF(AND(A1>0,B1<>""),VLOOKUP(C1,Sheet2!$A$1:$D$500,4,FALSE)*SUM(D1:D10)/COUNT(D1:D10),IFERROR(E1/F1,0))"#;
        let expr = gridcraft_formula::parse(text).ok();
        assert!(expr.is_some());
        assert_eq!(extra_shared_cost(shared_cost(text, expr.as_ref())), 0, "an ordinary formula costs nothing extra");
        let long = vec![format!("SUM({}1)", "1,".repeat(199)); 10].join("+");
        let expr = gridcraft_formula::parse(&long).ok();
        assert!(expr.is_some());
        let cost = shared_cost(&long, expr.as_ref());
        assert_eq!(extra_shared_cost(cost), cost - SHARED_FORMULA_FREE_COST);
        // A few thousand copies of a long master no longer fit the budget.
        assert!(extra_shared_cost(cost).saturating_mul(10_000) > MAX_SHARED_FORMULA_BYTES);
    }

    #[test]
    fn truncate_keeps_whole_characters() {
        assert_eq!(truncate_chars("ab\u{e9}d", 3), "ab\u{e9}");
        assert_eq!(truncate_chars("ab", 3), "ab");
    }

    #[test]
    fn iso_dates() {
        assert_eq!(parse_iso_datetime("1900-01-01", DateSystem::D1900), Some(1.0));
        assert_eq!(parse_iso_datetime("2024-01-01T12:00:00Z", DateSystem::D1900), Some(45292.5));
        assert_eq!(parse_iso_datetime("1904-01-02", DateSystem::D1904), Some(1.0));
        assert_eq!(parse_iso_datetime("06:00:00", DateSystem::D1900), Some(0.25));
        assert_eq!(parse_iso_datetime("garbage", DateSystem::D1900), None);
        assert_eq!(parse_iso_datetime("2024-13-01", DateSystem::D1900), None);
    }

    fn ctx(pkg: crate::package::Package<'_>) -> Ctx<'_> {
        Ctx {
            pkg,
            report: Default::default(),
            sst: vec![],
            phonetics: vec![],
            xf_map: vec![],
            dxfs: vec![],
            palette: vec![],
            date_system: DateSystem::D1900,
            styles: Default::default(),
            persons: Default::default(),
            next_id: 1,
            num_fmts: Default::default(),
            cell_pictures: vec![],
            cf_ext_ids: vec![],
            known_function: None,
            python: Default::default(),
            cells: 0,
            shared_formula_bytes: 0,
            legacy_formulas: vec![],
        }
    }

    /// #2159: the workbook-wide cell cap refuses the open once passed, across sheets.
    #[test]
    fn cells_past_the_workbook_cap_are_refused() {
        let bytes = crate::tests::minimal(r#"<sheetData><row r="1"><c r="A1"><v>1</v></c><c r="B1"><v>2</v></c></row></sheetData>"#, &[], "", "");
        let pkg = crate::package::Package::open(&bytes).unwrap();
        let mut cx = ctx(pkg);
        cx.cells = MAX_CELLS - 2;
        let (sheet, _) = read_sheet(&mut cx, "xl/worksheets/sheet1.xml", "Data").unwrap();
        assert_eq!(sheet.cells.len(), 2);
        assert!(matches!(read_sheet(&mut cx, "xl/worksheets/sheet1.xml", "Data"), Err(IoError::TooLarge(_))));
    }

    /// #2159: the cells of a row dropped for its row number still count against the cap, so a
    /// part read for many sheets can't parse cells uncounted.
    #[test]
    fn cells_of_dropped_rows_count_against_the_cap() {
        let rows = r#"<sheetData><row r="0"><c><v>1</v></c><c><v>2</v></c></row><row r="2000000"><c><v>3</v></c></row></sheetData>"#;
        let bytes = crate::tests::minimal(rows, &[], "", "");
        let pkg = crate::package::Package::open(&bytes).unwrap();
        let mut cx = ctx(pkg);
        let (sheet, _) = read_sheet(&mut cx, "xl/worksheets/sheet1.xml", "Data").unwrap();
        assert!(sheet.cells.is_empty(), "both rows are dropped");
        assert_eq!(cx.cells, 3);
        cx.cells = MAX_CELLS - 2;
        assert!(matches!(read_sheet(&mut cx, "xl/worksheets/sheet1.xml", "Data"), Err(IoError::TooLarge(_))));
    }

    #[test]
    fn sqref_keeps_more_than_ten_thousand_ranges() {
        let sq = (1..=12_000).map(|r| format!("A{r}")).collect::<Vec<_>>().join(" ");
        let ranges = parse_sqref(Some(&sq));
        assert_eq!(ranges.len(), 12_000);
        assert_eq!(ranges.last().copied(), RangeRef::parse("A12000"));
    }
}
