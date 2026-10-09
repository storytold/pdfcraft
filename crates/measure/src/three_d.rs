//! Embedded model access and PDF 3D measurement persistence.
//! Geometry uses ECMA-363; PDF dictionaries follow ISO 32000-1 section 13.6
//! and the Adobe ISO 32000 supplement, extension level 3, section 9.5.6.
use crate::{Result, invalid, page};
use pdfcraft_cos::{Dict, Document, Object};
pub use pdfcraft_model::three_d::{Hit, Matrix, Mesh, Point, Scene};
use serde::{Deserialize, Serialize};
const MAX_ANNOTATIONS: usize = 4096;
const MAX_VIEWS: usize = 256;
const MAX_MODEL_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Units {
    pub unit: String,
    pub factor: f64,
    pub declared: bool,
}
impl Default for Units {
    fn default() -> Self {
        Self { unit: "Model Units".into(), factor: 1., declared: false }
    }
}
#[derive(Clone, Debug)]
pub struct Artwork {
    pub annotation: usize,
    pub rect: [f64; 4],
    pub scene: Scene,
    pub units: Units,
    pub views: Vec<View>,
    pub default_view: Option<View>,
    pub edit_error: Option<String>,
}
/// One decoded scene and its selected PDF camera and measurements.
#[derive(Clone, Debug)]
pub struct ViewState {
    pub artwork: Artwork,
    pub camera: super::three_d_camera::Camera,
    pub measurements: Listing,
}
pub fn load_view(doc: &Document, page: usize, annotation: usize, selected: Option<usize>) -> Result<ViewState> {
    let artwork = artwork(doc, page, annotation)?;
    let view = match selected {
        Some(index) => Some(artwork.views.get(index).ok_or_else(|| invalid("3D view does not exist"))?),
        None => None,
    };
    let camera = super::three_d_camera::Camera::from_artwork(doc, &artwork, view)?;
    let measurements = measurements(doc, page, annotation, selected)?;
    Ok(ViewState { artwork, camera, measurements })
}
#[derive(Clone, Debug, Serialize)]
pub struct ModelInfo {
    pub annotation: usize,
    pub name: String,
    pub format: String,
    pub rect: std::result::Result<[f64; 4], String>,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct View {
    pub name: String,
    pub internal_name: String,
    /// Twelve PDF camera-to-world matrix entries, column order.
    pub camera_to_world: Option<[f64; 12]>,
    pub orbit_distance: Option<f64>,
    pub perspective: bool,
    pub field_of_view: f64,
    pub orthographic_scale: f64,
    #[serde(skip)]
    pub source: Dict,
}
fn string(doc: &Document, d: &Dict, key: &[u8]) -> Result<Option<String>> {
    let Some(value) = d.get(key) else { return Ok(None) };
    let value = doc.resolve(value);
    let Object::String(value) = value.as_ref() else { return Err(invalid("3D text entry is not a text string")) };
    let value = value.to_text();
    if value.len() > 512 || value.chars().any(char::is_control) {
        return Err(invalid("3D text entry exceeds 512 bytes or contains control characters"));
    }
    Ok(Some(value))
}
pub(crate) fn number(doc: &Document, d: &Dict, key: &[u8], default: f64) -> Result<f64> {
    let value = match d.get(key) {
        Some(v) => doc.resolve(v).as_f64().ok_or_else(|| invalid("invalid 3D numeric entry"))?,
        None => default,
    };
    if !value.is_finite() || value.abs() > 1e12 {
        return Err(invalid("3D number must be finite and within 1e12"));
    }
    Ok(value)
}
fn numbers<const N: usize>(doc: &Document, value: &Object) -> Result<[f64; N]> {
    let value = doc.resolve(value);
    let values = value.as_array().filter(|v| v.len() == N).ok_or_else(|| invalid("invalid 3D numeric array length"))?;
    let mut out = [0.; N];
    for (target, value) in out.iter_mut().zip(values) {
        *target =
            doc.resolve(value).as_f64().filter(|v| v.is_finite() && v.abs() <= 1e12).ok_or_else(|| invalid("invalid 3D numeric array entry"))?;
    }
    Ok(out)
}
fn rectangle(doc: &Document, d: &Dict) -> Result<[f64; 4]> {
    let rect = numbers(doc, d.get(b"3DB").or_else(|| d.get(b"Rect")).ok_or_else(|| invalid("3D annotation has no rectangle"))?)?;
    if rect[2] <= rect[0] || rect[3] <= rect[1] {
        return Err(invalid("3D annotation needs a positive rectangle"));
    }
    Ok(rect)
}
fn annotations(doc: &Document, page_index: usize) -> Result<Vec<Object>> {
    let p = page(doc, page_index)?;
    let Some(value) = p.dict.get(b"Annots") else { return Ok(Vec::new()) };
    let value = doc.resolve(value);
    let values = value.as_array().ok_or_else(|| invalid("invalid page annotations"))?;
    if values.len() > MAX_ANNOTATIONS {
        return Err(invalid("3D access exceeds 4096 page annotations"));
    }
    Ok(values.clone())
}
fn annotation(doc: &Document, page_index: usize, index: usize) -> Result<Dict> {
    let values = annotations(doc, page_index)?;
    let value = values.get(index).ok_or_else(|| invalid("3D annotation does not exist"))?;
    let object = doc.resolve(value);
    let d = object.as_dict().filter(|d| d.name(b"Subtype") == Some(b"3D")).ok_or_else(|| invalid("annotation is not a 3D model"))?;
    Ok(d.clone())
}
fn writable_annotation(doc: &Document, annotation: &Dict) -> Result<()> {
    let flags = match annotation.get(b"F") {
        None => 0,
        Some(value) => match doc.resolve(value).as_ref() {
            Object::Int(value) if (0..=i64::from(u32::MAX)).contains(value) => *value,
            _ => return Err(invalid("3D annotation flags must be a nonnegative integer")),
        },
    };
    // Each edit detaches the model's view on this annotation. ReadOnly, Locked
    // and LockedContents therefore all prevent that change of its properties.
    if flags & (64 | 128 | 512) != 0 {
        return Err(invalid("the 3D model annotation is read-only or locked"));
    }
    Ok(())
}
fn model_object(doc: &Document, d: &Dict) -> Result<std::sync::Arc<Object>> {
    let object = doc.resolve(d.get(b"3DD").ok_or_else(|| invalid("3D annotation has no model data"))?);
    if let Object::Dict(reference) = object.as_ref() {
        if reference.name(b"Type") != Some(b"3DRef") {
            return Err(invalid("invalid 3D model reference"));
        }
        return Ok(doc.resolve(reference.get(b"3DD").ok_or_else(|| invalid("3D reference has no model stream"))?));
    }
    Ok(object)
}
/// Inventory does not decompress models or execute embedded actions or scripts.
pub fn models(doc: &Document, page_index: usize) -> Result<Vec<ModelInfo>> {
    let mut out = Vec::new();
    for (index, value) in annotations(doc, page_index)?.iter().enumerate() {
        let object = doc.resolve(value);
        let Some(d) = object.as_dict().filter(|d| d.name(b"Subtype") == Some(b"3D")) else { continue };
        let format = model_object(doc, d)
            .ok()
            .and_then(|o| match o.as_ref() {
                Object::Stream(s) => s.dict.name(b"Subtype").map(|n| String::from_utf8_lossy(n).into_owned()),
                _ => None,
            })
            .unwrap_or_default();
        out.push(ModelInfo {
            annotation: index,
            name: string(doc, d, b"Contents")?.or(string(doc, d, b"NM")?).unwrap_or_else(|| format!("Model {}", out.len() + 1)),
            format,
            rect: rectangle(doc, d).map_err(|e| e.to_string()),
        });
    }
    Ok(out)
}
/// The supplement's page 61 algorithm defines displayed X as (m/n)*X.
/// User units replace creation units; display units multiply that selected ratio.
pub fn units(doc: &Document, d: &Dict, model_meters: Option<f64>) -> Result<Units> {
    let mut out = match model_meters {
        Some(factor) => Units { unit: "m".into(), factor, declared: true },
        None => Units::default(),
    };
    let Some(value) = d.get(b"3DU") else { return Ok(out) };
    let object = doc.resolve(value);
    let d = object.as_dict().ok_or_else(|| invalid("invalid 3D units dictionary"))?;
    for (label, m, n, display) in
        [(b"TU".as_slice(), b"TSm".as_slice(), b"TSn".as_slice(), false), (b"UU", b"USm", b"USn", false), (b"DU", b"DSm", b"DSn", true)]
    {
        let Some(unit) = string(doc, d, label)? else { continue };
        if unit.trim().is_empty() || unit.chars().count() > 24 {
            return Err(invalid("3D unit label must have 1 to 24 printable characters"));
        }
        let m = number(doc, d, m, 1.)?;
        let n = number(doc, d, n, 1.)?;
        if !(1e-12..=1e12).contains(&m) || !(1e-12..=1e12).contains(&n) {
            return Err(invalid("3D unit ratio must be positive and bounded"));
        }
        out.factor = if display { out.factor * m / n } else { m / n };
        out.unit = unit;
        out.declared = true;
        if !out.factor.is_finite() || !(1e-12..=1e12).contains(&out.factor) {
            return Err(invalid("3D unit conversion exceeds its bounds"));
        }
    }
    Ok(out)
}
fn view(doc: &Document, value: &Object) -> Result<View> {
    let object = doc.resolve(value);
    let d = object.as_dict().ok_or_else(|| invalid("invalid 3D view dictionary"))?;
    let name = string(doc, d, b"XN")?.ok_or_else(|| invalid("3D view has no external name"))?;
    let internal_name = string(doc, d, b"IN")?.unwrap_or_default();
    let camera_to_world = match d.name(b"MS") {
        Some(b"M") => Some(numbers(doc, d.get(b"C2W").ok_or_else(|| invalid("3D matrix view has no camera matrix"))?)?),
        Some(b"U3D") => None,
        None => None,
        _ => return Err(invalid("invalid 3D view matrix selector")),
    };
    let orbit_distance = if d.contains(b"CO") {
        let value = number(doc, d, b"CO", 0.)?;
        if value < 0. {
            return Err(invalid("3D orbit distance cannot be negative"));
        }
        Some(value)
    } else {
        None
    };
    let mut perspective = true;
    let mut field_of_view = 90.;
    let mut orthographic_scale = 1.;
    if let Some(value) = d.get(b"P") {
        let object = doc.resolve(value);
        let p = object.as_dict().ok_or_else(|| invalid("invalid 3D projection dictionary"))?;
        perspective = match p.name(b"Subtype") {
            None | Some(b"P") => true,
            Some(b"O") => false,
            _ => return Err(invalid("invalid 3D projection type")),
        };
        field_of_view = number(doc, p, b"FOV", 90.)?;
        orthographic_scale = number(doc, p, b"OS", 1.)?;
        if !(0.0..180.0).contains(&field_of_view) || field_of_view == 0. || !(1e-12..=1e12).contains(&orthographic_scale) {
            return Err(invalid("invalid 3D projection scale or field of view"));
        }
    }
    Ok(View { name, internal_name, camera_to_world, orbit_distance, perspective, field_of_view, orthographic_scale, source: d.clone() })
}
fn default_view(doc: &Document, selector: Option<&Object>, views: &[View]) -> Result<Option<View>> {
    let Some(selector) = selector else { return Ok(views.first().cloned()) };
    let selector = doc.resolve(selector);
    let selected = match selector.as_ref() {
        Object::Dict(_) => return view(doc, selector.as_ref()).map(Some),
        Object::Int(index) => usize::try_from(*index).ok().and_then(|index| views.get(index)),
        Object::Name(name) if name == b"F" => views.first(),
        Object::Name(name) if name == b"L" => views.last(),
        Object::String(name) => views.iter().find(|v| v.internal_name == name.to_text()),
        _ => return Err(invalid("invalid default 3D view selector")),
    };
    selected.cloned().map(Some).ok_or_else(|| invalid("default 3D view does not exist"))
}
/// Decode only the requested model, with a bounded decompression and geometry budget.
pub fn artwork(doc: &Document, page_index: usize, index: usize) -> Result<Artwork> {
    let d = annotation(doc, page_index, index)?;
    let rect = rectangle(doc, &d)?;
    let object = model_object(doc, &d)?;
    let Object::Stream(stream) = object.as_ref() else { return Err(invalid("3D model data is not a stream")) };
    if stream.dict.contains(b"F") {
        return Err(invalid("external 3D model streams must be embedded before measuring"));
    }
    if stream.raw.len() > MAX_MODEL_BYTES {
        return Err(invalid("encoded 3D model exceeds 64 MiB"));
    }
    let bytes = stream.decoded_within(MAX_MODEL_BYTES).map_err(|e| invalid(&e.to_string()))?;
    let scene = match stream.dict.name(b"Subtype") {
        Some(b"U3D") => Scene::decode_u3d(&bytes).map_err(|e| invalid(&e.to_string()))?,
        Some(b"PRC") => return Err(invalid("PRC geometry decoder is not yet available")),
        _ => return Err(invalid("invalid embedded 3D model format")),
    };
    let units = units(doc, &d, scene.meters_per_unit)?;
    let mut views = Vec::new();
    if let Some(value) = stream.dict.get(b"VA") {
        let object = doc.resolve(value);
        let values = object.as_array().ok_or_else(|| invalid("invalid 3D views array"))?;
        if values.len() > MAX_VIEWS {
            return Err(invalid("3D model exceeds 256 views"));
        }
        for value in values {
            views.push(view(doc, value)?);
        }
    }
    let default_view = default_view(doc, d.get(b"3DV").or_else(|| stream.dict.get(b"DV")), &views)?;
    let edit_error = writable_annotation(doc, &d).err().map(|e| e.to_string());
    Ok(Artwork { annotation: index, rect, scene, units, views, default_view, edit_error })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Linear,
    Perpendicular,
    Angular,
    Radial,
}
impl Kind {
    fn subtype(self) -> &'static [u8] {
        match self {
            Self::Linear => b"LD3",
            Self::Perpendicular => b"PD3",
            Self::Angular => b"AD3",
            Self::Radial => b"RD3",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Geometry {
    Linear { a: Point, b: Point },
    Perpendicular { a: Point, b: Point, direction: Point },
    Angular { a: Point, b: Point, first_direction: Point, second_direction: Point },
    Radial { center: Point, on_circle: Point, diameter: bool, arc: Option<[Point; 2]> },
}
pub(crate) fn subtract(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub(crate) fn dot(a: Point, b: Point) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}
pub(crate) fn length(a: Point) -> f64 {
    dot(a, a).sqrt()
}
pub(crate) fn cross(a: Point, b: Point) -> Point {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub(crate) fn check(point: Point) -> Result<()> {
    if point.iter().all(|v| v.is_finite() && v.abs() <= 1e12) { Ok(()) } else { Err(invalid("3D measurement point is not finite or exceeds 1e12")) }
}
pub(crate) fn direction(point: Point) -> Result<Point> {
    check(point)?;
    let n = length(point);
    if n < 1e-12 {
        return Err(invalid("3D measurement needs a nonzero direction"));
    }
    Ok(point.map(|v| v / n))
}
fn orthogonal_plane(axis: Point, hint: Point) -> Result<Point> {
    let axis = direction(axis)?;
    let hint = direction(hint)?;
    let projected = subtract(hint, axis.map(|v| v * dot(hint, axis)));
    if length(projected) > 1e-8 {
        return direction(projected);
    }
    let fallback = if axis[0].abs() < 0.8 { [1., 0., 0.] } else { [0., 1., 0.] };
    direction(cross(axis, fallback))
}
impl Geometry {
    /// Create a measurement from picked world-space points. Perpendicular uses
    /// a point followed by the two line endpoints; angle uses end, vertex, end.
    /// The plane hint is used only for a linear measurement's presentation.
    pub fn from_points(kind: Kind, points: &[Point], plane_hint: Point, diameter: bool) -> Result<(Self, Point)> {
        let required = if kind == Kind::Linear { 2 } else { 3 };
        if points.len() != required {
            return Err(invalid("3D selection needs exactly two distance points or three construction points"));
        }
        let a = *points.first().ok_or_else(|| invalid("missing first 3D point"))?;
        let b = *points.get(1).ok_or_else(|| invalid("missing second 3D point"))?;
        check(a)?;
        check(b)?;
        let (geometry, plane) = match kind {
            Kind::Linear => {
                let axis = direction(subtract(b, a))?;
                let plane = orthogonal_plane(axis, plane_hint)?;
                (Self::Linear { a, b }, plane)
            }
            Kind::Perpendicular => {
                let c = *points.get(2).ok_or_else(|| invalid("missing line endpoint"))?;
                check(c)?;
                let line = direction(subtract(c, b))?;
                let foot = std::array::from_fn(|i| b[i] + line[i] * dot(subtract(a, b), line));
                let plane = direction(cross(line, direction(subtract(a, foot))?))?;
                (Self::Perpendicular { a, b: foot, direction: line }, plane)
            }
            Kind::Angular => {
                let c = *points.get(2).ok_or_else(|| invalid("missing angular endpoint"))?;
                check(c)?;
                let first_direction = direction(subtract(a, b))?;
                let second_direction = direction(subtract(c, b))?;
                let normal = cross(first_direction, second_direction);
                let plane = if length(normal) > 1e-8 { direction(normal)? } else { orthogonal_plane(first_direction, plane_hint)? };
                (Self::Angular { a: b, b, first_direction, second_direction }, plane)
            }
            Kind::Radial => return Self::circle(a, b, *points.get(2).ok_or_else(|| invalid("missing circle point"))?, diameter),
        };
        geometry.value()?;
        Ok((geometry, plane))
    }
    pub fn kind(&self) -> Kind {
        match self {
            Self::Linear { .. } => Kind::Linear,
            Self::Perpendicular { .. } => Kind::Perpendicular,
            Self::Angular { .. } => Kind::Angular,
            Self::Radial { .. } => Kind::Radial,
        }
    }
    pub fn anchors(&self) -> [Point; 2] {
        match self {
            Self::Linear { a, b } | Self::Perpendicular { a, b, .. } | Self::Angular { a, b, .. } => [*a, *b],
            Self::Radial { center, on_circle, .. } => [*center, *on_circle],
        }
    }
    /// Geometry is measured in model world coordinates; angles are returned in radians.
    pub fn value(&self) -> Result<f64> {
        for point in self.anchors() {
            check(point)?;
        }
        let value = match self {
            Self::Linear { a, b } => length(subtract(*b, *a)),
            Self::Perpendicular { a, b, direction: d } => length(cross(subtract(*b, *a), direction(*d)?)),
            Self::Angular { first_direction, second_direction, .. } => {
                dot(direction(*first_direction)?, direction(*second_direction)?).clamp(-1., 1.).acos()
            }
            Self::Radial { center, on_circle, diameter, arc } => {
                let radius = length(subtract(*on_circle, *center));
                if let Some(arc) = arc {
                    for point in arc {
                        check(*point)?;
                        if (length(subtract(*point, *center)) - radius).abs() > radius.max(1.) * 1e-6 {
                            return Err(invalid("3D radial arc endpoints do not lie on the measured circle"));
                        }
                    }
                }
                radius * if *diameter { 2. } else { 1. }
            }
        };
        if !value.is_finite() || value <= 1e-12 || value > 1e12 {
            return Err(invalid("3D measurement must have a positive bounded value"));
        }
        Ok(value)
    }
    /// Fit a circle through three real model-space points, preserving its plane.
    pub fn circle(a: Point, b: Point, c: Point, diameter: bool) -> Result<(Self, Point)> {
        for point in [a, b, c] {
            check(point)?;
        }
        let u = subtract(b, a);
        let v = subtract(c, a);
        let n = cross(u, v);
        let denominator = 2. * dot(n, n);
        if denominator <= 1e-24 * dot(u, u) * dot(v, v) || length(u) < 1e-12 || length(v) < 1e-12 {
            return Err(invalid("3D circle needs three distinct non-collinear points"));
        }
        let un = cross(u, n);
        let nv = cross(n, v);
        let mut center = a;
        for (coordinate, (un, nv)) in center.iter_mut().zip(un.into_iter().zip(nv)) {
            *coordinate -= (dot(u, u) * nv + dot(v, v) * un) / denominator;
        }
        let geometry = Self::Radial { center, on_circle: a, diameter, arc: None };
        geometry.value()?;
        Ok((geometry, direction(cross(direction(u)?, direction(v)?))?))
    }
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Measurement {
    pub index: usize,
    pub geometry: Geometry,
    pub plane: Point,
    pub text_position: Point,
    pub text_x: Option<Point>,
    pub text_y: Point,
    pub text_size: f64,
    /// Radial extension length in default user space points.
    pub extension_length: f64,
    pub color: Point,
    pub value: f64,
    pub unit: String,
    pub precision: u8,
    pub degrees: bool,
    pub label: String,
    pub user_text: String,
    pub nodes: [Option<String>; 2],
    pub show_circle: bool,
    #[serde(skip)]
    pub source: Dict,
}
impl Measurement {
    pub fn new(geometry: Geometry, units: &Units, plane: Point, text_position: Point) -> Result<Self> {
        let plane = direction(plane)?;
        let value = geometry.value()?;
        let [a, b] = geometry.anchors();
        let axis = match &geometry {
            Geometry::Angular { first_direction, .. } => direction(*first_direction)?,
            _ => direction(subtract(b, a))?,
        };
        let text_y = direction(cross(plane, axis))?;
        let angular = geometry.kind() == Kind::Angular;
        let out = Self {
            index: 0,
            geometry,
            plane,
            text_position,
            text_x: Some(axis),
            text_y,
            text_size: 12.,
            extension_length: 60.,
            color: [0., 0.47, 0.84],
            value: if angular { value.to_degrees() } else { value * units.factor },
            unit: if angular { "°".into() } else { units.unit.clone() },
            precision: 3,
            degrees: true,
            label: String::new(),
            user_text: String::new(),
            nodes: [None, None],
            show_circle: false,
            source: Dict::new(),
        };
        out.dictionary()?;
        Ok(out)
    }
    pub fn from_points(kind: Kind, points: &[Point], units: &Units, plane_hint: Point, diameter: bool) -> Result<Self> {
        let (geometry, plane) = Geometry::from_points(kind, points, plane_hint, diameter)?;
        let plane = if dot(plane, direction(plane_hint)?) < 0. { plane.map(|value| -value) } else { plane };
        let [a, b] = geometry.anchors();
        let mut text_position = std::array::from_fn(|i| (a[i] + b[i]) / 2.);
        if let Geometry::Angular { first_direction, second_direction, .. } = &geometry {
            let radius = points.first().map(|p| length(subtract(*p, a))).unwrap_or(1.) * 0.35;
            let sum = std::array::from_fn(|i| first_direction[i] + second_direction[i]);
            let axis = if length(sum) > 1e-8 { direction(sum)? } else { direction(cross(plane, *first_direction))? };
            text_position = std::array::from_fn(|i| a[i] + radius * axis[i]);
        }
        Self::new(geometry, units, plane, text_position)
    }
    /// Model-space markup, including construction leaders and a circular arc.
    /// Projection and clipping are supplied by the selected PDF camera.
    pub fn segments(&self) -> Result<Vec<[Point; 2]>> {
        self.dictionary()?;
        let [a, b] = self.geometry.anchors();
        let mut lines = Vec::with_capacity(72);
        match &self.geometry {
            Geometry::Linear { .. } => {
                lines.push([a, b]);
                lines.push([std::array::from_fn(|i| (a[i] + b[i]) / 2.), self.text_position]);
            }
            Geometry::Perpendicular { direction: axis, .. } => {
                let axis = direction(*axis)?;
                let foot = std::array::from_fn(|i| b[i] + axis[i] * dot(subtract(a, b), axis));
                lines.push([a, foot]);
                let size = length(subtract(a, foot)) * 0.12;
                let normal = direction(subtract(a, foot))?;
                let corner = std::array::from_fn(|i| foot[i] + size * (axis[i] + normal[i]));
                lines.push([std::array::from_fn(|i| foot[i] + size * axis[i]), corner]);
                lines.push([corner, std::array::from_fn(|i| foot[i] + size * normal[i])]);
                lines.push([std::array::from_fn(|i| (a[i] + foot[i]) / 2.), self.text_position]);
            }
            Geometry::Angular { first_direction, second_direction, .. } => {
                let x = direction(*first_direction)?;
                let y = direction(cross(direction(self.plane)?, x))?;
                let angle = self.geometry.value()?;
                let sign = if dot(y, direction(*second_direction)?) < 0. { -1. } else { 1. };
                let radius = length(subtract(self.text_position, a)).max(1e-6);
                for axis in [x, direction(*second_direction)?] {
                    lines.push([a, std::array::from_fn(|i| a[i] + axis[i] * radius * 1.15)]);
                }
                let mut previous = std::array::from_fn(|i| a[i] + radius * x[i]);
                for step in 1..=32 {
                    let theta = angle * f64::from(step) / 32.;
                    let point = std::array::from_fn(|i| a[i] + radius * (x[i] * theta.cos() + y[i] * theta.sin() * sign));
                    lines.push([previous, point]);
                    previous = point;
                }
            }
            Geometry::Radial { center, on_circle, diameter, .. } => {
                let start = if *diameter { std::array::from_fn(|i| 2. * center[i] - on_circle[i]) } else { *center };
                lines.push([start, *on_circle]);
                lines.push([*on_circle, self.text_position]);
                if self.show_circle {
                    let x = direction(subtract(*on_circle, *center))?;
                    let y = direction(cross(direction(self.plane)?, x))?;
                    let radius = length(subtract(*on_circle, *center));
                    let mut previous = *on_circle;
                    for step in 1..=64 {
                        let theta = std::f64::consts::TAU * f64::from(step) / 64.;
                        let point = std::array::from_fn(|i| center[i] + radius * (x[i] * theta.cos() + y[i] * theta.sin()));
                        lines.push([previous, point]);
                        previous = point;
                    }
                }
            }
        }
        for line in &lines {
            for point in line {
                check(*point)?;
            }
        }
        Ok(lines)
    }
    /// Text axes on the annotation plane. TY chooses the up orientation;
    /// it does not replace the normal/axis cross product with an arbitrary vector.
    pub fn text_axes(&self) -> Result<[Point; 2]> {
        let normal = direction(self.plane)?;
        let [a, b] = self.geometry.anchors();
        let axis = match self.geometry {
            Geometry::Linear { .. } => subtract(b, a),
            Geometry::Perpendicular { direction: leader, .. } => {
                let leader = direction(leader)?;
                let delta = subtract(b, a);
                std::array::from_fn(|i| delta[i] - leader[i] * dot(delta, leader))
            }
            _ => self.text_x.ok_or_else(|| invalid("3D angular and radial text need a TX direction"))?,
        };
        let x = direction(axis)?;
        if dot(x, normal).abs() > 1e-6 {
            return Err(invalid("3D text direction must lie on the annotation plane"));
        }
        let mut up = direction(cross(normal, x))?;
        let orientation = dot(up, direction(self.text_y)?);
        if orientation.abs() <= 1e-12 {
            return Err(invalid("3D text up direction cannot orient the annotation plane"));
        }
        if orientation < 0. {
            up = up.map(|v| -v);
        }
        Ok([x, up])
    }
    pub fn caption(&self) -> String {
        format!("{:.*} {}{}{}", usize::from(self.precision), self.value, self.unit, if self.user_text.is_empty() { "" } else { " " }, self.user_text)
    }
    pub fn dictionary(&self) -> Result<Dict> {
        self.geometry.value()?;
        self.text_axes()?;
        for point in [self.plane, self.text_y] {
            direction(point)?;
        }
        check(self.text_position)?;
        if let Some(x) = self.text_x {
            direction(x)?;
        }
        if self.precision > 12
            || !self.value.is_finite()
            || self.value <= 0.
            || self.value > 1e12
            || !(1.0..=256.0).contains(&self.text_size)
            || !self.extension_length.is_finite()
            || !(0.0..=4096.0).contains(&self.extension_length)
            || self.color.iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err(invalid("invalid 3D measurement value or appearance"));
        }
        for text in [&self.label, &self.user_text, &self.unit] {
            if text.len() > 512 || text.chars().any(char::is_control) {
                return Err(invalid("3D measurement text is too long or contains control characters"));
            }
        }
        if self.geometry.kind() != Kind::Angular && self.unit.trim().is_empty() {
            return Err(invalid("3D linear measurement needs a units string"));
        }
        let mut d = self.source.clone();
        for (key, value) in [
            (b"Type".as_slice(), Object::Name(b"3DMeasure".to_vec())),
            (b"Subtype", Object::Name(self.geometry.kind().subtype().to_vec())),
            (b"AP", crate::nums(self.plane)),
            (b"TP", crate::nums(self.text_position)),
            (b"TY", crate::nums(self.text_y)),
            (b"TS", Object::Real(self.text_size)),
            (b"C", crate::nums(self.color)),
            (b"V", Object::Real(self.value)),
            (b"P", Object::Int(i64::from(self.precision))),
            (b"TRL", Object::String(pdfcraft_cos::PdfString::text(&self.label))),
            (b"UT", Object::String(pdfcraft_cos::PdfString::text(&self.user_text))),
        ] {
            d.set(key.to_vec(), value);
        }
        let [a, b] = self.geometry.anchors();
        d.set(b"A1".to_vec(), crate::nums(a));
        d.set(b"A2".to_vec(), crate::nums(b));
        for (key, node) in [(b"N1".as_slice(), &self.nodes[0]), (b"N2", &self.nodes[1])] {
            if let Some(node) = node {
                if node.len() > 512 || node.chars().any(char::is_control) {
                    return Err(invalid("invalid 3D measurement node name"));
                }
                d.set(key.to_vec(), Object::String(pdfcraft_cos::PdfString::text(node)));
            } else {
                d.remove(key);
            }
        }
        match &self.geometry {
            Geometry::Perpendicular { direction, .. } => d.set(b"D1".to_vec(), crate::nums(*direction)),
            Geometry::Angular { first_direction, second_direction, .. } => {
                d.set(b"D1".to_vec(), crate::nums(*first_direction));
                d.set(b"D2".to_vec(), crate::nums(*second_direction));
                d.set(b"DR".to_vec(), Object::Bool(self.degrees));
            }
            Geometry::Radial { diameter, arc, .. } => {
                d.set(b"EL".to_vec(), Object::Real(self.extension_length));
                d.set(b"R".to_vec(), Object::Bool(!diameter));
                d.set(b"SC".to_vec(), Object::Bool(self.show_circle));
                if let Some([a, b]) = arc {
                    d.set(b"A3".to_vec(), crate::nums(*a));
                    d.set(b"A4".to_vec(), crate::nums(*b));
                } else {
                    d.remove(b"A3");
                    d.remove(b"A4");
                }
            }
            Geometry::Linear { .. } => {}
        }
        if matches!(self.geometry.kind(), Kind::Angular | Kind::Radial) {
            d.set(b"TX".to_vec(), crate::nums(self.text_x.ok_or_else(|| invalid("3D angular and radial measurements need a text X direction"))?));
        } else {
            d.remove(b"TX");
        }
        if self.geometry.kind() != Kind::Angular {
            d.set(b"U".to_vec(), Object::String(pdfcraft_cos::PdfString::text(&self.unit)));
        } else {
            d.remove(b"U");
        }
        Ok(d)
    }
}

fn boolean(doc: &Document, d: &Dict, key: &[u8], default: bool) -> Result<bool> {
    match d.get(key) {
        None => Ok(default),
        Some(value) => match doc.resolve(value).as_ref() {
            Object::Bool(value) => Ok(*value),
            _ => Err(invalid("invalid 3D boolean entry")),
        },
    }
}
impl Measurement {
    pub fn read(doc: &Document, value: &Object, index: usize) -> Result<Self> {
        let object = doc.resolve(value);
        let d = object.as_dict().ok_or_else(|| invalid("invalid 3D measurement dictionary"))?;
        let point =
            |key: &[u8]| -> Result<Point> { numbers(doc, d.get(key).ok_or_else(|| invalid("3D measurement lacks a required point or direction"))?) };
        let a = point(b"A1")?;
        let b = point(b"A2")?;
        let geometry = match d.name(b"Subtype") {
            Some(b"LD3") => Geometry::Linear { a, b },
            Some(b"PD3") => Geometry::Perpendicular { a, b, direction: point(b"D1")? },
            Some(b"AD3") => Geometry::Angular { a, b, first_direction: point(b"D1")?, second_direction: point(b"D2")? },
            Some(b"RD3") => {
                let arc = match (d.contains(b"A3"), d.contains(b"A4")) {
                    (true, true) => Some([point(b"A3")?, point(b"A4")?]),
                    (false, false) => None,
                    _ => return Err(invalid("3D radial arc requires both endpoints")),
                };
                Geometry::Radial { center: a, on_circle: b, diameter: !boolean(doc, d, b"R", true)?, arc }
            }
            _ => return Err(invalid("unsupported 3D measurement subtype")),
        };
        let degrees = boolean(doc, d, b"DR", true)?;
        let angular = geometry.kind() == Kind::Angular;
        let precision = match d.get(b"P") {
            Some(value) => {
                doc.resolve(value).as_int().and_then(|n| u8::try_from(n).ok()).ok_or_else(|| invalid("invalid 3D measurement precision"))?
            }
            None => 3,
        };
        let out = Self {
            index,
            geometry,
            plane: point(b"AP")?,
            text_position: point(b"TP")?,
            text_x: if d.contains(b"TX") { Some(point(b"TX")?) } else { None },
            text_y: point(b"TY")?,
            text_size: number(doc, d, b"TS", 12.)?,
            extension_length: if d.name(b"Subtype") == Some(b"RD3") { number(doc, d, b"EL", 60.)? } else { 60. },
            color: if d.contains(b"C") { point(b"C")? } else { [1.; 3] },
            value: number(doc, d, b"V", f64::NAN)?,
            unit: if angular {
                if degrees { "°" } else { "rad" }.into()
            } else {
                string(doc, d, b"U")?.ok_or_else(|| invalid("3D measurement has no units string"))?
            },
            precision,
            degrees,
            label: string(doc, d, b"TRL")?.unwrap_or_default(),
            user_text: string(doc, d, b"UT")?.unwrap_or_default(),
            nodes: [string(doc, d, b"N1")?, string(doc, d, b"N2")?],
            show_circle: boolean(doc, d, b"SC", false)?,
            source: d.clone(),
        };
        out.dictionary()?;
        Ok(out)
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct UnsupportedMeasurement {
    pub index: usize,
    pub reason: String,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct Listing {
    pub measurements: Vec<Measurement>,
    pub unsupported: Vec<UnsupportedMeasurement>,
}
fn measurement_array(doc: &Document, d: &Dict) -> Result<Vec<Object>> {
    let Some(value) = d.get(b"MA") else { return Ok(Vec::new()) };
    let object = doc.resolve(value);
    let values = object.as_array().ok_or_else(|| invalid("invalid 3D measurement array"))?;
    if values.len() > 4096 {
        return Err(invalid("3D view exceeds 4096 measurements"));
    }
    Ok(values.clone())
}
struct ViewStorage {
    annotation: Dict,
    stream: pdfcraft_cos::Stream,
    views: Vec<Object>,
    index: usize,
    view: Dict,
}
fn view_storage(doc: &Document, page_index: usize, annotation_index: usize, view_index: Option<usize>) -> Result<ViewStorage> {
    let annotation = annotation(doc, page_index, annotation_index)?;
    let object = model_object(doc, &annotation)?;
    let Object::Stream(stream) = object.as_ref() else { return Err(invalid("3D model data is not a stream")) };
    let mut values = match stream.dict.get(b"VA") {
        None => Vec::new(),
        Some(value) => {
            let object = doc.resolve(value);
            let values = object.as_array().ok_or_else(|| invalid("invalid 3D view array"))?;
            if values.len() > MAX_VIEWS {
                return Err(invalid("3D model exceeds 256 views"));
            }
            values.clone()
        }
    };
    let selected = if let Some(index) = view_index {
        let value = values.get(index).ok_or_else(|| invalid("3D view does not exist"))?;
        view(doc, value)?.source
    } else {
        let views = values.iter().map(|value| view(doc, value)).collect::<Result<Vec<_>>>()?;
        match default_view(doc, annotation.get(b"3DV").or_else(|| stream.dict.get(b"DV")), &views)? {
            Some(view) => view.source,
            None => {
                let mut d = Dict::new();
                d.set(b"Type".to_vec(), Object::Name(b"3DView".to_vec()));
                d.set(b"XN".to_vec(), Object::String(pdfcraft_cos::PdfString::text("Measurements")));
                d
            }
        }
    };
    let index = if let Some(index) = view_index {
        index
    } else {
        match values.iter().position(|value| doc.resolve(value).as_dict() == Some(&selected)) {
            Some(index) => index,
            None => {
                if values.len() >= MAX_VIEWS {
                    return Err(invalid("3D model exceeds 256 views"));
                }
                let index = values.len();
                values.push(Object::Dict(selected.clone()));
                index
            }
        }
    };
    Ok(ViewStorage { annotation, stream: stream.clone(), views: values, index, view: selected })
}
/// Every unrecognized measurement remains in the PDF and receives a diagnostic.
fn editable_storage(doc: &Document, page: usize, annotation: usize, view: Option<usize>) -> Result<ViewStorage> {
    let storage = view_storage(doc, page, annotation, view)?;
    writable_annotation(doc, &storage.annotation)?;
    Ok(storage)
}
pub fn measurements(doc: &Document, page_index: usize, annotation: usize, view: Option<usize>) -> Result<Listing> {
    let storage = view_storage(doc, page_index, annotation, view)?;
    let mut out = Listing::default();
    for (index, value) in measurement_array(doc, &storage.view)?.iter().enumerate() {
        match Measurement::read(doc, value, index) {
            Ok(measurement) => out.measurements.push(measurement),
            Err(error) => out.unsupported.push(UnsupportedMeasurement { index, reason: error.to_string() }),
        }
    }
    Ok(out)
}
fn extension(doc: &mut Document) -> Result<()> {
    let root = doc.root().ok_or_else(|| invalid("PDF has no catalog"))?;
    let object = doc.get(root);
    let catalog = object.as_dict().ok_or_else(|| invalid("invalid PDF catalog"))?;
    let mut extensions = match catalog.get(b"Extensions") {
        None => Dict::new(),
        Some(value) => doc.resolve(value).as_dict().cloned().ok_or_else(|| invalid("invalid PDF extensions dictionary"))?,
    };
    let mut adobe = match extensions.get(b"ADBE") {
        None => Dict::new(),
        Some(value) => doc.resolve(value).as_dict().cloned().ok_or_else(|| invalid("invalid Adobe PDF extension dictionary"))?,
    };
    let level = adobe.int(b"ExtensionLevel").unwrap_or(0);
    // Preserve a newer declaration and every other vendor's extension.
    if level < 3 {
        adobe.set(b"BaseVersion".to_vec(), Object::Name(b"1.7".to_vec()));
        adobe.set(b"ExtensionLevel".to_vec(), Object::Int(3));
        extensions.set(b"ADBE".to_vec(), Object::Dict(adobe));
        doc.update_dict(root, |d| d.set(b"Extensions".to_vec(), Object::Dict(extensions))).map_err(|e| invalid(&e.to_string()))?;
    }
    Ok(())
}
fn camera_view(doc: &Document, camera: &crate::three_d_camera::Camera, mut source: Dict) -> Result<Dict> {
    use crate::three_d_camera::{Binding, Projection};
    camera.validate()?;
    source.set(b"MS".to_vec(), Object::Name(b"M".to_vec()));
    source.set(b"C2W".to_vec(), crate::nums(camera.matrix));
    source.remove(b"U3DPath");
    source.set(b"CO".to_vec(), Object::Real(camera.world_to_camera(camera.center)?[2].max(0.)));
    let mut projection = match source.get(b"P") {
        None => Dict::new(),
        Some(value) => doc.resolve(value).as_dict().cloned().ok_or_else(|| invalid("invalid 3D projection dictionary"))?,
    };
    let binding = |value: Binding| -> Object {
        match value {
            Binding::Width => Object::Name(b"W".to_vec()),
            Binding::Height => Object::Name(b"H".to_vec()),
            Binding::Minimum => Object::Name(b"Min".to_vec()),
            Binding::Maximum => Object::Name(b"Max".to_vec()),
            Binding::Absolute => Object::Name(b"Absolute".to_vec()),
            Binding::Number(n) => Object::Real(n),
        }
    };
    match camera.projection {
        Projection::Perspective { field_of_view, binding: b } => {
            projection.set(b"Subtype".to_vec(), Object::Name(b"P".to_vec()));
            projection.set(b"FOV".to_vec(), Object::Real(field_of_view));
            projection.set(b"PS".to_vec(), binding(b));
        }
        Projection::Orthographic { scale, binding: b } => {
            projection.set(b"Subtype".to_vec(), Object::Name(b"O".to_vec()));
            projection.set(b"OS".to_vec(), Object::Real(scale));
            projection.set(b"OB".to_vec(), binding(b));
        }
    }
    projection.set(b"CS".to_vec(), Object::Name(b"XNF".to_vec()));
    projection.set(b"N".to_vec(), Object::Real(camera.near));
    match camera.far {
        Some(far) => projection.set(b"F".to_vec(), Object::Real(far)),
        None => {
            projection.remove(b"F");
        }
    };
    source.set(b"P".to_vec(), Object::Dict(projection));
    Ok(source)
}
fn capture_camera(doc: &Document, new: &NewMeasurement, storage: &mut ViewStorage) -> Result<()> {
    if let Some(camera) = &new.camera {
        storage.view = camera_view(doc, camera, storage.view.clone())?;
    } else if !storage.view.contains(b"MS") {
        let artwork = artwork(doc, new.page, new.annotation)?;
        let view = if let Some(index) = new.view { Some(artwork.views.get(index).ok_or_else(|| invalid("3D view does not exist"))?) } else { None };
        let camera = crate::three_d_camera::Camera::from_artwork(doc, &artwork, view)?;
        storage.view = camera_view(doc, &camera, storage.view.clone())?;
    }
    Ok(())
}
fn write_view(doc: &mut Document, page_index: usize, annotation_index: usize, mut storage: ViewStorage, measurements: Vec<Object>) -> Result<()> {
    let page = page(doc, page_index)?;
    let mut annotations = annotations(doc, page_index)?;
    if annotation_index >= annotations.len() {
        return Err(invalid("3D annotation does not exist"));
    }
    storage.view.set(b"MA".to_vec(), Object::Array(measurements));
    let view = doc.add(Object::Dict(storage.view));
    let target = storage.views.get_mut(storage.index).ok_or_else(|| invalid("3D view does not exist"))?;
    *target = Object::Ref(view);
    let array = doc.add(Object::Array(storage.views));
    storage.stream.dict.set(b"VA".to_vec(), Object::Ref(array));
    let stream = doc.add(Object::Stream(storage.stream));
    storage.annotation.set(b"3DD".to_vec(), Object::Ref(stream));
    storage.annotation.set(b"3DV".to_vec(), Object::Ref(view));
    let annotation = doc.add(Object::Dict(storage.annotation));
    if let Some(target) = annotations.get_mut(annotation_index) {
        *target = Object::Ref(annotation);
    }
    let array = doc.add(Object::Array(annotations));
    doc.update_dict(page.obj, |d| d.set(b"Annots".to_vec(), Object::Ref(array))).map_err(|e| invalid(&e.to_string()))?;
    extension(doc)
}
#[derive(Clone, Debug, PartialEq)]
pub struct NewMeasurement {
    pub page: usize,
    pub annotation: usize,
    pub view: Option<usize>,
    pub measurement: Measurement,
    pub camera: Option<crate::three_d_camera::Camera>,
}
/// Original models, shared views and unknown dictionaries remain untouched.
fn add_inner(doc: &mut Document, new: &NewMeasurement) -> Result<usize> {
    let measurement = new.measurement.dictionary()?;
    let mut storage = editable_storage(doc, new.page, new.annotation, new.view)?;
    capture_camera(doc, new, &mut storage)?;
    let mut values = measurement_array(doc, &storage.view)?;
    if values.len() >= 4096 {
        return Err(invalid("3D view exceeds 4096 measurements"));
    }
    let index = values.len();
    let reference = doc.add(Object::Dict(measurement));
    values.push(Object::Ref(reference));
    write_view(doc, new.page, new.annotation, storage, values)?;
    Ok(index)
}
fn update_inner(doc: &mut Document, new: &NewMeasurement, index: usize) -> Result<()> {
    let mut storage = editable_storage(doc, new.page, new.annotation, new.view)?;
    capture_camera(doc, new, &mut storage)?;
    let mut values = measurement_array(doc, &storage.view)?;
    let old = values.get(index).ok_or_else(|| invalid("3D measurement does not exist"))?;
    let object = doc.resolve(old);
    let source = object.as_dict().cloned().ok_or_else(|| invalid("invalid 3D measurement dictionary"))?;
    let mut measurement = new.measurement.clone();
    measurement.source = source;
    let d = measurement.dictionary()?;
    let reference = doc.add(Object::Dict(d));
    if let Some(value) = values.get_mut(index) {
        *value = Object::Ref(reference);
    }
    write_view(doc, new.page, new.annotation, storage, values)
}
fn remove_inner(doc: &mut Document, page: usize, annotation: usize, view: Option<usize>, index: usize) -> Result<()> {
    let storage = editable_storage(doc, page, annotation, view)?;
    let mut values = measurement_array(doc, &storage.view)?;
    if index >= values.len() {
        return Err(invalid("3D measurement does not exist"));
    }
    values.remove(index);
    write_view(doc, page, annotation, storage, values)
}

/// Atomically add a measurement, leaving the document unchanged on failure.
pub fn add(doc: &mut Document, new: &NewMeasurement) -> Result<usize> {
    let mut working = doc.clone();
    let index = add_inner(&mut working, new)?;
    *doc = working;
    Ok(index)
}
pub fn update(doc: &mut Document, new: &NewMeasurement, index: usize) -> Result<()> {
    let mut working = doc.clone();
    update_inner(&mut working, new, index)?;
    *doc = working;
    Ok(())
}
pub fn remove(doc: &mut Document, page: usize, annotation: usize, view: Option<usize>, index: usize) -> Result<()> {
    let mut working = doc.clone();
    remove_inner(&mut working, page, annotation, view, index)?;
    *doc = working;
    Ok(())
}
