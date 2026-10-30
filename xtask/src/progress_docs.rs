//! One status line per progress doc, one row per milestone, enforced by `cargo xtask progress-docs`
//! (craftrules `standards/progress-docs.md`).
//!
//! A change adds its news to the existing status line and milestone row; it does not stack a new
//! `> **Last reviewed:**` line or a second `| M3 |` row on top, which is what made these files
//! conflict with almost every PR.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::bail;

/// The docs that carry the status line and so must have one; other docs may have none.
const PROGRESS_DOCS: &[&str] = &["ROADMAP.md", "docs/gaps.md", "docs/roadmap.md", "docs/target-app-parity.md"];

/// Problems in one doc's text: not exactly one status line when `required` (never more than one
/// otherwise), or a milestone that has several rows. Indented lines count.
fn check(text: &str, required: bool) -> Vec<String> {
    let mut problems = Vec::new();
    let status = text.lines().filter(|l| l.trim_start().starts_with("> **Last reviewed:**")).count();
    if status > 1 || (required && status == 0) {
        problems.push(format!("{status} `Last reviewed` status lines (want one)"));
    }
    let mut seen = BTreeSet::new();
    let mut dup = BTreeSet::new();
    for id in text.lines().filter_map(milestone_id) {
        if !seen.insert(id) {
            dup.insert(id);
        }
    }
    for id in dup {
        problems.push(format!("duplicate milestone row {id}"));
    }
    problems
}

/// `M3` for a table row that starts `| M3 |`.
fn milestone_id(line: &str) -> Option<&str> {
    let cell = line.trim_start().strip_prefix('|')?.split('|').next()?.trim();
    let digits = cell.strip_prefix('M')?;
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some(cell)
}

pub fn run(_: &[String]) -> anyhow::Result<()> {
    let root = crate::gates::root();
    let mut files = vec![root.join("ROADMAP.md")];
    files.extend(markdown_in(&root.join("docs"))?);
    let mut failures = 0;
    for f in &files {
        let text = std::fs::read_to_string(f)?;
        let rel = f.strip_prefix(&root).unwrap_or(f);
        let required = PROGRESS_DOCS.iter().any(|d| rel == Path::new(d));
        for p in check(&text, required) {
            println!("  - {}: {p}", rel.display());
            failures += 1;
        }
    }
    if failures > 0 {
        bail!("{failures} progress-doc problem(s); fold the extra lines and rows into the existing ones");
    }
    println!("OK: {} progress docs, one status line and one row per milestone.", files.len());
    Ok(())
}

fn markdown_in(dir: &Path) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{check, milestone_id};

    const ONE: &str = "# T\n\n> **Last reviewed:** 2026-10-25 | **Change:** x\n\n| M | Milestone |\n|---|---|\n| M2 | a |\n| M3 | b |\n";

    #[test]
    fn one_status_line_and_unique_rows_pass() {
        assert!(check(ONE, true).is_empty());
        assert!(check("no status, no table", false).is_empty());
    }

    #[test]
    fn stacked_status_lines_fail() {
        let text = format!("{ONE}> **Last reviewed:** 2026-10-10 · **Change:** y\n");
        assert_eq!(check(&text, true), ["2 `Last reviewed` status lines (want one)"]);
    }

    #[test]
    fn a_milestone_with_two_rows_fails_once() {
        let text = format!("{ONE}| M3 | c |\n| M3 | d |\n");
        assert_eq!(check(&text, true), ["duplicate milestone row M3"]);
    }

    #[test]
    fn a_progress_doc_must_have_its_status_line() {
        assert_eq!(check("# T\n", true), ["0 `Last reviewed` status lines (want one)"]);
    }

    #[test]
    fn indented_duplicates_are_still_duplicates() {
        let text = format!("{ONE} > **Last reviewed:** 2026-10-10\n | M3 | c |\n");
        assert_eq!(check(&text, false).len(), 2, "{:?}", check(&text, false));
        assert_eq!(milestone_id("  | M3 | x |"), Some("M3"));
    }

    #[test]
    fn only_milestone_rows_count() {
        assert_eq!(milestone_id("| M12 | x |"), Some("M12"));
        for not in ["| M | Milestone |", "| Milestone | x |", "| M1a | x |", "M3 | x", "| 3 | x |"] {
            assert_eq!(milestone_id(not), None, "{not}");
        }
    }
}
