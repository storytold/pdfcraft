//! Reading colours: Acrobat's Replace Document Colors as a tool (#897, AGENTS.md §3).
//!
//! A per-document view setting: solid text and line-art paints remap for readability and the
//! page clears to the setting's background when rendering. The file itself never changes.

use pdfcraft_render::{InvertPalette, ReadingColors};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError};

/// The inverted view's modes, for error messages.
const INVERT_MODES: &str = "keep_images, everything";

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

    /// Toggle or set the document's inverted view (View ▸ Invert colors) and, with it, its mode
    /// and palette. A view setting: the file doesn't change. While it is on it replaces the
    /// reading colours, which never stack on it. Omitted arguments keep what the document has.
    pub(crate) fn view_invert_colors(&mut self, a: &Args) -> Result<Value> {
        let id = self.doc(a)?.id;
        let current = self.session.invert_colors(id).unwrap_or(false);
        // Only a bare call toggles; choosing a mode or palette keeps the view as it is.
        let choosing = a.opt_str("mode")?.is_some() || a.opt_str("palette")?.is_some();
        let invert = a.opt_bool("invert")?.unwrap_or(if choosing { current } else { !current });
        let keep = match a.opt_str("mode")? {
            None => self.session.invert_keep_images(id).unwrap_or(true),
            Some("keep_images") => true,
            Some("everything") => false,
            Some(other) => return Err(ToolError::InvalidArgs(format!("unknown mode {other:?}; expected one of: {INVERT_MODES}"))),
        };
        let palette = match a.opt_str("palette")? {
            Some(name) => InvertPalette::parse(name, a.opt_str("paper")?, a.opt_str("ink")?).map_err(ToolError::InvalidArgs)?,
            None if a.opt_str("paper")?.is_some() || a.opt_str("ink")?.is_some() => {
                return Err(ToolError::InvalidArgs("paper and ink need palette \"custom\"".into()));
            }
            None => self.session.invert_palette(id).unwrap_or_default(),
        };
        let changed =
            [self.session.set_invert_palette(id, palette), self.session.set_invert_keep_images(id, keep), self.session.set_invert_colors(id, invert)];
        if changed.contains(&true) {
            // Cached renderers baked the old setting into their config; page_render must re-read it.
            self.renderers.remove(&id);
        }
        Ok(json!({
            "doc": id.0,
            "invert": invert,
            "mode": if keep { "keep_images" } else { "everything" },
            "palette": palette.name(),
            "paper": palette.paper_hex(),
            "ink": palette.ink_hex(),
        }))
    }
}
