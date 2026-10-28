//! Preflight Rule Profile Execution Runner.

use crate::report::PreflightAuditReport;
use crate::rule::PreflightRule;

pub struct PreflightRunner {
    rules: Vec<Box<dyn PreflightRule>>,
}

impl Default for PreflightRunner {
    fn default() -> Self {
        Self { rules: Vec::new() }
    }
}

impl PreflightRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_rule(&mut self, rule: Box<dyn PreflightRule>) {
        self.rules.push(rule);
    }

    /// Evaluates all rules against the document.
    pub fn run_analysis(&self, profile_name: &str, doc_text: &str) -> PreflightAuditReport {
        let mut violations = Vec::new();
        for rule in &self.rules {
            let mut v = rule.check(doc_text);
            violations.append(&mut v);
        }
        PreflightAuditReport::new(profile_name, violations)
    }

    /// Executes automated fixups for all fixable rules.
    pub fn run_fixups(&self, doc_text: &mut String) -> usize {
        let mut fixups_applied = 0;
        for rule in &self.rules {
            if let Ok(true) = rule.fixup(doc_text) {
                fixups_applied += 1;
            }
        }
        fixups_applied
    }
}
