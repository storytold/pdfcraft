//! Every command rejects misspelled long options before opening or writing a file.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

#[test]
fn non_edit_commands_reject_unknown_options_before_io() {
    for args in [
        vec!["info", "missing.pdf", "--passwrod", "secret"],
        vec!["text", "missing.pdf", "--pgae", "1"],
        vec!["combine", "a.pdf", "b.pdf", "--ot", "out.pdf"],
        vec!["extract", "missing.pdf", "--pgaes", "1", "--out", "out.pdf"],
        vec!["split", "missing.pdf", "--ever", "2"],
        vec!["render", "missing.pdf", "--dppi", "96", "--out", "out.png"],
        vec!["check-one", "missing.pdf", "--edti"],
        vec!["check", "missing.pdf", "--timout", "2"],
        vec!["run", "doc_open", "--roto", "/tmp"],
        vec!["ui", "--contro", "control.json", "state"],
    ] {
        let unknown = args.iter().find(|arg| arg.starts_with("--")).expect("test option");
        let out = Command::new(BIN).args(&args).output().expect("run cli");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?} unexpectedly succeeded");
        assert!(stderr.contains("unknown option") && stderr.contains(unknown), "{args:?}: {stderr}");
    }
}
