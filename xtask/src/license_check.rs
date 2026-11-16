//! Offline source-document checks. This does not inspect completed packages or dependencies.
use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::assets;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Documents {
    document: Vec<Document>,
    bundled: Vec<Bundled>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    source: String,
    destination: String,
    sha256: String,
    /// Audited identifiers of the full licence texts in this document, not inferred from its contents.
    #[serde(default)]
    licences: Vec<String>,
    #[serde(default)]
    origin: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bundled {
    #[serde(rename = "crate")]
    krate: String,
    path: String,
    licence: String,
    documents: Vec<String>,
}

pub fn run(args: &[String]) -> Result<()> {
    if !args.is_empty() {
        bail!("usage: cargo xtask license-check (no downloads or write mode)");
    }
    let root = assets::root();
    // Reuse the existing policy and generated-attribution validator, once.
    assets::run(&[])?;
    validate(&root)?;
    println!("license-check: source documents and licensing metadata are consistent");
    Ok(())
}

/// Every OCR asset filename constant the application loads at runtime, paired with the source file
/// that declares it. A language is only supported once its model and dictionary are listed here, so
/// a new recogniser cannot ship without a licensing record for what it downloads.
const REQUIRED_OCR_ASSETS: &[(&str, &str)] = &[
    ("crates/ocr/src/ocrs.rs", "DETECTION_MODEL"),
    ("crates/ocr/src/ocrs.rs", "RECOGNITION_MODEL"),
    ("crates/ocr/src/chinese.rs", "CHINESE_MODEL"),
    ("crates/ocr/src/chinese.rs", "CHINESE_DICTIONARY"),
];

fn validate(root: &Path) -> Result<()> {
    let text = std::fs::read_to_string(root.join("LICENSES.toml")).context("reading LICENSES.toml")?;
    let docs: Documents = toml::from_str(&text).context("parsing LICENSES.toml")?;
    let attribution = assets::load(root)?;
    let mut problems = check(root, &docs, &attribution);
    problems.extend(check_required_ocr_metadata(&ocr_asset_sources(root)?, &attribution)?);
    if !problems.is_empty() {
        bail!("source licensing errors:\n{}", problems.join("\n"));
    }
    Ok(())
}

/// Read every source file that declares a required OCR asset constant. Fail closed on an unreadable
/// file rather than checking only the constants whose source happened to load.
fn ocr_asset_sources(root: &Path) -> Result<Vec<(String, String)>> {
    let mut sources: Vec<(String, String)> = Vec::new();
    for &(path, _) in REQUIRED_OCR_ASSETS {
        if sources.iter().any(|(known, _)| known == path) {
            continue;
        }
        let text = std::fs::read_to_string(root.join(path)).with_context(|| format!("reading OCR asset filename constants from {path}"))?;
        sources.push((path.to_string(), text));
    }
    Ok(sources)
}

/// Check every required OCR asset constant against ATTRIBUTION.toml. Fail closed if a declaration
/// format changes or its source text is absent; do not guess filenames or add a dependency on the
/// OCR engine.
fn check_required_ocr_metadata(sources: &[(String, String)], attribution: &assets::Manifest) -> Result<Vec<String>> {
    let mut problems = Vec::new();
    for &(path, name) in REQUIRED_OCR_ASSETS {
        let Some((_, source)) = sources.iter().find(|(known, _)| known == path) else {
            bail!("OCR constant {name}: no source text was read for {path}; update the source validator if the file moved");
        };
        let prefix = format!("pub const {name}: &str = \"");
        let declarations: Vec<_> = source.lines().map(str::trim).filter_map(|line| line.strip_prefix(&prefix)).collect();
        let file = declarations.first().and_then(|value| value.strip_suffix("\";"));
        let Some(file) = file.filter(|file| declarations.len() == 1 && safe_path(file) && !file.contains('/')) else {
            bail!("OCR constant {name}: expected one literal filename declaration in {path}; update the source validator if its syntax changed");
        };
        let entries: Vec<_> = attribution.fetched.iter().filter(|f| f.file == file).collect();
        if entries.is_empty() {
            problems.push(format!("{file}: required OCR licensing metadata missing from ATTRIBUTION.toml"));
        } else {
            if entries.len() != 1 {
                problems.push(format!("{file}: required OCR licensing metadata must have exactly one entry"));
            }
            if entries.iter().any(|f| f.kind != "model") {
                problems.push(format!("{file}: required OCR licensing metadata must have kind = \"model\""));
            }
        }
    }
    Ok(problems)
}

/// The audited bundle inventory currently uses identifiers and flat AND/OR expressions only.
/// Keep their semantics distinct, and fail closed on mixed operators, grouping or exceptions
/// rather than introducing a general SPDX parser or flattening an unsupported expression.
fn licence_coverage(expression: &str, covered: &BTreeSet<&str>) -> Result<bool> {
    let mut tokens = expression.split_whitespace();
    let Some(first) = tokens.next().filter(|id| assets::ALLOWED_LICENCES.contains(id)) else {
        bail!("unsupported bundle licence expression `{expression}`");
    };
    let mut ids = vec![first];
    let mut operator = None;
    while let Some(op) = tokens.next() {
        if !matches!(op, "AND" | "OR") || operator.is_some_and(|previous| previous != op) {
            bail!("unsupported bundle licence expression `{expression}`; audit its applicability before extending the validator");
        }
        operator = Some(op);
        let Some(id) = tokens.next().filter(|id| assets::ALLOWED_LICENCES.contains(id)) else {
            bail!("unsupported bundle licence expression `{expression}`");
        };
        ids.push(id);
    }
    Ok(if operator == Some("OR") { ids.iter().any(|id| covered.contains(id)) } else { ids.iter().all(|id| covered.contains(id)) })
}

// A portable relative path, including on Windows; reject aliases before comparing mappings.
fn safe_path(s: &str) -> bool {
    !s.is_empty()
        && !s.contains(['\\', ':'])
        && s.split('/').all(|p| {
            !p.is_empty()
                && p != "."
                && p != ".."
                && !p.ends_with(['.', ' '])
                && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                && !matches!(
                    p.split('.').next().unwrap_or_default().to_ascii_uppercase().as_str(),
                    "CON"
                        | "PRN"
                        | "AUX"
                        | "NUL"
                        | "COM1"
                        | "COM2"
                        | "COM3"
                        | "COM4"
                        | "COM5"
                        | "COM6"
                        | "COM7"
                        | "COM8"
                        | "COM9"
                        | "LPT1"
                        | "LPT2"
                        | "LPT3"
                        | "LPT4"
                        | "LPT5"
                        | "LPT6"
                        | "LPT7"
                        | "LPT8"
                        | "LPT9"
                )
        })
}

fn hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn check(root: &Path, docs: &Documents, attribution: &assets::Manifest) -> Vec<String> {
    let mut problems = Vec::new();
    let mut sources = BTreeSet::new();
    let mut destinations = BTreeSet::new();
    for d in &docs.document {
        if !safe_path(&d.source) || !safe_path(&d.destination) {
            problems.push(format!("{}: unsafe document source or destination", d.source));
            continue;
        }
        let new_source = sources.insert(d.source.as_str());
        let new_destination = destinations.insert(d.destination.to_ascii_lowercase());
        if !new_source || !new_destination {
            problems.push(format!("{}: duplicate document source or destination", d.source));
        }
        if d.origin.as_ref().is_some_and(|u| !u.starts_with("https://") || u.trim().len() <= 8) {
            problems.push(format!("{}: invalid document origin", d.source));
        }
        let mut identifiers = BTreeSet::new();
        for id in &d.licences {
            if !assets::ALLOWED_LICENCES.contains(&id.as_str()) || !identifiers.insert(id.as_str()) {
                problems.push(format!("{}: invalid or duplicate audited licence identifier `{id}`", d.source));
            }
        }
        let path = root.join(&d.source);
        // Canonical confinement also catches symlinks escaping the checkout.
        let confined = root.canonicalize().and_then(|r| path.canonicalize().map(|p| p.starts_with(r))).unwrap_or(false);
        if !confined || !path.is_file() {
            problems.push(format!("{}: required document missing or outside checkout", d.source));
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) if !text.trim().is_empty() && !text.contains('\0') => {
                if !hash(&d.sha256) || assets::sha256_hex(text.as_bytes()) != d.sha256 {
                    problems.push(format!("{}: document hash mismatch; re-audit the source before updating LICENSES.toml", d.source));
                }
            }
            _ => problems.push(format!("{}: required document empty or invalid UTF-8 text", d.source)),
        }
    }
    for destination in &destinations {
        for (i, _) in destination.match_indices('/') {
            if destination.get(..i).is_some_and(|prefix| destinations.contains(prefix)) {
                problems.push(format!("{destination}: document destination conflicts with a parent file"));
            }
        }
    }
    for required in ["LICENSE-MIT", "LICENSE-APACHE", "NOTICE", "licenses/pdf-backend/hayro-interpret/assets/LICENSE_FOXIT"]
        .into_iter()
        .chain(attribution.asset.iter().map(|a| a.licence_file.as_str()))
    {
        if !sources.contains(required) {
            problems.push(format!("{required}: required document has no LICENSES.toml mapping"));
        }
    }
    // One mapping per bundled asset, using the existing attribution's licence expression.
    let mut seen = BTreeSet::new();
    for b in &docs.bundled {
        if !seen.insert((b.krate.as_str(), b.path.as_str())) {
            problems.push(format!("{} {}: duplicate bundled mapping", b.krate, b.path));
        }
        if !attribution.bundled.iter().any(|a| a.krate == b.krate && a.path == b.path && a.licence == b.licence) {
            problems.push(format!("{} {}: bundled mapping disagrees with ATTRIBUTION.toml", b.krate, b.path));
        }
        if b.documents.is_empty() || b.documents.iter().any(|d| !sources.contains(d.as_str())) {
            problems.push(format!("{} {}: bundled licence documents missing", b.krate, b.path));
        }
        for document in docs.document.iter().filter(|d| b.documents.contains(&d.source)) {
            if !document.licences.iter().any(|id| b.licence.split_whitespace().any(|token| token == id)) {
                problems.push(format!("{} {}: inapplicable mapped document {}", b.krate, b.path, document.source));
            }
        }
        let covered: BTreeSet<_> =
            docs.document.iter().filter(|d| b.documents.contains(&d.source)).flat_map(|d| d.licences.iter().map(String::as_str)).collect();
        match licence_coverage(&b.licence, &covered) {
            Ok(true) => {}
            Ok(false) => problems
                .push(format!("{} {}: inapplicable licence documents; mapped audited identifiers do not cover `{}`", b.krate, b.path, b.licence)),
            Err(error) => problems.push(format!("{} {}: {error}", b.krate, b.path)),
        }
    }
    for b in &attribution.bundled {
        if !seen.contains(&(b.krate.as_str(), b.path.as_str())) {
            problems.push(format!("{} {}: bundled asset has no licence document mapping", b.krate, b.path));
        }
    }
    for a in attribution.asset.iter().filter(|a| a.licence_file == "assets/icons/LICENSE-lucide.txt") {
        if a.licence != "ISC AND MIT" {
            problems.push(format!("{}: Lucide collection must credit ISC and Feather MIT", a.path));
        }
    }
    check_model_metadata(attribution, &mut problems);
    problems
}

fn check_model_metadata(attribution: &assets::Manifest, problems: &mut Vec<String>) {
    let mut model_paths = BTreeSet::new();
    for f in attribution.fetched.iter().filter(|f| f.kind == "model") {
        if [f.title.as_str(), &f.author, &f.source, &f.usage, &f.licence].iter().any(|v| v.trim().is_empty())
            || !hash(&f.sha256)
            || !hash(&f.licence_sha256)
            || !f.url.starts_with("https://")
            || !f.licence_url.starts_with("https://")
        {
            problems.push(format!("{}: incomplete OCR model licensing metadata", f.file));
        }
        for path in [f.file.clone(), format!("{}.LICENCE.txt", f.file)] {
            if !safe_path(&path)
                || path.contains('/')
                || path.eq_ignore_ascii_case("ATTRIBUTION.txt")
                || !model_paths.insert(path.to_ascii_lowercase())
            {
                problems.push(format!("{}: duplicate or unsafe OCR model destination", f.file));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn fixture() -> (std::path::PathBuf, Documents) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!("pdfcraft-license-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&root).unwrap();
        let document = ["LICENSE-MIT", "LICENSE-APACHE", "NOTICE", "licenses/pdf-backend/hayro-interpret/assets/LICENSE_FOXIT"]
            .into_iter()
            .map(|p| {
                let path = root.join(p);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, "synthetic licence text\n").unwrap();
                Document {
                    source: p.into(),
                    destination: p.into(),
                    sha256: assets::sha256_hex(b"synthetic licence text\n"),
                    licences: match p {
                        "LICENSE-MIT" => vec!["MIT".into()],
                        "LICENSE-APACHE" => vec!["Apache-2.0".into()],
                        _ => vec![],
                    },
                    origin: None,
                }
            })
            .collect();
        (root, Documents { document, bundled: vec![] })
    }

    #[test]
    fn missing_license_fails_and_restoring_recovers() {
        let (root, docs) = fixture();
        let m = assets::Manifest::default();
        assert!(check(&root, &docs, &m).is_empty());
        std::fs::remove_file(root.join("LICENSE-MIT")).unwrap();
        assert!(check(&root, &docs, &m).iter().any(|p| p.contains("LICENSE-MIT: required document missing")));
        std::fs::write(root.join("LICENSE-MIT"), "synthetic licence text\n").unwrap();
        assert!(check(&root, &docs, &m).is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_invalid_and_changed_documents_fail() {
        let (root, docs) = fixture();
        for text in [b" ".as_slice(), b"\xff", b"changed licence", b"text\0"] {
            std::fs::write(root.join("NOTICE"), text).unwrap();
            assert!(!check(&root, &docs, &assets::Manifest::default()).is_empty());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn duplicate_and_unsafe_destinations_fail() {
        let (root, mut docs) = fixture();
        for dest in ["license-mit", "../NOTICE", "/NOTICE", "C:/NOTICE", "a\\NOTICE", "a//NOTICE", "CON.txt", "NOTICE."] {
            docs.document.get_mut(2).unwrap().destination = dest.into();
            assert!(!check(&root, &docs, &assets::Manifest::default()).is_empty(), "{dest}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn removing_required_mapping_fails() {
        let (root, mut docs) = fixture();
        docs.document.retain(|d| d.source != "LICENSE-MIT");
        assert!(check(&root, &docs, &assets::Manifest::default()).iter().any(|p| p.contains("LICENSE-MIT: required document has no")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bundled_documents_must_match_attribution() {
        let (root, mut docs) = fixture();
        let mut m = assets::Manifest::default();
        m.bundled.push(assets::Bundled {
            krate: "synthetic".into(),
            version: "1.0.0".into(),
            path: "font.ttf".into(),
            title: "test".into(),
            author: "test".into(),
            source: "https://example.org".into(),
            licence: "MIT".into(),
            kind: "font".into(),
            usage: "test".into(),
            sha256: "a".repeat(64),
            adobe_data: false,
        });
        assert!(check(&root, &docs, &m).iter().any(|p| p.contains("no licence document mapping")));
        docs.bundled.push(Bundled {
            krate: "synthetic".into(),
            path: "font.ttf".into(),
            licence: "MIT".into(),
            documents: vec!["LICENSE-MIT".into()],
        });
        assert!(check(&root, &docs, &m).is_empty());
        docs.bundled.get_mut(0).unwrap().documents = vec!["LICENSE-APACHE".into()];
        assert!(check(&root, &docs, &m).iter().any(|p| p.contains("inapplicable")));
        docs.bundled.get_mut(0).unwrap().documents = vec!["LICENSE-MIT".into(), "LICENSE-APACHE".into()];
        assert!(check(&root, &docs, &m).iter().any(|p| p.contains("inapplicable mapped document")));
        docs.bundled.get_mut(0).unwrap().documents = vec!["LICENSE-MIT".into()];
        docs.bundled.get_mut(0).unwrap().licence = "ISC".into();
        assert!(check(&root, &docs, &m).iter().any(|p| p.contains("disagrees")));
        docs.bundled.get_mut(0).unwrap().documents.clear();
        assert!(check(&root, &docs, &m).iter().any(|p| p.contains("documents missing")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ocr_metadata_and_destinations_are_checked_without_models() {
        let (root, docs) = fixture();
        let mut m = assets::Manifest::default();
        m.fetched.push(assets::Fetched {
            file: "synthetic.rten".into(),
            url: "https://example.org/model".into(),
            title: "test model".into(),
            author: "test".into(),
            source: "https://example.org/card".into(),
            licence: "CC-BY-SA-4.0".into(),
            licence_url: "https://example.org/licence".into(),
            licence_sha256: "a".repeat(64),
            kind: "model".into(),
            usage: "test".into(),
            sha256: "b".repeat(64),
        });
        assert!(check(&root, &docs, &m).is_empty());
        m.fetched.get_mut(0).unwrap().licence_sha256 = "z".repeat(64);
        assert!(check(&root, &docs, &m).iter().any(|p| p.contains("incomplete OCR")));
        m.fetched.get_mut(0).unwrap().file = "../outside.rten".into();
        assert!(check(&root, &docs, &m).iter().any(|p| p.contains("unsafe OCR")));
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The source text of every file that declares a required OCR asset constant, read the same way
    /// the validator reads it.
    fn ocr_source_text() -> Vec<(String, String)> {
        ocr_asset_sources(&assets::root()).unwrap()
    }

    /// The filename each required OCR constant declares, in declaration order. Derived from the
    /// sources so that adding a language cannot leave a stale expectation behind in these tests.
    fn required_ocr_files() -> Vec<(&'static str, String)> {
        let sources = ocr_source_text();
        REQUIRED_OCR_ASSETS
            .iter()
            .map(|&(path, name)| {
                let source = &sources.iter().find(|(known, _)| known == path).expect("every required asset has a source").1;
                let prefix = format!("pub const {name}: &str = \"");
                let value = source.lines().map(str::trim).find_map(|line| line.strip_prefix(&prefix)).expect("the constant is declared as a literal");
                (name, value.strip_suffix("\";").expect("the literal is terminated").to_string())
            })
            .collect()
    }

    #[test]
    fn required_ocr_models_cannot_be_missing() {
        let original = assets::load(&assets::root()).unwrap();
        let sources = ocr_source_text();
        assert!(check_required_ocr_metadata(&sources, &original).unwrap().is_empty());
        // Each required asset owns exactly one model record; the set comes from the constants, so a
        // new recogniser is covered here without editing a count.
        for (name, file) in required_ocr_files() {
            let entries: Vec<_> = original.fetched.iter().filter(|f| f.file == file).collect();
            assert_eq!(entries.len(), 1, "{name} ({file}) needs exactly one ATTRIBUTION.toml entry");
            assert_eq!(entries[0].kind, "model", "{name} ({file}) must be kind = \"model\"");
            let mut m = assets::load(&assets::root()).unwrap();
            m.fetched.retain(|f| f.file != file);
            assert!(
                check_required_ocr_metadata(&sources, &m).unwrap().iter().any(|p| p.contains(&file) && p.contains("missing")),
                "{name} ({file}) was accepted after its metadata was removed"
            );
        }
    }

    #[test]
    fn required_ocr_models_cannot_be_misclassified() {
        let sources = ocr_source_text();
        for (name, file) in required_ocr_files() {
            let mut m = assets::load(&assets::root()).unwrap();
            m.fetched.iter_mut().find(|f| f.file == file).unwrap().kind = "font".into();
            assert!(
                check_required_ocr_metadata(&sources, &m).unwrap().iter().any(|p| p.contains(&file) && p.contains("kind")),
                "{name} ({file}) was accepted after its kind was changed"
            );
        }
    }

    /// The required set and the manifest must agree in both directions: a fetched model that no
    /// constant names would lose its removal check, and this is the case that a test derived from
    /// REQUIRED_OCR_ASSETS alone cannot see. Replace the old `count() == 2` assertion with this
    /// equivalence so a new language cannot silently expand either side.
    #[test]
    fn every_fetched_model_is_a_required_ocr_asset() {
        let original = assets::load(&assets::root()).unwrap();
        let required: BTreeSet<String> = required_ocr_files().into_iter().map(|(_, file)| file).collect();
        for model in original.fetched.iter().filter(|f| f.kind == "model") {
            assert!(
                required.contains(&model.file),
                "{} is fetched as a model but no required OCR constant names it, so nothing would notice its removal",
                model.file
            );
        }
        assert_eq!(required.len(), REQUIRED_OCR_ASSETS.len(), "each required constant must declare its own file");
    }

    #[test]
    fn licence_alternatives_and_combined_obligations_are_distinct() {
        let mit = BTreeSet::from(["MIT"]);
        assert!(licence_coverage("MIT OR Apache-2.0", &mit).unwrap());
        assert!(!licence_coverage("MIT AND Apache-2.0", &mit).unwrap());
        let both = BTreeSet::from(["MIT", "Bitstream-Vera"]);
        assert!(licence_coverage("MIT AND Bitstream-Vera", &both).unwrap());
        assert!(!licence_coverage("MIT OR Apache-2.0", &BTreeSet::from(["OFL-1.1"])).unwrap());
        for expression in ["", "MIT AND", "MIT OR Apache-2.0 AND ISC", "(MIT OR Apache-2.0)", "MIT WITH exception"] {
            assert!(licence_coverage(expression, &both).is_err(), "{expression}");
        }
    }

    #[test]
    fn changed_ocr_constant_syntax_fails_closed() {
        // A source that was not read refuses, instead of silently skipping the constants it declares.
        for &(path, _) in REQUIRED_OCR_ASSETS {
            let mut sources = ocr_source_text();
            sources.retain(|(known, _)| known.as_str() != path);
            let error = check_required_ocr_metadata(&sources, &assets::Manifest::default()).unwrap_err().to_string();
            assert!(error.contains(path), "{path}: {error}");
        }
        // So does a changed or emptied declaration.
        let emptied: Vec<(String, String)> = ocr_source_text().into_iter().map(|(path, _)| (path, String::new())).collect();
        assert!(check_required_ocr_metadata(&emptied, &assets::Manifest::default()).is_err());
    }

    #[test]
    fn parent_file_destination_conflicts_fail() {
        let (root, mut docs) = fixture();
        docs.document.get_mut(2).unwrap().destination = "LICENSE-MIT/NOTICE".into();
        assert!(check(&root, &docs, &assets::Manifest::default()).iter().any(|p| p.contains("parent file")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_fails() {
        let (root, docs) = fixture();
        let outside = root.with_extension("outside");
        std::fs::write(&outside, "synthetic licence text\n").unwrap();
        std::fs::remove_file(root.join("LICENSE-MIT")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("LICENSE-MIT")).unwrap();
        assert!(check(&root, &docs, &assets::Manifest::default()).iter().any(|p| p.contains("outside checkout")));
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_file(outside).unwrap();
    }
}
