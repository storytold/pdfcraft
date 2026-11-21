//! A batch run that fails a file publishes only the completed outputs, and a
//! publish the file system refuses publishes nothing.

use pdfcraft_automation::{Automation, Content};
use pdfcraft_engine::Session;
use serde_json::{Value, json};

fn result(auto: &mut Automation, args: Value) -> Value {
    match auto.call("doc_batch", &args).unwrap().remove(0) {
        Content::Json(value) => value,
        _ => panic!("expected a report"),
    }
}

fn pdf() -> Vec<u8> {
    Session::new().create_from_text("Original", "Original text").unwrap().as_ref().clone()
}

/// Sets a folder's permission bits.
#[cfg(unix)]
fn set_mode(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// Puts back the write permission the temporary folder needs before it is removed.
#[cfg(unix)]
struct RestoreMode(std::path::PathBuf);

#[cfg(unix)]
impl Drop for RestoreMode {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
    }
}

#[test]
fn failed_inputs_publish_only_completed_outputs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("input")).unwrap();
    std::fs::write(dir.path().join("input/good.pdf"), pdf()).unwrap();
    std::fs::write(dir.path().join("input/bad.pdf"), "not a pdf").unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let report = result(&mut auto, json!({"paths":["input"],"steps":["optimize"],"out_dir":"out","recursive":false}));
    assert_eq!(report["completed"], 1, "{report}");
    assert_eq!(report["failed"], 1, "{report}");
    let files = report["files"].as_array().unwrap();
    assert_eq!(files.len(), 2, "{report}");
    let bad = files.iter().find(|file| file["path"].as_str().is_some_and(|path| path.ends_with("bad.pdf"))).unwrap();
    assert_eq!(bad["status"], "error", "{report}");
    assert!(bad["output"].is_null(), "a failed file reports no output: {report}");
    let names: Vec<String> =
        std::fs::read_dir(dir.path().join("out")).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
    // Publish stages its bytes under a dot-prefixed name in the output folder, so
    // no entry carrying that prefix may survive the run.
    for name in &names {
        assert!(!name.starts_with('.'), "temporary name left behind: {name} ({names:?})");
    }
    assert_eq!(names, vec!["good.pdf".to_string()], "only the completed output is published");
}

#[cfg(unix)]
#[test]
fn read_only_output_folder_publishes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("input")).unwrap();
    std::fs::write(dir.path().join("input/good.pdf"), pdf()).unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    set_mode(&out, 0o555);
    let _restore = RestoreMode(out.clone());
    // A root process ignores the mode bits, and then the assertions below would
    // pass without testing the refusal. Fail loudly instead of skipping.
    assert!(std::fs::write(out.join("probe"), b"").is_err(), "run as a non-root user: the read-only output folder accepted a new file");
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let report = result(&mut auto, json!({"paths":["input"],"steps":["optimize"],"out_dir":"out","recursive":false}));
    assert_eq!(report["completed"], 0, "{report}");
    assert_eq!(report["failed"], 1, "{report}");
    let files = report["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{report}");
    // Publish cannot create its staging file in a folder it may not write, so
    // the entry carries the system's error and no output.
    assert_eq!(files[0]["status"], "error", "{report}");
    assert!(files[0]["output"].is_null(), "a refused publish reports no output: {report}");
    let error = files[0]["error"].as_str().unwrap();
    assert!(!error.is_empty(), "{report}");
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0, "the refused publish writes nothing into the read-only folder: {report}");
}
