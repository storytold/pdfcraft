//! `geometry FILE`: pages' line segments as one JSON document, from the automation tool
//! `page_geometry`, so the command and the tool give the same answer.
use serde_json::{Map, Value, json};

use super::commands::page_list;
use super::options::{GEOMETRY_OPTIONS, flag, positional, validate_options};
use super::{CliError, stdout_line};

/// The limit options, and the `page_geometry` argument each sets.
const LIMITS: [(&str, &str); 3] = [("--max-segments", "max_segments"), ("--max-targets", "max_targets"), ("--max-bytes", "max_bytes")];

/// The `page_geometry` arguments the command line asks for (without the document).
fn tool_args(args: &[String]) -> Result<Map<String, Value>, String> {
    let mut tool = Map::new();
    if let Some(pages) = flag(args, "--pages") {
        let pages = page_list(pages).map_err(|e| format!("geometry: {e}"))?;
        tool.insert("pages".into(), json!(pages.iter().map(|p| p + 1).collect::<Vec<_>>()));
    }
    if let Some(coords) = flag(args, "--coords") {
        tool.insert("coords".into(), json!(coords));
    }
    for (option, key) in LIMITS {
        if let Some(v) = flag(args, option) {
            let n: u64 = v.parse().map_err(|_| format!("geometry: bad {option} {v:?}: expected a whole number"))?;
            tool.insert(key.into(), json!(n));
        }
    }
    Ok(tool)
}

/// The JSON a tool returned.
fn json_of(content: Vec<pdfcraft_automation::Content>) -> Option<Value> {
    content.into_iter().find_map(|c| match c {
        pdfcraft_automation::Content::Json(v) => Some(v),
        pdfcraft_automation::Content::Png { .. } => None,
    })
}

pub(crate) fn geometry(args: &[String]) -> Result<(), CliError> {
    validate_options("geometry", args, GEOMETRY_OPTIONS)?;
    let path = *positional(args, GEOMETRY_OPTIONS).first().ok_or("geometry: missing file")?;
    let mut tool = tool_args(args)?;
    let mut auto = pdfcraft_automation::Automation::new();
    let mut open = json!({ "path": path });
    if let Some(password) = flag(args, "--password") {
        open["password"] = json!(password);
    }
    let opened = auto.call("doc_open", &open).map_err(|e| format!("geometry: {e}"))?;
    let doc = json_of(opened).and_then(|v| v["doc"].as_u64()).ok_or("geometry: the document did not open")?;
    tool.insert("doc".into(), json!(doc));
    let result = auto.call("page_geometry", &Value::Object(tool)).map_err(|e| format!("geometry: {e}"))?;
    let mut result = json_of(result).ok_or("geometry: no result")?;
    // `as_object_mut`, not `result["file"] =`, which panics on anything but an object.
    if let Some(object) = result.as_object_mut() {
        object.insert("file".into(), json!(path));
    }
    // One line: a heavy sheet lists millions of segments.
    stdout_line(format_args!("{}", serde_json::to_string(&result).unwrap_or_default()))
}
