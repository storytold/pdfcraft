//! Verify PDF/UA (ISO 14289): which failure conditions of the Matterhorn Protocol a document meets,
//! with the page or object at fault.
//!
//! [`verify`] reports on every condition in [`CONDITIONS`]. A condition software can decide is
//! Passed or Failed when PdfCraft checks it, and Not checked when it doesn't yet; one that only a
//! person can judge is Needs manual check. Nothing is reported as passed that was not looked at, so
//! a report without failures still leaves the manual and unchecked conditions to someone.
//!
//! The checks read the document only. Several are the Accessibility Checker's own rules, whose
//! findings are failure conditions as they stand (untagged content, figures without alternate
//! text, table and list nesting, heading levels, Unicode mapping); the rest are PDF/UA's own.
//!
//! PDF/UA-2 (ISO 14289-2, on PDF 2.0) is checked where it comes cheaply: its identification, the
//! PDF version, and the PDF/UA-1 conditions whose requirement PDF/UA-2 keeps unchanged. The others
//! are reported as not checked, and no clause of ISO 14289-2 is cited.
//!
//! The checks themselves are in `checks`, the condition table in `conditions`, and the readers of
//! the XMP packet and the XFA form in `xmp`.

mod checks;
mod conditions;
mod xmp;

use std::collections::HashMap;

use pdfcraft_cos::Document;

pub use conditions::CONDITIONS;

use crate::{AND_MORE, Finding, MAX_FINDINGS};

/// Which part of PDF/UA to verify against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Part {
    /// PDF/UA-1, ISO 14289-1 (PDF 1.7).
    Ua1,
    /// PDF/UA-2, ISO 14289-2 (PDF 2.0), checked in part.
    Ua2,
}

impl Part {
    pub const ALL: [Part; 2] = [Part::Ua1, Part::Ua2];

    /// The part number, as `pdfuaid:part` writes it.
    pub fn number(self) -> u8 {
        match self {
            Part::Ua1 => 1,
            Part::Ua2 => 2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Part::Ua1 => "PDF/UA-1",
            Part::Ua2 => "PDF/UA-2",
        }
    }

    pub fn from_number(n: u8) -> Option<Part> {
        Part::ALL.into_iter().find(|p| p.number() == n)
    }

    /// The part a document claims, when it claims PDF/UA-2; PDF/UA-1 otherwise.
    pub fn for_declared(declared: Option<u8>) -> Part {
        if declared == Some(2) { Part::Ua2 } else { Part::Ua1 }
    }
}

/// Whether software can decide a condition or only a person can.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    Machine,
    Human,
}

/// Which parts of PDF/UA a condition is checked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applies {
    /// Both: PDF/UA-2 keeps the requirement as it is.
    Both,
    /// PDF/UA-1. A PDF/UA-2 report lists it as not checked.
    Ua1,
    /// PDF/UA-1 only; a PDF/UA-2 report leaves it out (PDF/UA-2 has its own in its place).
    OnlyUa1,
    /// PDF/UA-2 only.
    OnlyUa2,
}

/// One failure condition: a way a file fails PDF/UA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Condition {
    /// The Matterhorn Protocol 1.1 id, such as `06-002`, or our own id for a condition the protocol
    /// doesn't list (`tagged-pdf`, `ua2-identification`, `ua2-pdf-version`).
    pub id: &'static str,
    /// `id` is a Matterhorn Protocol failure condition.
    pub matterhorn: bool,
    /// The protocol's checkpoint, such as "Metadata".
    pub checkpoint: &'static str,
    /// The section it breaks, as the protocol cites it: `UA1:7.18.3-1` is ISO 14289-1 clause
    /// 7.18.3, its first requirement. `UA2` is ISO 14289-2, whose clauses are not cited.
    pub section: &'static str,
    pub how: How,
    /// PdfCraft checks it (machine conditions only).
    pub checked: bool,
    pub applies: Applies,
    /// What fails, in a sentence.
    pub text: &'static str,
}

impl Condition {
    /// The standard and clause, such as `ISO 14289-1 7.18.3`.
    pub fn clause(&self) -> String {
        match self.section.strip_prefix("UA1:") {
            Some(rest) => format!("ISO 14289-1 {}", rest.split('-').next().unwrap_or(rest)),
            None => "ISO 14289-2".to_owned(),
        }
    }

    pub fn by_id(id: &str) -> Option<&'static Condition> {
        CONDITIONS.iter().find(|c| c.id == id)
    }

    fn in_report(&self, part: Part) -> bool {
        !matches!((self.applies, part), (Applies::OnlyUa2, Part::Ua1) | (Applies::OnlyUa1, Part::Ua2))
    }

    fn checked_for(&self, part: Part) -> bool {
        self.how == How::Machine && self.checked && (part == Part::Ua1 || self.applies != Applies::Ua1)
    }
}

/// What became of one condition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Outcome {
    Passed,
    Failed,
    /// Only a person can judge it.
    Manual,
    /// Software could decide it, but PdfCraft doesn't check it (yet).
    NotChecked,
}

impl Outcome {
    pub const ALL: [Outcome; 4] = [Outcome::Failed, Outcome::Manual, Outcome::NotChecked, Outcome::Passed];

    pub fn label(self) -> &'static str {
        match self {
            Outcome::Passed => "Passed",
            Outcome::Failed => "Failed",
            Outcome::Manual => "Needs manual check",
            Outcome::NotChecked => "Not checked",
        }
    }

    /// The stable id automation uses.
    pub fn id(self) -> &'static str {
        match self {
            Outcome::Passed => "passed",
            Outcome::Failed => "failed",
            Outcome::Manual => "manual",
            Outcome::NotChecked => "not_checked",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct UaResult {
    pub condition: &'static Condition,
    pub outcome: Outcome,
    /// What fails, and where, when the condition failed (the first [`MAX_FINDINGS`], then a count).
    pub findings: Vec<Finding>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UaReport {
    pub part: Part,
    /// The part the metadata declares (`pdfuaid:part`), if any.
    pub declared: Option<u8>,
    /// Every condition of the part, in [`CONDITIONS`] order.
    pub results: Vec<UaResult>,
}

impl UaReport {
    pub fn count(&self, outcome: Outcome) -> usize {
        self.results.iter().filter(|r| r.outcome == outcome).count()
    }

    pub fn with(&self, outcome: Outcome) -> impl Iterator<Item = &UaResult> {
        self.results.iter().filter(move |r| r.outcome == outcome)
    }

    pub fn result(&self, id: &str) -> Option<&UaResult> {
        self.results.iter().find(|r| r.condition.id == id)
    }

    /// No checked condition failed. The manual and unchecked conditions remain, so this alone
    /// does not make the file conform.
    pub fn machine_checks_pass(&self) -> bool {
        self.count(Outcome::Failed) == 0
    }
}

/// Findings per condition: the first [`MAX_FINDINGS`] of them, and how many there were in all.
#[derive(Default)]
struct Collected {
    by_id: HashMap<&'static str, (Vec<Finding>, usize)>,
}

impl Collected {
    fn add(&mut self, id: &'static str, finding: Finding) {
        let (kept, total) = self.by_id.entry(id).or_default();
        *total = total.saturating_add(1);
        if kept.len() < MAX_FINDINGS {
            kept.push(finding);
        }
    }

    /// Findings counted elsewhere: the first of them, and how many there were in all.
    fn extend(&mut self, id: &'static str, findings: Vec<Finding>, total: usize) {
        let (kept, all) = self.by_id.entry(id).or_default();
        let total = total.max(findings.len());
        *all = all.saturating_add(total);
        kept.extend(findings.into_iter().take(MAX_FINDINGS.saturating_sub(kept.len())));
    }

    /// A checker rule's findings, whose summary ("… and n more") is read back into a count.
    fn extend_rule(&mut self, id: &'static str, findings: Vec<Finding>) {
        let mut more = 0usize;
        let mut kept = Vec::with_capacity(findings.len());
        for f in findings {
            if f.pattern == AND_MORE {
                let n = f.values.iter().find(|(hole, _)| *hole == "{n}").and_then(|(_, v)| v.parse::<usize>().ok()).unwrap_or(0);
                more = more.saturating_add(n);
            } else {
                kept.push(f);
            }
        }
        let total = kept.len().saturating_add(more);
        self.extend(id, kept, total);
    }

    fn take(&mut self, id: &str) -> Vec<Finding> {
        let Some((mut kept, total)) = self.by_id.remove(id) else { return Vec::new() };
        if total > kept.len() {
            kept.push(Finding::worded(None, AND_MORE, vec![("{n}", (total - kept.len()).to_string())]));
        }
        kept
    }

    fn failed(&self, id: &str) -> bool {
        self.by_id.get(id).is_some_and(|(_, total)| *total > 0)
    }
}

/// Verify `doc` against `part` of PDF/UA.
pub fn verify(doc: &Document, part: Part) -> UaReport {
    let mut c = Collected::default();
    let declared = checks::run(doc, part, &mut c);
    let results = CONDITIONS
        .iter()
        .filter(|cond| cond.in_report(part))
        .map(|condition| {
            let outcome = match condition.how {
                How::Human => Outcome::Manual,
                How::Machine if !condition.checked_for(part) => Outcome::NotChecked,
                How::Machine if c.failed(condition.id) => Outcome::Failed,
                How::Machine => Outcome::Passed,
            };
            let findings = if outcome == Outcome::Failed { c.take(condition.id) } else { Vec::new() };
            UaResult { condition, outcome, findings }
        })
        .collect();
    UaReport { part, declared, results }
}

#[cfg(test)]
mod tests;
