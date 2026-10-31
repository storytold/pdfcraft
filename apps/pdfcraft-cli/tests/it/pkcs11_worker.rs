//! Exercise the actual executable's private mode without a GUI, provider, or token.
#![cfg(target_os = "linux")]
use std::io::Write;
use std::process::{Command, Stdio};

fn worker(input: &[u8]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pdfcraft-cli"))
        .arg("--pdfcraft-pkcs11-worker-v1")
        .env("PDFCRAFT_PKCS11_MODULE", "/nonexistent/pdfcraft-test-provider.so")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}
#[test]
fn worker_discovery_failure_is_framed_without_normal_cli_output() {
    let json = br#"{"List":{"decryption":true}}"#;
    let mut input = Vec::new();
    input.extend_from_slice(&(json.len() as u32).to_be_bytes());
    input.extend_from_slice(json);
    input.extend_from_slice(&0u32.to_be_bytes());
    let output = worker(&input);
    assert!(output.status.success(), "the worker returned a structured operation failure");
    assert!(output.stderr.is_empty());
    assert_eq!(output.stdout[0], 1);
    assert_eq!(u32::from_be_bytes(output.stdout[1..5].try_into().unwrap()) as usize, output.stdout.len() - 5);
    assert!(String::from_utf8_lossy(&output.stdout[5..]).contains("no PKCS #11 module"));
}
#[test]
fn worker_refuses_truncated_oversized_and_invalid_requests() {
    for input in [&b""[..], &b"\x00\x10\x00\x01"[..], &b"\x00\x00\x00\x01?\x00\x00\x00\x00"[..]] {
        let output = worker(input);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn worker_stops_if_its_owning_process_exits() {
    use std::io::BufRead;
    let mut owner = Command::new("/bin/sh")
        .args(["-c", "exec 3<&0; PDFCRAFT_PKCS11_PARENT=$$ \"$PDFCRAFT_TEST_WORKER\" --pdfcraft-pkcs11-worker-v1 <&3 & worker=$!; printf '%s\\n' \"$worker\"; wait"])
        .env("PDFCRAFT_TEST_WORKER", env!("CARGO_BIN_EXE_pdfcraft-cli"))
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    // Child::wait closes its stdin field. Hold this pipe separately until the assertion.
    let _keep_input_open = owner.stdin.take().unwrap();
    let mut pid = String::new();
    std::io::BufReader::new(owner.stdout.take().unwrap()).read_line(&mut pid).unwrap();
    let pid: i32 = pid.trim().parse().unwrap();
    let status_path = format!("/proc/{pid}/stat");
    let alive = || {
        std::fs::read_to_string(&status_path).ok().is_some_and(|s| s.rsplit_once(')').is_some_and(|(_, state)| !state.trim_start().starts_with('Z')))
    };
    std::thread::sleep(std::time::Duration::from_millis(40));
    assert!(alive(), "the worker must be waiting for its input before its owner exits");
    owner.kill().unwrap();
    owner.wait().unwrap();
    // Keep stdin open: EOF must not be what makes this worker stop.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while alive() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let survived = alive();
    if survived {
        let _ = rustix::process::kill_process(rustix::process::Pid::from_raw(pid).unwrap(), rustix::process::Signal::KILL);
    }
    assert!(!survived, "the signing helper must not survive the process that owned it");
}

#[test]
fn worker_rejects_missing_decryption_serial_before_provider_access() {
    for serial in ["", "   ", "\t\r\n", "\u{2003}\u{a0}"] {
        let json = serde_json::to_vec(&serde_json::json!({"Decrypt": {
            "identity": {"certificate": [], "chain": [], "label": null, "serial": serial}, "ciphertext": []
        }}))
        .unwrap();
        for pin in [b"disposable-test-pin".as_slice(), b"".as_slice()] {
            let mut input = Vec::new();
            input.extend_from_slice(&(json.len() as u32).to_be_bytes());
            input.extend_from_slice(&json);
            input.extend_from_slice(&(pin.len() as u32).to_be_bytes());
            input.extend_from_slice(pin);
            let output = worker(&input);
            assert!(output.status.success());
            assert!(output.stderr.is_empty());
            assert_eq!(output.stdout.first(), Some(&2), "missing serial fails before the nonexistent provider is accessed");
            assert!(!String::from_utf8_lossy(&output.stdout).contains("disposable-test-pin"));
        }
    }
}
