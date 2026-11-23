//! Explicit end-to-end check of Cargo's build-script wiring, not just the validator function.
#![cfg(target_os = "linux")]

use std::io::{Read, Seek, SeekFrom};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Check the real Cargo wiring across custom, invalid and default build environments.
#[test]
#[ignore = "invokes Cargo; run explicitly after building the UI crate"]
fn build_script_rejects_invalid_overrides_and_tracks_environment_changes() {
    for value in [Some("Example/PdfCraft"), Some("not-a-repository"), None] {
        let mut stdout = tempfile::tempfile().unwrap();
        let mut stderr = tempfile::tempfile().unwrap();
        let mut command = Command::new(env!("CARGO"));
        command.args(["check", "--locked", "--quiet", "--lib", "--message-format=json", "--manifest-path"]);
        command.arg(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
        command.env_remove("PDFCRAFT_UPDATE_REPO");
        if let Some(value) = value {
            command.env("PDFCRAFT_UPDATE_REPO", value);
        }
        let mut child = command.stdout(Stdio::from(stdout.try_clone().unwrap())).stderr(Stdio::from(stderr.try_clone().unwrap())).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(600);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Cargo did not exit for {value:?}");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        stdout.seek(SeekFrom::Start(0)).unwrap();
        stderr.seek(SeekFrom::Start(0)).unwrap();
        let mut output = String::new();
        let mut errors = String::new();
        stdout.read_to_string(&mut output).unwrap();
        stderr.read_to_string(&mut errors).unwrap();
        if value == Some("not-a-repository") {
            assert!(!status.success(), "an invalid override must fail the actual build: {output}");
            assert!(
                errors.contains("PDFCRAFT_UPDATE_REPO must be a GitHub owner/repo")
                    || output.contains("PDFCRAFT_UPDATE_REPO must be a GitHub owner/repo"),
                "{errors}\n{output}"
            );
        } else {
            assert!(status.success(), "{value:?}: {errors}");
            let expected = value.unwrap_or("storytold/pdfcraft");
            let emitted = output.lines().filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok()).any(|message| {
                message["reason"] == "build-script-executed"
                    && message["env"].as_array().is_some_and(|env| env.iter().any(|pair| pair[0] == "PDFCRAFT_UPDATE_REPO" && pair[1] == expected))
            });
            assert!(emitted, "Cargo must receive the selected repository after each environment change: {value:?}: {output}");
        }
    }
}
