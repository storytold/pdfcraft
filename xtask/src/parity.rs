//! `cargo xtask parity`: validate `parity/acrobat-features.toml` and report progress (M0.8).
//!
//! Checks:
//! - schema: unique ids (`area.kebab-name`), known area letters, tiers, milestones and statuses;
//! - for `shipped` and `partial` features, every listed command exists in the engine registry and
//!   every tool exists in the automation table (so the claim is reachable from the UI and
//!   headlessly);
//! - `shipped` features cite at least one test, and every cited `path::fn` exists.
//!
//! The report gives the shipped share per tier and per area (partial counts half in the
//! weighted figure). `--json` prints the numbers for STATUS.md; `--partial` lists partial
//! features with their notes. Fails on any validation error.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::gates::root;

const AREAS: &[(&str, &str)] = &[
    ("A", "Core model and fidelity"),
    ("B", "Viewing and navigation"),
    ("C", "Content editing"),
    ("D", "Organize pages"),
    ("E", "Comments and review"),
    ("F", "Forms and JavaScript"),
    ("G", "Protect, redact, sanitize"),
    ("H", "Digital signatures"),
    ("I", "Scan and OCR"),
    ("J", "Create and export"),
    ("K", "Optimize and standards"),
    ("L", "Print and print production"),
    ("M", "Accessibility"),
    ("N", "Compare, automation, AI, misc"),
];
const TIERS: &[&str] = &["P0", "P1", "P2", "P3", "N/A"];
const STATUSES: &[&str] = &["planned", "partial", "shipped", "na"];

#[derive(Debug, Deserialize)]
struct File {
    schema: Schema,
    #[serde(rename = "feature")]
    features: Vec<Feature>,
}

#[derive(Debug, Deserialize)]
struct Schema {
    version: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Feature {
    pub id: String,
    pub title: String,
    pub area: String,
    pub tier: String,
    pub milestone: String,
    pub status: String,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub notes: String,
}

/// Quoted ids that follow `prefix(` in a source file: `c("page.rotate", …)` → `page.rotate`.
fn quoted_after(src: &str, prefixes: &[&str]) -> HashSet<String> {
    let mut out = HashSet::new();
    for p in prefixes {
        let pat = format!("{p}(");
        let mut from = 0;
        while let Some(i) = src[from..].find(&pat).map(|i| i + from) {
            from = i + pat.len();
            // Whole identifiers only: `t(` must not match inside `ct(`.
            let boundary = src[..i].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            // The id may start on the next line (`t(\n    "doc_save",`).
            let rest = src[from..].trim_start();
            if boundary
                && let Some(body) = rest.strip_prefix('"')
                && let Some(end) = body.find('"')
            {
                out.insert(body[..end].to_string());
            }
        }
    }
    out
}

fn valid_id(id: &str) -> bool {
    let Some((area, name)) = id.split_once('.') else { return false };
    !area.is_empty()
        && area.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && !name.is_empty()
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
}

fn valid_milestone(m: &str) -> bool {
    m == "none" || m.strip_prefix('M').and_then(|n| n.parse::<u32>().ok()).is_some_and(|n| n <= 14)
}

/// Why an `evidence` entry (`path.rs::test_fn`) does not resolve to a test in the source tree, if it doesn't.
fn evidence_error(ev: &str, repo: &Path) -> Option<String> {
    let Some((path, func)) = ev.split_once("::") else {
        return Some(format!("evidence {ev:?} must be `path.rs::test_fn`"));
    };
    match std::fs::read_to_string(repo.join(path)) {
        Ok(src) if src.contains(&format!("fn {func}(")) => None,
        Ok(_) => Some(format!("evidence: no `fn {func}` in {path}")),
        Err(_) => Some(format!("evidence: {path} does not exist")),
    }
}

/// Validate features against the registry, tool table and source tree; returns the errors.
pub fn validate(features: &[Feature], commands: &HashSet<String>, tools: &HashSet<String>, repo: &Path) -> Vec<String> {
    let mut errors = Vec::new();
    let mut ids = HashSet::new();
    for f in features {
        let e = |m: String| format!("{}: {m}", f.id);
        if !ids.insert(f.id.as_str()) {
            errors.push(e("duplicate id".into()));
        }
        if !valid_id(&f.id) {
            errors.push(e("id must look like `area.kebab-name`".into()));
        }
        if f.title.trim().is_empty() {
            errors.push(e("empty title".into()));
        }
        if !AREAS.iter().any(|(a, _)| *a == f.area) {
            errors.push(e(format!("unknown area {:?} (A–N)", f.area)));
        }
        if !TIERS.contains(&f.tier.as_str()) {
            errors.push(e(format!("unknown tier {:?}", f.tier)));
        }
        if !STATUSES.contains(&f.status.as_str()) {
            errors.push(e(format!("unknown status {:?}", f.status)));
        }
        if !valid_milestone(&f.milestone) {
            errors.push(e(format!("milestone {:?} must be M0–M14 or none", f.milestone)));
        }
        if matches!(f.status.as_str(), "shipped" | "partial") {
            for c in f.commands.iter().filter(|c| !commands.contains(*c)) {
                errors.push(e(format!("command {c:?} is not in the engine registry")));
            }
            for t in f.tools.iter().filter(|t| !tools.contains(*t)) {
                errors.push(e(format!("tool {t:?} is not in the automation table")));
            }
        }
        if f.status == "shipped" && f.evidence.is_empty() {
            errors.push(e("shipped features must cite at least one test in `evidence`".into()));
        }
        for ev in &f.evidence {
            if let Some(m) = evidence_error(ev, repo) {
                errors.push(e(m));
            }
        }
    }
    errors
}

pub fn run(args: &[String]) -> Result<()> {
    let repo = root();
    let path = repo.join("parity/acrobat-features.toml");
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let file: File = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    if file.schema.version != 1 {
        bail!("unsupported parity schema version {}", file.schema.version);
    }
    let commands = quoted_after(&std::fs::read_to_string(repo.join("crates/engine/src/commands.rs"))?, &["c", "ct"]);
    let tools = quoted_after(&std::fs::read_to_string(repo.join("crates/automation/src/tools.rs"))?, &["t"]);
    let errors = validate(&file.features, &commands, &tools, &repo);

    // Progress: per tier and per area. N/A tier and `na` status are excluded from percentages.
    let counted: Vec<&Feature> = file.features.iter().filter(|f| f.status != "na" && f.tier != "N/A").collect();
    let score = |fs: &[&Feature]| {
        let shipped = fs.iter().filter(|f| f.status == "shipped").count();
        let partial = fs.iter().filter(|f| f.status == "partial").count();
        let pct = |x: f64| if fs.is_empty() { 0.0 } else { 100.0 * x / fs.len() as f64 };
        (fs.len(), shipped, partial, pct(shipped as f64), pct(shipped as f64 + partial as f64 / 2.0))
    };
    let mut by_tier = BTreeMap::new();
    for t in TIERS.iter().filter(|t| **t != "N/A") {
        let fs: Vec<&Feature> = counted.iter().copied().filter(|f| f.tier == *t).collect();
        by_tier.insert(*t, score(&fs));
    }
    let mut by_area = Vec::new();
    for (a, name) in AREAS {
        let fs: Vec<&Feature> = counted.iter().copied().filter(|f| f.area == *a).collect();
        by_area.push((*a, *name, score(&fs)));
    }
    let total = score(&counted);
    let without_headless: Vec<&str> = file.features.iter().filter(|f| f.status == "shipped" && f.tools.is_empty()).map(|f| f.id.as_str()).collect();

    if args.iter().any(|a| a == "--json") {
        let json = serde_json::json!({
            "features": file.features.len(),
            "total": { "counted": total.0, "shipped": total.1, "partial": total.2, "shipped_pct": total.3, "weighted_pct": total.4 },
            "tiers": by_tier.iter().map(|(t, s)| (t.to_string(), serde_json::json!({ "counted": s.0, "shipped": s.1, "partial": s.2, "shipped_pct": s.3 }))).collect::<serde_json::Map<_, _>>(),
            "errors": errors,
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        println!("parity: {} features ({} counted; {} na)", file.features.len(), counted.len(), file.features.len() - counted.len());
        println!("\n  tier  features  shipped  partial  shipped%  weighted%");
        for (t, s) in &by_tier {
            println!("  {t:<4}  {:>8}  {:>7}  {:>7}  {:>7.1}%  {:>8.1}%", s.0, s.1, s.2, s.3, s.4);
        }
        println!("  all   {:>8}  {:>7}  {:>7}  {:>7.1}%  {:>8.1}%", total.0, total.1, total.2, total.3, total.4);
        println!("\n  area                                features  shipped  partial  weighted%");
        for (a, name, s) in &by_area {
            println!("  {a} {name:<33} {:>8}  {:>7}  {:>7}  {:>8.1}%", s.0, s.1, s.2, s.4);
        }
        if args.iter().any(|a| a == "--partial") {
            println!("\npartial features and what is missing:");
            for f in file.features.iter().filter(|f| f.status == "partial") {
                println!("  {:<40} {}", f.id, f.notes);
            }
        }
        if !without_headless.is_empty() {
            println!(
                "\nnote: {} shipped feature(s) have no automation tool yet (AGENTS.md §3): {}",
                without_headless.len(),
                without_headless.join(", ")
            );
        }
    }
    if !errors.is_empty() {
        for e in &errors {
            eprintln!("parity error: {e}");
        }
        bail!("{} parity error(s)", errors.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(id: &str, status: &str) -> Feature {
        Feature {
            id: id.into(),
            title: "T".into(),
            area: "D".into(),
            tier: "P0".into(),
            milestone: "M4".into(),
            status: status.into(),
            commands: vec![],
            tools: vec![],
            evidence: vec![],
            notes: String::new(),
        }
    }

    #[test]
    fn committed_manifest_is_well_formed() {
        let path = root().join("parity/acrobat-features.toml");
        let text = std::fs::read_to_string(&path).expect("read the committed parity manifest");
        let _: File = toml::from_str(&text).expect("the committed parity manifest must be valid TOML");
    }

    #[test]
    fn registry_ids_are_extracted_from_source() {
        let src =
            "c(\"file.open\", \"Open…\", FILE, None), ct(\"edit.undo\", \"Undo\"), t(\"doc_open\", \"Open\"), t(\n    \"doc_save\",), format(x)";
        assert_eq!(quoted_after(src, &["c", "ct"]), HashSet::from(["file.open".to_string(), "edit.undo".to_string()]));
        assert_eq!(quoted_after(src, &["t"]), HashSet::from(["doc_open".to_string(), "doc_save".to_string()]));
    }

    #[test]
    fn validation_catches_false_claims() {
        let repo = root();
        let cmds = HashSet::from(["page.rotate".to_string()]);
        let tools = HashSet::from(["page_rotate".to_string()]);
        let mut ok = f("organize.rotate", "shipped");
        ok.commands = vec!["page.rotate".into()];
        ok.tools = vec!["page_rotate".into()];
        ok.evidence = vec!["xtask/src/parity.rs::validation_catches_false_claims".into()];
        assert!(validate(&[ok], &cmds, &tools, &repo).is_empty());

        let mut bad = f("organize.Rotate", "shipped");
        bad.commands = vec!["page.spin".into()];
        bad.evidence = vec!["xtask/src/parity.rs::no_such_test".into(), "nowhere.rs::x".into(), "nopath".into()];
        let errs = validate(&[bad, f("organize.x", "planned"), f("organize.x", "bogus")], &cmds, &tools, &repo);
        let all = errs.join("\n");
        for needle in ["kebab", "registry", "no `fn no_such_test`", "does not exist", "path.rs::test_fn", "duplicate id", "unknown status"] {
            assert!(all.contains(needle), "missing {needle:?} in:\n{all}");
        }
        assert!(validate(&[f("organize.y", "shipped")], &cmds, &tools, &repo)[0].contains("at least one test"));
    }
}
