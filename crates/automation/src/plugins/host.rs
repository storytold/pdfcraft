//! The sandbox's view of the session: the tool table, pinned to one document.

use craft_plugin::{Access, Manifest};
use pdfcraft_engine::{DocId, EditMark};
use serde_json::{Value, json};

use super::{MAX_NOTE_CHARS, MAX_NOTES, MAX_RUN_EDITS, Runtime, refusal, set_setting, tool_named};
use crate::{Automation, Content};

/// Every call is checked by [`refusal`], pinned to the run's document and run through
/// [`Automation::call`], as the CLI, MCP or `command_run` would run it. A call from a read-only
/// plug-in that changes the document is put back and refused.
pub(super) struct ToolHost<'s> {
    a: &'s mut Automation,
    rt: &'s mut Runtime,
    manifest: &'s Manifest,
    doc: DocId,
    /// The document as the run found it.
    mark: &'s EditMark,
    /// 1-based pages the run was told are selected.
    selection: Vec<i64>,
    /// Calls that changed the document.
    edits: u32,
    notes: Vec<String>,
}

impl<'s> ToolHost<'s> {
    pub(super) fn new(
        a: &'s mut Automation,
        rt: &'s mut Runtime,
        manifest: &'s Manifest,
        doc: DocId,
        mark: &'s EditMark,
        selection: Vec<i64>,
    ) -> Self {
        ToolHost { a, rt, manifest, doc, mark, selection, edits: 0, notes: Vec::new() }
    }

    /// What the run asked to show the user.
    pub(super) fn finish(self) -> Vec<String> {
        self.notes
    }

    /// `params` for `tool`, pinned to the run's document: `doc` is filled in, and another
    /// document's id is refused.
    fn pinned(&self, tool: &crate::ToolDef, params: Value) -> Result<Value, String> {
        let mut params = match params {
            Value::Object(m) => m,
            Value::Null => serde_json::Map::new(),
            _ => return Err(format!("the params for `{}` must be a JSON object", tool.name)),
        };
        let takes_doc = tool.input_schema.get("properties").and_then(|p| p.get("doc")).is_some();
        match params.get("doc") {
            Some(Value::Null) | None if takes_doc => {
                params.insert("doc".into(), json!(self.doc.0));
            }
            Some(Value::Null) | None => {}
            Some(given) if given.as_u64() == Some(self.doc.0) => {}
            Some(_) => return Err(format!("plug-ins work on the document they run on (doc {}), not on others", self.doc.0)),
        }
        Ok(Value::Object(params))
    }
}

/// A tool's result as the plug-in reads it: its JSON, or a list of the JSON parts when there are
/// several. (No allowed tool returns an image.)
fn result_text(content: Vec<Content>) -> String {
    let mut parts: Vec<Value> = content
        .into_iter()
        .map(|c| match c {
            Content::Json(v) => v,
            Content::Png { width, height, .. } => json!({ "image": "png", "width": width, "height": height }),
        })
        .collect();
    if parts.len() == 1 { parts.remove(0).to_string() } else { Value::Array(parts).to_string() }
}

impl craft_plugin::Host for ToolHost<'_> {
    fn call(&mut self, command: &str, params: &str) -> Result<String, String> {
        let command = command.trim();
        let params: Value = serde_json::from_str(params).map_err(|e| format!("the params for `{command}` aren't JSON: {e}"))?;
        if let Some(why) = refusal(self.manifest, command, &params) {
            return Err(why);
        }
        let shown: String = command.chars().take(80).collect();
        let tool = tool_named(command).ok_or_else(|| format!("PdfCraft has no tool or command `{shown}` (see command_list)"))?;
        // A registry id names a tool that the allowlist also checks by name.
        if tool.name != command && super::policy().refuses(tool.name) {
            return Err(format!("`{shown}` isn't available to plugins"));
        }
        // The registry knows which tools change the document: a read-only plug-in can't run them.
        craft_plugin::check_read_only(self.manifest, command, !tool.read_only)?;
        if self.edits >= MAX_RUN_EDITS {
            return Err(format!("a plug-in run may make at most {MAX_RUN_EDITS} edits; split the work into several runs"));
        }
        let params = self.pinned(tool, params)?;
        let generation = self.a.session().get(self.doc).map(pdfcraft_engine::Document::edit_generation);
        let result = self.a.call(tool.name, &params);
        let changed = self.a.session().get(self.doc).map(pdfcraft_engine::Document::edit_generation) != generation;
        if changed && self.manifest.permissions.access == Access::Read {
            // A read-only plug-in kept no edit before this call, so the run's mark is this call's.
            let undone = self.a.session.restore_mark(self.mark);
            let why = craft_plugin::check_read_only(self.manifest, command, true).err().unwrap_or_else(|| format!("`{shown}` edits the document"));
            return Err(match undone {
                Ok(()) => why,
                Err(e) => format!("{why} (and it couldn't be put back: {e})"),
            });
        }
        if changed {
            self.edits = self.edits.saturating_add(1);
        }
        result.map(result_text).map_err(|e| e.to_string())
    }

    fn selection(&mut self) -> String {
        json!(self.selection).to_string()
    }

    fn notify(&mut self, message: &str) {
        if self.notes.len() < MAX_NOTES {
            self.notes.push(message.chars().take(MAX_NOTE_CHARS).collect());
        }
    }

    fn log(&mut self, line: &str) {
        // Also in the run's result (`logs`).
        log::info!("plug-in {}: {line}", self.manifest.id);
    }

    fn setting(&mut self, key: &str) -> Option<String> {
        Some(self.rt.prefs.settings.get(&self.manifest.id)?.get(key)?.to_string())
    }

    fn set_setting(&mut self, key: &str, value: Option<&str>) -> Result<(), String> {
        set_setting(&mut self.rt.prefs.settings, &self.manifest.id, key, value)
    }
}
