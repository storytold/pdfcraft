//! Optional, one-shot subscription CLIs. No shell, tools, MCP servers, API credentials or project context.
use crate::{Endpoint, Kind};
use serde_json::Value;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);
const LIMIT: u64 = 8 << 20;

/// Find an already installed CLI without launching it or installing anything.
pub fn default_program(kind: Kind) -> String {
    let name = if kind == Kind::ClaudeCode { "claude" } else { "codex" };
    let mut dirs: Vec<std::path::PathBuf> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    if let Some(home) = std::env::var_os("HOME") {
        let home = std::path::PathBuf::from(home);
        dirs.push(home.join(".local/bin"));
        if let Ok(entries) = std::fs::read_dir(home.join(".nvm/versions/node")) {
            let mut nodes: Vec<_> = entries.flatten().map(|e| e.path().join("bin")).collect();
            nodes.sort();
            nodes.reverse();
            dirs.extend(nodes);
        }
    }
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file()).map_or_else(|| name.into(), |p| p.to_string_lossy().into_owned())
}

fn stop(child: &mut std::process::Child) {
    // This is our newly created process group, never an existing agent session.
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill").args(["-KILL", "--", &format!("-{}", child.id())]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

pub fn parse_reply(kind: Kind, text: &str) -> Result<String, String> {
    if kind == Kind::ClaudeCode {
        let v: Value = serde_json::from_str(text).map_err(|_| "Claude Code devolvió una respuesta ilegible")?;
        if v.get("is_error").and_then(Value::as_bool) == Some(true) {
            return Err("Claude Code informó un error. Revise su sesión y cuota.".into());
        }
        if let Some(v) = v.get("structured_output").filter(|v| v.is_object()) {
            return Ok(v.to_string());
        }
        return v
            .get("result")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| "Claude Code no devolvió texto".into());
    }
    let mut reply = None;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v.get("type").and_then(Value::as_str) == Some("error") || v.get("type").and_then(Value::as_str) == Some("turn.failed") {
            return Err("Codex informó un error. Revise su sesión y cuota.".into());
        }
        if v.pointer("/item/type").and_then(Value::as_str) == Some("agent_message") {
            reply = v.pointer("/item/text").and_then(Value::as_str).map(str::to_owned);
        }
    }
    reply.filter(|s| !s.trim().is_empty()).ok_or_else(|| "Codex no devolvió una respuesta final".into())
}

pub fn chat(ep: &Endpoint, system: &str, user: &str, schema: &Value, cancel: &AtomicBool, timeout: Duration) -> Result<String, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Cancelado.".into());
    }
    let dir = std::env::temp_dir().join(format!("pdfcraft-ai-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir(&dir).map_err(|e| format!("No se pudo crear una sesión temporal: {e}"))?;
    let result = run(ep, system, user, schema, cancel, timeout, &dir);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn run(
    ep: &Endpoint,
    system: &str,
    user: &str,
    schema: &Value,
    cancel: &AtomicBool,
    timeout: Duration,
    dir: &std::path::Path,
) -> Result<String, String> {
    let mut cmd = Command::new(&ep.base_url);
    cmd.current_dir(dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    for key in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "OPENAI_API_BASE",
        "CODEX_API_KEY",
        "ANTHROPIC_BASE_URL",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
    ] {
        cmd.env_remove(key);
    }
    if ep.kind == Kind::ClaudeCode {
        cmd.args([
            "-p",
            "--output-format",
            "json",
            "--tools",
            "",
            "--strict-mcp-config",
            "--mcp-config",
            "{\"mcpServers\":{}}",
            "--no-session-persistence",
            "--disable-slash-commands",
            "--setting-sources",
            "",
            "--json-schema",
            &schema.to_string(),
        ]);
    } else {
        cmd.args([
            "exec",
            "--ignore-user-config",
            "--ephemeral",
            "--sandbox",
            "read-only",
            "--skip-git-repo-check",
            "--json",
            "-c",
            "forced_login_method=\"chatgpt\"",
            "-c",
            "features.shell_tool=false",
        ]);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd
        .spawn()
        .map_err(|_| "No se pudo iniciar el CLI elegido. Indique la ruta del programa instalado e inicie sesión antes de usarlo.".to_owned())?;
    let prompt = format!(
        "Act only as a document question-answering model. Do not use tools, files, commands or the web. Return only JSON matching this schema: {schema}\n{system}\n\n{user}"
    );
    let input = child.stdin.take();
    let writer = std::thread::spawn(move || {
        if let Some(mut input) = input {
            let _ = input.write_all(prompt.as_bytes());
        }
    });
    let output = child.stdout.take();
    let reader = std::thread::spawn(move || -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        if let Some(out) = output {
            out.take(LIMIT + 1).read_to_end(&mut bytes).map_err(|_| "No se pudo leer el CLI")?;
        }
        Ok(bytes)
    });
    let start = Instant::now();
    let status = loop {
        if cancel.load(Ordering::Relaxed) || start.elapsed() >= timeout.min(Duration::from_secs(120)) {
            stop(&mut child);
            let _ = writer.join();
            let _ = reader.join();
            return Err(if cancel.load(Ordering::Relaxed) { "Cancelado." } else { "El CLI superó el tiempo máximo de 120 segundos." }.into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => {
                stop(&mut child);
                return Err("No se pudo supervisar el CLI.".into());
            }
        }
    };
    stop(&mut child);
    let _ = writer.join();
    let bytes = reader.join().map_err(|_| "Falló la lectura del CLI")??;
    if bytes.len() as u64 > LIMIT {
        return Err("La respuesta del CLI supera el límite permitido.".into());
    }
    if !status.success() {
        return Err("El CLI no terminó correctamente. Revise su inicio de sesión y cuota; no hay fallback de pago.".into());
    }
    parse_reply(ep.kind, &String::from_utf8_lossy(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn claude_structured_and_error() {
        assert_eq!(
            parse_reply(Kind::ClaudeCode, r#"{"structured_output":{"points":[],"not_found":true}}"#).unwrap(),
            r#"{"not_found":true,"points":[]}"#
        );
        assert!(parse_reply(Kind::ClaudeCode, r#"{"is_error":true,"result":"secret"}"#).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn cli_is_bounded_and_receives_no_api_credentials() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("pdfcraft-cli-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("fake-cli");
        std::fs::write(&script, "#!/bin/sh\n[ -z \"$ANTHROPIC_API_KEY$ANTHROPIC_AUTH_TOKEN$OPENAI_API_KEY$CODEX_API_KEY\" ] || exit 99\ncat >/dev/null\nprintf '%s' '{\"result\":\"{\\\"points\\\":[],\\\"not_found\\\":true}\"}'\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let ep = Endpoint { kind: Kind::ClaudeCode, base_url: script.to_string_lossy().into_owned(), model: "default".into(), api_key: None };
        let reply = run(&ep, "system", "synthetic", &serde_json::json!({}), &AtomicBool::new(false), Duration::from_secs(2), &dir).unwrap();
        assert!(reply.contains("not_found"));
        std::fs::write(&script, "#!/bin/sh\ncat >/dev/null\nsleep 10\n").unwrap();
        let start = Instant::now();
        assert!(run(&ep, "system", "synthetic", &serde_json::json!({}), &AtomicBool::new(false), Duration::from_millis(200), &dir).is_err());
        assert!(start.elapsed() < Duration::from_secs(3));
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn codex_final_and_failure() {
        assert_eq!(parse_reply(Kind::Codex, "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"{}\"}}\n").unwrap(), "{}");
        assert!(parse_reply(Kind::Codex, r#"{"type":"turn.failed"}"#).is_err());
        assert!(parse_reply(Kind::Codex, "").is_err());
    }
}
