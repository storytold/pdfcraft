#![cfg(windows)]

use std::process::Command;

#[test]
fn printer_worker_only_accepts_read_only_queries() {
    for args in [
        vec!["--craft-printer-query"],
        vec!["--craft-printer-query", "doc_print", "unused.pdf"],
        vec!["--craft-printer-query", "list", "", "unexpected"],
        vec!["--craft-printer-query", "list", "unexpected-printer"],
        vec!["--craft-printer-query", "two-sided", ""],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_pdfcraft-cli")).args(args).output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    let output = Command::new(env!("CARGO_BIN_EXE_pdfcraft-cli"))
        .args(["--craft-printer-query", "two-sided", "pdfcraft-test: no such printer 🖨"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(serde_json::from_slice::<Option<bool>>(&output.stdout).unwrap(), None);
}

#[test]
fn isolated_printer_list_preserves_the_headless_tools_names_and_default() {
    let worker = Command::new(env!("CARGO_BIN_EXE_pdfcraft-cli")).args(["--craft-printer-query", "list", ""]).output().unwrap();
    assert!(worker.status.success());
    let queues: Vec<(String, bool)> = serde_json::from_slice(&worker.stdout).unwrap();
    let tool = Command::new(env!("CARGO_BIN_EXE_pdfcraft-cli")).args(["run", "printers"]).output().unwrap();
    assert!(tool.status.success());
    let answer: serde_json::Value = serde_json::from_slice(&tool.stdout).unwrap();
    let expected: Vec<_> = queues.into_iter().map(|(name, default)| serde_json::json!({"name": name, "default": default})).collect();
    let actual: Vec<_> =
        answer["printers"].as_array().unwrap().iter().map(|p| serde_json::json!({"name": p["name"], "default": p["default"]})).collect();
    assert_eq!(actual, expected);
}
