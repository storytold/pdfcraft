//! `cargo xtask vendor-test`: the unit tests of the patched crates under `vendor/`.
//!
//! The workspace excludes `vendor/`, so `cargo xtask test` never builds these tests, and many of
//! the patches in `vendor/README.md` name a test there as their cover. Cargo also refuses
//! `cargo test -p <crate>` for a crate outside the workspace that has dev-dependencies, and
//! `cargo test --manifest-path vendor/<crate>/Cargo.toml` on its own resolves the crates.io
//! versions of the *other* vendored crates, which lack our patches (hayro-syntax then fails to
//! build against crates.io hayro-jbig2). So this command picks, for each crate patched in the
//! root `[patch.crates-io]`:
//!
//! - `cargo test --locked -p <name>@<version>` when it has no dev-dependencies: the workspace
//!   graph and lock file, patches included;
//! - otherwise `cargo test --manifest-path vendor/<dir>/Cargo.toml` with every root patch passed
//!   again through `--config`, so its own graph uses the vendored copies too. That rewrites the
//!   crate's own `Cargo.lock`, which is restored afterwards, pass or fail.
//!
//! Only library and integration tests run, not doctests. A few upstream tests can't run from this
//! repository; they are listed below with the reason, and skipped rather than reported as
//! failures.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

/// Vendored crates whose test targets don't build here: they include files the vendored copy
/// doesn't carry.
const UNBUILDABLE: &[(&str, &str)] = &[
    ("lopdf", "its reader tests include `assets/example.pdf`, which is not vendored"),
    ("vello_cpu", "its text tests include upstream's Roboto font, and font files are not committed here (AGENTS.md §1.4)"),
];

/// Upstream tests that read upstream's own test corpus (`../hayro-tests/downloads`), which is not
/// vendored. Skipped by exact name.
const NEEDS_UPSTREAM_CORPUS: &[(&str, &str)] =
    &[("hayro-syntax", "pdf::tests::pdf_version_header"), ("hayro-syntax", "pdf::tests::pdf_version_catalog")];

/// One crate the root `[patch.crates-io]` takes from `vendor/`.
#[derive(Debug, PartialEq, Eq)]
struct Patch {
    name: String,
    /// Relative to the repository root, as written in the patch entry.
    path: String,
}

/// How to run one vendored crate's tests.
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    /// `cargo test --locked -p name@version`.
    Workspace { spec: String },
    /// `cargo test --manifest-path …` with the patches repeated through `--config`.
    Standalone { manifest: PathBuf },
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    if let Some(arg) = args.first() {
        bail!("vendor-test takes no arguments (got `{arg}`)");
    }
    let root = crate::gates::root();
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).context("reading Cargo.toml")?;
    let patches = vendored_patches(&manifest)?;
    let mut failed = Vec::new();
    for patch in &patches {
        if let Some((_, why)) = UNBUILDABLE.iter().find(|(name, _)| *name == patch.name) {
            eprintln!("vendor-test: skipping {}: {why}", patch.name);
            continue;
        }
        let crate_manifest = root.join(&patch.path).join("Cargo.toml");
        let text = std::fs::read_to_string(&crate_manifest).with_context(|| format!("reading {}", crate_manifest.display()))?;
        let plan = plan_for(&patch.path, &text)?;
        if let Err(e) = run_plan(&root, patch, &plan, &patches) {
            eprintln!("vendor-test: {}: {e:#}", patch.name);
            failed.push(patch.name.clone());
        }
    }
    if !failed.is_empty() {
        bail!("vendored crate tests failed: {}", failed.join(", "));
    }
    Ok(())
}

/// The root manifest's `[patch.crates-io]` entries whose `path` is under `vendor/`, in order.
fn vendored_patches(root_manifest: &str) -> anyhow::Result<Vec<Patch>> {
    let doc: toml::Table = toml::from_str(root_manifest).context("parsing Cargo.toml")?;
    let Some(table) = doc.get("patch").and_then(|p| p.get("crates-io")).and_then(toml::Value::as_table) else {
        return Ok(Vec::new());
    };
    Ok(table
        .iter()
        .filter_map(|(name, entry)| {
            let path = entry.get("path")?.as_str()?;
            path.starts_with("vendor/").then(|| Patch { name: name.clone(), path: path.to_string() })
        })
        .collect())
}

/// How to test the crate at `dir` (relative to the root) whose manifest is `manifest`.
fn plan_for(dir: &str, manifest: &str) -> anyhow::Result<Plan> {
    let doc: toml::Table = toml::from_str(manifest).with_context(|| format!("parsing {dir}/Cargo.toml"))?;
    if has_dev_dependencies(&doc) {
        return Ok(Plan::Standalone { manifest: Path::new(dir).join("Cargo.toml") });
    }
    let package = doc.get("package").with_context(|| format!("{dir}/Cargo.toml has no [package]"))?;
    let name = package.get("name").and_then(toml::Value::as_str).with_context(|| format!("{dir}/Cargo.toml has no package name"))?;
    let version = package.get("version").and_then(toml::Value::as_str).with_context(|| format!("{dir}/Cargo.toml has no package version"))?;
    // `name@version`: the crates.io copy of a patched crate can stay in the graph under another
    // version (vello_common), which makes a bare `-p name` ambiguous.
    Ok(Plan::Workspace { spec: format!("{name}@{version}") })
}

/// Whether the manifest declares dev-dependencies, plain or under a `[target.'cfg(…)']` table.
fn has_dev_dependencies(doc: &toml::Table) -> bool {
    let non_empty = |v: Option<&toml::Value>| v.and_then(toml::Value::as_table).is_some_and(|t| !t.is_empty());
    non_empty(doc.get("dev-dependencies"))
        || doc.get("target").and_then(toml::Value::as_table).is_some_and(|targets| targets.values().any(|t| non_empty(t.get("dev-dependencies"))))
}

/// The `--config` value that patches `name` to `dir`, as a TOML string so any path is quoted
/// correctly. Forward slashes keep it readable on Windows.
fn patch_config(name: &str, dir: &Path) -> String {
    let path = dir.to_string_lossy().replace('\\', "/");
    format!("patch.crates-io.{name}.path={}", toml::Value::String(path))
}

/// The arguments after `--` that skip the upstream-corpus tests of `name`.
fn skips(name: &str) -> Vec<String> {
    NEEDS_UPSTREAM_CORPUS
        .iter()
        .filter(|(krate, _)| *krate == name)
        .flat_map(|(_, test)| ["--skip".to_string(), (*test).to_string()])
        .chain(NEEDS_UPSTREAM_CORPUS.iter().any(|(krate, _)| *krate == name).then(|| "--exact".to_string()))
        .collect()
}

fn run_plan(root: &Path, patch: &Patch, plan: &Plan, patches: &[Patch]) -> anyhow::Result<()> {
    let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    // Library and integration tests only: the doctests are upstream's usage examples, and some
    // read upstream's corpus (hayro-syntax's crate example loads `../hayro-tests/pdfs/…`).
    cmd.current_dir(root).args(["test", "--lib", "--tests"]);
    match plan {
        Plan::Workspace { spec } => {
            cmd.args(["--locked", "-p", spec]);
        }
        Plan::Standalone { manifest } => {
            cmd.arg("--manifest-path").arg(manifest);
            for p in patches {
                cmd.arg("--config").arg(patch_config(&p.name, &root.join(&p.path)));
            }
        }
    }
    let skip = skips(&patch.name);
    if !skip.is_empty() {
        cmd.arg("--").args(&skip);
    }
    eprintln!("$ cargo test ({}, {})", patch.name, if matches!(plan, Plan::Workspace { .. }) { "workspace" } else { "standalone" });
    let lock = match plan {
        Plan::Standalone { .. } => Some(root.join(&patch.path).join("Cargo.lock")),
        Plan::Workspace { .. } => None,
    };
    // A lock that exists but can't be read stops the run: treating it as absent would delete it
    // afterwards.
    let saved = match lock.as_deref() {
        Some(lock) if lock.exists() => Some(std::fs::read(lock).with_context(|| format!("reading {}", lock.display()))?),
        _ => None,
    };
    let status = cmd.status().with_context(|| format!("spawning cargo test for {}", patch.name));
    if let Some(lock) = &lock {
        restore_lock(lock, saved.as_deref())?;
    }
    let status = status?;
    if !status.success() {
        bail!("cargo test exited with {status}");
    }
    Ok(())
}

/// Put a vendored crate's `Cargo.lock` back as it was: rewrite it, or remove it when there was
/// none, so a run leaves the checkout unchanged.
fn restore_lock(lock: &Path, saved: Option<&[u8]>) -> anyhow::Result<()> {
    match saved {
        Some(bytes) => std::fs::write(lock, bytes).with_context(|| format!("restoring {}", lock.display())),
        None if lock.exists() => std::fs::remove_file(lock).with_context(|| format!("removing {}", lock.display())),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_patches_under_vendor_are_taken_in_order() {
        let manifest = r#"
[patch.crates-io]
hayro = { path = "vendor/hayro" }
egui = { git = "https://example.com/egui.git", rev = "abc" }
local = { path = "crates/local" }
lopdf = { path = "vendor/lopdf" }
"#;
        let names: Vec<String> = vendored_patches(manifest).unwrap().into_iter().map(|p| p.name).collect();
        // toml tables are ordered by key, which is the order the command runs them in.
        assert_eq!(names, ["hayro", "lopdf"]);
    }

    #[test]
    fn no_patch_table_means_nothing_to_run() {
        assert!(vendored_patches("[workspace]\nmembers = []\n").unwrap().is_empty());
    }

    #[test]
    fn a_crate_without_dev_dependencies_runs_in_the_workspace_by_name_and_version() {
        let manifest = "[package]\nname = \"vello_common\"\nversion = \"0.0.8\"\n[dependencies]\nlog = \"0.4\"\n";
        assert_eq!(plan_for("vendor/vello_common", manifest).unwrap(), Plan::Workspace { spec: "vello_common@0.0.8".into() });
    }

    #[test]
    fn dotted_and_target_dev_dependencies_both_make_a_crate_standalone() {
        let dotted = "[package]\nname = \"a\"\nversion = \"1.0.0\"\n[dev-dependencies.md5]\nversion = \"0.8\"\n";
        let target = "[package]\nname = \"b\"\nversion = \"1.0.0\"\n[target.'cfg(unix)'.dev-dependencies.criterion]\nversion = \"0.5\"\n";
        for (dir, manifest) in [("vendor/a", dotted), ("vendor/b", target)] {
            assert_eq!(plan_for(dir, manifest).unwrap(), Plan::Standalone { manifest: Path::new(dir).join("Cargo.toml") }, "{dir}");
        }
    }

    #[test]
    fn patch_config_quotes_the_path_as_a_toml_string() {
        let value = patch_config("hayro-syntax", Path::new(r"C:\repo it's\vendor\hayro-syntax"));
        let (key, path) = value.split_once('=').unwrap();
        assert_eq!(key, "patch.crates-io.hayro-syntax.path");
        let parsed: toml::Table = toml::from_str(&format!("p = {path}")).unwrap();
        assert_eq!(parsed["p"].as_str(), Some("C:/repo it's/vendor/hayro-syntax"));
    }

    #[test]
    fn only_crates_with_corpus_tests_get_exact_skips() {
        assert!(skips("hayro").is_empty());
        let s = skips("hayro-syntax");
        assert!(s.ends_with(&["--exact".to_string()]), "{s:?}");
        assert_eq!(s.iter().filter(|a| *a == "--skip").count(), 2);
    }

    #[test]
    fn restoring_a_lock_rewrites_it_or_removes_one_the_run_created() {
        let dir = std::env::temp_dir().join(format!("xtask-vendor-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lock = dir.join("Cargo.lock");
        std::fs::write(&lock, b"changed").unwrap();
        restore_lock(&lock, Some(b"original")).unwrap();
        assert_eq!(std::fs::read(&lock).unwrap(), b"original");
        restore_lock(&lock, None).unwrap();
        assert!(!lock.exists());
        std::fs::remove_dir(&dir).unwrap();
    }
}
