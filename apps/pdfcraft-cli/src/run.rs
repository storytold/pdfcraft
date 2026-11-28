use std::path::Path;

use serde_json::json;

use crate::cli::{BatchArgs, KeyValue, RunArgs};
use crate::error::CliError;
use crate::{Io, stdout_line};

/// The automation session, confined to `root` when one is given.
pub(crate) fn automation(root: Option<&Path>) -> Result<pdfcraft_automation::Automation, CliError> {
    let a = pdfcraft_automation::Automation::new();
    match root {
        Some(root) => a.with_root(root).map_err(|source| CliError::Root { path: root.to_path_buf(), source }),
        None => Ok(a),
    }
}

pub(crate) fn tools(io: &mut Io) -> Result<(), CliError> {
    let list: Vec<serde_json::Value> = pdfcraft_automation::tools()
        .iter()
        .map(|t| json!({ "name": t.name, "description": t.description, "read_only": t.read_only, "command": t.command, "input_schema": t.input_schema }))
        .collect();
    stdout_line(io, format_args!("{}", serde_json::to_string_pretty(&list).unwrap_or_default()))
}

/// Print a tool's result: JSON as JSON; images go to `--out` (or are summarised). The image is
/// written like a tool's own output, so `--root` confines it too.
pub(crate) fn print_output(
    io: &mut Io,
    auto: &pdfcraft_automation::Automation,
    content: Vec<pdfcraft_automation::Content>,
    out: Option<&str>,
) -> Result<(), CliError> {
    for value in output_values(auto, content, out)? {
        match value {
            // A JSON result is printed pretty; an image's one-line summary compact, as before.
            v if v.get("image").is_some() => stdout_line(io, format_args!("{v}"))?,
            v => stdout_line(io, format_args!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()))?,
        }
    }
    Ok(())
}

/// What a PNG result is reported as when it isn't written to a file.
fn png_summary(data: &[u8], width: u32, height: u32) -> serde_json::Value {
    json!({ "image": "png", "width": width, "height": height, "bytes": data.len(), "hint": "pass --out file.png to save it" })
}

/// A tool's result as JSON values: the content of a result, with an image written to `out`.
fn output_values(
    auto: &pdfcraft_automation::Automation,
    content: Vec<pdfcraft_automation::Content>,
    out: Option<&str>,
) -> Result<Vec<serde_json::Value>, CliError> {
    let mut values = Vec::new();
    for c in content {
        values.push(match c {
            pdfcraft_automation::Content::Json(v) => v,
            pdfcraft_automation::Content::Png { data, width, height } => match out {
                Some(path) => {
                    let written = auto.write_output(path, &data)?;
                    json!({ "image": written.to_string_lossy(), "width": width, "height": height })
                }
                None => png_summary(&data, width, height),
            },
        });
    }
    Ok(values)
}

/// Run a script and print one JSON document: per step its number, tool and `ok`/`error`/`skipped`
/// with the result or the message. The first failing step ends the run, as without `--report`;
/// the steps after it are listed as skipped and the exit status is the same.
fn run_script_report(io: &mut Io, auto: &mut pdfcraft_automation::Automation, steps: &[serde_json::Value]) -> Result<(), CliError> {
    let mut report = Vec::with_capacity(steps.len());
    let mut failure: Option<CliError> = None;
    for (i, step) in steps.iter().enumerate() {
        let n = i + 1;
        let tool = step["tool"].as_str();
        if failure.is_some() {
            report.push(json!({ "step": n, "tool": tool, "status": "skipped" }));
            continue;
        }
        let outcome = match tool {
            None => Err(CliError::StepWithoutTool { step: n }),
            Some(t) => auto
                .call(t, &step["args"])
                .map_err(CliError::from)
                .and_then(|content| output_values(auto, content, step["out"].as_str()))
                .map_err(|e| CliError::Step { step: n, tool: t.to_owned(), source: Box::new(e) }),
        };
        match outcome {
            Ok(result) => report.push(json!({ "step": n, "tool": tool, "status": "ok", "result": result })),
            Err(e) => {
                report.push(json!({ "step": n, "tool": tool, "status": "error", "error": e.to_string() }));
                failure = Some(e);
            }
        }
    }
    let document = json!({ "ok": failure.is_none(), "steps": report });
    stdout_line(io, format_args!("{}", serde_json::to_string_pretty(&document).unwrap_or_default()))?;
    failure.map_or(Ok(()), Err)
}

fn object(args: &[KeyValue]) -> serde_json::Value {
    serde_json::Value::Object(args.iter().map(|kv| (kv.key.clone(), kv.value.clone())).collect())
}

pub(crate) fn run(args: &RunArgs, io: &mut Io) -> Result<(), CliError> {
    let mut auto = automation(args.root.as_deref())?;
    if let Some(script) = &args.script {
        let text = std::fs::read_to_string(script).map_err(|e| CliError::file(script, e))?;
        let steps: Vec<serde_json::Value> = serde_json::from_str(&text).map_err(|source| CliError::Script { path: script.clone(), source })?;
        if args.report {
            return run_script_report(io, &mut auto, &steps);
        }
        for (i, step) in steps.iter().enumerate() {
            let n = i + 1;
            let tool = step["tool"].as_str().ok_or(CliError::StepWithoutTool { step: n })?;
            let wrap = |e: CliError| match e {
                stdout if matches!(stdout, CliError::Stdout(_)) => stdout,
                e => CliError::Step { step: n, tool: tool.to_owned(), source: Box::new(e) },
            };
            let content = auto.call(tool, &step["args"]).map_err(|e| wrap(e.into()))?;
            print_output(io, &auto, content, step["out"].as_str()).map_err(wrap)?;
        }
        return Ok(());
    }
    // The parser's group requires the tool when there is no script.
    let tool = args.tool.as_deref().ok_or_else(|| CliError::command("run", "missing tool name (see `pdfcraft-cli tools`)"))?;
    let content = auto.call(tool, &object(&args.args))?;
    let out = args.out.as_ref().map(|p| p.to_string_lossy().into_owned());
    print_output(io, &auto, content, out.as_deref())
}

/// `batch`: friendly flags for the same folder-job tool exposed by `run` and MCP.
pub(crate) fn batch(args: &BatchArgs, io: &mut Io) -> Result<(), CliError> {
    let steps: Vec<_> = args.steps.iter().map(|s| s.name()).collect();
    let paths: Vec<_> = args.paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    let mut params = json!({
        "paths": paths, "steps": steps, "out_dir": args.out_dir.to_string_lossy(),
        "recursive": args.recursive, "stop_on_error": args.stop_on_error,
    });
    if let Some(ppi) = args.ppi {
        params["ppi"] = json!(ppi);
    }
    if let Some(quality) = args.quality {
        params["quality"] = json!(quality);
    }
    if let Some(dpi) = args.dpi {
        params["dpi"] = json!(dpi);
    }
    if let Some(language) = &args.language {
        params["language"] = json!(language);
    }
    if args.ocr_all_pages {
        params["skip_text_pages"] = json!(false);
    }
    let mut auto = automation(args.root.as_deref())?;
    let content = auto.call("doc_batch", &params)?;
    let failed = content.iter().any(|c| matches!(c, pdfcraft_automation::Content::Json(v) if v["failed"].as_u64().is_some_and(|n| n > 0)));
    print_output(io, &auto, content, None)?;
    if failed { Err(CliError::BatchFailed) } else { Ok(()) }
}

/// The MCP server on the process's own stdin and stdout: it speaks the protocol there, so nothing
/// else may be written to stdout (`out` is not used), and it never starts on its own.
#[cfg(feature = "mcp")]
pub(crate) fn mcp(args: &crate::cli::McpArgs, io: &mut Io) -> Result<(), CliError> {
    let roots = pdfcraft_automation::file_access::AutomationRoots::new(args.automation_read_root.as_deref(), args.automation_write_root.as_deref())
        .map_err(|e| CliError::Roots(e.to_string()))?;
    let mut auto = automation(args.root.as_deref())?;
    if let Some(roots) = roots {
        let show = |root: Option<&Path>| root.map_or_else(|| "none".to_string(), |r| r.display().to_string());
        let _ = writeln!(io.err, "pdfcraft-cli: files confined: read root {}, write root {}", show(roots.read_root()), show(roots.write_root()));
        auto = auto.with_automation_roots(Some(roots));
    }
    let mut server = pdfcraft_automation::mcp::McpServer::new(auto).with_compact(args.compact);
    let _ = writeln!(
        io.err,
        "pdfcraft-cli: MCP server on stdio (protocol {}{}); close stdin to stop",
        pdfcraft_automation::mcp::PROTOCOL_VERSIONS[0],
        if args.compact { ", compact tool list" } else { "" }
    );
    server.serve(std::io::stdin().lock(), std::io::stdout().lock()).map_err(|e| CliError::command("mcp", e.to_string()))
}
