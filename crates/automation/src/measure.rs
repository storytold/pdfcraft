//! Measurement tools share the engine's PDF user-space geometry and scale semantics.
use crate::{Args, Automation, Result, ToolError, failed};
use pdfcraft_engine::{
    Edit, Style,
    measure::{self, Kind, NewMeasurement, Scale},
};
use serde_json::{Value, json};

impl Automation {
    /// Viewer layer choices also determine which geometry can be snapped to.
    pub(crate) fn layer_visibility(&mut self, a: &Args) -> Result<Value> {
        let id = self.doc(a)?.id;
        if let Some(visible) = a.opt_bool("visible")? {
            let index = a.opt_int("layer")?.ok_or_else(|| failed("changing visibility needs a layer index"))?;
            let index = usize::try_from(index.checked_sub(1).ok_or_else(|| failed("layer indices start at 1"))?)
                .map_err(|_| failed("layer indices start at 1"))?;
            if self.doc(a)?.info.layers.get(index).is_none() {
                return Err(failed("layer does not exist"));
            }
            if self.session.set_layer_visible(id, index, visible) {
                self.renderers.remove(&id);
                self.texts.remove(&id);
            }
        }
        let layers = self
            .doc(a)?
            .info
            .layers
            .iter()
            .enumerate()
            .map(|(index, l)| json!({"index":index+1,"name":l.name,"visible":l.visible}))
            .collect::<Vec<_>>();
        Ok(json!({"layers":layers}))
    }

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
        if let Some(index) = a.opt_int("viewport")? {
            let index = usize::try_from(index.checked_sub(1).ok_or_else(|| failed("viewport indices start at 1"))?)
                .map_err(|_| failed("viewport indices start at 1"))?;
            if a.opt_bool("remove")?.unwrap_or(false) {
                return self.apply(a, Edit::RemoveMeasurementViewport { page, index });
            }
        } else if a.opt_bool("remove")?.unwrap_or(false) {
            return Err(failed("removing a scale needs a viewport index"));
        }
        let rect = a.nums::<4>("rect")?.unwrap_or([0.0, 0.0, f64::from(info.width), f64::from(info.height)]);
        if rect.iter().any(|v| v.abs() > 1e9) {
            return Err(failed("viewport coordinates must be within 1e9 points"));
        }
        let from = self.doc(a)?.measurement_to_user(page, [rect[0], rect[1]]).map_err(failed)?;
        let to = self.doc(a)?.measurement_to_user(page, [rect[2], rect[3]]).map_err(failed)?;
        let bbox = [from[0].min(to[0]), from[1].min(to[1]), from[0].max(to[0]), from[1].max(to[1])];
        let scale = if let Some(geospatial) = a.get("geospatial") {
            if ["formats", "points", "units_per_point", "unit", "distance", "ratio"].iter().any(|key| a.get(key).is_some()) {
                return Err(failed("geospatial registration cannot be combined with a rectilinear calibration"));
            }
            let definition: measure::geo::GeoDefinition =
                serde_json::from_value(geospatial.clone()).map_err(|e| failed(format!("invalid geospatial registration: {e}")))?;
            let mut scale = Scale::from_geo(measure::geo::GeoScale::new(definition, bbox).map_err(failed)?).map_err(failed)?;
            scale.precision = precision;
            scale.validate().map_err(failed)?;
            scale
        } else if let Some(formats) = a.get("formats") {
            if a.get("points").is_some() || a.get("units_per_point").is_some() {
                return Err(failed("pass formats or a calibrated numeric scale, not both"));
            }
            let mut formats: measure::NumberFormats =
                serde_json::from_value(formats.clone()).map_err(|e| failed(format!("invalid number formats: {e}")))?;
            // First X/Y conversions arrive in physical display points, just like units_per_point.
            let from = self.doc(a)?.measurement_to_user(page, [0.0, 0.0]).map_err(failed)?;
            let to = self.doc(a)?.measurement_to_user(page, [1.0, 0.0]).map_err(failed)?;
            let conversion = measure::distance(from, to);
            if let Some(x) = formats.x.first_mut() {
                x.factor /= conversion;
            }
            if let Some(y) = formats.y.as_mut().and_then(|y| y.first_mut()) {
                y.factor /= conversion;
            }
            Scale::from_formats(formats, a.opt_str("ratio")?.unwrap_or("Custom drawing scale")).map_err(failed)?
        } else if a.get("points").is_some() {
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
            let doc = self.doc(a)?;
            let mut viewports = Vec::new();
            for v in doc.measurement_viewports(page).map_err(failed)? {
                let a = doc.measurement_to_view(page, [v.bbox[0], v.bbox[1]]).map_err(failed)?;
                let b = doc.measurement_to_view(page, [v.bbox[2], v.bbox[3]]).map_err(failed)?;
                viewports.push(
                    json!({"index":v.index + 1,"name":v.name,"rect":[a[0].min(b[0]),a[1].min(b[1]),a[0].max(b[0]),a[1].max(b[1])],"scale":v.scale}),
                );
            }
            return Ok(json!({"scale":doc.measurement_scale(page,[x,y]).map_err(failed)?,"viewports":viewports}));
        };
        let name = a.opt_str("name")?.unwrap_or("Drawing scale").into();
        let edit = match a.opt_int("viewport")? {
            Some(index) => {
                let index = usize::try_from(index.checked_sub(1).ok_or_else(|| failed("viewport indices start at 1"))?)
                    .map_err(|_| failed("viewport indices start at 1"))?;
                Edit::UpdateMeasurementViewport { page, index, bbox, name, scale: scale.clone() }
            }
            None => Edit::SetMeasurementScale { page, bbox, name, scale: scale.clone() },
        };
        let mut out = self.apply(a, edit)?;
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
        Ok(json!({"snap":snap,"truncated":geometry.truncated,"intersection_limited":intersection_limited,"segments":geometry.segments.len(),
            "unreadable":geometry.unreadable,"visibility_limited":geometry.visibility_limited,"glyphs":geometry.glyphs,"images":geometry.images}))
    }
    pub(crate) fn measurement_export(&self, a: &Args) -> Result<Value> {
        let all = self.doc(a)?.measurements().map_err(failed)?;
        let csv = measure::csv(&all.measurements);
        let target = self.resolve(a.str("out")?, true)?;
        crate::write_atomic(&target, csv.as_bytes())?;
        Ok(json!({"path":target,"count":all.measurements.len(),"unsupported":all.unsupported.len(),"truncated":all.truncated}))
    }
}
