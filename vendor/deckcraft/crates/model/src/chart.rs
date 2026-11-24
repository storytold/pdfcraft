//! Charts: an embedded data grid plus type and element options.

use serde::{Deserialize, Serialize};

use crate::style::{ColorRef, Fill, Line};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChartType {
    #[default]
    Column,
    StackedColumn,
    PercentColumn,
    Bar,
    StackedBar,
    PercentBar,
    Line,
    LineMarkers,
    StackedLine,
    Area,
    StackedArea,
    Pie,
    Doughnut,
    Scatter,
    Bubble,
    Radar,
    FilledRadar,
    Stock,
    Surface,
    Histogram,
    Pareto,
    BoxWhisker,
    Waterfall,
    Funnel,
    Treemap,
    Sunburst,
    /// A filled (choropleth) map of regions coloured by value, kept and drawn from its data; it
    /// is not offered in the chart menus (PowerPoint needs an online map service to make one).
    RegionMap,
    Combo,
}

impl ChartType {
    /// The Office 2016 (`chartEx`) types, offered after [`ChartType::MAIN`] in chart menus.
    pub const OFFICE_2016: [ChartType; 7] = [
        ChartType::Waterfall,
        ChartType::Funnel,
        ChartType::Histogram,
        ChartType::Pareto,
        ChartType::BoxWhisker,
        ChartType::Treemap,
        ChartType::Sunburst,
    ];

    /// Whether the type is saved as a `chartEx` part (with a plain chart as the fallback).
    pub fn is_chartex(self) -> bool {
        Self::OFFICE_2016.contains(&self) || self == ChartType::RegionMap
    }

    pub const MAIN: [ChartType; 12] = [
        ChartType::Column,
        ChartType::Bar,
        ChartType::Line,
        ChartType::Area,
        ChartType::Pie,
        ChartType::Doughnut,
        ChartType::Scatter,
        ChartType::Radar,
        ChartType::Waterfall,
        ChartType::Funnel,
        ChartType::Histogram,
        ChartType::Treemap,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ChartType::Column => "Clustered Column",
            ChartType::StackedColumn => "Stacked Column",
            ChartType::PercentColumn => "100% Stacked Column",
            ChartType::Bar => "Clustered Bar",
            ChartType::StackedBar => "Stacked Bar",
            ChartType::PercentBar => "100% Stacked Bar",
            ChartType::Line => "Line",
            ChartType::LineMarkers => "Line with Markers",
            ChartType::StackedLine => "Stacked Line",
            ChartType::Area => "Area",
            ChartType::StackedArea => "Stacked Area",
            ChartType::Pie => "Pie",
            ChartType::Doughnut => "Doughnut",
            ChartType::Scatter => "Scatter",
            ChartType::Bubble => "Bubble",
            ChartType::Radar => "Radar",
            ChartType::FilledRadar => "Filled Radar",
            ChartType::Stock => "Stock",
            ChartType::Surface => "Surface",
            ChartType::Histogram => "Histogram",
            ChartType::Pareto => "Pareto",
            ChartType::BoxWhisker => "Box and Whisker",
            ChartType::Waterfall => "Waterfall",
            ChartType::Funnel => "Funnel",
            ChartType::Treemap => "Treemap",
            ChartType::Sunburst => "Sunburst",
            ChartType::RegionMap => "Map",
            ChartType::Combo => "Combo",
        }
    }
    pub fn is_bar_like(self) -> bool {
        matches!(
            self,
            ChartType::Column
                | ChartType::StackedColumn
                | ChartType::PercentColumn
                | ChartType::Bar
                | ChartType::StackedBar
                | ChartType::PercentBar
                | ChartType::Histogram
                | ChartType::Pareto
                | ChartType::Waterfall
        )
    }
    pub fn horizontal(self) -> bool {
        matches!(self, ChartType::Bar | ChartType::StackedBar | ChartType::PercentBar | ChartType::Funnel)
    }
    pub fn stacked(self) -> bool {
        matches!(
            self,
            ChartType::StackedColumn
                | ChartType::StackedBar
                | ChartType::StackedLine
                | ChartType::StackedArea
                | ChartType::PercentColumn
                | ChartType::PercentBar
        )
    }
    pub fn percent(self) -> bool {
        matches!(self, ChartType::PercentColumn | ChartType::PercentBar)
    }
}

/// `c:trendlineType`: the six trendlines DrawingML charts store.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrendlineType {
    Exp,
    #[default]
    Linear,
    Log,
    MovingAvg,
    Poly,
    Power,
}

impl TrendlineType {
    pub fn xml(self) -> &'static str {
        match self {
            TrendlineType::Exp => "exp",
            TrendlineType::Linear => "linear",
            TrendlineType::Log => "log",
            TrendlineType::MovingAvg => "movingAvg",
            TrendlineType::Poly => "poly",
            TrendlineType::Power => "power",
        }
    }
    pub fn from_xml(s: &str) -> Option<Self> {
        Some(match s {
            "exp" => TrendlineType::Exp,
            "linear" => TrendlineType::Linear,
            "log" => TrendlineType::Log,
            "movingAvg" => TrendlineType::MovingAvg,
            "poly" => TrendlineType::Poly,
            "power" => TrendlineType::Power,
            _ => return None,
        })
    }
}

/// One `c:trendline` on a series. Polynomials use `order` (2–6); moving averages use `period`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Trendline {
    pub kind: TrendlineType,
    /// `c:name`, when the file sets one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Polynomial order (`c:order`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<u32>,
    /// Moving-average period (`c:period`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<u32>,
    /// Forecast forward and backward, in category units (`c:forward`, `c:backward`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backward: Option<f64>,
    /// Cross the value axis at this value (`c:intercept`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intercept: Option<f64>,
    /// Show the equation (`c:dispEq`).
    pub disp_eq: bool,
    /// Show R² (`c:dispRSqr`).
    pub disp_r_sqr: bool,
}

impl Default for Trendline {
    fn default() -> Self {
        Trendline {
            kind: TrendlineType::Linear,
            name: None,
            order: None,
            period: None,
            forward: None,
            backward: None,
            intercept: None,
            disp_eq: false,
            disp_r_sqr: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Series {
    pub name: String,
    pub values: Vec<Option<f64>>,
    /// X values for scatter/bubble.
    pub x_values: Vec<Option<f64>>,
    pub sizes: Vec<Option<f64>>,
    pub fill: Option<Fill>,
    pub line: Option<Line>,
    /// Per-point fill overrides (pie slices).
    pub point_fills: Vec<(usize, Fill)>,
    pub data_labels: Option<bool>,
    pub smooth: bool,
    pub marker: Option<String>,
    /// Combo charts: this series' own type.
    pub kind: Option<ChartType>,
    pub secondary_axis: bool,
    /// The number format of the series' data labels (Excel code): the labels' own, else the
    /// source data's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_format: Option<String>,
    /// Trendlines (`c:trendline`), in file order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub trendlines: Vec<Trendline>,
    /// `c:errBars` on the values (y bars; a bar chart's run along its value axis).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_bars: Option<ErrorBars>,
}

/// Error bars (`c:errBars`): how far each point's bar reaches below and above it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ErrorBars {
    /// `both`, `plus` or `minus`.
    pub bar_type: String,
    /// `fixedVal`, `percentage`, `stdDev`, `stdErr` or `cust`.
    pub val_type: String,
    pub value: f64,
    /// `cust`: the per-point plus and minus amounts.
    pub plus: Vec<Option<f64>>,
    pub minus: Vec<Option<f64>>,
    pub end_cap: bool,
}

impl Default for ErrorBars {
    fn default() -> Self {
        ErrorBars { bar_type: "both".into(), val_type: "fixedVal".into(), value: 1.0, plus: vec![], minus: vec![], end_cap: true }
    }
}

/// Count, mean and sample standard deviation of a series' finite values (for error bars).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SeriesStats {
    pub n: f64,
    pub mean: f64,
    pub sd: f64,
}

impl SeriesStats {
    pub fn of(values: &[Option<f64>]) -> Self {
        let vals = || values.iter().flatten().copied().filter(|v| v.is_finite());
        let n = vals().count() as f64;
        let mean = vals().sum::<f64>() / n.max(1.0);
        let sd = if n > 1.0 { (vals().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt() } else { 0.0 };
        SeriesStats { n, mean, sd }
    }
}

impl ErrorBars {
    /// Where point `i` (value `v`) of a series with `values` has its bar: (low, high) in data units.
    /// Standard deviation bars stand around the series' mean, as Excel draws them; the others
    /// around the point. `None` for an amount that isn't a finite number.
    pub fn range(&self, values: &[Option<f64>], i: usize, v: f64) -> Option<(f64, f64)> {
        self.range_in(&SeriesStats::of(values), i, v)
    }

    /// [`ErrorBars::range`] with the series' statistics worked out once ([`SeriesStats::of`]), so
    /// drawing every point of a long series stays linear.
    pub fn range_in(&self, stats: &SeriesStats, i: usize, v: f64) -> Option<(f64, f64)> {
        let SeriesStats { n, mean, sd } = *stats;
        let (centre, minus, plus) = match self.val_type.as_str() {
            "percentage" => (v, (v * self.value / 100.0).abs(), (v * self.value / 100.0).abs()),
            "stdDev" => (mean, sd * self.value, sd * self.value),
            "stdErr" => (v, sd / n.max(1.0).sqrt(), sd / n.max(1.0).sqrt()),
            "cust" => (v, self.minus.get(i).copied().flatten().unwrap_or(0.0), self.plus.get(i).copied().flatten().unwrap_or(0.0)),
            _ => (v, self.value, self.value),
        };
        let (lo, hi) = match self.bar_type.as_str() {
            "plus" => (centre, centre + plus),
            "minus" => (centre - minus, centre),
            _ => (centre - minus, centre + plus),
        };
        (lo.is_finite() && hi.is_finite()).then_some((lo, hi))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Chart {
    pub kind: ChartType,
    pub title: Option<String>,
    pub categories: Vec<String>,
    pub series: Vec<Series>,
    pub legend: Option<String>,
    /// `c:legend/c:overlay`: the legend is drawn over the plot instead of shrinking it.
    pub legend_overlay: bool,
    /// `c:legend/c:legendEntry`: entries whose text has its own size, weight or colour, over
    /// the legend's.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub legend_entries: Vec<LegendEntry>,
    /// Text size in points of the axis labels and legend (from their `c:txPr`, else the chart's);
    /// `None`: the file gives none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_size: Option<f64>,
    /// Title text size in points (its rich text or `c:txPr`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_size: Option<f64>,
    pub data_labels: bool,
    pub gridlines: bool,
    /// `c:minorGridlines` on the value axis: five to a major unit, between the major ones.
    pub minor_gridlines: bool,
    pub axis_titles: (Option<String>, Option<String>),
    /// Chart style number (1–48) and colour palette (`colorful1`, `monochrome3`…).
    pub style: u32,
    pub palette: String,
    pub plot_fill: Option<Fill>,
    pub chart_fill: Option<Fill>,
    pub text_color: Option<ColorRef>,
    pub hole_size: f64,
    pub gap_width: f64,
    pub overlap: f64,
    pub vary_colors: bool,
    pub y_min: Option<f64>,
    pub y_max: Option<f64>,
    /// The value axis' major unit, when the chart fixes it.
    pub y_major: Option<f64>,
    /// `c:scaling/c:logBase`: the value axis is logarithmic in this base (2–1000).
    pub y_log_base: Option<f64>,
    /// The value axis' number format (an Excel format code such as `#,##0` or `0.0%`).
    pub y_format: Option<String>,
    /// The value axis is deleted (`c:delete`): no labels, though gridlines may stay.
    pub y_hidden: bool,
    /// The category axis runs backwards (`c:orientation val="maxMin"`): right to left, or on a
    /// horizontal bar chart top to bottom (the default puts the first category at the bottom).
    pub cat_reversed: bool,
    /// A combo chart's secondary value axis: bounds, major unit and number format.
    pub y2_min: Option<f64>,
    pub y2_max: Option<f64>,
    pub y2_major: Option<f64>,
    pub y2_format: Option<String>,
    pub y2_hidden: bool,
    /// The chart's language (`c:lang`, e.g. `ru-RU`): digit grouping and decimal mark.
    pub lang: Option<String>,
    /// A waterfall's total points (`cx:subtotals`): bars that stand on the axis and restart
    /// the running total, instead of floating.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subtotals: Vec<usize>,
    /// Outer category levels of a treemap or sunburst, innermost first (the leaves' parents,
    /// then theirs…), each with one label per category. A blank label continues the one above it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub category_levels: Vec<Vec<String>>,
    /// Box and whisker quartiles by the inclusive method (QUARTILE.INC) instead of exclusive.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub inclusive_quartiles: bool,
    /// A stock chart's up/down bars (`c:upDownBars`): open-high-low-close, the box between
    /// the first and last series, hollow when the price rose.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub up_down_bars: bool,
    /// A histogram's bin count (`cx:binCount`); with neither this nor `bin_size` the bins are
    /// automatic (Scott's rule, as Excel does).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bin_count: Option<u32>,
    /// A histogram's bin width (`cx:binSize`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bin_size: Option<f64>,
    /// Original chart XML part kept for round-trip of unmodelled options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    /// The chart was too large to model: `raw` and `kept` are written back unchanged on save.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub verbatim: bool,
    /// Package parts the original chart still points at (embedded workbook, styles, drawings).
    /// The PPTX writer clears this before comparing modelled state. Native saves keep it too.
    #[serde(skip_serializing_if = "ChartKept::is_empty")]
    pub kept: ChartKept,
}

/// One legend entry's own text (`c:legendEntry/c:txPr`): what it sets wins over the legend's.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LegendEntry {
    /// The entry's place in the legend, from 0 (`c:idx`).
    pub idx: u32,
    /// Text size in points.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorRef>,
}

/// One relationship of a chart part, with the target string as it was written.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartRel {
    pub id: String,
    pub rel_type: String,
    pub target: String,
    /// Absolute name of the part an internal relationship reaches (matches a `ChartPart::name`
    /// up to case); empty for external ones. The writer renames kept parts and uses this to
    /// point the relationship at the new name.
    pub part: String,
    pub external: bool,
}

/// A part reached from a chart (the workbook, a drawing, an image it uses).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartPart {
    /// Absolute part name (`ppt/embeddings/Workbook.xlsx`).
    pub name: String,
    pub content_type: String,
    /// Runtime bytes; older native files stored these inline in the manifest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bytes: Vec<u8>,
    /// Native ZIP member while encoding/decoding. The native reader consumes this reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_entry: Option<String>,
}

/// Sidecar for an unchanged chart: relationships and the parts they reach.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChartKept {
    pub rels: Vec<ChartRel>,
    pub parts: Vec<ChartPart>,
}

impl ChartKept {
    fn is_empty(&self) -> bool {
        self.rels.is_empty() && self.parts.is_empty()
    }
}

impl Default for Chart {
    fn default() -> Self {
        Chart {
            kind: ChartType::Column,
            title: Some("Chart Title".into()),
            categories: vec![],
            series: vec![],
            legend: Some("b".into()),
            legend_overlay: false,
            legend_entries: vec![],
            text_size: None,
            title_size: None,
            data_labels: false,
            gridlines: true,
            minor_gridlines: false,
            axis_titles: (None, None),
            style: 2,
            palette: "colorful1".into(),
            plot_fill: None,
            chart_fill: None,
            text_color: None,
            hole_size: 0.5,
            gap_width: 2.19,
            overlap: -0.27,
            vary_colors: false,
            y_min: None,
            y_max: None,
            y_major: None,
            y_log_base: None,
            y_format: None,
            y_hidden: false,
            cat_reversed: false,
            y2_min: None,
            y2_max: None,
            y2_major: None,
            y2_format: None,
            y2_hidden: false,
            lang: None,
            subtotals: vec![],
            category_levels: vec![],
            inclusive_quartiles: false,
            up_down_bars: false,
            bin_count: None,
            bin_size: None,
            raw: None,
            verbatim: false,
            kept: ChartKept::default(),
        }
    }
}

impl Chart {
    fn sample_treemap_sunburst(kind: ChartType) -> Self {
        let mut c = Chart {
            kind,
            categories: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug"].iter().map(ToString::to_string).collect(),
            category_levels: vec![["Q1", "Q1", "Q1", "Q2", "Q2", "Q2", "Q3", "Q3"].iter().map(ToString::to_string).collect()],
            vary_colors: true,
            ..Default::default()
        };
        let v = [4.2, 2.8, 3.6, 4.7, 2.6, 4.1, 1.9, 3.0];
        c.series.push(Series { name: "Series 1".into(), values: v.iter().map(|x| Some(*x)).collect(), ..Default::default() });
        c
    }

    fn sample_observation_chart(kind: ChartType) -> Self {
        let obs = [12.0, 15.0, 17.0, 18.0, 21.0, 22.0, 22.0, 24.0, 25.0, 27.0, 28.0, 31.0, 33.0, 36.0, 41.0, 48.0];
        let n_series = if kind == ChartType::BoxWhisker { 2 } else { 1 };
        let categories = if kind == ChartType::BoxWhisker {
            (0..obs.len()).map(|i| if i % 2 == 0 { "Group A" } else { "Group B" }.to_string()).collect()
        } else {
            vec![]
        };
        let mut c =
            Chart { kind, categories, gridlines: true, legend: (kind == ChartType::BoxWhisker).then(|| "b".to_string()), ..Default::default() };
        for s in 0..n_series {
            c.series.push(Series {
                name: format!("Series {}", s + 1),
                values: obs.iter().map(|v| Some(v + s as f64 * 6.0)).collect(),
                ..Default::default()
            });
        }
        c
    }

    fn sample_stock() -> Self {
        let mut c = Chart {
            kind: ChartType::Stock,
            categories: ["Mon", "Tue", "Wed", "Thu", "Fri"].iter().map(ToString::to_string).collect(),
            ..Default::default()
        };
        let hlc = [("High", [55.0, 57.0, 57.0, 58.0, 58.0]), ("Low", [11.0, 12.0, 13.0, 11.0, 25.0]), ("Close", [25.0, 38.0, 50.0, 35.0, 43.0])];
        for (name, v) in hlc {
            c.series.push(Series { name: name.into(), values: v.iter().map(|x| Some(*x)).collect(), ..Default::default() });
        }
        c
    }

    /// The sample data a new chart starts with (our own numbers).
    pub fn sample(kind: ChartType) -> Self {
        if matches!(kind, ChartType::Treemap | ChartType::Sunburst) {
            return Self::sample_treemap_sunburst(kind);
        }
        if matches!(kind, ChartType::Histogram | ChartType::Pareto | ChartType::BoxWhisker) {
            return Self::sample_observation_chart(kind);
        }
        if kind == ChartType::Stock {
            return Self::sample_stock();
        }
        let cats = ["Q1", "Q2", "Q3", "Q4"];
        let data = [[4.2, 2.6, 2.1], [2.8, 4.1, 2.4], [3.6, 1.9, 3.3], [4.7, 3.0, 5.1]];
        let mut c = Chart { kind, categories: cats.iter().map(ToString::to_string).collect(), ..Default::default() };
        let n_series =
            if matches!(kind, ChartType::Pie | ChartType::Doughnut | ChartType::Funnel | ChartType::Waterfall | ChartType::Treemap) { 1 } else { 3 };
        for s in 0..n_series {
            c.series.push(Series {
                name: format!("Series {}", s + 1),
                values: data.iter().map(|row| row.get(s).copied()).collect(),
                x_values: if kind == ChartType::Scatter { (1..=4).map(|x| Some(x as f64)).collect() } else { vec![] },
                ..Default::default()
            });
        }
        c.vary_colors = n_series == 1;
        if kind == ChartType::Combo {
            // Columns with the last series as a line, as PowerPoint's combo starts.
            for (i, s) in c.series.iter_mut().enumerate() {
                s.kind = Some(if i + 1 == n_series { ChartType::Line } else { ChartType::Column });
            }
        }
        c
    }
    /// A series' own chart type: its own in a combo chart (columns when it names none), else the
    /// chart's.
    pub fn series_kind(&self, s: &Series) -> ChartType {
        match (self.kind, s.kind) {
            (ChartType::Combo, Some(k)) if k != ChartType::Combo => k,
            (ChartType::Combo, _) => ChartType::Column,
            (k, _) => k,
        }
    }

    /// Whether a series is plotted against the secondary value axis (combo charts only).
    pub fn on_secondary(&self, s: &Series) -> bool {
        self.kind == ChartType::Combo && s.secondary_axis
    }

    /// The value range of one value axis: the primary axis (`y_min`/`y_max` apply), or a combo
    /// chart's secondary axis. A combo chart stacks only its stacked series. `None` when no series
    /// uses that axis.
    pub fn axis_range(&self, secondary: bool) -> Option<(f64, f64)> {
        if self.kind != ChartType::Combo {
            return (!secondary).then(|| self.value_range());
        }
        let (lo, hi) = self.plotted_extent(secondary)?;
        let (lo, hi) = (lo.min(0.0), hi.max(0.0));
        Some(if secondary { (lo, hi) } else { (self.y_min.unwrap_or(lo), self.y_max.unwrap_or(hi)) })
    }

    /// The smallest and largest value plotted against one value axis: plain values as they are,
    /// stacked series as running sums from zero, 100 % series as 0–1. `(0, 0)` without finite
    /// values; `None` when no series uses the axis.
    fn plotted_extent(&self, secondary: bool) -> Option<(f64, f64)> {
        if self.kind == ChartType::Waterfall {
            return (!secondary).then(|| self.value_range());
        }
        let ser: Vec<&Series> = self.series.iter().filter(|s| self.on_secondary(s) == secondary).collect();
        if ser.is_empty() {
            return (!secondary && self.kind != ChartType::Combo).then_some((0.0, 0.0));
        }
        let mut ext = (f64::INFINITY, f64::NEG_INFINITY);
        let has_stacked_or_percent = ser.iter().any(|s| {
            let k = self.series_kind(s);
            k.stacked() || k.percent()
        });
        let percent = if has_stacked_or_percent {
            self.fold_stacked(&ser, &mut ext)
        } else {
            for s in &ser {
                for v in s.values.iter().flatten().copied().filter(|v| v.is_finite()) {
                    widen(&mut ext, v);
                }
            }
            false
        };
        if !percent {
            self.fold_trendlines(&ser, &mut ext);
        }
        if percent {
            ext.0 = ext.0.min(0.0);
            ext.1 = ext.1.max(1.0);
        }
        let (lo, hi) = ext;
        Some(if lo <= hi { (lo, hi) } else { (0.0, 0.0) })
    }

    /// Widens `ext` by the points of the stacked and 100 % series in `ser`; returns whether a
    /// 100 % series is among them.
    fn fold_stacked(&self, ser: &[&Series], ext: &mut (f64, f64)) -> bool {
        let mut percent = false;
        let n = ser.iter().map(|s| s.values.len()).max().unwrap_or(0);
        let budget = (ser.len() * n).min(10_000_000);
        let n_clamped = if ser.is_empty() { 0 } else { n.min(budget / ser.len()) };
        for i in 0..n_clamped {
            let (mut p, mut m, mut stacked) = (0.0, 0.0, false);
            for s in ser {
                let k = self.series_kind(s);
                let Some(v) = s.values.get(i).copied().flatten().filter(|v| v.is_finite()) else { continue };
                if k.percent() {
                    percent = true;
                } else if k.stacked() {
                    stacked = true;
                    if v >= 0.0 { p += v } else { m += v }
                } else {
                    widen(ext, v);
                }
            }
            if stacked {
                ext.1 = ext.1.max(p);
                ext.0 = ext.0.min(m);
            }
        }
        percent
    }

    /// Widens `ext` by the trendlines of the series in `ser` that are not stacked.
    fn fold_trendlines(&self, ser: &[&Series], ext: &mut (f64, f64)) {
        for s in ser {
            if self.series_kind(s).stacked() {
                continue;
            }
            for y in s.trendline_ys(crate::trendline::series_is_scatter(self.series_kind(s))) {
                widen(ext, y);
            }
        }
    }

    /// A waterfall's bars as (from, to) on the value axis: each point floats from the running
    /// total, a total point stands on zero and sets it.
    pub fn waterfall_bars(&self) -> Vec<(f64, f64)> {
        let Some(s) = self.series.first() else { return vec![] };
        let mut run = 0.0;
        s.values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let v = v.unwrap_or(0.0);
                if self.subtotals.contains(&i) {
                    run = v;
                    (0.0, v)
                } else {
                    let from = run;
                    run += v;
                    (from, run)
                }
            })
            .collect()
    }

    /// A value axis' scale — (min, max, major unit) — with at most `max_intervals` intervals:
    /// the bounds and unit the chart fixes, else Office's automatic ones ([`axis_scale`](fn@axis_scale)).
    /// `None` when no series uses the axis.
    pub fn axis_scale(&self, secondary: bool, max_intervals: usize) -> Option<(f64, f64, f64)> {
        let (lo, hi) = self.plotted_extent(secondary)?;
        let (fmin, fmax, major) = if secondary { (self.y2_min, self.y2_max, self.y2_major) } else { (self.y_min, self.y_max, self.y_major) };
        let full_percent = !secondary && self.kind.percent();
        Some(axis_scale(lo, hi, fmin, fmax, major, max_intervals, !full_percent))
    }

    /// A logarithmic value axis' scale in exponents — (min, max, unit), so its labels are
    /// `base^min`, `base^(min+unit)`… — covering the positive values: whole powers, as PowerPoint
    /// and LibreOffice draw them. `None` when the axis isn't logarithmic or nothing is positive.
    pub fn log_axis_scale(&self, max_intervals: usize) -> Option<(f64, f64, f64)> {
        let b = self.y_log_base.filter(|b| b.is_finite() && *b >= 2.0)?;
        let vals = self.series.iter().filter(|s| !self.on_secondary(s)).flat_map(|s| s.values.iter().flatten().copied());
        let (lo, hi) = vals
            .filter(|v| v.is_finite() && *v > 0.0)
            .fold(None, |r: Option<(f64, f64)>, v| Some(r.map_or((v, v), |(a, z)| (a.min(v), z.max(v)))))?;
        let e = |v: f64| v.log(b);
        let lo = self.y_min.filter(|v| *v > 0.0).map_or(e(lo).floor(), e);
        let hi = self.y_max.filter(|v| *v > 0.0).map_or(e(hi).ceil(), e).max(lo + 1.0);
        let unit = ((hi - lo) / max_intervals.max(1) as f64).ceil().max(1.0);
        Some((lo, lo + ((hi - lo) / unit).ceil() * unit, unit))
    }

    /// A value axis label in the axis' number format and the chart's language.
    pub fn axis_label(&self, secondary: bool, v: f64) -> String {
        let code = if secondary { &self.y2_format } else { &self.y_format };
        // A 100 % stacked chart's value axis counts in percent unless the file says otherwise.
        let code = match code.as_deref() {
            None | Some("General") if !secondary && self.kind.percent() => "0%",
            c => c.unwrap_or("General"),
        };
        crate::numfmt::format(code, v, self.lang.as_deref())
    }

    /// A data label of series `s`: its number format, else its axis', in the chart's language.
    pub fn data_label(&self, s: &Series, v: f64) -> String {
        let axis = if self.on_secondary(s) { &self.y2_format } else { &self.y_format };
        let code = s.number_format.as_ref().or(axis.as_ref());
        crate::numfmt::format(code.map_or("General", String::as_str), v, self.lang.as_deref())
    }

    /// Does series `s` show its data labels? The series' own `c:dLbls/c:showVal` wins — an
    /// explicit `false` hides them even when the chart-wide setting is on (#868).
    pub fn series_data_labels(&self, s: &Series) -> bool {
        s.data_labels.unwrap_or(self.data_labels)
    }

    fn waterfall_value_range(&self) -> (f64, f64) {
        let mut lo: f64 = 0.0;
        let mut hi: f64 = 0.0;
        for (a, b) in self.waterfall_bars() {
            lo = lo.min(a.min(b));
            hi = hi.max(a.max(b));
        }
        (lo, hi)
    }

    fn stacked_value_range(&self, n: usize) -> (f64, f64) {
        // Sums per category, from the values the series hold (an absent one adds nothing).
        let (mut pos, mut neg) = (vec![0.0f64; n], vec![0.0f64; n]);
        for s in &self.series {
            for (i, v) in s.values.iter().enumerate().take(n) {
                let v = v.unwrap_or(0.0);
                if v >= 0.0 {
                    pos[i] += v;
                } else {
                    neg[i] += v;
                }
            }
        }
        let hi = pos.iter().fold(0.0f64, |a, &p| a.max(p));
        let lo = neg.iter().fold(0.0f64, |a, &m| a.min(m));
        (lo, hi)
    }

    /// Value range across series (stacked sums for stacked types).
    pub fn value_range(&self) -> (f64, f64) {
        if self.kind == ChartType::Waterfall {
            return self.waterfall_value_range();
        }
        let n = self.categories.len().max(self.series.iter().map(|s| s.values.len()).max().unwrap_or(0));
        let (mut lo, mut hi) = if self.kind.stacked() {
            self.stacked_value_range(n)
        } else {
            let mut l: f64 = 0.0;
            let mut h: f64 = 0.0;
            for s in &self.series {
                for v in s.values.iter().flatten() {
                    if v.is_finite() {
                        h = h.max(*v);
                        l = l.min(*v);
                    }
                }
            }
            (l, h)
        };
        if self.kind.percent() {
            return (0.0, 1.0);
        }
        if !self.kind.stacked() {
            for s in &self.series {
                for y in s.trendline_ys(crate::trendline::series_is_scatter(self.series_kind(s))) {
                    hi = hi.max(y);
                    lo = lo.min(y);
                }
            }
        }
        (self.y_min.unwrap_or(lo), self.y_max.unwrap_or(hi))
    }
}

/// The (min, max, major unit) of an axis over the data range [lo, hi]: `fmin`/`fmax`/`major`
/// where the chart fixes them; otherwise Office's automatic limits, rounded out to the smallest
/// 1-2-5 unit giving at most `max_intervals` intervals:
/// The data range [lo, hi] an axis covers, clamped to ±1e100; (0, 1) without finite values.
///
/// - **Maximum:** `hi + (hi − lo)/20` (with `headroom`), rounded up to the unit.
/// - **Minimum:** zero when every value is ≥ 0 and `lo` is below 5/6 of `hi`; otherwise
///   `lo − (hi − lo)/20`, rounded down to the unit, so values bunched high get an axis that starts
///   near them.
/// - All-negative values mirror this: the axis ends at zero when `hi` is above 5/6 of `lo`.
///   Mixed signs get headroom at both ends.
fn axis_data_range(lo: f64, hi: f64) -> (f64, f64) {
    if lo.is_finite() && hi.is_finite() { (lo.min(hi).clamp(-1e100, 1e100), lo.max(hi).clamp(-1e100, 1e100)) } else { (0.0, 1.0) }
}

/// Office's automatic (min, max) of the data range [lo, hi]. "How chart axis limits are determined",
/// as Jon Peltier summarises it: 5 % of the data range past the data, and zero when the values don't
/// sit within the top sixth of their largest magnitude. A single value is measured from zero.
fn auto_limits(lo: f64, hi: f64, headroom: bool) -> (f64, f64) {
    let flat = hi - lo <= 1e-12 * lo.abs().max(hi.abs());
    let pad = if !headroom {
        0.0
    } else if flat {
        hi.abs() / 20.0
    } else {
        (hi - lo) / 20.0
    };
    let auto_lo = if lo >= 0.0 && (flat || lo < hi * 5.0 / 6.0) { 0.0 } else { lo - pad };
    let auto_hi = if hi <= 0.0 && (flat || hi > lo * 5.0 / 6.0) { 0.0 } else { hi + pad };
    (auto_lo, auto_hi)
}

/// The ends of a span too small to show: with one fixed limit on the far side of the data, the
/// automatic end gives way; otherwise the span is one `unit` wide.
fn flat_span_ends(fmin: Option<f64>, fmax: Option<f64>, lo: f64, hi: f64, unit: f64) -> (f64, f64) {
    match (fmin, fmax) {
        (None, Some(_)) => (if hi > 0.0 { 0.0 } else { hi - hi.abs().max(unit) }, hi),
        (Some(_), None) if lo < 0.0 => (lo, 0.0),
        _ => (lo, lo + unit),
    }
}

/// The major step: `major` when it gives at most 1000 intervals, else the smallest 1-2-5 step
/// that fits `n` intervals over [lo, hi].
fn axis_step(major: Option<f64>, lo: f64, hi: f64, n: f64, unit: f64) -> f64 {
    match major.filter(|m| *m > 0.0 && (hi - lo) / m <= 1000.0) {
        Some(m) => m,
        None => {
            let mut step = 10f64.powf(((hi - lo) / n).log10().floor());
            // Subnormal spans underflow to a zero (or non-finite) step: fall back to the unit.
            if !(step > 0.0 && step.is_finite()) {
                step = unit;
            }
            let mut k = 0;
            // 1, 2, 5, 10, 20… × the magnitude until the intervals fit.
            while ((hi / step).ceil() - (lo / step).floor()) > n && k < 30 {
                step *= if matches!(k % 3, 1) { 2.5 } else { 2.0 };
                k += 1;
            }
            step
        }
    }
}

pub fn axis_scale(
    lo: f64,
    hi: f64,
    fmin: Option<f64>,
    fmax: Option<f64>,
    major: Option<f64>,
    max_intervals: usize,
    headroom: bool,
) -> (f64, f64, f64) {
    // Values past ±1e100 aren't axis values; clamping keeps the arithmetic finite.
    let fin = |v: Option<f64>| v.filter(|v| v.is_finite()).map(|v| v.clamp(-1e100, 1e100));
    let (fmin, fmax) = (fin(fmin), fin(fmax));
    let (lo, hi) = axis_data_range(lo, hi);
    let (auto_lo, auto_hi) = auto_limits(lo, hi, headroom);
    let (mut lo, mut hi) = (fmin.unwrap_or(auto_lo), fmax.unwrap_or(auto_hi));
    let mag = lo.abs().max(hi.abs());
    // Relative to the data's magnitude, so tiny values don't count as flat; all-zero falls back to 1.
    let unit = if mag == 0.0 { 1.0 } else { mag };
    if hi - lo <= 1e-12 * mag {
        (lo, hi) = flat_span_ends(fmin, fmax, lo, hi, unit);
    }
    let n = max_intervals.clamp(1, 50) as f64;
    let step = axis_step(fin(major), lo, hi, n, unit);
    let min = fmin.unwrap_or((lo / step + 1e-9).floor() * step);
    let max = fmax.unwrap_or((hi / step - 1e-9).ceil() * step).max(min + step);
    (min, max, step)
}

/// "Nice" axis scale: (min, max, major step) covering [lo, hi].
pub fn nice_scale(lo: f64, hi: f64) -> (f64, f64, f64) {
    let (lo, hi) = if lo.is_finite() && hi.is_finite() { (lo.min(hi), lo.max(hi)) } else { (0.0, 1.0) };
    let data_mag = lo.abs().max(hi.abs());
    let span = if hi - lo <= 1e-12 * data_mag { data_mag } else { hi - lo };
    let span = if span == 0.0 { 1.0 } else { span };
    let raw = span / 5.0;
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag;
    let step = if norm <= 1.0 {
        1.0
    } else if norm <= 2.0 {
        2.0
    } else if norm <= 2.5 {
        2.5
    } else if norm <= 5.0 {
        5.0
    } else {
        10.0
    } * mag;
    // Subnormal spans underflow to a zero or NaN step: use the span itself.
    let step = if step > 0.0 && step.is_finite() { step } else { span };
    let min = (lo / step).floor() * step;
    let max = (hi / step).ceil() * step;
    let max = if (max - min).abs() <= 1e-12 * data_mag { min + step } else { max };
    (min, max, step)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiny_data_keeps_its_own_scale() {
        let (min, max, step) = axis_scale(1e-13, 5e-13, None, None, None, 8, true);
        assert!(min == 0.0 && (5e-13..1e-11).contains(&max), "{min} {max} {step}");
        assert!(step > 0.0 && step < 1e-11);
        let (min, max, step) = nice_scale(1e-13, 5e-13);
        assert!(min <= 1e-13 && (5e-13..1e-11).contains(&max) && step < 1e-11, "{min} {max} {step}");
        assert_eq!(axis_scale(0.0, 0.0, None, None, None, 8, true).1, 1.0);
    }

    #[test]
    fn subnormal_data_gives_a_finite_scale() {
        let (min, max, step) = axis_scale(0.0, 5e-324, None, None, None, 8, true);
        assert!(min.is_finite() && max.is_finite() && step.is_finite(), "{min} {max} {step}");
        assert!(step > 0.0 && max >= min, "{min} {max} {step}");
        let (min, max, step) = nice_scale(0.0, f64::from_bits(1));
        assert!(min.is_finite() && max.is_finite() && step > 0.0 && step.is_finite() && max > min, "{min} {max} {step}");
    }

    #[test]
    fn log_axes_run_over_whole_powers() {
        let mut c = Chart::sample(ChartType::Line);
        c.series.truncate(1);
        c.series[0].values = vec![Some(1.0), Some(10.0), Some(1000.0), Some(50000.0)];
        assert_eq!(c.log_axis_scale(8), None);
        c.y_log_base = Some(10.0);
        assert_eq!(c.log_axis_scale(8), Some((0.0, 5.0, 1.0)));
        assert_eq!(c.log_axis_scale(2), Some((0.0, 6.0, 3.0)));
    }

    /// #868: a series' own label setting wins; an unset series takes the chart-wide one.
    #[test]
    fn series_data_labels_override_the_chart_wide_setting() {
        let mut c = Chart::sample(ChartType::Column);
        c.data_labels = false;
        let mut s = c.series[0].clone();
        assert!(!c.series_data_labels(&s), "unset series follows the chart");
        s.data_labels = Some(true);
        assert!(c.series_data_labels(&s), "a series that asks for labels gets them");
        c.data_labels = true;
        s.data_labels = Some(false);
        assert!(!c.series_data_labels(&s), "an explicit series false hides them anyway");
    }

    #[test]
    fn error_bar_ranges() {
        let vals = [Some(3.0), Some(5.0), Some(2.0), Some(4.0)];
        let mut e = ErrorBars::default();
        assert_eq!(e.range(&vals, 0, 3.0), Some((2.0, 4.0)));
        e.val_type = "percentage".into();
        e.value = 20.0;
        assert_eq!(e.range(&vals, 1, 5.0), Some((4.0, 6.0)));
        e.bar_type = "plus".into();
        assert_eq!(e.range(&vals, 1, 5.0), Some((5.0, 6.0)));
        e = ErrorBars { val_type: "cust".into(), plus: vec![Some(0.5)], minus: vec![Some(1.5)], ..Default::default() };
        assert_eq!(e.range(&vals, 0, 3.0), Some((1.5, 3.5)));
        e = ErrorBars { val_type: "stdDev".into(), ..Default::default() };
        let (lo, hi) = e.range(&vals, 0, 3.0).unwrap();
        assert!((lo + hi - 7.0).abs() < 1e-9 && (hi - lo - 2.0 * 1.2909944487358056).abs() < 1e-9);
    }

    #[test]
    fn nice_scale_covers() {
        let (a, b, s) = nice_scale(0.0, 5.1);
        assert!(a <= 0.0 && b >= 5.1 && s > 0.0);
        let (a, b, _) = nice_scale(f64::NAN, 3.0);
        assert!(a.is_finite() && b.is_finite());
        let (a, b, _) = nice_scale(7.0, 7.0);
        assert!(b > a);
    }

    #[test]
    fn automatic_axis_scales_like_powerpoint() {
        // Weekly spend up to ~715 000 on a plot with room for 9 intervals: 0–800 000 by 100 000.
        assert_eq!(axis_scale(0.0, 715_000.0, None, None, None, 9, true), (0.0, 800_000.0, 100_000.0));
        // Clicks up to 6 600: 0–7 000 by 1 000.
        assert_eq!(axis_scale(0.0, 6_600.0, None, None, None, 9, true), (0.0, 7_000.0, 1_000.0));
        // Fewer intervals fit on a short axis.
        assert_eq!(axis_scale(0.0, 6_600.0, None, None, None, 4, true), (0.0, 8_000.0, 2_000.0));
        // Negative data gets room below; fixed bounds and unit are kept.
        let (min, max, _) = axis_scale(-30.0, 100.0, None, None, None, 8, true);
        assert!(min <= -30.0 && max >= 105.0);
        assert_eq!(axis_scale(0.0, 715_000.0, Some(100_000.0), Some(750_000.0), Some(50_000.0), 9, true), (100_000.0, 750_000.0, 50_000.0));
        // 100 % stacked stays at 0–1 without headroom.
        assert_eq!(axis_scale(0.0, 1.0, None, None, None, 5, false), (0.0, 1.0, 0.2));
        // Nonsense never loops or divides by zero.
        for (lo, hi) in [(f64::NAN, 1.0), (5.0, 5.0), (0.0, 0.0), (-1e300, 1e300)] {
            let (a, b, s) = axis_scale(lo, hi, None, None, Some(0.0), 0, true);
            assert!(a.is_finite() && b.is_finite() && s > 0.0 && b > a, "{lo} {hi}: {a} {b} {s}");
        }
    }

    fn column(values: &[f64]) -> Chart {
        let series = Series { name: "Sales".into(), values: values.iter().map(|v| Some(*v)).collect(), ..Default::default() };
        Chart { categories: (0..values.len()).map(|i| format!("C{i}")).collect(), series: vec![series], ..Default::default() }
    }

    /// The primary axis of a chart with room for ten intervals.
    fn scale(c: &Chart) -> (f64, f64, f64) {
        c.axis_scale(false, 10).unwrap_or_default()
    }

    /// Office's documented automatic limits (#144), with room for ten intervals.
    #[test]
    fn automatic_axis_matches_office() {
        // #144: 42.1 / 18.7 / 6.3 → 0–45 in steps of 5, not 0–50 in steps of 10.
        assert_eq!(scale(&column(&[42.1, 18.7, 6.3])), (0.0, 45.0, 5.0));
        // PowerPoint's default chart data (max 5): 0–6 in steps of 1.
        let mut c = column(&[4.3, 2.5, 3.5, 4.5]);
        c.series.push(Series { values: [2.4, 4.4, 1.8, 2.8].map(Some).to_vec(), ..Default::default() });
        c.series.push(Series { values: [2.0, 2.0, 3.0, 5.0].map(Some).to_vec(), ..Default::default() });
        assert_eq!(scale(&c), (0.0, 6.0, 1.0));
        // A maximum of 100 (or 10) gets 5 % headroom, which rounds up to the next major unit.
        assert_eq!(scale(&column(&[100.0, 40.0])), (0.0, 120.0, 20.0));
        assert_eq!(scale(&column(&[10.0, 3.0])), (0.0, 12.0, 2.0));
        // Peltier's example: 2.95 + 2.95/20 ≈ 3.1 rounds up to 3.5.
        assert_eq!(scale(&column(&[2.95, 0.0])), (0.0, 3.5, 0.5));
    }

    #[test]
    fn automatic_axis_minimum() {
        // Below 5/6 of the maximum the axis starts at zero; at or above it, a unit below
        // min − (max − min)/20.
        assert_eq!(scale(&column(&[59.0, 49.0])).0, 0.0);
        assert_eq!(scale(&column(&[59.0, 50.0])), (48.0, 60.0, 2.0));
        assert_eq!(scale(&column(&[85.0, 92.0, 97.0])), (84.0, 98.0, 2.0));
        // All-negative data mirrors it: the axis ends at zero, or near the data.
        assert_eq!(scale(&column(&[-42.1, -18.7, -6.3])), (-45.0, 0.0, 5.0));
        assert_eq!(scale(&column(&[-59.0, -49.0])).1, 0.0);
        assert_eq!(scale(&column(&[-59.0, -50.0])), (-60.0, -48.0, 2.0));
        // Mixed signs get headroom at both ends.
        assert_eq!(scale(&column(&[42.1, -18.7])), (-30.0, 50.0, 10.0));
        // Stacked bars build up from zero, so their axis keeps it.
        let mut s = column(&[85.0, 92.0]);
        s.kind = ChartType::StackedColumn;
        assert_eq!(scale(&s).0, 0.0);
        // A combo chart's secondary axis follows the same rule.
        let mut c = Chart::sample(ChartType::Combo);
        c.series.truncate(2);
        c.series[0].values = vec![Some(200_000.0), Some(310_000.0)];
        c.series[1].kind = Some(ChartType::Line);
        c.series[1].values = vec![Some(7_000.0), Some(7_400.0)];
        c.series[1].secondary_axis = true;
        assert_eq!(c.axis_scale(false, 10), Some((0.0, 350_000.0, 50_000.0)));
        assert_eq!(c.axis_scale(true, 10), Some((6_950.0, 7_450.0, 50.0)));
    }

    #[test]
    fn fixed_axis_limits_are_kept() {
        let mut c = column(&[42.1, 18.7, 6.3]);
        c.y_max = Some(50.0);
        assert_eq!(scale(&c), (0.0, 50.0, 5.0));
        c.y_min = Some(10.0);
        c.y_max = None;
        assert_eq!(scale(&c), (10.0, 45.0, 5.0));
        // A fixed limit beyond the data: the automatic end gives way to it.
        let mut c = column(&[85.0, 92.0, 97.0]);
        c.y_max = Some(50.0);
        assert_eq!(scale(&c), (0.0, 50.0, 5.0));
        let mut c = column(&[-50.0, -48.0]);
        c.y_min = Some(-10.0);
        assert_eq!(scale(&c), (-10.0, 0.0, 1.0));
    }

    #[test]
    fn percent_axis_is_zero_to_one() {
        let mut p = column(&[42.1, 18.7, 6.3]);
        p.kind = ChartType::PercentColumn;
        assert_eq!(scale(&p), (0.0, 1.0, 0.1));
        assert_eq!(p.axis_scale(false, 5), Some((0.0, 1.0, 0.2)));
    }

    #[test]
    fn degenerate_axes_are_finite() {
        for (a, b) in [(0.0, 0.0), (7.0, 7.0), (-7.0, -7.0), (f64::NAN, 3.0), (f64::INFINITY, 1.0), (-f64::MAX, f64::MAX), (0.0, 1e-320)] {
            let (min, max, step) = scale(&column(&[a, b]));
            assert!(min.is_finite() && max > min && step > 0.0, "{a} {b}: {min} {max} {step}");
        }
        let (min, max, _) = scale(&column(&[7.0, 7.0]));
        assert!(min == 0.0 && max > 7.0);
        assert_eq!(scale(&column(&[])), (0.0, 1.0, 0.1));
        for (fmin, fmax) in [(Some(5.0), Some(5.0)), (Some(5.0), Some(1.0)), (Some(f64::NAN), Some(f64::INFINITY)), (Some(-1e300), None)] {
            let (a, b, s) = axis_scale(0.0, 1.0, fmin, fmax, None, 10, true);
            assert!(a.is_finite() && b.is_finite() && b > a && s > 0.0, "{fmin:?} {fmax:?}: {a} {b} {s}");
        }
    }

    #[test]
    fn general_labels_keep_every_digit_the_data_has() {
        // #324: labels were rounded to one decimal ("100000.0" for 99999.99).
        for kind in [ChartType::Column, ChartType::Pie, ChartType::Doughnut, ChartType::Line, ChartType::Scatter] {
            let c = Chart::sample(kind);
            let s = &c.series[0];
            for (v, want) in
                [(1220.82, "1220.82"), (99999.99, "99999.99"), (1500.05, "1500.05"), (1500.0, "1500"), (1234.567, "1234.567"), (999.99, "999.99")]
            {
                assert_eq!(c.data_label(s, v), want, "{kind:?}");
            }
        }
        // Small ranges keep distinct value-axis ticks (0.123 used to give 0, 0.0, 0.1, 0.1…).
        let c = Chart::sample(ChartType::Column);
        let ticks: Vec<String> = (0..=5).map(|i| c.axis_label(false, f64::from(i) * 0.025)).collect();
        assert_eq!(ticks, ["0", "0.025", "0.05", "0.075", "0.1", "0.125"]);
    }

    #[test]
    fn a_forecast_grows_the_value_axis() {
        let mut c = Chart::sample(ChartType::Line);
        c.series.truncate(1);
        c.series[0].values = vec![Some(3.0), Some(5.0), Some(7.0), Some(9.0)];
        let before = c.value_range().1;
        c.series[0].trendlines.push(Trendline { forward: Some(1.0), ..Default::default() });
        let after = c.value_range().1;
        assert!(after > before && after >= 11.0 - 1e-6, "{before} {after}");
    }

    #[test]
    fn axis_labels_use_the_format_and_language() {
        let mut c = Chart::sample(ChartType::Column);
        c.y_format = Some(r#"_-* #\ ##0\ "₽"_-;\-* #\ ##0\ "₽"_-;_-* "-"??\ "₽"_-;_-@_-"#.into());
        c.lang = Some("ru-RU".into());
        assert_eq!(c.axis_label(false, 800_000.0), "800\u{a0}000 ₽");
        assert_eq!(c.axis_label(false, 0.0), "- ₽");
        // No format: General.
        assert_eq!(c.axis_label(true, 2.5), "2,5");
        c.lang = None;
        assert_eq!(c.axis_label(true, 2.5), "2.5");
        // Data labels: the series' format, else the axis'.
        c.lang = Some("ru-RU".into());
        c.series[0].number_format = Some(r#"#,##0\ "₽""#.into());
        assert_eq!(c.data_label(&c.series[0], 516_600_000.0), "516\u{a0}600\u{a0}000 ₽");
        assert_eq!(c.data_label(&c.series[1], 0.0), "- ₽");
    }

    #[test]
    fn full_percent_axes_count_in_percent() {
        let mut c = Chart::sample(ChartType::PercentColumn);
        assert_eq!(c.axis_label(false, 0.5), "50%");
        c.y_format = Some("0.0%".into());
        assert_eq!(c.axis_label(false, 0.5), "50.0%");
        let c = Chart::sample(ChartType::Column);
        assert_eq!(c.axis_label(false, 0.5), "0.5");
    }

    #[test]
    fn combo_axis_ranges() {
        // Spend in columns on the primary axis, clicks as a line on the secondary one.
        let mut c = Chart::sample(ChartType::Combo);
        c.series.truncate(2);
        c.series[0].kind = Some(ChartType::Column);
        c.series[0].values = vec![Some(200_000.0), Some(310_000.0)];
        c.series[1].kind = Some(ChartType::Line);
        c.series[1].values = vec![Some(6_000.0), Some(7_400.0)];
        c.series[1].secondary_axis = true;
        assert_eq!(c.axis_range(false), Some((0.0, 310_000.0)));
        assert_eq!(c.axis_range(true), Some((0.0, 7_400.0)));
        assert_eq!(c.series_kind(&c.series[1]), ChartType::Line);
        // Without a secondary series there is no secondary axis; a non-combo chart never has one.
        c.series[1].secondary_axis = false;
        assert_eq!(c.axis_range(true), None);
        let col = Chart::sample(ChartType::Column);
        assert_eq!(col.axis_range(false), Some(col.value_range()));
        assert_eq!(col.axis_range(true), None);
        assert!(!col.on_secondary(&col.series[0]));
    }

    #[test]
    fn sample_ranges() {
        let c = Chart::sample(ChartType::StackedColumn);
        let (_, hi) = c.value_range();
        assert!(hi > 10.0);
        assert_eq!(Chart::sample(ChartType::Pie).series.len(), 1);
    }
}

/// One histogram bin: `[lo, hi]` (the first bin is closed on both sides, later ones open on the
/// left, as Excel bins) and how many observations fall in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bin {
    pub lo: f64,
    pub hi: f64,
    pub count: usize,
}

/// Bins a histogram has at most (a hostile bin size cannot make millions).
pub const MAX_BINS: usize = 1000;

/// A box and whisker summary of one group: quartiles by Excel's exclusive method, whiskers at
/// the furthest points within 1.5 IQR of the box, and the points beyond them.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxStats {
    pub low: f64,
    pub q1: f64,
    pub median: f64,
    pub q3: f64,
    pub high: f64,
    pub mean: f64,
    pub outliers: Vec<f64>,
}

impl Chart {
    /// The first series' finite observations.
    fn observations(&self) -> Vec<f64> {
        self.series.first().map(|s| s.values.iter().flatten().copied().filter(|v| v.is_finite()).collect()).unwrap_or_default()
    }

    /// The histogram's bins over the first series: by `bin_size`, else `bin_count`, else
    /// Scott's rule (3.49·σ·n^(-1/3)). Empty without observations.
    pub fn histogram_bins(&self) -> Vec<Bin> {
        let mut v = self.observations();
        if v.is_empty() {
            return vec![];
        }
        v.sort_by(f64::total_cmp);
        let (lo, hi) = (v[0], v[v.len() - 1]);
        let span = hi - lo;
        let n = v.len() as f64;
        let width = if let Some(w) = self.bin_size.filter(|w| w.is_finite() && *w > 0.0) {
            w
        } else if let Some(k) = self.bin_count.filter(|k| *k > 0) {
            span / f64::from(k)
        } else {
            let mean = v.iter().sum::<f64>() / n;
            let sd = (v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0).max(1.0)).sqrt();
            3.49 * sd / n.cbrt()
        };
        let fixed = self.bin_count.filter(|k| *k > 0).filter(|_| self.bin_size.filter(|w| w.is_finite() && *w > 0.0).is_none());
        let count = if span <= 0.0 || !width.is_finite() || width <= 0.0 {
            1
        } else if let Some(k) = fixed {
            (k as usize).clamp(1, MAX_BINS)
        } else {
            ((span / width).ceil() as usize).clamp(1, MAX_BINS)
        };
        let width = if count == 1 && (span <= 0.0 || !width.is_finite() || width <= 0.0) { span.max(1.0) } else { width.max(span / MAX_BINS as f64) };
        let mut bins: Vec<Bin> = (0..count).map(|i| Bin { lo: lo + width * i as f64, hi: lo + width * (i + 1) as f64, count: 0 }).collect();
        for x in v {
            // (lo, hi] bins, the first one also holding the minimum.
            let mut i = (((x - lo) / width).ceil() as usize).saturating_sub(1).min(count - 1);
            // The division can round across an edge: settle on the bin whose stored bounds hold x.
            while i > 0 && x <= bins[i - 1].hi {
                i -= 1;
            }
            while i + 1 < count && x > bins[i].hi {
                i += 1;
            }
            if let Some(b) = bins.get_mut(i) {
                b.count += 1;
            }
        }
        bins
    }

    /// A histogram (or Pareto) as the chart that draws it: a column chart of the bin counts with
    /// touching columns, labelled by bin range. A Pareto sorts the bins by count and adds the
    /// cumulative percentage as a line on a 0–100 % secondary axis.
    pub fn histogram_view(&self) -> Chart {
        let mut bins = self.histogram_bins();
        let pareto = self.kind == ChartType::Pareto;
        if pareto {
            bins.sort_by_key(|b| std::cmp::Reverse(b.count));
        }
        let fmt = |v: f64| {
            let r = (v * 100.0).round() / 100.0;
            if (r - r.round()).abs() < 1e-9 { format!("{}", r.round() as i64) } else { format!("{r}") }
        };
        let first_lo = bins.iter().map(|b| b.lo).fold(f64::INFINITY, f64::min);
        let mut v = Chart {
            kind: if pareto { ChartType::Combo } else { ChartType::Column },
            categories: bins
                .iter()
                .map(|b| if b.lo <= first_lo { format!("[{}, {}]", fmt(b.lo), fmt(b.hi)) } else { format!("({}, {}]", fmt(b.lo), fmt(b.hi)) })
                .collect(),
            series: vec![],
            gap_width: 0.0,
            raw: None,
            verbatim: false,
            subtotals: vec![],
            ..self.clone()
        };
        let name = self.series.first().map(|s| s.name.clone()).unwrap_or_default();
        let base = self.series.first().cloned().unwrap_or_default();
        v.series.push(Series {
            name,
            values: bins.iter().map(|b| Some(b.count as f64)).collect(),
            kind: pareto.then_some(ChartType::Column),
            number_format: None,
            ..base
        });
        if pareto {
            let total = bins.iter().map(|b| b.count).sum::<usize>().max(1) as f64;
            let mut run = 0.0;
            v.series.push(Series {
                name: "Cumulative %".into(),
                values: bins
                    .iter()
                    .map(|b| {
                        run += b.count as f64;
                        Some(run / total)
                    })
                    .collect(),
                kind: Some(ChartType::Line),
                secondary_axis: true,
                data_labels: Some(false),
                ..Default::default()
            });
            v.y2_min = Some(0.0);
            v.y2_max = Some(1.0);
            v.y2_major = Some(0.2);
            v.y2_format = Some("0%".into());
        }
        v.vary_colors = false;
        v
    }

    /// The box chart's groups (distinct category labels in order of appearance; one unnamed
    /// group without categories) and, per series, each group's summary (None when it has no
    /// points).
    ///
    /// Each value is put in its group's bucket once (groups are found by name in a map), and the
    /// series x groups result is held to [`MAX_BOX_CELLS`]: groups past it are left out.
    pub fn box_groups(&self) -> (Vec<String>, Vec<Vec<Option<BoxStats>>>) {
        let max_groups = (MAX_BOX_CELLS / self.series.len().max(1)).max(1);
        let mut groups: Vec<String> = Vec::new();
        let mut index: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for c in &self.categories {
            if !index.contains_key(c.as_str()) && groups.len() < max_groups {
                index.insert(c, groups.len());
                groups.push(c.clone());
            }
        }
        if groups.is_empty() {
            groups.push(String::new());
            index.insert("", 0);
        }
        // A value past the categories belongs to the unnamed group, when there is one.
        let past_end = index.get("").copied();
        let stats = self
            .series
            .iter()
            .map(|s| {
                let mut buckets: Vec<Vec<f64>> = vec![Vec::new(); groups.len()];
                for (i, v) in s.values.iter().enumerate() {
                    let Some(v) = v.filter(|v| v.is_finite()) else { continue };
                    let g = match self.categories.get(i) {
                        Some(c) => index.get(c.as_str()).copied(),
                        None => past_end,
                    };
                    if let Some(b) = g.and_then(|g| buckets.get_mut(g)) {
                        b.push(v);
                    }
                }
                buckets.into_iter().map(|pts| box_stats_by(pts, self.inclusive_quartiles)).collect()
            })
            .collect();
        (groups, stats)
    }
}

/// Most series x group summaries [`Chart::box_groups`] makes.
pub const MAX_BOX_CELLS: usize = 1_000_000;

/// Widens the (min, max) extent `ext` to include `v`.
fn widen(ext: &mut (f64, f64), v: f64) {
    ext.0 = ext.0.min(v);
    ext.1 = ext.1.max(v);
}

/// Excel's QUARTILE.EXC position `p·(n+1)` (1-based) on sorted data, clamped to the ends.
fn quantile_exc(v: &[f64], p: f64) -> f64 {
    let n = v.len();
    let pos = (p * (n as f64 + 1.0)).clamp(1.0, n as f64);
    let i = pos.floor() as usize;
    let frac = pos - i as f64;
    let a = v[i.saturating_sub(1).min(n - 1)];
    let b = v[i.min(n - 1)];
    a + (b - a) * frac
}

/// Excel's QUARTILE.INC position `p·(n-1)` (0-based) on sorted data.
fn quantile_inc(v: &[f64], p: f64) -> f64 {
    let n = v.len();
    let pos = (p * (n as f64 - 1.0)).clamp(0.0, (n - 1) as f64);
    let i = pos.floor() as usize;
    let a = v[i.min(n - 1)];
    let b = v[(i + 1).min(n - 1)];
    a + (b - a) * (pos - i as f64)
}

/// The summary of one group's points, with exclusive quartiles.
pub fn box_stats(pts: Vec<f64>) -> Option<BoxStats> {
    box_stats_by(pts, false)
}

/// The summary of one group's points; `inclusive` picks QUARTILE.INC over QUARTILE.EXC.
pub fn box_stats_by(mut pts: Vec<f64>, inclusive: bool) -> Option<BoxStats> {
    if pts.is_empty() {
        return None;
    }
    pts.sort_by(f64::total_cmp);
    let q = |p: f64| if inclusive { quantile_inc(&pts, p) } else { quantile_exc(&pts, p) };
    let (q1, median, q3) = (q(0.25), q(0.5), q(0.75));
    let iqr = q3 - q1;
    let (lo_fence, hi_fence) = (q1 - 1.5 * iqr, q3 + 1.5 * iqr);
    let inside: Vec<f64> = pts.iter().copied().filter(|v| *v >= lo_fence && *v <= hi_fence).collect();
    let low = inside.first().copied().unwrap_or(q1).min(q1);
    let high = inside.last().copied().unwrap_or(q3).max(q3);
    let mean = pts.iter().sum::<f64>() / pts.len() as f64;
    let outliers = pts.iter().copied().filter(|v| *v < lo_fence || *v > hi_fence).collect();
    Some(BoxStats { low, q1, median, q3, high, mean, outliers })
}

/// A node of a treemap or sunburst hierarchy: its label, the total of the leaves under it, and
/// its children (none for a leaf, which names its category index).
#[derive(Clone, Debug, PartialEq)]
pub struct TreeNode {
    pub name: String,
    pub value: f64,
    pub leaf: Option<usize>,
    pub children: Vec<TreeNode>,
}

/// Levels of hierarchy a chart may have (a hostile file cannot nest deeper).
pub const MAX_LEVELS: usize = 16;

impl Chart {
    /// The first series as a hierarchy: top-level nodes, each holding the consecutive
    /// categories that share its label, down to the leaves. Without outer levels every
    /// category is a top-level leaf. Negative and missing values count as zero.
    pub fn category_tree(&self) -> Vec<TreeNode> {
        let Some(s) = self.series.first() else { return vec![] };
        let n = self.categories.len().max(s.values.len());
        let levels: Vec<&Vec<String>> = self.category_levels.iter().take(MAX_LEVELS).collect();
        // Each leaf's path from the top level down, blanks filled from the row above.
        // Labels are shared, not copied: an outer label given once stands for every leaf under it.
        use std::rc::Rc;
        let empty: Rc<str> = Rc::from("");
        let mut last: Vec<Rc<str>> = vec![empty.clone(); levels.len()];
        let paths: Vec<Vec<Rc<str>>> = (0..n)
            .map(|i| {
                let mut path = Vec::with_capacity(levels.len() + 1);
                for (d, lv) in levels.iter().enumerate().rev() {
                    let l = lv.get(i).map(|x| x.trim()).unwrap_or("");
                    if !l.is_empty() {
                        last[d] = Rc::from(l);
                        // A new outer label starts fresh inner groups.
                        for inner in last.iter_mut().take(d) {
                            *inner = empty.clone();
                        }
                    }
                    path.push(last[d].clone());
                }
                path.push(self.categories.get(i).map_or_else(|| empty.clone(), |c| Rc::from(c.as_str())));
                path
            })
            .collect();
        let value = |i: usize| s.values.get(i).copied().flatten().filter(|v| v.is_finite()).unwrap_or(0.0).max(0.0);
        fn build(rows: &[(usize, &[std::rc::Rc<str>])], value: &dyn Fn(usize) -> f64) -> Vec<TreeNode> {
            let mut out: Vec<TreeNode> = Vec::new();
            let mut i = 0;
            while i < rows.len() {
                let (idx, path) = rows[i];
                if path.len() <= 1 {
                    out.push(TreeNode {
                        name: path.first().map(|p| p.to_string()).unwrap_or_default(),
                        value: value(idx),
                        leaf: Some(idx),
                        children: vec![],
                    });
                    i += 1;
                    continue;
                }
                let head = &path[0];
                let mut j = i;
                while j < rows.len() && rows[j].1.len() > 1 && &rows[j].1[0] == head {
                    j += 1;
                }
                let sub: Vec<(usize, &[std::rc::Rc<str>])> = rows[i..j].iter().map(|(k, p)| (*k, &p[1..])).collect();
                let children = build(&sub, value);
                out.push(TreeNode { name: head.to_string(), value: children.iter().map(|c| c.value).sum(), leaf: None, children });
                i = j;
            }
            out
        }
        let rows: Vec<(usize, &[Rc<str>])> = paths.iter().enumerate().map(|(i, p)| (i, p.as_slice())).collect();
        build(&rows, &value)
    }
}

#[cfg(test)]
mod chartex_tests {
    use super::*;

    fn obs(v: &[f64]) -> Chart {
        Chart {
            kind: ChartType::Histogram,
            series: vec![Series { values: v.iter().map(|x| Some(*x)).collect(), ..Default::default() }],
            ..Default::default()
        }
    }

    #[test]
    fn histogram_bins_by_count_size_and_scott() {
        let mut c = obs(&[1.0, 2.0, 2.0, 3.0, 4.0, 5.0, 9.0]);
        c.bin_count = Some(4);
        let b = c.histogram_bins();
        assert_eq!(b.len(), 4);
        assert_eq!((b[0].lo, b[0].hi), (1.0, 3.0));
        // (lo, hi] bins with the minimum in the first: 1, 2, 2, 3 | 4, 5 | - | 9.
        assert_eq!(b.iter().map(|b| b.count).collect::<Vec<_>>(), vec![4, 2, 0, 1]);
        c.bin_count = None;
        c.bin_size = Some(5.0);
        assert_eq!(c.histogram_bins().iter().map(|b| b.count).collect::<Vec<_>>(), vec![6, 1]);
        c.bin_size = None;
        let auto = c.histogram_bins();
        assert!(!auto.is_empty() && auto.iter().map(|b| b.count).sum::<usize>() == 7);
        // A tiny bin size is capped; one value gives one bin.
        c.bin_size = Some(1e-12);
        assert!(c.histogram_bins().len() <= MAX_BINS);
        assert_eq!(obs(&[3.0]).histogram_bins(), vec![Bin { lo: 3.0, hi: 4.0, count: 1 }]);
        assert!(obs(&[]).histogram_bins().is_empty());
    }

    #[test]
    fn histogram_fixed_bin_count_is_exact_despite_float_rounding() {
        let mut c = obs(&[0.0, 1.0, 2.1]);
        c.bin_count = Some(7);
        assert_eq!(c.histogram_bins().len(), 7);
    }

    #[test]
    fn histogram_value_on_a_bin_edge_counts_in_that_bin() {
        let mut c = obs(&[0.0, 2.1, 3.0]);
        c.bin_size = Some(0.3);
        let bins = c.histogram_bins();
        let own = bins.iter().find(|b| b.lo < 2.1 && 2.1 <= b.hi).expect("a bin holds 2.1");
        assert_eq!(own.count, 1, "2.1 must be counted in ({}, {}]", own.lo, own.hi);
        assert_eq!(bins.iter().map(|b| b.count).sum::<usize>(), 3);
    }

    #[test]
    fn pareto_view_sorts_bins_and_adds_the_cumulative_line() {
        let mut c = obs(&[1.0, 5.0, 5.0, 5.0, 9.0, 9.0]);
        c.kind = ChartType::Pareto;
        c.bin_count = Some(3);
        let v = c.histogram_view();
        assert_eq!(v.kind, ChartType::Combo);
        assert_eq!(v.series[0].values, vec![Some(3.0), Some(2.0), Some(1.0)]);
        let cum = &v.series[1];
        assert!(cum.secondary_axis && cum.kind == Some(ChartType::Line));
        assert_eq!(cum.values.last(), Some(&Some(1.0)));
        assert_eq!(v.categories[0], "(3.67, 6.33]");
    }

    #[test]
    fn box_stats_use_exclusive_quartiles_and_fence_outliers() {
        let b = box_stats(vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 100.0]).unwrap();
        // QUARTILE.EXC on 8 points: positions 2.25, 4.5, 6.75.
        assert_eq!((b.q1, b.median, b.q3), (2.25, 4.5, 6.75));
        assert_eq!(b.outliers, vec![100.0]);
        assert_eq!((b.low, b.high), (1.0, 7.0));
        assert!(box_stats(vec![]).is_none());
        let one = box_stats(vec![5.0]).unwrap();
        assert_eq!((one.low, one.q1, one.median, one.q3, one.high), (5.0, 5.0, 5.0, 5.0, 5.0));
    }

    #[test]
    fn inclusive_quartiles_follow_quartile_inc() {
        let b = box_stats_by(vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 100.0], true).unwrap();
        // QUARTILE.INC on 8 points: positions 1.75, 3.5, 5.25 (0-based).
        assert_eq!((b.q1, b.median, b.q3), (2.75, 4.5, 6.25));
    }

    #[test]
    fn category_tree_groups_runs_and_fills_blank_parents() {
        let mut c = Chart::sample(ChartType::Treemap);
        let t = c.category_tree();
        assert_eq!(t.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["Q1", "Q2", "Q3"]);
        assert_eq!(t[0].children.len(), 3);
        assert_eq!(t[0].children[1].leaf, Some(1));
        assert!((t[0].value - (4.2 + 2.8 + 3.6)).abs() < 1e-9);
        // Blank parents continue the label above; two outer levels nest.
        c.category_levels = vec![
            vec!["a".into(), "".into(), "b".into(), "".into(), "c".into(), "".into(), "".into(), "".into()],
            vec!["H1".into(), "".into(), "".into(), "".into(), "H2".into(), "".into(), "".into(), "".into()],
        ];
        let t = c.category_tree();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].children.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(t[1].children[0].children.len(), 4);
        // No levels: every category is a leaf.
        c.category_levels.clear();
        assert!(c.category_tree().iter().all(|n| n.leaf.is_some()));
    }

    #[test]
    fn large_box_hierarchy_and_stacked_charts_take_linear_work() {
        // 50,000 distinct categories in one series: grouped through a map, one pass.
        let n = 50_000;
        let mut c = Chart::sample(ChartType::BoxWhisker);
        c.series.truncate(1);
        c.categories = (0..n).map(|i| format!("g{i}")).collect();
        c.series[0].values = (0..n).map(|i| Some(i as f64)).collect();
        let (groups, stats) = c.box_groups();
        assert_eq!((groups.len(), stats[0].len()), (n, n));
        assert!(stats[0].iter().all(Option::is_some));
        // 2,000 series cap the groups so the result stays within MAX_BOX_CELLS.
        c.series = vec![c.series[0].clone(); 2000];
        let (groups, stats) = c.box_groups();
        assert_eq!(groups.len(), MAX_BOX_CELLS / 2000);
        assert!(stats.iter().all(|s| s.len() == groups.len()));
        // One long outer label over 50,000 leaves is shared, not copied per leaf.
        let mut t = Chart::sample(ChartType::Treemap);
        t.series.truncate(1);
        t.categories = (0..n).map(|i| format!("c{i}")).collect();
        t.series[0].values = (0..n).map(|_| Some(1.0)).collect();
        let mut outer = vec![String::new(); n];
        outer[0] = "o".repeat(1 << 16);
        t.category_levels = vec![outer];
        let tree = t.category_tree();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].children.len(), n);
        // Stacked sums over many sparse series.
        let mut st = Chart::sample(ChartType::StackedBar);
        st.categories = (0..n).map(|i| i.to_string()).collect();
        st.series = (0..2000).map(|k| Series { values: vec![Some(1.0); if k == 0 { n } else { 1 }], ..Default::default() }).collect();
        assert!(st.value_range().1 >= 2000.0, "{:?}", st.value_range());
    }

    #[test]
    fn box_groups_follow_category_labels() {
        let c = Chart::sample(ChartType::BoxWhisker);
        let (groups, stats) = c.box_groups();
        assert_eq!(groups, vec!["Group A".to_string(), "Group B".to_string()]);
        assert_eq!(stats.len(), 2);
        assert!(stats.iter().all(|s| s.len() == 2 && s.iter().all(Option::is_some)));
    }
}
