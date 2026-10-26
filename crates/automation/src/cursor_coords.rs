//! Cursor coordinates calculation and formatting (Acrobat's View ▸ Show/Hide ▸ Cursor Coordinates, `Ctrl+Y`).
//!
//! Provides headless automation tool support for querying page dimensions, evaluating cursor
//! positions in user and view coordinates, and formatting values in points, inches, millimeters, or centimeters.

use pdfcraft_engine::measure::PageUnit;
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError};

/// Units supported by the cursor coordinates tool.
const UNITS: &str = "pt, in, mm, cm";

/// Coordinate spaces accepted for input points.
const SPACES: &str = "user, view";

/// Origin anchors accepted for coordinate offsets.
const ORIGINS: &str = "bottom-left, top-left";

impl Automation {
    /// Calculate cursor coordinates and page dimensions for a document page.
    ///
    /// Accepts an optional point in either user or view coordinates and returns the converted
    /// positions along with formatted strings in the requested measurement unit.
    pub(crate) fn view_cursor_coordinates(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let page_idx = match a.opt_int("page")? {
            Some(p) => {
                let n = doc.info.pages.len();
                if p < 1 || (p as usize) > n {
                    return Err(ToolError::InvalidArgs(format!("page {p} is out of range: the document has {n} pages")));
                }
                (p as usize) - 1
            }
            None => 0,
        };
        let p_info = &doc.info.pages[page_idx];

        let unit_str = a.opt_str("unit")?.unwrap_or("pt");
        let unit = PageUnit::from_label(unit_str).ok_or_else(|| ToolError::InvalidArgs(format!("unit must be one of: {UNITS}")))?;

        let space = a.opt_str("space")?.unwrap_or("user");
        if space != "user" && space != "view" {
            return Err(ToolError::InvalidArgs(format!("space must be one of: {SPACES}")));
        }

        let origin = a.opt_str("origin")?.unwrap_or("bottom-left");
        if origin != "bottom-left" && origin != "top-left" {
            return Err(ToolError::InvalidArgs(format!("origin must be one of: {ORIGINS}")));
        }

        let [cx0, cy0, _cx1, _cy1] = p_info.crop;
        let page_w_pt = f64::from(p_info.width);
        let page_h_pt = f64::from(p_info.height);

        let scale = 1.0 / unit.points();
        let decimals = match unit {
            PageUnit::Pt | PageUnit::Mm => 1,
            PageUnit::In | PageUnit::Cm => 2,
        };

        let page_w = page_w_pt * scale;
        let page_h = page_h_pt * scale;

        let mut out = json!({
            "doc": doc.id.0,
            "page": page_idx + 1,
            "unit": unit.label(),
            "origin": origin,
            "page_width": page_w,
            "page_height": page_h,
            "formatted_width": format!("{page_w:.decimals$} {}", unit.label()),
            "formatted_height": format!("{page_h:.decimals$} {}", unit.label()),
        });

        if let Some(coords) = a.get("point") {
            let arr = coords.as_array().ok_or_else(|| ToolError::InvalidArgs("point must be an array of two numbers".into()))?;
            if arr.len() != 2 {
                return Err(ToolError::InvalidArgs("point must contain exactly 2 numbers [x, y]".into()));
            }
            let raw_x = arr[0].as_f64().ok_or_else(|| ToolError::InvalidArgs("point x coordinate must be a number".into()))?;
            let raw_y = arr[1].as_f64().ok_or_else(|| ToolError::InvalidArgs("point y coordinate must be a number".into()))?;

            let (ux, uy, vx, vy) = if space == "view" {
                let (vx_f32, vy_f32) = (raw_x as f32, raw_y as f32);
                let [ux_f32, uy_f32] = p_info.view_to_user(vx_f32, vy_f32);
                (f64::from(ux_f32), f64::from(uy_f32), f64::from(vx_f32), f64::from(vy_f32))
            } else {
                let (ux_f32, uy_f32) = (raw_x as f32, raw_y as f32);
                let [vx_f32, vy_f32] = p_info.user_to_view(ux_f32, uy_f32);
                (f64::from(ux_f32), f64::from(uy_f32), f64::from(vx_f32), f64::from(vy_f32))
            };

            let (coord_x_pt, coord_y_pt) = if origin == "bottom-left" { (ux - f64::from(cx0), uy - f64::from(cy0)) } else { (vx, vy) };

            let x_val = coord_x_pt * scale;
            let y_val = coord_y_pt * scale;

            if let Some(map) = out.as_object_mut() {
                map.insert(
                    "point".into(),
                    json!({
                        "x": x_val,
                        "y": y_val,
                        "formatted_x": format!("{x_val:.decimals$} {}", unit.label()),
                        "formatted_y": format!("{y_val:.decimals$} {}", unit.label()),
                        "user": [ux, uy],
                        "view": [vx, vy],
                    }),
                );
            }
        }

        Ok(out)
    }
}
