//! `pdfcraft-cli print --wsd`: the confirmation and address rules need no network; the full print
//! runs against a loopback fake printer in builds with the `network-print` feature.
#![cfg(feature = "network-print")]

use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

fn run(args: &[&str]) -> Output {
    Command::new(BIN).args(args).output().unwrap()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("pdfcraft-cli-print-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn printing_needs_yes_and_a_printer_address() {
    let dir = tmp("flags");
    let file = dir.join("a.pdf");
    std::fs::write(&file, b"%PDF-1.4\n%%EOF").unwrap();
    let file = file.to_str().unwrap();
    let o = run(&["print", "--wsd", "http://127.0.0.1:9/x", file]);
    assert!(!o.status.success() && err(&o).contains("--yes"), "{}", err(&o));
    let o = run(&["print", file, "--yes"]);
    assert!(!o.status.success() && err(&o).contains("--wsd"), "{}", err(&o));
    let o = run(&["print", "--wsd", "http://127.0.0.1:9/x", "--yes"]);
    assert!(!o.status.success() && err(&o).contains("exactly one FILE"), "{}", err(&o));
    for bad in [
        vec!["print", "--wsd"],
        vec!["print", "--wsd", "http://127.0.0.1/", "--wsd", "http://127.0.0.1/", file, "--yes"],
        vec!["print", "--bogus"],
        vec!["print", "--wsd", "http://127.0.0.1/", file, "--yes", "--copies", "many"],
    ] {
        let o = run(&bad);
        assert!(!o.status.success() && o.stdout.is_empty(), "{bad:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hostnames_and_public_addresses_are_refused_before_any_connection() {
    let dir = tmp("urls");
    let file = dir.join("a.pdf");
    std::fs::write(&file, b"%PDF-1.4\n%%EOF").unwrap();
    let file = file.to_str().unwrap();
    for url in ["http://printer.example.com/", "http://8.8.8.8/", "https://127.0.0.1/", "http://localhost/"] {
        let o = run(&["print", "--wsd", url, file, "--yes"]);
        assert!(!o.status.success());
        assert!(err(&o).contains("numeric") || err(&o).contains("loopback") || err(&o).contains("http://"), "{url}: {}", err(&o));
        let o = run(&["print", "--wsd", url, "--info"]);
        assert!(!o.status.success(), "{url}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

mod live {
    use super::*;
    use craft_print::wsd::testing::{FakeConfig as Config, FakePrinter, printer_server};

    #[test]
    fn a_pdf_prints_to_a_loopback_printer_and_the_report_is_json() {
        let printer = FakePrinter::new(Config::default());
        let server = printer_server(printer.clone());
        let dir = tmp("live");
        // A real one-page PDF from the tool set, so the document route opens it.
        let blank = dir.join("blank.pdf");
        let script = dir.join("steps.json");
        std::fs::write(
            &script,
            format!(
                r#"[{{"tool":"doc_create","args":{{"from":"blank","pages":1}}}},{{"tool":"doc_save","args":{{"doc":1,"path":{:?}}}}}]"#,
                blank.to_str().unwrap()
            ),
        )
        .unwrap();
        let saved = run(&["run", "--script", script.to_str().unwrap()]);
        assert!(saved.status.success(), "{}", err(&saved));
        let url = server.url("/print");
        let o = run(&["print", "--wsd", &url, blank.to_str().unwrap(), "--yes", "--copies", "2", "--wait", "20", "--job-name", "cli test"]);
        assert!(o.status.success(), "{}\n{}", err(&o), String::from_utf8_lossy(&o.stdout));
        let report: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!((report["outcome"].as_str(), report["format"].as_str()), (Some("completed"), Some("application/pdf")), "{report}");
        assert!(printer.job(1).unwrap().ticket.contains("cli test"));
        let info = run(&["print", "--wsd", &url, "--info"]);
        assert!(info.status.success(), "{}", err(&info));
        let v: serde_json::Value = serde_json::from_slice(&info.stdout).unwrap();
        assert_eq!(v["capabilities"]["formats"][0], "application/pdf");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The printer accepts the document but the job never ends within the wait: the command still exits
    /// non-zero, but says the printer has the document so the person checks before sending it again.
    #[test]
    fn a_job_still_running_is_reported_as_accepted_not_as_a_failure() {
        use craft_print::wsd::testing::Ending;
        let printer = FakePrinter::new(Config { ending: Ending::Hangs, ..Default::default() });
        let server = printer_server(printer.clone());
        let dir = tmp("hangs");
        let file = dir.join("blank.pdf");
        let script = dir.join("steps.json");
        std::fs::write(
            &script,
            format!(
                r#"[{{"tool":"doc_create","args":{{"from":"blank","pages":1}}}},{{"tool":"doc_save","args":{{"doc":1,"path":{:?}}}}}]"#,
                file.to_str().unwrap()
            ),
        )
        .unwrap();
        let saved = run(&["run", "--script", script.to_str().unwrap()]);
        assert!(saved.status.success(), "{}", err(&saved));
        let o = run(&["print", "--wsd", &server.url("/print"), file.to_str().unwrap(), "--yes", "--wait", "1"]);
        assert!(!o.status.success());
        let message = err(&o);
        assert!(message.contains("accepted the document") && message.contains("check the printer before sending again"), "{message}");
        assert!(!message.contains("did not complete successfully"), "{message}");
        let report: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(report["outcome"], "still_running", "{report}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
