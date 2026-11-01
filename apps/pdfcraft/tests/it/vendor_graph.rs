//! Dependency-graph verification for the egui 0.37 window stack.
//!
//! The twelve `winit_*` modules next to this one read the retained `vendor/winit` /
//! `vendor/egui-wgpu` copies. They are provenance for W1–W13 and E2+E3, not proof of what the app
//! runs: the root manifest pins both crates to immutable storytold git revisions, and the retained
//! copies are not path patches. These tests check the resolved graph and the retained evidence
//! instead of treating the copies as the runtime source. The winit fork's own W17 tests own patch
//! behaviour.

/// Return the workspace root above the PdfCraft app crate.
fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every `[[package]]` block in `Cargo.lock` whose `name` is exactly `name`.
fn locked_packages(name: &str) -> Vec<String> {
    let lock = std::fs::read_to_string(root().join("Cargo.lock")).expect("Cargo.lock");
    let needle = format!("name = \"{name}\"");
    lock.split("[[package]]").filter(|package| package.lines().any(|line| line.trim() == needle.as_str())).map(str::to_owned).collect()
}

/// The one resolved package called `name`. A duplicate name is a lockfile problem, not something
/// to resolve by taking the first match.
fn locked(name: &str) -> String {
    let mut packages = locked_packages(name);
    assert_eq!(packages.len(), 1, "Cargo.lock must contain exactly one `{name}` package, found {}", packages.len());
    packages.remove(0)
}

/// The resolved package must be the exact canonical storytold git source at the reviewed
/// 40-character revision, including the `Cargo.lock` URL and fragment. A path source (no `source`
/// line) is refused: it would make the graph depend on a developer's checkout. A published path
/// patch is refused as well.
fn assert_resolved(name: &str, version: &str, url: &str, rev: &str) {
    let package = locked(name);
    let expected_version = format!("version = \"{version}\"");
    assert!(package.lines().any(|line| line.trim() == expected_version.as_str()), "{name} must resolve to {version}: {package}");
    let sources: Vec<&str> = package.lines().filter_map(|line| line.trim().strip_prefix("source = ")).collect();
    assert_eq!(sources.len(), 1, "{name} must have exactly one `source` in Cargo.lock; a local path source is not accepted: {package}");
    let expected_source = format!("\"git+{url}?rev={rev}#{rev}\"");
    assert_eq!(sources[0], expected_source.as_str(), "{name} must resolve to the exact canonical git URL, revision and fragment, not {}", sources[0]);

    let manifest = std::fs::read_to_string(root().join("Cargo.toml")).expect("root Cargo.toml");
    assert!(!manifest.contains(&format!("{name} = {{ path = \"vendor/")), "vendor/{name} cannot stay a published path patch");
}

/// Regression test: the resolved window stack points at the reviewed forks and the retained vendor provenance
/// files remain.
#[test]
fn vendor_provenance_window_stack_graph_is_resolved() {
    assert_resolved("winit", "0.30.13", "https://github.com/storytold/winit.git", "a517b3846ee8457d84ee4975110bfda6797c44dc");
    assert_resolved("egui-wgpu", "0.37.0", "https://github.com/storytold/egui.git", "6d518fc799681571963842f95b55a6b68431b15a");
    // Retained provenance: the audited copies and their recorded W1–W13 / E2+E3 rows.
    let readme = std::fs::read_to_string(root().join("vendor/README.md")).expect("vendor/README.md");
    for marker in ["W1–W13", "surface_fit", "catch_errors", "e9809ef54b18499bb4f2cac945719ecc2a61061b"] {
        assert!(readme.contains(marker), "vendor/README.md lost window-stack provenance: {marker}");
    }
    let vcs = std::fs::read_to_string(root().join("vendor/winit/.cargo_vcs_info.json")).expect("vendor/winit provenance");
    assert!(vcs.contains("e9809ef54b18499bb4f2cac945719ecc2a61061b"), "vendor/winit keeps its upstream base");
    for path in ["vendor/winit/src/platform_impl", "vendor/egui-wgpu/src/winit.rs"] {
        assert!(root().join(path).exists(), "retain {path} as provenance for the pinned fork");
    }
}

/// Renderer, security and PostScript vendors are unrelated to the UI port and must stay path patches.
#[test]
fn renderer_and_security_vendors_remain() {
    let manifest = std::fs::read_to_string(root().join("Cargo.toml")).expect("root Cargo.toml");
    for vendor in [
        "vendor/hayro",
        "vendor/hayro-interpret",
        "vendor/hayro-syntax",
        "vendor/hayro-cmap",
        "vendor/hayro-jbig2",
        "vendor/hayro-postscript",
        "vendor/lopdf",
        "vendor/vello_common",
    ] {
        assert!(manifest.contains(&format!("path = \"{vendor}\"")), "{vendor} must stay a path patch");
    }
}
