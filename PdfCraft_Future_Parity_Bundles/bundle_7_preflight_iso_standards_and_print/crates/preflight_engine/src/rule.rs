//! Preflight Rule and Violation Definitions.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreflightSeverity {
    Info,
    Warning,
    Error, // Blocks conformance
}

#[derive(Debug, Clone)]
pub struct PreflightViolation {
    pub rule_id: String,
    pub severity: PreflightSeverity,
    pub message: String,
    pub object_id: Option<u32>,
    pub page_number: Option<usize>,
    pub fixable: bool,
}

pub trait PreflightRule: Send + Sync {
    fn id(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn severity(&self) -> PreflightSeverity;
    fn check(&self, doc_text: &str) -> Vec<PreflightViolation>;
    fn fixup(&self, doc_text: &mut String) -> Result<bool, &'static str>;
}
