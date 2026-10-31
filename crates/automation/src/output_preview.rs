//! Output Preview (Print production ▸ Output preview): a page's inks, their coverage, total
//! area coverage against a limit, and the page as its inks print it, overprinting included.

use pdfcraft_render::{Separations, Simulation};
use serde_json::{Value, json};

use crate::{Args, Automation, Content, Result, ToolError, encode_png, failed};

/// Resolution the tool separates at unless told otherwise.
const DEFAULT_DPI: f64 = 72.0;
/// Every ink is a render of its own, so the tool goes lower than `page_render`.
pub(crate) const MAX_DPI: f64 = 300.0;
/// The total area coverage limit, in percent, unless told otherwise: a common limit for coated
/// stock (presses publish their own).
pub(crate) const DEFAULT_TAC_LIMIT: f64 = 300.0;
/// Points the tool reads coverage at, per call.
pub(crate) const MAX_POINTS: usize = 256;
/// The colour `mark_tac` paints pixels over the limit with.
const MARK: [u8; 3] = [0, 220, 0];

fn bad(m: impl Into<String>) -> ToolError {
    ToolError::InvalidArgs(m.into())
}

/// A percentage to one decimal.
fn pct(v: f32) -> f64 {
    (f64::from(v) * 10.0).round() / 10.0
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// `[x, y]` pairs in displayed points, at most [`MAX_POINTS`].
fn points(a: &Args) -> Result<Vec<[f64; 2]>> {
    let Some(v) = a.get("points") else { return Ok(Vec::new()) };
    let wrong = || bad("points must be an array of [x, y] pairs of numbers");
    let list = v.as_array().ok_or_else(wrong)?;
    if list.len() > MAX_POINTS {
        return Err(bad(format!("at most {MAX_POINTS} points")));
    }
    list.iter()
        .map(|p| match p.as_array().map(Vec::as_slice) {
            Some([x, y]) => match (x.as_f64(), y.as_f64()) {
                (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Ok([x, y]),
                _ => Err(wrong()),
            },
            _ => Err(wrong()),
        })
        .collect()
}

impl Automation {
    pub(crate) fn print_output_preview(&mut self, a: &Args) -> Result<Vec<Content>> {
        let id = self.doc(a)?.id;
        let page = self.page(a)?;
        let dpi = a.opt_num("dpi")?.unwrap_or(DEFAULT_DPI);
        if !(1.0..=MAX_DPI).contains(&dpi) {
            return Err(bad(format!("dpi must be between 1 and {MAX_DPI}")));
        }
        let overprint = a.opt_bool("overprint")?.unwrap_or(true);
        let limit = a.opt_num("tac_limit")?.unwrap_or(DEFAULT_TAC_LIMIT);
        if !(0.0..=10_000.0).contains(&limit) {
            return Err(bad("tac_limit must be a percentage between 0 and 10000"));
        }
        let image = a.opt_str("image")?.unwrap_or("none");
        if !matches!(image, "none" | "composite" | "plate") {
            return Err(bad("image must be none, composite or plate"));
        }
        let mark = a.opt_bool("mark_tac")?.unwrap_or(false);
        let only = if a.get("inks").is_some() { Some(a.strs("inks")?) } else { None };
        let plate = a.opt_str("plate")?;
        let points = points(a)?;

        let s = self.renderer(id)?.separate(page, (dpi / 72.0) as f32, overprint).map_err(failed)?;
        let known = |name: &str| s.inks.iter().any(|i| i.name() == name) || s.unseparated.iter().any(|u| u == name);
        if let Some(missing) = only.iter().flatten().chain(plate.iter()).find(|n| !known(n)) {
            let names: Vec<&str> = s.inks.iter().map(|i| i.name()).collect();
            return Err(bad(format!("page {} has no ink {missing:?}; its inks are {}", page + 1, names.join(", "))));
        }
        let enabled: Vec<bool> = s.inks.iter().map(|i| only.as_ref().is_none_or(|o| o.contains(&i.name()))).collect();
        let mut out = vec![Content::Json(report(&s, &enabled, limit as f32, &points))];
        match image {
            "composite" => {
                let pixels = s.composite(&enabled, mark.then_some((limit as f32, MARK)));
                out.push(Content::Png { data: encode_png(s.width, s.height, &pixels)?, width: s.width, height: s.height });
            }
            "plate" => {
                let name = plate.ok_or_else(|| bad("image plate needs plate (an ink name)"))?;
                let index = s
                    .inks
                    .iter()
                    .position(|i| i.name() == name)
                    .ok_or_else(|| failed(format!("ink {name:?} is not separated (too many spot inks)")))?;
                let film = s.film(index).ok_or_else(|| failed("no such plate"))?;
                out.push(Content::Png { data: encode_png(s.width, s.height, &film)?, width: s.width, height: s.height });
            }
            _ => {}
        }
        Ok(out)
    }
}

/// How process colour from other spaces became CMYK: `label` is what the panel shows after
/// "Simulation:" (the output intent's profile, or "device formulas").
fn simulation(s: &Simulation) -> Value {
    match s {
        Simulation::OutputIntent { profile, condition, perceptual } => json!({
            "label": s.label(),
            "method": "output_intent",
            "profile": profile,
            "condition": condition,
            "rendering_intent": if *perceptual { "Perceptual" } else { "RelativeColorimetric" },
        }),
        Simulation::DeviceFormulas { reason } => json!({ "label": s.label(), "method": "device_formulas", "reason": reason }),
    }
}

/// The tool's JSON: inks, total area coverage and the coverage at `points`.
fn report(s: &Separations, enabled: &[bool], limit: f32, points: &[[f64; 2]]) -> Value {
    let scale = f64::from(s.scale);
    let stats = s.stats();
    let inks: Vec<Value> = s
        .inks
        .iter()
        .zip(&stats)
        .zip(enabled)
        .map(|((ink, stat), on)| {
            json!({
                "name": ink.name(),
                "kind": if ink.is_spot() { "spot" } else { "process" },
                "color": hex(ink.swatch),
                "enabled": on,
                "max": pct(stat.max),
                "area": pct(stat.area),
            })
        })
        .collect();
    let to_points = |x: u32, y: u32| [(f64::from(x) + 0.5) / scale, (f64::from(y) + 0.5) / scale].map(|v| (v * 10.0).round() / 10.0);
    let above = s.above(limit, enabled);
    let above_area = above.iter().filter(|o| **o).count() as f32 * 100.0 / above.len().max(1) as f32;
    let max = s.max_total(enabled).map(|(max, x, y)| json!({ "percent": pct(max), "at": to_points(x, y) }));
    let at: Vec<Value> = points
        .iter()
        .map(|&[x, y]| {
            let pixel = ((x * scale).floor(), (y * scale).floor());
            let coverage = (pixel.0 >= 0.0 && pixel.1 >= 0.0)
                .then(|| s.coverage_at(pixel.0.min(f64::from(u32::MAX)) as u32, pixel.1.min(f64::from(u32::MAX)) as u32, enabled))
                .flatten();
            match coverage {
                Some(c) => {
                    let inks: serde_json::Map<String, Value> =
                        s.inks.iter().zip(&c.inks).map(|(i, v)| (i.name().to_string(), json!(pct(*v)))).collect();
                    json!({ "at": [x, y], "inks": inks, "total": pct(c.total) })
                }
                None => json!({ "at": [x, y], "outside": true }),
            }
        })
        .collect();
    json!({
        "page": s.page + 1,
        "width": s.width,
        "height": s.height,
        "dpi": (scale * 72.0 * 10.0).round() / 10.0,
        "overprint": s.overprint,
        "simulation": simulation(&s.simulation),
        "inks": inks,
        "unseparated": s.unseparated,
        "total_area_coverage": { "max": max, "limit": limit, "above_area": pct(above_area) },
        "points": at,
    })
}
