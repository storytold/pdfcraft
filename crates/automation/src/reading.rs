//! Reading colours: Acrobat's Replace Document Colors as a tool (#897, AGENTS.md §3).
//!
//! A per-document view setting: solid text and line-art paints remap for readability and the
//! page clears to the setting's background when rendering. The file itself never changes.

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

    /// Toggle or set color inversion on the document.
    pub(crate) fn view_invert_colors(&mut self, a: &Args) -> Result<Value> {
        let id = self.doc(a)?.id;
        let current = self.session.invert_colors(id).unwrap_or(false);
        let next = a.opt_bool("invert")?.unwrap_or(!current);
        if self.session.set_invert_colors(id, next) {
            self.renderers.remove(&id);
        }
        Ok(json!({
            "doc": id.0,
            "invert": next,
        }))
    }
}
