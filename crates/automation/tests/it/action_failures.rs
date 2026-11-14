//! A step that fails fails its file: the file is reported with an error and nothing is written
//! for it. A step that finds nothing to do is still not a failure.

use std::sync::Arc;

use pdfcraft_automation::{Automation, Content};
use pdfcraft_engine::Session;
use pdfcraft_engine::actions::{Action, Step, run_on};
use serde_json::json;

fn pdf() -> Vec<u8> {
    Session::new().create_from_text("Original", "Original text").unwrap().as_ref().clone()
}

fn one_step(step: Step) -> Action {
    Action { name: "Custom".into(), description: String::new(), steps: vec![step], builtin: false }
}

/// The standard fonts can't draw the character, so the watermark step fails and no file is produced.
#[test]
fn failing_watermark_step_fails_the_file_and_publishes_nothing() {
    let r = run_on(&one_step(Step::AddWatermark("中".into())), "one.pdf", Arc::new(pdf()), true, |_, _| {});
    assert!(r.is_err(), "a failed step must fail its file: {r:?}");
}

/// A comment step on a document with no comments has nothing to do, which is not a failure.
#[test]
fn nothing_to_do_step_still_succeeds() {
    let r = run_on(&one_step(Step::FlattenComments), "one.pdf", Arc::new(pdf()), true, |_, _| {});
    assert!(r.is_ok(), "a step with nothing to do is not a failure: {r:?}");
}

/// Through the tool: the failing file gets an error entry and no output, and the folder stays empty.
#[test]
fn action_run_marks_the_failing_file_as_error_without_output() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in");
    std::fs::create_dir_all(&input).unwrap();
    let in_file = input.join("one.pdf");
    std::fs::write(&in_file, pdf()).unwrap();
    let out = dir.path().join("out");
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let reply = auto.call(
        "action_run",
        &json!({
            "paths": [in_file.to_string_lossy()],
            "folder": out.to_string_lossy(),
            "steps": [{ "step": "add_watermark", "arg": "中" }],
        }),
    );
    let report = match reply.unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected a report: {other:?}"),
    };
    let entry = &report["files"][0];
    assert!(entry["error"].is_string(), "{report}");
    assert!(entry.get("output").is_none(), "{report}");
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0, "nothing is published for a failed file");
}

/// A removal step with nothing to remove keeps its existing note and succeeds.
#[test]
fn removal_step_with_nothing_to_remove_still_succeeds() {
    let r = run_on(&one_step(Step::RemoveHiddenInformation), "one.pdf", Arc::new(pdf()), true, |_, _| {});
    assert!(r.is_ok(), "nothing to remove is not a failure: {r:?}");
}
