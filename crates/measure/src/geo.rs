//! Geospatial registration of page viewports: where a point on a map page is on the Earth.
//!
//! ISO 32000-2 §12.9 defines a geospatial measure dictionary (`/Subtype /GEO`) that a viewport
//! carries. It pairs points in the viewport's unit square (`/LPTS`, defaulting to `/Bounds`) with
//! latitudes and longitudes (`/GPTS`) in the coordinate system that `/GCS` names, by EPSG code or
//! well-known text (WKT). Only the area inside `/Bounds` is registered.
//!
//! A projected coordinate system is handled as the model intends: the control points are projected,
//! page space is fitted to the projected plane, and a page point is fitted and then unprojected.
//! Transverse Mercator (UTM and its national variants), Lambert Conformal Conic (one or two standard
//! parallels) and Web Mercator are implemented from their published formulas. For a geographic
//! system page space is fitted to latitude and longitude directly. For any other projection the
//! latitudes and longitudes are fitted directly too, and the result is marked approximate.
//!
//! Every input is untrusted: arrays, text and nesting are bounded, and a viewport that can't be
//! read is reported with a reason, never a panic. The PDF data is only read.
use super::{MAX_VIEWPORTS, Point, Result, check_points, invalid, page, text};
use pdfcraft_cos::{Dict, Document, Object};
use serde::Serialize;
use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

/// Most control points (and `/Bounds` vertices) read from one geospatial measure dictionary.
const MAX_GEO_POINTS: usize = 256;
/// Longest WKT one coordinate system may have.
const MAX_WKT: usize = 64 * 1024;
/// WKT bytes one page reads in all, however many viewports name a coordinate system.
const MAX_PAGE_WKT: usize = 4 * 1024 * 1024;
/// Deepest WKT nesting read.
const MAX_WKT_DEPTH: usize = 32;
/// Most WKT nodes read from one text.
const MAX_WKT_NODES: usize = 4096;

/// How page space is related to the Earth.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Method {
    /// Fitted to latitude and longitude of a geographic coordinate system.
    Geographic,
    /// Fitted in the plane of a projection PdfCraft implements, then unprojected.
    Projected,
    /// The projection isn't one PdfCraft implements: latitude and longitude are fitted directly,
    /// which is close over a small map and drifts over a large one.
    Approximate,
}

/// The coordinate system a viewport is registered in.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Crs {
    /// Its name in the WKT, else `EPSG:n`.
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epsg: Option<i64>,
    /// A projected coordinate system (`PROJCS`) rather than a geographic one (`GEOGCS`).
    pub projected: bool,
}

/// A point on the Earth found from a point on the page.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Location {
    /// Degrees north, -90 to 90.
    pub lat: f64,
    /// Degrees east, -180 to 180.
    pub lon: f64,
    /// `34.136750, -118.125028`.
    pub decimal: String,
    /// `34°08'12.3"N 118°07'30.1"W`.
    pub dms: String,
    pub crs: Crs,
    pub method: Method,
    /// The `/Name` of the viewport that registered the point, if it has one.
    pub viewport: String,
}

/// An ellipsoid by its semi-major axis (metres) and flattening.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Ellipsoid {
    a: f64,
    f: f64,
}
const WGS84: Ellipsoid = Ellipsoid { a: 6_378_137.0, f: 1.0 / 298.257_223_563 };
const GRS80: Ellipsoid = Ellipsoid { a: 6_378_137.0, f: 1.0 / 298.257_222_101 };
const CLARKE_1866: Ellipsoid = Ellipsoid { a: 6_378_206.4, f: 1.0 / 294.978_698_214 };
impl Ellipsoid {
    fn new(a: f64, inverse_flattening: f64) -> Option<Self> {
        let f = if inverse_flattening == 0.0 { 0.0 } else { 1.0 / inverse_flattening };
        (a.is_finite() && (1e5..=1e8).contains(&a) && f.is_finite() && (0.0..0.1).contains(&f)).then_some(Self { a, f })
    }
    fn e(self) -> f64 {
        (self.f * (2.0 - self.f)).sqrt()
    }
}

/// A map projection between geodetic latitude/longitude (radians) and a plane. The plane's
/// units are those of its false origin; the page fit absorbs them, and constant offsets cancel.
#[derive(Clone, Debug, PartialEq)]
enum Projection {
    TransverseMercator(TransverseMercator),
    Lambert(Lambert),
    WebMercator { r: f64 },
}
impl Projection {
    fn forward(&self, lat: f64, lon: f64) -> Option<[f64; 2]> {
        let out = match self {
            Self::TransverseMercator(p) => p.forward(lat, lon),
            Self::Lambert(p) => p.forward(lat, lon),
            Self::WebMercator { r } => {
                if lat.abs() >= FRAC_PI_2 {
                    return None;
                }
                [r * lon, r * (FRAC_PI_4 + lat / 2.0).tan().ln()]
            }
        };
        out.iter().all(|v| v.is_finite()).then_some(out)
    }
    fn inverse(&self, at: [f64; 2]) -> Option<(f64, f64)> {
        let (lat, lon) = match self {
            Self::TransverseMercator(p) => p.inverse(at),
            Self::Lambert(p) => p.inverse(at)?,
            Self::WebMercator { r } => (2.0 * (at[1] / r).exp().atan() - FRAC_PI_2, at[0] / r),
        };
        (lat.is_finite() && lon.is_finite()).then_some((lat, lon))
    }
}

/// Wrap an angle in radians to -π..π.
fn wrap(angle: f64) -> f64 {
    let a = (angle + PI).rem_euclid(2.0 * PI) - PI;
    if a.is_finite() { a } else { angle }
}

/// Where a projection is centred, its scale there and its false origin (plane units).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Origin {
    lat: f64,
    lon: f64,
    k0: f64,
    east: f64,
    north: f64,
}
impl Origin {
    fn valid(self) -> bool {
        self.k0.is_finite() && self.k0 > 0.0 && self.lat.abs() < FRAC_PI_2 && [self.lon, self.east, self.north].iter().all(|v| v.is_finite())
    }
}

/// Transverse Mercator by Krüger's series in the third flattening, to fourth order (Karney, 2011):
/// well within a millimetre across a UTM zone.
#[derive(Clone, Debug, PartialEq)]
struct TransverseMercator {
    e: f64,
    /// The rectifying radius, `A`.
    radius: f64,
    alpha: [f64; 4],
    beta: [f64; 4],
    delta: [f64; 4],
    k0: f64,
    lon0: f64,
    false_easting: f64,
    false_northing: f64,
    /// The rectifying latitude of the origin, in units of `radius`.
    xi0: f64,
}
impl TransverseMercator {
    fn new(ellipsoid: Ellipsoid, o: Origin) -> Option<Self> {
        if !o.valid() {
            return None;
        }
        let n = ellipsoid.f / (2.0 - ellipsoid.f);
        let (n2, n3, n4) = (n * n, n * n * n, n * n * n * n);
        let mut tm = Self {
            e: ellipsoid.e(),
            radius: ellipsoid.a / (1.0 + n) * (1.0 + n2 / 4.0 + n2 * n2 / 64.0),
            alpha: [
                n / 2.0 - 2.0 * n2 / 3.0 + 5.0 * n3 / 16.0 + 41.0 * n4 / 180.0,
                13.0 * n2 / 48.0 - 3.0 * n3 / 5.0 + 557.0 * n4 / 1440.0,
                61.0 * n3 / 240.0 - 103.0 * n4 / 140.0,
                49561.0 * n4 / 161_280.0,
            ],
            beta: [
                n / 2.0 - 2.0 * n2 / 3.0 + 37.0 * n3 / 96.0 - n4 / 360.0,
                n2 / 48.0 + n3 / 15.0 - 437.0 * n4 / 1440.0,
                17.0 * n3 / 480.0 - 37.0 * n4 / 840.0,
                4397.0 * n4 / 161_280.0,
            ],
            delta: [
                2.0 * n - 2.0 * n2 / 3.0 - 2.0 * n3 + 116.0 * n4 / 45.0,
                7.0 * n2 / 3.0 - 8.0 * n3 / 5.0 - 227.0 * n4 / 45.0,
                56.0 * n3 / 15.0 - 136.0 * n4 / 35.0,
                4279.0 * n4 / 630.0,
            ],
            k0: o.k0,
            lon0: o.lon,
            false_easting: o.east,
            false_northing: o.north,
            xi0: 0.0,
        };
        tm.xi0 = tm.series(o.lat, 0.0)[0];
        tm.xi0.is_finite().then_some(tm)
    }
    /// ξ and η of a point `dlon` from the central meridian.
    fn series(&self, lat: f64, dlon: f64) -> [f64; 2] {
        let s = lat.sin();
        let t = (s.atanh() - self.e * (self.e * s).atanh()).sinh();
        let xi_p = t.atan2(dlon.cos());
        let eta_p = (dlon.sin() / (1.0 + t * t).sqrt()).atanh();
        let (mut xi, mut eta) = (xi_p, eta_p);
        for (j, a) in (1..).zip(self.alpha) {
            let k = 2.0 * f64::from(j);
            xi += a * (k * xi_p).sin() * (k * eta_p).cosh();
            eta += a * (k * xi_p).cos() * (k * eta_p).sinh();
        }
        [xi, eta]
    }
    fn forward(&self, lat: f64, lon: f64) -> [f64; 2] {
        let [xi, eta] = self.series(lat, wrap(lon - self.lon0));
        [self.false_easting + self.k0 * self.radius * eta, self.false_northing + self.k0 * self.radius * (xi - self.xi0)]
    }
    fn inverse(&self, at: [f64; 2]) -> (f64, f64) {
        let xi = (at[1] - self.false_northing) / (self.k0 * self.radius) + self.xi0;
        let eta = (at[0] - self.false_easting) / (self.k0 * self.radius);
        let (mut xi_p, mut eta_p) = (xi, eta);
        for (j, b) in (1..).zip(self.beta) {
            let k = 2.0 * f64::from(j);
            xi_p -= b * (k * xi).sin() * (k * eta).cosh();
            eta_p -= b * (k * xi).cos() * (k * eta).sinh();
        }
        let chi = (xi_p.sin() / eta_p.cosh()).clamp(-1.0, 1.0).asin();
        let mut lat = chi;
        for (j, d) in (1..).zip(self.delta) {
            lat += d * (2.0 * f64::from(j) * chi).sin();
        }
        (lat, wrap(self.lon0 + eta_p.sinh().atan2(xi_p.cos())))
    }
}

/// Lambert Conformal Conic on the ellipsoid (Snyder, *Map Projections: A Working Manual*, §15).
#[derive(Clone, Debug, PartialEq)]
struct Lambert {
    e: f64,
    n: f64,
    /// `a·F·k0`, signed as `n` is.
    af: f64,
    rho0: f64,
    lon0: f64,
    false_easting: f64,
    false_northing: f64,
}
impl Lambert {
    fn t(e: f64, lat: f64) -> f64 {
        let s = lat.sin();
        (FRAC_PI_4 - lat / 2.0).tan() / ((1.0 - e * s) / (1.0 + e * s)).powf(e / 2.0)
    }
    fn m(e: f64, lat: f64) -> f64 {
        let s = lat.sin();
        lat.cos() / (1.0 - e * e * s * s).sqrt()
    }
    /// Two standard parallels `sp1` and `sp2` (scale 1 on them), or one, `sp1` = the origin's
    /// latitude, with `sp2` `None` and scale `o.k0` on it.
    fn new(ellipsoid: Ellipsoid, o: Origin, sp1: f64, sp2: Option<f64>) -> Option<Self> {
        if !o.valid() || sp1.abs() >= FRAC_PI_2 || sp2.is_some_and(|p| p.abs() >= FRAC_PI_2) {
            return None;
        }
        let k0 = if sp2.is_some() { 1.0 } else { o.k0 };
        let e = ellipsoid.e();
        let (m1, t1) = (Self::m(e, sp1), Self::t(e, sp1));
        let n = match sp2 {
            Some(sp2) if (sp1 - sp2).abs() > 1e-10 => (m1.ln() - Self::m(e, sp2).ln()) / (t1.ln() - Self::t(e, sp2).ln()),
            _ => sp1.sin(),
        };
        if !n.is_finite() || n.abs() < 1e-9 {
            return None;
        }
        let af = ellipsoid.a * k0 * m1 / (n * t1.powf(n));
        let rho0 = af * Self::t(e, o.lat).powf(n);
        let out = Self { e, n, af, rho0, lon0: o.lon, false_easting: o.east, false_northing: o.north };
        [af, rho0].iter().all(|v| v.is_finite()).then_some(out)
    }
    fn forward(&self, lat: f64, lon: f64) -> [f64; 2] {
        let rho = self.af * Self::t(self.e, lat).powf(self.n);
        let theta = self.n * wrap(lon - self.lon0);
        [self.false_easting + rho * theta.sin(), self.false_northing + self.rho0 - rho * theta.cos()]
    }
    fn inverse(&self, at: [f64; 2]) -> Option<(f64, f64)> {
        let sign = self.n.signum();
        let dx = at[0] - self.false_easting;
        let dy = self.rho0 - (at[1] - self.false_northing);
        let rho = sign * dx.hypot(dy);
        let theta = (sign * dx).atan2(sign * dy);
        let t = (rho / self.af).powf(1.0 / self.n);
        let mut lat = FRAC_PI_2 - 2.0 * t.atan();
        // Converges to well below a micro-arc-second in a handful of steps.
        for _ in 0..16 {
            let s = lat.sin();
            let next = FRAC_PI_2 - 2.0 * (t * ((1.0 - self.e * s) / (1.0 + self.e * s)).powf(self.e / 2.0)).atan();
            let done = (next - lat).abs() < 1e-13;
            lat = next;
            if done {
                break;
            }
        }
        Some((lat, wrap(theta / self.n + self.lon0)))
    }
}

/// A UTM zone (1 to 60) of `ellipsoid`, north or south.
fn utm(ellipsoid: Ellipsoid, zone: i64, south: bool) -> Option<Projection> {
    if !(1..=60).contains(&zone) {
        return None;
    }
    let lon0 = (zone as f64 * 6.0 - 183.0).to_radians();
    let false_northing = if south { 10_000_000.0 } else { 0.0 };
    TransverseMercator::new(ellipsoid, Origin { lat: 0.0, lon: lon0, k0: 0.9996, east: 500_000.0, north: false_northing })
        .map(Projection::TransverseMercator)
}

/// A coordinate system PdfCraft knows: geographic, or projected with the projection if it is one
/// PdfCraft implements.
#[derive(Debug)]
struct Known {
    name: String,
    projected: bool,
    projection: Option<Projection>,
}

/// The common EPSG codes of map PDFs: geographic systems, UTM zones of the usual datums, and Web
/// Mercator. Anything else needs WKT.
fn epsg(code: i64) -> Option<Known> {
    let geographic = |name: &str| Some(Known { name: name.into(), projected: false, projection: None });
    let projected = |name: String, projection: Option<Projection>| Some(Known { name, projected: true, projection });
    match code {
        4326 => geographic("WGS 84"),
        4269 => geographic("NAD83"),
        4267 => geographic("NAD27"),
        4258 => geographic("ETRS89"),
        4283 => geographic("GDA94"),
        7844 => geographic("GDA2020"),
        4617 => geographic("NAD83(CSRS)"),
        4230 => geographic("ED50"),
        4277 => geographic("OSGB 1936"),
        32601..=32660 => projected(format!("WGS 84 / UTM zone {}N", code - 32600), utm(WGS84, code - 32600, false)),
        32701..=32760 => projected(format!("WGS 84 / UTM zone {}S", code - 32700), utm(WGS84, code - 32700, true)),
        26901..=26923 => projected(format!("NAD83 / UTM zone {}N", code - 26900), utm(GRS80, code - 26900, false)),
        26701..=26722 => projected(format!("NAD27 / UTM zone {}N", code - 26700), utm(CLARKE_1866, code - 26700, false)),
        25828..=25838 => projected(format!("ETRS89 / UTM zone {}N", code - 25800), utm(GRS80, code - 25800, false)),
        28348..=28358 => projected(format!("GDA94 / MGA zone {}", code - 28300), utm(GRS80, code - 28300, true)),
        3857 | 900_913 | 3785 | 102_100 | 102_113 => projected("WGS 84 / Pseudo-Mercator".into(), Some(Projection::WebMercator { r: WGS84.a })),
        _ => None,
    }
}

/// One WKT node: `NAME[ "text", 1.5, CHILD[...], KEYWORD ]` (or with parentheses).
#[derive(Debug, Default)]
struct Node {
    name: String,
    strings: Vec<String>,
    numbers: Vec<f64>,
    children: Vec<Node>,
}
impl Node {
    /// The first node, depth first (this one included), with one of `names`.
    fn find(&self, names: &[&str]) -> Option<&Node> {
        if names.contains(&self.name.as_str()) {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(names))
    }
    fn all<'a>(&'a self, name: &str, out: &mut Vec<&'a Node>) {
        if self.name == name {
            out.push(self);
        }
        for c in &self.children {
            c.all(name, out);
        }
    }
    fn label(&self) -> Option<&str> {
        self.strings.first().map(String::as_str)
    }
}

/// A bounded parser for WKT (OGC 01-009 and ISO 19162): just the structure, which every version
/// shares.
struct Wkt<'a> {
    s: &'a [u8],
    at: usize,
    nodes: usize,
}
impl Wkt<'_> {
    fn parse(text: &str) -> std::result::Result<Node, String> {
        if text.len() > MAX_WKT {
            return Err("its WKT is too long".into());
        }
        let mut p = Wkt { s: text.as_bytes(), at: 0, nodes: 0 };
        let (root, _) = p.node(0)?;
        p.space();
        if p.at < p.s.len() {
            return Err("its WKT has text after the coordinate system".into());
        }
        Ok(root)
    }
    fn peek(&self) -> Option<u8> {
        self.s.get(self.at).copied()
    }
    fn space(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.at += 1;
        }
    }
    fn word(&mut self) -> String {
        let start = self.at;
        while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_') {
            self.at += 1;
        }
        self.s.get(start..self.at).map(|w| String::from_utf8_lossy(w).to_ascii_uppercase()).unwrap_or_default()
    }
    /// A node, and whether it had brackets (a bare keyword such as `EAST` has none).
    fn node(&mut self, depth: usize) -> std::result::Result<(Node, bool), String> {
        let bad = || "its WKT can't be read".to_string();
        if depth >= MAX_WKT_DEPTH {
            return Err("its WKT is nested too deeply".into());
        }
        self.nodes += 1;
        if self.nodes > MAX_WKT_NODES {
            return Err("its WKT is too long".into());
        }
        self.space();
        let name = self.word();
        if name.is_empty() {
            return Err(bad());
        }
        self.space();
        let close = match self.peek() {
            Some(b'[') => b']',
            Some(b'(') => b')',
            _ => return Ok((Node { name, ..Node::default() }, false)),
        };
        self.at += 1;
        let mut node = Node { name, ..Node::default() };
        let mut first = true;
        loop {
            self.space();
            match self.peek() {
                Some(c) if c == close && first => {
                    self.at += 1;
                    return Ok((node, true));
                }
                Some(b'"') => node.strings.push(self.string()?),
                Some(c) if c == b'-' || c == b'+' || c == b'.' || c.is_ascii_digit() => node.numbers.push(self.number()?),
                Some(c) if c.is_ascii_alphabetic() || c == b'_' => {
                    // A bare keyword (AXIS["x", EAST]) is kept as text.
                    match self.node(depth + 1)? {
                        (child, true) => node.children.push(child),
                        (child, false) => node.strings.push(child.name),
                    }
                }
                _ => return Err(bad()),
            }
            first = false;
            self.space();
            match self.peek() {
                Some(b',') => {
                    self.at += 1;
                    // An item must follow a comma.
                    self.space();
                    if self.peek() == Some(close) {
                        return Err(bad());
                    }
                }
                Some(c) if c == close => {
                    self.at += 1;
                    return Ok((node, true));
                }
                _ => return Err(bad()),
            }
        }
    }
    fn string(&mut self) -> std::result::Result<String, String> {
        self.at += 1;
        let mut out = Vec::new();
        loop {
            match self.peek() {
                None => return Err("its WKT has an unterminated string".into()),
                Some(b'"') if self.s.get(self.at + 1) == Some(&b'"') => {
                    out.push(b'"');
                    self.at += 2;
                }
                Some(b'"') => {
                    self.at += 1;
                    return Ok(String::from_utf8_lossy(&out).into_owned());
                }
                Some(c) => {
                    out.push(c);
                    self.at += 1;
                }
            }
        }
    }
    fn number(&mut self) -> std::result::Result<f64, String> {
        let start = self.at;
        while self.peek().is_some_and(|c| c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.' | b'e' | b'E')) {
            self.at += 1;
        }
        let text = self.s.get(start..self.at).and_then(|w| std::str::from_utf8(w).ok()).unwrap_or("");
        text.parse::<f64>().ok().filter(|v| v.is_finite()).ok_or_else(|| "its WKT has an invalid number".to_string())
    }
}

/// Lowercase letters and digits only: `Latitude of natural origin` and `latitude_of_origin` alike.
fn squash(s: &str) -> String {
    s.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

/// A coordinate system from its WKT: geographic, or projected with the projection when it is one
/// PdfCraft implements (`Err` naming it when it isn't).
fn from_wkt(text: &str) -> std::result::Result<(Known, std::result::Result<(), String>), String> {
    let mut root = Wkt::parse(text)?;
    // A compound system (horizontal + vertical) is located by its horizontal part.
    if matches!(root.name.as_str(), "COMPD_CS" | "COMPOUNDCRS") {
        let horizontal = root.children.into_iter().find(|c| {
            matches!(c.name.as_str(), "PROJCS" | "PROJCRS" | "PROJECTEDCRS" | "GEOGCS" | "GEOGCRS" | "GEODCRS" | "GEOGRAPHICCRS" | "GEODETICCRS")
        });
        root = horizontal.ok_or("its compound coordinate system has no horizontal part")?;
    }
    let name = root.label().unwrap_or("").chars().filter(|c| !c.is_control()).take(128).collect::<String>();
    let ellipsoid = root.find(&["SPHEROID", "ELLIPSOID"]).and_then(|n| match n.numbers.as_slice() {
        [a, inverse_flattening, ..] => Ellipsoid::new(*a, *inverse_flattening),
        _ => None,
    });
    match root.name.as_str() {
        "GEOGCS" | "GEOGCRS" | "GEODCRS" | "GEOGRAPHICCRS" | "GEODETICCRS" => Ok((Known { name, projected: false, projection: None }, Ok(()))),
        "PROJCS" | "PROJCRS" | "PROJECTEDCRS" => {
            let projection = projection(&root, ellipsoid.unwrap_or(WGS84));
            let (projection, why) = match projection {
                Ok(p) => (Some(p), Ok(())),
                Err(e) => (None, Err(e)),
            };
            Ok((Known { name, projected: true, projection }, why))
        }
        other => Err(format!("its WKT describes a {other}, not a horizontal coordinate system")),
    }
}

/// The projection of a projected coordinate system's WKT.
fn projection(root: &Node, ellipsoid: Ellipsoid) -> std::result::Result<Projection, String> {
    let method = root.find(&["PROJECTION", "METHOD"]).and_then(Node::label).unwrap_or("");
    let mut params = Vec::new();
    root.all("PARAMETER", &mut params);
    // A parameter's value by any of its names; angles in radians.
    let value = |names: &[&str]| {
        params.iter().find(|p| p.label().is_some_and(|l| names.contains(&squash(l).as_str()))).and_then(|p| p.numbers.first().copied())
    };
    // A parameter without a unit of its own is in the base geographic system's angular unit
    // (OGC 01-009 §7; ISO 19162 §9.3), in radians per unit; degrees when that names none.
    let radians_per = |n: &Node| {
        n.children
            .iter()
            .find(|c| matches!(c.name.as_str(), "ANGLEUNIT" | "UNIT"))
            .and_then(|u| u.numbers.first().copied())
            .filter(|r| r.is_finite() && *r > 0.0)
    };
    let base = root.find(&["GEOGCS", "BASEGEOGCRS", "BASEGEODCRS"]).and_then(radians_per).unwrap_or(1f64.to_radians());
    let angle = |names: &[&str]| -> Option<f64> {
        let p = params.iter().find(|p| p.label().is_some_and(|l| names.contains(&squash(l).as_str())))?;
        let v = p.numbers.first().copied()?;
        Some(v * radians_per(p).unwrap_or(base))
    };
    let lat = angle(&["latitudeoforigin", "latitudeofnaturalorigin", "latitudeoffalseorigin", "latitudeofcenter", "latitudeofprojectioncentre"])
        .unwrap_or(0.0);
    let lon =
        angle(&["centralmeridian", "longitudeofnaturalorigin", "longitudeoffalseorigin", "longitudeoforigin", "longitudeofcenter"]).unwrap_or(0.0);
    let k0 = value(&["scalefactor", "scalefactoratnaturalorigin"]).unwrap_or(1.0);
    let east = value(&["falseeasting", "eastingatfalseorigin"]).unwrap_or(0.0);
    let north = value(&["falsenorthing", "northingatfalseorigin"]).unwrap_or(0.0);
    let origin = Origin { lat, lon, k0, east, north };
    let sp1 = angle(&["standardparallel1", "latitudeof1ststandardparallel"]);
    let sp2 = angle(&["standardparallel2", "latitudeof2ndstandardparallel"]);
    let kind = squash(method);
    let unsupported = || format!("the {} projection is not supported yet", if method.is_empty() { "unnamed" } else { method });
    let out = if (kind.contains("transversemercator") && !kind.contains("south")) || kind.contains("gausskruger") {
        TransverseMercator::new(ellipsoid, origin).map(Projection::TransverseMercator)
    } else if kind.contains("lambertconformalconic") || kind.contains("lambertconicconformal") {
        let one = kind.contains("1sp") || (!kind.contains("2sp") && sp1.is_none());
        if one {
            Lambert::new(ellipsoid, origin, lat, None)
        } else {
            Lambert::new(ellipsoid, origin, sp1.unwrap_or(lat), Some(sp2.or(sp1).unwrap_or(lat)))
        }
        .map(Projection::Lambert)
    } else if kind.contains("pseudomercator") || kind.contains("mercatorauxiliarysphere") {
        Some(Projection::WebMercator { r: ellipsoid.a })
    } else {
        return Err(unsupported());
    };
    out.ok_or_else(|| format!("the {method} projection has invalid parameters"))
}

/// A viewport's registration: a least-squares affine fit from page user space to the model plane
/// (projected coordinates, or longitude and latitude in degrees).
#[derive(Clone, Debug, PartialEq)]
pub struct Registration {
    pub crs: Crs,
    pub method: Method,
    /// Why the projection isn't implemented, when `method` is [`Method::Approximate`].
    pub note: Option<String>,
    projection: Option<Projection>,
    /// The registered area in user space.
    bounds: Vec<Point>,
    /// User space to the model plane, `[a b c d e f]`: `x' = a·x + b·y + c`, `y' = d·x + e·y + f`.
    fit: [f64; 6],
    /// The model plane to user space.
    unfit: [f64; 6],
}

/// `bbox` as `[x0 y0 x1 y1]` with x0 < x1 and y0 < y1.
fn normalized(b: [f64; 4]) -> [f64; 4] {
    [b[0].min(b[2]), b[1].min(b[3]), b[0].max(b[2]), b[1].max(b[3])]
}

/// The numbers of an array of at most `max` numbers, or what is wrong with it.
fn numbers(doc: &Document, o: &Object, max: usize) -> std::result::Result<Vec<f64>, String> {
    let o = doc.resolve(o);
    let a = o.as_array().ok_or("is not an array")?;
    if a.len() > max {
        return Err("has too many numbers".into());
    }
    a.iter().map(|v| doc.resolve(v).as_f64().filter(|v| v.is_finite()).ok_or_else(|| "holds something other than a number".to_string())).collect()
}

/// Pairs of an array of numbers: at least three, at most [`MAX_GEO_POINTS`].
fn pairs(doc: &Document, d: &Dict, key: &str) -> Result<Option<Vec<Point>>> {
    let Some(o) = d.get(key.as_bytes()) else { return Ok(None) };
    let v = numbers(doc, o, MAX_GEO_POINTS * 2).map_err(|e| invalid(&format!("its /{key} {e}")))?;
    if v.len() % 2 != 0 || v.len() < 6 {
        return Err(invalid(&format!("its /{key} needs at least three pairs of numbers")));
    }
    Ok(Some(v.as_chunks::<2>().0.to_vec()))
}

/// Least squares `q ≈ a·x + b·y + c` over `points`, centred for precision. `None` when the points
/// are (nearly) on one line.
fn fit_axis(points: &[(Point, f64)]) -> Option<[f64; 3]> {
    let n = points.len() as f64;
    if points.len() < 3 {
        return None;
    }
    let (mx, my, mq) = points.iter().fold((0.0, 0.0, 0.0), |(x, y, q), (p, v)| (x + p[0] / n, y + p[1] / n, q + v / n));
    let (mut sxx, mut sxy, mut syy, mut sxq, mut syq) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (p, v) in points {
        let (x, y, q) = (p[0] - mx, p[1] - my, v - mq);
        sxx += x * x;
        sxy += x * y;
        syy += y * y;
        sxq += x * q;
        syq += y * q;
    }
    let det = sxx * syy - sxy * sxy;
    if !det.is_finite() || det <= 1e-9 * sxx * syy || det == 0.0 {
        return None;
    }
    let a = (sxq * syy - syq * sxy) / det;
    let b = (syq * sxx - sxq * sxy) / det;
    let out = [a, b, mq - a * mx - b * my];
    out.iter().all(|v| v.is_finite()).then_some(out)
}

fn apply(m: [f64; 6], p: Point) -> Point {
    [m[0] * p[0] + m[1] * p[1] + m[2], m[3] * p[0] + m[4] * p[1] + m[5]]
}

fn invert(m: [f64; 6]) -> Option<[f64; 6]> {
    let det = m[0] * m[4] - m[1] * m[3];
    if !det.is_finite() || det == 0.0 {
        return None;
    }
    let (a, b, d, e) = (m[4] / det, -m[1] / det, -m[3] / det, m[0] / det);
    let out = [a, b, -(a * m[2] + b * m[5]), d, e, -(d * m[2] + e * m[5])];
    out.iter().all(|v| v.is_finite()).then_some(out)
}

/// Whether `p` is on the segment `a`–`b`, within a billionth of its length.
fn on_segment(a: Point, b: Point, p: Point) -> bool {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length = dx.hypot(dy);
    if length == 0.0 {
        return (p[0] - a[0]).hypot(p[1] - a[1]) <= 1e-9;
    }
    let t = ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / (length * length);
    let (qx, qy) = (a[0] + t * dx, a[1] + t * dy);
    (-1e-12..=1.0 + 1e-12).contains(&t) && (p[0] - qx).hypot(p[1] - qy) <= 1e-9 * length.max(1.0)
}

/// Even-odd point in polygon, its edges included.
fn inside(polygon: &[Point], p: Point) -> bool {
    let mut odd = false;
    for (a, b) in polygon.iter().zip(polygon.iter().cycle().skip(1)).take(polygon.len()) {
        if on_segment(*a, *b, p) {
            return true;
        }
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if p[0] < x {
                odd = !odd;
            }
        }
    }
    odd
}

impl Registration {
    /// Read a geospatial measure dictionary of a viewport with `bbox` (user space).
    pub(crate) fn read(doc: &Document, bbox: [f64; 4], measure: &Object, budget: &mut usize) -> Result<Self> {
        let o = doc.resolve(measure);
        let d = o.as_dict().ok_or_else(|| invalid("its measure dictionary is invalid"))?;
        if d.get(b"Subtype").map(|o| doc.resolve(o)).as_deref().and_then(Object::as_name) != Some(b"GEO".as_slice()) {
            return Err(invalid("it is not a geospatial measure dictionary"));
        }
        let [x0, y0, x1, y1] = normalized(bbox);
        if !(x1 - x0 > 1e-6 && y1 - y0 > 1e-6) {
            return Err(invalid("its viewport has no area"));
        }
        let geo = pairs(doc, d, "GPTS")?.ok_or_else(|| invalid("it has no /GPTS (latitudes and longitudes)"))?;
        let square = [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]].to_vec();
        let bounds = pairs(doc, d, "Bounds")?.unwrap_or(square);
        let local = pairs(doc, d, "LPTS")?.unwrap_or_else(|| bounds.clone());
        if local.len() != geo.len() {
            return Err(invalid("its /LPTS and /GPTS have different numbers of points"));
        }
        if geo.iter().any(|g| g[0].abs() > 90.0 || g[1].abs() > 360.0) {
            return Err(invalid("its /GPTS holds a latitude or longitude out of range"));
        }
        if local.iter().chain(&bounds).flatten().any(|v| v.abs() > 1e6) {
            return Err(invalid("its unit-square points are out of range"));
        }
        let to_user = |u: &Point| [x0 + u[0] * (x1 - x0), y0 + u[1] * (y1 - y0)];
        let (crs, projection, note) = crs(doc, d, budget)?;
        let method = match (&projection, crs.projected) {
            (Some(_), _) => Method::Projected,
            (None, false) => Method::Geographic,
            (None, true) => Method::Approximate,
        };
        // Longitudes are fitted as written (242 stays 242, a world map keeps -180 and 180), except
        // a map written -180..180 across the antimeridian (170 and -170), whose longitudes are
        // fitted from 0 to 360 so they stay continuous: for latitude/longitude fits and for Web
        // Mercator alike (the other projections wrap about their own central meridian).
        let span = |lons: &mut dyn Iterator<Item = f64>| lons.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), l| (lo.min(l), hi.max(l)));
        let (lo, hi) = span(&mut geo.iter().map(|g| g[1]));
        let (elo, ehi) = span(&mut geo.iter().map(|g| g[1].rem_euclid(360.0)));
        let east = lo >= -180.0 && hi <= 180.0 && hi - lo > 180.0 && ehi - elo > 0.0 && ehi - elo < hi - lo;
        let mut model = Vec::with_capacity(geo.len());
        for g in &geo {
            let lon = if east { g[1].rem_euclid(360.0) } else { g[1] };
            let m = match &projection {
                Some(p) => p.forward(g[0].to_radians(), lon.to_radians()).ok_or_else(|| invalid("a control point lies outside its projection"))?,
                None => [lon, g[0]],
            };
            model.push(m);
        }
        let user: Vec<Point> = local.iter().map(to_user).collect();
        check_points(&user).map_err(|_| invalid("its control points are out of range"))?;
        let axis = |i: usize| -> Option<[f64; 3]> {
            let pts: Vec<(Point, f64)> = user.iter().zip(&model).filter_map(|(u, m)| m.get(i).map(|v| (*u, *v))).collect();
            fit_axis(&pts)
        };
        let (Some([a, b, c]), Some([dd, e, f])) = (axis(0), axis(1)) else {
            return Err(invalid("its control points lie on one line"));
        };
        let fit = [a, b, c, dd, e, f];
        let unfit = invert(fit).ok_or_else(|| invalid("its control points lie on one line"))?;
        Ok(Self { crs, method, note, projection, bounds: bounds.iter().map(to_user).collect(), fit, unfit })
    }

    /// The latitude and longitude (degrees) of a user-space point, if it is in the registered area.
    pub fn locate(&self, at: Point) -> Option<(f64, f64)> {
        if !inside(&self.bounds, at) {
            return None;
        }
        let m = apply(self.fit, at);
        let (lat, lon) = match &self.projection {
            Some(p) => {
                let (lat, lon) = p.inverse(m)?;
                (lat.to_degrees(), lon.to_degrees())
            }
            None => (m[1], m[0]),
        };
        if !lat.is_finite() || !lon.is_finite() || lat.abs() > 90.0 + 1e-9 {
            return None;
        }
        let lon = (lon + 180.0).rem_euclid(360.0) - 180.0;
        Some((lat.clamp(-90.0, 90.0), if lon.is_finite() { lon } else { 0.0 }))
    }

    /// The user-space point of a latitude and longitude (degrees), if it is in the registered area.
    pub fn find(&self, lat: f64, lon: f64) -> Option<Point> {
        if !lat.is_finite() || !lon.is_finite() || lat.abs() > 90.0 {
            return None;
        }
        // The longitude as -180..180, then a turn either way: the first that lands in the registered
        // area (control longitudes may be written 0..360, or continue across the antimeridian).
        let lon = (lon + 180.0).rem_euclid(360.0) - 180.0;
        [lon, lon + 360.0, lon - 360.0].into_iter().find_map(|lon| {
            let m = match &self.projection {
                Some(p) => p.forward(lat.to_radians(), lon.to_radians())?,
                None => [lon, lat],
            };
            let p = apply(self.unfit, m);
            (p.iter().all(|v| v.is_finite()) && inside(&self.bounds, p)).then_some(p)
        })
    }
}

/// The coordinate system of a geospatial measure dictionary, its projection when PdfCraft
/// implements it, and a note when it is approximated.
fn crs(doc: &Document, d: &Dict, budget: &mut usize) -> Result<(Crs, Option<Projection>, Option<String>)> {
    let g = d.get(b"GCS").map(|o| doc.resolve(o)).ok_or_else(|| invalid("it has no /GCS (coordinate system)"))?;
    let g = g.as_dict().ok_or_else(|| invalid("its /GCS is not a dictionary"))?;
    let kind = g.get(b"Type").map(|o| doc.resolve(o)).and_then(|o| o.as_name().map(<[u8]>::to_vec));
    let code = g.get(b"EPSG").and_then(|o| doc.resolve(o).as_int());
    let wkt = match g.get(b"WKT").map(|o| doc.resolve(o)) {
        Some(o) => match o.as_string() {
            Some(s) if s.bytes.len() > MAX_WKT => return Err(invalid("its coordinate system WKT is too long")),
            Some(s) => {
                *budget = budget.checked_sub(s.bytes.len()).ok_or_else(|| invalid("the page holds too much coordinate system text"))?;
                Some(s.to_text())
            }
            None => None,
        },
        None => None,
    };
    let from_text = match wkt.as_deref().filter(|w| !w.trim().is_empty()) {
        Some(w) => Some(from_wkt(w).map_err(|e| invalid(&format!("its coordinate system can't be read: {e}")))?),
        None => None,
    };
    let known = code.and_then(epsg);
    let named = |k: &Known| if k.name.is_empty() { code.map_or_else(|| "Unnamed".into(), |c| format!("EPSG:{c}")) } else { k.name.clone() };
    Ok(match (from_text, known) {
        // The WKT's projection, else the EPSG code's when the WKT's isn't implemented.
        (Some((k, Ok(()))), _) => (Crs { name: named(&k), epsg: code, projected: k.projected }, k.projection, None),
        (Some((k, Err(_))), Some(e)) if e.projected && e.projection.is_some() => {
            (Crs { name: named(&k), epsg: code, projected: true }, e.projection, None)
        }
        (Some((k, Err(why))), _) => (Crs { name: named(&k), epsg: code, projected: true }, None, Some(why)),
        (None, Some(k)) => (Crs { name: k.name.clone(), epsg: code, projected: k.projected }, k.projection, None),
        (None, None) => {
            let geographic = match kind.as_deref() {
                Some(b"GEOGCS") => true,
                Some(b"PROJCS") => false,
                _ => code.is_some_and(|c| (4000..5000).contains(&c)),
            };
            let name = code.map_or_else(|| if geographic { "Geographic".into() } else { "Projected".into() }, |c| format!("EPSG:{c}"));
            let note = (!geographic).then(|| "the projection of this coordinate system is not supported yet".to_string());
            (Crs { name, epsg: code, projected: !geographic }, None, note)
        }
    })
}

/// What a page viewport holds.
#[derive(Clone, Debug, PartialEq)]
pub enum ViewportKind {
    Geo(Box<Registration>),
    /// A geospatial measure dictionary that can't be used, and why.
    Unsupported(String),
    /// Another measure dictionary (a drawing scale).
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GeoViewport {
    pub name: String,
    /// User space, normalized.
    pub bbox: [f64; 4],
    pub kind: ViewportKind,
}

/// The measured viewports of a page in drawing order, read once for many lookups.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GeoPage {
    pub viewports: Vec<GeoViewport>,
}

impl GeoPage {
    /// The viewports of page `page_index` that carry a measure dictionary. The last
    /// [`MAX_VIEWPORTS`] are read, as for [`super::scale_at`].
    pub fn read(doc: &Document, page_index: usize) -> Result<Self> {
        let p = page(doc, page_index)?;
        let Some(vp) = p.dict.get(b"VP") else { return Ok(Self::default()) };
        let vp = doc.resolve(vp);
        let list = vp.as_array().ok_or_else(|| invalid("invalid page viewports"))?;
        let mut budget = MAX_PAGE_WKT;
        let mut viewports = Vec::new();
        for v in list.iter().skip(list.len().saturating_sub(MAX_VIEWPORTS)) {
            let v = doc.resolve(v);
            let Some(d) = v.as_dict() else { continue };
            let Some(measure) = d.get(b"Measure") else { continue };
            let Some(bbox) = d.get(b"BBox").and_then(|b| numbers(doc, b, 4).ok()).and_then(|b| <[f64; 4]>::try_from(b).ok()) else { continue };
            let geo = doc.resolve(measure).as_dict().and_then(|m| m.get(b"Subtype")).map(|s| doc.resolve(s)).as_deref().and_then(Object::as_name)
                == Some(b"GEO".as_slice());
            let kind = if geo {
                match Registration::read(doc, bbox, measure, &mut budget) {
                    Ok(r) => ViewportKind::Geo(Box::new(r)),
                    Err(e) => ViewportKind::Unsupported(e.to_string()),
                }
            } else {
                ViewportKind::Other
            };
            viewports.push(GeoViewport {
                name: text(doc, d, b"Name").chars().filter(|c| !c.is_control()).take(256).collect(),
                bbox: normalized(bbox),
                kind,
            });
        }
        Ok(Self { viewports })
    }

    /// Whether any viewport is geospatially registered.
    pub fn georeferenced(&self) -> bool {
        self.viewports.iter().any(|v| matches!(v.kind, ViewportKind::Geo(_)))
    }

    /// Where a user-space point is on the Earth. The last viewport containing it decides (as for
    /// drawing scales): none when it isn't a usable geospatial one, or the point is outside its
    /// registered area.
    pub fn locate(&self, at: Point) -> Option<Location> {
        if !at.iter().all(|v| v.is_finite()) {
            return None;
        }
        let v = self.viewports.get(self.top(at)?)?;
        let ViewportKind::Geo(r) = &v.kind else { return None };
        let (lat, lon) = r.locate(at)?;
        Some(Location {
            lat,
            lon,
            decimal: format!("{lat:.6}, {lon:.6}"),
            dms: dms(lat, lon),
            crs: r.crs.clone(),
            method: r.method,
            viewport: v.name.clone(),
        })
    }

    /// The user-space point of a latitude and longitude: the topmost registered viewport that
    /// holds it, if any, and only where that viewport is the one the page shows (not covered by a
    /// later viewport).
    pub fn find(&self, lat: f64, lon: f64) -> Option<Point> {
        self.viewports.iter().enumerate().rev().find_map(|(i, v)| match &v.kind {
            ViewportKind::Geo(r) => r.find(lat, lon).filter(|p| self.top(*p) == Some(i)),
            _ => None,
        })
    }

    /// The index of the last viewport containing a user-space point: the one that decides there.
    fn top(&self, at: Point) -> Option<usize> {
        self.viewports.iter().rposition(|v| at[0] >= v.bbox[0] && at[0] <= v.bbox[2] && at[1] >= v.bbox[1] && at[1] <= v.bbox[3])
    }
}

/// `34°08'12.3"N 118°07'30.1"W`, rounded to a tenth of a second.
pub fn dms(lat: f64, lon: f64) -> String {
    let part = |v: f64, positive: char, negative: char| {
        // At most 180° · 36 000 tenths of a second: well within u64.
        let tenths = (v.abs().min(360.0) * 36_000.0).round() as u64;
        let (d, m, s) = (tenths / 36_000, tenths % 36_000 / 600, tenths % 600);
        format!("{d}°{m:02}'{:02}.{}\"{}", s / 10, s % 10, if v < 0.0 { negative } else { positive })
    };
    format!("{} {}", part(lat, 'N', 'S'), part(lon, 'E', 'W'))
}

#[cfg(test)]
mod tests;
