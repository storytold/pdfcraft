#![cfg(windows)]

use std::process::Command;

fn query(args: &[&str]) -> std::process::Output {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let mut child = Command::new(env!("CARGO_BIN_EXE_pdfcraft-cli")).args(args).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = (&mut stdout).take(4 << 20).read_to_end(&mut bytes);
        let _ = std::io::copy(&mut stdout, &mut std::io::sink());
        let _ = sender.send(result.map(|_| bytes));
    });
    // The public printers tool still queries drivers directly; a hung driver must fail this test, not hang the suite.
    let deadline = Instant::now() + Duration::from_secs(300);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("printer query exceeded the test deadline: {args:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    std::process::Output {
        status: child.wait().unwrap(),
        stdout: receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())).unwrap().unwrap(),
        stderr: Vec::new(),
    }
}

#[test]
fn printer_worker_only_accepts_read_only_queries() {
    for args in [
        vec!["--craft-printer-query"],
        vec!["--craft-printer-query", "doc_print", "unused.pdf"],
        vec!["--craft-printer-query", "list", "", "unexpected"],
        vec!["--craft-printer-query", "list", "unexpected-printer"],
        vec!["--craft-printer-query", "two-sided", ""],
        vec!["--craft-printer-query", "paper-info", ""],
    ] {
        let output = query(&args);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    let output = query(&["--craft-printer-query", "two-sided", "pdfcraft-test: no such printer 🖨"]);
    assert!(output.status.success());
    assert_eq!(serde_json::from_slice::<Option<bool>>(&output.stdout).unwrap(), None);
    let output = query(&["--craft-printer-query", "paper-info", "pdfcraft-test: no such printer"]);
    assert!(output.status.success());
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(), serde_json::json!([[], null]));
}

#[test]
fn printer_worker_preserves_available_names_and_default() {
    let worker = query(&["--craft-printer-query", "list", ""]);
    assert!(worker.status.success());
    let queues: Vec<(String, bool)> = serde_json::from_slice(&worker.stdout).unwrap();
    let tool = query(&["run", "printers"]);
    assert!(tool.status.success());
    let answer: serde_json::Value = serde_json::from_slice(&tool.stdout).unwrap();
    let expected: Vec<_> =
        pdfcraft_engine::print::spool::printer_queues().into_iter().map(|p| serde_json::json!({"name": p.name, "default": p.default})).collect();
    let actual: Vec<_> =
        answer["printers"].as_array().unwrap().iter().map(|p| serde_json::json!({"name": p["name"], "default": p["default"]})).collect();
    assert_eq!(actual, expected);
    // Windows can list queues while PdfCraft's required print DLLs are absent.
    if !expected.is_empty() {
        let listed: Vec<_> = queues.into_iter().map(|(name, default)| serde_json::json!({"name": name, "default": default})).collect();
        assert_eq!(listed, expected);
    }
    if let Some(printer) = answer["printers"].as_array().unwrap().iter().find(|p| p["two_sided"].is_boolean()) {
        let capability = query(&["--craft-printer-query", "two-sided", printer["name"].as_str().unwrap()]);
        assert!(capability.status.success());
        assert_eq!(serde_json::from_slice::<Option<bool>>(&capability.stdout).unwrap(), printer["two_sided"].as_bool());
    }
    if let Some(printer) = answer["printers"].as_array().unwrap().iter().find(|p| p["name"] == "Microsoft Print to PDF") {
        let name = printer["name"].as_str().unwrap();
        let expected = pdfcraft_engine::print::spool::printer_paper_info(name);
        let output = query(&["--craft-printer-query", "paper-info", name]);
        assert!(output.status.success());
        let actual: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(actual[0], serde_json::json!(expected.0));
        if let Some((_, (w, h))) = expected.1 {
            assert!((actual[1][0].as_f64().unwrap() - w).abs() < 0.01);
            assert!((actual[1][1].as_f64().unwrap() - h).abs() < 0.01);
        } else {
            assert!(actual[1].is_null());
        }
    }
}
