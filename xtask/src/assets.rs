//! `cargo xtask assets`: enforce the asset policy (AGENTS.md §1).
//!
//! `ATTRIBUTION.toml` lists every asset: committed or vendored files (`[[asset]]`), files that
//! Cargo dependencies compile into PdfCraft (`[[bundled]]`), and files xtask downloads at build
//! time (`[[fetched]]`). This gate fails when:
//! - an asset-like file in the repository has no entry, or its SHA-256 differs;
//! - a licence is not on the allowlist, or a declared licence file is missing;
//! - an icon, image or font names Adobe as author or source, or an `adobe_data` entry is not on
//!   the closed list approved in AGENTS.md §1.1;
//! - a `[[bundled]]` crate version is not the one in Cargo.lock;
//! - `ATTRIBUTION.md` is out of date (`--write` regenerates it).

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Licences allowed by AGENTS.md §1.2 (SPDX identifiers).
pub const ALLOWED_LICENCES: &[&str] = &[
    "ISC",
    "MIT",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "Apache-2.0",
    "OFL-1.1",
    "Ubuntu-font-1.0",
    "Bitstream-Vera",
    "CC0-1.0",
    "CC-BY-4.0",
    "CC-BY-SA-4.0",
    "LicenseRef-Public-Domain",
];

/// The closed list of Adobe-authored, non-visual technical data (AGENTS.md §1.1): (crate, path).
/// Storyteller's own brand marks (the ArtCraft name and logos): shipped in the app and docs, but
/// not licensed for reuse by forks. Allowed only for `kind = "logo"` entries under `docs/brand/`
/// (AGENTS.md §1.2).
pub const BRAND_LICENCE: &str = "LicenseRef-Storyteller-Trademark";

pub const ADOBE_DATA_ALLOWED: &[(&str, &str)] = &[("hayro-cmap", "assets/cmaps.brotli"), ("hayro-interpret", "src/font/generated/metrics.rs")];

/// First-party files on that same closed list: generated tables of Adobe-authored non-visual
/// technical data we hold ourselves rather than through a dependency. Each still needs an
/// `[[asset]]` entry with `adobe_data = true`, so its SHA-256 is checked and any change to the
/// table has to be re-attested deliberately.
pub const ADOBE_DATA_FILES: &[&str] = &["crates/fonts/src/std14.rs"];

/// File extensions that count as assets wherever they appear in the repository.
const ASSET_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "svg", "ico", "icns", "webp", "bmp", "tif", "tiff", "avif", "heic", "ttf", "otf", "ttc", "woff", "woff2", "pfb",
    "pfa", "afm", "icc", "icm", "pdf", "eps", "ps", "ai", "psd", "mp3", "wav", "ogg", "flac", "mp4", "mov", "webm", "cur", "ani", "brotli", "tsv",
];

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    #[serde(default)]
    pub asset: Vec<Asset>,
    #[serde(default)]
    pub bundled: Vec<Bundled>,
    #[serde(default)]
    pub fetched: Vec<Fetched>,
    #[serde(default)]
    pub build_input: Vec<BuildInput>,
}

/// Material compiled in only when the builder opts in (e.g. `CRAFT_FONTS_DIR`): not in this
/// repository, not downloaded by it. Its own repository attributes each file.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildInput {
    pub name: String,
    /// The environment variable that turns it on.
    pub option: String,
    pub title: String,
    pub author: String,
    pub source: String,
    pub licence: String,
    /// The input's own per-file attribution.
    pub attribution: String,
    pub kind: String,
    pub usage: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub path: String,
    pub title: String,
    pub author: String,
    pub source: String,
    pub licence: String,
    pub licence_file: String,
    pub kind: String,
    pub usage: String,
    pub sha256: String,
    #[serde(default)]
    pub adobe_data: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundled {
    #[serde(rename = "crate")]
    pub krate: String,
    pub version: String,
    pub path: String,
    pub title: String,
    pub author: String,
    pub source: String,
    pub licence: String,
    pub kind: String,
    pub usage: String,
    pub sha256: String,
    #[serde(default)]
    pub adobe_data: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fetched {
    pub file: String,
    pub url: String,
    pub title: String,
    pub author: String,
    pub source: String,
    pub licence: String,
    pub licence_url: String,
    pub licence_sha256: String,
    pub kind: String,
    pub usage: String,
    pub sha256: String,
}

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask lives in the workspace").to_path_buf()
}

pub fn load(root: &Path) -> Result<Manifest> {
    let path = root.join("ATTRIBUTION.toml");
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// `cargo xtask assets [--write]`.
pub fn run(args: &[String]) -> Result<()> {
    let root = root();
    let manifest = load(&root)?;
    let markdown = render_markdown(&manifest);
    if args.iter().any(|a| a == "--write") {
        std::fs::write(root.join("ATTRIBUTION.md"), &markdown)?;
        println!("assets: wrote ATTRIBUTION.md");
    }
    let mut problems = check(&root, &manifest, &repo_files(&root)?, &cargo_lock_versions(&root)?);
    match std::fs::read_to_string(root.join("ATTRIBUTION.md")) {
        Ok(existing) if existing == markdown => {}
        _ => problems.push("ATTRIBUTION.md is out of date: run `cargo xtask assets --write`".into()),
    }
    if problems.is_empty() {
        println!(
            "assets: ok ({} repository assets, {} bundled by dependencies, {} fetched at build time)",
            manifest.asset.len(),
            manifest.bundled.len(),
            manifest.fetched.len()
        );
        Ok(())
    } else {
        for p in &problems {
            eprintln!("  ✗ {p}");
        }
        bail!("{} asset policy violation(s); see AGENTS.md §1", problems.len())
    }
}

/// Files in the working tree that git tracks or would track (respects .gitignore).
fn repo_files(root: &Path) -> Result<Vec<String>> {
    let out = Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard", "-z"])
        .current_dir(root)
        .output()
        .context("running git ls-files")?;
    if !out.status.success() {
        bail!("git ls-files failed");
    }
    Ok(out.stdout.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect())
}

fn cargo_lock_versions(root: &Path) -> Result<BTreeSet<(String, String)>> {
    let text = std::fs::read_to_string(root.join("Cargo.lock")).context("reading Cargo.lock")?;
    let mut out = BTreeSet::new();
    let mut name = None;
    for line in text.lines() {
        if let Some(n) = line.strip_prefix("name = \"") {
            name = Some(n.trim_end_matches('"').to_string());
        } else if let (Some(v), Some(n)) = (line.strip_prefix("version = \""), name.take()) {
            out.insert((n, v.trim_end_matches('"').to_string()));
        }
    }
    Ok(out)
}

pub fn is_asset_path(path: &str) -> bool {
    Path::new(path).extension().and_then(|e| e.to_str()).is_some_and(|e| ASSET_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Every licence id in an SPDX expression (`MIT OR Apache-2.0`, `MIT AND Bitstream-Vera`).
fn licence_ids(expr: &str) -> impl Iterator<Item = &str> {
    expr.split(|c: char| c.is_whitespace() || c == '(' || c == ')').filter(|t| !t.is_empty() && *t != "OR" && *t != "AND" && *t != "WITH")
}

fn mentions_adobe(s: &str) -> bool {
    s.to_ascii_lowercase().contains("adobe")
}

/// All policy checks that don't depend on the filesystem layout beyond `root` (unit-testable).
pub fn check(root: &Path, m: &Manifest, repo_files: &[String], lock: &BTreeSet<(String, String)>) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    // Kinds that may never come from Adobe (AGENTS.md §1.1): visual design, and UI translations
    // (which must be clean-room, never taken from a product's string tables).
    let visual = |kind: &str| matches!(kind, "icon" | "image" | "font" | "logo" | "cursor" | "video" | "translation");
    let licence_ok = |what: &str, licence: &str, problems: &mut Vec<String>| {
        for id in licence_ids(licence) {
            if !ALLOWED_LICENCES.contains(&id) {
                problems.push(format!("{what}: licence `{id}` is not on the allowlist (AGENTS.md §1.2)"));
            }
        }
    };
    for a in &m.asset {
        if !seen.insert(a.path.clone()) {
            problems.push(format!("{}: listed twice", a.path));
        }
        let brand = a.licence == BRAND_LICENCE && a.kind == "logo" && a.path.starts_with("docs/brand/");
        if !brand {
            licence_ok(&a.path, &a.licence, &mut problems);
        }
        if !root.join(&a.licence_file).is_file() {
            problems.push(format!("{}: licence file {} is missing", a.path, a.licence_file));
        }
        if visual(&a.kind) && (mentions_adobe(&a.author) || mentions_adobe(&a.source) || mentions_adobe(&a.title)) {
            problems.push(format!("{}: {} from Adobe is forbidden (AGENTS.md §1.1)", a.path, a.kind));
        }
        if a.adobe_data && !(ADOBE_DATA_FILES.contains(&a.path.as_str()) && a.kind == "data") {
            problems.push(format!("{}: adobe_data is only allowed for the closed list in AGENTS.md §1.1", a.path));
        }
        match std::fs::read(root.join(&a.path)) {
            Ok(bytes) if sha256_hex(&bytes) == a.sha256 => {}
            Ok(_) => problems.push(format!("{}: SHA-256 does not match ATTRIBUTION.toml (re-verify provenance, then update the entry)", a.path)),
            Err(_) => problems.push(format!("{}: listed but missing", a.path)),
        }
    }
    for f in repo_files {
        if is_asset_path(f) && !seen.contains(f) && root.join(f).is_file() {
            problems.push(format!("{f}: asset has no ATTRIBUTION.toml entry"));
        }
    }
    for b in &m.bundled {
        let what = format!("{}@{} {}", b.krate, b.version, b.path);
        licence_ok(&what, &b.licence, &mut problems);
        if !lock.contains(&(b.krate.clone(), b.version.clone())) {
            problems.push(format!("{what}: crate version not in Cargo.lock (dependency changed: re-audit its bundled assets)"));
        }
        if visual(&b.kind) && (mentions_adobe(&b.author) || mentions_adobe(&b.source)) {
            problems.push(format!("{what}: {} from Adobe is forbidden (AGENTS.md §1.1)", b.kind));
        }
        let adobe = mentions_adobe(&b.author);
        let approved = ADOBE_DATA_ALLOWED.contains(&(b.krate.as_str(), b.path.as_str())) && b.kind == "data";
        if (adobe || b.adobe_data) && !(approved && b.adobe_data) {
            problems.push(format!("{what}: Adobe-authored material outside the closed list in AGENTS.md §1.1"));
        }
        if b.sha256.len() != 64 {
            problems.push(format!("{what}: sha256 must be 64 hex digits"));
        }
    }
    for f in &m.fetched {
        licence_ok(&f.file, &f.licence, &mut problems);
        if visual(&f.kind) && (mentions_adobe(&f.author) || mentions_adobe(&f.source) || mentions_adobe(&f.title)) {
            problems.push(format!("{}: {} from Adobe is forbidden (AGENTS.md §1.1)", f.file, f.kind));
        }
        if f.sha256.len() != 64 || f.licence_sha256.len() != 64 {
            problems.push(format!("{}: sha256 and licence_sha256 must be 64 hex digits", f.file));
        }
        if !f.url.starts_with("https://") || !f.licence_url.starts_with("https://") {
            problems.push(format!("{}: URLs must be https", f.file));
        }
    }
    for b in &m.build_input {
        licence_ok(&b.name, &b.licence, &mut problems);
        if visual(&b.kind) && (mentions_adobe(&b.author) || mentions_adobe(&b.source) || mentions_adobe(&b.title)) {
            problems.push(format!("{}: {} from Adobe is forbidden (AGENTS.md §1.1)", b.name, b.kind));
        }
        if !b.source.starts_with("https://") || !b.attribution.starts_with("https://") {
            problems.push(format!("{}: URLs must be https", b.name));
        }
    }
    problems
}

/// Download (if needed) and verify every `[[fetched]]` entry of `kind` into `dir`, with its licence
/// text. Returns the paths of the fetched files. Uses `curl`, present on macOS, Windows 10+ and Linux.
pub fn fetch_all(m: &Manifest, dir: &Path, kind: &str) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let mut out = Vec::new();
    for f in m.fetched.iter().filter(|f| f.kind == kind) {
        let path = dir.join(&f.file);
        fetch_verified(&f.url, &path, &f.sha256)?;
        let licence = dir.join(format!("{}.LICENCE.txt", f.file));
        fetch_verified(&f.licence_url, &licence, &f.licence_sha256)?;
        out.push(path);
    }
    Ok(out)
}

fn fetch_verified(url: &str, path: &Path, sha256: &str) -> Result<()> {
    if std::fs::read(path).is_ok_and(|b| sha256_hex(&b) == sha256) {
        return Ok(());
    }
    println!("assets: fetching {url}");
    let tmp = path.with_extension("part");
    let status = Command::new("curl").args(["-sSfL", "--retry", "3", "-o"]).arg(&tmp).arg(url).status().context("running curl")?;
    if !status.success() {
        bail!("download failed: {url}");
    }
    let bytes = std::fs::read(&tmp)?;
    if sha256_hex(&bytes) != sha256 {
        let _ = std::fs::remove_file(&tmp);
        bail!("{url}: SHA-256 mismatch (expected {sha256}); refusing to use it");
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// `cargo xtask models [DIR]`: fetch the OCR models (`kind = "model"`) into `assets/models/` (git-ignored),
/// where the app and tests look for them.
pub fn models(args: &[String]) -> Result<()> {
    let root = root();
    let m = load(&root)?;
    let dir = args.first().map(PathBuf::from).unwrap_or_else(|| root.join("assets/models"));
    for p in fetch_all(&m, &dir, "model")? {
        println!("models: {}", p.display());
    }
    Ok(())
}

/// ATTRIBUTION.md, generated from the manifest.
pub fn render_markdown(m: &Manifest) -> String {
    let mut s = String::new();
    s.push_str("# Attribution\n\n");
    s.push_str("<!-- Generated from ATTRIBUTION.toml by `cargo xtask assets --write`. Do not edit by hand. -->\n\n");
    s.push_str("Every asset PdfCraft includes, bundles or uses to build its published material, with its author, source and licence. ");
    s.push_str(
        "The policy is in [AGENTS.md](AGENTS.md) §1. The machine-readable list, with SHA-256 hashes, is [ATTRIBUTION.toml](ATTRIBUTION.toml). ",
    );
    s.push_str("Licence texts are kept beside the assets and summarised in [NOTICE](NOTICE).\n\n");
    let esc = |t: &str| t.replace('|', "\\|");
    s.push_str(&format!(
        "## In this repository ({})\n\n| Asset | Title | Author | Licence | Source | Used for |\n|---|---|---|---|---|---|\n",
        m.asset.len()
    ));
    for a in &m.asset {
        let _ = writeln!(s, "| `{}` | {} | {} | {} | {} | {} |", a.path, esc(&a.title), esc(&a.author), a.licence, esc(&a.source), esc(&a.usage));
    }
    s.push_str(&format!(
        "\n## Compiled in through dependencies ({})\n\n| Crate | File | Title | Author | Licence | Source | Used for |\n|---|---|---|---|---|---|---|\n",
        m.bundled.len()
    ));
    for b in &m.bundled {
        let note = if b.adobe_data { " (non-visual data, AGENTS.md §1.1)" } else { "" };
        let _ = writeln!(
            s,
            "| `{}` {} | `{}` | {}{} | {} | {} | {} | {} |",
            b.krate,
            b.version,
            b.path,
            esc(&b.title),
            note,
            esc(&b.author),
            b.licence,
            esc(&b.source),
            esc(&b.usage)
        );
    }
    s.push_str(&format!(
        "\n## Downloaded at build time ({})\n\nFonts are fetched by `cargo xtask demo-pdf` into `target/demo-fonts/`, OCR models by `cargo xtask models` into `assets/models/`; each is verified by SHA-256 and never committed.\n\n| File | Title | Author | Licence | Source | Used for |\n|---|---|---|---|---|---|\n",
        m.fetched.len()
    ));
    for f in &m.fetched {
        let _ = writeln!(s, "| `{}` | {} | {} | {} | {} | {} |", f.file, esc(&f.title), esc(&f.author), f.licence, esc(&f.source), esc(&f.usage));
    }
    s.push_str(&format!(
        "\n## Optional build inputs ({})\n\nNot in this repository and never downloaded by it: compiled in only when the build sets the option (official releases do). Each input attributes its own files.\n\n| Input | Option | Title | Author | Licence | Source | Attribution | Used for |\n|---|---|---|---|---|---|---|---|\n",
        m.build_input.len()
    ));
    for b in &m.build_input {
        let _ = writeln!(
            s,
            "| `{}` | `{}` | {} | {} | {} | {} | {} | {} |",
            b.name,
            b.option,
            esc(&b.title),
            esc(&b.author),
            b.licence,
            esc(&b.source),
            esc(&b.attribution),
            esc(&b.usage)
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(path: &str, author: &str, kind: &str, licence: &str) -> Asset {
        Asset {
            path: path.into(),
            title: "t".into(),
            author: author.into(),
            source: "s".into(),
            licence: licence.into(),
            licence_file: "Cargo.toml".into(),
            kind: kind.into(),
            usage: "u".into(),
            sha256: sha256_hex(&std::fs::read(root().join(path)).unwrap_or_default()),
            adobe_data: false,
        }
    }

    fn bundled(krate: &str, path: &str, author: &str, kind: &str, adobe_data: bool) -> Bundled {
        Bundled {
            krate: krate.into(),
            version: "1.0.0".into(),
            path: path.into(),
            title: "t".into(),
            author: author.into(),
            source: "s".into(),
            licence: "BSD-3-Clause".into(),
            kind: kind.into(),
            usage: "u".into(),
            sha256: "0".repeat(64),
            adobe_data,
        }
    }

    #[test]
    fn build_inputs_need_an_open_licence_and_https_links() {
        let input = |licence: &str, attribution: &str| BuildInput {
            name: "craft-fonts".into(),
            option: "CRAFT_FONTS_DIR".into(),
            title: "t".into(),
            author: "a".into(),
            source: "https://github.com/storytold/craft-fonts".into(),
            licence: licence.into(),
            attribution: attribution.into(),
            kind: "font".into(),
            usage: "u".into(),
        };
        let check_one = |b: BuildInput| check(&root(), &Manifest { build_input: vec![b], ..Default::default() }, &[], &lock(&[]));
        assert!(check_one(input("OFL-1.1", "https://example.org/ATTRIBUTION.md")).is_empty());
        assert_eq!(check_one(input("LicenseRef-Proprietary", "https://example.org/A.md")).len(), 1);
        assert_eq!(check_one(input("OFL-1.1", "ATTRIBUTION.md")).len(), 1);
    }

    fn lock(entries: &[(&str, &str)]) -> BTreeSet<(String, String)> {
        entries.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn the_real_manifest_passes() {
        let root = root();
        let m = load(&root).unwrap();
        let problems = check(&root, &m, &repo_files(&root).unwrap(), &cargo_lock_versions(&root).unwrap());
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn unlisted_asset_is_rejected() {
        let p = check(&root(), &Manifest::default(), &["assets/icons/x.svg".into()], &lock(&[]));
        assert!(p.iter().any(|p| p.contains("no ATTRIBUTION.toml entry")), "{p:?}");
    }

    #[test]
    fn translation_catalogs_require_attribution() {
        assert!(is_asset_path("crates/ui-egui/src/i18n/ja.tsv"));
        let p = check(&root(), &Manifest::default(), &["crates/ui-egui/src/i18n/ja.tsv".into()], &lock(&[]));
        assert!(p.iter().any(|p| p.contains("no ATTRIBUTION.toml entry")), "{p:?}");
        let m = Manifest { asset: vec![asset("crates/ui-egui/src/i18n/ja.tsv", "Adobe", "translation", "MIT")], ..Default::default() };
        assert!(check(&root(), &m, &[], &lock(&[])).iter().any(|p| p.contains("from Adobe is forbidden")));
    }

    #[test]
    fn adobe_icons_are_rejected() {
        let m = Manifest { asset: vec![asset("assets/icons/x.svg", "Adobe Inc.", "icon", "MIT")], ..Default::default() };
        let p = check(&root(), &m, &[], &lock(&[]));
        assert!(p.iter().any(|p| p.contains("from Adobe is forbidden")), "{p:?}");
    }

    #[test]
    fn disallowed_licences_are_rejected() {
        let m = Manifest { asset: vec![asset("assets/icons/x.svg", "Someone", "icon", "LicenseRef-Proprietary OR MIT")], ..Default::default() };
        let p = check(&root(), &m, &[], &lock(&[]));
        assert!(p.iter().any(|p| p.contains("not on the allowlist")), "{p:?}");
    }

    #[test]
    fn changed_file_fails_hash_check() {
        let mut a = asset("assets/icons/x.svg", "Lucide", "icon", "ISC");
        a.sha256 = "0".repeat(64);
        let p = check(&root(), &Manifest { asset: vec![a], ..Default::default() }, &[], &lock(&[]));
        assert!(p.iter().any(|p| p.contains("SHA-256 does not match")), "{p:?}");
    }

    #[test]
    fn brand_licence_only_for_brand_logos() {
        let root = super::root();
        for (path, kind, pass) in [
            ("docs/brand/artcraft-mark.svg", "logo", true),
            ("docs/brand/artcraft-mark.svg", "icon", false),
            ("assets/icons/mark.svg", "logo", false),
        ] {
            let mut a = asset(path, "Storyteller", kind, BRAND_LICENCE);
            a.licence_file = "docs/brand/LICENSE-brand.txt".into();
            let m = Manifest { asset: vec![a], ..Default::default() };
            let p = check(&root, &m, &[], &BTreeSet::new());
            assert_eq!(p.iter().all(|p| !p.contains("allowlist")), pass, "{path} {kind}: {p:?}");
        }
    }

    #[test]
    fn adobe_data_only_on_the_closed_list() {
        let l = lock(&[("hayro-cmap", "1.0.0"), ("other", "1.0.0")]);
        let ok = Manifest { bundled: vec![bundled("hayro-cmap", "assets/cmaps.brotli", "Adobe", "data", true)], ..Default::default() };
        assert!(check(&root(), &ok, &[], &l).is_empty());
        let sneaky = Manifest { bundled: vec![bundled("other", "fonts/Myriad.otf", "Adobe", "data", true)], ..Default::default() };
        assert!(check(&root(), &sneaky, &[], &l).iter().any(|p| p.contains("closed list")));
        let font = Manifest { bundled: vec![bundled("hayro-cmap", "assets/cmaps.brotli", "Adobe", "font", true)], ..Default::default() };
        assert!(check(&root(), &font, &[], &l).iter().any(|p| p.contains("forbidden")));
    }

    #[test]
    fn first_party_adobe_data_only_on_the_closed_list() {
        let l = lock(&[]);
        // The generated standard-14 table is on the list, as `kind = "data"`.
        let mut ok = asset("crates/fonts/src/std14.rs", "Adobe (Core14 AFM metrics); tables by the PdfCraft contributors", "data", "MIT");
        ok.adobe_data = true;
        let m = Manifest { asset: vec![ok], ..Default::default() };
        assert!(check(&root(), &m, &[], &l).is_empty(), "{:?}", check(&root(), &m, &[], &l));

        // Any other first-party file claiming it is refused.
        let mut sneaky = asset("Cargo.toml", "Adobe", "data", "MIT");
        sneaky.adobe_data = true;
        let m = Manifest { asset: vec![sneaky], ..Default::default() };
        assert!(check(&root(), &m, &[], &l).iter().any(|p| p.contains("closed list")));

        // Even on the list, it may only be data — never a typeface.
        let mut wrong_kind = asset("crates/fonts/src/std14.rs", "Adobe", "font", "MIT");
        wrong_kind.adobe_data = true;
        let m = Manifest { asset: vec![wrong_kind], ..Default::default() };
        assert!(!check(&root(), &m, &[], &l).is_empty());
    }

    #[test]
    fn dependency_upgrade_forces_reaudit() {
        let m = Manifest { bundled: vec![bundled("other", "a.ttf", "Someone", "font", false)], ..Default::default() };
        assert!(check(&root(), &m, &[], &lock(&[("other", "2.0.0")])).iter().any(|p| p.contains("not in Cargo.lock")));
    }

    #[test]
    fn spdx_expressions_split() {
        assert_eq!(licence_ids("(MIT OR Apache-2.0) AND OFL-1.1").collect::<Vec<_>>(), ["MIT", "Apache-2.0", "OFL-1.1"]);
    }
}
