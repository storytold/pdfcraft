//! Print production ▸ Rename ink as a tool (`doc_rename_ink`).

use pdfcraft_engine::Edit;
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError, error::failed};

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
        let found = self.session.count_ink_rename(id, &from, &to).map_err(failed)?;
        let counts = json!({
            "spaces": found.spaces,
            "objects": found.objects,
            "refused": found.refused,
            "merged": found.merged,
            "alternates_differ": found.alternates_differ,
            "complete": found.complete,
        });
        if found.spaces == 0 {
            return Ok(counts);
        }
        let mut out = self.apply(a, Edit::RenameInk { from, to })?;
        if let (Some(out), Value::Object(counts)) = (out.as_object_mut(), counts) {
            out.extend(counts);
        }
        Ok(out)
    }
}
