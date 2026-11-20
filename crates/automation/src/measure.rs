//! Measurement tools share the engine's PDF user-space geometry and scale semantics.
use crate::{Args, Automation, Result, ToolError, failed};
use pdfcraft_engine::{
    Edit, Style,
    measure::{self, Kind, NewMeasurement, PageUnit, Scale},
};
use serde_json::{Map, Value, json};

impl Automation {
    fn measurement_points(&self, a: &Args) -> Result<(usize, Vec<measure::Point>)> {
        let page = self.page(a)?;
        let arr = a.get("points").and_then(Value::as_array).ok_or_else(|| ToolError::InvalidArgs("points must be arrays of [x,y]".into()))?;
        if arr.len() > measure::MAX_POINTS {
            return Err(ToolError::InvalidArgs("at most 2048 measurement vertices are allowed".into()));
        }
        let mut points = Vec::new();
        for p in arr {
            let pair = p.as_array().filter(|p| p.len() == 2).ok_or_else(|| failed("each measurement point needs two coordinates"))?;
            let x = pair.first().and_then(Value::as_f64).ok_or_else(|| failed("invalid x coordinate"))?;
            let y = pair.get(1).and_then(Value::as_f64).ok_or_else(|| failed("invalid y coordinate"))?;
            if !x.is_finite() || !y.is_finite() || x.abs() > 1e9 || y.abs() > 1e9 {
                return Err(failed("measurement coordinates must be finite and within 1e9 points"));
            }
            points.push(self.doc(a)?.measurement_to_user(page, [x, y]).map_err(failed)?);
        }
        Ok((page, points))
    }
    pub(crate) fn measurement_add(&mut self, a: &Args, kind: Kind) -> Result<Value> {
        let (page, points) = self.measurement_points(a)?;
        let at = points.first().copied().ok_or_else(|| failed("measurement needs points"))?;
        let scale = self.doc(a)?.measurement_scale(page, at).map_err(failed)?;
        let edit = Edit::AddMeasurement(NewMeasurement {
            page,
            kind,
            points,
            scale,
            style: Style { color: [0.0, 0.47, 0.84], ..Style::default() },
            label: a.opt_str("label")?.unwrap_or("").into(),
            author: a.opt_str("author")?.unwrap_or(super::comments::DEFAULT_AUTHOR).into(),
        });
        self.apply(a, edit)
    }
    pub(crate) fn measurement_info(&self, a: &Args) -> Result<Value> {
        let (page, points) = self.measurement_points(a)?;
        let kind = Kind::from_name(a.opt_str("type")?.unwrap_or("distance")).ok_or_else(|| failed("type must be distance, perimeter or area"))?;
        let at = points.first().copied().ok_or_else(|| failed("measurement needs points"))?;
        let scale = self.doc(a)?.measurement_scale(page, at).map_err(failed)?;
        let reading = measure::reading(kind, &points, &scale).map_err(failed)?;
        Ok(json!({"reading":reading,"scale":scale}))
    }
    pub(crate) fn measurement_list(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let all = doc.measurements().map_err(failed)?;
        let only = a.opt_int("page")?.map(|n| usize::try_from(n.max(1).saturating_sub(1)).unwrap_or(usize::MAX));
        let measurements: Vec<Value> = all
            .measurements
            .iter()
            .filter(|m| only.is_none_or(|p| m.page == p))
            .map(|m| -> Result<Value> {
                let mut v = json!(m);
                v["page"] = json!(m.page + 1);
                v["index"] = json!(m.index + 1);
                let points = m.points.iter().map(|p| doc.measurement_to_view(m.page, *p).map_err(failed)).collect::<Result<Vec<_>>>()?;
                v["points"] = json!(points);
                Ok(v)
            })
            .collect::<Result<_>>()?;
        let unsupported: Vec<Value> = all
            .unsupported
            .iter()
            .filter(|u| only.is_none_or(|p| u.page == p))
            .map(|u| json!({"page":u.page.saturating_add(1),"index":u.index.saturating_add(1),"reason":u.reason}))
            .collect();
        Ok(json!({"count":measurements.len(),"measurements":measurements,"unsupported":unsupported,"truncated":all.truncated}))
    }
    pub(crate) fn measurement_scale(&mut self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let info = self.doc(a)?.info.pages.get(page).ok_or_else(|| failed("no such page"))?.clone();
        let precision = u8::try_from(a.opt_int("precision")?.unwrap_or(2)).map_err(|_| failed("precision must be between 0 and 6"))?;
        let unit = a.opt_str("unit")?.unwrap_or("in");
        if ["points", "units_per_point", "page_distance"].iter().filter(|k| a.get(k).is_some()).count() > 1 {
            return Err(failed("pass one of calibration points, units_per_point or page_distance"));
        }
        if a.get("page_unit").is_some() && a.get("page_distance").is_none() {
            return Err(failed("page_unit is the unit of page_distance; pass page_distance too"));
        }
        let scale = if a.get("points").is_some() {
            let (_, points) = self.measurement_points(a)?;
            let [from, to] = points.as_slice() else {
                return Err(failed("scale calibration needs exactly two points"));
            };
            let length = a.opt_num("distance")?.ok_or_else(|| failed("calibration needs a real-world distance"))?;
            Scale::calibrate(*from, *to, length, unit, precision).map_err(failed)?
        } else if let Some(page_distance) = a.opt_num("page_distance")? {
            // A ratio, "page_distance page_unit on the page = distance unit"; the page unit
            // defaults to the real-world one, as "1:50" means 1 mm = 50 mm.
            let page_unit = match a.opt_str("page_unit")? {
                Some(u) => PageUnit::from_label(u).ok_or_else(|| failed("page_unit must be pt, in, mm or cm"))?,
                None => PageUnit::from_label(unit).ok_or_else(|| failed("pass page_unit (pt, in, mm or cm) when unit is not one of them"))?,
            };
            let length = a.opt_num("distance")?.ok_or_else(|| failed("a scale ratio needs the real-world distance"))?;
            let from = self.doc(a)?.measurement_to_user(page, [0.0, 0.0]).map_err(failed)?;
            let to = self.doc(a)?.measurement_to_user(page, [1.0, 0.0]).map_err(failed)?;
            Scale::ratio(from, to, page_distance, page_unit, length, unit, precision).map_err(failed)?
        } else if let Some(factor) = a.opt_num("units_per_point")? {
            {
                let from = self.doc(a)?.measurement_to_user(page, [0.0, 0.0]).map_err(failed)?;
                let to = self.doc(a)?.measurement_to_user(page, [1.0, 0.0]).map_err(failed)?;
                Scale::calibrate(from, to, factor, unit, precision).map_err(failed)?
            }
        } else {
            let at = a.nums::<2>("at")?.unwrap_or([0.0, 0.0]);
            let [x, y] = self.doc(a)?.measurement_to_user(page, at).map_err(failed)?;
            return Ok(json!({"scale":self.doc(a)?.measurement_scale(page,[x,y]).map_err(failed)?}));
        };
        let rect = a.nums::<4>("rect")?.unwrap_or([0.0, 0.0, f64::from(info.width), f64::from(info.height)]);
        if rect.iter().any(|v| v.abs() > 1e9) {
            return Err(failed("viewport coordinates must be within 1e9 points"));
        }
        let from = self.doc(a)?.measurement_to_user(page, [rect[0], rect[1]]).map_err(failed)?;
        let to = self.doc(a)?.measurement_to_user(page, [rect[2], rect[3]]).map_err(failed)?;
        let bbox = [from[0].min(to[0]), from[1].min(to[1]), from[0].max(to[0]), from[1].max(to[1])];
        let mut out = self
            .apply(a, Edit::SetMeasurementScale { page, bbox, name: a.opt_str("name")?.unwrap_or("Drawing scale").into(), scale: scale.clone() })?;
        out["scale"] = json!(scale);
        Ok(out)
    }
    pub(crate) fn measurement_snap(&self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let at = a.nums::<2>("at")?.ok_or_else(|| failed("snapping needs at"))?;
        let [x, y] = self.doc(a)?.measurement_to_user(page, at).map_err(failed)?;
        let geometry = self.doc(a)?.measurement_paths(page).map_err(failed)?;
        let options = measure::snap::SnapOptions {
            endpoints: a.opt_bool("endpoints")?.unwrap_or(true),
            midpoints: a.opt_bool("midpoints")?.unwrap_or(true),
            intersections: a.opt_bool("intersections")?.unwrap_or(true),
            paths: a.opt_bool("paths")?.unwrap_or(true),
        };
        let tolerance = a.opt_num("tolerance")?.unwrap_or(6.0);
        let adjacent = self.doc(a)?.measurement_to_user(page, [at[0] + tolerance, at[1]]).map_err(failed)?;
        let tolerance = measure::distance([x, y], adjacent);
        let intersection_limited = options.intersections && geometry.intersection_limited([x, y], tolerance);
        let hit = geometry.snap([x, y], tolerance, options).map_err(failed)?;
        let snap = hit
            .map(|s| -> Result<Value> {
                let point = self.doc(a)?.measurement_to_view(page, s.point).map_err(failed)?;
                Ok(json!({"point":point,"kind":s.kind,"distance":measure::distance(at,point)}))
            })
            .transpose()?;
        Ok(json!({"snap":snap,"truncated":geometry.truncated,"intersection_limited":intersection_limited,"segments":geometry.segment_count()}))
    }
    /// The segments `measurement_snap` snaps to, listed for whole pages. Each page is bounded by
    /// snapping's limits and the answer by `max_total_segments`; `next_page` and `pages_left`
    /// appear only when the answer stopped short, as `link_list`'s `text_links_next_page` does.
    pub(crate) fn page_geometry(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let pages = if a.get("pages").is_some() { self.pages(a, "pages")? } else { (0..doc.info.pages.len()).collect() };
        let display = match a.opt_str("coords")?.unwrap_or("page") {
            "page" => false,
            "display" => true,
            _ => return Err(ToolError::InvalidArgs("coords must be page or display".into())),
        };
        let limits = geometry_limits(a)?;
        let budget = total_segments(a, limits)?;
        let (listed, stopped) = pages_within(doc, &pages, limits, budget, display)?;
        // Built by moving the pages in: `json!` would serialize a copy of every segment.
        let mut out = Map::new();
        out.insert("coords".into(), json!(if display { "display" } else { "page" }));
        out.insert("limits".into(), json!({"segments": limits.segments, "targets": limits.targets, "bytes": limits.bytes}));
        out.insert("pages".into(), Value::Array(listed));
        if let Some(i) = stopped {
            out.insert("next_page".into(), json!(pages[i] + 1));
            out.insert("pages_left".into(), json!(pages.len() - i));
        }
        Ok(Value::Object(out))
    }
    pub(crate) fn measurement_export(&self, a: &Args) -> Result<Value> {
        let all = self.doc(a)?.measurements().map_err(failed)?;
        let csv = measure::csv(&all.measurements);
        let target = self.resolve(a.str("out")?, true)?;
        crate::write_atomic(&target, csv.as_bytes())?;
        Ok(json!({"path":target,"count":all.measurements.len(),"unsupported":all.unsupported.len(),"truncated":all.truncated}))
    }
}

/// The most a caller may raise `page_geometry`'s limits to: about 2.5 times what heavy CAD sheets
/// need, so one call stays within a few gigabytes.
const MAX_GEOMETRY_LIMITS: measure::snap::Limits = measure::snap::Limits { segments: 20_000_000, targets: 60_000_000, bytes: 2 << 30 };

/// The snapping limits a call asks for, each defaulting to what snapping uses.
fn geometry_limits(a: &Args) -> Result<measure::snap::Limits> {
    let get = |key: &str, default: usize, most: usize| -> Result<usize> {
        let Some(n) = a.opt_int(key)? else { return Ok(default) };
        usize::try_from(n)
            .ok()
            .filter(|n| (1..=most).contains(n))
            .ok_or_else(|| ToolError::InvalidArgs(format!("{key} must be between 1 and {most}")))
    };
    let (default, most) = (measure::snap::Limits::DEFAULT, MAX_GEOMETRY_LIMITS);
    Ok(measure::snap::Limits {
        segments: get("max_segments", default.segments, most.segments)?,
        targets: get("max_targets", default.targets, most.targets)?,
        bytes: get("max_bytes", default.bytes, most.bytes)?,
    })
}

/// Coordinates to 0.0001 units: finer than the `f32` geometry keeps on a large sheet, and short to
/// print.
fn round(v: f64) -> f64 {
    let r = (v * 10_000.0).round() / 10_000.0;
    if r == 0.0 { 0.0 } else { r }
}

/// The page's user-to-display transform as `[a, b, c, d, e, f]`. It is affine, so where it puts
/// the origin and the two unit points fixes it, and a sheet's millions of points then convert
/// without reading the page's boxes again for each one.
fn view_matrix(doc: &pdfcraft_engine::Document, page: usize) -> Result<[f64; 6]> {
    let view = |p: measure::Point| doc.measurement_to_view(page, p).map_err(failed);
    let [e, f] = view([0.0, 0.0])?;
    let [a, b] = view([1.0, 0.0])?;
    let [c, d] = view([0.0, 1.0])?;
    Ok([a - e, b - f, c - e, d - f, e, f])
}

/// The most segments one answer may hold across its pages (`max_total_segments`). By default
/// the call's `max_segments`, so a whole answer holds no more than one page may.
fn total_segments(a: &Args, limits: measure::snap::Limits) -> Result<usize> {
    let most = MAX_GEOMETRY_LIMITS.segments;
    let Some(n) = a.opt_int("max_total_segments")? else { return Ok(limits.segments) };
    usize::try_from(n)
        .ok()
        .filter(|n| (1..=most).contains(n))
        .ok_or_else(|| ToolError::InvalidArgs(format!("max_total_segments must be between 1 and {most}")))
}

/// The pages' answers, in the order asked, up to the first page that would take the segments
/// listed past `budget`; that page's index in `pages` comes back as where to resume. The first
/// page is always listed (its own limits bound it), so following `next_page` always progresses.
fn pages_within(
    doc: &pdfcraft_engine::Document,
    pages: &[usize],
    limits: measure::snap::Limits,
    budget: usize,
    display: bool,
) -> Result<(Vec<Value>, Option<usize>)> {
    let (mut listed, mut total) = (Vec::new(), 0usize);
    for (i, &page) in pages.iter().enumerate() {
        let geometry = doc.measurement_paths_within(page, limits).map_err(failed)?;
        // Counted before any JSON is built, so a page that doesn't fit costs only its extraction.
        total = total.saturating_add(geometry.segment_count());
        if i > 0 && total > budget {
            return Ok((listed, Some(i)));
        }
        listed.push(page_geometry_json(doc, page, geometry, display)?);
    }
    Ok((listed, None))
}

fn page_geometry_json(doc: &pdfcraft_engine::Document, page: usize, geometry: measure::snap::Geometry, display: bool) -> Result<Value> {
    let m = if display { Some(view_matrix(doc, page)?) } else { None };
    let point = |[x, y]: measure::Point| match m {
        Some(m) => [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]],
        None => [x, y],
    };
    let segments: Vec<Value> = geometry
        .segments()
        .map(|[from, to]| {
            let ([x0, y0], [x1, y1]) = (point(from), point(to));
            json!([x0, y0, x1, y1].map(round))
        })
        .collect();
    let (truncated, unreadable) = (geometry.truncated, geometry.unreadable);
    // The extraction and its snapping index go before the page's answer is assembled, and the
    // segments are moved in rather than copied by `json!`.
    drop(geometry);
    let mut out = Map::new();
    out.insert("page".into(), json!(page + 1));
    out.insert("count".into(), json!(segments.len()));
    out.insert("segments".into(), Value::Array(segments));
    out.insert("truncated".into(), json!(truncated));
    out.insert("unreadable".into(), json!(unreadable));
    Ok(Value::Object(out))
}
