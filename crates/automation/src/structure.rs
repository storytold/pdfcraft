//! Validate structure: the document checked against the Arlington PDF Model.

use pdfcraft_engine::structure::{MODEL_COMMIT, Options, Report, Severity};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError, failed};

/// The most findings an agent can ask for per severity.
const MAX_FINDINGS: i64 = 5_000;

/// A structure report as the `structure_validate` tool and `pdfcraft-cli validate` give it.
pub fn report_json(r: &Report) -> Value {
    let findings: Vec<Value> = r
        .findings
        .iter()
        .map(|f| {
            json!({
                "severity": f.severity.id(),
                "rule": f.rule.id(),
                "message": f.message(),
                "type": f.type_name,
                "key": f.key,
                "path": f.path,
                "object": f.object.map(|o| o.to_string()),
                "page": f.page.map(|p| p + 1),
                "bbox_user_space": f.rect,
            })
        })
        .collect();
    let not_checked: Vec<Value> = r.not_checked.iter().map(|(what, n)| json!({ "predicate": what, "count": n })).collect();
    json!({
        "valid": r.is_valid(),
        "version": r.version.to_string(),
        "version_assumed": r.version_assumed,
        "model": format!("Arlington PDF Model {MODEL_COMMIT}"),
        "errors": r.errors,
        "warnings": r.warnings,
        "infos": r.infos,
        "objects": r.objects,
        "findings": findings,
        "not_checked": not_checked,
        "stopped_early": r.stopped_early,
        "truncated": r.truncated,
    })
}

impl Automation {
    pub(crate) fn structure_validate(&self, a: &Args) -> Result<Value> {
        let mut options = Options::default();
        if let Some(n) = a.opt_int("max_findings")? {
            if !(1..=MAX_FINDINGS).contains(&n) {
                return Err(ToolError::InvalidArgs(format!("max_findings must be 1 to {MAX_FINDINGS}")));
            }
            options.max_findings = usize::try_from(n).unwrap_or(1);
        }
        if let Some(unknown) = a.opt_bool("unknown_keys")? {
            options.unknown_keys = unknown;
        }
        let least = match a.opt_str("severity")?.unwrap_or("info") {
            "error" => Severity::Error,
            "warning" => Severity::Warning,
            "info" => Severity::Info,
            other => return Err(ToolError::InvalidArgs(format!("unknown severity {other:?} (error, warning or info)"))),
        };
        let doc = self.doc(a)?;
        let mut report = doc.validate_structure(&options).ok_or_else(|| failed("the document can't be read for checking"))?;
        report.findings.retain(|f| f.severity <= least);
        Ok(report_json(&report))
    }
}
