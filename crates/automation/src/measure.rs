//! Measurement tools share the engine's PDF user-space geometry and scale semantics.
use crate::{Args, Automation, Result, ToolError, failed};
use pdfcraft_engine::{
    Edit, Style,
    measure::{self, Kind, NewMeasurement, Scale},
};
use serde_json::{Value, json};

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
        let scale = if a.get("points").is_some() {
            if a.get("units_per_point").is_some() {
                return Err(failed("pass calibration points or units_per_point, not both"));
            }
            let (_, points) = self.measurement_points(a)?;
            let [from, to] = points.as_slice() else {
                return Err(failed("scale calibration needs exactly two points"));
            };
            let length = a.opt_num("distance")?.ok_or_else(|| failed("calibration needs a real-world distance"))?;
            Scale::calibrate(*from, *to, length, unit, precision).map_err(failed)?
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
        Ok(json!({"snap":snap,"truncated":geometry.truncated,"intersection_limited":intersection_limited,"segments":geometry.segments.len()}))
    }
    pub(crate) fn measurement_export(&self, a: &Args) -> Result<Value> {
        let all = self.doc(a)?.measurements().map_err(failed)?;
        let csv = measure::csv(&all.measurements);
        let target = self.resolve(a.str("out")?, true)?;
        crate::write_atomic(&target, csv.as_bytes())?;
        Ok(json!({"path":target,"count":all.measurements.len(),"unsupported":all.unsupported.len(),"truncated":all.truncated}))
    }
}
