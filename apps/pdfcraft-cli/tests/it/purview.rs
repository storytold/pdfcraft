use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("pdfcraft-cli-purview-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn protected(&self) -> PathBuf {
        let mut bytes = pdfcraft_engine::Session::new().create_from_text("Protected", "Synthetic marker").unwrap().to_vec();
        bytes.extend_from_slice(b"\n% /MicrosoftIRMServices /EncryptedPayload\n");
        let path = self.0.join("protected.pdf");
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn ordinary(&self) -> PathBuf {
        let path = self.0.join("ordinary.pdf");
        std::fs::write(&path, pdfcraft_engine::Session::new().create_from_text("Ordinary", "Safe PDF").unwrap().as_slice()).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(BIN).args(args).env_remove("PDFCRAFT_PURVIEW_APP_ID").env_remove("PDFCRAFT_PURVIEW_HELPER").output().unwrap()
}

fn assert_purview_failure(output: &Output) {
    assert!(!output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("Microsoft Purview") || error.contains("MIP SDK"), "{error}");
}

#[test]
fn every_cli_plaintext_path_rejects_a_mip_shaped_pdf() {
    let fixture = Fixture::new();
    let protected = fixture.protected();
    let ordinary = fixture.ordinary();
    let rendered = fixture.0.join("render.png");
    let edited = fixture.0.join("edited.pdf");
    let extracted = fixture.0.join("extracted.pdf");
    let combined = fixture.0.join("combined.pdf");
    let split_dir = fixture.0.join("split");
    let path = protected.to_str().unwrap();
    let ordinary = ordinary.to_str().unwrap();

    for args in [
        vec!["info", path],
        vec!["text", path],
        vec!["audit", path],
        vec!["render", path, "--out", rendered.to_str().unwrap()],
        vec!["edit", path, "--out", edited.to_str().unwrap(), "--rotate", "1:90"],
        vec!["extract", path, "--pages", "1", "--out", extracted.to_str().unwrap()],
        vec!["split", path, "--every", "1", "--out-dir", split_dir.to_str().unwrap()],
        vec!["combine", path, ordinary, "--out", combined.to_str().unwrap()],
    ] {
        assert_purview_failure(&run(&args));
    }
    assert!(!rendered.exists());
    assert!(!edited.exists());
    assert!(!extracted.exists());
    assert!(!combined.exists());
    assert!(!split_dir.exists());
}

#[test]
fn check_records_mip_open_failure_without_rendering_the_payload() {
    let fixture = Fixture::new();
    let protected = fixture.protected();
    let output = run(&["check-one", protected.to_str().unwrap(), "--edit"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["pages"], 0);
    assert!(report["status"].as_str().unwrap().starts_with("open-error:"));
    assert!(report["detail"].as_str().unwrap().contains("Microsoft Purview"));
}

#[test]
fn pfile_extension_is_authorized_before_pdf_inspection() {
    let fixture = Fixture::new();
    let pdf = fixture.ordinary();
    let pfile = fixture.0.join("ordinary.pfile");
    std::fs::copy(&pdf, &pfile).unwrap();
    let output = run(&["info", pfile.to_str().unwrap()]);
    assert_purview_failure(&output);
    assert!(output.stdout.is_empty());
}
