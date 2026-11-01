//! The robustness harness visits each directory once, including symlink aliases and cycles.
//! Its `--timeout` and `--dpi` options are checked before anything is scanned.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

struct Workdir(PathBuf);

impl Workdir {
    fn new(test: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("pdfcraft-cli-check-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("input/nested")).unwrap();
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 20 30] >>",
            "<< /Type /Page /Parent 2 0 R >>",
        ];
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
        std::fs::write(dir.join("input/a.pdf"), &bytes).unwrap();
        std::fs::write(dir.join("input/nested/b.PDF"), &bytes).unwrap();
        Self(dir)
    }

    fn check(&self, inputs: &[&Path]) -> Vec<serde_json::Value> {
        let report = self.0.join("report.json");
        let mut child = Command::new(BIN)
            .arg("check")
            .args(inputs)
            .args(["--dpi", "36", "--timeout", "2", "--json"])
            .arg(&report)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!("check did not finish scanning directories: {}", String::from_utf8_lossy(&output.stderr));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap()
    }
}

impl Drop for Workdir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn assert_two_valid_pdfs(results: &[serde_json::Value]) {
    assert_eq!(results.len(), 2, "each PDF should be checked once: {results:?}");
    for result in results {
        assert_eq!(result["status"], "ok", "{result}");
        assert_eq!(result["pages"], 1, "{result}");
    }
    assert!(results[0]["file"].as_str().unwrap().ends_with("a.pdf"));
    assert!(results[1]["file"].as_str().unwrap().ends_with("b.PDF"));
}

#[test]
fn check_scans_overlapping_directories_once() {
    let dir = Workdir::new("overlap");
    let input = dir.0.join("input");
    assert_two_valid_pdfs(&dir.check(&[&input, &input, &input.join("nested")]));
}

#[cfg(unix)]
#[test]
fn check_scans_a_directory_symlink_cycle_once() {
    let dir = Workdir::new("cycle");
    let input = dir.0.join("input");
    std::os::unix::fs::symlink(".", input.join("again")).unwrap();
    assert_two_valid_pdfs(&dir.check(&[&input]));
}

#[cfg(unix)]
#[test]
fn check_follows_directory_and_file_symlinks() {
    let dir = Workdir::new("aliases");
    let input = dir.0.join("input");
    let alias = dir.0.join("alias");
    std::os::unix::fs::symlink(&input, &alias).unwrap();
    assert_two_valid_pdfs(&dir.check(&[&input, &alias]));

    let file_alias = dir.0.join("page.pdf");
    std::os::unix::fs::symlink(input.join("a.pdf"), &file_alias).unwrap();
    let results = dir.check(&[&file_alias]);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["status"], "ok");
    assert_eq!(results[0]["file"], file_alias.to_string_lossy().as_ref());
}

/// `--timeout` and `--dpi` are checked before any input is scanned: a zero timeout used to
/// make every file time out at once, and a dpi that is not a positive finite number used to
/// reach each child, which printed nothing and was reported as a crash per file.
#[test]
fn a_bad_timeout_or_dpi_is_refused_before_scanning() {
    let out = Command::new(BIN).args(["check", "no-such-input", "--timeout", "0"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(out.stderr, b"pdfcraft-cli: bad --timeout: expected a positive whole number of seconds, got \"0\"\n");
    for value in ["abc", "0", "-3", "NaN"] {
        let out = Command::new(BIN).args(["check", "no-such-input", "--dpi", value]).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "--dpi {value}");
        assert_eq!(
            out.stderr,
            format!("pdfcraft-cli: bad --dpi: expected a positive number of dots per inch, got {value:?}\n").as_bytes(),
            "--dpi {value}"
        );
    }
}
