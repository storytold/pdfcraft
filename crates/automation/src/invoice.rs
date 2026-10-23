//! Read-only draft preview through the engine facade; no file access or PDF mutation.

use serde_json::Value;

use crate::{Args, Automation, Result, ToolError, failed};

/// Bound work and copies before deserializing a caller-owned JSON tree.
/// The tool accepts at most 1000 array entries, 10,000 nodes, 8 levels and
/// 1024 UTF-8 bytes per string. These limits also apply to unknown fields.
fn bounded(value: &Value, nodes: &mut usize, depth: usize) -> bool {
    if *nodes >= 10_000 || depth > 8 {
        return false;
    }
    *nodes += 1;
    match value {
        Value::String(text) => text.len() <= pdfcraft_engine::invoice::MAX_TEXT_BYTES,
        Value::Array(items) => items.len() <= pdfcraft_engine::invoice::MAX_LINES && items.iter().all(|v| bounded(v, nodes, depth + 1)),
        Value::Object(fields) => fields.len() <= 32 && fields.iter().all(|(key, v)| key.len() <= 64 && bounded(v, nodes, depth + 1)),
        _ => true,
    }
}

impl Automation {
    /// Bound and decode the draft, then serialize the engine's immutable arithmetic report.
    /// Malformed or oversized JSON is an argument error; semantic failures remain field issues.
    /// This handler grants no file or document access and performs no standards validation.
    pub(crate) fn invoice_calculate(&self, args: &Args) -> Result<Value> {
        let raw = args.get("draft").ok_or_else(|| ToolError::InvalidArgs("draft is required".into()))?;
        if !bounded(raw, &mut 0, 0) {
            return Err(ToolError::InvalidArgs(
                "draft exceeds limits: 1000 array entries, 10000 nodes, 8 levels, 32 fields per object, 1024 UTF-8 bytes per string".into(),
            ));
        }
        let draft = serde_json::from_value::<pdfcraft_engine::invoice::Draft>(raw.clone())
            .map_err(|e| ToolError::InvalidArgs(format!("invalid invoice draft: {e}")))?;
        serde_json::to_value(pdfcraft_engine::invoice::calculate(&draft)).map_err(failed)
    }
}
