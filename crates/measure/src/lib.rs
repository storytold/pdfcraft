//! Calibrated 2D measurements in PDF user space (ISO 32000-2 §12.9, measurement properties).
//! Viewports and annotations carry standard rectilinear /Measure dictionaries; the
//! annotations use the measurement intents of line, polygon and polyline annotations (§12.5.6).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod snap;
use pdfcraft_annot::{Meta, NewAnnotation, Shape, Style};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString};
use serde::Serialize;

pub type Point = [f64; 2];
pub const MAX_POINTS: usize = 2048;
const MAX_COORD: f64 = 1e9;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum MeasureError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Annotation(#[from] pdfcraft_annot::AnnotError),
}
type Result<T> = std::result::Result<T, MeasureError>;
fn invalid(s: &str) -> MeasureError {
    MeasureError::Invalid(s.into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Distance,
    Perimeter,
    Area,
}
impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Distance => "distance",
            Self::Perimeter => "perimeter",
            Self::Area => "area",
        }
    }
    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "distance" => Some(Self::Distance),
            "perimeter" => Some(Self::Perimeter),
            "area" => Some(Self::Area),
            _ => None,
        }
    }
    fn intent(self) -> &'static str {
        match self {
            Self::Distance => "LineDimension",
            Self::Perimeter => "PolyLineDimension",
            Self::Area => "PolygonDimension",
        }
    }
}

/// Conversion from PDF user units to real-world units. Y can use a different scale.
/// Distance and area conversion factors are relative to X (the rectilinear measure
/// dictionary, ISO 32000-2 §12.9).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Scale {
    pub x: f64,
    pub y: f64,
    pub unit: String,
    pub distance_factor: f64,
    pub area_factor: f64,
    pub area_unit: String,
    pub precision: u8,
    pub ratio: String,
}
impl Default for Scale {
    fn default() -> Self {
        Self {
            x: 1.0 / 72.0,
            y: 1.0 / 72.0,
            unit: "in".into(),
            distance_factor: 1.0,
            area_factor: 1.0,
            area_unit: "in^2".into(),
            precision: 2,
            ratio: "1 in = 1 in".into(),
        }
    }
}
impl Scale {
    pub fn new(units_per_user_unit: f64, unit: &str, precision: u8) -> Result<Self> {
        let out = Self {
            x: units_per_user_unit,
            y: units_per_user_unit,
            unit: unit.into(),
            distance_factor: 1.0,
            area_factor: 1.0,
            area_unit: format!("{unit}^2"),
            precision,
            ratio: format!("1 pt = {units_per_user_unit} {unit}"),
        };
        out.validate()?;
        Ok(out)
    }
    pub fn calibrate(a: Point, b: Point, real_distance: f64, unit: &str, precision: u8) -> Result<Self> {
        check_points(&[a, b])?;
        if !real_distance.is_finite() || real_distance <= 0.0 {
            return Err(invalid("calibration distance must be finite and positive"));
        }
        let length = distance(a, b);
        if length < 1e-6 {
            return Err(invalid("calibration needs two distinct points"));
        }
        Self::new(real_distance / length, unit, precision)
    }
    pub fn validate(&self) -> Result<()> {
        for f in [self.x, self.y, self.distance_factor, self.area_factor] {
            if !f.is_finite() || !(1e-12..=1e12).contains(&f) {
                return Err(invalid("scale factors must be finite and between 1e-12 and 1e12"));
            }
        }
        if self.precision > 6 {
            return Err(invalid("precision must be between 0 and 6 decimal places"));
        }
        // Units are text strings, so non-ASCII labels such as "m²" are allowed.
        for s in [&self.unit, &self.area_unit] {
            let n = s.chars().count();
            if s.trim().is_empty() || n > 24 || s.chars().any(char::is_control) {
                return Err(invalid("unit labels must be 1 to 24 printable characters"));
            }
        }
        if self.ratio.len() > 256 || self.ratio.chars().any(char::is_control) {
            return Err(invalid("invalid scale ratio"));
        }
        Ok(())
    }
    pub fn dictionary(&self) -> Result<Dict> {
        self.validate()?;
        let format = |unit: &str, factor: f64| {
            let mut d = Dict::new();
            d.set(b"Type".to_vec(), Object::name("NumberFormat"));
            d.set(b"U".to_vec(), PdfString::text(unit));
            d.set(b"C".to_vec(), Object::Real(factor));
            d.set(b"F".to_vec(), Object::name(if self.precision == 0 { "R" } else { "D" }));
            if self.precision > 0 {
                d.set(b"D".to_vec(), Object::Int(10_i64.pow(u32::from(self.precision))));
            }
            d.set(b"FD".to_vec(), Object::Bool(true));
            d.set(b"SS".to_vec(), PdfString::text(""));
            Object::Array(vec![Object::Dict(d)])
        };
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("Measure"));
        d.set(b"Subtype".to_vec(), Object::name("RL"));
        d.set(b"R".to_vec(), PdfString::text(&self.ratio));
        d.set(b"X".to_vec(), format(&self.unit, self.x));
        if self.x != self.y {
            d.set(b"Y".to_vec(), format(&self.unit, self.y));
            d.set(b"CYX".to_vec(), Object::Real(1.0));
        }
        d.set(b"D".to_vec(), format(&self.unit, self.distance_factor));
        d.set(b"A".to_vec(), format(&self.area_unit, self.area_factor));
        Ok(d)
    }
    /// Single-unit decimal scales are editable. Compound/fractional and non-rectilinear
    /// imports are refused explicitly; their dictionaries and appearance remain untouched.
    pub fn read(doc: &Document, object: &Object) -> Result<Self> {
        let o = doc.resolve(object);
        let d = o.as_dict().ok_or_else(|| invalid("invalid measure dictionary"))?;
        if d.name(b"Subtype").is_some_and(|n| n != b"RL") {
            return Err(invalid("only rectilinear measurement scales are supported"));
        }
        let format = |key: &[u8]| -> Result<(String, f64, u8)> {
            let a = doc.resolve(d.get(key).ok_or_else(|| invalid("measurement scale lacks a number format"))?);
            let a = a.as_array().ok_or_else(|| invalid("invalid measurement number format"))?;
            if a.len() != 1 {
                return Err(invalid("compound measurement units are not supported yet"));
            }
            let o = doc.resolve(a.first().ok_or_else(|| invalid("empty number format"))?);
            let f = o.as_dict().ok_or_else(|| invalid("invalid number format dictionary"))?;
            let precision = match f.name(b"F") {
                Some(b"R") => 0,
                None | Some(b"D") => {
                    let denom = f.int(b"D").unwrap_or(100);
                    (0..=6).find(|p| 10_i64.pow(*p) == denom).ok_or_else(|| invalid("unsupported measurement precision"))?
                }
                _ => return Err(invalid("fractional measurement formats are not supported yet")),
            };
            Ok((text(f, b"U"), f.get(b"C").and_then(Object::as_f64).ok_or_else(|| invalid("missing conversion factor"))?, precision as u8))
        };
        let (unit, x, _) = format(b"X")?;
        let y = if d.contains(b"Y") {
            let (_, y, _) = format(b"Y")?;
            y * d.get(b"CYX").and_then(Object::as_f64).ok_or_else(|| invalid("Y scale requires CYX for lengths and areas"))?
        } else {
            x
        };
        let (distance_unit, distance_factor, precision) = format(b"D")?;
        if unit != distance_unit {
            return Err(invalid("different coordinate and distance units are not supported yet"));
        }
        let (area_unit, area_factor, _) = format(b"A")?;
        let out = Self { x, y, unit, distance_factor, area_factor, area_unit, precision, ratio: text(d, b"R") };
        out.validate()?;
        Ok(out)
    }
}
pub(crate) fn check_points(points: &[Point]) -> Result<()> {
    if points.len() > MAX_POINTS || points.iter().flatten().any(|v| !v.is_finite() || v.abs() > MAX_COORD) {
        return Err(invalid("measurement geometry must be finite, within 1e9 user units, and have at most 2048 vertices"));
    }
    Ok(())
}
pub fn distance(a: Point, b: Point) -> f64 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Reading {
    pub kind: Kind,
    pub value: f64,
    pub unit: String,
    pub label: String,
    pub delta_x: f64,
    pub delta_y: f64,
    pub angle: f64,
    pub length: f64,
    pub area: f64,
}
/// Live reading allows incomplete paths; adding a saved annotation validates its vertex count.
pub fn reading(kind: Kind, points: &[Point], scale: &Scale) -> Result<Reading> {
    check_points(points)?;
    scale.validate()?;
    let mut length = 0.0;
    for w in points.windows(2) {
        if let [a, b] = w {
            length += ((b[0] - a[0]) * scale.x).hypot((b[1] - a[1]) * scale.y);
        }
    }
    let area = if points.len() >= 3 {
        let origin = points.first().copied().unwrap_or_default();
        points
            .iter()
            .zip(points.iter().cycle().skip(1))
            .take(points.len())
            .map(|(a, b)| ((a[0] - origin[0]) * (b[1] - origin[1]) - (b[0] - origin[0]) * (a[1] - origin[1])) * 0.5)
            .sum::<f64>()
            .abs()
            * scale.x
            * scale.y
            * scale.area_factor
    } else {
        0.0
    };
    let delta = match (points.first(), points.last()) {
        (Some(a), Some(b)) => [(b[0] - a[0]) * scale.x, (b[1] - a[1]) * scale.y],
        _ => [0.0; 2],
    };
    length *= scale.distance_factor;
    let value = if kind == Kind::Area { area } else { length };
    let unit = if kind == Kind::Area { scale.area_unit.clone() } else { scale.unit.clone() };
    let label = format!("{value:.precision$} {unit}", precision = usize::from(scale.precision));
    if !value.is_finite() {
        return Err(invalid("measurement overflows its scale"));
    }
    Ok(Reading { kind, value, unit, label, delta_x: delta[0], delta_y: delta[1], angle: delta[1].atan2(delta[0]).to_degrees(), length, area })
}
#[derive(Clone, Debug, PartialEq)]
pub struct NewMeasurement {
    pub page: usize,
    pub kind: Kind,
    pub points: Vec<Point>,
    pub scale: Scale,
    pub style: Style,
    pub label: String,
    pub author: String,
}
/// An area's vertices without a repeated closing vertex (last == first), which some
/// writers include; the closing edge is implicit.
pub fn open_polygon(points: &[Point]) -> &[Point] {
    match points {
        [first, rest @ .., last] if rest.len() >= 2 && distance(*first, *last) < 1e-6 => points.get(..points.len() - 1).unwrap_or(points),
        _ => points,
    }
}
fn validate_geometry(kind: Kind, points: &[Point]) -> Result<()> {
    check_points(points)?;
    let points = if kind == Kind::Area { open_polygon(points) } else { points };
    match kind {
        Kind::Distance if points.len() != 2 => return Err(invalid("distance needs exactly two points")),
        Kind::Perimeter if points.len() < 2 => return Err(invalid("perimeter needs at least two points")),
        Kind::Area if points.len() < 3 => return Err(invalid("area needs at least three points")),
        _ => {}
    }
    if points.windows(2).any(|p| matches!(p,[a,b] if distance(*a,*b)<1e-6)) {
        return Err(invalid("adjacent measurement vertices must be distinct"));
    }
    if kind == Kind::Area {
        for (i, (a, b)) in points.iter().zip(points.iter().cycle().skip(1)).take(points.len()).enumerate() {
            for (j, (c, d)) in points.iter().zip(points.iter().cycle().skip(1)).take(points.len()).enumerate().skip(i + 2) {
                if i == 0 && j + 1 == points.len() {
                    continue;
                }
                if snap::intersection(*a, *b, *c, *d).is_some() {
                    return Err(invalid("area boundary must not intersect itself"));
                }
            }
        }
    }
    Ok(())
}
fn text(d: &Dict, k: &[u8]) -> String {
    d.get(k).and_then(Object::as_string).map(PdfString::to_text).unwrap_or_default()
}
fn nums(v: impl IntoIterator<Item = f64>) -> Object {
    Object::Array(v.into_iter().map(Object::Real).collect())
}
fn page(doc: &Document, index: usize) -> Result<pdfcraft_model::Page> {
    pdfcraft_model::pages(doc).get(index).cloned().ok_or_else(|| invalid("measurement page does not exist"))
}
fn annot(doc: &Document, page_index: usize, index: usize) -> Result<(ObjRef, Dict)> {
    let p = page(doc, page_index)?;
    let list = doc.resolve(p.dict.get(b"Annots").ok_or_else(|| invalid("page has no annotations"))?);
    let o = list.as_array().and_then(|a| a.get(index)).ok_or_else(|| invalid("measurement annotation does not exist"))?;
    let r = o.as_ref().ok_or_else(|| invalid("inline measurement annotations cannot be edited yet"))?;
    let o = doc.get(r);
    Ok((r, o.as_dict().cloned().ok_or_else(|| invalid("invalid annotation dictionary"))?))
}
pub fn add(doc: &mut Document, new: &NewMeasurement, meta: &Meta) -> Result<usize> {
    validate_geometry(new.kind, &new.points)?;
    let info = reading(new.kind, &new.points, &new.scale)?;
    if info.value <= 0.0 {
        return Err(invalid("measurement must have a positive length or area"));
    }
    if new.label.len() > 512 || new.author.len() > 512 {
        return Err(invalid("measurement label and author must be at most 512 bytes"));
    }
    let shape = match new.kind {
        Kind::Distance => Shape::Line {
            from: new.points.first().copied().ok_or_else(|| invalid("missing start"))?,
            to: new.points.last().copied().ok_or_else(|| invalid("missing end"))?,
            arrow: true,
        },
        Kind::Perimeter => Shape::PolyLine { vertices: new.points.clone() },
        Kind::Area => Shape::Polygon { vertices: new.points.clone(), cloud: false },
    };
    let index = pdfcraft_annot::add_annotation(
        doc,
        &NewAnnotation { page: new.page, shape, style: new.style.clone(), contents: info.label.clone(), author: new.author.clone() },
        meta,
    )?;
    let (r, mut d) = annot(doc, new.page, index)?;
    d.set(b"IT".to_vec(), Object::name(new.kind.intent()));
    d.set(b"Measure".to_vec(), Object::Dict(new.scale.dictionary()?));
    d.set(b"Subj".to_vec(), PdfString::text(new.kind.name()));
    d.set(b"PCMeasureLabel".to_vec(), PdfString::text(&new.label));
    d.set(b"PCMeasureValue".to_vec(), PdfString::text(&info.label));
    if new.kind == Kind::Distance {
        d.set(b"LE".to_vec(), Object::Array(vec![Object::name("OpenArrow"), Object::name("OpenArrow")]));
        d.set(b"Cap".to_vec(), Object::Bool(true));
        d.set(b"CP".to_vec(), Object::name("Top"));
    }
    // Leave room for the readable numeric caption above the geometry.
    if let Some(rect) = d.get(b"Rect").and_then(Object::as_array) {
        let values: Option<Vec<f64>> = rect.iter().map(Object::as_f64).collect();
        if let Some(v) = values.filter(|v| v.len() == 4) {
            let width = (info.label.len() as f64 * 6.0).max(40.0);
            let cx = (v[0] + v[2]) * 0.5;
            d.set(b"Rect".to_vec(), nums([v[0].min(cx - width / 2.0), v[1], v[2].max(cx + width / 2.0), v[3] + 16.0]));
        }
    }
    doc.update_dict(r, |target| *target = d).map_err(|e| invalid(&e.to_string()))?;
    pdfcraft_annot::set_appearance(doc, r)?;
    Ok(index)
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Measurement {
    pub page: usize,
    pub index: usize,
    pub id: String,
    pub author: String,
    pub label: String,
    pub points: Vec<Point>,
    pub scale: Scale,
    pub reading: Reading,
}
/// A measurement annotation that couldn't be read (unsupported scale format, invalid
/// geometry). Its PDF data is left untouched.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Unsupported {
    pub page: usize,
    pub index: usize,
    pub reason: String,
}
/// Saved measurements plus the ones that were skipped.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Listing {
    pub measurements: Vec<Measurement>,
    pub unsupported: Vec<Unsupported>,
    /// The annotation limit stopped enumeration early.
    pub truncated: bool,
}
const MAX_ANNOTATIONS: usize = 100_000;
fn read_measurement(doc: &Document, d: &Dict, kind: Kind, page: usize, index: usize) -> Result<Measurement> {
    let scale = Scale::read(doc, d.get(b"Measure").ok_or_else(|| invalid("measurement has no scale"))?)?;
    let key = if kind == Kind::Distance { &b"L"[..] } else { b"Vertices" };
    let o = doc.resolve(d.get(key).ok_or_else(|| invalid("measurement has no vertices"))?);
    let a = o.as_array().ok_or_else(|| invalid("invalid measurement vertices"))?;
    if a.len() % 2 != 0 || a.len() > MAX_POINTS * 2 {
        return Err(invalid("invalid measurement vertex count"));
    }
    let v: Vec<f64> = a.iter().map(|n| doc.resolve(n).as_f64().ok_or_else(|| invalid("invalid vertex"))).collect::<Result<_>>()?;
    let points: Vec<Point> = v.as_chunks::<2>().0.to_vec();
    validate_geometry(kind, &points)?;
    let reading = reading(kind, &points, &scale)?;
    Ok(Measurement { page, index, id: text(d, b"NM"), author: text(d, b"T"), label: text(d, b"PCMeasureLabel"), points, scale, reading })
}
/// Every measurement annotation. One that can't be read is reported in `unsupported`
/// rather than failing the whole list.
pub fn list(doc: &Document) -> Listing {
    let mut out = Listing::default();
    let mut count = 0usize;
    for (page, p) in pdfcraft_model::pages(doc).iter().enumerate() {
        let Some(o) = p.dict.get(b"Annots") else { continue };
        let o = doc.resolve(o);
        let Some(arr) = o.as_array() else { continue };
        for (index, a) in arr.iter().enumerate() {
            count = count.saturating_add(1);
            if count > MAX_ANNOTATIONS {
                out.truncated = true;
                return out;
            }
            let o = doc.resolve(a);
            let Some(d) = o.as_dict() else { continue };
            let kind = match d.name(b"IT") {
                Some(b"LineDimension") => Kind::Distance,
                Some(b"PolyLineDimension") => Kind::Perimeter,
                Some(b"PolygonDimension") => Kind::Area,
                _ => continue,
            };
            match read_measurement(doc, d, kind, page, index) {
                Ok(m) => out.measurements.push(m),
                Err(e) => out.unsupported.push(Unsupported { page, index, reason: e.to_string() }),
            }
        }
    }
    out
}
/// Last containing viewport wins, using the first point of the measurement (ISO 32000-2 §12.9).
pub fn scale_at(doc: &Document, page_index: usize, at: Point) -> Result<Scale> {
    check_points(&[at])?;
    let p = page(doc, page_index)?;
    if let Some(o) = p.dict.get(b"VP") {
        let o = doc.resolve(o);
        let arr = o.as_array().ok_or_else(|| invalid("invalid page viewports"))?;
        for viewport in arr.iter().rev() {
            let o = doc.resolve(viewport);
            let Some(d) = o.as_dict() else { continue };
            if let Some(bbox) = d.get(b"BBox") {
                let o = doc.resolve(bbox);
                let b: Option<Vec<f64>> = o.as_array().and_then(|a| a.iter().map(|v| doc.resolve(v).as_f64()).collect());
                if let Some(b) = b.filter(|b| b.len() == 4 && b.iter().all(|v| v.is_finite()))
                    && at[0] >= b[0].min(b[2])
                    && at[0] <= b[0].max(b[2])
                    && at[1] >= b[1].min(b[3])
                    && at[1] <= b[1].max(b[3])
                {
                    return Scale::read(doc, d.get(b"Measure").ok_or_else(|| invalid("viewport has no measurement scale"))?);
                }
            }
        }
    }
    let user_unit = p.dict.get(b"UserUnit").and_then(Object::as_f64).unwrap_or(1.0);
    Scale::new(user_unit / 72.0, "in", 2)
}
/// Add a named rectangular viewport. Existing viewports are preserved in drawing order.
pub fn set_scale(doc: &mut Document, page_index: usize, bbox: [f64; 4], name: &str, scale: &Scale) -> Result<()> {
    check_points(&[[bbox[0], bbox[1]], [bbox[2], bbox[3]]])?;
    if bbox[2] <= bbox[0] || bbox[3] <= bbox[1] || name.len() > 256 {
        return Err(invalid("viewport needs a positive rectangle and a name of at most 256 bytes"));
    }
    let p = page(doc, page_index)?;
    let vp = p.dict.get(b"VP");
    let mut list = match vp {
        Some(o) => doc.resolve(o).as_array().cloned().ok_or_else(|| invalid("invalid page viewports"))?,
        None => Vec::new(),
    };
    if list.len() >= 1024 {
        return Err(invalid("page has too many measurement viewports"));
    }
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("Viewport"));
    d.set(b"BBox".to_vec(), nums(bbox));
    d.set(b"Name".to_vec(), PdfString::text(name));
    d.set(b"Measure".to_vec(), Object::Dict(scale.dictionary()?));
    list.push(Object::Dict(d));
    // An indirect /VP array stays indirect (it may be shared); otherwise the page holds it.
    match vp.and_then(Object::as_ref) {
        Some(r) => doc.set(r, Object::Array(list)),
        None => doc.update_dict(p.obj, |d| d.set(b"VP".to_vec(), Object::Array(list))).map_err(|e| invalid(&e.to_string()))?,
    }
    Ok(())
}
pub fn csv(measurements: &[Measurement]) -> String {
    // Prefix spreadsheet formula triggers; quoting alone does not prevent evaluation.
    let field = |s: &str| {
        let prefix = if s.trim_start().starts_with(['=', '+', '-', '@']) || s.starts_with(['\t', '\r']) { "'" } else { "" };
        format!("\"{prefix}{}\"", s.replace('"', "\"\""))
    };
    let mut out = "Page,Index,Type,Value,Unit,Label,Author,Scale\r\n".to_string();
    for m in measurements {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{}\r\n",
            m.page.saturating_add(1),
            m.index.saturating_add(1),
            m.reading.kind.name(),
            m.reading.value,
            field(&m.reading.unit),
            field(&m.label),
            field(&m.author),
            field(&m.scale.ratio)
        ));
    }
    out
}

#[cfg(test)]
mod tests;

/// Display points (top-left, y down, after Rotate/UserUnit) to PDF user space.
pub fn view_to_user(doc: &Document, page_index: usize, at: Point) -> Result<Point> {
    check_points(&[at])?;
    let page = page(doc, page_index)?;
    let unit = page.dict.get(b"UserUnit").and_then(Object::as_f64).unwrap_or(1.0);
    if !unit.is_finite() || unit <= 0.0 || unit > 75000.0 {
        return Err(invalid("invalid page UserUnit"));
    }
    let (_, height) = page.display_size(doc);
    let m = pdfcraft_content::Matrix(page.view_matrix(doc));
    let (x, y) = m.apply(at[0] / unit, height - at[1] / unit);
    check_points(&[[x, y]])?;
    Ok([x, y])
}
pub fn user_to_view(doc: &Document, page_index: usize, at: Point) -> Result<Point> {
    check_points(&[at])?;
    let page = page(doc, page_index)?;
    let unit = page.dict.get(b"UserUnit").and_then(Object::as_f64).unwrap_or(1.0);
    if !unit.is_finite() || unit <= 0.0 || unit > 75000.0 {
        return Err(invalid("invalid page UserUnit"));
    }
    let (_, height) = page.display_size(doc);
    let m = pdfcraft_content::Matrix(page.view_matrix(doc)).invert().ok_or_else(|| invalid("invalid page transform"))?;
    let (x, y) = m.apply(at[0], at[1]);
    Ok([x * unit, (height - y) * unit])
}
