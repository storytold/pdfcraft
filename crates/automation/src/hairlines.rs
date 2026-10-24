//! Print production ▸ Fix hairlines as a tool: strokes narrower than a threshold on the page get a
//! width (`doc_fix_hairlines`).

use pdfcraft_engine::{Edit, Hairlines};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError};

impl Automation {
    /// Strokes thinner than `narrower_than` points on the page (after every transformation;
    /// zero-width lines always) are given `width` points; Type 3 glyphs and tiling patterns on
    /// request. Nothing to fix is not an error: `fixed` is 0 and no edit is made.
    pub(crate) fn fix_hairlines(&mut self, a: &Args) -> Result<Value> {
        let n = self.doc(a)?.info.pages.len();
        let pages = match a.opt_ints("pages")? {
            Some(_) => self.pages(a, "pages")?,
            None => (0..n).collect(),
        };
        let settings = Hairlines {
            narrower_than: a.opt_num("narrower_than")?.unwrap_or(0.003 * 72.0),
            width: a.opt_num("width")?.unwrap_or(0.01 * 72.0),
            type3: a.opt_bool("type3")?.unwrap_or(false),
            patterns: a.opt_bool("patterns")?.unwrap_or(false),
        };
        if !(settings.narrower_than >= 0.0 && settings.width > 0.0) {
            return Err(ToolError::InvalidArgs("narrower_than must be 0 or more and width above 0 (points)".into()));
        }
        let id = self.doc(a)?.id;
        let found = self.session.count_hairlines(id, &pages, &settings);
        if found.lines == 0 {
            return Ok(json!({ "fixed": 0, "pages": 0 }));
        }
        let r = self.apply(a, Edit::FixHairlines { pages, settings })?;
        Ok(json!({ "fixed": found.lines, "pages": found.pages, "undo": r["undo"] }))
    }
}
