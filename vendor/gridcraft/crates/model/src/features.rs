//! Sheet features beyond cells: tables, conditional formatting, data validation, comments,
//! hyperlinks, charts, images, autofilter, print settings.

use gridcraft_core::{CellRef, RangeRef};
use serde::{Deserialize, Serialize};

use crate::style::{Color, Style};

mod pivot;
mod print;

pub use pivot::{
    PIVOT_INDEXER, PivotCalcField, PivotDataCell, PivotDateGroup, PivotField, PivotFilter, PivotFunc, PivotIndexer, PivotItemRef, PivotLayout,
    PivotLookup, PivotShowAs, PivotSort, PivotTable, PivotValue, pivot_key,
};
pub use print::{EditRange, HfPage, LineInfo, Orientation, PageOrder, PrintSettings, SheetProtection, password_hash};

// ---------------------------------------------------------------- tables

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TotalsFn {
    #[default]
    None,
    Average,
    Count,
    CountNums,
    Max,
    Min,
    Sum,
    StdDev,
    Var,
    Custom,
}

impl TotalsFn {
    /// SUBTOTAL function number.
    pub fn subtotal_code(&self) -> Option<u32> {
        Some(match self {
            TotalsFn::Average => 101,
            TotalsFn::Count => 103,
            TotalsFn::CountNums => 102,
            TotalsFn::Max => 104,
            TotalsFn::Min => 105,
            TotalsFn::Sum => 109,
            TotalsFn::StdDev => 107,
            TotalsFn::Var => 110,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TableColumn {
    pub name: String,
    pub totals: TotalsFn,
    /// Totals row label (first column) or custom formula.
    pub totals_label: Option<String>,
    /// Calculated column formula.
    pub formula: Option<String>,
    /// `uniqueName`: the name XML maps know the column by.
    #[serde(default)]
    pub unique_name: Option<String>,
    /// Binding to an element of an XML map. It moves with the column and goes when the column is
    /// deleted; inserted columns are unmapped.
    #[serde(default)]
    pub xml: Option<XmlColumnPr>,
}

/// A table column's binding to an XML map (`<xmlColumnPr>`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct XmlColumnPr {
    pub map_id: u32,
    pub xpath: String,
    /// `xmlDataType` as written in the file (`string`, `boolean`, `integer`…).
    pub data_type: String,
    pub denormalized: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub id: u32,
    pub name: String,
    /// Whole table including header and totals rows.
    pub range: RangeRef,
    pub header_row: bool,
    pub totals_row: bool,
    pub columns: Vec<TableColumn>,
    pub style: String,
    pub banded_rows: bool,
    pub banded_cols: bool,
    pub first_col: bool,
    pub last_col: bool,
    pub filter_button: bool,
    /// Criteria of the table's AutoFilter (column offset within the table → criterion). With
    /// criteria present, hidden data rows are filtered out rather than hidden by hand, which
    /// SUBTOTAL distinguishes.
    #[serde(default)]
    pub filter: Vec<(u32, FilterCriterion)>,
    /// `tableType` when it isn't a plain worksheet table: `xml` for a table bound to an XML map.
    #[serde(default)]
    pub table_type: Option<String>,
    /// The connection (`xl/connections.xml`) an XML table is bound through.
    #[serde(default)]
    pub connection_id: Option<u32>,
}

impl Table {
    pub fn data_range(&self) -> Option<RangeRef> {
        let r0 = self.range.start.row + self.header_row as u32;
        let r1 = self.range.end.row.checked_sub(self.totals_row as u32)?;
        if r1 < r0 {
            return None;
        }
        Some(RangeRef::new(CellRef::new(r0, self.range.start.col), CellRef::new(r1, self.range.end.col)))
    }
    pub fn header_range(&self) -> Option<RangeRef> {
        self.header_row.then(|| RangeRef::new(self.range.start, CellRef::new(self.range.start.row, self.range.end.col)))
    }
    pub fn totals_range(&self) -> Option<RangeRef> {
        self.totals_row.then(|| RangeRef::new(CellRef::new(self.range.end.row, self.range.start.col), self.range.end))
    }
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name.eq_ignore_ascii_case(name))
    }
}

// ---------------------------------------------------------------- conditional formatting

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CfOperator {
    Between,
    NotBetween,
    Equal,
    NotEqual,
    Greater,
    Less,
    GreaterOrEqual,
    LessOrEqual,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CfValueKind {
    Min,
    Max,
    Number(f64),
    Percent(f64),
    Percentile(f64),
    Formula(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CfRule {
    /// Cell value compared with one or two formulas.
    CellIs {
        op: CfOperator,
        a: String,
        b: Option<String>,
        style: Box<Style>,
    },
    /// Custom formula (relative to the top-left cell of the range).
    Expression {
        formula: String,
        style: Box<Style>,
    },
    ContainsText {
        text: String,
        style: Box<Style>,
    },
    NotContainsText {
        text: String,
        style: Box<Style>,
    },
    BeginsWith {
        text: String,
        style: Box<Style>,
    },
    EndsWith {
        text: String,
        style: Box<Style>,
    },
    Blanks {
        style: Box<Style>,
    },
    NoBlanks {
        style: Box<Style>,
    },
    Errors {
        style: Box<Style>,
    },
    NoErrors {
        style: Box<Style>,
    },
    Duplicate {
        style: Box<Style>,
    },
    Unique {
        style: Box<Style>,
    },
    /// Top/bottom N or N percent.
    Top10 {
        bottom: bool,
        percent: bool,
        rank: u32,
        style: Box<Style>,
    },
    AboveAverage {
        below: bool,
        equal: bool,
        std_dev: u8,
        style: Box<Style>,
    },
    /// `yesterday`, `today`, `tomorrow`, `last7Days`, `lastWeek`, `thisWeek`, `nextWeek`, `lastMonth`, `thisMonth`, `nextMonth`.
    TimePeriod {
        period: String,
        style: Box<Style>,
    },
    ColorScale {
        stops: Vec<(CfValueKind, Color)>,
    },
    DataBar {
        min: CfValueKind,
        max: CfValueKind,
        color: Color,
        gradient: bool,
        show_value: bool,
    },
    /// Icon set name (`3Arrows`, `3TrafficLights1`, `4Rating`, `5Quarters`…).
    IconSet {
        set: String,
        thresholds: Vec<CfValueKind>,
        reverse: bool,
        show_value: bool,
    },
}

impl CfRule {
    /// The formulas written for the rule's range's top-left cell (and read at each cell
    /// relative to it): a cell-value rule's values and an expression. What moving the rule
    /// re-hosts. Thresholds (colour scale, data bar, icon set) are read as written.
    pub fn relative_formulas_mut(&mut self) -> Vec<&mut String> {
        match self {
            CfRule::CellIs { a, b, .. } => std::iter::once(a).chain(b.as_mut()).collect(),
            CfRule::Expression { formula, .. } => vec![formula],
            _ => Vec::new(),
        }
    }

    /// [`CfRule::formulas_mut`], read only.
    pub fn formulas(&self) -> Vec<&String> {
        let kinds: Vec<&CfValueKind> = match self {
            CfRule::CellIs { a, b, .. } => return std::iter::once(a).chain(b.as_ref()).collect(),
            CfRule::Expression { formula, .. } => return vec![formula],
            CfRule::ColorScale { stops } => stops.iter().map(|(k, _)| k).collect(),
            CfRule::DataBar { min, max, .. } => vec![min, max],
            CfRule::IconSet { thresholds, .. } => thresholds.iter().collect(),
            _ => Vec::new(),
        };
        kinds
            .into_iter()
            .filter_map(|k| match k {
                CfValueKind::Formula(t) => Some(t),
                _ => None,
            })
            .collect()
    }

    /// All the rule's formulas, thresholds included: what an edit that moves references
    /// rewrites.
    pub fn formulas_mut(&mut self) -> Vec<&mut String> {
        let kinds: Vec<&mut CfValueKind> = match self {
            CfRule::CellIs { a, b, .. } => return std::iter::once(a).chain(b.as_mut()).collect(),
            CfRule::Expression { formula, .. } => return vec![formula],
            CfRule::ColorScale { stops } => stops.iter_mut().map(|(k, _)| k).collect(),
            CfRule::DataBar { min, max, .. } => vec![min, max],
            CfRule::IconSet { thresholds, .. } => thresholds.iter_mut().collect(),
            _ => Vec::new(),
        };
        kinds
            .into_iter()
            .filter_map(|k| match k {
                CfValueKind::Formula(t) => Some(t),
                _ => None,
            })
            .collect()
    }

    /// The differential style a matching rule applies; colour scales, data bars and icon sets have none.
    pub fn style(&self) -> Option<&Style> {
        match self {
            CfRule::CellIs { style, .. }
            | CfRule::Expression { style, .. }
            | CfRule::ContainsText { style, .. }
            | CfRule::NotContainsText { style, .. }
            | CfRule::BeginsWith { style, .. }
            | CfRule::EndsWith { style, .. }
            | CfRule::Blanks { style }
            | CfRule::NoBlanks { style }
            | CfRule::Errors { style }
            | CfRule::NoErrors { style }
            | CfRule::Duplicate { style }
            | CfRule::Unique { style }
            | CfRule::Top10 { style, .. }
            | CfRule::AboveAverage { style, .. }
            | CfRule::TimePeriod { style, .. } => Some(style),
            CfRule::ColorScale { .. } | CfRule::DataBar { .. } | CfRule::IconSet { .. } => None,
        }
    }
}

impl Validation {
    /// The validation's formulas (for its range's top-left cell).
    pub fn formulas_mut(&mut self) -> Vec<&mut String> {
        std::iter::once(&mut self.f1).chain(self.f2.as_mut()).collect()
    }

    /// [`Validation::formulas_mut`], read only.
    pub fn formulas(&self) -> Vec<&String> {
        std::iter::once(&self.f1).chain(self.f2.as_ref()).collect()
    }
}

/// Number of icons in an icon set (`3Arrows` → 3, `5Quarters` → 5), always 3 to 5.
pub fn icon_count(set: &str) -> usize {
    set.chars().next().and_then(|c| c.to_digit(10)).unwrap_or(3).clamp(3, 5) as usize
}

/// Excel's default icon-set thresholds: one per icon, the first being the lower bound,
/// then even percent steps (`0, 33, 67` for three icons).
pub fn default_icon_thresholds(set: &str) -> Vec<CfValueKind> {
    let k = icon_count(set);
    (0..k).map(|i| CfValueKind::Percent((i as f64 * 100.0 / k as f64).round())).collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CondFormat {
    pub ranges: Vec<RangeRef>,
    pub rule: CfRule,
    pub priority: u32,
    pub stop_if_true: bool,
}

// ---------------------------------------------------------------- data validation

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValidationKind {
    #[default]
    Any,
    Whole,
    Decimal,
    List,
    Date,
    Time,
    TextLength,
    Custom,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorStyle {
    #[default]
    Stop,
    Warning,
    Information,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Validation {
    pub ranges: Vec<RangeRef>,
    pub kind: ValidationKind,
    pub op: CfOperator,
    /// Formula 1 (list source: `"a,b,c"` literal list or `=$A$1:$A$5`).
    pub f1: String,
    pub f2: Option<String>,
    pub allow_blank: bool,
    pub in_cell_dropdown: bool,
    pub input_title: String,
    pub input_message: String,
    pub show_input: bool,
    pub error_title: String,
    pub error_message: String,
    pub error_style: ErrorStyle,
    pub show_error: bool,
}

impl Default for Validation {
    fn default() -> Self {
        Validation {
            ranges: vec![],
            kind: ValidationKind::Any,
            op: CfOperator::Between,
            f1: String::new(),
            f2: None,
            allow_blank: true,
            in_cell_dropdown: true,
            input_title: String::new(),
            input_message: String::new(),
            show_input: true,
            error_title: String::new(),
            error_message: String::new(),
            error_style: ErrorStyle::Stop,
            show_error: true,
        }
    }
}

// ---------------------------------------------------------------- comments, links

/// A note (classic comment) or a threaded comment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub author: String,
    pub text: String,
    /// Replies of a threaded comment (author, text).
    pub replies: Vec<(String, String)>,
    pub threaded: bool,
    pub resolved: bool,
    pub visible: bool,
    /// Threaded comments: when the thread (first entry) and each reply were written, ISO 8601 as
    /// read from the file (`dT`); empty when unknown, e.g. for comments made here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dates: Vec<String>,
    /// Where the note's box sits and how big it is, as read from the file; `None` (a note made
    /// here, or a file without the shape) writes the default box next to the cell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_box: Option<NoteBox>,
}

/// A note box's place and size, from the note's VML shape: Excel's `<x:Anchor>` with its corners
/// stored relative to the note's cell, so the box moves with the note when rows or columns are
/// inserted or deleted, and the shape's width and height.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoteBox {
    /// Top-left corner: columns and rows from the note's cell (negative = left of or above it),
    /// then the pixel offset into that column and row.
    pub from: NoteCorner,
    /// Bottom-right corner, the same way.
    pub to: NoteCorner,
    /// The shape's width and height in points (the VML style's `width`/`height`), when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_pt: Option<(f64, f64)>,
}

/// One corner of a [`NoteBox`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteCorner {
    pub col: i64,
    pub col_px: u32,
    pub row: i64,
    pub row_px: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hyperlink {
    /// URL, `mailto:`, a file path (`Book.xlsx`, `Book.xlsx#Sheet1!A2`), or an in-workbook location
    /// like `Sheet2!A1`.
    pub target: String,
    pub tooltip: Option<String>,
    /// The target is outside this workbook (Excel's "Existing File or Web Page"): a relationship in
    /// the file, not a `location`. A bare file name can't be told from a defined name by its text.
    #[serde(default)]
    pub external: bool,
}

// ---------------------------------------------------------------- drawing objects

/// How a floating object is pinned to the grid when its cells move or resize. Mirrors Excel's
/// Format ▸ Size and Properties ▸ Properties, stored in the file as the anchor's `editAs`:
/// `twoCell` (absent `editAs`), `oneCell` and `absolute`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AnchorMode {
    /// Move and size with cells (Excel's default; `<xdr:twoCellAnchor>`).
    #[default]
    MoveAndSize,
    /// Move but don't size with cells (`<xdr:twoCellAnchor editAs="oneCell">` / `<xdr:oneCellAnchor>`).
    MoveOnly,
    /// Don't move or size with cells (`editAs="absolute"` / `<xdr:absoluteAnchor>`).
    Absolute,
}

/// Position of a floating object: top-left cell plus offset, size in points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub cell: CellRef,
    pub dx: f32,
    pub dy: f32,
    pub width: f32,
    pub height: f32,
    /// How the object follows its anchor cell when rows/columns move (sort, insert, delete).
    #[serde(default)]
    pub mode: AnchorMode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChartKind {
    #[default]
    ColumnClustered,
    ColumnStacked,
    ColumnStacked100,
    BarClustered,
    BarStacked,
    BarStacked100,
    Line,
    LineMarkers,
    LineStacked,
    Pie,
    Doughnut,
    Area,
    AreaStacked,
    AreaStacked100,
    Scatter,
    ScatterLines,
    Bubble,
    Radar,
    Histogram,
    Waterfall,
    Funnel,
    Treemap,
    Sunburst,
    BoxWhisker,
    Stock,
    Combo,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LegendPos {
    None,
    #[default]
    Bottom,
    Top,
    Left,
    Right,
}

/// Quartile calculation for a box & whisker series.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuartileMethod {
    #[default]
    Exclusive,
    Inclusive,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Series {
    /// Formula text for the series name (`Sheet1!$B$1`) or a literal.
    pub name: Option<String>,
    pub categories: Option<String>,
    pub values: String,
    /// X values for scatter / bubble sizes.
    pub bubble_sizes: Option<String>,
    pub color: Option<Color>,
    /// Combo charts: this series drawn as a line on the secondary axis.
    pub secondary: bool,
    pub kind: Option<ChartKind>,
    /// Draw line segments as a smoothed (spline) curve.
    #[serde(default)]
    pub smooth: bool,
    /// Box & whisker quartiles; Excel defaults to exclusive.
    #[serde(default)]
    pub quartiles: QuartileMethod,
    /// Trendlines fitted to this series (Excel allows several per series).
    #[serde(default)]
    pub trendlines: Vec<Trendline>,
}

/// Regression a trendline fits (`c:trendlineType`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrendlineKind {
    #[default]
    Linear,
    /// `y = c·e^(b·x)`.
    Exponential,
    /// `y = c·ln(x) + b`.
    Logarithmic,
    /// Polynomial of this order (2..=6).
    Polynomial(u8),
    /// `y = c·x^b`.
    Power,
    /// Average of this many consecutive points (2..=255).
    MovingAverage(u32),
}

/// DrawingML preset line dash (`a:prstDash`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineDash {
    #[default]
    Solid,
    Dot,
    Dash,
    LgDash,
    DashDot,
    LgDashDot,
    LgDashDotDot,
    SysDash,
    SysDot,
    SysDashDot,
    SysDashDotDot,
}

/// A chart trendline (Add Chart Element ▸ Trendline, Format Trendline pane).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Trendline {
    pub kind: TrendlineKind,
    /// Custom name shown in the legend; `None` = automatic ("Linear (Sales)").
    pub name: Option<String>,
    /// Forecast past the last point, in x units (categories on a category axis).
    pub forward: f64,
    /// Forecast before the first point, in x units.
    pub backward: f64,
    /// Set Intercept: fixed y at x = 0 (linear, exponential, polynomial).
    pub intercept: Option<f64>,
    pub display_equation: bool,
    pub display_r_squared: bool,
    /// Line colour; `None` = the series colour.
    pub color: Option<Color>,
    /// Line width in points; `None` = default.
    pub width: Option<f32>,
    /// Line dash; `None` = default.
    pub dash: Option<LineDash>,
}

impl Trendline {
    /// Longest forward / backward forecast kept from a file or a command, in x units. Excel's own
    /// limit is unconfirmed; the renderer additionally bounds the forecast by the plot's slot budget.
    pub const MAX_FORECAST: f64 = 100_000.0;

    /// A forecast distance as stored: finite, not negative (NaN and infinity become 0) and at most
    /// [`Self::MAX_FORECAST`].
    pub fn clamp_forecast(v: f64) -> f64 {
        if v.is_finite() && v > 0.0 { v.min(Self::MAX_FORECAST) } else { 0.0 }
    }
}

/// Charts saved before `hole_size` existed were drawn with a 50% hole. New charts and charts read
/// from XLSX set 75 explicitly (Excel's default), so only a missing JSON field keeps the old look.
fn default_hole_size() -> u8 {
    50
}

/// A chart's data table (`c:dTable`), independent of the chart legend.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChartDataTable {
    pub show_keys: bool,
    pub horizontal_border: bool,
    pub vertical_border: bool,
    pub outline: bool,
}

impl Default for ChartDataTable {
    fn default() -> Self {
        Self { show_keys: true, horizontal_border: true, vertical_border: true, outline: true }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chart {
    pub id: u32,
    pub kind: ChartKind,
    pub anchor: Anchor,
    pub title: Option<String>,
    pub series: Vec<Series>,
    pub legend: LegendPos,
    pub data_labels: bool,
    #[serde(default)]
    pub data_table: Option<ChartDataTable>,
    pub gridlines: bool,
    pub style: u32,
    /// Doughnut hole size as a percentage of the plot radius (Excel's 10–90 range). New charts and
    /// XLSX imports use Excel's 75; a document saved before this field existed reads as 50.
    #[serde(default = "default_hole_size")]
    pub hole_size: u8,
    /// Pie/doughnut first slice angle in degrees clockwise from 12 o'clock (0–359).
    #[serde(default)]
    pub first_slice_angle: u16,
    pub x_title: Option<String>,
    pub y_title: Option<String>,
    /// Source range when created from a selection (for Switch Row/Column).
    pub source: Option<String>,
    pub by_rows: bool,
    /// A PivotChart's PivotTable (`Sheet2!PivotTable1`): its series are rebuilt from the report
    /// whenever the PivotTable is refreshed or its layout changes.
    #[serde(default)]
    pub pivot: Option<String>,
}

/// Whether a series of kind `b` can be drawn in a chart of kind `a` without making it a combo:
/// the same kind, or a line with or without markers, or a scatter with or without lines.
pub fn same_family(a: ChartKind, b: ChartKind) -> bool {
    use ChartKind::*;
    a == b || matches!((a, b), (Line | LineMarkers, Line | LineMarkers) | (Scatter | ScatterLines, Scatter | ScatterLines))
}

/// The kind a combo chart's series `i` with its own kind `own` is drawn as.
fn combo_series_kind(own: Option<ChartKind>, i: usize) -> ChartKind {
    match own {
        // A combination chart's bars stand upright: its categories run across.
        Some(ChartKind::BarClustered) => ChartKind::ColumnClustered,
        Some(ChartKind::BarStacked) => ChartKind::ColumnStacked,
        Some(ChartKind::BarStacked100) => ChartKind::ColumnStacked100,
        // A combination chart is drawn on category axes: its pies, doughnuts, bubbles and chartex
        // kinds (histogram, waterfall, funnel, treemap, sunburst, box & whisker) as clustered
        // columns, its scatter and radar series as lines (a markers-only scatter with markers).
        Some(
            ChartKind::Pie
            | ChartKind::Doughnut
            | ChartKind::Bubble
            | ChartKind::Histogram
            | ChartKind::Waterfall
            | ChartKind::Funnel
            | ChartKind::Treemap
            | ChartKind::Sunburst
            | ChartKind::BoxWhisker,
        ) => ChartKind::ColumnClustered,
        Some(ChartKind::Scatter) => ChartKind::LineMarkers,
        Some(ChartKind::ScatterLines | ChartKind::Radar) => ChartKind::Line,
        Some(k) if k != ChartKind::Combo => k,
        _ if i == 0 => ChartKind::ColumnClustered,
        _ => ChartKind::Line,
    }
}

/// The types whose series on one pair of axes share one grouping (clustered or stacked).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Grouped {
    Column,
    Line,
    Area,
}

fn grouped(k: ChartKind) -> Option<Grouped> {
    use ChartKind::*;
    match k {
        ColumnClustered | ColumnStacked | ColumnStacked100 => Some(Grouped::Column),
        Line | LineMarkers | LineStacked => Some(Grouped::Line),
        Area | AreaStacked | AreaStacked100 => Some(Grouped::Area),
        _ => None,
    }
}

/// Kind `k` drawn in the grouping of `first`, the first series of its type on its axes: columns
/// and areas take its kind; a line is stacked when it is, and unstacked (keeping its own
/// markers) when it isn't.
fn regroup(k: ChartKind, first: ChartKind) -> ChartKind {
    match (grouped(k), first) {
        (Some(Grouped::Line), ChartKind::LineStacked) => ChartKind::LineStacked,
        (Some(Grouped::Line), _) if k == ChartKind::LineStacked => ChartKind::Line,
        (Some(Grouped::Line), _) => k,
        _ => first,
    }
}

impl Chart {
    /// The kind series `i` is drawn as, and saved as (the one rule the renderer and every file
    /// format share).
    ///
    /// In a combo chart: the series' own kind, else a column for the first series and a line
    /// for the rest. A combo is drawn on category axes, so bars stand upright as columns; pies,
    /// doughnuts, bubbles and chartex kinds are clustered columns; scatter series are lines with
    /// markers, and scatter-with-lines and radar series plain lines. The series of one type on
    /// one pair of axes share the first one's grouping (as Excel keeps them): a stacked column
    /// after a clustered one is clustered.
    ///
    /// In any other chart: the chart's kind, or the series' own kind where it is of the same
    /// family ([`same_family`]: a line's own markers, a scatter's own lines).
    pub fn series_kind(&self, i: usize) -> ChartKind {
        let own = self.series.get(i).and_then(|s| s.kind).filter(|k| *k != ChartKind::Combo);
        match (self.kind, own) {
            (ChartKind::Combo, own) => {
                let k = combo_series_kind(own, i);
                let Some(t) = grouped(k) else { return k };
                let secondary = self.series.get(i).is_some_and(|s| s.secondary);
                let first = self.series.iter().enumerate().take(i).find_map(|(j, s)| {
                    #[cfg(any(test, feature = "test-support"))]
                    work::visit();
                    let kj = combo_series_kind(s.kind, j);
                    (s.secondary == secondary && grouped(kj) == Some(t)).then_some(kj)
                });
                first.map_or(k, |f| regroup(k, f))
            }
            (k, Some(own)) if same_family(k, own) => own,
            (k, _) => k,
        }
    }

    /// The kind every series is drawn as ([`Chart::series_kind`]), in one pass.
    pub fn series_kinds(&self) -> Vec<ChartKind> {
        if self.kind != ChartKind::Combo {
            return (0..self.series.len()).map(|i| self.series_kind(i)).collect();
        }
        let mut firsts: Vec<(Grouped, bool, ChartKind)> = Vec::new();
        self.series
            .iter()
            .enumerate()
            .map(|(i, s)| {
                #[cfg(any(test, feature = "test-support"))]
                work::visit();
                let k = combo_series_kind(s.kind, i);
                let Some(t) = grouped(k) else { return k };
                match firsts.iter().find(|(ft, sec, _)| *ft == t && *sec == s.secondary) {
                    Some((_, _, f)) => regroup(k, *f),
                    None => {
                        firsts.push((t, s.secondary, k));
                        k
                    }
                }
            })
            .collect()
    }

    /// Sets the chart's kind and each series' own kind from the kinds a file draws its series
    /// as (XLSX chart groups, ODS `chart:class`), one per series in order; `empty` is the
    /// chart's kind when it has no series. Series of one family make a chart of the first's
    /// kind, a series keeping its own kind only where it differs (a line's own markers, a
    /// scatter's own lines); series of other families make a combination chart, every series
    /// keeping its kind, except that the series of one type on one pair of axes share the first
    /// one's grouping, as [`Chart::series_kinds`] draws and every format saves them (a stacked
    /// column after a clustered one on the same axes is kept clustered; on the other axes it
    /// stays stacked). Each series' own kind is then the kind it is drawn as. Set the series'
    /// axes (`secondary`) first.
    pub fn set_drawn_kinds(&mut self, drawn: &[ChartKind], empty: ChartKind) {
        let first = drawn.first().copied().unwrap_or(empty);
        let combo = drawn.iter().any(|k| !same_family(*k, first));
        self.kind = if combo { ChartKind::Combo } else { first };
        for (s, k) in self.series.iter_mut().zip(drawn) {
            s.kind = (combo || *k != first).then_some(*k);
        }
        if combo {
            let kinds = self.series_kinds();
            for (s, k) in self.series.iter_mut().zip(kinds) {
                s.kind = Some(k);
            }
        }
    }

    /// Data tables belong to category-axis column, bar, line and area charts.
    pub fn supports_data_table(&self) -> bool {
        let eligible = |kind| {
            matches!(
                kind,
                ChartKind::ColumnClustered
                    | ChartKind::ColumnStacked
                    | ChartKind::ColumnStacked100
                    | ChartKind::BarClustered
                    | ChartKind::BarStacked
                    | ChartKind::BarStacked100
                    | ChartKind::Line
                    | ChartKind::LineMarkers
                    | ChartKind::LineStacked
                    | ChartKind::Area
                    | ChartKind::AreaStacked
                    | ChartKind::AreaStacked100
            )
        };
        eligible(self.kind) || (self.kind == ChartKind::Combo && self.series_kinds().into_iter().all(eligible))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Image {
    pub id: u32,
    pub anchor: Anchor,
    /// Encoded bytes (PNG/JPEG), base64 in JSON. Shared, so a copy of the sheet (an undo step)
    /// doesn't copy the picture.
    #[serde(with = "crate::b64::shared")]
    pub data: std::sync::Arc<Vec<u8>>,
    pub mime: String,
    pub alt: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeKind {
    #[default]
    Rectangle,
    RoundedRectangle,
    Ellipse,
    Triangle,
    Line,
    Arrow,
    TextBox,
    /// An icon from GridCraft's own icon set; `Shape::text` holds the icon name.
    Icon,
    /// A freehand ink stroke; `Shape::text` holds the points as `x,y;x,y;…` relative to the
    /// anchor (sheet points), `Shape::line` the ink colour and `fill` unused.
    Ink,
    /// Decorative text (`insert.wordArt`): a transparent shape whose `Shape::text` is drawn with
    /// the large, filled/outlined/shadowed text properties of `Shape::format`.
    WordArt,
    /// An equation: `Shape::text` holds its LaTeX source, `Shape::line` its ink colour
    /// (`Color::Auto` for the sheet's text colour) and `Shape::math` its cached layout.
    Equation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub id: u32,
    pub kind: ShapeKind,
    pub anchor: Anchor,
    pub fill: Color,
    pub line: Color,
    pub text: String,
    /// Outline visibility and text formatting (DrawingML `a:ln`, `a:bodyPr`, `a:pPr`, `a:rPr`).
    #[serde(default)]
    pub format: ShapeFormat,
    /// Degrees clockwise about the anchor rect's centre, in `[0, 360)`.
    #[serde(default)]
    pub rotation: f32,
    /// An equation's layout, rebuilt from `text` and never saved.
    #[serde(skip)]
    pub math: EquationCache,
}

/// The laid-out form of an equation's LaTeX source. It is derived data: rebuilt by
/// [`EquationCache::refresh`] whenever the source changes, never saved, and ignored when shapes
/// are compared. Painters read it and never lay equations out themselves, so nothing in a paint
/// path can reach the layout engine.
///
/// An entry answers only for the source it was built from, compared in full: a hash match alone
/// is not enough, so two sources whose hashes collide never share a layout. A source over
/// [`gridcraft_math::MAX_SOURCE_BYTES`] never lays out, so [`EquationCache::get`] answers for it
/// by its length alone, with [`gridcraft_math::SOURCE_TOO_LONG`], and an entry built from one holds
/// neither the source nor its hash: a painter asking every frame about a source of megabytes read
/// from a file does no work on it.
#[derive(Clone, Debug, Default)]
pub struct EquationCache {
    /// A hash of the source the entry was built from (a quick first check).
    key: u64,
    /// The source's length in bytes.
    len: usize,
    /// The source itself, when it is within the length cap.
    source: Option<std::sync::Arc<str>>,
    layout: Option<Result<std::sync::Arc<gridcraft_math::MathList>, gridcraft_math::MathError>>,
}

impl PartialEq for EquationCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

fn source_key(src: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    #[cfg(test)]
    equation_cache_tests::HASHES.with(|h| h.set(h.get() + 1));
    let mut h = std::collections::hash_map::DefaultHasher::new();
    src.hash(&mut h);
    h.finish()
}

impl EquationCache {
    /// Whether this entry was built from `src`. Over the cap the length is the match (an entry for
    /// such a source is always the same error); within it, the hash and then the source itself.
    fn holds(&self, src: &str) -> bool {
        if self.layout.is_none() || self.len != src.len() {
            return false;
        }
        if src.len() > gridcraft_math::MAX_SOURCE_BYTES {
            return self.source.is_none();
        }
        self.key == source_key(src) && self.source.as_deref() == Some(src)
    }

    /// Makes this the entry for `src`, with `layout`. A source over the cap is neither copied nor
    /// hashed.
    fn store(&mut self, src: &str, layout: Result<std::sync::Arc<gridcraft_math::MathList>, gridcraft_math::MathError>) {
        let within = src.len() <= gridcraft_math::MAX_SOURCE_BYTES;
        self.key = if within { source_key(src) } else { 0 };
        self.len = src.len();
        self.source = within.then(|| std::sync::Arc::from(src));
        self.layout = Some(layout);
    }

    /// The layout of `src`, or why it has none; `None` until [`EquationCache::refresh`] has run
    /// for this source. A source over [`gridcraft_math::MAX_SOURCE_BYTES`] is answered at once,
    /// whatever the entry holds, by its length alone.
    pub fn get(&self, src: &str) -> Option<Result<&gridcraft_math::MathList, &gridcraft_math::MathError>> {
        if src.len() > gridcraft_math::MAX_SOURCE_BYTES {
            return Some(Err(&*gridcraft_math::SOURCE_TOO_LONG));
        }
        if !self.holds(src) {
            return None;
        }
        self.layout.as_ref().map(|r| r.as_ref().map(|l| l.as_ref()))
    }

    /// Lays `src` out again unless the cache already holds its layout. True when it did.
    pub fn refresh(&mut self, src: &str) -> bool {
        if self.holds(src) {
            return false;
        }
        self.store(src, gridcraft_math::layout(src).map(std::sync::Arc::new));
        true
    }

    /// Records that `src` was not laid out, and why, without running the layout engine.
    pub fn refuse(&mut self, src: &str, message: &str) {
        self.store(src, Err(gridcraft_math::MathError { span: None, message: message.to_string() }));
    }
}

/// What a form control on the sheet is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ControlKind {
    #[default]
    Button,
    Slider,
    Spin,
}

impl ControlKind {
    /// The kind's name in the window: Button, Slider, Spin Button.
    pub fn label(self) -> &'static str {
        match self {
            ControlKind::Button => "Button",
            ControlKind::Slider => "Slider",
            ControlKind::Spin => "Spin Button",
        }
    }
}

/// The largest value a slider or spin button takes (Excel's scroll bar limit). Values are whole
/// numbers from 0 to this.
pub const CONTROL_MAX: u32 = 30_000;

/// What a button runs when it is pressed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ControlAction {
    /// Runs a macro of the workbook on `input`, optionally sending its output to `output`.
    Macro {
        name: String,
        #[serde(default)]
        input: MacroInput,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    /// Runs one GridCraft command with fixed parameters.
    Command {
        id: String,
        #[serde(default)]
        params: serde_json::Value,
    },
}

/// The range a button's macro runs on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MacroInput {
    /// The selection when the button is pressed.
    #[default]
    Selection,
    /// Always this range (`B2:D9` or `Data!B2:D9`).
    Fixed(String),
}

/// A form control (button, slider, spin button) floating over the sheet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FormControl {
    pub id: u32,
    pub kind: ControlKind,
    pub anchor: Anchor,
    /// The name shown in the Selection Pane (`Button 3`); empty means the kind and id.
    pub name: String,
    /// The caption of a button.
    pub text: String,
    /// The linked cell as reference text: `$B$2` on the control's own sheet, or `Sheet2!$B$2`.
    /// Structural edits rewrite it like a formula reference.
    pub link: Option<String>,
    /// The current value of a slider or spin button (the linked cell holds it too).
    pub value: u32,
    pub min: u32,
    pub max: u32,
    /// Incremental change: one arrow click or arrow key.
    pub step: u32,
    /// Page change (sliders): a click on the track, or Page Up / Page Down.
    pub page: u32,
    /// A slider lies horizontally (false: vertically).
    pub horizontal: bool,
    /// What a button runs.
    pub action: Option<ControlAction>,
    /// Printed with the sheet.
    pub print: bool,
    /// Locked when the sheet is protected.
    pub locked: bool,
}

impl Default for FormControl {
    fn default() -> Self {
        FormControl {
            id: 0,
            kind: ControlKind::default(),
            anchor: Anchor::default(),
            name: String::new(),
            text: String::new(),
            link: None,
            value: 0,
            min: 0,
            max: 100,
            step: 1,
            page: 10,
            horizontal: true,
            action: None,
            print: true,
            locked: true,
        }
    }
}

impl FormControl {
    /// The name shown for the control: its own, else `Button 3`, `Slider 4`, `Spin Button 5`.
    pub fn display_name(&self) -> String {
        if self.name.trim().is_empty() { format!("{} {}", self.kind.label(), self.id) } else { self.name.clone() }
    }

    /// Checks the numbers of a slider or spin button: 0 ≤ min < max ≤ 30000, 1 ≤ step ≤ max − min,
    /// and (sliders) page ≥ step. Each rule has its own message.
    pub fn check_numbers(&self) -> Result<(), String> {
        if self.kind == ControlKind::Button {
            return Ok(());
        }
        if self.max > CONTROL_MAX {
            return Err(format!("Maximum value can be at most {CONTROL_MAX}."));
        }
        if self.min > CONTROL_MAX {
            return Err(format!("Minimum value must be a whole number from 0 to {CONTROL_MAX}."));
        }
        if self.min >= self.max {
            return Err(format!("Maximum value ({}) must be greater than Minimum value ({}).", self.max, self.min));
        }
        if self.step < 1 {
            return Err("Incremental change must be at least 1.".into());
        }
        let span = self.max - self.min;
        if self.step > span {
            return Err(format!("Incremental change ({}) can be at most Maximum minus Minimum ({span}).", self.step));
        }
        if self.kind == ControlKind::Slider {
            if self.page < self.step {
                return Err(format!("Page change ({}) must be at least the incremental change ({}).", self.page, self.step));
            }
            if self.page > CONTROL_MAX {
                return Err(format!("Page change can be at most {CONTROL_MAX}."));
            }
        }
        Ok(())
    }

    /// `v` clamped to min..max and snapped to the nearest step from min (max stays reachable).
    pub fn snap(&self, v: f64) -> u32 {
        let (min, max) = (self.min.min(CONTROL_MAX), self.max.min(CONTROL_MAX).max(self.min.min(CONTROL_MAX)));
        if !v.is_finite() || v <= min as f64 {
            return min;
        }
        if v >= max as f64 {
            return max;
        }
        let step = self.step.max(1) as f64;
        let snapped = (min as f64 + ((v - min as f64) / step).round() * step).clamp(min as f64, max as f64);
        // When the step doesn't divide the range, the maximum is a stop of its own.
        if (max as f64 - v).abs() < (snapped - v).abs() { max } else { snapped as u32 }
    }
}

/// The kinds of floating object a sheet holds. Commands name an object by kind and
/// workbook-unique id (`{kind: "chart", id}`); [`ObjKind::name`] is that parameter's spelling.
/// Equations are stored as shapes ([`ShapeKind::Equation`]) but select, open and delete as their
/// own kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ObjKind {
    Chart,
    Image,
    Shape,
    Slicer,
    Equation,
    Control,
}

impl ObjKind {
    pub const ALL: [ObjKind; 6] = [ObjKind::Chart, ObjKind::Image, ObjKind::Shape, ObjKind::Slicer, ObjKind::Equation, ObjKind::Control];

    /// The name used in command parameters.
    pub fn name(self) -> &'static str {
        match self {
            ObjKind::Chart => "chart",
            ObjKind::Image => "image",
            ObjKind::Shape => "shape",
            ObjKind::Slicer => "slicer",
            ObjKind::Equation => "equation",
            ObjKind::Control => "control",
        }
    }

    /// The kind named `s` (any letter case).
    pub fn parse(s: &str) -> Option<ObjKind> {
        ObjKind::ALL.into_iter().find(|k| k.name().eq_ignore_ascii_case(s))
    }
}

/// One floating object on a sheet: its kind and its workbook-unique id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObjRef {
    pub kind: ObjKind,
    pub id: u32,
}

impl ObjRef {
    pub fn new(kind: ObjKind, id: u32) -> ObjRef {
        ObjRef { kind, id }
    }
}

/// Horizontal alignment of a shape's text (`a:pPr algn`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeTextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Vertical anchor of a shape's text in its box (`a:bodyPr anchor`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeTextAnchor {
    #[default]
    Top,
    Middle,
    Bottom,
}

/// How a shape's outline and text look. The text properties apply to the whole text (the first
/// paragraph's and first run's properties when a file has several).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShapeFormat {
    /// No outline (`a:ln/a:noFill`), whatever `Shape::line` says.
    pub no_line: bool,
    /// Text size in points (`a:rPr sz` / 100); `None` is the default size.
    pub font_size: Option<f32>,
    pub bold: bool,
    pub italic: bool,
    /// Typeface (`a:latin typeface`); `None` is the default font.
    pub font: Option<String>,
    /// Text fill colour; `None` is the default (black in text boxes).
    pub text_color: Option<Color>,
    /// Text outline (`a:rPr/a:ln`): colour and width in points.
    pub text_outline: Option<(Color, f32)>,
    /// Text drop shadow colour (`a:rPr/a:effectLst/a:outerShdw`); `None` is no shadow.
    pub text_shadow: Option<Color>,
    /// `None`: the shape's default (left for text boxes, centred for other shapes).
    pub align: Option<ShapeTextAlign>,
    /// `None`: the shape's default (top for text boxes, middle for other shapes).
    pub anchor: Option<ShapeTextAnchor>,
    /// Paragraphs run right to left (`a:pPr rtl`).
    pub rtl: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SparklineKind {
    #[default]
    Line,
    Column,
    WinLoss,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sparkline {
    pub cell: CellRef,
    pub source: String,
    pub kind: SparklineKind,
    pub color: Color,
    pub markers: bool,
}

// ---------------------------------------------------------------- filter & sort

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum FilterCriterion {
    /// Show rows whose display text is one of these (case-insensitive); `blanks` includes empty.
    Values {
        values: Vec<String>,
        blanks: bool,
    },
    /// Custom: one or two comparisons (`op` like `>=`, `=*abc*`), joined by and/or.
    Custom {
        a: (String, String),
        b: Option<(String, String)>,
        and: bool,
    },
    Top10 {
        bottom: bool,
        percent: bool,
        count: u32,
    },
    AboveAverage(bool),
    FillColor(Color),
    FontColor(Color),
    /// Rows whose cell shows icon `id` (0-based; `None` = no icon) of conditional-format icon set
    /// `set` (`iconFilter`). Kept from files; re-applying the filter leaves those rows as they are.
    Icon {
        set: String,
        id: Option<u32>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutoFilter {
    pub range: RangeRef,
    /// Column offset within `range` → criterion.
    pub criteria: Vec<(u32, FilterCriterion)>,
}

/// A slicer: an on-sheet filter widget bound to one column of a table. Clicking its value tiles
/// sets that column's `FilterCriterion::Values` in the sheet's [`AutoFilter`] and re-applies it,
/// so a slicer is just a visual front-end over the existing filter machinery.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Slicer {
    pub id: u32,
    /// Name of the table this slicer filters.
    pub table: String,
    /// Header name of the filtered column within the table.
    pub column: String,
    /// Caption shown in the slicer header (defaults to the column name).
    pub caption: String,
    pub anchor: Anchor,
    /// Selected display values. `None` means every value (no filter from this slicer); an empty
    /// list means nothing is selected (all rows hidden), matching Excel's "clear all buttons".
    pub selected: Option<Vec<String>>,
    /// Number of button columns in the layout (Excel default 1).
    pub columns: u32,
}

/// Work counters for growth tests here and in other crates (`test-support`; never in a shipped
/// build): a test compares the counts at two input sizes instead of timing the work.
#[cfg(any(test, feature = "test-support"))]
pub mod work {
    thread_local! {
        /// Series a combo chart's kind lookups have visited on this thread.
        static SERIES_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    pub(crate) fn visit() {
        SERIES_VISITS.with(|n| n.set(n.get().saturating_add(1)));
    }

    /// The series visited since the last call, and starts counting again.
    pub fn take_series_visits() -> usize {
        SERIES_VISITS.with(|n| n.replace(0))
    }
}

#[cfg(test)]
mod series_kind_tests {
    use super::{Chart, ChartKind, Series};

    fn chart(kind: ChartKind, own: &[Option<ChartKind>]) -> Chart {
        Chart {
            id: 1,
            kind,
            anchor: Default::default(),
            title: None,
            series: own.iter().map(|k| Series { kind: *k, ..Series::default() }).collect(),
            legend: Default::default(),
            data_labels: false,
            data_table: None,
            gridlines: true,
            style: 2,
            hole_size: 75,
            first_slice_angle: 0,
            x_title: None,
            y_title: None,
            source: None,
            by_rows: false,
            pivot: None,
        }
    }

    fn drawn(c: &Chart) -> Vec<ChartKind> {
        (0..c.series.len()).map(|i| c.series_kind(i)).collect()
    }

    #[test]
    fn combo_series_default_to_a_column_then_lines_and_bars_and_pies_stand_upright() {
        use ChartKind::*;
        let c = chart(Combo, &[None, None, Some(Combo), Some(Area), Some(BarClustered), Some(Doughnut)]);
        assert_eq!(drawn(&c), vec![ColumnClustered, Line, Line, Area, ColumnClustered, ColumnClustered]);
        for (bar, column) in [(BarStacked, ColumnStacked), (BarStacked100, ColumnStacked100)] {
            assert_eq!(drawn(&chart(Combo, &[Some(bar), Some(Line)])), vec![column, Line]);
        }
        // On a combo's category axes scatter and radar series are lines (a markers-only scatter
        // keeps its markers) and bubbles are columns.
        let c = chart(Combo, &[Some(Scatter), Some(ScatterLines), Some(Radar), Some(Bubble)]);
        assert_eq!(drawn(&c), vec![LineMarkers, Line, Line, ColumnClustered]);
        // Out of range: the default for that position, never a panic.
        assert_eq!(c.series_kind(usize::MAX), Line);
    }

    #[test]
    fn outside_a_combo_a_series_keeps_only_its_own_markers_or_lines() {
        use ChartKind::*;
        assert_eq!(drawn(&chart(Line, &[None, Some(LineMarkers), Some(Area)])), vec![Line, LineMarkers, Line]);
        assert_eq!(drawn(&chart(ScatterLines, &[Some(Scatter), Some(Line)])), vec![Scatter, ScatterLines]);
        assert_eq!(drawn(&chart(ColumnClustered, &[Some(Line)])), vec![ColumnClustered]);
    }

    #[test]
    fn combo_series_of_one_type_on_one_axis_share_the_first_grouping() {
        use ChartKind::*;
        let c = chart(
            Combo,
            &[Some(ColumnStacked), Some(ColumnClustered), Some(Pie), Some(LineStacked), Some(LineMarkers), Some(Area), Some(AreaStacked100)],
        );
        assert_eq!(drawn(&c), vec![ColumnStacked, ColumnStacked, ColumnStacked, LineStacked, LineStacked, Area, Area]);
        assert_eq!(c.series_kinds(), drawn(&c));
        // An unstacked line keeps its own markers; a stacked one after it is unstacked.
        let c = chart(Combo, &[Some(Line), Some(LineMarkers), Some(LineStacked)]);
        assert_eq!(drawn(&c), vec![Line, LineMarkers, Line]);
        // Each pair of axes has its own first.
        let mut c = chart(Combo, &[Some(ColumnClustered), Some(ColumnStacked)]);
        c.series[1].secondary = true;
        assert_eq!(drawn(&c), vec![ColumnClustered, ColumnStacked]);
        assert_eq!(c.series_kinds(), drawn(&c));
    }

    /// A series' own markers or lines outside a combo are honoured wherever they come from: an
    /// XLSX file's per-series markers, or a kind left in an older GridCraft JSON workbook by a
    /// type change made before `chart.set` cleared series kinds on a change of type (not
    /// migrated: the two can't be told apart).
    /// A file's per-series kinds make a chart of one family, or a combo, that draws them so.
    #[test]
    fn drawn_kinds_set_the_chart_and_its_series() {
        use ChartKind::*;
        let mut c = chart(ColumnClustered, &[None, None, None]);
        c.set_drawn_kinds(&[Line, LineMarkers, Line], ColumnClustered);
        assert_eq!((c.kind, c.series.iter().map(|s| s.kind).collect::<Vec<_>>()), (Line, vec![None, Some(LineMarkers), None]));
        assert_eq!(c.series_kinds(), vec![Line, LineMarkers, Line]);
        c.set_drawn_kinds(&[ColumnStacked, Line, Area], ColumnClustered);
        assert_eq!(c.kind, Combo);
        assert_eq!(c.series_kinds(), vec![ColumnStacked, Line, Area]);
        let mut empty = chart(ColumnClustered, &[]);
        empty.set_drawn_kinds(&[], Radar);
        assert_eq!(empty.kind, Radar);
    }

    /// Series of one type on one pair of axes keep the first one's grouping, so the kinds a
    /// file gives them are stored as they are drawn and saved; on the other axes they keep their
    /// own.
    #[test]
    fn drawn_kinds_of_one_type_share_a_grouping_per_pair_of_axes() {
        use ChartKind::*;
        let mut c = chart(ColumnClustered, &[None, None, None, None]);
        c.set_drawn_kinds(&[ColumnClustered, ColumnStacked, ColumnStacked, Line], ColumnClustered);
        assert_eq!(
            c.series.iter().map(|s| s.kind).collect::<Vec<_>>(),
            vec![Some(ColumnClustered); 3].into_iter().chain([Some(Line)]).collect::<Vec<_>>()
        );
        assert_eq!(c.series.iter().map(|s| s.kind).collect::<Vec<_>>(), c.series_kinds().into_iter().map(Some).collect::<Vec<_>>());
        let mut c = chart(ColumnClustered, &[None, None, None, None]);
        c.series[1].secondary = true;
        c.series[2].secondary = true;
        c.set_drawn_kinds(&[ColumnClustered, ColumnStacked, ColumnStacked, Line], ColumnClustered);
        assert_eq!(c.series_kinds(), vec![ColumnClustered, ColumnStacked, ColumnStacked, Line]);
        assert_eq!(c.series.iter().map(|s| s.kind).collect::<Vec<_>>(), c.series_kinds().into_iter().map(Some).collect::<Vec<_>>());
    }

    #[test]
    fn a_leftover_kind_in_the_same_family_is_drawn() {
        use ChartKind::*;
        let json = serde_json::to_value(chart(Line, &[None, Some(LineMarkers)])).unwrap();
        let c: Chart = serde_json::from_value(json).unwrap();
        assert_eq!(drawn(&c), vec![Line, LineMarkers]);
        assert_eq!(drawn(&chart(ScatterLines, &[Some(Scatter)])), vec![Scatter]);
    }

    #[test]
    fn a_combo_supports_a_data_table_by_its_drawn_kinds() {
        use ChartKind::*;
        assert!(chart(Combo, &[None, None]).supports_data_table());
        // Its scatter series are drawn as lines on its category axes; stock series have no table.
        assert!(chart(Combo, &[None, Some(Scatter)]).supports_data_table());
        assert!(!chart(Combo, &[None, Some(Stock)]).supports_data_table());
    }
}

#[cfg(test)]
mod trendline_forecast_tests {
    use super::Trendline;

    #[test]
    fn clamp_forecast_bounds_hostile_values() {
        assert_eq!(Trendline::clamp_forecast(2.5), 2.5);
        assert_eq!(Trendline::clamp_forecast(0.0), 0.0);
        assert_eq!(Trendline::clamp_forecast(-4.0), 0.0);
        assert_eq!(Trendline::clamp_forecast(f64::NAN), 0.0);
        assert_eq!(Trendline::clamp_forecast(f64::INFINITY), 0.0);
        assert_eq!(Trendline::clamp_forecast(f64::NEG_INFINITY), 0.0);
        assert_eq!(Trendline::clamp_forecast(1e300), Trendline::MAX_FORECAST);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chart_without_hole_size_keeps_the_legacy_hole() {
        let chart = Chart {
            id: 1,
            kind: ChartKind::Doughnut,
            anchor: Anchor::default(),
            title: None,
            series: vec![],
            legend: LegendPos::Right,
            data_labels: false,
            data_table: None,
            gridlines: false,
            style: 1,
            hole_size: 75,
            first_slice_angle: 0,
            x_title: None,
            y_title: None,
            source: None,
            by_rows: false,
            pivot: None,
        };
        let mut json = serde_json::to_value(&chart).expect("serialize chart");
        json.as_object_mut().expect("chart object").remove("hole_size");
        let legacy: Chart = serde_json::from_value(json).expect("deserialize legacy chart");
        assert_eq!(legacy.hole_size, 50, "documents saved before hole_size existed keep the old 50% hole");
        assert_eq!(chart.hole_size, 75, "new charts keep Excel's 75% default");
    }
}

#[cfg(test)]
mod equation_cache_tests {
    use super::{EquationCache, source_key};

    thread_local! {
        /// How many sources `source_key` has hashed on this thread.
        pub(super) static HASHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    fn glyphs(c: &EquationCache, src: &str) -> Option<String> {
        let list = c.get(src)?.ok()?;
        Some(list.items.iter().filter_map(|i| if let gridcraft_math::Item::Glyph(g) = i { Some(g.ch) } else { None }).collect())
    }

    #[test]
    fn a_hash_collision_never_returns_another_sources_layout() {
        let (a, b) = ("x^2", "y^3");
        let mut c = EquationCache::default();
        assert!(c.refresh(a));
        assert_eq!(glyphs(&c, a).as_deref(), Some("x2"));
        // Forge a collision: the entry claims `b`'s hash; `b` has `a`'s length, so only the
        // full comparison tells them apart.
        c.key = source_key(b);
        assert!(c.get(b).is_none(), "`b` is a miss, not `a`'s layout");
        assert!(c.refresh(b), "`b` is laid out afresh");
        assert_eq!(glyphs(&c, b).as_deref(), Some("y3"));
        assert!(c.get(a).is_none(), "the entry is `b`'s now");
        // A refusal does not answer for a colliding source either.
        c.refuse(a, "not laid out");
        c.key = source_key(b);
        assert!(c.get(b).is_none());
    }

    #[test]
    fn sources_over_the_cap_are_not_copied_hashed_or_laid_out() {
        let long = "x".repeat(gridcraft_math::MAX_SOURCE_BYTES + 1);
        let mut c = EquationCache::default();
        HASHES.with(|h| h.set(0));
        assert!(matches!(c.get(&long), Some(Err(e)) if e.message.contains("4096")), "answered before any refresh");
        assert!(c.refresh(&long));
        assert!(c.source.is_none(), "an over-cap source is not kept");
        assert_eq!(c.key, 0, "nor hashed");
        assert!(matches!(c.get(&long), Some(Err(_))));
        assert!(!c.refresh(&long), "still cached");
        let other = "y".repeat(gridcraft_math::MAX_SOURCE_BYTES + 1);
        assert!(matches!(c.get(&other), Some(Err(_))), "another over-cap source gets the same answer");
        assert_eq!(HASHES.with(|h| h.get()), 0, "nothing over the cap is hashed");
        // Within the cap the lookup is unchanged: a hash, then the source compared in full.
        assert!(c.refresh("x"));
        assert!(c.get("x").is_some_and(|r| r.is_ok()));
        assert!(HASHES.with(|h| h.get()) >= 2);
    }

    #[test]
    #[allow(clippy::disallowed_methods)] // tests run only off wasm, where the clock exists
    fn a_source_of_a_megabyte_is_looked_up_without_being_read() {
        // The sheet painter asks the cache every frame and the PDF export every draw; a source
        // read from a file can be megabytes, and the lookup must not scan it each time.
        let big = "l".repeat(1 << 20);
        let mut c = EquationCache::default();
        c.refuse(&big, "budget");
        HASHES.with(|h| h.set(0));
        let start = std::time::Instant::now();
        for _ in 0..1000 {
            assert!(matches!(c.get(&big), Some(Err(_))));
        }
        let took = start.elapsed();
        assert_eq!(HASHES.with(|h| h.get()), 0, "a thousand lookups read nothing");
        assert!(took < std::time::Duration::from_millis(500), "{took:?}");
    }
}
