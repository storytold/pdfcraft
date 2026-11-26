//! GridCraft charts (layer L3, toolkit-free).
//!
//! - [`resolve`] pulls series names, categories and values out of a workbook,
//! - [`render`] lays a chart out into a list of drawing primitives ([`Prim`]) that any UI
//!   toolkit (or the built-in [`rasterize`]) can paint,
//! - [`render_sparkline`] draws in-cell sparklines,
//! - [`rasterize`] turns primitives into RGBA pixels for PNG export and tests.
//!
//! The look (palette, greys, spacing) is GridCraft's own design.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod axis;
mod raster;
mod render;
mod resolve;
mod sparkline;
mod trend;

#[cfg(test)]
mod tests;

pub use gridcraft_model::{Chart, ChartKind, LegendPos, SparklineKind, Trendline, TrendlineKind};
use serde::{Deserialize, Serialize};

pub use axis::{Scale, format_number, nice_scale};
pub use raster::rasterize;
pub use render::{Swatch, legend_entries, render, supports_trendline};
pub use resolve::{default_palette, has_volatile_series, resolve, series_color};
pub use sparkline::render_sparkline;

/// Straight (non-premultiplied) RGBA colour.
pub type Rgba = [u8; 4];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HAlign {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VAlign {
    #[default]
    Top,
    Middle,
    Bottom,
}

/// A drawing primitive in chart coordinates (origin top-left, y down, units = points/pixels).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Prim {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fill: Option<Rgba>,
        stroke: Option<(Rgba, f32)>,
        radius: f32,
    },
    Line {
        pts: Vec<[f32; 2]>,
        color: Rgba,
        width: f32,
        dash: bool,
    },
    Polygon {
        pts: Vec<[f32; 2]>,
        fill: Rgba,
        stroke: Option<(Rgba, f32)>,
    },
    /// Pie/doughnut sector: centre, radii, angles in radians (0 = 12 o'clock, clockwise).
    Wedge {
        cx: f32,
        cy: f32,
        r_outer: f32,
        r_inner: f32,
        a0: f32,
        a1: f32,
        fill: Rgba,
        stroke: Option<(Rgba, f32)>,
    },
    Circle {
        cx: f32,
        cy: f32,
        r: f32,
        fill: Option<Rgba>,
        stroke: Option<(Rgba, f32)>,
    },
    /// Text anchored at `(x, y)` per `align`/`valign`, rotated by `rotation` radians about the anchor.
    Text {
        x: f32,
        y: f32,
        text: String,
        size: f32,
        color: Rgba,
        bold: bool,
        align: HAlign,
        valign: VAlign,
        rotation: f32,
    },
}

/// Measures text width (the UI passes real font metrics; tests use a fixed-width approximation).
pub trait Measure {
    fn text_width(&self, text: &str, size: f32, bold: bool) -> f32;
}

/// Fixed-width approximation: `0.55 × size` per character (bold 10% wider).
#[derive(Clone, Copy, Debug, Default)]
pub struct ApproxMeasure;

impl Measure for ApproxMeasure {
    fn text_width(&self, text: &str, size: f32, bold: bool) -> f32 {
        let n = text.chars().count() as f32;
        n * 0.55 * size * if bold { 1.1 } else { 1.0 }
    }
}

/// Resolved data: series names, categories and values pulled from the workbook.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChartData {
    pub categories: Vec<String>,
    /// Outer levels of a multi-level category axis (a category range two columns wide beside a
    /// column of values: year beside month), nearest the labels first. One entry per category,
    /// non-empty where a group starts: `["2023", "", "", "2024", "", ""]`.
    #[serde(default)]
    pub category_levels: Vec<Vec<String>>,
    pub series: Vec<SeriesData>,
    /// Workbook-theme colours for the points of pie-like charts, repeating (point `j` uses entry `j % len`); empty uses the default theme.
    #[serde(default)]
    pub palette: Vec<Rgba>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeriesData {
    pub name: String,
    pub values: Vec<Option<f64>>,
    /// Numeric X values (scatter, bubble).
    pub x: Option<Vec<Option<f64>>>,
    /// Bubble sizes.
    pub sizes: Option<Vec<Option<f64>>>,
    pub color: Rgba,
    /// The kind this series is drawn as. `resolve` sets it from `Chart::series_kind`. `render`
    /// reads it for every series of a vertical cartesian or scatter chart, so a hand-built
    /// `ChartData` sets it too (`SeriesData::default()` gives `ColumnClustered`).
    pub kind: ChartKind,
    /// Plotted against the secondary (right) value axis.
    pub secondary: bool,
    /// Draw line segments as a smoothed (spline) curve.
    pub smooth: bool,
    /// Trendlines to fit and draw over this series.
    pub trendlines: Vec<TrendlineData>,
    /// Number format code of the source values ("General" by default).
    pub number_format: String,
}

/// A trendline to draw: the model settings with the line colour resolved against the theme.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrendlineData {
    pub spec: Trendline,
    pub color: Rgba,
}

impl Default for SeriesData {
    fn default() -> Self {
        SeriesData {
            name: String::new(),
            values: vec![],
            x: None,
            sizes: None,
            color: [0x15, 0x60, 0x82, 0xFF],
            kind: ChartKind::ColumnClustered,
            secondary: false,
            smooth: false,
            trendlines: vec![],
            number_format: "General".into(),
        }
    }
}
