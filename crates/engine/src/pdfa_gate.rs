//! The PDF/A gate: the one place the engine asks whether an open document declares PDF/A, and
//! which of that part's rules the changes made to it break.
//!
//! A file that declares PDF/A (`pdfaid:part` and `pdfaid:conformance` in its XMP, ISO 19005-4
//! §6.7.3 and ISO 19005-2 §6.6.4, as [`pdfcraft_preflight::declared`] reads them) tells every
//! reader it meets that part's rules, and it keeps saying so after any edit. The gate keeps the
//! document as it was when it first declared PDF/A in this session (when it was opened, or after
//! the edit that identified it, such as Convert to PDF/A) and checks the document as it is now
//! against the declared part with [`pdfcraft_preflight::verify`]. What fails now and didn't fail
//! then is what the changes broke ([`PdfaCheck::breaks`]): the automation tools return it with
//! every change and save, and an interface can ask the same place before it lets one through.
//! How a save writes a PDF/A-4 file's metadata is decided next to it, in `crate::metadata`.
//!
//! Only a rule the document didn't break before counts, so a file that already fails a check,
//! or a check PdfCraft gets wrong on it, doesn't warn on every edit.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use pdfcraft_cos::{Document as Cos, ReadState};
use pdfcraft_preflight::{Issue, Level};

use crate::{DocId, Session};

/// What a document declares about PDF/A: `pdfaid:part` and `pdfaid:conformance` (upper case,
/// empty for a PDF/A-4 file that names no conformance level).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdfaClaim {
    pub part: u8,
    pub conformance: String,
}

impl PdfaClaim {
    /// What `cos` declares, if it declares PDF/A.
    pub fn of(cos: &Cos) -> Option<PdfaClaim> {
        pdfcraft_preflight::declared(cos).pdfa.map(|(part, conformance)| PdfaClaim { part, conformance })
    }

    /// The claim as it is written: "PDF/A-2b", "PDF/A-4", "PDF/A-4f".
    pub fn label(&self) -> String {
        format!("PDF/A-{}{}", self.part, self.conformance.to_ascii_lowercase())
    }

    /// The level changes are checked against, when PdfCraft checks this part: PDF/A-4, 4e and
    /// 4f as declared (ISO 19005-4); PDF/A-2 and PDF/A-3 at level b, whose rules levels u and a
    /// also have (ISO 19005-2 §2), so whatever breaks level b breaks them too. `None` for a part
    /// [`pdfcraft_preflight::verify`] doesn't check (PDF/A-1) or a level the part doesn't have.
    pub fn checked_as(&self) -> Option<Level> {
        match (self.part, self.conformance.as_str()) {
            (2, "A" | "B" | "U") => Some(Level::A2b),
            (3, "A" | "B" | "U") => Some(Level::A3b),
            (4, "") => Some(Level::A4),
            (4, "E") => Some(Level::A4e),
            (4, "F") => Some(Level::A4f),
            _ => None,
        }
    }
}

/// What an open document declares about PDF/A, and what its changes break.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfaCheck {
    pub claim: PdfaClaim,
    /// The level the changes were checked against ([`PdfaClaim::checked_as`]); `None` when
    /// PdfCraft doesn't check the declared part, and [`breaks`](Self::breaks) is then empty.
    pub checked_as: Option<Level>,
    /// The rules the document breaks now that it didn't break when it first declared PDF/A in
    /// this session, in the order [`pdfcraft_preflight::verify`] lists them.
    pub breaks: Vec<Issue>,
}

impl PdfaCheck {
    /// The breaks `earlier` (what an earlier check of the same document found broken) doesn't
    /// account for, matched as the baseline is: what has broken since.
    pub fn breaks_since(&self, earlier: &[Issue]) -> Vec<Issue> {
        breaks(earlier, &self.breaks)
    }
}

/// Issues of one check, shared between the cache and its callers.
type Issues = Arc<Vec<Issue>>;

/// The gate's state for one open document.
#[derive(Default)]
pub(crate) struct Gate {
    /// The document as it was when it first declared PDF/A in this session. Kept only for a
    /// document that declares PDF/A (a copy-on-write clone: it shares what the edits didn't
    /// change).
    baseline: Option<Cos>,
    cache: Mutex<Cache>,
}

/// Checks already run, so asking again after a change that wasn't one costs nothing.
#[derive(Default)]
struct Cache {
    baseline: Option<(Level, Issues)>,
    current: Option<(ReadState, Level, Issues)>,
}

impl Gate {
    /// The gate of a document opened as `cos`.
    pub(crate) fn opened(cos: Option<&Cos>) -> Gate {
        Gate { baseline: cos.filter(|c| PdfaClaim::of(c).is_some()).cloned(), cache: Mutex::default() }
    }

    /// After an edit left the document as `cos`: the first state that declares PDF/A becomes the
    /// baseline, so an edit that identifies the file (Convert to PDF/A) is measured from the
    /// file it identified, not from the file before it.
    pub(crate) fn edited(&mut self, cos: &Cos) {
        if self.baseline.is_none() && PdfaClaim::of(cos).is_some() {
            self.baseline = Some(cos.clone());
        }
    }

    /// The check of the document as it is now (`cos`).
    pub(crate) fn check(&self, cos: &Cos) -> Option<PdfaCheck> {
        let claim = PdfaClaim::of(cos)?;
        let Some(level) = claim.checked_as() else {
            return Some(PdfaCheck { claim, checked_as: None, breaks: Vec::new() });
        };
        let cached = self.lock().current.as_ref().filter(|(s, l, _)| s.matches(cos) && *l == level).map(|(_, _, i)| i.clone());
        let now = match cached {
            Some(issues) => issues,
            None => {
                let issues: Issues = Arc::new(pdfcraft_preflight::verify(cos, level));
                self.lock().current = Some((cos.read_state(), level, issues.clone()));
                issues
            }
        };
        // Not changed since the baseline (just opened, or every edit undone): nothing is new,
        // and the baseline's own check is this one.
        if self.baseline.as_ref().is_some_and(|b| b.read_state().matches(cos)) {
            self.lock().baseline = Some((level, now));
            return Some(PdfaCheck { claim, checked_as: Some(level), breaks: Vec::new() });
        }
        let breaks = breaks(&self.baseline_issues(level), &now);
        Some(PdfaCheck { claim, checked_as: Some(level), breaks })
    }

    /// The check of a copy of the document about to be written on its own (an optimized or
    /// reduced copy), measured from the same baseline. Not cached: each copy is new.
    pub(crate) fn check_copy(&self, copy: &Cos) -> Option<PdfaCheck> {
        let claim = PdfaClaim::of(copy)?;
        let Some(level) = claim.checked_as() else {
            return Some(PdfaCheck { claim, checked_as: None, breaks: Vec::new() });
        };
        let now = pdfcraft_preflight::verify(copy, level);
        let breaks = breaks(&self.baseline_issues(level), &now);
        Some(PdfaCheck { claim, checked_as: Some(level), breaks })
    }

    /// What the baseline breaks at `level` (nothing without a baseline: every issue is then new).
    fn baseline_issues(&self, level: Level) -> Issues {
        if let Some((l, issues)) = &self.lock().baseline
            && *l == level
        {
            return issues.clone();
        }
        let issues: Issues = Arc::new(self.baseline.as_ref().map(|b| pdfcraft_preflight::verify(b, level)).unwrap_or_default());
        self.lock().baseline = Some((level, issues.clone()));
        issues
    }

    /// The cache. A check that panicked while holding it left at worst a stale entry, which is
    /// keyed by the state it was read from, so the lock is taken back rather than given up.
    fn lock(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The issues of `now` that `before` doesn't account for.
///
/// Each issue of `before` accounts for one issue of `now`: first one with the same clause and
/// message, then, among those left, an issue on a page with the same clause and wording
/// ([`Issue::pattern`]), so an issue that only names another page after pages were deleted or
/// moved stays accounted for. An issue that names no page (a font, an object) is matched by its
/// message alone. What is left over is new.
pub(crate) fn breaks(before: &[Issue], now: &[Issue]) -> Vec<Issue> {
    let mut exact: HashMap<(&str, &str), usize> = HashMap::new();
    for i in before {
        *exact.entry((i.clause, i.message.as_str())).or_default() += 1;
    }
    let mut unmatched = Vec::new();
    for i in now {
        match exact.get_mut(&(i.clause, i.message.as_str())) {
            Some(n) if *n > 0 => *n -= 1,
            _ => unmatched.push(i),
        }
    }
    // What `before` has left over on pages, by clause and wording.
    let mut loose: HashMap<(&str, &str), usize> = HashMap::new();
    for i in before {
        if let Some(n) = exact.get_mut(&(i.clause, i.message.as_str()))
            && *n > 0
        {
            *n -= 1;
            if i.page.is_some() {
                *loose.entry((i.clause, i.pattern)).or_default() += 1;
            }
        }
    }
    unmatched
        .into_iter()
        .filter(|i| match loose.get_mut(&(i.clause, i.pattern)).filter(|_| i.page.is_some()) {
            Some(n) if *n > 0 => {
                *n -= 1;
                false
            }
            _ => true,
        })
        .cloned()
        .collect()
}

impl Session {
    /// What document `id` declares about PDF/A and the rules of that part its changes break
    /// ([`PdfaCheck`]); `None` when it declares no PDF/A part, or is open read-only. Checked
    /// once per state of the document and kept until it changes.
    pub fn pdfa_check(&self, id: DocId) -> Option<PdfaCheck> {
        let doc = self.get(id)?;
        doc.pdfa_gate.check(&doc.editor.as_ref()?.cos)
    }

    /// The same for `bytes`, a copy of document `id` written on its own (optimized, reduced):
    /// `None` when the copy declares no PDF/A part or can't be read back.
    pub fn pdfa_check_copy(&self, id: DocId, bytes: &Arc<Vec<u8>>) -> Option<PdfaCheck> {
        let doc = self.get(id)?;
        let password = doc.editor.as_ref().and_then(|e| e.keys.reopen.clone());
        let copy = Cos::open_with_password(bytes.clone(), password.as_deref()).ok()?;
        doc.pdfa_gate.check_copy(&copy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(clause: &'static str, pattern: &'static str, value: &str) -> Issue {
        let page = value.parse::<usize>().ok().map(|p| p.saturating_sub(1));
        Issue {
            clause,
            message: pattern.replace("{}", value),
            pattern,
            value: Some(value.to_string()),
            values: Vec::new(),
            page,
            object: None,
            fixable: false,
        }
    }

    fn clauses(v: &[Issue]) -> Vec<String> {
        v.iter().map(|i| i.message.clone()).collect()
    }

    #[test]
    fn only_what_the_earlier_issues_dont_account_for_is_new() {
        let font = |n| issue("6.2.10.4.1", "The font {} isn't embedded", n);
        let actions = |p| issue("6.5.2", "Page {} has additional actions", p);
        assert!(breaks(&[], &[]).is_empty());
        assert_eq!(clauses(&breaks(&[], &[font("Helvetica")])), ["The font Helvetica isn't embedded"]);
        assert!(breaks(&[font("Helvetica")], &[font("Helvetica")]).is_empty(), "the same issue");
        assert!(breaks(&[font("Helvetica"), actions("3")], &[]).is_empty(), "fixed issues are not breaks");
        // One earlier issue accounts for one issue now: a second of the same is new.
        assert_eq!(clauses(&breaks(&[font("Helvetica")], &[font("Helvetica"), font("Helvetica")])), ["The font Helvetica isn't embedded"]);
        // An issue that names another page after page 1 was deleted is the same issue.
        assert!(breaks(&[actions("3")], &[actions("2")]).is_empty(), "renumbered");
        // But not twice, and not for another wording or clause.
        assert_eq!(clauses(&breaks(&[actions("3")], &[actions("2"), actions("4")])).len(), 1);
        assert_eq!(clauses(&breaks(&[actions("3")], &[font("Times")])), ["The font Times isn't embedded"]);
        // An issue on no page is matched by its message alone: one font fixed doesn't hide another,
        // and an issue on a page that went away doesn't hide one of the same wording on no page.
        let mut elsewhere = actions("x");
        elsewhere.page = None;
        assert_eq!(clauses(&breaks(&[actions("3")], &[elsewhere])), ["Page x has additional actions"]);
        assert_eq!(clauses(&breaks(&[font("A")], &[font("B")])), ["The font B isn't embedded"]);
        // An exact match is used first, so a renumbered match never takes a new issue's place.
        assert_eq!(clauses(&breaks(&[actions("2"), actions("7")], &[actions("2"), actions("5"), actions("6")])), ["Page 6 has additional actions"]);
    }

    #[test]
    fn the_level_checked_is_the_declared_one_or_its_level_b() {
        let claim = |part, conformance: &str| PdfaClaim { part, conformance: conformance.to_string() };
        assert_eq!(claim(2, "U").checked_as(), Some(Level::A2b));
        assert_eq!(claim(3, "A").checked_as(), Some(Level::A3b));
        assert_eq!(claim(4, "").checked_as(), Some(Level::A4));
        assert_eq!(claim(4, "E").checked_as(), Some(Level::A4e));
        assert_eq!((claim(4, "F").checked_as(), claim(4, "F").label()), (Some(Level::A4f), "PDF/A-4f".to_string()));
        for (part, conformance) in [(1, "B"), (2, ""), (4, "B"), (5, "B"), (0, "")] {
            assert_eq!(claim(part, conformance).checked_as(), None, "{part}{conformance}");
        }
    }
}
