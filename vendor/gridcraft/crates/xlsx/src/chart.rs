//! DrawingML charts (`xl/charts/chartN.xml`): reading the common 2-D/3-D chart groups into the
//! model, and writing model charts (with cached series values) from scratch.

use std::collections::HashMap;
use std::fmt::Write as _;

use craft_drawingml::chart::{Chart as DmlChart, ChartType, Trendline as DmlTrendline, TrendlineType};
use craft_drawingml::color::SchemeSlot;
use craft_drawingml::style::{ColorBase, ColorRef as DmlColor, Fill as DmlFill};
use gridcraft_core::{RangeRef, Value, category_levels};
use gridcraft_formula::{Expr, SheetSel};
use gridcraft_model::{Chart, ChartDataTable, ChartKind, Color, DefinedName, LegendPos, LineDash, Series, Trendline, TrendlineKind, Workbook};

use crate::drawing::{dml_color, srgb};
use crate::xml::{El, esc, num};

// ---------------------------------------------------------------- reading

pub(crate) fn to_craft_el(e: &El) -> craft_xml::El {
    let mut out = craft_xml::El::new(e.name.as_str());
    out.attrs = e.attrs.clone();
    for c in &e.children {
        out.children.push(craft_xml::Node::El(to_craft_el(c)));
    }
    if !e.text.is_empty() {
        out.children.push(craft_xml::Node::Text(e.text.clone()));
    }
    out
}

pub(crate) fn map_dml_color(c: &DmlColor) -> Option<Color> {
    match &c.base {
        ColorBase::Rgb { rgb } => Some(Color::Rgb((u32::from(rgb.r) << 16) | (u32::from(rgb.g) << 8) | u32::from(rgb.b))),
        ColorBase::Scheme { slot } => {
            let i = match slot {
                SchemeSlot::Bg1 | SchemeSlot::Lt1 => 0,
                SchemeSlot::Tx1 | SchemeSlot::Dk1 => 1,
                SchemeSlot::Bg2 | SchemeSlot::Lt2 => 2,
                SchemeSlot::Tx2 | SchemeSlot::Dk2 => 3,
                SchemeSlot::Accent1 => 4,
                SchemeSlot::Accent2 => 5,
                SchemeSlot::Accent3 => 6,
                SchemeSlot::Accent4 => 7,
                SchemeSlot::Accent5 => 8,
                SchemeSlot::Accent6 => 9,
                SchemeSlot::Hlink => 10,
                SchemeSlot::FolHlink => 11,
                SchemeSlot::PhClr => return None,
            };
            Some(Color::Theme(i, 0))
        }
        ColorBase::System { last, .. } => Some(Color::Rgb((u32::from(last.r) << 16) | (u32::from(last.g) << 8) | u32::from(last.b))),
        ColorBase::Preset { .. } => None,
    }
}

fn map_chart_type(k: ChartType, plot: Option<&El>) -> ChartKind {
    match k {
        ChartType::Column => ChartKind::ColumnClustered,
        ChartType::StackedColumn => ChartKind::ColumnStacked,
        ChartType::PercentColumn => ChartKind::ColumnStacked100,
        ChartType::Bar => ChartKind::BarClustered,
        ChartType::StackedBar => ChartKind::BarStacked,
        ChartType::PercentBar => ChartKind::BarStacked100,
        ChartType::Line => {
            let markers = plot
                .into_iter()
                .flat_map(|p| p.kids("lineChart").chain(p.kids("line3DChart")))
                .filter(|g| !matches!(g.child_val("grouping"), Some("stacked" | "percentStacked")))
                .any(|g| {
                    let group_markers = g.child_val("marker").is_none_or(|v| v == "1" || v == "true");
                    let all_none = g.kids("ser").all(|s| s.path(&["marker", "symbol"]).and_then(|m| m.attr("val")) == Some("none"));
                    group_markers && !(all_none && g.kids("ser").next().is_some())
                });
            if markers { ChartKind::LineMarkers } else { ChartKind::Line }
        }
        ChartType::LineMarkers => ChartKind::LineMarkers,
        ChartType::StackedLine => ChartKind::LineStacked,
        ChartType::Area => ChartKind::Area,
        ChartType::StackedArea => {
            let pct = plot
                .into_iter()
                .flat_map(|p| p.kids("areaChart").chain(p.kids("area3DChart")))
                .any(|g| g.child_val("grouping") == Some("percentStacked"));
            if pct { ChartKind::AreaStacked100 } else { ChartKind::AreaStacked }
        }
        ChartType::Pie => ChartKind::Pie,
        ChartType::Doughnut => ChartKind::Doughnut,
        ChartType::Scatter => {
            let marker_only = plot.and_then(|p| p.child("scatterChart")).is_some_and(|g| {
                let no_lines = g.kids("ser").all(|s| s.path(&["spPr", "ln"]).is_some_and(|l| l.child("noFill").is_some()));
                g.child_val("scatterStyle") == Some("marker") || (no_lines && g.kids("ser").next().is_some())
            });
            if marker_only { ChartKind::Scatter } else { ChartKind::ScatterLines }
        }
        ChartType::Bubble => ChartKind::Bubble,
        ChartType::Radar | ChartType::FilledRadar => ChartKind::Radar,
        ChartType::Stock => ChartKind::Stock,
        ChartType::Combo => ChartKind::Combo,
        _ => ChartKind::ColumnClustered,
    }
}

fn map_trendline(t: &DmlTrendline, el: Option<&El>) -> Trendline {
    let kind = match t.kind {
        TrendlineType::Linear => TrendlineKind::Linear,
        TrendlineType::Exp => TrendlineKind::Exponential,
        TrendlineType::Log => TrendlineKind::Logarithmic,
        TrendlineType::Power => TrendlineKind::Power,
        TrendlineType::Poly => TrendlineKind::Polynomial(t.order.unwrap_or(2).clamp(2, 6) as u8),
        TrendlineType::MovingAvg => TrendlineKind::MovingAverage(t.period.unwrap_or(2).clamp(2, 255)),
    };
    let ln = el.and_then(|e| e.path(&["spPr", "ln"]));
    let forecast = |name: &str, dml: Option<f64>| {
        Trendline::clamp_forecast(el.and_then(|e| e.child(name)).and_then(|c| c.attr_f64("val")).or(dml).unwrap_or(0.0))
    };
    Trendline {
        kind,
        name: t.name.clone(),
        forward: forecast("forward", t.forward),
        backward: forecast("backward", t.backward),
        intercept: t.intercept,
        display_equation: el.and_then(|e| e.child("dispEq")).map_or(t.disp_eq, |c| c.flag("val", true)),
        display_r_squared: el.and_then(|e| e.child("dispRSqr")).map_or(t.disp_r_sqr, |c| c.flag("val", true)),
        color: ln.and_then(|l| l.child("solidFill")).and_then(dml_color),
        width: ln.and_then(|l| l.attr_f64("w")).map(|w| (w / EMU_PER_PT) as f32).filter(|w| w.is_finite() && *w >= 0.0),
        dash: ln.and_then(|l| l.child("prstDash")).and_then(|d| d.attr("val")).and_then(|v| DASHES.iter().find(|(_, n)| *n == v).map(|(d, _)| *d)),
    }
}

fn literal_array(e: Option<&El>) -> Option<String> {
    let e = e?;
    for k in ["numLit", "strLit"] {
        if let Some(l) = e.child(k) {
            let pts: Vec<String> = l
                .kids("pt")
                .filter_map(|p| p.child("v"))
                .map(|v| if k == "numLit" { v.text.trim().to_string() } else { format!("\"{}\"", v.text.replace('"', "\"\"")) })
                .collect();
            return Some(format!("{{{}}}", pts.join(",")));
        }
    }
    None
}

/// Excel's file notation for "this workbook" in front of a workbook-scoped defined name.
const THIS_BOOK: &str = "[0]!";

/// A chart reference (`<c:f>`) read from a file → model text: drops a leading `=` and the
/// `[0]!` in front of workbook-scoped names (`[0]!Sales` → `Sales`), which the formula parser
/// would take for a structured reference. External books (`[1]Sheet1!$A$1`), quoted sheet
/// names and string literals are kept as they are.
pub(crate) fn chart_ref_from_file(text: &str) -> String {
    let t = text.trim();
    let t = t.strip_prefix('=').unwrap_or(t);
    let mut out = String::with_capacity(t.len());
    let mut quote: Option<char> = None;
    // The last character kept: `[0]!` only counts at the start or after an operator,
    // never inside a structured reference (`T[[0]]`) or a longer name.
    let mut prev: Option<char> = None;
    let mut i = 0;
    while let Some(c) = t.get(i..).and_then(|r| r.chars().next()) {
        let rest = t.get(i..).unwrap_or("");
        if quote.is_none()
            && rest.starts_with(THIS_BOOK)
            && prev.is_none_or(|p| !(p.is_alphanumeric() || "_.\\[]'\"!$#".contains(p)))
            && rest.get(THIS_BOOK.len()..).and_then(|n| n.chars().next()).is_some_and(|n| n.is_alphabetic() || n == '_' || n == '\\')
        {
            i += THIS_BOOK.len();
            continue;
        }
        match (c, quote) {
            ('"' | '\'', None) => quote = Some(c),
            (q, Some(open)) if q == open => quote = None,
            _ => {}
        }
        out.push(c);
        prev = Some(c);
        i += c.len_utf8();
    }
    out
}

const DASHES: [(LineDash, &str); 11] = [
    (LineDash::Solid, "solid"),
    (LineDash::Dot, "dot"),
    (LineDash::Dash, "dash"),
    (LineDash::LgDash, "lgDash"),
    (LineDash::DashDot, "dashDot"),
    (LineDash::LgDashDot, "lgDashDot"),
    (LineDash::LgDashDotDot, "lgDashDotDot"),
    (LineDash::SysDash, "sysDash"),
    (LineDash::SysDot, "sysDot"),
    (LineDash::SysDashDot, "sysDashDot"),
    (LineDash::SysDashDotDot, "sysDashDotDot"),
];

const EMU_PER_PT: f64 = 12_700.0;

fn group_kind(g: &El) -> Option<ChartKind> {
    let grouping = g.child_val("grouping").unwrap_or("standard");
    let stacked = grouping == "stacked";
    let pct = grouping == "percentStacked";
    Some(match g.name.as_str() {
        "barChart" | "bar3DChart" => {
            let bar = g.child_val("barDir") == Some("bar");
            match (bar, stacked, pct) {
                (false, true, _) => ChartKind::ColumnStacked,
                (false, _, true) => ChartKind::ColumnStacked100,
                (false, _, _) => ChartKind::ColumnClustered,
                (true, true, _) => ChartKind::BarStacked,
                (true, _, true) => ChartKind::BarStacked100,
                (true, _, _) => ChartKind::BarClustered,
            }
        }
        "lineChart" | "line3DChart" => {
            if stacked || pct {
                ChartKind::LineStacked
            } else {
                // The group's kind is its first series' (each series may differ; see
                // `series_kind_in`).
                match g.kids("ser").next() {
                    Some(first) => series_kind_in(g, ChartKind::LineMarkers, first),
                    None if g.child_val("marker").is_some_and(|v| v == "0" || v == "false") => ChartKind::Line,
                    None => ChartKind::LineMarkers,
                }
            }
        }
        "pieChart" | "pie3DChart" | "ofPieChart" => ChartKind::Pie,
        "doughnutChart" => ChartKind::Doughnut,
        "areaChart" | "area3DChart" => {
            if pct {
                ChartKind::AreaStacked100
            } else if stacked {
                ChartKind::AreaStacked
            } else {
                ChartKind::Area
            }
        }
        "scatterChart" => match g.kids("ser").next() {
            _ if g.child_val("scatterStyle") == Some("marker") => ChartKind::Scatter,
            Some(first) => series_kind_in(g, ChartKind::ScatterLines, first),
            None => ChartKind::ScatterLines,
        },
        "bubbleChart" => ChartKind::Bubble,
        "radarChart" => ChartKind::Radar,
        "stockChart" => ChartKind::Stock,
        _ => return None,
    })
}

/// The kind series `s` of group `g` (of kind `k`) is drawn as: in a line group its own markers
/// (none when it says so), in a scatter group its own lines; otherwise the group's.
fn series_kind_in(g: &El, k: ChartKind, s: &El) -> ChartKind {
    match k {
        ChartKind::Line | ChartKind::LineMarkers => {
            let group_markers = g.child_val("marker").is_none_or(|v| v == "1" || v == "true");
            let none = s.path(&["marker", "symbol"]).and_then(|m| m.attr("val")) == Some("none");
            if group_markers && !none { ChartKind::LineMarkers } else { ChartKind::Line }
        }
        ChartKind::Scatter | ChartKind::ScatterLines if g.child_val("scatterStyle") != Some("marker") => {
            if s.path(&["spPr", "ln"]).is_some_and(|l| l.child("noFill").is_some()) { ChartKind::Scatter } else { ChartKind::ScatterLines }
        }
        k => k,
    }
}

fn rich_title(t: &El) -> Option<String> {
    let tx = t.child("tx")?;
    if let Some(rich) = tx.child("rich") {
        let paras: Vec<String> =
            rich.kids("p").map(|p| p.kids("r").filter_map(|r| r.child("t")).map(|t| t.text.as_str()).collect::<String>()).collect();
        let s = paras.join("\n");
        return if s.is_empty() { None } else { Some(s) };
    }
    if let Some(sr) = tx.child("strRef") {
        if let Some(v) = sr.path(&["strCache", "pt", "v"]) {
            return Some(v.text.clone());
        }
        return sr.child("f").map(|f| f.text.trim().to_string());
    }
    None
}

/// The text of an axis title (`c:title`): its rich text or cached reference, else Excel's default.
fn axis_title(t: &El) -> String {
    rich_title(t).unwrap_or_else(|| "Axis Title".into())
}

// Value axes looked at (and looked up) while finding a chart's primary axes, so a test can check
// the work grows with the file, not with its square.
#[cfg(test)]
thread_local! {
    pub(crate) static VAL_AXIS_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(crate) fn from_dml_chart(dc: DmlChart, root: Option<&El>) -> Chart {
    let plot = root.and_then(|r| r.path(&["chart", "plotArea"]));
    let groups: Vec<&El> = plot.map(|p| p.children.iter().filter(|g| g.name.ends_with("Chart")).collect()).unwrap_or_default();
    fn ax_ids(g: &El) -> Vec<&str> {
        g.kids("axId").filter_map(|a| a.attr("val")).collect()
    }
    // An empty group draws nothing: the chart's kind and axes come from the groups with series
    // (a chart without series from its first group).
    let kinded: Vec<(&El, ChartKind)> = groups.iter().filter_map(|g| group_kind(g).map(|k| (*g, k))).collect();
    let with_series = kinded.iter().any(|(g, _)| g.child("ser").is_some());
    let drawing: Vec<(&El, ChartKind)> = kinded.iter().copied().filter(|(g, _)| !with_series || g.child("ser").is_some()).collect();
    let first_kind = drawing.first().map_or_else(|| map_chart_type(dc.kind, plot), |(_, k)| *k);
    // The primary axes are the pair whose value axis sits left or bottom (a secondary pair is on
    // the right or top), else the first group's; series on other axes are secondary.
    let mut val_sides: HashMap<&str, &str> = HashMap::new();
    for a in plot.into_iter().flat_map(|p| p.kids("valAx")) {
        #[cfg(test)]
        VAL_AXIS_VISITS.with(|n| n.set(n.get().saturating_add(1)));
        if let (Some(id), Some(pos)) = (a.child_val("axId"), a.child_val("axPos")) {
            val_sides.entry(id).or_insert(pos);
        }
    }
    let pairs: Vec<Vec<&str>> = drawing.iter().map(|(g, _)| ax_ids(g)).collect();
    let primary_axes: Vec<&str> = pairs
        .iter()
        .find(|ids| {
            #[cfg(test)]
            VAL_AXIS_VISITS.with(|n| n.set(n.get().saturating_add(1)));
            ids.get(1).is_some_and(|id| matches!(val_sides.get(id).copied(), Some("l" | "b")))
        })
        .or_else(|| pairs.iter().find(|ids| !ids.is_empty()))
        .cloned()
        .unwrap_or_default();
    // The axis titles are the primary pair's (an axis of another pair, or of an empty group, holds
    // another scale's titles), else the first of their kind in the file; a shown axis before a
    // deleted one.
    let primary = |a: &El| a.child_val("axId").is_some_and(|id| primary_axes.contains(&id));
    let shown = |a: &El| !a.child("delete").is_some_and(|c| c.flag("val", true));
    let first_of = |tags: &[&str], pos: &[&str]| -> Option<&El> {
        let fits = |a: &&El| tags.contains(&a.name.as_str()) && (pos.is_empty() || a.child_val("axPos").is_some_and(|p| pos.contains(&p)));
        let axes = plot.into_iter().flat_map(|p| p.children.iter()).filter(fits);
        let ranked: [&dyn Fn(&El) -> bool; 4] = [&|a| primary(a) && shown(a), &shown, &primary, &|_| true];
        ranked.into_iter().find_map(|r| axes.clone().find(|a| r(a)))
    };
    // The primary pair's group says whether its axes are both value axes (a scatter's).
    let primary_kind = drawing.iter().zip(&pairs).find(|(_, ids)| **ids == primary_axes).map_or(first_kind, |((_, k), _)| *k);
    let (x_ax, y_ax) = if matches!(primary_kind, ChartKind::Scatter | ChartKind::ScatterLines | ChartKind::Bubble) {
        (first_of(&["valAx"], &["b", "t"]), first_of(&["valAx"], &["l", "r"]))
    } else {
        (first_of(&["catAx", "dateAx"], &[]), first_of(&["valAx"], &[]))
    };
    let title_of = |a: Option<&El>| a.and_then(|a| a.child("title")).map(axis_title);
    let axis_titles = if plot.is_some() { (title_of(x_ax), title_of(y_ax)) } else { dc.axis_titles.clone() };
    // Each series element with its axes and the kind its group draws it as (a line's markers and
    // a scatter's lines are the series' own), in the order the shared reader lists them.
    let ser_els: Vec<(&El, bool, ChartKind)> = groups
        .iter()
        .flat_map(|g| {
            let ids = ax_ids(g);
            let sec = !ids.is_empty() && ids != primary_axes;
            let k = group_kind(g);
            g.kids("ser").map(move |s| (s, sec, k.map_or(first_kind, |k| series_kind_in(g, k, s))))
        })
        .collect();
    let series_labels = dc.series.iter().any(|s| s.data_labels == Some(true));
    let series = dc
        .series
        .into_iter()
        .enumerate()
        .map(|(i, s)| {
            let (sel, sec, drawn) = match ser_els.get(i).copied() {
                Some((el, sec, k)) => (Some(el), sec, k),
                None => (None, s.secondary_axis, s.kind.map_or(first_kind, |k| map_chart_type(k, plot))),
            };
            let color = s
                .fill
                .as_ref()
                .and_then(|f| match f {
                    DmlFill::Solid { color } => map_dml_color(color),
                    _ => None,
                })
                .or_else(|| {
                    s.line.as_ref().and_then(|l| l.fill.as_ref()).and_then(|f| match f {
                        DmlFill::Solid { color } => map_dml_color(color),
                        _ => None,
                    })
                })
                .or_else(|| sel.and_then(|e| e.path(&["marker", "spPr", "solidFill"])).and_then(dml_color));
            let mut trendline_els = sel
                .into_iter()
                .flat_map(|e| e.kids("trendline").take(16))
                .filter(|e| e.child("trendlineType").and_then(|c| c.attr("val")).and_then(TrendlineType::from_xml).is_some());
            let series = Series {
                name: s.refs.name.as_deref().map(chart_ref_from_file).or_else(|| (!s.name.is_empty()).then_some(s.name)),
                categories: s
                    .refs
                    .categories
                    .as_deref()
                    .map(chart_ref_from_file)
                    .or_else(|| literal_array(sel.and_then(|e| e.child("cat").or_else(|| e.child("xVal"))))),
                values: s
                    .refs
                    .values
                    .as_deref()
                    .map(chart_ref_from_file)
                    .or_else(|| literal_array(sel.and_then(|e| e.child("val").or_else(|| e.child("yVal")))))
                    .unwrap_or_default(),
                bubble_sizes: s.refs.sizes.as_deref().map(chart_ref_from_file).or_else(|| literal_array(sel.and_then(|e| e.child("bubbleSize")))),
                color,
                secondary: sec,
                kind: None,
                quartiles: Default::default(),
                smooth: sel.map_or(s.smooth, |e| e.child("smooth").is_some_and(|c| c.flag("val", true))),
                trendlines: s.trendlines.iter().map(|t| map_trendline(t, trendline_els.next())).collect(),
            };
            (series, drawn, sel)
        })
        .collect::<Vec<_>>();
    // Series in the order the file gives them (`c:order`), whichever group holds them.
    let order = |e: Option<&El>| e.and_then(|s| s.child_val("order")).and_then(|v| v.parse::<u32>().ok());
    let mut series = series;
    if series.iter().all(|(_, _, e)| order(*e).is_some()) {
        series.sort_by_key(|(_, _, e)| order(*e));
    }
    let (series, drawn): (Vec<Series>, Vec<ChartKind>) = series.into_iter().map(|(s, k, _)| (s, k)).unzip();
    let legend = match dc.legend.as_deref() {
        None => LegendPos::None,
        Some("b") => LegendPos::Bottom,
        Some("t") => LegendPos::Top,
        Some("l") => LegendPos::Left,
        Some(_) => LegendPos::Right,
    };
    let hole_size =
        if plot.is_some_and(|p| p.child("doughnutChart").is_some()) { (dc.hole_size * 100.0).round().clamp(10.0, 90.0) as u8 } else { 75 };
    let mut ch = Chart {
        id: 0,
        kind: first_kind,
        anchor: Default::default(),
        title: dc.title,
        series,
        legend,
        data_labels: dc.data_labels || series_labels,
        data_table: dc.data_table.map(|t| ChartDataTable {
            show_keys: t.show_keys,
            horizontal_border: t.horizontal_border,
            vertical_border: t.vertical_border,
            outline: t.outline,
        }),
        gridlines: dc.gridlines,
        style: dc.style,
        hole_size,
        first_slice_angle: dc.first_slice_angle,
        x_title: axis_titles.0,
        y_title: axis_titles.1,
        source: None,
        by_rows: false,
        pivot: dc.pivot_source.as_deref().map(strip_book).filter(|n| !n.is_empty()).map(str::to_string),
    };
    // Series of one family are one chart; series of other families make a combination chart.
    ch.set_drawn_kinds(&drawn, first_kind);
    ch
}

pub fn read_chart(root: &El) -> Option<Chart> {
    let craft = to_craft_el(root);
    let dc = craft_drawingml::chart::read::chart_from(&craft)?;
    Some(from_dml_chart(dc, Some(root)))
}

// ---------------------------------------------------------------- writing

#[derive(Clone, Copy, PartialEq)]
enum G {
    Bar { bar: bool, grouping: &'static str },
    Line { stacked: bool, markers: bool },
    Pie,
    Doughnut,
    Area { grouping: &'static str },
    Scatter { lines: bool },
    Bubble,
    Radar,
    Stock,
}

/// The kind of axes a group is drawn on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Axes {
    /// None: pie and doughnut.
    None,
    /// Categories across, values up: columns, lines, areas, stock, radar.
    Category,
    /// Categories down, values across: horizontal bars.
    Horizontal,
    /// Values both ways: scatter and bubble.
    Values,
}

fn axes_of(g: G) -> Axes {
    match g {
        G::Pie | G::Doughnut => Axes::None,
        G::Bar { bar: true, .. } => Axes::Horizontal,
        G::Scatter { .. } | G::Bubble => Axes::Values,
        _ => Axes::Category,
    }
}

/// Group `g` drawn on axes of kind `family`: itself where it fits, else the nearest group that
/// does (a line-like series as lines, others as columns, bars or scatter).
fn fit_axes(g: G, family: Axes) -> G {
    let own = axes_of(g);
    if own == family || family == Axes::None {
        return g;
    }
    let line_like = matches!(g, G::Line { .. } | G::Area { .. } | G::Radar | G::Scatter { .. });
    match family {
        Axes::None => g,
        Axes::Category if line_like => G::Line { stacked: false, markers: false },
        Axes::Category => G::Bar { bar: false, grouping: "clustered" },
        Axes::Horizontal => G::Bar { bar: true, grouping: "clustered" },
        Axes::Values => G::Scatter { lines: true },
    }
}

fn group_for(k: ChartKind) -> G {
    match k {
        ChartKind::ColumnStacked => G::Bar { bar: false, grouping: "stacked" },
        ChartKind::ColumnStacked100 => G::Bar { bar: false, grouping: "percentStacked" },
        ChartKind::BarClustered => G::Bar { bar: true, grouping: "clustered" },
        ChartKind::BarStacked => G::Bar { bar: true, grouping: "stacked" },
        ChartKind::BarStacked100 => G::Bar { bar: true, grouping: "percentStacked" },
        ChartKind::Line => G::Line { stacked: false, markers: false },
        ChartKind::LineMarkers => G::Line { stacked: false, markers: true },
        ChartKind::LineStacked => G::Line { stacked: true, markers: false },
        ChartKind::Pie => G::Pie,
        ChartKind::Doughnut => G::Doughnut,
        ChartKind::Area => G::Area { grouping: "standard" },
        ChartKind::AreaStacked => G::Area { grouping: "stacked" },
        ChartKind::AreaStacked100 => G::Area { grouping: "percentStacked" },
        ChartKind::Scatter => G::Scatter { lines: false },
        ChartKind::ScatterLines => G::Scatter { lines: true },
        ChartKind::Bubble => G::Bubble,
        ChartKind::Radar => G::Radar,
        ChartKind::Stock => G::Stock,
        // Combo charts start from clustered columns; chartex kinds have their own part.
        _ => G::Bar { bar: false, grouping: "clustered" },
    }
}

const MAX_CACHE: u64 = 100_000;

/// The sheet and range a same-workbook reference (`Sheet1!$B$2:$B$9`) names.
fn ref_target(wb: &Workbook, sheet_idx: usize, f: &str) -> Option<(usize, RangeRef)> {
    let e = gridcraft_formula::parse(f).ok()?;
    let Expr::Ref(r) = e else { return None };
    let si = match &r.sheet {
        SheetSel::Current => sheet_idx,
        SheetSel::Named(n) => wb.sheet_index(n)?,
        SheetSel::Span(..) => return None,
    };
    Some((si, r.range()))
}

/// Values of a same-workbook reference (`Sheet1!$B$2:$B$9`), row-major.
pub(crate) fn ref_values(wb: &Workbook, sheet_idx: usize, f: &str) -> Option<Vec<Value>> {
    let (si, mut range) = ref_target(wb, sheet_idx, f)?;
    let sheet = wb.sheet(si)?;
    // Whole columns/rows: cut the far end at the used range. The start stays put, so point indices keep matching the reference.
    if let Some(u) = sheet.used_range() {
        if range.is_full_cols() {
            range.end.row = range.end.row.min(u.end.row.max(range.start.row));
        }
        if range.is_full_rows() {
            range.end.col = range.end.col.min(u.end.col.max(range.start.col));
        }
    }
    if range.count() > MAX_CACHE {
        return None;
    }
    Some(range.iter().map(|c| sheet.value(c)).collect())
}

fn is_ref(f: &str) -> bool {
    matches!(gridcraft_formula::parse(f), Ok(Expr::Ref(_)))
}

/// A chart reference that is a name: its sheet when qualified (`Sheet1!Sales`), the bare
/// name, and the definition formulas would find from sheet `si` (none when undefined). `None`
/// when it isn't a name, or names a sheet the workbook doesn't have.
fn chart_name<'a>(wb: &'a Workbook, si: usize, f: &str) -> Option<(Option<usize>, String, Option<&'a DefinedName>)> {
    let Ok(Expr::Name(n)) = gridcraft_formula::parse(f.trim()) else { return None };
    // The parser has already unquoted the sheet (`'Bob''s'!Name` → `Bob's!Name`, #2012): split
    // at the last `!` (`'a!b'!Name`) and keep the name as is (`' Sales '` keeps its spaces).
    match n.rsplit_once('!') {
        Some((s, bare)) => {
            let i = wb.sheet_index(s)?;
            Some((Some(i), bare.to_string(), wb.name(bare, i)))
        }
        None => {
            let def = wb.name(&n, si);
            Some((None, n, def))
        }
    }
}

/// A chart reference for `<c:f>`. Excel needs a name qualified there: with a bare `Name` it
/// draws the whole chart blank, with no repair prompt (observed in Excel for Windows,
/// Microsoft 365). A workbook-scoped name is written `[0]!Name`, as Excel writes it, a
/// sheet-scoped one `Sheet!Name`; an undefined name gets `[0]!`, as the file it came from had.
fn chart_ref_to_file(wb: &Workbook, si: usize, f: &str) -> String {
    let on_sheet = |i: usize, n: &str| wb.sheet(i).map(|sh| format!("{}!{n}", gridcraft_formula::quote_sheet(&sh.name)));
    match chart_name(wb, si, f) {
        Some((Some(i), n, _)) => on_sheet(i, &n).unwrap_or_else(|| crate::fmla::text_to_file(f)),
        Some((None, n, def)) => def.and_then(|d| d.scope).and_then(|i| on_sheet(i, &n)).unwrap_or_else(|| format!("{THIS_BOOK}{n}")),
        None => crate::fmla::text_to_file(f),
    }
}

/// A series name that refers to a defined name (as opposed to literal text that parses as one).
fn is_defined_name(wb: &Workbook, si: usize, f: &str) -> bool {
    chart_name(wb, si, f).is_some_and(|(_, _, d)| d.is_some())
}

fn num_data(tag: &str, f: Option<&str>, wb: &Workbook, si: usize) -> String {
    let f = f.unwrap_or("").trim();
    if f.is_empty() {
        return format!("<c:{tag}><c:numLit><c:ptCount val=\"0\"/></c:numLit></c:{tag}>");
    }
    if f.starts_with('{') {
        let vals: Vec<&str> = f.trim_matches(|c| c == '{' || c == '}').split([',', ';']).collect();
        let mut s = format!("<c:{tag}><c:numLit><c:formatCode>General</c:formatCode><c:ptCount val=\"{}\"/>", vals.len());
        for (i, v) in vals.iter().enumerate() {
            if let Ok(n) = v.trim().parse::<f64>() {
                let _ = write!(s, "<c:pt idx=\"{i}\"><c:v>{}</c:v></c:pt>", num(n));
            }
        }
        let _ = write!(s, "</c:numLit></c:{tag}>");
        return s;
    }
    let mut s = format!("<c:{tag}><c:numRef><c:f>{}</c:f>", esc(&chart_ref_to_file(wb, si, f)));
    if let Some(vals) = ref_values(wb, si, f) {
        let _ = write!(s, "<c:numCache><c:formatCode>General</c:formatCode><c:ptCount val=\"{}\"/>", vals.len());
        for (i, v) in vals.iter().enumerate() {
            if let Value::Number(n) = v {
                let _ = write!(s, "<c:pt idx=\"{i}\"><c:v>{}</c:v></c:pt>", num(*n));
            }
        }
        s.push_str("</c:numCache>");
    }
    let _ = write!(s, "</c:numRef></c:{tag}>");
    s
}

/// Splits the inside of an array constant on `,` and `;` outside double quotes; inside quotes `""` is a literal quote.
/// Unquoted items are trimmed; quoted ones keep the text between the quotes and drop the spaces around them.
fn split_array_items(inner: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut in_quote = false;
    let mut chars = inner.chars().peekable();
    let finish = |cur: &mut String, quoted: &mut bool| {
        let text = std::mem::take(cur);
        let item = if *quoted { text } else { text.trim().to_string() };
        *quoted = false;
        item
    };
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quote && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => {
                if !quoted && cur.trim().is_empty() {
                    cur.clear();
                }
                in_quote = !in_quote;
                quoted = true;
            }
            ',' | ';' if !in_quote => items.push(finish(&mut cur, &mut quoted)),
            c if quoted && !in_quote && c.is_whitespace() => {}
            _ => cur.push(c),
        }
    }
    items.push(finish(&mut cur, &mut quoted));
    items
}

fn str_data(tag: &str, f: &str, wb: &Workbook, si: usize) -> String {
    let f = f.trim();
    if f.starts_with('{') {
        let vals = split_array_items(f.trim_matches(|c| c == '{' || c == '}'));
        let mut s = format!("<c:{tag}><c:strLit><c:ptCount val=\"{}\"/>", vals.len());
        for (i, v) in vals.iter().enumerate() {
            let _ = write!(s, "<c:pt idx=\"{i}\"><c:v>{}</c:v></c:pt>", esc(v));
        }
        let _ = write!(s, "</c:strLit></c:{tag}>");
        return s;
    }
    let mut s = format!("<c:{tag}><c:strRef><c:f>{}</c:f>", esc(&chart_ref_to_file(wb, si, f)));
    if let Some(vals) = ref_values(wb, si, f) {
        let _ = write!(s, "<c:strCache><c:ptCount val=\"{}\"/>", vals.len());
        for (i, v) in vals.iter().enumerate() {
            if !v.is_empty() {
                let _ = write!(s, "<c:pt idx=\"{i}\"><c:v>{}</c:v></c:pt>", esc(&v.display()));
            }
        }
        s.push_str("</c:strCache>");
    }
    let _ = write!(s, "</c:strRef></c:{tag}>");
    s
}

/// `<c:cat>` of a series: a string reference, or a multi-level one (`multiLvlStrRef`) when the
/// category range has more columns than a column of values (or more rows than a row of values:
/// year beside month). Its cache lists the innermost level first, then each outer level with
/// only the cells that start a group, as Excel writes it.
fn cat_data(f: &str, values: &str, wb: &Workbook, si: usize) -> String {
    multi_level_cat(f.trim(), values, wb, si).unwrap_or_else(|| str_data("cat", f, wb, si))
}

fn multi_level_cat(f: &str, values: &str, wb: &Workbook, si: usize) -> Option<String> {
    let (sheet, range) = ref_target(wb, si, f)?;
    let value_range = ref_target(wb, si, values).map(|(_, r)| r);
    let levels = category_levels(&range, value_range.as_ref())?;
    if range.count() > MAX_CACHE {
        return None;
    }
    let sheet = wb.sheet(sheet)?;
    let (w, h) = (range.width() as usize, range.height() as usize);
    let text = |row: usize, col: usize| -> String {
        let v = range.start.offset(row as i64, col as i64).map(|c| sheet.value(c));
        v.filter(|v| !v.is_empty()).map(|v| v.display()).unwrap_or_default()
    };
    // The innermost level (nearest the values), then the outer ones.
    let (n, lvls): (usize, Vec<Vec<String>>) = if levels.by_columns {
        (h, (0..=levels.outer as usize).filter_map(|k| (w - 1).checked_sub(k)).map(|c| (0..h).map(|r| text(r, c)).collect()).collect())
    } else {
        (w, (0..=levels.outer as usize).filter_map(|k| (h - 1).checked_sub(k)).map(|r| (0..w).map(|c| text(r, c)).collect()).collect())
    };
    let mut s = format!("<c:cat><c:multiLvlStrRef><c:f>{}</c:f><c:multiLvlStrCache><c:ptCount val=\"{n}\"/>", esc(&crate::fmla::text_to_file(f)));
    for lvl in &lvls {
        s.push_str("<c:lvl>");
        for (i, v) in lvl.iter().enumerate() {
            if !v.is_empty() {
                let _ = write!(s, "<c:pt idx=\"{i}\"><c:v>{}</c:v></c:pt>", esc(v));
            }
        }
        s.push_str("</c:lvl>");
    }
    s.push_str("</c:multiLvlStrCache></c:multiLvlStrRef></c:cat>");
    Some(s)
}

/// Series `s` in group `g`, drawn as kind `own`: a line or scatter series keeps its own markers
/// or lines within its group.
fn series_xml(s: &Series, idx: usize, g: G, own: ChartKind, wb: &Workbook, si: usize) -> String {
    let g = match (g, group_for(own)) {
        (G::Line { stacked, .. }, G::Line { markers, .. }) => G::Line { stacked, markers },
        (G::Scatter { .. }, mine @ G::Scatter { .. }) => mine,
        _ => g,
    };
    let mut x = format!("<c:ser><c:idx val=\"{idx}\"/><c:order val=\"{idx}\"/>");
    if let Some(n) = &s.name {
        if is_ref(n) || is_defined_name(wb, si, n) {
            x.push_str(&str_data("tx", n, wb, si));
        } else {
            let lit = n.trim().strip_prefix('"').and_then(|t| t.strip_suffix('"')).unwrap_or(n);
            let _ = write!(x, "<c:tx><c:v>{}</c:v></c:tx>", esc(lit));
        }
    }
    let color = s.color.as_ref().and_then(|c: &Color| srgb(c, &wb.theme));
    let line_like = matches!(g, G::Line { .. } | G::Radar | G::Scatter { lines: true });
    match (&color, line_like) {
        (Some(c), true) => {
            let _ = write!(x, "<c:spPr><a:ln w=\"28575\" cap=\"rnd\"><a:solidFill>{c}</a:solidFill><a:round/></a:ln></c:spPr>");
        }
        (Some(c), false) if g == (G::Scatter { lines: false }) => {
            let _ = write!(
                x,
                "<c:spPr><a:ln w=\"19050\"><a:noFill/></a:ln></c:spPr><c:marker><c:symbol val=\"circle\"/><c:size val=\"5\"/><c:spPr><a:solidFill>{c}</a:solidFill></c:spPr></c:marker>"
            );
        }
        (Some(c), false) => {
            let _ = write!(x, "<c:spPr><a:solidFill>{c}</a:solidFill></c:spPr>");
        }
        (None, _) if g == (G::Scatter { lines: false }) || g == G::Stock => x.push_str("<c:spPr><a:ln w=\"19050\"><a:noFill/></a:ln></c:spPr>"),
        _ => {}
    }
    match g {
        G::Bar { .. } => x.push_str("<c:invertIfNegative val=\"0\"/>"),
        G::Line { markers: false, .. } | G::Scatter { lines: true } | G::Stock => x.push_str("<c:marker><c:symbol val=\"none\"/></c:marker>"),
        G::Bubble => x.push_str("<c:invertIfNegative val=\"0\"/>"),
        _ => {}
    }
    // Trendlines sit between dLbls and the data in CT_LineSer, CT_BarSer, CT_AreaSer,
    // CT_ScatterSer and CT_BubbleSer; pie and radar series have none.
    if !matches!(g, G::Pie | G::Doughnut | G::Radar) {
        for t in &s.trendlines {
            x.push_str(&trendline_xml(t, wb));
        }
    }
    match g {
        G::Scatter { .. } | G::Bubble => {
            if let Some(c) = &s.categories {
                x.push_str(&num_data("xVal", Some(c), wb, si));
            }
            x.push_str(&num_data("yVal", Some(&s.values), wb, si));
            if g == G::Bubble {
                x.push_str(&num_data("bubbleSize", s.bubble_sizes.as_deref().or(Some(&s.values)), wb, si));
                x.push_str("<c:bubble3D val=\"0\"/>");
            } else {
                let _ = write!(x, "<c:smooth val=\"{}\"/>", s.smooth as u8);
            }
        }
        _ => {
            if let Some(c) = &s.categories {
                x.push_str(&cat_data(c, &s.values, wb, si));
            }
            x.push_str(&num_data("val", Some(&s.values), wb, si));
            if matches!(g, G::Line { .. }) {
                let _ = write!(x, "<c:smooth val=\"{}\"/>", s.smooth as u8);
            }
        }
    }
    x.push_str("</c:ser>");
    x
}

/// `<c:trendline>` with children in schema order: name, spPr, trendlineType, order, period,
/// forward, backward, intercept, dispRSqr, dispEq, trendlineLbl.
fn trendline_xml(t: &Trendline, wb: &Workbook) -> String {
    let mut x = String::from("<c:trendline>");
    if let Some(n) = &t.name {
        let _ = write!(x, "<c:name>{}</c:name>", esc(n));
    }
    let color = t.color.as_ref().and_then(|c| srgb(c, &wb.theme));
    if color.is_some() || t.width.is_some() || t.dash.is_some() {
        x.push_str("<c:spPr><a:ln");
        if let Some(w) = t.width.filter(|w| w.is_finite()) {
            // ST_LineWidth: 0..=20116800 EMU.
            let _ = write!(x, " w=\"{}\"", (f64::from(w) * EMU_PER_PT).round().clamp(0.0, 20_116_800.0) as i64);
        }
        x.push_str(" cap=\"rnd\">");
        if let Some(c) = &color {
            let _ = write!(x, "<a:solidFill>{c}</a:solidFill>");
        }
        if let Some(d) = t.dash.and_then(|d| DASHES.iter().find(|(k, _)| *k == d)) {
            let _ = write!(x, "<a:prstDash val=\"{}\"/>", d.1);
        }
        x.push_str("</a:ln></c:spPr>");
    }
    let ty = match t.kind {
        TrendlineKind::Linear => "linear",
        TrendlineKind::Exponential => "exp",
        TrendlineKind::Logarithmic => "log",
        TrendlineKind::Polynomial(_) => "poly",
        TrendlineKind::Power => "power",
        TrendlineKind::MovingAverage(_) => "movingAvg",
    };
    let _ = write!(x, "<c:trendlineType val=\"{ty}\"/>");
    match t.kind {
        TrendlineKind::Polynomial(o) => {
            let _ = write!(x, "<c:order val=\"{}\"/>", o.clamp(2, 6));
        }
        TrendlineKind::MovingAverage(p) => {
            let _ = write!(x, "<c:period val=\"{}\"/>", p.clamp(2, 255));
        }
        _ => {}
    }
    for (tag, v) in [("forward", t.forward), ("backward", t.backward)] {
        if v.is_finite() && v > 0.0 {
            let _ = write!(x, "<c:{tag} val=\"{}\"/>", num(v));
        }
    }
    if let Some(i) = t.intercept.filter(|i| i.is_finite()) {
        let _ = write!(x, "<c:intercept val=\"{}\"/>", num(i));
    }
    let _ = write!(x, "<c:dispRSqr val=\"{}\"/><c:dispEq val=\"{}\"/>", t.display_r_squared as u8, t.display_equation as u8);
    if t.display_r_squared || t.display_equation {
        x.push_str("<c:trendlineLbl><c:numFmt formatCode=\"General\" sourceLinked=\"0\"/></c:trendlineLbl>");
    }
    x.push_str("</c:trendline>");
    x
}

fn dlbls(show: bool) -> String {
    format!(
        "<c:dLbls><c:showLegendKey val=\"0\"/><c:showVal val=\"{}\"/><c:showCatName val=\"0\"/><c:showSerName val=\"0\"/><c:showPercent val=\"0\"/><c:showBubbleSize val=\"0\"/></c:dLbls>",
        show as u8
    )
}

/// Group `g` of `series`, each drawn as its kind in `kinds` (by series index).
fn group_xml(g: G, series: &[(usize, &Series)], kinds: &[ChartKind], ch: &Chart, axes: (u32, u32), wb: &Workbook, si: usize) -> String {
    let sers: String =
        series.iter().map(|(i, s)| series_xml(s, *i, g, kinds.get(*i).copied().unwrap_or(ChartKind::ColumnClustered), wb, si)).collect();
    let dl = dlbls(ch.data_labels);
    let ax = format!("<c:axId val=\"{}\"/><c:axId val=\"{}\"/>", axes.0, axes.1);
    match g {
        G::Bar { bar, grouping } => {
            let overlap = if grouping == "clustered" { String::new() } else { "<c:overlap val=\"100\"/>".into() };
            format!(
                "<c:barChart><c:barDir val=\"{}\"/><c:grouping val=\"{grouping}\"/><c:varyColors val=\"0\"/>{sers}{dl}<c:gapWidth val=\"150\"/>{overlap}{ax}</c:barChart>",
                if bar { "bar" } else { "col" }
            )
        }
        // Each series states its own markers; an empty group's say whether the chart has them.
        G::Line { stacked, markers } => format!(
            "<c:lineChart><c:grouping val=\"{}\"/><c:varyColors val=\"0\"/>{sers}{dl}<c:marker val=\"{}\"/>{ax}</c:lineChart>",
            if stacked { "stacked" } else { "standard" },
            u8::from(markers || !series.is_empty())
        ),
        G::Pie => format!("<c:pieChart><c:varyColors val=\"1\"/>{sers}{dl}<c:firstSliceAng val=\"{}\"/></c:pieChart>", ch.first_slice_angle % 360),
        G::Doughnut => format!(
            "<c:doughnutChart><c:varyColors val=\"1\"/>{sers}{dl}<c:firstSliceAng val=\"{}\"/><c:holeSize val=\"{}\"/></c:doughnutChart>",
            ch.first_slice_angle % 360,
            ch.hole_size.clamp(10, 90)
        ),
        G::Area { grouping } => format!("<c:areaChart><c:grouping val=\"{grouping}\"/><c:varyColors val=\"0\"/>{sers}{dl}{ax}</c:areaChart>"),
        // Each series states its own lines; an empty group's style says whether the chart has them.
        G::Scatter { lines } => {
            let style = if lines || !series.is_empty() { "lineMarker" } else { "marker" };
            format!("<c:scatterChart><c:scatterStyle val=\"{style}\"/><c:varyColors val=\"0\"/>{sers}{dl}{ax}</c:scatterChart>")
        }
        G::Bubble => {
            format!("<c:bubbleChart><c:varyColors val=\"0\"/>{sers}{dl}<c:bubbleScale val=\"100\"/><c:showNegBubbles val=\"0\"/>{ax}</c:bubbleChart>")
        }
        G::Radar => format!("<c:radarChart><c:radarStyle val=\"marker\"/><c:varyColors val=\"0\"/>{sers}{dl}{ax}</c:radarChart>"),
        G::Stock => format!("<c:stockChart>{sers}{dl}<c:hiLowLines/>{ax}</c:stockChart>"),
    }
}

fn title_xml(t: &str) -> String {
    let mut s = String::from("<c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/>");
    for line in t.split('\n') {
        let _ = write!(s, "<a:p><a:pPr><a:defRPr/></a:pPr><a:r><a:t>{}</a:t></a:r></a:p>", esc(line));
    }
    s.push_str("</c:rich></c:tx><c:overlay val=\"0\"/></c:title>");
    s
}

struct Ax<'a> {
    cat: bool,
    id: u32,
    cross: u32,
    pos: &'a str,
    delete: bool,
    grid: bool,
    title: Option<&'a str>,
    crosses_max: bool,
}

fn axis_xml(a: &Ax<'_>) -> String {
    let mut s = format!(
        "<c:{0}><c:axId val=\"{1}\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"{2}\"/><c:axPos val=\"{3}\"/>",
        if a.cat { "catAx" } else { "valAx" },
        a.id,
        a.delete as u8,
        a.pos
    );
    if a.grid {
        s.push_str("<c:majorGridlines/>");
    }
    if let Some(t) = a.title {
        s.push_str(&title_xml(t));
    }
    let _ = write!(
        s,
        "<c:numFmt formatCode=\"General\" sourceLinked=\"1\"/><c:majorTickMark val=\"out\"/><c:minorTickMark val=\"none\"/><c:tickLblPos val=\"nextTo\"/><c:crossAx val=\"{}\"/><c:crosses val=\"{}\"/>",
        a.cross,
        if a.crosses_max { "max" } else { "autoZero" }
    );
    if a.cat {
        s.push_str("<c:auto val=\"1\"/><c:lblAlgn val=\"ctr\"/><c:lblOffset val=\"100\"/><c:noMultiLvlLbl val=\"0\"/></c:catAx>");
    } else {
        s.push_str("<c:crossBetween val=\"between\"/></c:valAx>");
    }
    s
}

/// A PivotChart link without the workbook in brackets: `[Book.xlsx]Sheet2!PT` → `Sheet2!PT`.
fn strip_book(link: &str) -> &str {
    link.strip_prefix('[').and_then(|r| r.split_once(']')).map_or(link, |(_, rest)| rest)
}

/// Chart part XML for a model chart on sheet `si`.
pub fn write_chart(wb: &Workbook, si: usize, ch: &Chart, book_name: &str) -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><c:roundedCorners val=\"0\"/>",
    );
    if (1..=48).contains(&ch.style) {
        let _ = write!(s, "<c:style val=\"{}\"/>", ch.style);
    }
    // A PivotChart names its PivotTable behind the workbook in brackets (`[Book.xlsx]Sheet2!PivotTable1`);
    // without the brackets Excel shows an ordinary chart.
    if let Some(link) = ch.pivot.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
        let link = strip_book(link);
        let _ = write!(s, "<c:pivotSource><c:name>[{}]{}</c:name><c:fmtId val=\"0\"/></c:pivotSource>", esc(book_name), esc(link));
    }
    s.push_str("<c:chart>");
    match &ch.title {
        Some(t) if !t.is_empty() => {
            s.push_str(&title_xml(t));
            s.push_str("<c:autoTitleDeleted val=\"0\"/>");
        }
        _ => s.push_str("<c:autoTitleDeleted val=\"1\"/>"),
    }
    s.push_str("<c:plotArea><c:layout/>");
    let all: Vec<(usize, &Series)> = ch.series.iter().enumerate().collect();
    // A stock chart is one group of its 3 or 4 series (high-low-close, open-high-low-close);
    // other charts get one group per kind their series are drawn as, on each axis, in the order
    // they first appear (a line's markers and a scatter's lines are the series' own).
    let stock = ch.kind == ChartKind::Stock;
    let kinds = ch.series_kinds();
    let group_of = |i: usize| match group_for(kinds.get(i).copied().unwrap_or(ChartKind::ColumnClustered)) {
        G::Line { stacked, .. } => G::Line { stacked, markers: false },
        G::Scatter { .. } => G::Scatter { lines: true },
        g => g,
    };
    // A chart without axes (a pie or doughnut chart) has no secondary axes either: its series
    // are all one group. (A combination chart's pies are drawn as columns, on axes.)
    let has_axes = (0..all.len()).any(|i| axes_of(group_of(i)) != Axes::None);
    let groups = |secondary: bool| {
        let mut out: Vec<(G, Vec<(usize, &Series)>)> = Vec::new();
        for &(i, x) in all.iter().filter(|(_, x)| stock || (x.secondary && has_axes) == secondary) {
            let g = group_of(i);
            match out.iter_mut().find(|(k, _)| *k == g) {
                Some((_, members)) => members.push((i, x)),
                None => out.push((g, vec![(i, x)])),
            }
        }
        // The groups on one pair of axes share its kind of axes; one that can't (a pie beside
        // columns, a scatter beside lines) is drawn in the nearest group that can.
        // A combination chart is drawn on category axes (its series' kinds say how).
        let family = if ch.kind == ChartKind::Combo {
            Axes::Category
        } else {
            out.iter().map(|(g, _)| axes_of(*g)).find(|a| *a != Axes::None).unwrap_or(Axes::None)
        };
        // A stock group holds 3 or 4 series (high-low-close, open-high-low-close); others are
        // lines, which join a line group already there rather than setting its grouping.
        let (out, stock_lines): (Vec<_>, Vec<_>) = out.into_iter().partition(|(g, m)| *g != G::Stock || (3..=4).contains(&m.len()));
        let stock_lines = stock_lines.into_iter().map(|(_, m)| (G::Line { stacked: false, markers: false }, m));
        // One group per type on a pair of axes (as Excel keeps them; `Chart::series_kind` gives
        // the series of one type one grouping). Groups that fit the axes come first, so a group
        // moved onto them joins one of its type there rather than setting its grouping.
        let (fits, moved): (Vec<_>, Vec<_>) = out.into_iter().partition(|(g, _)| fit_axes(*g, family) == *g);
        let mut fitted: Vec<(G, Vec<(usize, &Series)>)> = Vec::new();
        let moved = moved.into_iter().chain(stock_lines).map(|(g, m)| (fit_axes(g, family), m));
        for (g, members) in fits.into_iter().chain(moved) {
            match fitted.iter_mut().find(|(k, _)| std::mem::discriminant(k) == std::mem::discriminant(&g)) {
                Some((_, m)) => m.extend(members),
                None => fitted.push((g, members)),
            }
        }
        // Each group's series in their own order.
        for (_, members) in &mut fitted {
            members.sort_by_key(|(i, _)| *i);
        }
        fitted
    };
    let (mut primary, mut sec) = if stock { (groups(false), Vec::new()) } else { (groups(false), groups(true)) };
    // With every series on the secondary axis, that axis is the only one.
    if primary.is_empty() {
        std::mem::swap(&mut primary, &mut sec);
    }
    // A chart without series still has a group of its kind, or it would not be read back (a
    // stock group must hold series, so an empty stock chart has a line group).
    if primary.is_empty() {
        let g = match group_for(ch.kind) {
            G::Stock => G::Line { stacked: false, markers: false },
            g => g,
        };
        primary.push((g, Vec::new()));
    }
    for (g, members) in &primary {
        s.push_str(&group_xml(*g, members, &kinds, ch, (1, 2), wb, si));
    }
    for (g, members) in &sec {
        s.push_str(&group_xml(*g, members, &kinds, ch, (3, 4), wb, si));
    }
    let family = |gs: &[(G, Vec<(usize, &Series)>)]| gs.iter().map(|(g, _)| axes_of(*g)).find(|a| *a != Axes::None).unwrap_or(Axes::None);
    let primary_axes = family(&primary);
    // The primary axes, placed by their kind.
    let (cat, cat_pos, val_pos) = match primary_axes {
        Axes::None => (true, "", ""),
        Axes::Values => (false, "b", "l"),
        Axes::Horizontal => (true, "l", "b"),
        Axes::Category => (true, "b", "l"),
    };
    if primary_axes != Axes::None {
        s.push_str(&axis_xml(&Ax {
            cat,
            id: 1,
            cross: 2,
            pos: cat_pos,
            delete: false,
            grid: false,
            title: ch.x_title.as_deref(),
            crosses_max: false,
        }));
        s.push_str(&axis_xml(&Ax {
            cat: false,
            id: 2,
            cross: 1,
            pos: val_pos,
            delete: false,
            grid: ch.gridlines,
            title: ch.y_title.as_deref(),
            crosses_max: false,
        }));
    }
    // The secondary axes: their categories (or x values) hidden, their values on the far side.
    let (cat, cat_pos, val_pos) = match family(&sec) {
        Axes::None => (true, "", ""),
        Axes::Values => (false, "b", "r"),
        Axes::Horizontal => (true, "l", "t"),
        Axes::Category => (true, "b", "r"),
    };
    if !cat_pos.is_empty() {
        s.push_str(&axis_xml(&Ax { cat, id: 3, cross: 4, pos: cat_pos, delete: true, grid: false, title: None, crosses_max: false }));
        s.push_str(&axis_xml(&Ax { cat: false, id: 4, cross: 3, pos: val_pos, delete: false, grid: false, title: None, crosses_max: true }));
    }
    if let Some(t) = &ch.data_table {
        let _ = write!(
            s,
            "<c:dTable><c:showHorzBorder val=\"{}\"/><c:showVertBorder val=\"{}\"/><c:showOutline val=\"{}\"/><c:showKeys val=\"{}\"/></c:dTable>",
            u8::from(t.horizontal_border),
            u8::from(t.vertical_border),
            u8::from(t.outline),
            u8::from(t.show_keys)
        );
    }
    s.push_str("</c:plotArea>");
    let pos = match ch.legend {
        LegendPos::None => None,
        LegendPos::Bottom => Some("b"),
        LegendPos::Top => Some("t"),
        LegendPos::Left => Some("l"),
        LegendPos::Right => Some("r"),
    };
    if let Some(p) = pos {
        let _ = write!(s, "<c:legend><c:legendPos val=\"{p}\"/><c:overlay val=\"0\"/></c:legend>");
    }
    s.push_str("<c:plotVisOnly val=\"1\"/><c:dispBlanksAs val=\"gap\"/></c:chart></c:chartSpace>");
    s
}
