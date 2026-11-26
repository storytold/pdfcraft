//! `pdfcraft-cli mcp --automation-read-root DIR --automation-write-root DIR`, on the real binary:
//! both spellings of the flags confine the server, and a malformed flag stops it instead of
//! leaving it unconfined.
#![cfg(feature = "mcp")]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

/// `in/a.pdf` and `outside/secret.pdf`, blank one-page PDFs made by the binary itself.
fn folders(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("pdfcraft-cli-roots-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for d in ["in", "out", "outside"] {
        std::fs::create_dir_all(base.join(d)).unwrap();
    }
    for f in ["in/a.pdf", "outside/secret.pdf"] {
        let script = json!([
            {"tool": "doc_create", "args": {"from": "blank", "pages": 1, "width": 100, "height": 100}},
            {"tool": "doc_save", "args": {"doc": 1, "path": f}},
        ]);
        std::fs::write(base.join("steps.json"), script.to_string()).unwrap();
        let out = Command::new(BIN).args(["run", "--script"]).arg(base.join("steps.json")).arg("--root").arg(&base).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
    base
}

/// Run `mcp <flags>`, send `calls` as `tools/call` requests and return the replies.
fn session(flags: &[String], calls: &[(&str, Value)]) -> (Vec<Value>, String) {
    let mut child = Command::new(BIN).arg("mcp").args(flags).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut input = String::from("{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{}}\n");
    for (i, (name, args)) in calls.iter().enumerate() {
        let r = json!({"jsonrpc": "2.0", "id": i + 1, "method": "tools/call", "params": {"name": name, "arguments": args}});
        input.push_str(&format!("{r}\n"));
    }
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let replies = String::from_utf8(out.stdout).unwrap().lines().skip(1).map(|l| serde_json::from_str(l).unwrap()).collect();
    (replies, String::from_utf8(out.stderr).unwrap())
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn flags(spelling: &str, base: &Path) -> Vec<String> {
    let (r, w) = (s(&base.join("in")), s(&base.join("out")));
    if spelling == "equals" {
        vec![format!("--automation-read-root={r}"), format!("--automation-write-root={w}")]
    } else {
        vec!["--automation-read-root".into(), r, "--automation-write-root".into(), w]
    }
}

fn text(reply: &Value) -> String {
    reply["result"]["content"][0]["text"].as_str().unwrap_or_default().to_owned()
}

#[test]
fn both_spellings_confine_the_server() {
    for spelling in ["equals", "separate"] {
        let base = folders(spelling);
        let calls = [
            ("doc_open", json!({"path": s(&base.join("in/a.pdf"))})),
            ("doc_open", json!({"path": s(&base.join("outside/secret.pdf"))})),
            ("doc_save", json!({"doc": 1, "path": s(&base.join("out/copy.pdf"))})),
            ("doc_save", json!({"doc": 1, "path": s(&base.join("outside/copy.pdf"))})),
        ];
        let (replies, stderr) = session(&flags(spelling, &base), &calls);
        assert_eq!(replies[0]["result"]["isError"], false, "{spelling}: {replies:?}");
        assert!(text(&replies[1]).contains("automation path rejected: outside the read root"), "{spelling}: {replies:?}");
        assert_eq!(replies[2]["result"]["isError"], false, "{spelling}: {replies:?}");
        assert!(text(&replies[3]).contains("automation path rejected: outside the write root"), "{spelling}: {replies:?}");
        assert!(base.join("out/copy.pdf").is_file() && !base.join("outside/copy.pdf").exists());
        assert!(stderr.contains("files confined: read root") && stderr.contains("write root"), "{stderr}");
    }
}

#[test]
fn a_root_left_out_grants_none_of_its_access() {
    let base = folders("one");
    let read_only = vec!["--automation-read-root".to_string(), s(&base.join("in"))];
    let calls = [("doc_open", json!({"path": s(&base.join("in/a.pdf"))})), ("doc_save", json!({"doc": 1, "path": s(&base.join("out/x.pdf"))}))];
    let (replies, _) = session(&read_only, &calls);
    assert_eq!(replies[0]["result"]["isError"], false);
    assert!(text(&replies[1]).contains("write authority is absent"), "{replies:?}");
    assert!(!base.join("out/x.pdf").exists());
}

#[test]
fn no_flags_means_no_confinement() {
    let base = folders("none");
    let (replies, stderr) = session(&[], &[("doc_open", json!({"path": s(&base.join("outside/secret.pdf"))}))]);
    assert_eq!(replies[0]["result"]["isError"], false);
    assert!(!stderr.contains("confined"), "{stderr}");
}

fn fails(args: &[&str]) -> String {
    let out: Output = Command::new(BIN).arg("mcp").args(args).stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{args:?} should fail");
    assert!(out.stdout.is_empty(), "{args:?} must not start a server");
    String::from_utf8(out.stderr).unwrap()
}

#[test]
fn malformed_flags_are_errors_not_a_missing_root() {
    let dir = s(&std::env::temp_dir());
    assert!(fails(&["--automation-read-root"]).contains("--automation-read-root needs a folder"));
    assert!(fails(&["--automation-write-root", "--compact"]).contains("--automation-write-root needs a folder; use --automation-write-root=<dir>"));
    assert!(fails(&["--automation-read-root="]).contains("--automation-read-root needs a non-empty folder"));
    assert!(fails(&["--automation-read-root", &dir, "--automation-read-root", &dir]).contains("--automation-read-root given twice"));
    assert!(fails(&["--automation-write-root", &dir, &format!("--automation-write-root={dir}")]).contains("--automation-write-root given twice"));
    let missing = fails(&["--automation-read-root", "/definitely/not/a/folder"]);
    assert!(missing.contains("mcp: cannot open the automation read root"), "{missing}");
    let file = std::env::current_exe().unwrap();
    assert!(fails(&["--automation-write-root", &s(&file)]).contains("cannot open the automation write root"));
}
