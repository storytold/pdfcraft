//! Local email preparation only: this tool never starts a mail client or sends a message.

use crate::{Args, Automation, Result, failed};
use serde_json::Value;
#[cfg(not(target_arch = "wasm32"))]
use {crate::ToolError, serde_json::json};

impl Automation {
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn doc_email_draft(&mut self, _args: &Args) -> Result<Value> {
        Err(failed("email draft bundles require a native filesystem host"))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn doc_email_draft(&mut self, args: &Args) -> Result<Value> {
        let paths = args.strs("paths")?;
        if paths.is_empty() || paths.len() > pdfcraft_engine::email::MAX_FILES {
            return Err(ToolError::InvalidArgs("paths must list between 1 and 100 supported source files".into()));
        }
        let parent = self.resolve(args.str("out_dir")?, false)?;
        if !parent.is_dir() {
            return Err(ToolError::InvalidArgs("out_dir must be an existing directory".into()));
        }
        let sources = paths.into_iter().map(|path| {
            let path = self.resolve(path, false).map_err(|e| e.to_string())?;
            pdfcraft_engine::email::read_source(&path)
        });
        let draft = pdfcraft_engine::email::prepare(&self.session, sources).map_err(failed)?;
        let path = draft.save_in(&parent).map_err(failed)?;
        let directory = path.parent().ok_or_else(|| failed("draft directory is unavailable"))?;
        let attachments: Vec<_> = draft.attachment_names().map(|name| directory.join(name).to_string_lossy().into_owned()).collect();
        Ok(json!({"path": path.to_string_lossy(), "attachments": attachments, "sent": false}))
    }
}
