//! Geospatial page registration and ellipsoidal measurement (ISO 32000-2 §12.10).
//! PDF geographic control pairs are latitude/longitude; projected pairs are easting/northing.
//! Projection calculations never silently substitute WGS84 for the declared datum.
use crate::{Kind, Point, Reading, Result, invalid, nums};
use geographiclib_rs::{Geodesic, InverseGeodesic, PolygonArea, Winding};
use pdfcraft_cos::{Dict, Document, Object, PdfString};
use proj_core::operation::{CoordinateOperation, OperationMethod, SelectionOptions};
use proj_core::{Coord, CrsDef, GeographicCrsDef, Transform};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const MAX_CONTROLS: usize = 128;
const MAX_WKT_BYTES: usize = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CoordinateKind {
    Geographic,
    Projected,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoordinateSystem {
    pub kind: CoordinateKind,
    pub epsg: Option<u32>,
    pub wkt: Option<String>,
}
impl CoordinateSystem {
    fn compile(&self) -> Result<CrsDef> {
        let definition = match (&self.epsg, &self.wkt) {
            (Some(code), None) if *code > 0 => format!("EPSG:{code}"),
            (None, Some(wkt)) if !wkt.is_empty() && wkt.len() <= MAX_WKT_BYTES && wkt.is_ascii() => {
                // Limit nesting before handing untrusted definitions to the parser.
                let mut depth = 0usize;
                let mut quoted = false;
                for byte in wkt.bytes() {
                    if byte == b'"' {
                        quoted = !quoted;
                    }
                    if !quoted {
                        if matches!(byte, b'[' | b'(') {
                            depth += 1;
                        }
                        if matches!(byte, b']' | b')') {
                            depth = depth.checked_sub(1).ok_or_else(|| invalid("unbalanced coordinate system definition"))?;
                        }
                        if depth > 32 {
                            return Err(invalid("coordinate system definition exceeds 32 nesting levels"));
                        }
                    }
                }
                if depth != 0 || quoted {
                    return Err(invalid("unbalanced coordinate system definition"));
                }
                if !wkt.trim_start().starts_with(if self.kind == CoordinateKind::Geographic { "GEOG" } else { "PROJ" }) {
                    return Err(invalid("geospatial WKT must describe a geographic or projected coordinate system"));
                }
                wkt.clone()
            }
            _ => return Err(invalid("a coordinate system needs exactly one positive EPSG code or ASCII WKT definition of at most 16384 bytes")),
        };
        let crs = proj_wkt::parse_crs(&definition).map_err(|e| invalid(&format!("coordinate system: {e}")))?;
        if crs.is_compound() || crs.is_geographic() != (self.kind == CoordinateKind::Geographic) {
            return Err(invalid("coordinate system type does not match its EPSG code or WKT"));
        }
        let ellipsoid = crs.datum().ellipsoid();
        if !(1.0..=1e9).contains(&ellipsoid.semi_major_axis()) || !(0.0..1.0).contains(&ellipsoid.flattening()) {
            return Err(invalid("coordinate system has an invalid ellipsoid"));
        }
        Ok(crs)
    }
    fn read(doc: &Document, object: &Object) -> Result<Self> {
        let object = doc.resolve(object);
        let dict = object.as_dict().ok_or_else(|| invalid("geospatial coordinate system is not a dictionary"))?;
        let kind = match dict.name(b"Type") {
            Some(b"GEOGCS") => CoordinateKind::Geographic,
            Some(b"PROJCS") => CoordinateKind::Projected,
            _ => return Err(invalid("coordinate system Type must be GEOGCS or PROJCS")),
        };
        let epsg = dict
            .get(b"EPSG")
            .map(|o| doc.resolve(o))
            .map(|o| o.as_int().and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0).ok_or_else(|| invalid("EPSG code must be a positive integer")))
            .transpose()?;
        let wkt = dict
            .get(b"WKT")
            .map(|o| doc.resolve(o))
            .map(|o| o.as_string().map(PdfString::to_text).ok_or_else(|| invalid("WKT must be a text string")))
            .transpose()?;
        let out = Self { kind, epsg, wkt };
        out.compile()?;
        Ok(out)
    }
    fn dictionary(&self, original: Option<Dict>) -> Dict {
        let mut d = original.unwrap_or_default();
        d.set(b"Type".to_vec(), Object::name(if self.kind == CoordinateKind::Geographic { "GEOGCS" } else { "PROJCS" }));
        d.remove(b"EPSG");
        d.remove(b"WKT");
        if let Some(epsg) = self.epsg {
            d.set(b"EPSG".to_vec(), i64::from(epsg));
        }
        if let Some(wkt) = &self.wkt {
            d.set(b"WKT".to_vec(), PdfString::text(wkt));
        }
        d
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlPoint {
    pub local: Point,
    pub position: Point,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoDefinition {
    pub gcs: CoordinateSystem,
    pub dcs: Option<CoordinateSystem>,
    pub controls: Vec<ControlPoint>,
    pub bounds: Vec<Point>,
    /// PDF preferred display units: linear, area, angular.
    pub units: [String; 3],
    pub matrix: Option<[f64; 12]>,
}
struct Compiled {
    definition: GeoDefinition,
    crs: CrsDef,
    geographic: Transform,
    display: Transform,
    display_crs: CrsDef,
    map: [f64; 8],
    origin: Point,
    magnitude: f64,
    geodesic: Geodesic,
    residual: f64,
}
#[derive(Clone, Serialize)]
pub struct GeoScale {
    pub definition: GeoDefinition,
    pub bbox: [f64; 4],
    pub registration_residual: f64,
    #[serde(skip)]
    pub source: Option<Dict>,
    #[serde(skip)]
    source_gcs: Option<Dict>,
    #[serde(skip)]
    source_dcs: Option<Dict>,
    #[serde(skip)]
    compiled: Arc<Compiled>,
}
impl std::fmt::Debug for GeoScale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeoScale")
            .field("definition", &self.definition)
            .field("bbox", &self.bbox)
            .field("registration_residual", &self.registration_residual)
            .finish()
    }
}
impl PartialEq for GeoScale {
    fn eq(&self, other: &Self) -> bool {
        self.definition == other.definition && self.bbox == other.bbox && self.source == other.source
    }
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GeoCoordinate {
    pub latitude: f64,
    pub longitude: f64,
    pub native: Point,
    pub display: Point,
    pub display_system: String,
    pub display_unit: String,
    pub display_operation: String,
    pub display_accuracy_meters: Option<f64>,
    pub display_approximate: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GeoReading {
    pub coordinates: Vec<GeoCoordinate>,
    pub registration_residual: f64,
    pub ellipsoid_semi_major: f64,
    pub ellipsoid_flattening: f64,
}
fn linear(unit: &str) -> Result<(f64, &'static str)> {
    Ok(match unit {
        "M" => (1.0, "m"),
        "KM" => (1000.0, "km"),
        "FT" => (0.3048, "ft"),
        "USFT" => (1200.0 / 3937.0, "US ft"),
        "MI" => (1609.344, "mi"),
        "NM" => (1852.0, "nmi"),
        _ => return Err(invalid("linear map unit must be M, KM, FT, USFT, MI or NM")),
    })
}
fn area_unit(unit: &str) -> Result<(f64, &'static str)> {
    Ok(match unit {
        "SQM" => (1.0, "m²"),
        "HA" => (10_000.0, "ha"),
        "SQKM" => (1_000_000.0, "km²"),
        "SQFT" => ((1200.0_f64 / 3937.0).powi(2), "US ft²"),
        "A" => (4046.8564224, "acre"),
        "SQMI" => (1609.344_f64.powi(2), "mi²"),
        _ => return Err(invalid("area map unit must be SQM, HA, SQKM, SQFT, A or SQMI")),
    })
}
fn normalize_lon(v: f64) -> f64 {
    (v + 180.0).rem_euclid(360.0) - 180.0
}
fn inside(p: Point, polygon: &[Point]) -> bool {
    let mut odd = false;
    for (a, b) in polygon.iter().zip(polygon.iter().cycle().skip(1)).take(polygon.len()) {
        let cross = (p[0] - a[0]) * (b[1] - a[1]) - (p[1] - a[1]) * (b[0] - a[0]);
        if cross.abs() < 1e-10
            && p[0] >= a[0].min(b[0]) - 1e-10
            && p[0] <= a[0].max(b[0]) + 1e-10
            && p[1] >= a[1].min(b[1]) - 1e-10
            && p[1] <= a[1].max(b[1]) + 1e-10
        {
            return true;
        }
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            odd = !odd;
        }
    }
    odd
}
fn solve(mut a: [[f64; 9]; 8], n: usize) -> Result<[f64; 8]> {
    for k in 0..n {
        let pivot =
            (k..n).max_by(|i, j| a[*i][k].abs().total_cmp(&a[*j][k].abs())).ok_or_else(|| invalid("geospatial registration lacks control points"))?;
        if a[pivot][k].abs() < 1e-12 {
            return Err(invalid("geospatial control points are collinear or degenerate"));
        }
        a.swap(k, pivot);
        let divisor = a[k][k];
        for v in &mut a[k][k..=n] {
            *v /= divisor;
        }
        for row in 0..n {
            if row != k {
                let factor = a[row][k];
                let pivot_row = a[k];
                for (value, pivot) in a[row][k..=n].iter_mut().zip(&pivot_row[k..=n]) {
                    *value -= factor * pivot;
                }
            }
        }
    }
    let mut out = [0.0; 8];
    for i in 0..n {
        out[i] = a[i][n];
    }
    if out.iter().any(|v| !v.is_finite()) {
        return Err(invalid("geospatial registration overflows"));
    }
    Ok(out)
}
fn project(map: [f64; 8], p: Point) -> Result<Point> {
    let denominator = 1.0 + map[6] * p[0] + map[7] * p[1];
    if !denominator.is_finite() || denominator.abs() < 1e-12 {
        return Err(invalid("geospatial point lies on a singular registration transform"));
    }
    let out = [(map[0] * p[0] + map[1] * p[1] + map[2]) / denominator, (map[3] * p[0] + map[4] * p[1] + map[5]) / denominator];
    if out.iter().any(|v| !v.is_finite()) {
        return Err(invalid("geospatial registration overflows"));
    }
    Ok(out)
}
// Both definitions here share the very same declared datum, including custom
// datums with no path to WGS84. No geographic datum transformation is needed.
fn within_map_datum(from: &CrsDef, to: &CrsDef) -> Result<Transform> {
    let operation = CoordinateOperation {
        id: None,
        name: "Projection within the declared map datum".into(),
        source_crs_epsg: None,
        target_crs_epsg: None,
        source_datum_epsg: None,
        target_datum_epsg: None,
        accuracy: None,
        areas_of_use: Default::default(),
        deprecated: false,
        preferred: true,
        approximate: false,
        superseded: false,
        method: OperationMethod::Identity,
    };
    Transform::from_crs_defs_with_selection_options(from, to, SelectionOptions::new().with_coordinate_operation(operation))
        .map_err(|e| invalid(&e.to_string()))
}
impl GeoScale {
    pub fn new(definition: GeoDefinition, bbox: [f64; 4]) -> Result<Self> {
        crate::check_points(&[[bbox[0], bbox[1]], [bbox[2], bbox[3]]])?;
        if bbox[2] <= bbox[0] || bbox[3] <= bbox[1] {
            return Err(invalid("geospatial viewport bounds must have positive width and height"));
        }
        let uses_matrix = definition.matrix.is_some() && definition.gcs.kind == CoordinateKind::Projected;
        let minimum = if uses_matrix { 1 } else { 3 };
        if !(minimum..=MAX_CONTROLS).contains(&definition.controls.len()) {
            return Err(invalid("geospatial registration needs 3 to 128 control points, or 1 to 128 with a projected matrix"));
        }
        if !(3..=128).contains(&definition.bounds.len()) || definition.bounds.iter().flatten().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)) {
            return Err(invalid("geospatial bounds need 3 to 128 points inside the unit square"));
        }
        crate::validate_geometry(Kind::Area, &definition.bounds)?;
        let boundary_area = definition
            .bounds
            .iter()
            .zip(definition.bounds.iter().cycle().skip(1))
            .take(definition.bounds.len())
            .map(|(a, b)| a[0] * b[1] - a[1] * b[0])
            .sum::<f64>()
            .abs();
        if boundary_area < 1e-12 {
            return Err(invalid("geospatial map bounds must enclose a positive area"));
        }
        let crs = definition.gcs.compile()?;
        let geographic_crs = CrsDef::Geographic(GeographicCrsDef::new(0, crs.datum().clone(), "Map geographic coordinates"));
        let geographic = within_map_datum(&crs, &geographic_crs)?;
        let display_crs = definition.dcs.as_ref().map(CoordinateSystem::compile).transpose()?.unwrap_or(geographic_crs);
        let display = if definition.dcs.is_none() || definition.dcs.as_ref() == Some(&definition.gcs) {
            within_map_datum(&crs, &display_crs)?
        } else {
            Transform::from_crs_defs(&crs, &display_crs).map_err(|e| invalid(&format!("display coordinate system: {e}")))?
        };
        linear(&definition.units[0])?;
        area_unit(&definition.units[1])?;
        if !matches!(definition.units[2].as_str(), "DEG" | "GRD") {
            return Err(invalid("angular map unit must be DEG or GRD"));
        }
        if definition.matrix.is_some_and(|m| m.iter().any(|v| !v.is_finite() || v.abs() > 1e12)) {
            return Err(invalid("geospatial matrix must contain 12 finite numbers within 1e12"));
        }
        let mut positions = Vec::with_capacity(definition.controls.len());
        for control in &definition.controls {
            if control.local.iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
                || control.position.iter().any(|v| !v.is_finite() || v.abs() > 1e12)
            {
                return Err(invalid("invalid geospatial control point"));
            }
            let mut p = control.position;
            if crs.is_geographic() {
                if p[0].abs() > 90.0 || p[1].abs() > 180.0 {
                    return Err(invalid("geographic control points must be latitude then longitude in degrees"));
                }
                p.swap(0, 1);
                if let Some(first) = positions.first() {
                    let first: &Point = first;
                    p[0] = first[0] + normalize_lon(p[0] - first[0]);
                }
            }
            positions.push(p);
        }
        let origin = positions.first().copied().ok_or_else(|| invalid("missing geospatial controls"))?;
        let magnitude = positions.iter().flatten().zip(origin.iter().cycle()).map(|(v, o)| (v - o).abs()).fold(1e-12_f64, f64::max);
        let (map, residual) = if uses_matrix {
            let matrix = definition.matrix.ok_or_else(|| invalid("missing projected coordinate matrix"))?;
            let determinant = matrix[0] * matrix[4] - matrix[1] * matrix[3];
            if determinant.abs() < 1e-15 {
                return Err(invalid("projected coordinate matrix collapses the map plane"));
            }
            ([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0], 0.0)
        } else {
            let n = if positions.len() == 3 { 6 } else { 8 };
            let mut normal = [[0.0; 9]; 8];
            for (control, p) in definition.controls.iter().zip(&positions) {
                let [u, v] = control.local;
                let x = (p[0] - origin[0]) / magnitude;
                let y = (p[1] - origin[1]) / magnitude;
                for (row, target) in [([u, v, 1.0, 0.0, 0.0, 0.0, -u * x, -v * x], x), ([0.0, 0.0, 0.0, u, v, 1.0, -u * y, -v * y], y)] {
                    for i in 0..n {
                        for j in 0..n {
                            normal[i][j] += row[i] * row[j];
                        }
                        normal[i][n] += row[i] * target;
                    }
                }
            }
            let map = solve(normal, n)?;
            // A denominator changing sign inside the unit square is an invalid page mapping.
            let signs: [[f64; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
            let denominators = signs.map(|p| 1.0 + map[6] * p[0] + map[7] * p[1]);
            if denominators.iter().any(|v| !v.is_finite() || *v < 1e-10) {
                return Err(invalid("geospatial registration has a singularity within the viewport"));
            }
            let mut residual = 0.0_f64;
            for (control, p) in definition.controls.iter().zip(&positions) {
                let predicted = project(map, control.local)?;
                residual = residual
                    .max(((predicted[0] * magnitude + origin[0] - p[0]).powi(2) + (predicted[1] * magnitude + origin[1] - p[1]).powi(2)).sqrt());
            }
            if residual / magnitude > 0.01 {
                return Err(invalid("geospatial control points disagree by more than one percent of the map span"));
            }
            (map, residual)
        };
        let ellipsoid = crs.datum().ellipsoid();
        let geodesic = Geodesic::new(ellipsoid.semi_major_axis(), ellipsoid.flattening());
        let compiled =
            Arc::new(Compiled { definition: definition.clone(), crs, geographic, display, display_crs, map, origin, magnitude, geodesic, residual });
        Ok(Self { definition, bbox, registration_residual: residual, source: None, source_gcs: None, source_dcs: None, compiled })
    }
    /// Recompile an edited registration while retaining unknown fields and unchanged references.
    pub fn updated(&self, definition: GeoDefinition, bbox: [f64; 4]) -> Result<Self> {
        let mut out = Self::new(definition, bbox)?;
        let fresh = out.dictionary();
        let mut original = self.dictionary();
        if out.definition.gcs != self.definition.gcs {
            let d = out.definition.gcs.dictionary(self.source_gcs.clone());
            original.set(b"GCS".to_vec(), d.clone());
            out.source_gcs = Some(d);
        } else {
            out.source_gcs = self.source_gcs.clone();
        }
        if out.definition.dcs != self.definition.dcs {
            match &out.definition.dcs {
                Some(s) => {
                    let d = s.dictionary(self.source_dcs.clone());
                    original.set(b"DCS".to_vec(), d.clone());
                    out.source_dcs = Some(d);
                }
                None => {
                    original.remove(b"DCS");
                }
            }
        } else {
            out.source_dcs = self.source_dcs.clone();
        }
        for (key, changed) in [
            (b"GPTS".as_slice(), out.definition.controls != self.definition.controls),
            (b"LPTS", out.definition.controls != self.definition.controls),
            (b"Bounds", out.definition.bounds != self.definition.bounds),
            (b"PDU", out.definition.units != self.definition.units),
            (b"PCSM", out.definition.matrix != self.definition.matrix),
        ] {
            if changed {
                match fresh.get(key) {
                    Some(value) => {
                        original.set(key.to_vec(), value.clone());
                    }
                    None => {
                        original.remove(key);
                    }
                }
            }
        }
        original.set(b"PCGeoBBox".to_vec(), nums(bbox));
        out.source = Some(original);
        Ok(out)
    }
    pub fn validate(&self) -> Result<()> {
        if self.definition != self.compiled.definition {
            return Err(invalid("geospatial registration was edited without rebuilding its projection; use GeoScale::new"));
        }
        crate::check_points(&[[self.bbox[0], self.bbox[1]], [self.bbox[2], self.bbox[3]]])?;
        if self.bbox[2] <= self.bbox[0] || self.bbox[3] <= self.bbox[1] {
            return Err(invalid("geospatial viewport bounds must have positive width and height"));
        }
        Ok(())
    }
    pub fn linear_unit(&self) -> &'static str {
        linear(&self.definition.units[0]).map(|(_, label)| label).unwrap_or("m")
    }
    pub fn area_unit(&self) -> &'static str {
        area_unit(&self.definition.units[1]).map(|(_, label)| label).unwrap_or("m²")
    }
    pub fn coordinate(&self, point: Point) -> Result<GeoCoordinate> {
        self.validate()?;
        crate::check_points(&[point])?;
        let local = [(point[0] - self.bbox[0]) / (self.bbox[2] - self.bbox[0]), (point[1] - self.bbox[1]) / (self.bbox[3] - self.bbox[1])];
        if !inside(local, &self.definition.bounds) {
            return Err(invalid("measurement point is outside the geospatial map bounds"));
        }
        let c = &self.compiled;
        let mut native = if let Some(m) = self.definition.matrix.filter(|_| c.crs.is_projected()) {
            [point[0] * m[0] + point[1] * m[3] + m[9], point[0] * m[1] + point[1] * m[4] + m[10]]
        } else {
            let p = project(c.map, local)?;
            [p[0] * c.magnitude + c.origin[0], p[1] * c.magnitude + c.origin[1]]
        };
        if c.crs.is_geographic() {
            native[0] = normalize_lon(native[0]);
        }
        let geo = c.geographic.convert(Coord::new(native[0], native[1])).map_err(|e| invalid(&format!("map projection: {e}")))?;
        let display = c.display.convert(Coord::new(native[0], native[1])).map_err(|e| invalid(&format!("display projection: {e}")))?;
        if !geo.x.is_finite() || !geo.y.is_finite() || geo.y.abs() > 90.0 || !display.x.is_finite() || !display.y.is_finite() {
            return Err(invalid("geospatial transform produces invalid coordinates"));
        }
        let angular = if self.definition.units[2] == "GRD" { 10.0 / 9.0 } else { 1.0 };
        let (display, display_unit) = if c.display_crs.is_geographic() {
            ([display.y * angular, normalize_lon(display.x) * angular], if angular == 1.0 { "°".into() } else { "grad".into() })
        } else {
            ([display.x, display.y], format!("{} m per unit", c.display_crs.as_projected().map(|p| p.linear_unit().meters_per_unit()).unwrap_or(1.0)))
        };
        Ok(GeoCoordinate {
            latitude: geo.y,
            longitude: normalize_lon(geo.x),
            native: if c.crs.is_geographic() { [native[1], native[0]] } else { native },
            display,
            display_system: c.display_crs.name().into(),
            display_unit,
            display_operation: c.display.selected_operation().name.clone(),
            display_accuracy_meters: c.display.selected_operation().accuracy.map(|a| a.meters),
            display_approximate: c.display.selected_operation().approximate,
        })
    }
    pub fn reading(&self, kind: Kind, points: &[Point], precision: u8) -> Result<Reading> {
        crate::check_points(points)?;
        if precision > 6 {
            return Err(invalid("precision must be between 0 and 6 decimal places"));
        }
        let coordinates: Vec<_> = points.iter().map(|p| self.coordinate(*p)).collect::<Result<_>>()?;
        self.validate()?;
        let c = &self.compiled;
        let (linear_factor, linear_label) = linear(&self.definition.units[0])?;
        let (area_factor, area_label) = area_unit(&self.definition.units[1])?;
        let mut length = 0.0;
        for pair in coordinates.windows(2) {
            if let [a, b] = pair {
                let distance: f64 = c.geodesic.inverse(a.latitude, a.longitude, b.latitude, b.longitude);
                length += distance;
            }
        }
        length /= linear_factor;
        let mut polygon = PolygonArea::new(&c.geodesic, Winding::CounterClockwise);
        for p in &coordinates {
            polygon.add_point(p.latitude, p.longitude);
        }
        let area = if coordinates.len() >= 3 { polygon.compute(true).1.abs() / area_factor } else { 0.0 };
        let (delta, angle) = match (coordinates.first(), coordinates.last()) {
            (Some(a), Some(b)) => {
                let (azimuth, _, _): (f64, f64, f64) = c.geodesic.inverse(a.latitude, a.longitude, b.latitude, b.longitude);
                ([normalize_lon(b.longitude - a.longitude), b.latitude - a.latitude], azimuth)
            }
            _ => ([0.0; 2], 0.0),
        };
        let angle = if points.len() < 2 || delta == [0.0; 2] { 0.0 } else { angle };
        let angular = if self.definition.units[2] == "GRD" { 10.0 / 9.0 } else { 1.0 };
        let angular_label = if angular == 1.0 { "°" } else { " grad" };
        let delta = [delta[0] * angular, delta[1] * angular];
        let value = if kind == Kind::Area { area } else { length };
        if !value.is_finite() || !angle.is_finite() {
            return Err(invalid("geospatial measurement overflows"));
        }
        let unit = if kind == Kind::Area { area_label } else { linear_label };
        let label = crate::format::label(value, &[crate::NumberFormat::decimal(unit, 1.0, precision)])?;
        let ellipsoid = c.crs.datum().ellipsoid();
        Ok(Reading {
            kind,
            value,
            unit: unit.into(),
            label,
            delta_x: delta[0],
            delta_y: delta[1],
            delta_x_label: format!("{:.6}{angular_label}", delta[0]),
            delta_y_label: format!("{:.6}{angular_label}", delta[1]),
            angle,
            angle_label: format!("{:.2}{angular_label}", angle * angular),
            length,
            area,
            geospatial: Some(GeoReading {
                coordinates,
                registration_residual: c.residual,
                ellipsoid_semi_major: ellipsoid.semi_major_axis(),
                ellipsoid_flattening: ellipsoid.flattening(),
            }),
        })
    }
    pub fn read(doc: &Document, dict: &Dict, bbox: [f64; 4]) -> Result<Self> {
        fn numbers(doc: &Document, dict: &Dict, key: &[u8], max: usize) -> Result<Vec<f64>> {
            let object = dict.get(key).ok_or_else(|| invalid(&format!("geospatial measure lacks {}", String::from_utf8_lossy(key))))?;
            let object = doc.resolve(object);
            let values = object.as_array().ok_or_else(|| invalid("geospatial coordinates must be an array"))?;
            if values.len() > max {
                return Err(invalid("geospatial coordinate array exceeds its limit"));
            }
            values.iter().map(|o| doc.resolve(o).as_f64().filter(|v| v.is_finite()).ok_or_else(|| invalid("invalid geospatial coordinate"))).collect()
        }
        let gcs = CoordinateSystem::read(doc, dict.get(b"GCS").ok_or_else(|| invalid("geospatial measure lacks GCS"))?)?;
        let dcs = dict.get(b"DCS").map(|o| CoordinateSystem::read(doc, o)).transpose()?;
        let gpts = numbers(doc, dict, b"GPTS", MAX_CONTROLS * 2)?;
        let lpts = numbers(doc, dict, b"LPTS", MAX_CONTROLS * 2)?;
        if gpts.len() != lpts.len() || gpts.len() % 2 != 0 {
            return Err(invalid("GPTS and LPTS need the same number of coordinate pairs"));
        }
        let controls = lpts.as_chunks::<2>().0.iter().zip(gpts.as_chunks::<2>().0).map(|(l, g)| ControlPoint { local: *l, position: *g }).collect();
        let bounds = if dict.contains(b"Bounds") {
            let n = numbers(doc, dict, b"Bounds", 256)?;
            if n.len() % 2 != 0 {
                return Err(invalid("geospatial Bounds need coordinate pairs"));
            }
            n.as_chunks::<2>().0.to_vec()
        } else {
            vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
        };
        let mut units = ["M".into(), "SQM".into(), "DEG".into()];
        if let Some(pdu) = dict.get(b"PDU") {
            let pdu = doc.resolve(pdu);
            let values = pdu.as_array().filter(|a| a.len() == 3).ok_or_else(|| invalid("PDU needs exactly three unit names"))?;
            for (unit, value) in units.iter_mut().zip(values) {
                let value = doc.resolve(value);
                *unit = std::str::from_utf8(value.as_name().ok_or_else(|| invalid("PDU entries must be unit names"))?)
                    .map_err(|_| invalid("invalid PDU unit"))?
                    .into();
            }
        }
        let matrix = if dict.contains(b"PCSM") && gcs.kind == CoordinateKind::Projected {
            Some(numbers(doc, dict, b"PCSM", 12)?.try_into().map_err(|_| invalid("PCSM needs 12 numbers"))?)
        } else {
            None
        };
        let mut out = Self::new(GeoDefinition { gcs, dcs, controls, bounds, units, matrix }, bbox)?;
        out.source_gcs = dict.get(b"GCS").and_then(|o| doc.dict(o));
        out.source_dcs = dict.get(b"DCS").and_then(|o| doc.dict(o));
        out.source = Some(dict.clone());
        Ok(out)
    }
    pub fn dictionary(&self) -> Dict {
        if let Some(source) = &self.source {
            let mut d = source.clone();
            d.set(b"PCGeoBBox".to_vec(), nums(self.bbox));
            return d;
        }
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("Measure"));
        d.set(b"Subtype".to_vec(), Object::name("GEO"));
        let original = |key: &[u8]| d.get(key).and_then(Object::as_dict).cloned();
        let gcs = self.definition.gcs.dictionary(original(b"GCS"));
        let dcs = self.definition.dcs.as_ref().map(|s| s.dictionary(original(b"DCS")));
        d.set(b"GCS".to_vec(), gcs);
        if let Some(dcs) = dcs {
            d.set(b"DCS".to_vec(), dcs);
        } else {
            d.remove(b"DCS");
        }
        d.set(b"GPTS".to_vec(), nums(self.definition.controls.iter().flat_map(|p| p.position)));
        d.set(b"LPTS".to_vec(), nums(self.definition.controls.iter().flat_map(|p| p.local)));
        d.set(b"Bounds".to_vec(), nums(self.definition.bounds.iter().flatten().copied()));
        d.set(b"PDU".to_vec(), Object::Array(self.definition.units.iter().map(|s| Object::name(s)).collect()));
        if let Some(matrix) = self.definition.matrix {
            d.set(b"PCSM".to_vec(), nums(matrix));
        } else if self.definition.gcs.kind == CoordinateKind::Projected {
            d.remove(b"PCSM");
        }
        // Capture the originating viewport for a saved measurement. Later viewport edits
        // must not reinterpret old annotation points against a new registration rectangle.
        d.set(b"PCGeoBBox".to_vec(), nums(self.bbox));
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn definition() -> GeoDefinition {
        GeoDefinition {
            gcs: CoordinateSystem { kind: CoordinateKind::Geographic, epsg: Some(4326), wkt: None },
            dcs: None,
            controls: vec![
                ControlPoint { local: [0.0, 0.0], position: [0.0, 0.0] },
                ControlPoint { local: [1.0, 0.0], position: [0.0, 1.0] },
                ControlPoint { local: [1.0, 1.0], position: [1.0, 1.0] },
                ControlPoint { local: [0.0, 1.0], position: [1.0, 0.0] },
            ],
            bounds: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            units: ["M".into(), "SQM".into(), "DEG".into()],
            matrix: None,
        }
    }
    #[test]
    fn geographic_coordinates_ellipsoidal_length_and_area() {
        let geo = GeoScale::new(definition(), [10.0, 20.0, 110.0, 120.0]).unwrap();
        let p = geo.coordinate([60.0, 70.0]).unwrap();
        assert!((p.latitude - 0.5).abs() < 1e-10);
        assert!((p.longitude - 0.5).abs() < 1e-10);
        let line = geo.reading(Kind::Distance, &[[10.0, 20.0], [110.0, 20.0]], 3).unwrap();
        assert!((line.value - 111319.49079327357).abs() < 1e-6);
        let area = geo.reading(Kind::Area, &[[10.0, 20.0], [110.0, 20.0], [110.0, 120.0], [10.0, 120.0]], 3).unwrap();
        assert!((area.value - 12308778361.469452).abs() < 0.01);
        assert!(geo.coordinate([9.0, 20.0]).is_err());
    }
    #[test]
    fn projected_controls_native_units_and_display_projection() {
        let mut d = definition();
        d.gcs = CoordinateSystem { kind: CoordinateKind::Projected, epsg: Some(3857), wkt: None };
        for control in &mut d.controls {
            control.position = [control.local[0] * 111319.49079327357, control.local[1] * 111325.1428663851];
        }
        d.dcs = Some(CoordinateSystem { kind: CoordinateKind::Geographic, epsg: Some(4326), wkt: None });
        let geo = GeoScale::new(d, [0.0, 0.0, 100.0, 100.0]).unwrap();
        let p = geo.coordinate([100.0, 100.0]).unwrap();
        assert!((p.latitude - 1.0).abs() < 1e-9);
        assert!((p.longitude - 1.0).abs() < 1e-9);
        let line = geo.reading(Kind::Distance, &[[0.0, 0.0], [100.0, 0.0]], 3).unwrap();
        assert!((line.value - 111319.49079327357).abs() < 1e-6);
    }
    #[test]
    fn antimeridian_and_non_wgs84_ellipsoid_are_preserved() {
        let mut d = definition();
        for c in &mut d.controls {
            c.position[1] = if c.local[0] == 0.0 { 179.0 } else { -179.0 };
        }
        let geo = GeoScale::new(d, [0.0, 0.0, 100.0, 100.0]).unwrap();
        let p = geo.coordinate([50.0, 50.0]).unwrap();
        assert!((p.longitude.abs() - 180.0).abs() < 1e-9);
        let line = geo.reading(Kind::Distance, &[[0.0, 0.0], [100.0, 0.0]], 3).unwrap();
        assert!((line.value - 222638.98158654713).abs() < 1e-5);
        let mut d = definition();
        d.gcs.epsg = Some(4269);
        let geo = GeoScale::new(d, [0.0, 0.0, 100.0, 100.0]).unwrap();
        assert!((geo.compiled.geodesic.flattening() - 1.0 / 298.257222101).abs() < 1e-15);
    }
    #[test]
    fn singular_and_hostile_registration_definitions_return_errors() {
        let mut d = definition();
        for c in &mut d.controls {
            c.local[1] = 0.0;
        }
        assert!(GeoScale::new(d, [0.0, 0.0, 100.0, 100.0]).is_err());
        let mut d = definition();
        d.controls[0].position[0] = f64::NAN;
        assert!(GeoScale::new(d, [0.0, 0.0, 100.0, 100.0]).is_err());
        let mut d = definition();
        d.gcs.epsg = None;
        d.gcs.wkt = Some(format!("GEOGCS{}{}", "[".repeat(10000), "]".repeat(10000)));
        assert!(GeoScale::new(d, [0.0, 0.0, 100.0, 100.0]).is_err());
        let mut geo = GeoScale::new(definition(), [0.0, 0.0, 100.0, 100.0]).unwrap();
        geo.definition.units[0] = "MI".into();
        assert!(geo.coordinate([0.0, 0.0]).is_err());
    }
    #[test]
    fn dictionary_preserves_unknown_entries_and_indirect_data() {
        let mut doc = Document::new_empty();
        let geo = GeoScale::new(definition(), [10.0, 20.0, 110.0, 120.0]).unwrap();
        let mut dictionary = geo.dictionary();
        dictionary.set(b"VendorGeo".to_vec(), Object::Int(42));
        let gcs = doc.add(dictionary.get(b"GCS").unwrap().clone());
        dictionary.set(b"GCS".to_vec(), gcs);
        let parsed = GeoScale::read(&doc, &dictionary, geo.bbox).unwrap();
        assert_eq!(parsed.dictionary(), dictionary);
    }
    #[test]
    fn projected_matrix_has_priority_and_uses_pdf_user_coordinates() {
        let mut d = definition();
        d.gcs = CoordinateSystem { kind: CoordinateKind::Projected, epsg: Some(3857), wkt: None };
        d.controls.truncate(1);
        d.controls[0].position = [123.0, 456.0];
        d.matrix = Some([1113.1949079327357, 0.0, 0.0, 0.0, 1113.251428663851, 0.0, 0.0, 0.0, 1.0, -11131.949079327357, -22265.02857327702, 0.0]);
        let geo = GeoScale::new(d.clone(), [10.0, 20.0, 110.0, 120.0]).unwrap();
        let p = geo.coordinate([110.0, 120.0]).unwrap();
        assert!((p.longitude - 1.0).abs() < 1e-9);
        assert!((p.latitude - 1.0).abs() < 1e-9);
        d.matrix.as_mut().unwrap()[4] = 0.0;
        assert!(GeoScale::new(d, geo.bbox).is_err());
        let mut d = definition();
        d.matrix = Some([0.0; 12]);
        let geo = GeoScale::new(d, [0.0, 0.0, 100.0, 100.0]).unwrap();
        assert!((geo.coordinate([100.0, 100.0]).unwrap().longitude - 1.0).abs() < 1e-9);
    }
    #[test]
    fn edited_coordinate_system_preserves_unknown_nested_data() {
        let mut doc = Document::new_empty();
        let geo = GeoScale::new(definition(), [0.0, 0.0, 100.0, 100.0]).unwrap();
        let mut dict = geo.dictionary();
        let mut gcs = dict.get(b"GCS").unwrap().as_dict().unwrap().clone();
        gcs.set(b"VendorDatum".to_vec(), PdfString::text("retain me"));
        let gcs_ref = doc.add(gcs);
        dict.set(b"GCS".to_vec(), gcs_ref);
        dict.set(b"VendorGeo".to_vec(), 71i64);
        let parsed = GeoScale::read(&doc, &dict, geo.bbox).unwrap();
        let unchanged = parsed.updated(parsed.definition.clone(), [10.0, 20.0, 110.0, 120.0]).unwrap();
        assert_eq!(unchanged.dictionary().get(b"GCS"), dict.get(b"GCS"));
        let mut definition = parsed.definition.clone();
        definition.gcs.epsg = Some(4269);
        let edited = parsed.updated(definition, geo.bbox).unwrap().dictionary();
        assert_eq!(edited.get(b"VendorGeo"), dict.get(b"VendorGeo"));
        let new_gcs = edited.get(b"GCS").unwrap().as_dict().unwrap();
        assert_eq!(new_gcs.get(b"VendorDatum"), Some(&Object::String(PdfString::text("retain me"))));
        assert_eq!(new_gcs.get(b"EPSG"), Some(&Object::Int(4269)));
        assert_eq!(doc.get(gcs_ref).as_dict().unwrap().get(b"EPSG"), Some(&Object::Int(4326)));
    }
    #[test]
    fn all_preferred_units_and_custom_wkt_ellipsoid_are_used() {
        let bbox = [0.0, 0.0, 100.0, 100.0];
        for linear_unit in ["M", "KM", "FT", "USFT", "MI", "NM"] {
            for area in ["SQM", "HA", "SQKM", "SQFT", "A", "SQMI"] {
                let mut d = definition();
                d.units = [linear_unit.into(), area.into(), "GRD".into()];
                let geo = GeoScale::new(d, bbox).unwrap();
                let line = geo.reading(Kind::Distance, &[[0.0, 0.0], [100.0, 0.0]], 6).unwrap();
                assert!((line.value * linear(linear_unit).unwrap().0 - 111319.49079327357).abs() < 1e-6);
                assert_eq!(line.angle_label, "100.00 grad");
                let polygon = geo.reading(Kind::Area, &[[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]], 6).unwrap();
                assert!((polygon.value * area_unit(area).unwrap().0 - 12308778361.469452).abs() < 0.01);
            }
        }
        let mut d = definition();
        d.gcs.epsg = None;
        d.gcs.wkt = Some(
            r#"GEOGCS["Custom sphere",DATUM["Custom",SPHEROID["Sphere",6371000,0]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#.into(),
        );
        let geo = GeoScale::new(d, bbox).unwrap();
        let line = geo.reading(Kind::Distance, &[[0.0, 0.0], [100.0, 0.0]], 3).unwrap();
        assert!((line.value - 6371000.0 * std::f64::consts::PI / 180.0).abs() < 1e-6);
        assert_eq!(line.geospatial.unwrap().ellipsoid_flattening, 0.0);
    }
}
