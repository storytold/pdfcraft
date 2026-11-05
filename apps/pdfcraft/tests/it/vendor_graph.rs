//! Dependency-graph verification for the egui 0.37 window stack.
//!
//! The `winit_*` modules next to this one read the retained `vendor/winit` /
//! `vendor/egui-wgpu` copies. They are provenance for W1–W13 and E2+E3, not proof of what the app
//! runs. egui-wgpu is pinned to an immutable storytold git revision in the root manifest. winit is
//! an interim exception (W28): its tablet delta is not on `storytold/winit` yet, so the pinned
//! fork revision is vendored with the delta in `vendor/winit-tablet` and patched by path — the
//! reviewed tree inside this repository is then the runtime source, and the tests below check
//! exactly that. The retained copies stay evidence-only.

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
fn assert_git_resolved(name: &str, version: &str, url: &str, rev: &str) {
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

/// The winit exception (W28): the resolved package must be a pure path patch on the audited
/// vendored tree, which carries the pinned fork revision in `.cargo_vcs_info.json`. No `source`
/// line means exactly that tree builds; a git source would silently drop the W28 delta review.
fn assert_winit_path_resolved(version: &str, rev: &str) {
    let package = locked("winit");
    let expected_version = format!("version = \"{version}\"");
    assert!(package.lines().any(|line| line.trim() == expected_version.as_str()), "winit must resolve to {version}: {package}");
    assert!(
        !package.lines().any(|line| line.trim().starts_with("source = ") || line.trim().starts_with("checksum = ")),
        "winit must resolve to the vendored path patch (no git/registry source): {package}"
    );

    let manifest = std::fs::read_to_string(root().join("Cargo.toml")).expect("root Cargo.toml");
    assert!(manifest.contains("winit = { path = \"vendor/winit-tablet\" }"), "the root manifest must patch winit from vendor/winit-tablet");
    assert!(
        !manifest.contains("winit = { git = \"https://github.com/storytold/winit.git\""),
        "winit must not go back to an unpatched git pin while W28 is pending"
    );

    let vcs = std::fs::read_to_string(root().join("vendor/winit-tablet/.cargo_vcs_info.json")).expect("vendor/winit-tablet provenance");
    assert!(vcs.contains(rev), "vendor/winit-tablet must be based on the pinned fork revision {rev}: {vcs}");
}

/// Regression test: the resolved window stack points at the reviewed forks and the retained vendor provenance
/// files remain.
#[test]
fn vendor_provenance_window_stack_graph_is_resolved() {
    // W28 interim exception: winit patches from the audited vendored tree.
    assert_winit_path_resolved("0.30.13", "a517b3846ee8457d84ee4975110bfda6797c44dc");
    assert_git_resolved("egui-wgpu", "0.37.0", "https://github.com/storytold/egui.git", "6d518fc799681571963842f95b55a6b68431b15a");
    // Retained provenance: the audited copies and their recorded W1–W13 / E2+E3 rows.
    let readme = std::fs::read_to_string(root().join("vendor/README.md")).expect("vendor/README.md");
    for marker in ["W1–W13", "surface_fit", "catch_errors", "e9809ef54b18499bb4f2cac945719ecc2a61061b", "W28"] {
        assert!(readme.contains(marker), "vendor/README.md lost window-stack provenance: {marker}");
    }
    let vcs = std::fs::read_to_string(root().join("vendor/winit/.cargo_vcs_info.json")).expect("vendor/winit provenance");
    assert!(vcs.contains("e9809ef54b18499bb4f2cac945719ecc2a61061b"), "vendor/winit keeps its upstream base");
    for path in ["vendor/winit/src/platform_impl", "vendor/winit-tablet/src/platform_impl", "vendor/egui-wgpu/src/winit.rs"] {
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
