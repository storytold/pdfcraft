//! Print production ▸ Rename ink as a tool (`doc_rename_ink`).

use pdfcraft_engine::Edit;
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError};

impl Automation {
    /// Every colour space naming spot ink `from` names `to` instead. No space naming `from` is
    /// not an error: `spaces` is 0 and no edit is made.
    pub(crate) fn rename_ink(&mut self, a: &Args) -> Result<Value> {
        let from = a.str("from")?.to_owned();
        let to = a.str("to")?.to_owned();
        if let Some(why) = pdfcraft_engine::ink_rename_refusal(&from, &to) {
            return Err(ToolError::InvalidArgs(why));
        }
        let id = self.doc(a)?.id;
        let found = self.session.count_ink_rename(id, &from, &to);
        let mut out = json!({
            "spaces": found.spaces,
            "objects": found.objects,
            "refused": found.refused,
            "merged": found.merged,
            "alternates_differ": found.alternates_differ,
            "complete": found.complete,
        });
        if found.spaces > 0 {
            let r = self.apply(a, Edit::RenameInk { from, to })?;
            out["undo"] = r["undo"].clone();
        }
        Ok(out)
    }
}
