//! PdfCraft patch (104): what the recovery layer keeps for a whole document.
//!
//! The interpreter builds a page's fonts afresh for every page (each page has its own
//! [`crate::InterpreterCache`]), and a font's rule guards were decided again with them. A
//! [`RecoveryDocument`], shared by every page read from one version of a document
//! ([`crate::InterpreterSettings::recovery_document`]), keeps each font's decisions for the whole
//! document instead: a font is decided once, whichever page asks first and on whichever thread.
//!
//! The guards are a function of the font alone (its dictionary and program), so a decision kept
//! here reads every page the same as a decision made on that page would; the key is the font
//! dictionary's [`crate::CacheKey`], as the interpreter's own font cache uses.

use super::{Counter, Guards, count};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

/// Most fonts whose decisions one document keeps: a document with more decides each further
/// font on every page that shows it, as without a [`RecoveryDocument`], reading the same.
const MAX_DOCUMENT_FONTS: usize = 1 << 14;

/// The recovery layer's state for one version of one document: the fonts' guard decisions,
/// shared by every page and thread that reads it. Create one per document version (an edit makes
/// a new version) and pass it through [`crate::InterpreterSettings::recovery_document`]; without
/// one, each page decides its fonts itself.
#[derive(Default)]
pub struct RecoveryDocument {
    decisions: Mutex<HashMap<u128, Arc<Guards>>>,
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
    m.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl RecoveryDocument {
    /// A document with nothing decided yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// The decisions for the font keyed `key`: the document's, or `decide`'s, kept for the
    /// document while it has room ([`MAX_DOCUMENT_FONTS`]). Deciding runs without the lock, so
    /// two threads may decide one font at once; the decisions are the same, and the first kept
    /// is the one every later page reads.
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
        if decisions.len() < MAX_DOCUMENT_FONTS {
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
