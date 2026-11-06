//! `model_pick`: what of a page's 3D model lies under a position of the picture `model_render` draws.

use pdfcraft_prc_render::pick;
use serde_json::{Value, json};

use super::Automation;
use super::error::{Result, failed};
use super::json::Args;

impl Automation {
    /// The triangle under `at` (pixels, y down, from the picture's top left) of the picture
    /// `model_render` draws with the same arguments, or `{ "hit": null }` over the background.
    pub(crate) fn model_pick(&mut self, a: &Args) -> Result<Value> {
        let [x, y] = position(a)?;
        let view = self.model_view(a)?;
        if !(x <= f64::from(view.width) && y <= f64::from(view.height)) {
            return Err(Args::wrong("at", &format!("inside the {}x{} picture", view.width, view.height)));
        }
        let ray = view.camera.ray_through(x, y, view.width, view.height).map_err(failed)?;
        let hit = pick(&view.meshes, &ray).map(|h| {
            json!({ "mesh": h.mesh.saturating_add(1), "triangle": h.triangle.saturating_add(1), "point": h.point, "normal": h.normal, "distance": h.distance })
        });
        Ok(json!({ "hit": hit }))
    }
}

/// `at`: two finite, non-negative numbers.
fn position(a: &Args) -> Result<[f64; 2]> {
    let wrong = || Args::wrong("at", "two numbers [x, y], pixels from the picture's top left");
    let items = a.get("at").and_then(Value::as_array).filter(|x| x.len() == 2).ok_or_else(wrong)?;
    let mut out = [0.0; 2];
    for (o, v) in out.iter_mut().zip(items) {
        *o = v.as_f64().filter(|f| f.is_finite() && *f >= 0.0).ok_or_else(wrong)?;
    }
    Ok(out)
}
