//! Keep the shared reader pinned while preserving the app-owned inspector boundary.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

const SHARED_REV: &str = "9eb02be84f21898e247daeb2af5827c9fe85a3a0";
const PACKAGES: &[(&str, &str)] = &[
    ("craft-pdf", "0.1.0"),
    ("hayro", "0.7.1"),
    ("hayro-interpret", "0.7.0"),
    ("hayro-syntax", "0.7.2"),
    ("hayro-jbig2", "0.3.0"),
    ("hayro-cmap", "0.1.0"),
];

pub fn run(root: &Path) -> Result<(), String> {
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(root)
        .args(["metadata", "--locked", "--format-version", "1"])
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!("cargo metadata: {}", String::from_utf8_lossy(&output.stderr)));
    }
    let metadata: Value = serde_json::from_slice(&output.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    validate(root, &metadata)?;
    println!("PDF reader: craft-pdf and Hayro packages pinned to {SHARED_REV}; local lopdf inspector retained");
    Ok(())
}

fn validate(root: &Path, metadata: &Value) -> Result<(), String> {
    let packages = metadata.get("packages").and_then(Value::as_array).ok_or("cargo metadata has no packages")?;
    let one_package = |name: &str| -> Result<&Value, String> {
        let matches: Vec<_> = packages.iter().filter(|p| p.get("name").and_then(Value::as_str) == Some(name)).collect();
        if matches.len() != 1 {
            return Err(format!("{name}: expected one resolved package, found {}", matches.len()));
        }
        matches.first().copied().ok_or_else(|| format!("missing {name}"))
    };
    let source = format!("git+https://github.com/storytold/craft-libs?rev={SHARED_REV}#{SHARED_REV}");
    for &(name, version) in PACKAGES {
        let package = one_package(name)?;
        if package.get("version").and_then(Value::as_str) != Some(version) || package.get("source").and_then(Value::as_str) != Some(source.as_str()) {
            return Err(format!("{name}: expected {version} from {source}; got {package}"));
        }
    }
    let inspector = one_package("lopdf")?;
    let manifest = root.join("vendor/lopdf/Cargo.toml");
    if inspector.get("version").and_then(Value::as_str) != Some("0.45.0")
        || inspector.get("source") != Some(&Value::Null)
        || inspector.get("manifest_path").and_then(Value::as_str).map(Path::new) != Some(manifest.as_path())
    {
        return Err("lopdf: expected the existing local vendor/lopdf inspector at version 0.45.0".into());
    }
    let nodes = metadata.get("resolve").and_then(|r| r.get("nodes")).and_then(Value::as_array).ok_or("cargo metadata has no resolved nodes")?;
    let shared_id = one_package("craft-pdf")?.get("id").and_then(Value::as_str).ok_or("craft-pdf has no package id")?;
    let node = nodes.iter().find(|n| n.get("id").and_then(Value::as_str) == Some(shared_id)).ok_or("craft-pdf has no resolved node")?;
    let features = node.get("features").and_then(Value::as_array).ok_or("craft-pdf has no resolved features")?;
    let actual: BTreeSet<_> = features.iter().filter_map(Value::as_str).collect();
    let expected: BTreeSet<_> = ["read", "interpret", "raster"].into_iter().collect();
    if !expected.is_subset(&actual) {
        return Err(format!("craft-pdf: expected reader features {expected:?}; got {features:?}"));
    }
    Ok(())
}
