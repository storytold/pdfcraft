//! Office 2016 chart types (histogram, box & whisker, waterfall, funnel, treemap, sunburst),
//! which files keep as `cx:chartSpace` "chartex" parts (`xl/charts/chartExN.xml`) rather than
//! DrawingML chart parts.

use std::fmt::Write as _;

use gridcraft_core::RangeRef;
use gridcraft_formula::{Expr, SheetSel};
use gridcraft_model::{Chart, ChartKind, LegendPos, QuartileMethod, Series, Workbook};

use crate::drawing::srgb;
use crate::xml::{El, esc, esc_attr};

pub const CT_CHARTEX: &str = "application/vnd.ms-office.chartex+xml";
pub const REL_CHARTEX: &str = "http://schemas.microsoft.com/office/2014/relationships/chartEx";
pub const CT_STYLE: &str = "application/vnd.ms-office.chartstyle+xml";
pub const CT_COLORS: &str = "application/vnd.ms-office.chartcolorstyle+xml";
pub const REL_STYLE: &str = "http://schemas.microsoft.com/office/2011/relationships/chartStyle";
pub const REL_COLORS: &str = "http://schemas.microsoft.com/office/2011/relationships/chartColorStyle";

/// Kinds written as chartex parts.
pub fn is_chartex(k: ChartKind) -> bool {
    use craft_drawingml::chart::ChartType;
    match k {
        ChartKind::Histogram => ChartType::Histogram.is_chartex(),
        ChartKind::BoxWhisker => ChartType::BoxWhisker.is_chartex(),
        ChartKind::Waterfall => ChartType::Waterfall.is_chartex(),
        ChartKind::Funnel => ChartType::Funnel.is_chartex(),
        ChartKind::Treemap => ChartType::Treemap.is_chartex(),
        ChartKind::Sunburst => ChartType::Sunburst.is_chartex(),
        _ => false,
    }
}

/// The `mc:Choice` requirement for a kind's drawing frame: (prefix, namespace). Funnel charts
/// came in a later revision of the format.
pub fn requires(k: ChartKind) -> (&'static str, &'static str) {
    match k {
        ChartKind::Funnel => ("cx2", "http://schemas.microsoft.com/office/drawing/2015/10/21/chartex"),
        _ => ("cx1", "http://schemas.microsoft.com/office/drawing/2015/9/8/chartex"),
    }
}

// ---------------------------------------------------------------- reading

fn chartex_formula(dim: &El) -> Option<String> {
    let f = dim.child("f")?;
    Some(crate::fmla::from_file(&crate::chart::chart_ref_from_file(&f.text)))
}

fn chartex_text(tx: &El) -> Option<String> {
    if let Some(d) = tx.child("txData") {
        if let Some(f) = d.child("f") {
            return Some(crate::chart::chart_ref_from_file(&f.text));
        }
        return d.child("v").map(|v| v.text.clone());
    }
    let rich = tx.child("rich")?;
    let paras: Vec<String> = rich.kids("p").map(|p| p.kids("r").filter_map(|r| r.child("t")).map(|t| t.text.as_str()).collect()).collect();
    let s = paras.join("\n");
    if s.is_empty() { None } else { Some(s) }
}

pub(crate) fn from_dml_chartex(dc: craft_drawingml::chart::Chart, root: Option<&El>) -> Option<Chart> {
    use craft_drawingml::chart::ChartType;
    let kind = match dc.kind {
        ChartType::Histogram | ChartType::Pareto => ChartKind::Histogram,
        ChartType::BoxWhisker => ChartKind::BoxWhisker,
        ChartType::Waterfall => ChartKind::Waterfall,
        ChartType::Funnel => ChartKind::Funnel,
        ChartType::Treemap => ChartKind::Treemap,
        ChartType::Sunburst => ChartKind::Sunburst,
        _ => return None,
    };
    let default_quartiles = if dc.inclusive_quartiles { QuartileMethod::Inclusive } else { QuartileMethod::Exclusive };
    let data: Vec<(String, Option<String>, Option<String>)> = root
        .and_then(|r| r.child("chartData"))
        .map(|cd| {
            cd.kids("data")
                .map(|d| {
                    let id = d.attr("id").unwrap_or("").to_string();
                    let cat =
                        d.kids("strDim").chain(d.kids("numDim")).find(|x| matches!(x.attr("type"), Some("cat" | "x"))).and_then(chartex_formula);
                    let val = d.kids("numDim").find(|x| matches!(x.attr("type"), Some("val" | "size" | "y"))).and_then(chartex_formula);
                    (id, cat, val)
                })
                .collect()
        })
        .unwrap_or_default();
    let series_els: Vec<&El> = root
        .and_then(|r| r.path(&["chart", "plotArea", "plotAreaRegion"]))
        .map(|region| region.kids("series").filter(|s| s.attr("layoutId") != Some("paretoLine")).collect())
        .unwrap_or_default();
    let series = dc
        .series
        .into_iter()
        .enumerate()
        .map(|(idx, s)| {
            let sel = series_els.get(idx).copied();
            let (cat, val) = sel
                .and_then(|el| el.child_val("dataId"))
                .and_then(|id| data.iter().find(|(i, _, _)| i == id))
                .map(|(_, c, v)| (c.clone(), v.clone()))
                .unwrap_or_default();
            let color = s
                .fill
                .as_ref()
                .and_then(|f| match f {
                    craft_drawingml::style::Fill::Solid { color } => crate::chart::map_dml_color(color),
                    _ => None,
                })
                .or_else(|| sel.and_then(|el| el.path(&["spPr", "solidFill"])).and_then(crate::drawing::dml_color))
                .filter(|c| *c != gridcraft_model::Color::Theme(4 + (idx % 6) as u8, 0));
            let quartiles = sel
                .and_then(|el| el.path(&["layoutPr", "statistics"]))
                .and_then(|st| st.attr("quartileMethod"))
                .map(|m| if m == "inclusive" { QuartileMethod::Inclusive } else { QuartileMethod::Exclusive })
                .unwrap_or(default_quartiles);
            let name = sel
                .and_then(|el| el.child("tx"))
                .and_then(chartex_text)
                .or_else(|| s.refs.name.as_deref().map(crate::chart::chart_ref_from_file))
                .or_else(|| (!s.name.is_empty()).then_some(s.name));
            let categories = cat.or_else(|| s.refs.categories.as_deref().map(|f| crate::fmla::from_file(&crate::chart::chart_ref_from_file(f))));
            let values =
                val.or_else(|| s.refs.values.as_deref().map(|f| crate::fmla::from_file(&crate::chart::chart_ref_from_file(f)))).unwrap_or_default();
            Series { quartiles, name, categories, values, color, ..Default::default() }
        })
        .collect();
    let legend = match dc.legend.as_deref() {
        None => LegendPos::None,
        Some("b") => LegendPos::Bottom,
        Some("t") => LegendPos::Top,
        Some("l") => LegendPos::Left,
        Some(_) => LegendPos::Right,
    };
    let axis_title = |is_val: bool| -> Option<String> {
        let pa = root?.path(&["chart", "plotArea"])?;
        let ax = pa.kids("axis").find(|a| if is_val { a.child("valScaling").is_some() } else { a.child("catScaling").is_some() })?;
        let title = ax.child("title")?;
        if let Some(v) = title.path(&["tx", "txData", "v"]).map(|v| v.text.trim()).filter(|t| !t.is_empty()) {
            return Some(v.to_string());
        }
        let rich = title.path(&["tx", "rich"])?;
        let text: String = rich
            .kids("p")
            .map(|p| p.kids("r").filter_map(|r| r.child("t")).map(|t| t.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        (!text.trim().is_empty()).then_some(text)
    };
    Some(Chart {
        id: 0,
        kind,
        anchor: Default::default(),
        title: dc.title,
        series,
        legend,
        data_labels: dc.data_labels,
        data_table: None,
        gridlines: dc.gridlines,
        style: 2,
        hole_size: 75,
        first_slice_angle: 0,
        x_title: dc.axis_titles.0.or_else(|| axis_title(false)),
        y_title: dc.axis_titles.1.or_else(|| axis_title(true)),
        source: None,
        by_rows: false,
        pivot: None,
    })
}

pub fn read_chartex(root: &El) -> Option<Chart> {
    let mut craft = crate::chart::to_craft_el(root);
    if craft.child(craft_xml::Local("chartData")).is_none() {
        craft.push(craft_xml::El::new("chartData").with_child(craft_xml::El::new("data").with_attr("id", "0")));
    }
    let dc = craft_drawingml::chart::chartex::chartex_from(&craft)?;
    from_dml_chartex(dc, Some(root))
}

// ---------------------------------------------------------------- writing

/// Prefix of the hidden defined names chartex formulas go through: a chartex part refers to
/// its data by names (`_xlchart.v1.0`), which the workbook defines, not by references.
pub const NAME_PREFIX: &str = "_xlchart.v1.";

/// `<cx:f>` for a reference: a new hidden workbook name for it (collected in `names`, written
/// with the workbook's defined names). A reference along a row says so; otherwise each column
/// is one level.
fn f_xml(wb: &Workbook, si: usize, f: &str, names: &mut Vec<String>) -> String {
    let row = ref_range(wb, si, f).is_some_and(|(_, r)| r.height() == 1 && r.width() > 1);
    names.push(f.to_string());
    format!("<cx:f{}>{NAME_PREFIX}{}</cx:f>", if row { " dir=\"row\"" } else { "" }, names.len() - 1)
}

/// A reference's sheet and range (for the row/column direction).
fn ref_range(wb: &Workbook, si: usize, f: &str) -> Option<(usize, RangeRef)> {
    let Ok(Expr::Ref(r)) = gridcraft_formula::parse(f) else { return None };
    let sheet = match &r.sheet {
        SheetSel::Current => si,
        SheetSel::Named(n) => wb.sheet_index(n)?,
        SheetSel::Span(..) => return None,
    };
    Some((sheet, r.range()))
}

fn tx_xml(t: &str, wb: &Workbook, si: usize, names: &mut Vec<String>) -> String {
    if ref_range(wb, si, t).is_some() {
        let v = crate::chart::ref_values(wb, si, t).and_then(|v| v.into_iter().next()).map(|v| v.display()).unwrap_or_default();
        format!("<cx:tx><cx:txData>{}<cx:v>{}</cx:v></cx:txData></cx:tx>", f_xml(wb, si, t, names), esc(&v))
    } else {
        let lit = t.trim().strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(t);
        format!("<cx:tx><cx:txData><cx:v>{}</cx:v></cx:txData></cx:tx>", esc(lit))
    }
}

/// The chart style part a chartex part comes with: every element at its plain default.
pub fn style_xml() -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<cs:chartStyle xmlns:cs=\"http://schemas.microsoft.com/office/drawing/2012/chartStyle\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" id=\"201\">",
    );
    for e in [
        "axisTitle",
        "categoryAxis",
        "chartArea",
        "dataLabel",
        "dataLabelCallout",
        "dataPoint",
        "dataPoint3D",
        "dataPointLine",
        "dataPointMarker",
        "dataPointMarkerLayout",
        "dataPointWireframe",
        "dataTable",
        "downBar",
        "dropLine",
        "errorBar",
        "floor",
        "gridlineMajor",
        "gridlineMinor",
        "hiLoLine",
        "leaderLine",
        "legend",
        "plotArea",
        "plotArea3D",
        "seriesAxis",
        "seriesLine",
        "title",
        "trendline",
        "trendlineLabel",
        "upBar",
        "valueAxis",
        "wall",
    ] {
        if e == "dataPointMarkerLayout" {
            s.push_str("<cs:dataPointMarkerLayout symbol=\"circle\" size=\"5\"/>");
        } else {
            let _ = write!(
                s,
                "<cs:{e}><cs:lnRef idx=\"0\"/><cs:fillRef idx=\"0\"/><cs:effectRef idx=\"0\"/><cs:fontRef idx=\"minor\"><a:schemeClr val=\"tx1\"/></cs:fontRef></cs:{e}>"
            );
        }
    }
    s.push_str("</cs:chartStyle>");
    s
}

/// The colour style part a chartex part comes with: the theme accents in turn.
pub fn colors_xml() -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<cs:colorStyle xmlns:cs=\"http://schemas.microsoft.com/office/drawing/2012/chartStyle\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" meth=\"cycle\" id=\"10\">",
    );
    for i in 1..=6 {
        let _ = write!(s, "<a:schemeClr val=\"accent{i}\"/>");
    }
    s.push_str("<cs:variation/></cs:colorStyle>");
    s
}

/// A solid fill (and outline, for line-drawn parts like whiskers) as `cx:spPr`.
fn fill_xml(color: &str, line: bool) -> String {
    let ln = if line { format!("<a:ln w=\"9525\"><a:solidFill>{color}</a:solidFill></a:ln>") } else { String::new() };
    format!("<cx:spPr><a:solidFill>{color}</a:solidFill>{ln}</cx:spPr>")
}

/// Light grey for gridlines and the chart border: the text colour at a quarter strength.
const GREY: &str = "<a:schemeClr val=\"tx1\"><a:lumMod val=\"25000\"/><a:lumOff val=\"75000\"/></a:schemeClr>";

fn accent(i: usize) -> String {
    format!("<a:schemeClr val=\"accent{}\"/>", i % 6 + 1)
}

/// Chartex part XML for a model chart on sheet `si`. The references it uses are appended to
/// `names` (see [`NAME_PREFIX`]).
pub fn write_chartex(wb: &Workbook, si: usize, ch: &Chart, names: &mut Vec<String>) -> String {
    let hierarchy = matches!(ch.kind, ChartKind::Treemap | ChartKind::Sunburst);
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<cx:chartSpace xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:cx=\"http://schemas.microsoft.com/office/drawing/2014/chartex\"><cx:chartData>",
    );
    for (i, ser) in ch.series.iter().enumerate() {
        let _ = write!(s, "<cx:data id=\"{i}\">");
        if let Some(c) = ser.categories.as_deref().filter(|_| ch.kind != ChartKind::Histogram) {
            let _ = write!(s, "<cx:strDim type=\"cat\">{}</cx:strDim>", f_xml(wb, si, c, names));
        }
        let _ = write!(s, "<cx:numDim type=\"{}\">{}</cx:numDim>", if hierarchy { "size" } else { "val" }, f_xml(wb, si, &ser.values, names));
        s.push_str("</cx:data>");
    }
    s.push_str("</cx:chartData><cx:chart>");
    if let Some(t) = ch.title.as_deref().filter(|t| !t.is_empty()) {
        let _ = write!(s, "<cx:title pos=\"t\" align=\"ctr\" overlay=\"0\"><cx:tx><cx:txData><cx:v>{}</cx:v></cx:txData></cx:tx></cx:title>", esc(t));
    }
    s.push_str("<cx:plotArea><cx:plotAreaRegion>");
    let layout = match ch.kind {
        ChartKind::BoxWhisker => "boxWhisker",
        ChartKind::Waterfall => "waterfall",
        ChartKind::Funnel => "funnel",
        ChartKind::Treemap => "treemap",
        ChartKind::Sunburst => "sunburst",
        _ => "clusteredColumn",
    };
    for (i, ser) in ch.series.iter().enumerate() {
        let _ = write!(s, "<cx:series layoutId=\"{layout}\" uniqueId=\"{{00000000-0000-0000-0000-{:08X}{i:04X}}}\">", ch.id);
        if let Some(n) = &ser.name {
            s.push_str(&tx_xml(n, wb, si, names));
        }
        // Fills are written out: they don't come from the style part. Waterfall decreases and
        // treemap/sunburst boxes get their own colours.
        let color = ser.color.as_ref().and_then(|c| srgb(c, &wb.theme)).unwrap_or_else(|| accent(i));
        s.push_str(&fill_xml(&color, ch.kind == ChartKind::BoxWhisker));
        let points = || crate::chart::ref_values(wb, si, &ser.values).unwrap_or_default().into_iter().take(10_000).enumerate();
        match ch.kind {
            ChartKind::Waterfall => {
                for (j, v) in points() {
                    if v.as_f64().is_some_and(|n| n < 0.0) {
                        let _ = write!(s, "<cx:dataPt idx=\"{j}\">{}</cx:dataPt>", fill_xml(&accent(i + 1), false));
                    }
                }
            }
            ChartKind::Treemap | ChartKind::Sunburst if ser.color.is_none() => {
                for (j, _) in points() {
                    let _ = write!(s, "<cx:dataPt idx=\"{j}\">{}</cx:dataPt>", fill_xml(&accent(j), false));
                }
            }
            _ => {}
        }
        if ch.data_labels {
            s.push_str("<cx:dataLabels><cx:visibility seriesName=\"0\" categoryName=\"0\" value=\"1\"/></cx:dataLabels>");
        }
        let _ = write!(s, "<cx:dataId val=\"{i}\"/>");
        match ch.kind {
            ChartKind::Histogram => s.push_str("<cx:layoutPr><cx:binning intervalClosed=\"r\"/></cx:layoutPr>"),
            ChartKind::BoxWhisker => {
                let method = match ser.quartiles {
                    QuartileMethod::Exclusive => "exclusive",
                    QuartileMethod::Inclusive => "inclusive",
                };
                let _ = write!(
                    s,
                    "<cx:layoutPr><cx:visibility meanLine=\"0\" meanMarker=\"1\" nonoutliers=\"0\" outliers=\"1\"/><cx:statistics quartileMethod=\"{method}\"/></cx:layoutPr>"
                );
            }
            ChartKind::Treemap => s.push_str("<cx:layoutPr><cx:parentLabelLayout val=\"overlapping\"/></cx:layoutPr>"),
            _ => {}
        }
        s.push_str("</cx:series>");
    }
    s.push_str("</cx:plotAreaRegion>");
    let gap = match ch.kind {
        ChartKind::Histogram => Some("0"),
        ChartKind::BoxWhisker => Some("1"),
        ChartKind::Waterfall => Some("0.5"),
        ChartKind::Funnel => Some("0.06"),
        _ => None,
    };
    let title = |t: Option<&str>| {
        t.filter(|t| !t.is_empty())
            .map(|t| format!("<cx:title><cx:tx><cx:txData><cx:v>{}</cx:v></cx:txData></cx:tx></cx:title>", esc(t)))
            .unwrap_or_default()
    };
    if let Some(gap) = gap {
        let _ =
            write!(s, "<cx:axis id=\"0\"><cx:catScaling gapWidth=\"{}\"/>{}<cx:tickLabels/></cx:axis>", esc_attr(gap), title(ch.x_title.as_deref()));
        if ch.kind != ChartKind::Funnel {
            let _ = write!(
                s,
                "<cx:axis id=\"1\"><cx:valScaling/>{}{}<cx:tickLabels/></cx:axis>",
                title(ch.y_title.as_deref()),
                if ch.gridlines {
                    format!("<cx:majorGridlines><cx:spPr><a:ln w=\"9525\"><a:solidFill>{GREY}</a:solidFill></a:ln></cx:spPr></cx:majorGridlines>")
                } else {
                    String::new()
                }
            );
        }
    }
    s.push_str("</cx:plotArea>");
    let pos = match ch.legend {
        LegendPos::None => None,
        LegendPos::Bottom => Some("b"),
        LegendPos::Top => Some("t"),
        LegendPos::Left => Some("l"),
        LegendPos::Right => Some("r"),
    };
    if let Some(p) = pos {
        let _ = write!(s, "<cx:legend pos=\"{p}\" align=\"ctr\" overlay=\"0\"/>");
    }
    // A white chart area with a light border, like other charts.
    let _ = write!(
        s,
        "</cx:chart><cx:spPr><a:solidFill><a:schemeClr val=\"bg1\"/></a:solidFill><a:ln w=\"9525\"><a:solidFill>{GREY}</a:solidFill></a:ln></cx:spPr></cx:chartSpace>"
    );
    s
}

#[cfg(test)]
mod quartile_tests {
    use super::*;

    #[test]
    fn box_whisker_reads_quartile_method_and_defaults() {
        for (statistics, expected) in [
            ("", QuartileMethod::Exclusive),
            ("<cx:statistics/>", QuartileMethod::Exclusive),
            (r#"<cx:statistics quartileMethod="unknown"/>"#, QuartileMethod::Exclusive),
            (r#"<cx:statistics quartileMethod="exclusive"/>"#, QuartileMethod::Exclusive),
            (r#"<cx:statistics quartileMethod="inclusive"/>"#, QuartileMethod::Inclusive),
        ] {
            let xml = format!(
                r#"<cx:chartSpace xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex"><cx:chart><cx:plotArea><cx:plotAreaRegion><cx:series layoutId="boxWhisker"><cx:layoutPr>{statistics}</cx:layoutPr></cx:series></cx:plotAreaRegion></cx:plotArea></cx:chart></cx:chartSpace>"#
            );
            let root = crate::xml::parse(xml.as_bytes()).unwrap();
            let chart = read_chartex(&root).unwrap();
            assert_eq!(chart.series[0].quartiles, expected);
        }
    }
}
