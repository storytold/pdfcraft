//! PdfCraft patch (104): what the recovery layer keeps for a whole document.
//!
//! The interpreter builds a page's fonts afresh for every page (each page has its own
//! [`crate::InterpreterCache`]), and a font's rule guards were decided again with them. A
//! [`RecoveryDocument`], shared by every page read from one version of a document
//! ([`crate::InterpreterSettings::recovery_document`]), keeps each font's decisions for the whole
//! document instead: a font is decided once, whichever page asks first and on whichever thread.
//!
//! It also holds the document's evidence for rules that read more than one font's own facts:
//! how often each glyph of a font is shown across the document, and the fonts that are subsets
//! of one font ([`FontUsage`], from the survey, `super::survey`). [`RecoveryDocument::prepare`]
//! surveys the document once, before a page's fonts are decided, so every decision sees the
//! whole document's evidence whichever page asks first.
//!
//! The guards are a function of the font and that evidence, so a decision kept here reads every
//! page the same as a decision made on that page would. A font is known by [`font_key`]: its
//! dictionary and the object it is, since a document may write two font objects with the same
//! dictionary (the same descendant and program) and show different glyphs through each.

use super::{Counter, Guards, count};
use crate::{CacheKey, InterpreterSettings};
use hayro_syntax::Pdf;
use hayro_syntax::object::Dict;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

/// Most fonts whose decisions one document keeps: a document with more decides each further
/// font on every page that shows it, as without a [`RecoveryDocument`], reading the same.
const MAX_DOCUMENT_FONTS: usize = 1 << 14;

/// The key a font has in a [`RecoveryDocument`] (and, while a page is read with one, in the
/// interpreter's font cache): its dictionary's [`CacheKey`] and its object. Two objects with the
/// same dictionary are the same font to draw, but each shows its own glyphs, and a producer that
/// merges subsets into one program may order each object's glyphs differently.
pub(crate) fn font_key(dict: &Dict<'_>) -> u128 {
    let object = dict.obj_id().map(|id| (id.obj_number, id.gen_number));
    crate::util::hash128(&(dict.cache_key(), object))
}

/// The recovery layer's state for one version of one document: the fonts' guard decisions,
/// shared by every page and thread that reads it. Create one per document version (an edit makes
/// a new version) and pass it through [`crate::InterpreterSettings::recovery_document`]; without
/// one, each page decides its fonts itself.
#[derive(Default)]
pub struct RecoveryDocument {
    decisions: Mutex<HashMap<u128, Arc<Guards>>>,
    /// The survey's evidence, once [`Self::prepare`] has read the document.
    evidence: OnceLock<Evidence>,
    /// Held while one thread surveys, so the others wait for its evidence instead of
    /// surveying too.
    surveying: Mutex<()>,
}

/// What the survey found in a document.
#[derive(Debug)]
pub(crate) enum Evidence {
    /// The fonts it counted, by their dictionaries' cache keys.
    Counted(HashMap<u128, Arc<FontUsage>>),
    /// The survey passed one of its caps: no font has evidence in this document.
    OverCap,
}

/// How often a font's glyphs are shown across the document, by the number a rule reads for
/// each (the glyph id of a TrueType or OpenType program, the CID of a CID-keyed CFF one).
#[derive(Debug)]
#[allow(
    dead_code,
    reason = "read by the rules whose evidence is the document's text, and by tests"
)]
pub(crate) struct FontUsage {
    /// (number, times shown), in number order.
    pub(crate) numbers: Vec<(u32, u64)>,
    /// How many glyphs the font shows, including codes no glyph number is read for (a CID a
    /// `/CIDToGIDMap` stream leaves out).
    pub(crate) shown: u64,
    /// The font's group: every font of the document with its `/BaseFont` (without the subset
    /// tag), glyph count and kind of program, when there are two or more.
    pub(crate) group: Option<Arc<UsageGroup>>,
}

/// Fonts of one document that are subsets of one font (`survey::PoolingKey`), their usage
/// added together.
#[allow(
    dead_code,
    reason = "read by the rules whose evidence is the document's text, and by tests"
)]
pub(crate) struct UsageGroup {
    /// How many fonts the group has.
    pub(crate) members: usize,
    /// The members' `/BaseFont` without the subset tag, as written.
    pub(crate) name: Vec<u8>,
    /// Each member's own (number, times shown) and glyphs shown, as its [`FontUsage`] has them,
    /// in the order the survey met them: for rules that weigh the group against its members.
    pub(crate) member_usage: Vec<(Vec<(u32, u64)>, u64)>,
    /// The members' (number, times shown) added together, in number order.
    pub(crate) numbers: Vec<(u32, u64)>,
    /// A rule's decision for the group as a whole, worked out once for all its members.
    pub(crate) decision: OnceLock<Arc<dyn std::any::Any + Send + Sync>>,
}

impl UsageGroup {
    pub(crate) fn new(
        name: Vec<u8>,
        member_usage: Vec<(Vec<(u32, u64)>, u64)>,
        numbers: Vec<(u32, u64)>,
    ) -> Self {
        Self {
            members: member_usage.len(),
            name,
            member_usage,
            numbers,
            decision: OnceLock::new(),
        }
    }
}

impl std::fmt::Debug for UsageGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsageGroup")
            .field("members", &self.members)
            .field("numbers", &self.numbers.len())
            .finish()
    }
}

impl std::fmt::Debug for RecoveryDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecoveryDocument")
            .field("fonts", &lock(&self.decisions).len())
            .finish()
    }
}

/// `m` locked; a lock poisoned by a panic elsewhere still holds whole decisions (each is
/// inserted complete), so it is taken over.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl RecoveryDocument {
    /// A document with nothing decided yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Survey `pdf` for the rules that read the whole document's evidence, unless that is
    /// done: once per document, before any of its fonts is decided (`extract_page_recovering`
    /// calls it first). Bounded by counts, not time (see `recovery::survey`); a render pool runs
    /// it outside a page's time limit. A read cancelled through `settings.cancelled` keeps
    /// nothing, and the next call surveys again.
    pub fn prepare(&self, pdf: &Pdf, settings: &InterpreterSettings) {
        if self.evidence.get().is_some() {
            return;
        }
        let _surveying = lock(&self.surveying);
        if self.evidence.get().is_some() {
            return;
        }
        if let Some(evidence) = super::survey::survey(pdf, settings) {
            let _ = self.evidence.set(evidence);
        }
    }

    /// Whether [`Self::prepare`] has surveyed the document.
    pub fn prepared(&self) -> bool {
        self.evidence.get().is_some()
    }

    /// The survey's evidence for the font keyed `key`: none before [`Self::prepare`], for a
    /// font it didn't count, or in a document past the survey's caps.
    pub(crate) fn usage(&self, key: u128) -> Option<Arc<FontUsage>> {
        match self.evidence.get()? {
            Evidence::Counted(fonts) => fonts.get(&key).cloned(),
            Evidence::OverCap => None,
        }
    }

    /// Test hook: what the survey counted, font by font in a stable order: each font's
    /// (number, times shown) and, for a font in a group, the group's size and pooled counts.
    #[cfg(feature = "test-counters")]
    #[allow(clippy::type_complexity, reason = "a test's view of the evidence")]
    pub fn surveyed_fonts(&self) -> Vec<(Vec<(u32, u64)>, Option<(usize, Vec<(u32, u64)>)>)> {
        let Some(Evidence::Counted(fonts)) = self.evidence.get() else {
            return Vec::new();
        };
        let mut out: Vec<_> = fonts
            .values()
            .map(|u| {
                let group = u.group.as_ref().map(|g| (g.members, g.numbers.clone()));
                (u.numbers.clone(), group)
            })
            .collect();
        out.sort();
        out
    }

    /// Test hook: the survey's groups, in a stable order: each one's name and its members'
    /// own (number, times shown) and glyphs shown.
    #[cfg(feature = "test-counters")]
    #[allow(clippy::type_complexity, reason = "a test's view of the evidence")]
    pub fn surveyed_groups(&self) -> Vec<(Vec<u8>, Vec<(Vec<(u32, u64)>, u64)>)> {
        let Some(Evidence::Counted(fonts)) = self.evidence.get() else {
            return Vec::new();
        };
        let mut out: Vec<_> = fonts
            .values()
            .filter_map(|u| u.group.as_ref())
            .map(|g| {
                let mut members = g.member_usage.clone();
                members.sort();
                (g.name.clone(), members)
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Whether the survey passed one of its caps: no font has evidence in this document.
    pub fn survey_over_cap(&self) -> bool {
        matches!(self.evidence.get(), Some(Evidence::OverCap))
    }

    /// The decisions for the font keyed `key`: the document's, or `decide`'s, kept for the
    /// document while it has room ([`MAX_DOCUMENT_FONTS`]) and once it is surveyed (a decision
    /// made before the evidence is ready is the page's alone). Deciding runs without the lock,
    /// so two threads may decide one font at once; the decisions are the same, and the first
    /// kept is the one every later page reads.
    pub(crate) fn guards(&self, key: u128, decide: impl FnOnce() -> Guards) -> Arc<Guards> {
        if let Some(kept) = lock(&self.decisions).get(&key) {
            count(Counter::DecisionReused);
            return kept.clone();
        }
        let decided = Arc::new(decide());
        let mut decisions = lock(&self.decisions);
        if let Some(kept) = decisions.get(&key) {
            return kept.clone();
        }
        if decisions.len() < MAX_DOCUMENT_FONTS && self.prepared() {
            decisions.insert(key, decided.clone());
        }
        decided
    }
}

/// The decisions are shared between threads.
const _: () = {
    const fn shared<T: Send + Sync>() {}
    shared::<RecoveryDocument>();
};
