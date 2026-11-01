//! Connected mode: MCP tool calls that go to the open PdfCraft window instead of a session of
//! the server's own (`pdfcraft-cli mcp --connect FILE`).
//!
//! **Opt-in, like the rest.** The window answers only when it was started with `--control FILE`,
//! and only to a client that presents the token in that file (`pdfcraft-ui-egui`'s `control`).
//!
//! Both ends live here, so they can't drift apart:
//! - the window runs a table tool on its own session with [`call_for_window`] (its control
//!   method `tool.call {name, arguments?, root?}`), which refuses what only the person may do
//!   there ([`refused_in_window`]) and returns the result as JSON ([`to_wire`]);
//! - the MCP server relays every table tool and resource through a [`Window`], reads the result
//!   back with [`from_wire`], and adds the `ui_*` tools ([`ui_tools`]), which relay the window's
//!   own `ui.*` methods.

use std::path::Path;

use base64::Engine as _;
use pdfcraft_engine::Session;
use serde_json::{Value, json};

use crate::{Automation, Content};

/// The window's control method that runs a table tool on its documents.
pub const TOOL_METHOD: &str = "tool.call";

/// An open PdfCraft window, reached through its control channel.
pub trait Window {
    /// Call one control method: `Ok` is its result, `Err` says what went wrong in words an agent
    /// can act on (the window isn't running, the token was refused, the method failed).
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String>;
    /// Where the window is, for messages ("the PdfCraft window of control file …").
    fn describe(&self) -> String;
}

/// Run the table tool `name` on the window's `session` (see [`Automation::run_on`]), for the
/// control method [`TOOL_METHOD`]. Refused first: what [`refused_in_window`] lists.
pub fn call_for_window(session: &mut Session, root: Option<&Path>, name: &str, args: &Value) -> Result<Value, String> {
    if let Some(why) = refused_in_window(name, args) {
        return Err(why);
    }
    Automation::run_on(session, root, name, args).map(to_wire).map_err(|e| e.to_string())
}

/// Why the window refuses tool `name` with `args` from a connected client, if it does.
///
/// - **Signing is the person's act.** In the window an agent may prepare a document for
///   signature (a signature field with `form_add_field`, text or a date with `fill_sign_add`), but
///   it never signs, makes a digital ID or changes which certificates the window trusts:
///   all `sign_*` tools and `fill_sign_add` with a signature or
///   initials are refused. Applying redactions and exporting embedded signature images also stay with the person.
///   The person signs in the window, with their own ID and password, which
///   never go through the control channel.
/// - **Closing a tab is the person's decision**: `doc_close` is refused (it can discard unsaved
///   changes); File ▸ Close (`file.close`) asks first.
pub fn refused_in_window(name: &str, args: &Value) -> Option<String> {
    let signing = || {
        Some(format!(
            "{name}: signing in the PdfCraft window is for the person to do, not an agent. An agent can prepare the document (add a signature field with form_add_field, type text or a date with fill_sign_add); the person then signs in the window (Fill & Sign ▸ Sign, or Use a certificate) with their own ID, which never goes through the control channel"
        ))
    };
    match name {
        n if n.starts_with("sign_") => signing(),
        "fill_sign_add" if matches!(args.get("type").and_then(Value::as_str), Some("signature" | "initials")) => signing(),
        "redact_apply" => Some("redact_apply: irreversible redaction needs the person's confirmation in the window; an agent may prepare marks with redact_mark, then ask the person to review and apply them".into()),
        "comment_image_preview" => Some("comment_image_preview: an embedded signature image is private; inspect the rendered document instead of exporting its reusable image".into()),
        "doc_close" => Some(
            "doc_close: closing a tab of the PdfCraft window is the person's decision (it can discard unsaved changes); ask them to use File ▸ Close (file.close) in the app, which asks before discarding anything".into(),
        ),
        _ => None,
    }
}

/// Preflight a relay before sending any arguments to the window, so signing credentials in
/// a direct call or command wrapper never leave the MCP process. The window independently
/// enforces the same policy at every dispatch as a second boundary.
pub(crate) fn refused_relay(name: &str, args: &Value) -> Option<String> {
    if let Some(why) = refused_in_window(name, args) {
        return Some(why);
    }
    let command = |step: &Value| {
        let id = step.get("id").and_then(Value::as_str)?;
        let inner = crate::tools::tool_for_command(id)?;
        refused_in_window(inner, step.get("params").unwrap_or(&Value::Null))
    };
    match name {
        "command_run" => command(args),
        "command_batch" => args.get("steps").and_then(Value::as_array).and_then(|steps| steps.iter().find_map(command)),
        _ => None,
    }
}

/// A tool's result as the control channel carries it: `{"content": [...]}`, each piece
/// `{"type": "json", "value": …}` or `{"type": "png", "data": base64, "width", "height"}`.
pub fn to_wire(content: Vec<Content>) -> Value {
    let pieces: Vec<Value> = content
        .into_iter()
        .map(|c| match c {
            Content::Json(value) => json!({ "type": "json", "value": value }),
            Content::Png { data, width, height } => {
                json!({ "type": "png", "data": base64::engine::general_purpose::STANDARD.encode(data), "width": width, "height": height })
            }
        })
        .collect();
    json!({ "content": pieces })
}

/// Read a [`to_wire`] result back. Anything else (another version's format, garbage) is an
/// error, never a panic.
pub fn from_wire(v: &Value) -> Result<Vec<Content>, String> {
    let bad = |what: &str| format!("the PdfCraft window sent a tool result this version can't read ({what})");
    let pieces = v.get("content").and_then(Value::as_array).ok_or_else(|| bad("no content list"))?;
    pieces
        .iter()
        .map(|p| match p.get("type").and_then(Value::as_str) {
            Some("json") => Ok(Content::Json(p.get("value").cloned().unwrap_or(Value::Null))),
            Some("png") => {
                let data = p.get("data").and_then(Value::as_str).ok_or_else(|| bad("an image without data"))?;
                let data = base64::engine::general_purpose::STANDARD.decode(data).map_err(|_| bad("an image that isn't base64"))?;
                let dim = |k: &str| p.get(k).and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
                Ok(Content::Png { data, width: dim("width"), height: dim("height") })
            }
            _ => Err(bad("a piece of unknown type")),
        })
        .collect()
}

/// One `ui_*` tool: an MCP name for one of the window's `ui.*` methods.
struct UiTool {
    name: &'static str,
    method: &'static str,
    title: &'static str,
    description: &'static str,
    properties: fn() -> Value,
    required: &'static [&'static str],
    read_only: bool,
}

const UI_TOOLS: &[UiTool] = &[
    UiTool {
        name: "ui_state",
        method: "ui.state",
        title: "Window state",
        description: "What the PdfCraft window shows: its documents (each `doc` id is the one the other tools take), the active document with its page, zoom, selected pages and where pages are on screen, the mode, panels, dialog and notice.",
        properties: || json!({}),
        required: &[],
        read_only: true,
    },
    UiTool {
        name: "ui_inspect",
        method: "tool.inspect",
        title: "Find widgets in the window",
        description: "The window's widgets (from its accessibility tree) with id, role, label, value, rect in points and state. Filter by `query` (words in the label, value or description), `role` and `focused`; `limit` defaults to 500.",
        properties: || {
            json!({
                "query": { "type": "string" }, "role": { "type": "string" }, "focused": { "type": "boolean" },
                "limit": { "type": "integer", "minimum": 1 },
            })
        },
        required: &[],
        read_only: true,
    },
    UiTool {
        name: "ui_screenshot",
        method: "ui.screenshot",
        title: "Screenshot of the window",
        description: "A PNG of the PdfCraft window as the person sees it, optionally cropped to `region` [x0, y0, x1, y1] in points. Needs the window on screen: a hidden, minimized or locked one draws nothing.",
        properties: || json!({ "region": { "type": "array", "items": { "type": "number" }, "minItems": 4, "maxItems": 4 } }),
        required: &[],
        read_only: true,
    },
];

/// The `ui_*` tools in MCP's `tools/list` form.
pub(crate) fn ui_tools() -> Vec<Value> {
    UI_TOOLS
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "title": t.title,
                "description": t.description,
                "inputSchema": { "type": "object", "properties": (t.properties)(), "required": t.required, "additionalProperties": false },
                "annotations": { "title": t.title, "readOnlyHint": t.read_only, "destructiveHint": false, "idempotentHint": t.read_only, "openWorldHint": false },
            })
        })
        .collect()
}

/// The window method behind `ui_*` tool `name`, and the argument keys it takes.
pub(crate) fn ui_method(name: &str) -> Option<(&'static str, Vec<String>)> {
    UI_TOOLS
        .iter()
        .find(|t| t.name == name)
        .map(|t| (t.method, (t.properties)().as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-page PDF, built here.
    fn pdf() -> Vec<u8> {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] >>",
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    fn window_session() -> (Session, u64) {
        let mut s = Session::new();
        let id = s.open("form.pdf", None, std::sync::Arc::new(pdf()), None).unwrap();
        (s, id.0)
    }

    #[test]
    fn tools_run_on_the_windows_own_session() {
        let (mut s, doc) = window_session();
        let r = call_for_window(&mut s, None, "page_insert_blank", &json!({ "doc": doc, "at": 1 })).unwrap();
        assert!(from_wire(&r).is_ok(), "{r}");
        // The window's document changed, and the session came back with it.
        assert_eq!(s.get(pdfcraft_engine::DocId(doc)).unwrap().info.pages.len(), 2);
        let listed = from_wire(&call_for_window(&mut s, None, "doc_list", &json!({})).unwrap()).unwrap();
        let Some(Content::Json(v)) = listed.first() else { panic!("{listed:?}") };
        assert_eq!(v["documents"][0]["doc"], doc, "{v}");
        // A failing tool still hands the session back.
        let e = call_for_window(&mut s, None, "doc_info", &json!({ "doc": 99 })).unwrap_err();
        assert!(e.contains("99"), "{e}");
        assert_eq!(s.docs().len(), 1);
    }

    #[test]
    fn the_window_refuses_signing_and_closing_tabs() {
        let (mut s, doc) = window_session();
        for (name, args) in [
            ("sign_document", json!({ "doc": doc })),
            ("sign_id_create", json!({ "name": "A", "path": "id.p12", "password": "x" })),
            ("sign_trust", json!({ "clear": true })),
            ("fill_sign_add", json!({ "doc": doc, "page": 1, "type": "signature", "at": [10, 10], "text": "A" })),
            ("fill_sign_add", json!({ "doc": doc, "page": 1, "type": "initials", "at": [10, 10], "text": "A" })),
        ] {
            let e = call_for_window(&mut s, None, name, &args).unwrap_err();
            assert!(e.contains("for the person to do") && e.contains("form_add_field"), "{name}: {e}");
        }
        let e = call_for_window(&mut s, None, "doc_close", &json!({ "doc": doc, "discard_changes": true })).unwrap_err();
        assert!(e.contains("file.close"), "{e}");
        assert_eq!(s.docs().len(), 1, "the tab stays open");
        // Preparing a document for signature is allowed: typed text and dates.
        let r = call_for_window(&mut s, None, "fill_sign_add", &json!({ "doc": doc, "page": 1, "type": "text", "at": [10, 10], "text": "Name" }));
        assert!(r.is_ok(), "{r:?}");
    }

    #[test]
    fn nested_commands_cannot_sign_or_close_the_window() {
        let (mut s, doc) = window_session();
        let signature =
            json!({ "id": "sign.fill.text", "params": { "doc": doc, "page": 1, "type": "signature", "at": [10, 10], "text": "Synthetic" } });
        for step in [signature, json!({"id":"file.close", "params":{"doc":doc,"discard_changes":true}})] {
            let e = call_for_window(&mut s, None, "command_run", &step).unwrap_err();
            assert!(e.contains("person"), "{e}");
            let r = call_for_window(&mut s, None, "command_batch", &json!({"steps":[step],"stop_on_error":false})).unwrap();
            assert_eq!(r["content"][0]["value"]["failed"], 1, "{r}");
            assert!(r["content"][0]["value"]["results"][0]["error"].as_str().unwrap().contains("person"), "{r}");
        }
        for name in ["sign_list", "sign_keychain_ids", "sign_windows_ids", "sign_unknown", "redact_apply", "comment_image_preview"] {
            assert!(call_for_window(&mut s, None, name, &json!({"doc":doc})).is_err(), "{name}");
        }
        assert_eq!(s.docs().len(), 1);
        assert!(!s.docs()[0].dirty, "no signature or other edit was applied");
    }

    #[test]
    fn a_root_confines_the_windows_tools_too() {
        let (mut s, _) = window_session();
        let root = std::env::temp_dir().join(format!("pdfcraft-window-root-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let e = call_for_window(&mut s, Some(&root), "doc_open", &json!({ "path": "../outside.pdf" })).unwrap_err();
        assert!(e.contains("is outside the allowed directory"), "the error must prove confinement, not just name outside.pdf: {e}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn wire_round_trip_and_garbage() {
        let content = vec![Content::Json(json!({ "a": 1 })), Content::Png { data: vec![1, 2, 3], width: 4, height: 5 }];
        assert_eq!(from_wire(&to_wire(content.clone())).unwrap(), content);
        for junk in
            [json!(null), json!({ "content": 3 }), json!({ "content": [{ "type": "gif" }] }), json!({ "content": [{ "type": "png", "data": "%%" }] })]
        {
            assert!(from_wire(&junk).is_err(), "{junk}");
        }
    }

    #[test]
    fn ui_tools_name_real_methods_and_close_their_schemas() {
        let tools = ui_tools();
        assert_eq!(tools.len(), 3);
        for t in &tools {
            let name = t["name"].as_str().unwrap();
            assert!(name.starts_with("ui_"), "{name}");
            assert_eq!(t["inputSchema"]["additionalProperties"], false, "{name}");
            let (method, _) = ui_method(name).unwrap();
            assert_eq!(method, if name == "ui_inspect" { "tool.inspect".into() } else { format!("ui.{}", name.trim_start_matches("ui_")) });
            assert!(crate::tools::find(name).is_none(), "{name} clashes with a table tool");
        }
    }
}
