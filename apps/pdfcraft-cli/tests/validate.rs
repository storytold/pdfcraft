//! `validate` checks a file against the Arlington PDF Model, fails when it has errors, and says
//! what `structure_validate` says through `run --script`.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

fn pdf(objects: &[&str]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

fn good() -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> >>",
    ])
}

/// The page has no /Resources (required), and the catalog's /PageLayout isn't a layout.
fn broken() -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /PageLayout /Spiral >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>",
    ])
}

#[test]
fn validate_fails_on_errors_and_agrees_with_the_tool() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-validate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("good.pdf"), good()).unwrap();
    std::fs::write(dir.join("broken.pdf"), broken()).unwrap();

    let out = Command::new(BIN).arg("validate").arg(dir.join("good.pdf")).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!((report["valid"].clone(), report["errors"].clone()), (true.into(), 0.into()), "{report}");

    let out = Command::new(BIN).arg("validate").arg(dir.join("broken.pdf")).output().unwrap();
    assert!(!out.status.success(), "errors must fail validate");
    // A reader that closes the pipe before the report is written doesn't turn that into a pass.
    let mut child = Command::new(BIN)
        .arg("validate")
        .arg(dir.join("broken.pdf"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    assert!(!child.wait().unwrap().success(), "a closed pipe must not make validate succeed");
    assert!(String::from_utf8_lossy(&out.stderr).contains("2 error(s)"), "{}", String::from_utf8_lossy(&out.stderr));
    let cli: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let rules: Vec<(&str, &str, Option<u64>)> = cli["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["severity"] == "error")
        .map(|f| (f["rule"].as_str().unwrap(), f["key"].as_str().unwrap(), f["page"].as_u64()))
        .collect();
    assert!(rules.contains(&("required", "Resources", Some(1))), "{cli}");
    assert!(rules.contains(&("value", "PageLayout", None)), "{cli}");

    // --severity leaves the less severe findings out of the list, not out of the counts.
    let out = Command::new(BIN).args(["validate", "--severity", "error"]).arg(dir.join("broken.pdf")).output().unwrap();
    let errors_only: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(errors_only["findings"].as_array().unwrap().iter().all(|f| f["severity"] == "error"));
    assert_eq!(errors_only["warnings"], cli["warnings"]);
    let out = Command::new(BIN).args(["validate", "--severity", "loud"]).arg(dir.join("broken.pdf")).output().unwrap();
    assert!(!out.status.success() && out.stdout.is_empty());
    // An option left without its value fails, on a valid file too, instead of falling back.
    for option in ["--severity", "--password"] {
        let out = Command::new(BIN).arg("validate").arg(dir.join("good.pdf")).arg(option).output().unwrap();
        assert!(!out.status.success() && out.stdout.is_empty(), "{option}");
        assert!(String::from_utf8_lossy(&out.stderr).contains(&format!("{option} needs a value")), "{}", String::from_utf8_lossy(&out.stderr));
    }

    // The tool reports the same.
    let steps = serde_json::json!([
        { "tool": "doc_open", "args": { "path": "broken.pdf" } },
        { "tool": "structure_validate", "args": { "doc": 1 } },
    ]);
    std::fs::write(dir.join("steps.json"), steps.to_string()).unwrap();
    let out = Command::new(BIN).args(["run", "--script"]).arg(dir.join("steps.json")).arg("--root").arg(&dir).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let tool = serde_json::Deserializer::from_str(&stdout).into_iter::<serde_json::Value>().last().unwrap().unwrap();
    for key in ["valid", "errors", "warnings", "infos", "findings", "not_checked"] {
        assert_eq!(tool[key], cli[key], "{key}: {tool}");
    }
    std::fs::remove_dir_all(&dir).ok();
}
