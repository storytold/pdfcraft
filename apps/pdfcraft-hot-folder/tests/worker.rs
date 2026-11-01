//! Exercise the shipped parent/child protocol, without installing a service or launching a GUI.
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use pdfcraft_engine::{Session, hot_folder as hf, ocr};

fn run(args: &[&str], config: &Path) -> ExitStatus {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pdfcraft-hot-folder"));
    command.args(args).arg("--config").arg(config).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().unwrap();
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if started.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            // Poll, rather than an unbounded wait even in the failing test path.
            let killing = Instant::now();
            while child.try_wait().unwrap().is_none() && killing.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(50));
            }
            panic!("supervised one-pass processing did not finish within 30 seconds");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn real_task_children_continue_after_an_unavailable_source_and_reuse_receipts_after_restart() {
    let base = tempfile::tempdir().unwrap();
    let config = base.path().join("tasks.json");
    #[cfg(unix)]
    if rustix::process::geteuid().is_root() {
        // Root processing is intentionally unsupported, including in test containers.
        assert!(!run(&["--once"], &config).success());
        assert!(!config.exists());
        return;
    }
    let input = base.path().join("local-in");
    let output = base.path().join("local-out");
    std::fs::create_dir(&input).unwrap();
    std::fs::create_dir(&output).unwrap();
    let local = hf::Task {
        id: "local".into(),
        name: "Local".into(),
        input: input.clone(),
        output: output.clone(),
        enabled: true,
        interval_seconds: 1,
        settle_seconds: 1,
        settings: ocr::OcrSettings { recognize: false, auto_rotate: false, deskew: false, ..Default::default() },
        ..Default::default()
    };
    let unavailable = hf::Task {
        id: "unavailable".into(),
        name: "Unavailable".into(),
        input: base.path().join("missing-input"),
        output: base.path().join("missing-output"),
        ..local.clone()
    };
    let settings = hf::Config { tasks: vec![unavailable, local] };
    hf::validate_structure(&settings, None).unwrap();
    std::fs::write(&config, serde_json::to_vec(&settings).unwrap()).unwrap();
    let source = Session::new().create_blank(100.0, 100.0, 1).unwrap();
    std::fs::write(input.join("page.pdf"), source.as_slice()).unwrap();
    assert!(run(&["--once"], &config).success());
    std::thread::sleep(Duration::from_millis(1100));
    assert!(run(&["--once"], &config).success());
    let state = hf::read_state(&config).unwrap();
    assert_eq!(state.tasks["local"].completed, 1);
    assert!(state.tasks["unavailable"].retry_count > 0);
    assert!(!state.tasks["unavailable"].blocked);
    assert!(output.join("page.pdf").is_file());
    assert_eq!(std::fs::read(input.join("page.pdf")).unwrap(), *source);
    std::thread::sleep(Duration::from_millis(1100));
    assert!(run(&["--once"], &config).success());
    assert_eq!(hf::read_state(&config).unwrap().tasks["local"].completed, 1);
    assert_eq!(std::fs::read_dir(&output).unwrap().count(), 1);
}

#[test]
fn internal_child_refuses_to_process_without_its_supervisor_pipe() {
    let base = tempfile::tempdir().unwrap();
    let config = base.path().join("tasks.json");
    assert!(!run(&["--task-child", "--task-id", "task"], &config).success());
    assert!(!config.exists());
    assert!(!config.with_extension("state.json").exists());
}
