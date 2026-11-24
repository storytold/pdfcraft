//! SyncTeX search tools: thin adapters over the engine's lookups.
use std::path::Path;

use crate::{Args, Automation, Result, ToolError, failed, file_access};
use pdfcraft_engine::synctex::SyncTexError;
use serde_json::{Value, json};

impl Automation {
    /// The SyncTeX file the call names, or else the one beside the PDF, held to the root like any
    /// other path a tool reads. Each place beside the PDF is checked against the root before it is
    /// looked at, so a link there that leads outside is refused whether or not its target exists.
    /// With no file in either place the call fails here and says where it looked: the engine is
    /// never left to look beside the PDF itself, past the root.
    fn synctex_file(&self, a: &Args) -> Result<std::path::PathBuf> {
        if let Some(named) = a.opt_str("synctex")? {
            return self.resolve(named, false);
        }
        let beside = self.doc(a)?.synctex_beside();
        for place in &beside {
            let held = self.resolve_for(&place.to_string_lossy(), file_access::Access::Read, true)?;
            if held.is_file() {
                return Ok(held);
            }
        }
        Err(failed(SyncTexError::NoFile { looked: beside }.to_string()))
    }

    pub(crate) fn synctex_inverse(&self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let at = a.nums::<2>("at")?.ok_or_else(|| ToolError::InvalidArgs("synctex_inverse needs `at`".into()))?;
        let file = self.synctex_file(a)?;
        let pos = self.doc(a)?.synctex_inverse(page, at, Some(&file)).map_err(|e| failed(e.to_string()))?;
        Ok(json!({"file": pos.file.to_string_lossy(), "line": pos.line, "column": pos.column}))
    }

    pub(crate) fn synctex_forward(&self, a: &Args) -> Result<Value> {
        let source = a.str("file")?;
        let line = u32::try_from(a.int("line")?).ok().filter(|l| *l >= 1).ok_or_else(|| ToolError::InvalidArgs("line must be 1 or more".into()))?;
        let file = self.synctex_file(a)?;
        let (page, rects) = self.doc(a)?.synctex_forward(Path::new(source), line, Some(&file)).map_err(|e| failed(e.to_string()))?;
        Ok(json!({"page": page + 1, "rects": rects}))
    }
}
