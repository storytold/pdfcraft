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
resources: pdfcraft://doc/{doc}/info, /text, /page/{page}/text and /page/{page}/image. \
command_list discovers command ids and tool schemas; command_run executes one, command_batch runs several. \
doc_inspect returns open document state; render_preview returns a bounded PNG.";

/// Appended to the instructions in compact mode.
const COMPACT_INSTRUCTIONS: &str = " Only a core set of tools is listed. Every other tool is still available: find it with \
tool_search (optional query and category, or name for its full input schema) and run it with tool_call.";

/// The tools `tools/list` returns in compact mode, besides the two meta tools.
pub const COMPACT_CORE_TOOLS: &[&str] = &[
    "doc_open",
    "doc_info",
    "doc_save",
    "doc_close",
    "page_render",
    "text_extract",
    "text_find",
    "doc_combine",
    "doc_split",
    "edit_undo",
    "command_list",
    "command_run",
    "command_batch",
    "doc_inspect",
    "render_preview",
];

/// Meta tools that exist only in compact mode.
const TOOL_SEARCH: &str = "tool_search";
const TOOL_CALL: &str = "tool_call";

pub struct McpServer {
    automation: Automation,
    compact: bool,
}

impl McpServer {
    pub fn new(automation: Automation) -> Self {
        Self { automation, compact: false }
    }

    /// In compact mode `tools/list` returns only [`COMPACT_CORE_TOOLS`] plus `tool_search` and
    /// `tool_call`, which find and run any other tool. `tools/call` accepts every tool by name
    /// either way. Off by default.
    pub fn with_compact(mut self, on: bool) -> Self {
        self.compact = on;
        self
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
        let result = if method == "tools/call" { guarded_tool(|| self.dispatch(method, &params)) } else { self.dispatch(method, &params) };
        Some(match result {
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
                    "instructions": if self.compact { format!("{INSTRUCTIONS}{COMPACT_INSTRUCTIONS}") } else { INSTRUCTIONS.to_string() },
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": self.tool_list() })),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).ok_or((INVALID_PARAMS, "tools/call needs a tool name".to_string()))?;
                let args = params.get("arguments").cloned().unwrap_or(Value::Null);
                if self.compact && name == TOOL_SEARCH {
                    check_keys(name, &args, &["query", "category", "name"]).map_err(|e| (INVALID_PARAMS, e))?;
                    return Ok(match search_tools(&args) {
                        Ok(found) => call_result(vec![Content::Json(found)]),
                        Err(message) => tool_error(&message),
                    });
                }
                if self.compact && name == TOOL_CALL {
                    check_keys(name, &args, &["name", "arguments"]).map_err(|e| (INVALID_PARAMS, e))?;
                    let (inner, inner_args) = match unwrap_tool_call(&args) {
                        Ok(pair) => pair,
                        Err(message) => return Ok(tool_error(&message)),
                    };
                    return Ok(match self.automation.call(&inner, &inner_args) {
                        Ok(content) => tool_content(&inner, content),
                        Err(ToolError::UnknownTool(t)) => tool_error(&format!("unknown tool {t:?}; use tool_search to find the right name")),
                        Err(e) => tool_error(&e.to_string()),
                    });
                }
                if let Some(def) = crate::tools::find(name) {
                    let keys: Vec<&str> = def
                        .input_schema
                        .get("properties")
                        .and_then(Value::as_object)
                        .map(|p| p.keys().map(String::as_str).collect())
                        .unwrap_or_default();
                    check_keys(name, &args, &keys).map_err(|e| (INVALID_PARAMS, e))?;
                }
                match self.automation.call(name, &args) {
                    Ok(content) => Ok(tool_content(name, content)),
                    Err(ToolError::UnknownTool(t)) => Err((INVALID_PARAMS, format!("unknown tool {t:?}"))),
                    Err(e) => Ok(tool_error(&e.to_string())),
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

impl McpServer {
    /// What `tools/list` returns: every tool, or in compact mode the core set and the meta tools.
    fn tool_list(&self) -> Vec<Value> {
        let all = crate::tools();
        if !self.compact {
            return all.iter().map(tool_json).collect();
        }
        let mut out: Vec<Value> = COMPACT_CORE_TOOLS.iter().filter_map(|n| all.iter().find(|t| t.name == *n)).map(tool_json).collect();
        out.extend(meta_tools());
        out
    }
}

/// The two tools only compact mode lists.
fn meta_tools() -> [Value; 2] {
    [
        json!({
            "name": TOOL_SEARCH,
            "title": "Find PdfCraft tools",
            "description": "Search and list all PdfCraft automation tools, including the core tools, with a one-sentence description each. Filter by query \
        (words matched against name, title and description) and/or category (the name prefix: doc, page, text, form, …). \
        With name, return that one tool in full, including its input_schema. Run a tool with tool_call.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Words to look for, all of which must match (case-insensitive)." },
                    "category": { "type": "string", "description": "Tool name prefix, e.g. page, text, form, redact." },
                    "name": { "type": "string", "description": "Exact tool name: return its description and input_schema." },
                },
                "required": [],
                "additionalProperties": false,
            },
            "annotations": { "title": "Find PdfCraft tools", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
        }),
        json!({
            "name": TOOL_CALL,
            "title": "Run any PdfCraft tool",
            "description": "Run any PdfCraft tool by name with its arguments, exactly as if it were called directly. \
        Find names and argument schemas with tool_search. The result is the tool's own result.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "The tool to run, e.g. page_rotate." },
                    "arguments": { "type": "object", "description": "The tool's arguments (see its input_schema from tool_search)." },
                },
                "required": ["name"],
                "additionalProperties": false,
            },
            "annotations": { "title": "Run any PdfCraft tool", "readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": false },
        }),
    ]
}

/// The category of a tool: its name up to the first underscore (`page_rotate` → `page`).
fn category_of(name: &str) -> &str {
    name.split('_').next().unwrap_or(name)
}

/// A description up to and including its first sentence.
fn first_sentence(text: &str) -> &str {
    // ASCII lowercasing keeps byte offsets, so positions found in `lower` are valid in `text`.
    let lower = text.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower.get(from..).and_then(|rest| rest.find(". ")).map(|i| i + from) {
        // "e.g. " and "i.e. " are abbreviations, not the end of the sentence.
        if !lower.get(..i).is_some_and(|before| before.ends_with("e.g") || before.ends_with("i.e")) {
            return text.get(..=i).unwrap_or(text);
        }
        from = i + 2;
    }
    text
}

/// Reject keys other than `allowed`, as the tool table does for a tool's own arguments.
fn check_keys(tool: &str, args: &Value, allowed: &[&str]) -> Result<(), String> {
    match args.as_object().and_then(|o| o.keys().find(|k| !allowed.contains(&k.as_str()))) {
        Some(k) => Err(format!("{tool}: unknown argument {k:?} (expected: {})", allowed.join(", "))),
        None => Ok(()),
    }
}

/// `tool_search`: filter the tool table by query and category, or describe one tool in full.
fn search_tools(args: &Value) -> Result<Value, String> {
    if !(args.is_object() || args.is_null()) {
        return Err(format!("{TOOL_SEARCH} takes an object with optional query, category and name"));
    }
    check_keys(TOOL_SEARCH, args, &["query", "category", "name"])?;
    let text = |key: &str| match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.trim())),
        Some(_) => Err(format!("{TOOL_SEARCH}: {key} must be a string")),
    };
    let (query, category, name) = (text("query")?, text("category")?, text("name")?);
    let all = crate::tools();
    if let Some(name) = name {
        let t =
            all.iter().find(|t| t.name == name).ok_or_else(|| format!("unknown tool {name:?}; call {TOOL_SEARCH} without name to list the tools"))?;
        return Ok(json!({ "tool": {
            "name": t.name, "title": t.title, "category": category_of(t.name), "description": t.description,
            "read_only": t.read_only, "destructive": t.destructive, "input_schema": t.input_schema,
        } }));
    }
    let category = category.map(|c| c.trim_end_matches('_').to_lowercase()).filter(|c| !c.is_empty());
    let words: Vec<String> = query.map(|q| q.to_lowercase().split_whitespace().map(str::to_owned).collect()).unwrap_or_default();
    let mut categories: Vec<&str> = all.iter().map(|t| category_of(t.name)).collect();
    categories.sort_unstable();
    categories.dedup();
    let found: Vec<Value> = all
        .iter()
        .filter(|t| category.as_deref().is_none_or(|c| category_of(t.name) == c))
        .filter(|t| {
            let haystack = format!("{} {} {}", t.name, t.title, t.description).to_lowercase();
            words.iter().all(|w| haystack.contains(w.as_str()))
        })
        .map(|t| json!({ "name": t.name, "category": category_of(t.name), "description": first_sentence(t.description), "read_only": t.read_only }))
        .collect();
    Ok(json!({ "count": found.len(), "tools": found, "categories": categories }))
}

/// `tool_call`: the tool name and its arguments from the meta tool's own arguments.
fn unwrap_tool_call(args: &Value) -> Result<(String, Value), String> {
    let Some(obj) = args.as_object() else { return Err(format!("{TOOL_CALL} needs an object with name and arguments")) };
    check_keys(TOOL_CALL, args, &["name", "arguments"])?;
    let name = obj
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{TOOL_CALL} needs the name of the tool to run; find it with {TOOL_SEARCH}"))?;
    if name == TOOL_CALL || name == TOOL_SEARCH {
        return Err(format!("{TOOL_SEARCH} and {TOOL_CALL} are called directly, not through {TOOL_CALL}"));
    }
    let inner = match obj.get("arguments") {
        None | Some(Value::Null) => Value::Null,
        Some(v @ Value::Object(_)) => v.clone(),
        Some(_) => return Err(format!("{TOOL_CALL}: arguments must be an object")),
    };
    Ok((name.to_owned(), inner))
}

/// Last-resort boundary shared by every tools/call route, including compact dispatch.
fn guarded_tool(f: impl FnOnce() -> Result<Value, (i64, String)>) -> Result<Value, (i64, String)> {
    pdfcraft_engine::guard(f).unwrap_or_else(|message| Ok(tool_error(&format!("internal error: {message}"))))
}

fn tool_content(name: &str, content: Vec<Content>) -> Value {
    let failed = name == "command_batch"
        && content.iter().any(|c| match c {
            Content::Json(v) => v.get("failed").and_then(Value::as_u64).is_some_and(|n| n > 0),
            _ => false,
        });
    let mut result = call_result(content);
    if let Some(object) = result.as_object_mut() {
        object.insert("isError".into(), json!(failed));
    }
    result
}

/// A failed tool call, as the agent should read it.
fn tool_error(message: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
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
        out.extend([
            json!({"uri":"pdfcraft://document","name":"Open documents","mimeType":"application/json","description":"Session document state, as doc_inspect without arguments."}),
            json!({"uri":"pdfcraft://commands","name":"Command catalog","mimeType":"application/json","description":"Commands and mapped tool schemas, as command_list without arguments."}),
        ]);
        out
    }

    fn read_resource(&mut self, uri: &str) -> Result<Value, (i64, String)> {
        if let Some(tool) = match uri {
            "pdfcraft://document" => Some("doc_inspect"),
            "pdfcraft://commands" => Some("command_list"),
            _ => None,
        } {
            let content = self.automation.call(tool, &json!({})).map_err(|e| (INVALID_PARAMS, e.to_string()))?;
            let value = content
                .into_iter()
                .find_map(|c| match c {
                    Content::Json(v) => Some(v),
                    _ => None,
                })
                .ok_or((INVALID_PARAMS, "resource returned no JSON".into()))?;
            return Ok(json!({"uri":uri,"mimeType":"application/json","text":value.to_string()}));
        }
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
        "annotations": { "title": t.title, "readOnlyHint": t.read_only, "destructiveHint": t.destructive, "idempotentHint": t.read_only || matches!(t.name, "doc_save" | "doc_optimize" | "doc_export_data" | "measure_export" | "doc_export_images" | "doc_export_office" | "doc_export_all_images" | "doc_export_text" | "image_save"), "openWorldHint": false },
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
    use super::{Resource, first_sentence, parse_uri};

    #[test]
    fn conventions_panic_is_a_tool_error() {
        let r = super::guarded_tool(|| panic!("synthetic tool failure")).unwrap();
        assert_eq!(r["isError"], true);
        assert!(r["content"][0]["text"].as_str().unwrap().contains("synthetic tool failure"));
        let next = super::guarded_tool(|| Ok(serde_json::json!({"ok":true}))).unwrap();
        assert_eq!(next["ok"], true);
    }

    #[test]
    fn first_sentence_skips_abbreviations() {
        assert_eq!(first_sentence("Rotate pages. Undoable."), "Rotate pages.");
        assert_eq!(first_sentence("Label pages (i.e. roman numerals). Undoable."), "Label pages (i.e. roman numerals).");
        assert_eq!(first_sentence("Label pages (I.E. roman numerals, E.G. i, ii). Undoable."), "Label pages (I.E. roman numerals, E.G. i, ii).");
        assert_eq!(first_sentence("Use a value, e.g. en-US, or none. Then more."), "Use a value, e.g. en-US, or none.");
        assert_eq!(first_sentence("No full stop here"), "No full stop here");
        assert_eq!(first_sentence("Only an abbreviation, e.g. this"), "Only an abbreviation, e.g. this");
        assert_eq!(first_sentence(""), "");
        assert_eq!(first_sentence("Zażółć gęślą, i.e. jaźń. Dalej."), "Zażółć gęślą, i.e. jaźń.");
    }

    #[test]
    fn resource_uris_parse() {
        assert_eq!(parse_uri("pdfcraft://doc/3/info"), Some(Resource::Info(3)));
        assert_eq!(parse_uri("pdfcraft://doc/3/page/2/image?dpi=36"), Some(Resource::PageImage(3, 2, Some(36.0))));
        assert_eq!(parse_uri("pdfcraft://doc/3/page/2/text"), Some(Resource::PageText(3, 2)));
        assert_eq!(parse_uri("pdfcraft://doc/x/info"), None);
        assert_eq!(parse_uri("file:///etc/passwd"), None);
    }
}
