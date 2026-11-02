//! Layers (optional content groups) as a tool: the Layers panel's eye icon (#2986, AGENTS.md §3).
//!
//! A per-document view setting, like hidden comments: `page_render` draws with it, and the file
//! itself never changes.

use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError};

impl Automation {
    /// Show or hide a layer, named by `layer`: its name as `doc_info` lists it, or its 1-based
    /// position in that list. A name picks every layer that has it, since CAD exporters often
    /// write one group per page or per drawing under the same name.
    pub(crate) fn layer_set(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let id = doc.id;
        let visible = a.opt_bool("visible")?.ok_or_else(|| ToolError::InvalidArgs("missing argument visible".into()))?;
        let layers = &doc.info.layers;
        let targets: Vec<usize> = match a.get("layer") {
            Some(Value::String(name)) => layers.iter().enumerate().filter(|(_, l)| l.name == *name).map(|(i, _)| i).collect(),
            Some(v) => match v.as_u64() {
                Some(n) if n >= 1 && usize::try_from(n - 1).is_ok_and(|i| i < layers.len()) => vec![(n - 1) as usize],
                Some(_) => Vec::new(),
                None => return Err(ToolError::InvalidArgs("layer must be a layer name or its 1-based position in doc_info's layers".into())),
            },
            None => return Err(ToolError::InvalidArgs("missing argument layer".into())),
        };
        if targets.is_empty() {
            let names: Vec<&str> = layers.iter().map(|l| l.name.as_str()).collect();
            return Err(ToolError::InvalidArgs(if names.is_empty() {
                "the document has no layers".into()
            } else {
                format!("no such layer; the document's layers are: {}", names.join(", "))
            }));
        }
        let mut changed = false;
        for i in targets {
            changed |= self.session.set_layer_visible(id, i, visible);
        }
        if changed {
            // The cached renderer drew with the old visibility; page_render must read it again.
            self.renderers.remove(&id);
        }
        let layers = &self.doc(a)?.info.layers;
        Ok(json!({ "layers": layers.iter().map(|l| json!({ "name": l.name, "visible": l.visible })).collect::<Vec<_>>() }))
    }
}
