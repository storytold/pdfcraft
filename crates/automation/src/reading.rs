//! Reading colours (Acrobat's Replace Document Colors, #897) and Smooth images (Acrobat's Page
//! Display ▸ Smooth images) as tools (AGENTS.md §3).
//!
//! Per-document view settings: with reading colours, solid text and line-art paints remap for
//! readability and the page clears to the setting's background; with Smooth images off, images
//! that don't ask for `/Interpolate` are drawn with their own samples. The file never changes.

use pdfcraft_render::ReadingColors;
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError};

/// The modes `ReadingColors::parse` accepts, for error messages.
const MODES: &str = "off, black-on-white, white-on-black, yellow-on-black, green-on-black, custom";

impl Automation {
    /// Set the document's reading colours. Colours are only meaningful with `mode: "custom"`;
    /// `parse` enforces the rest and its errors are actionable.
    pub(crate) fn view_reading_colors(&mut self, a: &Args) -> Result<Value> {
        let id = self.doc(a)?.id;
        let Some(mode) = a.opt_str("mode")? else {
            return Err(ToolError::InvalidArgs(format!("mode is required (one of: {MODES})")));
        };
        let colors = ReadingColors::parse(mode, a.opt_str("text")?, a.opt_str("background")?, a.opt_bool("only_black")?, a.opt_bool("line_art")?)
            .map_err(ToolError::InvalidArgs)?;
        if self.session.set_reading_colors(id, colors) {
            // Cached renderers baked the old setting into their config; page_render must re-read it.
            self.renderers.remove(&id);
        }
        Ok(json!({
            "mode": colors.mode_name(),
            "text": colors.text_hex(),
            "background": colors.background_hex(),
            "only_black": colors.params.is_some_and(|p| p.only_black),
            "line_art": colors.params.is_some_and(|p| p.line_art),
        }))
    }

    /// Smooth images on or off for the document (`smooth: false` draws its images that don't
    /// ask for `/Interpolate` with their own samples, in page_render, exports and printing).
    pub(crate) fn view_smooth_images(&mut self, a: &Args) -> Result<Value> {
        let id = self.doc(a)?.id;
        let Some(smooth) = a.opt_bool("smooth")? else {
            return Err(ToolError::InvalidArgs("smooth is required (true or false)".into()));
        };
        if self.session.set_sharp_images(id, !smooth) {
            // Cached renderers baked the old setting into their config; page_render must re-read it.
            self.renderers.remove(&id);
        }
        Ok(json!({ "smooth": smooth }))
    }
}
