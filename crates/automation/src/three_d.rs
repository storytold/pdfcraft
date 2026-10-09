//! Headless tools use the same model-space geometry and edits as the desktop.
use crate::{Args, Automation, Result, failed};
use pdfcraft_engine::{
    Edit,
    measure::three_d::{self, Geometry, Measurement, NewMeasurement},
};
use serde_json::{Value, json};
fn index(a: &Args, key: &str) -> Result<Option<usize>> {
    a.opt_int(key)?
        .map(|n| {
            usize::try_from(n.checked_sub(1).ok_or_else(|| failed(format!("{key} indices start at 1")))?)
                .map_err(|_| failed(format!("{key} indices start at 1")))
        })
        .transpose()
}
fn preview_camera(
    a: &Args,
    doc: &pdfcraft_engine::Document,
    artwork: &three_d::Artwork,
    view: Option<usize>,
) -> Result<(pdfcraft_engine::measure::three_d_camera::Camera, [u32; 2])> {
    let mut camera = doc.three_d_camera(artwork, view).map_err(failed)?;
    let width = u32::try_from(a.opt_int("width")?.unwrap_or(640)).map_err(|_| failed("invalid 3D preview width"))?;
    let height = u32::try_from(a.opt_int("height")?.unwrap_or(480)).map_err(|_| failed("invalid 3D preview height"))?;
    if width == 0 || height == 0 || width > 2048 || height > 2048 {
        return Err(failed("3D viewport width and height must be 1 to 2048"));
    }
    if a.get("yaw").is_some() || a.get("pitch").is_some() {
        camera.orbit(a.opt_num("yaw")?.unwrap_or(0.), a.opt_num("pitch")?.unwrap_or(0.)).map_err(failed)?;
    }
    if let Some(factor) = a.opt_num("zoom")? {
        camera.zoom(factor).map_err(failed)?;
    }
    if let Some(delta) = a.nums::<2>("pan")? {
        camera.pan(delta, [f64::from(width), f64::from(height)]).map_err(failed)?;
    }
    Ok((camera, [width, height]))
}
impl Automation {
    fn three_d_target(&self, a: &Args) -> Result<(usize, usize, Option<usize>)> {
        Ok((self.page(a)?, index(a, "annotation")?.ok_or_else(|| failed("a 3D model annotation index is required"))?, index(a, "view")?))
    }
    pub(crate) fn three_d_models(&self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let models = self.doc(a)?.three_d_models(page).map_err(failed)?;
        Ok(json!({"models":models.into_iter().map(|m| {
                let (rect, error) = match m.rect { Ok(rect) => (Some(rect), None), Err(error) => (None, Some(error)) };
                json!({"annotation":m.annotation+1,"name":m.name,"format":m.format,"rect_user_space":rect,"rect_error":error})
            }).collect::<Vec<_>>()}))
    }
    pub(crate) fn three_d_info(&self, a: &Args) -> Result<Value> {
        let (page, annotation, _) = self.three_d_target(a)?;
        let artwork = self.doc(a)?.three_d_artwork(page, annotation).map_err(failed)?;
        let meshes=artwork.scene.meshes.iter().enumerate().map(|(index,m)|json!({"index":index+1,"name":m.name,"positions":m.positions.len(),"triangles":m.triangles.len(),"points":m.points.len(),"lines":m.lines.len(),"normals":m.normals.len()})).collect::<Vec<_>>();
        let instances = artwork
            .scene
            .instances
            .iter()
            .enumerate()
            .map(|(index, i)| json!({"index":index+1,"name":i.name,"mesh":i.mesh+1,"transform":i.transform,"visibility":i.visibility}))
            .collect::<Vec<_>>();
        Ok(
            json!({"annotation":annotation+1,"edit_error":artwork.edit_error,"units":artwork.units,"bounds":artwork.scene.bounds().map_err(failed)?,"views":artwork.views.iter().enumerate().map(|(index,v)|json!({"index":index+1,"view":v})).collect::<Vec<_>>(),"default_view":artwork.default_view,"meshes":meshes,"instances":instances}),
        )
    }
    pub(crate) fn three_d_pick(&self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::measure::three_d_snap::{Hit, Kind, Mode};
        let (page, annotation, view) = self.three_d_target(a)?;
        let artwork = self.doc(a)?.three_d_artwork(page, annotation).map_err(failed)?;
        let hit = if let Some(pixel) = a.nums::<2>("pixel")? {
            if a.get("origin").is_some() || a.get("direction").is_some() {
                return Err(failed("screen picking cannot be combined with a world ray"));
            }
            let (camera, size) = preview_camera(a, self.doc(a)?, &artwork, view)?;
            camera
                .snap(
                    &artwork.scene,
                    pixel,
                    size.map(f64::from),
                    Mode::from_name(a.opt_str("snap")?.unwrap_or("surface")).map_err(failed)?,
                    a.opt_num("radius")?.unwrap_or(8.),
                )
                .map_err(failed)?
        } else {
            for key in ["width", "height", "yaw", "pitch", "pan", "zoom", "snap", "radius"] {
                if a.get(key).is_some() {
                    return Err(failed("camera controls require screen pixel picking"));
                }
            }
            let origin = a.nums::<3>("origin")?.ok_or_else(|| failed("3D picking needs a screen pixel or a model-space ray origin"))?;
            let direction = a.nums::<3>("direction")?.ok_or_else(|| failed("3D picking needs a model-space ray direction"))?;
            artwork.scene.pick(origin, direction).map_err(failed)?.map(|h| Hit {
                instance: h.instance,
                primitive: h.triangle,
                triangle: Some(h.triangle),
                kind: Kind::Surface,
                point: h.point,
                screen_distance: 0.,
                ray_distance: h.ray_distance,
                barycentric: Some(h.barycentric),
            })
        };
        Ok(
            json!({"hit":hit.map(|h|json!({"instance":h.instance+1,"triangle":h.triangle.map(|n|n+1),"primitive":h.primitive+1,"kind":h.kind,"point":h.point,"screen_distance":h.screen_distance,"ray_distance":h.ray_distance,"barycentric":h.barycentric}))}),
        )
    }
    pub(crate) fn three_d_render(&self, a: &Args) -> Result<crate::Content> {
        let (page, annotation, view) = self.three_d_target(a)?;
        let artwork = self.doc(a)?.three_d_artwork(page, annotation).map_err(failed)?;
        let (camera, [width, height]) = preview_camera(a, self.doc(a)?, &artwork, view)?;
        let raster = pdfcraft_engine::measure::three_d_raster::render(
            &artwork.scene,
            &camera,
            pdfcraft_engine::measure::three_d_raster::Options { width, height, ..Default::default() },
        )
        .map_err(failed)?;
        let data = crate::encode_png(width, height, &raster.rgba)?;
        Ok(crate::Content::Png { data, width, height })
    }
    pub(crate) fn three_d_list(&self, a: &Args) -> Result<Value> {
        let (page, annotation, view) = self.three_d_target(a)?;
        let list = self.doc(a)?.three_d_measurements(page, annotation, view).map_err(failed)?;
        let measurements = list
            .measurements
            .into_iter()
            .map(|m| {
                let mut value = json!(m);
                value["index"] = json!(m.index + 1);
                value["caption"] = json!(m.caption());
                value
            })
            .collect::<Vec<_>>();
        Ok(
            json!({"measurements":measurements,"unsupported":list.unsupported.into_iter().map(|u|json!({"index":u.index+1,"reason":u.reason})).collect::<Vec<_>>()}),
        )
    }
    pub(crate) fn three_d_measure(&mut self, a: &Args) -> Result<Value> {
        let (page, annotation, view) = self.three_d_target(a)?;
        let item = index(a, "index")?;
        if a.opt_bool("remove")?.unwrap_or(false) {
            if [
                "geometry",
                "points",
                "kind",
                "diameter",
                "capture_view",
                "save",
                "yaw",
                "pitch",
                "pan",
                "zoom",
                "width",
                "height",
                "plane",
                "text_position",
                "text_pixel",
                "text_size",
                "text_y",
                "extension_length",
                "show_circle",
                "unit",
                "units_per_model_unit",
                "precision",
                "radians",
                "color",
                "label",
                "text",
            ]
            .iter()
            .any(|key| a.get(key).is_some())
            {
                return Err(failed("removal cannot be combined with new geometry or save"));
            }
            return self.apply(
                a,
                Edit::RemoveThreeDMeasurement {
                    page,
                    annotation,
                    view,
                    index: item.ok_or_else(|| failed("removing a 3D measurement requires its index"))?,
                },
            );
        }
        let artwork = self.doc(a)?.three_d_artwork(page, annotation).map_err(failed)?;
        let text_pixel = a.nums::<2>("text_pixel")?;
        if text_pixel.is_some() && a.get("text_position").is_some() {
            return Err(failed("choose either text_pixel or text_position"));
        }
        let capture = a.opt_bool("capture_view")?.unwrap_or(false)
            || text_pixel.is_some()
            || ["yaw", "pitch", "pan", "zoom"].iter().any(|key| a.get(key).is_some());
        let camera = if capture || ["width", "height"].iter().any(|key| a.get(key).is_some()) {
            Some(preview_camera(a, self.doc(a)?, &artwork, view)?)
        } else {
            None
        };

        for key in ["label", "text"] {
            if let Some(text) = a.opt_str(key)?
                && (text.len() > 512 || text.chars().any(char::is_control))
            {
                return Err(failed(format!("{key} must have at most 512 bytes and no control characters")));
            }
        }
        let mut units = artwork.units.clone();
        match (a.opt_str("unit")?, a.opt_num("units_per_model_unit")?) {
            (Some(unit), Some(factor)) if !unit.trim().is_empty() && unit.chars().count() <= 24 && (1e-12..=1e12).contains(&factor) => {
                units.unit = unit.into();
                units.factor = factor;
                units.declared = true;
            }
            (None, None) => {}
            _ => return Err(failed("a unit override requires a printable unit and a positive bounded units_per_model_unit")),
        }
        let mut measurement = match (a.get("geometry"), a.get("points")) {
            (Some(geometry), None) => {
                if a.get("kind").is_some() || a.get("diameter").is_some() {
                    return Err(failed("kind and diameter apply to point constructions"));
                }
                let geometry: Geometry =
                    serde_json::from_value(geometry.clone()).map_err(|e| failed(format!("invalid 3D measurement geometry: {e}")))?;
                let [first, second] = geometry.anchors();
                let default_position = std::array::from_fn(|i| (first[i] + second[i]) / 2.);
                Measurement::new(
                    geometry,
                    &units,
                    a.nums::<3>("plane")?.unwrap_or([0., 0., 1.]),
                    a.nums::<3>("text_position")?.unwrap_or(default_position),
                )
                .map_err(failed)?
            }
            (None, Some(points)) => {
                let points =
                    points.as_array().filter(|p| (2..=3).contains(&p.len())).ok_or_else(|| failed("3D construction needs two or three points"))?;
                let points: Vec<three_d::Point> = points
                    .iter()
                    .map(|p| serde_json::from_value(p.clone()).map_err(|e| failed(format!("invalid 3D point: {e}"))))
                    .collect::<Result<_>>()?;
                let kind = match a.opt_str("kind")?.ok_or_else(|| failed("a point construction requires kind"))? {
                    "linear" => three_d::Kind::Linear,
                    "perpendicular" => three_d::Kind::Perpendicular,
                    "angular" => three_d::Kind::Angular,
                    "radial" => three_d::Kind::Radial,
                    _ => return Err(failed("3D kind must be linear, perpendicular, angular or radial")),
                };
                let mut measurement = Measurement::from_points(
                    kind,
                    &points,
                    &units,
                    a.nums::<3>("plane")?.unwrap_or([0., 0., 1.]),
                    a.opt_bool("diameter")?.unwrap_or(false),
                )
                .map_err(failed)?;
                if let Some(position) = a.nums::<3>("text_position")? {
                    measurement.text_position = position;
                }
                measurement
            }
            _ => return Err(failed("supply either 3D geometry or construction points")),
        };
        if let Some(pixel) = text_pixel {
            let (camera, size) = camera.as_ref().ok_or_else(|| failed("3D caption camera is unavailable"))?;
            measurement.text_position =
                camera.plane_point(pixel, size.map(f64::from), measurement.text_position, measurement.plane).map_err(failed)?;
        }
        if let Some(up) = a.nums::<3>("text_y")? {
            measurement.text_y = up;
        }
        if let Some(size) = a.opt_num("text_size")? {
            measurement.text_size = size;
        }
        if let Some(length) = a.opt_num("extension_length")? {
            if measurement.geometry.kind() != three_d::Kind::Radial {
                return Err(failed("extension_length applies only to radial measurements"));
            }
            measurement.extension_length = length;
        }
        measurement.show_circle = a.opt_bool("show_circle")?.unwrap_or(false);
        measurement.precision = u8::try_from(a.opt_int("precision")?.unwrap_or(3)).map_err(|_| failed("3D precision must be between 0 and 12"))?;
        measurement.label = a.opt_str("label")?.unwrap_or("").into();
        measurement.user_text = a.opt_str("text")?.unwrap_or("").into();
        if a.opt_bool("radians")?.unwrap_or(false) {
            if measurement.geometry.kind() != three_d::Kind::Angular {
                return Err(failed("radians applies only to angular measurements"));
            }
            measurement.degrees = false;
            measurement.value = measurement.geometry.value().map_err(failed)?;
            measurement.unit = "rad".into();
        }
        if let Some(color) = a.nums::<3>("color")? {
            measurement.color = color;
        }
        measurement.dictionary().map_err(failed)?;
        let mut out = json!({"measurement":measurement,"caption":measurement.caption()});
        if a.opt_bool("save")?.unwrap_or(false) {
            let camera = if capture { camera.map(|(camera, _)| camera) } else { None };
            let new = NewMeasurement { page, annotation, view, measurement, camera };
            let edit = match item {
                Some(index) => Edit::UpdateThreeDMeasurement { measurement: new, index },
                None => Edit::AddThreeDMeasurement(new),
            };
            out["edit"] = self.apply(a, edit)?;
        } else if item.is_some() {
            return Err(failed("editing a saved measurement requires save=true"));
        }
        Ok(out)
    }
}
