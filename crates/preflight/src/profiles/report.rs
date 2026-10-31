//! Preflight reports: plain text (which the engine also turns into a PDF) and JSON.

use serde_json::{Value, json};

use super::{Results, Severity, Status};
use crate::Words;

/// Every wording the text report says around the results, for an interface to translate.
pub const REPORT_WORDS: &[&str] = &[
    "Preflight report",
    "Document: {name}",
    "Profile: {profile}",
    "Date: {date}",
    "Summary: {errors} errors, {warnings} warnings, {info} information",
    "No problems found",
    "Checks passed",
    "Checks not run",
    " (page {p})",
    "Fixup: {fixup}",
    "Content not read to the end on pages: {pages}",
];

/// The results as a plain-text report, said in the language `words` translates into.
pub fn text(results: &Results, document: &str, date: &str, words: Words) -> String {
    let say = |pattern: &str, values: &[(&str, String)]| super::fill(&words(pattern), values);
    let mut out = String::new();
    out.push_str(&words("Preflight report"));
    out.push('\n');
    out.push_str(&say("Document: {name}", &[("{name}", document.to_owned())]));
    out.push('\n');
    out.push_str(&say("Profile: {profile}", &[("{profile}", words(&results.profile.name))]));
    out.push('\n');
    out.push_str(&say("Date: {date}", &[("{date}", date.to_owned())]));
    out.push('\n');
    out.push_str(&say(
        "Summary: {errors} errors, {warnings} warnings, {info} information",
        &[
            ("{errors}", results.count(Severity::Error).to_string()),
            ("{warnings}", results.count(Severity::Warning).to_string()),
            ("{info}", results.count(Severity::Info).to_string()),
        ],
    ));
    out.push('\n');
    let found = |s: Severity| results.checks.iter().filter(move |c| c.severity == s && c.status == Status::Found);
    if Severity::ALL.into_iter().all(|s| found(s).next().is_none()) {
        out.push('\n');
        out.push_str(&words("No problems found"));
        out.push('\n');
    }
    for severity in Severity::ALL {
        let mut checks = found(severity).peekable();
        if checks.peek().is_none() {
            continue;
        }
        out.push('\n');
        out.push_str(&words(severity.label()));
        out.push('\n');
        for c in checks {
            let params: Vec<String> = c.check.params().into_iter().map(|(name, value)| format!("{}: {value}", words(name))).collect();
            let params = if params.is_empty() { String::new() } else { format!(" [{}]", params.join(", ")) };
            out.push_str(&format!("  {} ({}){params}\n", c.check.said(words), c.hits.len()));
            for f in c.fixups() {
                out.push_str("    ");
                out.push_str(&say("Fixup: {fixup}", &[("{fixup}", words(f.label()))]));
                out.push('\n');
            }
            for h in &c.hits {
                let page = h.page.map(|p| say(" (page {p})", &[("{p}", (p + 1).to_string())])).unwrap_or_default();
                // Objects as PDF names them: the same in every language.
                let object = h.object.map(|n| format!(" · {n} 0 obj")).unwrap_or_default();
                out.push_str(&format!("    - {}{page}{object}\n", h.said(words)));
                for (name, value) in &h.details {
                    out.push_str(&format!("      {}: {value}\n", words(name)));
                }
            }
        }
    }
    let passed: Vec<String> = results.checks.iter().filter(|c| c.status == Status::Passed).map(|c| c.check.said(words)).collect();
    if !passed.is_empty() {
        out.push('\n');
        out.push_str(&words("Checks passed"));
        out.push('\n');
        for p in passed {
            out.push_str(&format!("  {p}\n"));
        }
    }
    let skipped: Vec<String> = results
        .checks
        .iter()
        .filter(|c| c.status == Status::Skipped)
        .map(|c| match c.note {
            Some(note) => format!("{}: {}", c.check.said(words), words(note)),
            None => c.check.said(words),
        })
        .collect();
    if !skipped.is_empty() {
        out.push('\n');
        out.push_str(&words("Checks not run"));
        out.push('\n');
        for s in skipped {
            out.push_str(&format!("  {s}\n"));
        }
    }
    if !results.unread_pages.is_empty() {
        let pages: Vec<String> = results.unread_pages.iter().map(|p| (p + 1).to_string()).collect();
        out.push('\n');
        out.push_str(&say("Content not read to the end on pages: {pages}", &[("{pages}", pages.join(", "))]));
        out.push('\n');
    }
    out
}

/// The results as JSON, in English, pages counted from 1.
pub fn json(results: &Results, document: &str) -> Value {
    let checks: Vec<Value> = results
        .checks
        .iter()
        .map(|c| {
            let hits: Vec<Value> = c
                .hits
                .iter()
                .map(|h| {
                    let details: serde_json::Map<String, Value> = h.details.iter().map(|(k, v)| ((*k).to_owned(), json!(v))).collect();
                    json!({
                        "message": h.message,
                        "page": h.page.map(|p| p + 1),
                        "object": h.object,
                        "bbox": h.bbox,
                        "rule": h.rule,
                        "details": details,
                        "fixup": h.fixup.map(|f| f.id()),
                    })
                })
                .collect();
            json!({
                "check": c.check.id(),
                "title": c.check.said(&crate::in_english),
                "severity": c.severity.id(),
                "status": c.status.id(),
                "params": serde_json::to_value(&c.check).unwrap_or(Value::Null),
                "note": c.note,
                "fixups": c.fixups().iter().map(|f| serde_json::to_value(f).unwrap_or(Value::Null)).collect::<Vec<_>>(),
                "hits": hits,
            })
        })
        .collect();
    json!({
        "document": document,
        "profile": { "id": results.profile.id, "name": results.profile.name, "description": results.profile.description },
        "passed": results.passed(),
        "summary": {
            "errors": results.count(Severity::Error),
            "warnings": results.count(Severity::Warning),
            "info": results.count(Severity::Info),
        },
        "pages": results.pages,
        "unread_pages": results.unread_pages.iter().map(|p| p + 1).collect::<Vec<_>>(),
        "checks": checks,
    })
}
