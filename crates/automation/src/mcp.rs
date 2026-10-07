//! A Model Context Protocol server over the automation tools.
//!
//! **Opt-in only.** Nothing in PdfCraft starts this server on its own: it runs when a user
//! launches `pdfcraft-cli mcp` (usually by adding that command to their agent's MCP
//! configuration), and stops when its input closes. It opens no network port; the transport is
//! newline-delimited JSON-RPC 2.0 over stdin/stdout.
//!
//! Implemented: `initialize`, `ping`, `tools/list`, `tools/call`, `resources/list`,
//! `resources/templates/list`, `resources/read`, and the `notifications/*` the client sends.
//! Resources expose the open documents read-only: `pdfcraft://doc/{doc}/info` (JSON),
//! `…/text` (plain text), `…/page/{page}/text` and `…/page/{page}/image` (PNG; `?dpi=` 1–600). Tool failures are reported in the result (`isError: true`) so the agent can read
//! them; protocol errors use JSON-RPC error codes.

use std::io::{BufRead, Write};

use base64::Engine as _;
use serde_json::{Value, json};

use crate::{Automation, Content, ToolError};

/// Protocol revisions we speak, newest first.
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

const INSTRUCTIONS: &str = "PdfCraft edits PDFs. Open a file with doc_open to get a document id, then inspect \
(doc_info, text_extract, text_find, page_render) or edit it (page_*, doc_set_info). Edits are undoable \
(edit_undo) and stay in memory until doc_save. Page numbers are 1-based. Open documents are also \
resources: pdfcraft://doc/{doc}/info, /text, /page/{page}/text and /page/{page}/image.";

pub struct McpServer {
    automation: Automation,
}

impl McpServer {
    pub fn new(automation: Automation) -> Self {
        Self { automation }
    }

    pub fn automation(&self) -> &Automation {
        &self.automation
    }

    /// Serve until `input` reaches end of file.
    pub fn serve(&mut self, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Some(reply) = self.handle_line(&line) {
                writeln!(output, "{reply}")?;
                output.flush()?;
            }
        }
        Ok(())
    }

    /// Handle one JSON-RPC message; returns the serialized reply, if one is due.
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let reply = match serde_json::from_str::<Value>(line) {
            Ok(msg) => self.handle(&msg),
            Err(e) => Some(error(Value::Null, PARSE_ERROR, &format!("parse error: {e}"))),
        };
        reply.map(|r| r.to_string())
    }

    /// Handle one parsed message. Notifications (no `id`) get no reply.
    pub fn handle(&mut self, msg: &Value) -> Option<Value> {
        let Some(obj) = msg.as_object() else { return Some(error(Value::Null, INVALID_REQUEST, "expected a JSON-RPC request object")) };
        let id = obj.get("id").cloned();
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            // A response to something we sent (we send no requests) or garbage.
            return id.map(|id| error(id, INVALID_REQUEST, "missing method"));
        };
        let params = obj.get("params").cloned().unwrap_or(Value::Null);
        let id = id?; // notifications/initialized, notifications/cancelled, …: nothing to answer
        Some(match self.dispatch(method, &params) {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => error(id, code, &message),
        })
    }

    fn dispatch(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        match method {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str);
                let version = asked.filter(|v| PROTOCOL_VERSIONS.contains(v)).unwrap_or(PROTOCOL_VERSIONS[0]);
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false }, "resources": { "listChanged": false, "subscribe": false } },
                    "serverInfo": { "name": "pdfcraft", "title": "PdfCraft", "version": env!("CARGO_PKG_VERSION"), "websiteUrl": pdfcraft_engine::links::APP_PAGE },
                    "instructions": INSTRUCTIONS,
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": crate::tools().iter().map(tool_json).collect::<Vec<_>>() })),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).ok_or((INVALID_PARAMS, "tools/call needs a tool name".to_string()))?;
                let args = params.get("arguments").cloned().unwrap_or(Value::Null);
                match self.automation.call(name, &args) {
                    Ok(content) => Ok(call_result(content)),
                    Err(ToolError::UnknownTool(t)) => Err((INVALID_PARAMS, format!("unknown tool {t:?}"))),
                    Err(e) => Ok(json!({ "content": [{ "type": "text", "text": e.to_string() }], "isError": true })),
                }
            }
            "resources/list" => Ok(json!({ "resources": self.resource_list() })),
            "resources/templates/list" => Ok(json!({ "resourceTemplates": resource_templates() })),
            "resources/read" => {
                let uri = params.get("uri").and_then(Value::as_str).ok_or((INVALID_PARAMS, "resources/read needs a uri".to_string()))?;
                self.read_resource(uri).map(|c| json!({ "contents": [c] }))
            }
            other => Err((METHOD_NOT_FOUND, format!("method not found: {other}"))),
        }
    }
}

/// The resource behind a `pdfcraft://` URI.
#[derive(Debug, PartialEq)]
enum Resource {
    Info(u64),
    Text(u64),
    PageText(u64, u64),
    PageImage(u64, u64, Option<f64>),
}

fn parse_uri(uri: &str) -> Option<Resource> {
    let rest = uri.strip_prefix("pdfcraft://doc/")?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let dpi = query.split('&').find_map(|kv| kv.strip_prefix("dpi=")).and_then(|v| v.parse::<f64>().ok());
    let parts: Vec<&str> = path.split('/').collect();
    let doc = parts.first()?.parse().ok()?;
    match parts[1..] {
        ["info"] => Some(Resource::Info(doc)),
        ["text"] => Some(Resource::Text(doc)),
        ["page", p, "text"] => Some(Resource::PageText(doc, p.parse().ok()?)),
        ["page", p, "image"] => Some(Resource::PageImage(doc, p.parse().ok()?, dpi)),
        _ => None,
    }
}

fn resource_templates() -> Value {
    json!([
        { "uriTemplate": "pdfcraft://doc/{doc}/info", "name": "Document information", "mimeType": "application/json",
          "description": "Metadata, pages, bookmarks, annotations, fields, links, layers, attachments, fonts and security of an open document (as doc_info)." },
        { "uriTemplate": "pdfcraft://doc/{doc}/text", "name": "Document text", "mimeType": "text/plain",
          "description": "The text of every page in reading order, each page under a \"Page n\" heading." },
        { "uriTemplate": "pdfcraft://doc/{doc}/page/{page}/text", "name": "Page text", "mimeType": "text/plain",
          "description": "The text of one page (1-based) in reading order." },
        { "uriTemplate": "pdfcraft://doc/{doc}/page/{page}/image{?dpi}", "name": "Page image", "mimeType": "image/png",
          "description": "One page (1-based) rendered to PNG; dpi 1–600, default 96." },
    ])
}

impl McpServer {
    fn resource_list(&mut self) -> Vec<Value> {
        let docs = self.automation.call("doc_list", &json!({})).ok().and_then(|c| match c.into_iter().next() {
            Some(Content::Json(v)) => v["documents"].as_array().cloned(),
            _ => None,
        });
        let mut out = Vec::new();
        for d in docs.unwrap_or_default() {
            let (id, name) = (d["doc"].as_u64().unwrap_or(0), d["name"].as_str().unwrap_or("document"));
            out.push(json!({ "uri": format!("pdfcraft://doc/{id}/info"), "name": format!("{name} (information)"), "mimeType": "application/json" }));
            out.push(json!({ "uri": format!("pdfcraft://doc/{id}/text"), "name": format!("{name} (text)"), "mimeType": "text/plain" }));
            // Pages are listed for short documents; longer ones use the templates.
            let pages = d["pages"].as_u64().unwrap_or(0);
            if pages <= 50 {
                for p in 1..=pages {
                    out.push(
                        json!({ "uri": format!("pdfcraft://doc/{id}/page/{p}/image"), "name": format!("{name}, page {p}"), "mimeType": "image/png" }),
                    );
                }
            }
        }
        out
    }

    fn read_resource(&mut self, uri: &str) -> Result<Value, (i64, String)> {
        let r = parse_uri(uri).ok_or((INVALID_PARAMS, format!("unknown resource {uri:?}")))?;
        let call = |a: &mut Automation, tool: &str, args: Value| a.call(tool, &args).map_err(|e| (INVALID_PARAMS, e.to_string()));
        let json_of = |c: Vec<Content>| c.into_iter().find_map(|c| if let Content::Json(v) = c { Some(v) } else { None }).unwrap_or(Value::Null);
        let text_of = |v: &Value, headings: bool| {
            let pages = v["pages"].as_array().cloned().unwrap_or_default();
            let parts: Vec<String> = pages
                .iter()
                .map(|p| {
                    let t = p["text"].as_str().unwrap_or("");
                    if headings { format!("Page {}\n{t}", p["page"]) } else { t.to_owned() }
                })
                .collect();
            parts.join("\n\n")
        };
        Ok(match r {
            Resource::Info(doc) => {
                let v = json_of(call(&mut self.automation, "doc_info", json!({ "doc": doc }))?);
                json!({ "uri": uri, "mimeType": "application/json", "text": serde_json::to_string_pretty(&v).unwrap_or_default() })
            }
            Resource::Text(doc) => {
                let v = json_of(call(&mut self.automation, "text_extract", json!({ "doc": doc }))?);
                json!({ "uri": uri, "mimeType": "text/plain", "text": text_of(&v, true) })
            }
            Resource::PageText(doc, page) => {
                let v = json_of(call(&mut self.automation, "text_extract", json!({ "doc": doc, "pages": [page] }))?);
                json!({ "uri": uri, "mimeType": "text/plain", "text": text_of(&v, false) })
            }
            Resource::PageImage(doc, page, dpi) => {
                let mut args = json!({ "doc": doc, "page": page });
                if let Some(d) = dpi {
                    args["dpi"] = json!(d);
                }
                let png = call(&mut self.automation, "page_render", args)?
                    .into_iter()
                    .find_map(|c| if let Content::Png { data, .. } = c { Some(data) } else { None });
                let png = png.ok_or((INVALID_PARAMS, "the page could not be rendered".to_string()))?;
                json!({ "uri": uri, "mimeType": "image/png", "blob": base64::engine::general_purpose::STANDARD.encode(png) })
            }
        })
    }
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tool_json(t: &crate::ToolDef) -> Value {
    json!({
        "name": t.name,
        "title": t.title,
        "description": t.description,
        "inputSchema": t.input_schema,
        "annotations": { "title": t.title, "readOnlyHint": t.read_only, "destructiveHint": t.destructive, "openWorldHint": false },
    })
}

fn call_result(content: Vec<Content>) -> Value {
    let mut structured = None;
    let blocks: Vec<Value> = content
        .into_iter()
        .map(|c| match c {
            Content::Json(v) => {
                let text = serde_json::to_string_pretty(&v).unwrap_or_default();
                if v.is_object() && structured.is_none() {
                    structured = Some(v);
                }
                json!({ "type": "text", "text": text })
            }
            Content::Png { data, .. } => {
                json!({ "type": "image", "mimeType": "image/png", "data": base64::engine::general_purpose::STANDARD.encode(data) })
            }
        })
        .collect();
    let mut result = json!({ "content": blocks, "isError": false });
    if let Some(s) = structured {
        result["structuredContent"] = s;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{Resource, parse_uri};

    #[test]
    fn resource_uris_parse() {
        assert_eq!(parse_uri("pdfcraft://doc/3/info"), Some(Resource::Info(3)));
        assert_eq!(parse_uri("pdfcraft://doc/3/page/2/image?dpi=36"), Some(Resource::PageImage(3, 2, Some(36.0))));
        assert_eq!(parse_uri("pdfcraft://doc/3/page/2/text"), Some(Resource::PageText(3, 2)));
        assert_eq!(parse_uri("pdfcraft://doc/x/info"), None);
        assert_eq!(parse_uri("file:///etc/passwd"), None);
    }
}
