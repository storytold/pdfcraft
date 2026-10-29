//! `pdfcraft-cli run --script --report`: one JSON document for the whole run (#1891).

use std::process::{Command, Output};

fn run_script(test: &str, steps: &serde_json::Value, extra: &[&str]) -> Output {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("steps.json");
    std::fs::write(&script, steps.to_string()).unwrap();
    let out =
        Command::new(env!("CARGO_BIN_EXE_pdfcraft-cli")).current_dir(&dir).arg("run").arg("--script").arg(&script).args(extra).output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    out
}

fn blank() -> serde_json::Value {
    serde_json::json!({ "tool": "doc_create", "args": { "from": "blank", "width": 100, "height": 100 } })
}

#[test]
fn a_successful_run_is_one_document_with_a_result_per_step() {
    let steps = serde_json::json!([blank(), { "tool": "page_render", "args": { "doc": 1, "page": 1, "dpi": 10 } }]);
    let o = run_script("report-ok", &steps, &["--report"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let report: serde_json::Value = serde_json::from_slice(&o.stdout).expect("stdout is one JSON document");
    assert_eq!(report["ok"], true);
    let steps = report["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!((steps[0]["step"].clone(), steps[0]["tool"].clone(), steps[0]["status"].clone()), (1.into(), "doc_create".into(), "ok".into()));
    assert_eq!(steps[1]["result"][0]["image"], "png", "an image with no `out` is summarised: {report}");
}

#[test]
fn a_failing_step_is_named_and_the_rest_are_skipped() {
    let steps = serde_json::json!([blank(), { "tool": "page_rotate", "args": { "doc": 1, "pages": [999], "degrees": 90 } }, { "tool": "doc_close", "args": { "doc": 1 } }]);
    let o = run_script("report-fail", &steps, &["--report"]);
    assert_eq!(o.status.code(), Some(1), "exit status is unchanged");
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(stderr.contains("step 2 (page_rotate)"), "{stderr}");
    let report: serde_json::Value = serde_json::from_slice(&o.stdout).expect("stdout is one JSON document");
    assert_eq!(report["ok"], false);
    let steps = report["steps"].as_array().unwrap();
    assert_eq!(steps.iter().map(|s| s["status"].as_str().unwrap()).collect::<Vec<_>>(), ["ok", "error", "skipped"]);
    assert!(steps[1]["error"].as_str().unwrap().contains("step 2 (page_rotate)"), "{report}");
    assert_eq!(steps[2]["tool"], "doc_close");
}

#[test]
fn a_step_without_a_tool_is_an_error_step() {
    let o = run_script("report-notool", &serde_json::json!([{ "args": {} }]), &["--report"]);
    assert_eq!(o.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(report["steps"][0]["status"], "error");
    assert!(report["steps"][0]["tool"].is_null());
}

#[test]
fn without_the_flag_the_output_is_unchanged() {
    let o = run_script("report-off", &serde_json::json!([blank()]), &[]);
    assert!(o.status.success());
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert!(!stdout.contains("\"steps\""), "{stdout}");
}

#[test]
fn the_flag_needs_a_script() {
    let o = Command::new(env!("CARGO_BIN_EXE_pdfcraft-cli")).args(["run", "doc_create", "--report"]).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("--report needs --script"));
}
