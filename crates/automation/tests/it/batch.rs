use pdfcraft_automation::{Automation, Content};
use pdfcraft_engine::Session;
use serde_json::{Value, json};

fn result(auto: &mut Automation, args: Value) -> Value {
    match auto.call("doc_batch", &args).unwrap().remove(0) {
        Content::Json(value) => value,
        _ => panic!("expected a report"),
    }
}

// Fresh fixtures include the current timestamp; retain their bytes for output comparisons.
fn pdf() -> Vec<u8> {
    // Reuse one synthetic document so byte comparisons cannot straddle the
    // writer's modification-time clock. Each caller gets an independent buffer.
    static PDF: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    PDF.get_or_init(|| Session::new().create_from_text("Original", "Original text").unwrap().as_ref().clone()).clone()
}

#[test]
fn pdf_fixture_remains_identical_across_metadata_clock_tick() {
    let before = pdf();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    assert_eq!(before, pdf());
}

#[test]
fn batch_recurses_preserves_paths_continues_errors_and_never_replaces_outputs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("input/sub")).unwrap();
    std::fs::create_dir_all(dir.path().join("input/output/sub")).unwrap();
    std::fs::write(dir.path().join("input/a.txt"), "Hello").unwrap();
    std::fs::write(dir.path().join("input/b.pdf"), b"broken").unwrap();
    std::fs::write(dir.path().join("input/sub/a.txt"), "World").unwrap();
    std::fs::write(dir.path().join("input/notes.bin"), b"ignore").unwrap();
    std::fs::write(dir.path().join("input/output/sub/a.pdf"), b"precious").unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let args = json!({"paths":["input","input/a.txt"],"steps":["convert","optimize"],"out_dir":"input/output","recursive":true});
    let report = result(&mut auto, args.clone());
    assert_eq!(report["completed"], 2, "{report}");
    assert_eq!(report["failed"], 1);
    assert_eq!(report["skipped"], 1);
    assert_eq!(std::fs::read(dir.path().join("input/output/sub/a.pdf")).unwrap(), b"precious");
    assert_eq!(std::fs::read_to_string(dir.path().join("input/a.txt")).unwrap(), "Hello");
    for name in ["a.pdf", "sub/a (2).pdf"] {
        let bytes = std::fs::read(dir.path().join("input/output").join(name)).unwrap();
        let mut session = Session::new();
        let id = session.open(name, None, std::sync::Arc::new(bytes), None).unwrap();
        assert_eq!(session.get(id).unwrap().info.pages.len(), 1);
    }
    assert!(!dir.path().join("input/output/b.pdf").exists());
    assert!(auto.session().docs().is_empty());
    let again = result(&mut auto, args);
    assert_eq!(again["completed"], 2, "previous outputs must not become new inputs: {again}");
    assert!(dir.path().join("input/output/a (2).pdf").exists());
}

#[test]
fn batch_is_nonrecursive_by_default_and_stop_reports_unprocessed_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("input/sub")).unwrap();
    std::fs::write(dir.path().join("input/0.pdf"), b"broken").unwrap();
    let original = pdf();
    std::fs::write(dir.path().join("input/1.pdf"), &original).unwrap();
    std::fs::write(dir.path().join("input/sub/2.pdf"), &original).unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let report = result(&mut auto, json!({"paths":["input"],"steps":["optimize"],"out_dir":"out","stop_on_error":true}));
    assert_eq!(report["failed"], 1);
    assert_eq!(report["completed"], 0);
    assert_eq!(report["skipped"], 1);
    assert_eq!(report["stopped"], true);
    assert!(!dir.path().join("out").exists());
    let report = result(&mut auto, json!({"paths":["input"],"steps":["optimize"],"out_dir":"out"}));
    assert_eq!(report["completed"], 1);
    assert!(!dir.path().join("out/sub").exists());
    assert!(std::fs::metadata(dir.path().join("out/1.pdf")).unwrap().len() <= original.len() as u64);
}

#[test]
fn invalid_batch_arguments_and_outside_paths_write_nothing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("input")).unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    for change in [
        json!({"steps":["convert","convert"]}),
        json!({"steps":["optimize","convert"]}),
        json!({"steps":[]}),
        json!({"steps":["nope"]}),
        json!({"recursive":"yes"}),
        json!({"paths":[]}),
        json!({"paths":["input","../outside"]}),
        json!({"out_dir":"../outside"}),
        json!({"out_dir":"input"}),
        json!({"out_dir":"."}),
        json!({"quality":0}),
        json!({"quality":"80"}),
        json!({"ppi":2000}),
        json!({"steps":["ocr"],"language":"not-a-language"}),
        json!({"steps":["ocr"],"dpi":1}),
    ] {
        let mut args = json!({"paths":["input"],"steps":["optimize"],"out_dir":"out"});
        args.as_object_mut().unwrap().extend(change.as_object().unwrap().clone());
        assert!(auto.call("doc_batch", &args).is_err(), "{args}");
        assert!(!dir.path().join("out").exists(), "invalid arguments must be refused before writing");
    }
}

#[test]
fn explicitly_selected_subfolder_is_processed_without_recursive() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("input/sub")).unwrap();
    std::fs::write(dir.path().join("input/sub/page.pdf"), pdf()).unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let report = result(&mut auto, json!({"paths":["input","input/sub"],"steps":["convert"],"out_dir":"out"}));
    assert_eq!(report["completed"], 1, "{report}");
    assert!(dir.path().join("out/page.pdf").exists());
}

#[test]
fn explicit_file_can_write_a_numbered_copy_beside_its_source() {
    let dir = tempfile::tempdir().unwrap();
    let original = pdf();
    std::fs::write(dir.path().join("page.pdf"), &original).unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let report = result(&mut auto, json!({"paths":["page.pdf"],"steps":["convert"],"out_dir":"."}));
    assert_eq!(report["completed"], 1, "{report}");
    assert_eq!(std::fs::read(dir.path().join("page.pdf")).unwrap(), original);
    assert_eq!(std::fs::read(dir.path().join("page (2).pdf")).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn non_utf8_discovered_names_are_reported_without_panicking() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("input")).unwrap();
    let invalid = dir.path().join("input").join(std::ffi::OsStr::from_bytes(b"bad-\xff.pdf"));
    if let Err(error) = std::fs::write(&invalid, pdf()) {
        // APFS rejects this filename before batch discovery can see it. Keep the
        // discovery assertions on filesystems that allow it; no other I/O error
        // is an unsupported fixture.
        #[cfg(target_os = "macos")]
        if error.raw_os_error() == Some(92) {
            assert!(!invalid.exists());
            assert_eq!(std::fs::read_dir(dir.path().join("input")).unwrap().count(), 0);
            assert!(!dir.path().join("out").exists());
            let valid = dir.path().join("input/valid-λ.pdf");
            let original = pdf();
            std::fs::write(&valid, &original).unwrap();
            assert_eq!(std::fs::read(&valid).unwrap(), original);
            let mut auto = Automation::new().with_root(dir.path()).unwrap();
            let report = result(&mut auto, json!({"paths":["input"],"steps":["convert"],"out_dir":"out"}));
            assert_eq!(report["completed"], 1, "{report}");
            assert_eq!(report["failed"], 0, "{report}");
            assert_eq!(std::fs::read(dir.path().join("out/valid-λ.pdf")).unwrap(), original);
            eprintln!(
                "non-UTF-8 fixture capability unavailable: macOS filesystem rejected creation with EILSEQ92; valid-Unicode batch companion passed; invalid-name discovery still requires Linux/FreeBSD or a supporting filesystem"
            );
            return;
        }
        panic!("could not create the non-UTF-8 fixture: {error}");
    }
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let report = result(&mut auto, json!({"paths":["input"],"steps":["convert"],"out_dir":"out"}));
    assert_eq!(report["failed"], 1);
    assert!(report["files"][0]["error"].as_str().unwrap().contains("UTF-8"));
}

#[cfg(unix)]
#[test]
fn directory_links_are_not_followed_and_output_links_cannot_escape() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("input/sub")).unwrap();
    std::fs::create_dir(dir.path().join("outside")).unwrap();
    std::fs::create_dir(dir.path().join("out")).unwrap();
    std::fs::write(dir.path().join("input/sub/page.pdf"), pdf()).unwrap();
    std::os::unix::fs::symlink(dir.path().join("input"), dir.path().join("input/loop")).unwrap();
    std::os::unix::fs::symlink(dir.path().join("outside"), dir.path().join("out/sub")).unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let report = result(&mut auto, json!({"paths":["input"],"steps":["convert"],"out_dir":"out","recursive":true}));
    assert_eq!(report["skipped"], 1);
    assert_eq!(report["failed"], 1);
    assert_eq!(std::fs::read_dir(dir.path().join("outside")).unwrap().count(), 0);
}

#[test]
fn batch_ocr_uses_existing_models_or_reports_the_missing_models_per_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("page.pdf"), pdf()).unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let report = result(&mut auto, json!({"paths":["page.pdf"],"steps":["ocr","optimize"],"out_dir":"out"}));
    if pdfcraft_engine::ocr::available() {
        assert_eq!(report["completed"], 1, "{report}");
        assert_eq!(report["files"][0]["steps"][0]["skipped_pages"], 1);
        assert!(dir.path().join("out/page.pdf").exists());
    } else {
        assert_eq!(report["failed"], 1, "{report}");
        assert!(report["files"][0]["error"].as_str().unwrap().contains("model"), "{report}");
        assert!(!dir.path().join("out/page.pdf").exists());
    }
}

#[test]
fn batch_output_collisions_are_safe_under_concurrent_writers() {
    let dir = tempfile::tempdir().unwrap();
    let original = pdf();
    std::fs::write(dir.path().join("page.pdf"), &original).unwrap();
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(|| {
                    let mut auto = Automation::new().with_root(dir.path()).unwrap();
                    barrier.wait();
                    result(&mut auto, json!({"paths":["page.pdf"],"steps":["convert"],"out_dir":"out"}))
                })
            })
            .collect();
        let reports: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(reports.iter().all(|r| r["completed"] == 1), "{reports:?}");
        assert_ne!(reports[0]["files"][0]["output"], reports[1]["files"][0]["output"]);
    });
    for name in ["page.pdf", "page (2).pdf"] {
        assert_eq!(std::fs::read(dir.path().join("out").join(name)).unwrap(), original);
    }
    assert_eq!(std::fs::read_dir(dir.path().join("out")).unwrap().count(), 2, "no staging files remain");
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_batch_errors_keep_the_structured_per_file_report() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("broken.pdf"), b"broken").unwrap();
    let input = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"doc_batch","arguments":{"paths":["broken.pdf"],"steps":["optimize"],"out_dir":"out"}}}).to_string() + "\n";
    let mut out = Vec::new();
    pdfcraft_automation::mcp::McpServer::new(Automation::new().with_root(dir.path()).unwrap()).serve(input.as_bytes(), &mut out).unwrap();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["result"]["isError"], true, "{report}");
    assert_eq!(report["result"]["structuredContent"]["failed"], 1);
}
