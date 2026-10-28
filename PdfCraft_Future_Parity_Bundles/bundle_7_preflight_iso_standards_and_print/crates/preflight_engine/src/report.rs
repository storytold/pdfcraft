//! Preflight Audit Report Generator (JSON & XML).

use crate::rule::{PreflightSeverity, PreflightViolation};

#[derive(Debug, Clone)]
pub struct PreflightAuditReport {
    pub profile_name: String,
    pub passed: bool,
    pub violations: Vec<PreflightViolation>,
}

impl PreflightAuditReport {
    pub fn new(profile_name: impl Into<String>, violations: Vec<PreflightViolation>) -> Self {
        let passed = !violations.iter().any(|v| v.severity == PreflightSeverity::Error);
        Self {
            profile_name: profile_name.into(),
            passed,
            violations,
        }
    }

    /// Serializes audit report to clean JSON.
    pub fn to_json(&self) -> String {
        let mut json = String::new();
        json.push_str("{\n");
        json.push_str(&format!("  \"profile\": \"{}\",\n", self.profile_name));
        json.push_str(&format!("  \"passed\": {},\n", self.passed));
        json.push_str("  \"violations\": [\n");

        for (i, v) in self.violations.iter().enumerate() {
            json.push_str("    {\n");
            json.push_str(&format!("      \"rule\": \"{}\",\n", v.rule_id));
            json.push_str(&format!("      \"severity\": \"{:?}\",\n", v.severity));
            json.push_str(&format!("      \"message\": \"{}\",\n", v.message.replace('"', "\\\"")));
            json.push_str(&format!("      \"fixable\": {}\n", v.fixable));
            json.push_str("    }");
            if i + 1 < self.violations.len() {
                json.push(',');
            }
            json.push('\n');
        }

        json.push_str("  ]\n}");
        json
    }
}
