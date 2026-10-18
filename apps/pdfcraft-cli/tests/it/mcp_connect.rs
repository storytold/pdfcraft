//! `pdfcraft-cli mcp --connect FILE`: the MCP tools act on the open window started with
//! `pdfcraft --control FILE`, through its control channel, and `ui_*` tools drive the window.
#![cfg(feature = "mcp")]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(test: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-connect-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

/// Write a control file the way the app does: readable by the owner only.
fn write_private(path: &Path, port: u16) {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let text = json!({ "port": port, "token": TOKEN, "pid": 1 }).to_string();
    opts.open(path).unwrap().write_all(text.as_bytes()).unwrap();
}

/// A stand-in for the app's control listener: it wants the token first, like the app, answers
/// `tool.call` and `ui.*` the way the app does, and records every request after `auth`.
fn fake_window() -> (u16, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let log = Arc::clone(&log);
            std::thread::spawn(move || {
                let mut write = stream.try_clone().unwrap();
                let mut authed = false;
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { break };
                    let msg: Value = serde_json::from_str(&line).unwrap();
                    let (id, method, params) = (msg["id"].clone(), msg["method"].as_str().unwrap_or("").to_string(), msg["params"].clone());
                    let result = if !authed {
                        authed = method == "auth" && params["token"] == TOKEN;
                        if authed { Ok(json!({ "ok": true })) } else { Err("authenticate first: auth {token}".to_string()) }
                    } else {
                        log.lock().unwrap().push(json!({ "method": method, "params": params }));
                        match (method.as_str(), params["name"].as_str()) {
                            ("tool.call", Some("doc_list")) => Ok(
                                json!({ "content": [{ "type": "json", "value": { "documents": [{ "doc": 3, "name": "open-in-window.pdf", "pages": 2 }] } }] }),
                            ),
                            ("tool.call", Some("sign_document")) => {
                                Err("sign_document: signing in the PdfCraft window is for the person to do, not an agent".to_string())
                            }
                            ("ui.state", _) => Ok(json!({ "documents": [{ "doc": 3, "name": "open-in-window.pdf", "active": true }] })),
                            _ => Err(format!("unknown method {method:?}")),
                        }
                    };
                    let reply = match result {
                        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
                        Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": e } }),
                    };
                    if writeln!(write, "{reply}").is_err() || !authed {
                        break;
                    }
                }
            });
        }
    });
    (port, seen)
}

/// Run `pdfcraft-cli mcp --connect control`, send `requests` (one JSON-RPC message per line),
/// close its input and return its replies by id, and its standard error.
fn mcp(control: &Path, requests: &[Value]) -> (Vec<Value>, String, bool) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pdfcraft-cli"))
        .arg("mcp")
        .arg("--connect")
        .arg(control)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for r in requests {
        let _ = writeln!(stdin, "{r}");
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let replies = String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    (replies, String::from_utf8_lossy(&out.stderr).into_owned(), out.status.success())
}

fn call(id: u64, name: &str, arguments: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": arguments } })
}

fn reply(replies: &[Value], id: u64) -> &Value {
    replies.iter().find(|r| r["id"] == id).unwrap_or_else(|| panic!("no reply {id} in {replies:?}"))
}

#[test]
fn mcp_connect_runs_the_tools_in_the_open_window() {
    let dir = scratch("tools");
    let (port, seen) = fake_window();
    let control = dir.0.join("pc.json");
    write_private(&control, port);
    let (replies, stderr, ok) = mcp(
        &control,
        &[
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} }),
            call(3, "doc_list", json!({})),
            call(4, "ui_state", json!({})),
            call(5, "sign_document", json!({ "doc": 3 })),
        ],
    );
    assert!(ok, "{stderr}");
    let tools: Vec<&str> = reply(&replies, 2)["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(tools.contains(&"doc_list") && tools.contains(&"ui_state") && tools.contains(&"ui_screenshot"), "{tools:?}");
    // The document list is the window's, not an empty session of the server's own.
    let docs = &reply(&replies, 3)["result"]["structuredContent"]["documents"];
    assert_eq!(docs[0]["name"], "open-in-window.pdf", "{replies:?}");
    assert_eq!(reply(&replies, 4)["result"]["structuredContent"]["documents"][0]["doc"], 3, "{replies:?}");
    // The window's refusal reaches the agent as a tool error it can read.
    let refused = &reply(&replies, 5)["result"];
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(refused["content"][0]["text"].as_str().unwrap().contains("for the person to do"), "{refused}");
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0], json!({ "method": "tool.call", "params": { "name": "doc_list", "arguments": {} } }), "{seen:?}");
    assert_eq!(seen[1]["method"], "ui.state", "{seen:?}");
}

#[test]
fn mcp_connect_without_a_running_window_says_how_to_start_it() {
    let dir = scratch("absent");
    let control = dir.0.join("pc.json");
    let (replies, stderr, ok) = mcp(&control, &[call(1, "ui_state", json!({})), call(2, "doc_list", json!({}))]);
    assert!(ok, "the server keeps serving: {stderr}");
    assert!(stderr.contains("pdfcraft --control"), "{stderr}");
    for id in [1, 2] {
        let r = &reply(&replies, id)["result"];
        assert_eq!(r["isError"], true, "{r}");
        assert!(r["content"][0]["text"].as_str().unwrap().contains("pdfcraft --control"), "{r}");
    }
}

#[test]
fn mcp_connect_refuses_automation_roots() {
    let dir = scratch("roots");
    let control = dir.0.join("pc.json");
    let child = Command::new(env!("CARGO_BIN_EXE_pdfcraft-cli"))
        .arg("mcp")
        .arg("--connect")
        .arg(&control)
        .arg("--automation-read-root")
        .arg(&dir.0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--connect does not support --automation-read-root"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn mcp_connect_refuses_a_control_file_other_users_can_read() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("mode");
    let (port, seen) = fake_window();
    let control = dir.0.join("pc.json");
    write_private(&control, port);
    std::fs::set_permissions(&control, std::fs::Permissions::from_mode(0o644)).unwrap();
    let (_, stderr, ok) = mcp(&control, &[call(1, "doc_list", json!({}))]);
    assert!(!ok && stderr.contains("other users"), "{stderr}");
    assert!(seen.lock().unwrap().is_empty(), "nothing reached the listener");
}
