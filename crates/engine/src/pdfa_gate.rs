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

use pdfcraft_cos::{Document as Cos, ObjRef, ReadState};
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
    /// The page objects of the state checked, in page order: whether an issue's
    /// [`page`](Issue::page) number still names the page it named in an earlier check.
    pub pages: Arc<Vec<ObjRef>>,
}

impl PdfaCheck {
    /// The breaks `earlier` (an earlier check of the same document) doesn't account for, matched
    /// as the baseline is: what has broken since.
    pub fn breaks_since(&self, earlier: &PdfaCheck) -> Vec<Issue> {
        breaks((&earlier.breaks, &earlier.pages), (&self.breaks, &self.pages))
    }
}

/// Issues of one check, shared between the cache and its callers.
type Issues = Arc<Vec<Issue>>;

/// The page objects of a state, in page order ([`PdfaCheck::pages`]).
type Pages = Arc<Vec<ObjRef>>;

/// The page objects of `cos`, in the order [`pdfcraft_preflight::verify`] numbers pages.
fn page_objects(cos: &Cos) -> Pages {
    Arc::new(pdfcraft_model::pages(cos).into_iter().map(|p| p.obj).collect())
}

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
    baseline: Option<(Level, Issues, Pages)>,
    current: Option<(ReadState, Level, Issues, Pages)>,
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
            return Some(PdfaCheck { claim, checked_as: None, breaks: Vec::new(), pages: Pages::default() });
        };
        let cached = self.lock().current.as_ref().filter(|(s, l, _, _)| s.matches(cos) && *l == level).map(|(_, _, i, p)| (i.clone(), p.clone()));
        let (now, pages) = match cached {
            Some(found) => found,
            None => {
                let issues: Issues = Arc::new(pdfcraft_preflight::verify(cos, level));
                let pages = page_objects(cos);
                self.lock().current = Some((cos.read_state(), level, issues.clone(), pages.clone()));
                (issues, pages)
            }
        };
        // Not changed since the baseline (just opened, or every edit undone): nothing is new,
        // and the baseline's own check is this one.
        if self.baseline.as_ref().is_some_and(|b| b.read_state().matches(cos)) {
            self.lock().baseline = Some((level, now, pages.clone()));
            return Some(PdfaCheck { claim, checked_as: Some(level), breaks: Vec::new(), pages });
        }
        let (was, was_pages) = self.baseline_issues(level);
        let breaks = breaks((&was, &was_pages), (&now, &pages));
        Some(PdfaCheck { claim, checked_as: Some(level), breaks, pages })
    }

    /// The check of a copy of the document written on its own (an optimized or reduced copy, or
    /// the bytes a save wrote), measured from the same baseline. Not cached: each copy is new. A
    /// copy may number its objects anew, but it has the pages of the document it was written from
    /// (`from`) in the same order, so those stand for its pages.
    pub(crate) fn check_copy(&self, copy: &Cos, from: &Cos) -> Option<PdfaCheck> {
        let claim = PdfaClaim::of(copy)?;
        let Some(level) = claim.checked_as() else {
            return Some(PdfaCheck { claim, checked_as: None, breaks: Vec::new(), pages: Pages::default() });
        };
        let now = pdfcraft_preflight::verify(copy, level);
        let pages = page_objects(from);
        let (was, was_pages) = self.baseline_issues(level);
        let breaks = breaks((&was, &was_pages), (&now, &pages));
        Some(PdfaCheck { claim, checked_as: Some(level), breaks, pages })
    }

    /// What the baseline breaks at `level`, with its pages (nothing without a baseline: every
    /// issue is then new).
    fn baseline_issues(&self, level: Level) -> (Issues, Pages) {
        if let Some((l, issues, pages)) = &self.lock().baseline
            && *l == level
        {
            return (issues.clone(), pages.clone());
        }
        let issues: Issues = Arc::new(self.baseline.as_ref().map(|b| pdfcraft_preflight::verify(b, level)).unwrap_or_default());
        let pages = self.baseline.as_ref().map(page_objects).unwrap_or_default();
        self.lock().baseline = Some((level, issues.clone(), pages.clone()));
        (issues, pages)
    }

    /// The cache. A check that panicked while holding it left at worst a stale entry, which is
    /// keyed by the state it was read from, so the lock is taken back rather than given up.
    fn lock(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The issues of `now` that `before` doesn't account for, each list with the page objects of its
/// state in page order ([`PdfaCheck::pages`]).
///
/// Each issue of `before` accounts for one issue of `now` with the same clause and message.
/// When the pages are no longer the same objects in the same order (pages were added, deleted or
/// moved, or a full save numbered them anew), an issue on a page may name another page number
/// than before: then, among those left, an issue on a page also accounts for one with the same
/// clause and wording ([`Issue::pattern`]) on another page. While the pages stay as they were,
/// a page number names the same page, so an issue a change fixed on one page doesn't hide the
/// same issue a change made on another. An issue that names no page (a font, an object) is
/// matched by its message alone. What is left over is new.
pub(crate) fn breaks(before: (&[Issue], &[ObjRef]), now: (&[Issue], &[ObjRef])) -> Vec<Issue> {
    let ((before, before_pages), (now, now_pages)) = (before, now);
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
    if before_pages == now_pages {
        return unmatched.into_iter().cloned().collect();
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

    /// The same for the file document `id` was last saved to (or opened from), whatever undo or
    /// edits did after the save: its pages are those of the state the save wrote, read from the
    /// file itself, so an issue on a page is matched to the page it was on when the file was
    /// written. `None` when the file declares no PDF/A part or can't be read back.
    pub fn pdfa_check_saved(&self, id: DocId) -> Option<PdfaCheck> {
        let doc = self.get(id)?;
        let editor = doc.editor.as_ref()?;
        let file = Cos::open_with_password(editor.saved.clone(), editor.saved_keys.reopen.as_deref()).ok()?;
        doc.pdfa_gate.check_copy(&file, &file)
    }

    /// The same for `bytes`, a copy of document `id` written on its own (optimized, reduced):
    /// `None` when the copy declares no PDF/A part or can't be read back.
    pub fn pdfa_check_copy(&self, id: DocId, bytes: &Arc<Vec<u8>>) -> Option<PdfaCheck> {
        let doc = self.get(id)?;
        let password = doc.editor.as_ref().and_then(|e| e.keys.reopen.clone());
        let copy = Cos::open_with_password(bytes.clone(), password.as_deref()).ok()?;
        doc.pdfa_gate.check_copy(&copy, &doc.editor.as_ref()?.cos)
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
        // Eight pages, as they were, and after the first was deleted.
        let pages: Vec<ObjRef> = (10..18).map(|n| ObjRef::new(n, 0)).collect();
        let after_delete = pages.get(1..).unwrap_or_default().to_vec();
        let same = |before: &[Issue], now: &[Issue]| breaks((before, &pages), (now, &pages));
        let moved = |before: &[Issue], now: &[Issue]| breaks((before, &pages), (now, &after_delete));
        assert!(same(&[], &[]).is_empty());
        assert_eq!(clauses(&same(&[], &[font("Helvetica")])), ["The font Helvetica isn't embedded"]);
        assert!(same(&[font("Helvetica")], &[font("Helvetica")]).is_empty(), "the same issue");
        assert!(same(&[font("Helvetica"), actions("3")], &[]).is_empty(), "fixed issues are not breaks");
        // One earlier issue accounts for one issue now: a second of the same is new.
        assert_eq!(clauses(&same(&[font("Helvetica")], &[font("Helvetica"), font("Helvetica")])), ["The font Helvetica isn't embedded"]);
        // An issue that names another page after page 1 was deleted is the same issue.
        assert!(moved(&[actions("3")], &[actions("2")]).is_empty(), "renumbered");
        // But not twice, and not for another wording or clause.
        assert_eq!(clauses(&moved(&[actions("3")], &[actions("2"), actions("4")])).len(), 1);
        assert_eq!(clauses(&moved(&[actions("3")], &[font("Times")])), ["The font Times isn't embedded"]);
        // While no page has moved, a page number names the same page: an issue fixed on page 3
        // doesn't hide the same issue made on page 2.
        assert_eq!(clauses(&same(&[actions("3")], &[actions("2")])), ["Page 2 has additional actions"]);
        // An issue on no page is matched by its message alone: one font fixed doesn't hide another,
        // and an issue on a page that went away doesn't hide one of the same wording on no page.
        let mut elsewhere = actions("x");
        elsewhere.page = None;
        assert_eq!(clauses(&moved(&[actions("3")], &[elsewhere])), ["Page x has additional actions"]);
        assert_eq!(clauses(&same(&[font("A")], &[font("B")])), ["The font B isn't embedded"]);
        // An exact match is used first, so a renumbered match never takes a new issue's place.
        assert_eq!(clauses(&moved(&[actions("2"), actions("7")], &[actions("2"), actions("5"), actions("6")])), ["Page 6 has additional actions"]);
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
