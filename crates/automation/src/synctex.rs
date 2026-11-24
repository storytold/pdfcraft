//! SyncTeX search tools: thin adapters over the engine's lookups.
use std::path::Path;

use crate::{Args, Automation, Result, ToolError, failed};
use serde_json::{Value, json};

impl Automation {
    /// The SyncTeX file the call names, or else the one beside the PDF, held to the root like any
    /// other path a tool reads: a link beside the PDF that leads outside the root is refused, not
    /// followed. `None` when the call names none and none is beside the PDF; the lookup then says
    /// where it looked.
    fn synctex_file(&self, a: &Args) -> Result<Option<std::path::PathBuf>> {
        if let Some(named) = a.opt_str("synctex")? {
            return self.resolve(named, false).map(Some);
        }
        let beside = self.doc(a)?.synctex_beside();
        let Some(found) = beside.iter().find(|p| p.is_file()) else { return Ok(None) };
        self.resolve(&found.to_string_lossy(), false).map(Some)
    }

    pub(crate) fn synctex_inverse(&self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let at = a.nums::<2>("at")?.ok_or_else(|| ToolError::InvalidArgs("synctex_inverse needs `at`".into()))?;
        let file = self.synctex_file(a)?;
        let pos = self.doc(a)?.synctex_inverse(page, at, file.as_deref()).map_err(|e| failed(e.to_string()))?;
        Ok(json!({"file": pos.file.to_string_lossy(), "line": pos.line, "column": pos.column}))
    }

    pub(crate) fn synctex_forward(&self, a: &Args) -> Result<Value> {
        let source = a.str("file")?;
        let line = u32::try_from(a.int("line")?).ok().filter(|l| *l >= 1).ok_or_else(|| ToolError::InvalidArgs("line must be 1 or more".into()))?;
        let file = self.synctex_file(a)?;
        let (page, rects) = self.doc(a)?.synctex_forward(Path::new(source), line, file.as_deref()).map_err(|e| failed(e.to_string()))?;
        Ok(json!({"page": page + 1, "rects": rects}))
    }
}
