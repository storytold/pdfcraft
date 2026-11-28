//! Scan & OCR ▸ Correct recognized text: the OCR Verify screen. It takes the document area,
//! under the window's tabs, menus and toolbar, with four panes (page list, source page,
//! recognized text, zoom + word inspector) where a recognition is reviewed before it is
//! applied: words are colored by confidence, corrected, removed (and restored), sent to a
//! per-language user dictionary, and navigated by their uncertainty. Accept applies the
//! review — as one undoable step on the open document (searchable), or as a new
//! `<name>_ocr.pdf` of visible text (editable). Reject throws the reading away.
//!
//! The state machine is [`Phase`]: recognition runs on a worker thread ([`Phase::Running`], a
//! real Cancel keeps the pages already read), results land in [`Phase::ReviewReady`], failures
//! land in [`Phase::RecoverableError`] with the cause named and Run enabled again, and
//! Accept/Reject/Re-OCR are ignored outside ReviewReady instead of acting on nothing (Reject
//! also throws away a reading whose Accept failed).

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use craft::host::queue::Priority;
use pdfcraft_engine::ocr::{OcrGuard, OcrPage, OcrSettings, OutputMode, PlacedWord, clamp_region};
use pdfcraft_engine::{DocId, Session};

// PLAN: assumed `super::job_count::JobCount` (pdf-print-ocr-w6): `Default`, `set(done, total)`, `read() -> (usize, usize)`, `cancel()`, `cancelled() -> bool`.
use super::job_count::JobCount;
use crate::PdfCraftApp;

mod panes;
mod screen;
mod source;

use screen::Intents;

pub(crate) use screen::show;

/// Where the screen stands. Run is enabled in Idle, ReviewReady and RecoverableError; the
/// review tools (Accept, Reject, navigation, the inspector, re-OCR) only in ReviewReady, and
/// Reject also in RecoverableError while a reading is kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Running,
    ReviewReady,
    Saving,
    RecoverableError,
}

/// One review change, so it can be undone within the panel (review edits are not document
/// edits; they live until the review is applied or rejected).
#[derive(Clone, Debug)]
pub enum Op {
    /// The correction text field committed (an empty commit removes instead).
    Corrected {
        key: (usize, usize),
        before: String,
        after: String,
    },
    Removed {
        key: (usize, usize),
    },
    Restored {
        key: (usize, usize),
    },
    WordVerified {
        key: (usize, usize),
        on: bool,
    },
    Dictionary {
        word: String,
    },
    /// A re-read (a region or the whole page) replaced words on the page. Review marks are
    /// keyed by word position, so the page's marks as they were are kept for the undo.
    Region {
        page: usize,
        before: Vec<PlacedWord>,
        keys: PageKeys,
    },
    PageVerified {
        page: usize,
        on: bool,
    },
    /// "Reject entire page": the page's words were cleared (and its review marks with them).
    PageCleared {
        page: usize,
        before: Vec<PlacedWord>,
        keys: PageKeys,
    },
}

/// One page's positional review marks (word indices), saved when its word list is replaced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageKeys {
    pub removed: Vec<usize>,
    pub verified: Vec<usize>,
}

/// The review state: which words are struck out, which are vouched for, which are in the
/// per-language dictionary, and the undo history. Never persisted.
#[derive(Clone, Debug, Default)]
pub struct Review {
    pub removed: BTreeSet<(usize, usize)>,
    pub verified: BTreeSet<(usize, usize)>,
    pub verified_pages: BTreeSet<usize>,
    pub dictionary: BTreeSet<String>,
    /// Pages whose words were already applied to the document ("Accept entire page"); a later
    /// Accept of the whole review never writes them twice.
    pub applied_pages: BTreeSet<usize>,
    pub history: Vec<Op>,
}

/// A word that asks for review: its confidence is in the Low band and the reviewer has not
/// dealt with it — it is not removed, not corrected into the dictionary, not marked verified,
/// and not on a page marked verified. A corrected word stays uncertain by its ORIGINAL
/// confidence: the reviewer changed the text, not the engine's confidence in it.
pub fn is_uncertain(page: &OcrPage, key: (usize, usize), review: &Review) -> bool {
    let (_, wi) = key;
    if review.removed.contains(&key) || review.verified.contains(&key) || review.verified_pages.contains(&key.0) {
        return false;
    }
    let Some(w) = page.words.get(wi) else { return false };
    if w.text.trim().is_empty() || review.dictionary.contains(&w.text.to_lowercase()) {
        return false;
    }
    pdfcraft_engine::ocr::is_suspect(w.confidence)
}

/// Candidate corrections for `text`: dictionary words and the page's own words within edit
/// distance 2, nearest first, at most 5, never the word itself.
pub fn suggestions(text: &str, dictionary: &BTreeSet<String>, page_words: &[PlacedWord]) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut out: Vec<(usize, String)> = Vec::new();
    let consider = |candidate: &str, out: &mut Vec<(usize, String)>| {
        let c = candidate.to_lowercase();
        if c.is_empty() || c == lower {
            return;
        }
        if let Some(d) = edit_distance_within_2(&lower, &c, 2) {
            out.push((d, candidate.to_owned()));
        }
    };
    for w in dictionary {
        consider(w, &mut out);
    }
    for w in page_words {
        consider(&w.text, &mut out);
    }
    // Nearest first; ties break alphabetically; at most five.
    out.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    out.truncate(5);
    out.into_iter().map(|(_, s)| s).collect()
}

/// Levenshtein distance between `a` and `b`, abandoned as soon as it is sure to exceed `max`
/// (small bounded DP over char boundaries; `None` means "farther than `max`").
pub fn edit_distance_within_2(a: &str, b: &str, max: usize) -> Option<usize> {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > max {
        return None;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        let mut row_min = cur[0];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            row_min = row_min.min(cur[j]);
        }
        if row_min > max {
            return None;
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    (prev[b.len()] <= max).then_some(prev[b.len()])
}

/// What the screen asks its job queue to run. The number is the ticket the request was made
/// under, so the queue's key says which request a result answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VerifyJob {
    /// A recognition run.
    Run(u64),
    /// A page or region re-read.
    Reread(u64),
}

/// What a finished [`VerifyJob`] hands back.
pub enum VerifyDone {
    /// The pages a run read, or why it could not read.
    Run(Result<Vec<OcrPage>, String>),
    /// The words a re-read found, or why it found none.
    Reread(Result<Vec<PlacedWord>, String>),
}

/// A page or region re-read in flight ([`OcrVerify::start_reread`]): the page being read
/// again (and the drawn region, in view-space points, for a region re-read), and the ticket
/// its result must come back under.
pub struct Reread {
    pub page: usize,
    pub region: Option<[f32; 4]>,
    pub(crate) ticket: u64,
}

/// What the Verify screen was asked to read while the web build was still downloading the
/// models ([`OcrVerify::models_landed`] starts it once they are in).
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) enum AwaitingModels {
    Run,
    Reread { page: usize, region: Option<[f32; 4]> },
}

/// The OCR Verify screen. One instance per app; lives across frames.
pub struct OcrVerify {
    pub open: bool,
    pub phase: Phase,
    /// The document being reviewed; before a run, the document in front (the source pane shows
    /// its page).
    pub doc: Option<DocId>,
    /// The recognition results, in page order, as read (review edits mutate them).
    pub pages: Vec<OcrPage>,
    /// What applying the results still expects of the document.
    pub guard: Option<OcrGuard>,
    pub review: Review,
    /// The selected word (page, index) and the page the source pane shows.
    pub selected: Option<(usize, usize)>,
    pub current: usize,
    /// A region drawn on the source pane, in view-space points on the shown page, snapped
    /// outward to whole pixels when it is used ([`Self::start_reread`]).
    pub region: Option<[f32; 4]>,
    /// The status line: where the run stands, why something was refused.
    pub message: String,
    /// The run in flight: its ticket and the progress and cancel flag it shares with its job.
    pub(crate) run: Option<(u64, Arc<JobCount>)>,
    /// The last ticket handed out. It survives [`Self::finish`] and [`Self::reject`], so a late
    /// result of an earlier request never matches a later one.
    pub(crate) ticket: u64,
    /// A page or region re-read in the job queue: the screen stays live (the review edits and
    /// navigation work) and the reading is applied by [`Self::land_reread`].
    pub reread: Option<Reread>,
    /// A run or re-read waiting for the web build's model download.
    pub(crate) awaiting_models: Option<AwaitingModels>,
    /// The correction text field's current contents.
    pub correction: String,
    /// Where the per-language dictionary files live (`None`: off, e.g. the web build).
    pub dictionary_dir: Option<PathBuf>,
    /// Language of the captured recognition, independent of the interface locale.
    pub dictionary_language: String,
    /// The mode the screen was last drawn in (`None` while it is closed): switching to another
    /// puts the screen away.
    pub(crate) shown_in: Option<crate::state::Mode>,
}

fn dictionary_path(dir: &std::path::Path, language: &str) -> Result<PathBuf, String> {
    if language.is_empty() || language.len() > 32 || !language.bytes().all(|b| b.is_ascii_alphabetic() || b == b'-') {
        return Err("invalid OCR dictionary language".into());
    }
    Ok(dir.join(format!("ocr-dictionary-{language}.txt")))
}

fn read_dictionary(path: &std::path::Path) -> Result<String, String> {
    use std::io::Read as _;
    let mut text = String::new();
    match std::fs::File::open(path) {
        Ok(file) => {
            file.take((pdfcraft_engine::ocr::dictionary::MAX_BYTES + 1) as u64).read_to_string(&mut text).map_err(|e| e.to_string())?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    Ok(text)
}

impl Default for OcrVerify {
    fn default() -> Self {
        OcrVerify {
            open: false,
            phase: Phase::Idle,
            doc: None,
            pages: Vec::new(),
            guard: None,
            review: Review::default(),
            selected: None,
            current: 0,
            region: None,
            message: String::new(),
            run: None,
            ticket: 0,
            reread: None,
            awaiting_models: None,
            correction: String::new(),
            dictionary_dir: None,
            dictionary_language: "en".into(),
            shown_in: None,
        }
    }
}

impl OcrVerify {
    /// Begin a review of `found` for `doc`: ReviewReady even when nothing was found (the
    /// status then says so), with the guard remembered for Accept.
    pub fn load(&mut self, doc: DocId, guard: OcrGuard, found: Vec<OcrPage>) {
        self.doc = Some(doc);
        self.guard = Some(guard);
        self.pages = found;
        self.review = Review::default();
        self.selected = None;
        self.region = None;
        self.show_a_read_page();
        self.phase = Phase::ReviewReady;
        self.load_dictionary_pref();
    }

    /// Show a page the reading has words for: the current page if it was read, else the first
    /// page read. When every page was skipped, the current page if the reading has it, else its
    /// first page; a reading with no pages (a run cancelled before its first page) keeps the
    /// page the run was started on.
    fn show_a_read_page(&mut self) {
        if self.page(self.current).is_some_and(|p| p.skipped.is_none()) {
            return;
        }
        if let Some(p) = self.pages.iter().find(|p| p.skipped.is_none()) {
            self.current = p.page;
        } else if self.page(self.current).is_none()
            && let Some(p) = self.pages.first()
        {
            self.current = p.page;
        }
    }

    /// Whether the screen holds a run or a reading not yet accepted or rejected (a failed run
    /// that read nothing holds none).
    fn has_reading(&self) -> bool {
        !(self.phase == Phase::Idle || (self.phase == Phase::RecoverableError && self.pages.is_empty()))
    }

    /// Whether the screen holds a run or a reading of `doc`: closing that document would leave
    /// the review with nothing to apply to.
    pub(crate) fn holds_reading_of(&self, doc: DocId) -> bool {
        self.doc == Some(doc) && self.has_reading()
    }

    /// The info strip's PAGE cell: the shown page's number in the document over the document's
    /// page count (`page_count`), the numbering the page list uses; "—" before a reading.
    fn page_indicator(&self, page_count: Option<usize>) -> String {
        if self.pages.is_empty() {
            return "—".into();
        }
        let shown = self.current.saturating_add(1);
        match page_count {
            Some(total) if total > 0 => format!("{shown} / {total}"),
            _ => shown.to_string(),
        }
    }

    /// The count of live (not removed, not empty) words across the review.
    pub fn word_count(&self) -> usize {
        self.pages
            .iter()
            .enumerate()
            .map(|(p, pg)| pg.words.iter().enumerate().filter(|(i, w)| !w.text.trim().is_empty() && !self.review.removed.contains(&(p, *i))).count())
            .sum()
    }

    /// The words that ask for review, in reading order across pages.
    pub fn uncertain(&self) -> Vec<(usize, usize)> {
        self.pages
            .iter()
            .flat_map(|pg| pg.suspects().into_iter().map(move |i| (pg.page, i)))
            .filter(|k| self.page(k.0).is_some_and(|p| is_uncertain(p, *k, &self.review)))
            .collect()
    }

    /// The count of Low-band live words (the info strip's LOW-CONFIDENCE WORDS).
    pub fn suspect_count(&self) -> usize {
        self.pages
            .iter()
            .enumerate()
            .map(|(p, pg)| {
                pg.words
                    .iter()
                    .enumerate()
                    .filter(|(i, w)| {
                        !self.review.removed.contains(&(p, *i))
                            && !w.text.trim().is_empty()
                            && !self.review.dictionary.contains(&w.text.to_lowercase())
                            && pdfcraft_engine::ocr::is_suspect(w.confidence)
                    })
                    .count()
            })
            .sum()
    }

    /// The share of words the reviewer vouched for: marked verified, or on a page marked
    /// verified (0.0 with no words).
    pub fn verified_fraction(&self) -> f32 {
        let (mut yes, mut all) = (0usize, 0usize);
        for (p, pg) in self.pages.iter().enumerate() {
            let page_done = self.review.verified_pages.contains(&pg.page);
            for (i, w) in pg.words.iter().enumerate() {
                if w.text.trim().is_empty() || self.review.removed.contains(&(p, i)) {
                    continue;
                }
                all += 1;
                if page_done || self.review.verified.contains(&(p, i)) {
                    yes += 1;
                }
            }
        }
        if all == 0 { 0.0 } else { yes as f32 / all as f32 }
    }

    pub fn page(&self, page: usize) -> Option<&OcrPage> {
        self.pages.iter().find(|p| p.page == page)
    }

    pub fn word(&self, key: (usize, usize)) -> Option<&PlacedWord> {
        self.page(key.0).and_then(|p| p.words.get(key.1))
    }

    pub fn is_removed(&self, key: (usize, usize)) -> bool {
        self.review.removed.contains(&key)
    }

    pub fn is_verified(&self, key: (usize, usize)) -> bool {
        self.review.verified.contains(&key) || self.review.verified_pages.contains(&key.0)
    }

    /// The word's text with any correction applied.
    pub fn text_of(&self, key: (usize, usize)) -> Option<&str> {
        self.word(key).map(|w| w.text.as_str())
    }

    fn in_review(&self) -> bool {
        self.phase == Phase::ReviewReady
    }

    /// Whether Reject has a reading to throw away: in review, and after a failed Accept, which
    /// keeps the reading (a stale document, say). Its document stays open until the reading is
    /// accepted or rejected, so Reject must not wait for a new run that would replace it.
    pub fn can_reject(&self) -> bool {
        self.in_review() || (self.phase == Phase::RecoverableError && !self.pages.is_empty())
    }

    /// Commit the correction field for `key`. An empty (after trim) commit removes the word;
    /// the box never moves. Only in ReviewReady.
    pub fn correct(&mut self, key: (usize, usize), text: &str) -> bool {
        if !self.in_review() || self.word(key).is_none() {
            return false;
        }
        if text.trim().is_empty() {
            return self.set_removed(key, true);
        }
        let Some(w) = self.page_mut(key.0).and_then(|p| p.words.get_mut(key.1)) else { return false };
        let before = w.text.clone();
        if before == text {
            return false;
        }
        w.text = text.to_string();
        self.review.history.push(Op::Corrected { key, before, after: text.to_string() });
        true
    }

    /// Strike a word out (or restore it). Removed words stay in place — reviewable, restorable,
    /// and never written by Accept.
    pub fn set_removed(&mut self, key: (usize, usize), removed: bool) -> bool {
        if !self.in_review() || self.word(key).is_none() || self.is_removed(key) == removed {
            return false;
        }
        if removed {
            self.review.removed.insert(key);
            self.review.history.push(Op::Removed { key });
        } else {
            self.review.removed.remove(&key);
            self.review.history.push(Op::Restored { key });
        }
        true
    }

    /// Vouch for one word.
    pub fn set_word_verified(&mut self, key: (usize, usize), on: bool) -> bool {
        if !self.in_review() || self.word(key).is_none() || self.review.verified.contains(&key) == on {
            return false;
        }
        if on {
            self.review.verified.insert(key);
        } else {
            self.review.verified.remove(&key);
        }
        self.review.history.push(Op::WordVerified { key, on });
        true
    }

    /// Mark the page reviewed (the toolbar's Verified toggle, Ctrl+T).
    pub fn set_page_verified(&mut self, page: usize, on: bool) -> bool {
        if !self.in_review() || self.page(page).is_none() || self.review.verified_pages.contains(&page) == on {
            return false;
        }
        if on {
            self.review.verified_pages.insert(page);
        } else {
            self.review.verified_pages.remove(&page);
        }
        self.review.history.push(Op::PageVerified { page, on });
        true
    }

    /// Add a word to the per-language dictionary (case-insensitive, duplicate-free) and
    /// remember it in the dictionary file when one can be written. Privacy: that file
    /// (`ocr-dictionary-<language>.txt` in the preferences folder) is plain text outside any
    /// document, so a word added from a document stays on disk after the document is closed,
    /// until it is removed (the undo removes it again).
    pub fn add_to_dictionary(&mut self, word: &str, language: &str) -> bool {
        if !self.in_review() {
            return false;
        }
        if language != self.dictionary_language {
            self.message = "The dictionary language differs from the captured recognition".into();
            return false;
        }
        let lower = word.trim().to_lowercase();
        if self.review.dictionary.contains(&lower) {
            return false;
        }
        match self.updated_dictionary(&[word], &[]) {
            Ok((entries, changed)) => {
                self.review.dictionary = entries;
                if !changed {
                    return false;
                }
                self.review.history.push(Op::Dictionary { word: lower });
                true
            }
            Err(why) => {
                self.message = format!("OCR dictionary: {why}");
                false
            }
        }
    }

    /// Validate the complete file and persist atomically before changing review state.
    fn updated_dictionary(&self, add: &[&str], remove: &[&str]) -> Result<(BTreeSet<String>, bool), String> {
        use pdfcraft_engine::ocr::dictionary;
        let path = self.dictionary_dir.as_ref().map(|dir| dictionary_path(dir, &self.dictionary_language)).transpose()?;
        let current =
            if let Some(path) = &path { read_dictionary(path)? } else { self.review.dictionary.iter().cloned().collect::<Vec<_>>().join("\n") };
        let before = dictionary::words(&current)?;
        let text = dictionary::update(&current, add, remove)?;
        let entries = dictionary::words(&text)?;
        let changed = before != entries;
        if let Some(path) = path.filter(|_| changed) {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            // PLAN: assumed `craft::atomic::atomic_write(&Path, &[u8]) -> Result<(), impl Display>` (pdf-shell-w14 note).
            craft::atomic::atomic_write(&path, text.as_bytes()).map_err(|e| e.to_string())?;
        }
        Ok((entries, changed))
    }

    /// Read the captured recognition language's dictionary; errors preserve its live state.
    pub fn load_dictionary_pref(&mut self) {
        let Some(dir) = &self.dictionary_dir else { return };
        let path = match dictionary_path(dir, &self.dictionary_language) {
            Ok(path) => path,
            Err(why) => {
                self.message = why;
                return;
            }
        };
        match read_dictionary(&path).and_then(|text| pdfcraft_engine::ocr::dictionary::words(&text)) {
            Ok(words) => self.review.dictionary = words,
            Err(why) => self.message = format!("OCR dictionary: {why}"),
        }
    }

    /// Undo the last review change (Ctrl+Z / the inspector's Undo).
    pub fn undo(&mut self) -> bool {
        if !self.in_review() {
            return false;
        }
        let Some(op) = self.review.history.pop() else { return false };
        match op {
            Op::Corrected { key, before, .. } => {
                if let Some(w) = self.page_mut(key.0).and_then(|p| p.words.get_mut(key.1)) {
                    w.text = before;
                }
            }
            Op::Removed { key } => {
                self.review.removed.remove(&key);
            }
            Op::Restored { key } => {
                self.review.removed.insert(key);
            }
            Op::WordVerified { key, on } => {
                if on {
                    self.review.verified.remove(&key);
                } else {
                    self.review.verified.insert(key);
                }
            }
            Op::Dictionary { word } => match self.updated_dictionary(&[], &[&word]) {
                Ok((entries, _)) => self.review.dictionary = entries,
                Err(why) => {
                    self.message = format!("OCR dictionary: {why}");
                    self.review.history.push(Op::Dictionary { word });
                    return false;
                }
            },
            Op::Region { page, before, keys } => {
                if let Some(p) = self.page_mut(page) {
                    p.words = before;
                }
                self.restore_page_keys(page, keys);
            }
            Op::PageVerified { page, on } => {
                if on {
                    self.review.verified_pages.remove(&page);
                } else {
                    self.review.verified_pages.insert(page);
                }
            }
            Op::PageCleared { page, before, keys } => {
                if let Some(p) = self.page_mut(page) {
                    p.words = before;
                }
                self.restore_page_keys(page, keys);
            }
        }
        true
    }

    fn reread_settings(&self, draft: &super::ocr_ui::OcrDraft) -> OcrSettings {
        let mut settings = draft.settings();
        settings.language = self.dictionary_language.clone();
        settings
    }

    /// Whether this word's text was corrected in the review (the inspector says so, and the
    /// source pane outlines it).
    pub fn is_corrected(&self, key: (usize, usize)) -> bool {
        // Newest first: a correction recorded before the page's words were replaced (a re-read,
        // a cleared page) names a position in the OLD word list, not this word.
        for op in self.review.history.iter().rev() {
            match op {
                Op::Corrected { key: k, .. } if *k == key => return true,
                Op::Region { page, .. } | Op::PageCleared { page, .. } if *page == key.0 => return false,
                _ => {}
            }
        }
        false
    }

    /// Take `page`'s positional review marks out of the review (its word list is about to be
    /// replaced), returning them for the undo.
    fn take_page_keys(&mut self, page: usize) -> PageKeys {
        let take = |set: &mut BTreeSet<(usize, usize)>| {
            let on_page: Vec<usize> = set.iter().filter(|k| k.0 == page).map(|k| k.1).collect();
            set.retain(|k| k.0 != page);
            on_page
        };
        PageKeys { removed: take(&mut self.review.removed), verified: take(&mut self.review.verified) }
    }

    /// Put back `page`'s marks exactly as `keys` holds them (the undo of a replacement).
    fn restore_page_keys(&mut self, page: usize, keys: PageKeys) {
        self.review.removed.retain(|k| k.0 != page);
        self.review.verified.retain(|k| k.0 != page);
        self.review.removed.extend(keys.removed.into_iter().map(|i| (page, i)));
        self.review.verified.extend(keys.verified.into_iter().map(|i| (page, i)));
    }

    /// Accept ONE page ("Accept entire page"): the page's live words become one undoable step
    /// on the open document, the page is marked applied, and a later whole-review Accept (or a
    /// second click on the same page) never writes it again. Ignored outside ReviewReady; zero
    /// live words on the page is a typed failure; a stale session is refused with its reason
    /// (edits kept).
    pub fn accept_page(&mut self, session: &mut Session, page: usize) -> Result<usize, String> {
        if !self.in_review() {
            return Ok(0);
        }
        let Some(guard) = self.guard.clone() else { return Ok(0) };
        let Some(doc) = self.doc else { return Ok(0) };
        let Some(pg) = self.live_pages().into_iter().find(|p| p.page == page) else { return Ok(0) };
        if pg.words.is_empty() {
            return Err("no words are left to apply on this page".into());
        }
        // The page's words are already in the document: never append the OCR layer twice.
        // (A page accepted and then rejected still counts: its words were written, and the
        // empty-words check above has already spoken for it.)
        if self.review.applied_pages.contains(&page) {
            return Ok(0);
        }
        match session.apply_ocr_checked(doc, &[pg], &guard) {
            Ok(n) => {
                self.review.applied_pages.insert(page);
                // This apply is now the document's newest edit: the next page (or the final
                // Accept) checks against the document as it stands after it, so only an edit
                // made elsewhere in between makes it stale.
                self.guard = session.ocr_guard(doc);
                Ok(n)
            }
            Err(why) => {
                self.phase = Phase::RecoverableError;
                self.message = why.clone();
                Err(why)
            }
        }
    }

    /// Reject ONE page ("Reject entire page"): clear its words (undoable within the panel),
    /// drop the selection if it was there.
    pub fn reject_page(&mut self, page: usize) -> bool {
        if !self.in_review() {
            return false;
        }
        let Some(pg) = self.page_mut(page) else { return false };
        if pg.words.is_empty() {
            return false;
        }
        let before = std::mem::take(&mut pg.words);
        let keys = self.take_page_keys(page);
        self.review.history.push(Op::PageCleared { page, before, keys });
        if self.selected.is_some_and(|k| k.0 == page) {
            self.selected = None;
        }
        true
    }

    /// Replace a page's words wholesale ("Re-OCR entire page"), undoable within the panel. The
    /// page's review marks belonged to the old words (they are kept by position), so they are
    /// cleared with them and come back with the undo.
    pub fn replace_page_words(&mut self, page: usize, words: Vec<PlacedWord>) -> bool {
        if !self.in_review() || self.page(page).is_none() {
            return false;
        }
        let Some(pg) = self.page_mut(page) else { return false };
        let before = std::mem::replace(&mut pg.words, words);
        let keys = self.take_page_keys(page);
        self.review.history.push(Op::Region { page, before, keys });
        if self.selected.is_some_and(|k| k.0 == page) {
            self.selected = None;
        }
        true
    }

    fn page_mut(&mut self, page: usize) -> Option<&mut OcrPage> {
        self.pages.iter_mut().find(|p| p.page == page)
    }

    /// Walk the uncertain words. Wraps around, skips removed words; with none to show it is a
    /// no-op. A corrected word that is still Low-band stays in the walk.
    pub fn goto_uncertain(&mut self, forward: bool) -> bool {
        let list = self.uncertain();
        let Some(target) = (match self.selected {
            None if forward => list.first().copied(),
            None => list.last().copied(),
            Some(sel) => {
                let pos = list.iter().position(|k| *k == sel);
                match pos {
                    Some(i) => {
                        let next = if forward { i + 1 } else { i + list.len() - 1 };
                        list.get(next % list.len()).copied()
                    }
                    None if forward => list.first().copied(),
                    None => list.last().copied(),
                }
            }
        }) else {
            return false;
        };
        self.select(target);
        true
    }

    /// Select a word and show its page.
    pub fn select(&mut self, key: (usize, usize)) {
        self.selected = Some(key);
        self.current = key.0;
        self.correction = self.text_of(key).unwrap_or_default().to_string();
    }

    /// The results Accept would write: the read pages with removed words stripped.
    pub fn live_pages(&self) -> Vec<OcrPage> {
        self.pages
            .iter()
            .map(|p| OcrPage {
                already_on_page: 0,
                page: p.page,
                skipped: p.skipped.clone(),
                notes: p.notes.clone(),
                words: p
                    .words
                    .iter()
                    .enumerate()
                    .filter(|(i, w)| !self.review.removed.contains(&(p.page, *i)) && !w.text.trim().is_empty())
                    .map(|(_, w)| w.clone())
                    .collect(),
            })
            .collect()
    }

    /// Accept in searchable mode: apply the reviewed words as ONE undoable step on the open
    /// document, but only while the session is still the one that was read. Ignored outside
    /// ReviewReady; zero words and a stale session are typed failures (edits kept).
    /// Returns the number of words written.
    pub fn accept(&mut self, session: &mut Session) -> Result<usize, String> {
        if !self.in_review() {
            return Ok(0); // ignored, not an error
        }
        let Some(guard) = self.guard.clone() else { return Ok(0) };
        let Some(doc) = self.doc else { return Ok(0) };
        // Pages accepted page-by-page are already in the document; never write them twice.
        let applied = &self.review.applied_pages;
        let pages: Vec<OcrPage> = self.live_pages().into_iter().filter(|p| !applied.contains(&p.page)).collect();
        let words = pages.iter().map(|p| p.words.len()).sum::<usize>();
        if words == 0 {
            if !applied.is_empty() {
                self.finish();
                return Ok(0);
            }
            return Err("no words are left to apply: every word was removed or empty".into());
        }
        match session.apply_ocr_checked(doc, &pages, &guard) {
            Ok(n) => {
                self.finish();
                Ok(n)
            }
            Err(why) => {
                self.phase = Phase::RecoverableError;
                self.message = why.clone();
                Err(why)
            }
        }
    }

    /// Accept in editable-text mode: the NEW document's bytes and its default file name
    /// (`<name>_ocr.pdf`). The open document is not touched; refuses zero words.
    pub fn editable_output(&self, session: &Session) -> Result<(Arc<Vec<u8>>, String), String> {
        if !self.in_review() {
            return Err("the review is not ready".into());
        }
        let Some(doc) = self.doc else { return Err("the review is not ready".into()) };
        // The review can stay open while the document is edited (Exit keeps it): words read before a
        // redaction must not be written out as visible text afterwards.
        if let Some(why) = self.guard.as_ref().and_then(|g| session.stale_reason(g)) {
            return Err(why);
        }
        let pages = self.live_pages();
        if pages.iter().map(|p| p.words.len()).sum::<usize>() == 0 {
            return Err("no words are left to write: every word was removed or empty".into());
        }
        let bytes = session.editable_ocr_bytes(doc, &pages)?;
        let d = session.get(doc).ok_or("the document is no longer open")?;
        let base = d.name.strip_suffix(".pdf").unwrap_or(&d.name).to_string();
        Ok((bytes, format!("{base}_ocr.pdf")))
    }

    /// Reject: clear the words, the session, the selection and the flags; back to Idle.
    /// Ignored outside ReviewReady, except after a failed Accept ([`Self::can_reject`]).
    pub fn reject(&mut self) -> bool {
        if !self.can_reject() {
            return false;
        }
        *self = OcrVerify { open: self.open, ticket: self.ticket, dictionary_dir: self.dictionary_dir.take(), ..Default::default() };
        true
    }

    /// The state after a successful accept (or a saved editable copy): the review is done.
    pub(crate) fn finish(&mut self) {
        *self = OcrVerify {
            open: self.open,
            phase: Phase::Idle,
            doc: self.doc,
            ticket: self.ticket,
            dictionary_dir: self.dictionary_dir.take(),
            ..Default::default()
        };
    }

    /// Ask the running recognition to stop. The pages already read are kept; the rest of the
    /// document is not read.
    pub fn cancel(&mut self) {
        if self.phase == Phase::Running
            && let Some((_, count)) = &self.run
        {
            count.cancel();
        }
    }

    pub fn can_run(&self) -> bool {
        matches!(self.phase, Phase::Idle | Phase::ReviewReady | Phase::RecoverableError)
    }

    /// Start re-reading a whole page (`region` `None`) or a drawn region in the job queue:
    /// the UI thread only builds the job and refuses an empty or out-of-page region BEFORE
    /// anything runs — the render, the recognition and the engine probe (a `--list-langs`
    /// process for Tesseract) happen off it, with the status line and a Cancel while they run.
    /// The reading is applied by [`Self::land_reread`] when it lands.
    pub fn start_reread(&mut self, app: &mut PdfCraftApp, page: usize, region: Option<[f32; 4]>) {
        if self.phase != Phase::ReviewReady || self.reread.is_some() {
            return; // one re-read at a time, and never under a run
        }
        let Some(doc) = self.doc else { return };
        let settings = self.reread_settings(&app.ocr_draft);
        #[cfg(target_arch = "wasm32")]
        if app.web_models_missing(settings.engine) {
            self.wait_for_models(AwaitingModels::Reread { page, region });
            return;
        }
        if let Some(region) = region {
            let Some(info) = app.session.get(doc).and_then(|d| d.info.pages.get(page)) else {
                app.notify_error("the page is gone");
                return;
            };
            // Refuse before running: an empty, inverted or out-of-page rectangle never
            // reaches the engine (the words are untouched).
            let scale = app.session.ocr_job(doc, &[page], settings.clone()).map_or(1.0, |job| job.render_scale(page));
            let (w, h) = (info.width * scale, info.height * scale);
            let px = [region[0] * scale, region[1] * scale, region[2] * scale, region[3] * scale];
            if clamp_region(px, w.max(1.0) as u32, h.max(1.0) as u32).is_none() {
                app.notify_error("the selected region is empty or outside the page");
                return;
            }
        }
        let Some(mut job) = app.session.ocr_job(doc, &[page], settings) else {
            app.notify_error("the document is no longer open");
            return;
        };
        if let Some(region) = region {
            let scale = job.render_scale(page);
            job.settings.region = Some([region[0] * scale, region[1] * scale, region[2] * scale, region[3] * scale]);
        }
        let is_region = region.is_some();
        self.message = if is_region { tl!("Re-reading the region…").to_string() } else { tl!("Re-reading the page…").to_string() };
        self.ticket = self.ticket.wrapping_add(1);
        let ticket = self.ticket;
        // PLAN: assumed `PdfCraftApp::verify_jobs: JobQueue<VerifyJob, VerifyDone>` and `PdfCraftApp::wake: craft_host::Wake` (pdf-hub-w1).
        let wake = app.wake.clone();
        app.verify_jobs.submit(VerifyJob::Reread(ticket), Priority::default(), move |cancel| {
            let result = pdfcraft_engine::ocr::recognizers(&job.settings).and_then(|r| {
                let found = job.run(&r, |_, _| !cancel.is_cancelled());
                let words: Vec<PlacedWord> = found.into_iter().flat_map(|pg| pg.words).collect();
                if words.is_empty() {
                    Err(if is_region {
                        "the region re-read found no text; the existing words are unchanged".into()
                    } else {
                        "the re-read found no text; the existing words are unchanged".into()
                    })
                } else {
                    Ok(words)
                }
            });
            wake.wake();
            VerifyDone::Reread(result)
        });
        self.reread = Some(Reread { page, region, ticket });
    }

    /// The web build is downloading the models: remember what was asked for and say so; it
    /// starts when they land ([`Self::poll_models`]). Before, the read ran at once and failed
    /// with the desktop's "set PDFCRAFT_MODELS" message, which means nothing in a browser.
    #[cfg(target_arch = "wasm32")]
    fn wait_for_models(&mut self, what: AwaitingModels) {
        self.awaiting_models = Some(what);
        self.message = tl!("Downloading the text recognition models…").to_string();
    }

    /// The web build: start what waited for the models once the download is over.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn poll_models(&mut self, app: &mut PdfCraftApp) {
        if self.awaiting_models.is_none() {
            return;
        }
        use pdfcraft_engine::ocr::{EngineChoice, engine_unavailable_reason};
        if engine_unavailable_reason(EngineChoice::Ocrs).is_none() {
            // The models are in. An engine the browser can never run (Tesseract) says why
            // rather than waiting for ever.
            let outcome = engine_unavailable_reason(app.ocr_draft.engine).map_or(Ok(()), Err);
            self.models_landed(app, outcome);
        } else if let crate::ocr_web::State::Failed(e) = crate::ocr_web::state() {
            self.models_landed(app, Err(e));
        }
    }

    /// The model download is over: run what waited for it, or say why it can't. Nothing runs
    /// once the screen is closed.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn models_landed(&mut self, app: &mut PdfCraftApp, outcome: Result<(), String>) {
        let Some(what) = self.awaiting_models.take() else { return };
        self.message.clear();
        if let Err(e) = outcome {
            self.message = crate::i18n::fmt(tl!("Couldn't download the text recognition models: {e}"), &[("e", &e)]);
            return;
        }
        if !self.open {
            return;
        }
        match what {
            AwaitingModels::Run => self.start_run(app),
            AwaitingModels::Reread { page, region } => self.start_reread(app, page, region),
        }
    }

    /// Drop a re-read in flight: its key leaves the queue, so whatever the job makes of it is
    /// never applied and the page's words stay as they were.
    pub fn cancel_reread(&mut self, app: &mut PdfCraftApp) {
        let Some(reread) = self.reread.take() else { return };
        let key = VerifyJob::Reread(reread.ticket);
        let _ = app.verify_jobs.reprioritize(|k, p| (*k != key).then_some(p));
        self.message = tl!("The re-read was cancelled; the words are unchanged").to_string();
    }

    /// Apply a finished re-read: a page re-read replaces the page's words, a region re-read the
    /// words inside the drawn rectangle. A result that is not the re-read in flight (an earlier
    /// ticket) is dropped, and so is a refusal — a document changed since the review began no
    /// longer matches the page the words were read from.
    pub(crate) fn land_reread(&mut self, app: &mut PdfCraftApp, ticket: u64, result: Result<Vec<PlacedWord>, String>) {
        let Some(reread) = &self.reread else { return };
        if reread.ticket != ticket {
            return;
        }
        let (page, region) = (reread.page, reread.region);
        self.reread = None;
        if self.phase != Phase::ReviewReady {
            return;
        }
        // The document changed since the review began: the reading may no longer fit the page
        // it was read from, so it is refused with the reason and the words stay.
        if let Some(why) = self.guard.as_ref().and_then(|g| app.session.stale_reason(g)) {
            self.message = why.clone();
            app.notify_error(why);
            return;
        }
        match result {
            Err(why) => {
                self.message = why.clone();
                app.notify_error(why);
            }
            Ok(words) => {
                let applied = if let Some(region) = region {
                    self.replace_region_words(&app.session, page, region, words).map(|_| true)
                } else if self.replace_page_words(page, words) {
                    Ok(false)
                } else {
                    Err("the page is gone".to_string())
                };
                match applied {
                    Ok(is_region) => {
                        if is_region {
                            self.message = tl!("The region was read again; its words were replaced (undo in the panel)").to_string();
                            app.notify_tr("The region was read again; its words were replaced (undo in the panel)");
                        } else {
                            self.message = tl!("The page was read again; its words were replaced (undo in the panel)").to_string();
                            app.notify_tr("The page was read again; its words were replaced (undo in the panel)");
                        };
                    }
                    Err(why) => app.notify_error(why),
                }
            }
        }
    }

    /// Put `words` (the worker's reading of the drawn rectangle) in place of the words inside
    /// it: the rectangle's bbox in user space decides which old words go; the kept words'
    /// review marks follow them to their new positions, the replacement is undoable within
    /// the panel, and the first NEW word is selected. An empty reading was refused before the
    /// run, so `words` is never empty here.
    fn replace_region_words(&mut self, session: &Session, page: usize, region_view: [f32; 4], words: Vec<PlacedWord>) -> Result<(), String> {
        let Some(doc) = self.doc else { return Ok(()) };
        let Some(info) = session.get(doc).and_then(|d| d.info.pages.get(page)) else { return Err("the page is gone".into()) };
        // The rectangle in user space (a bbox of its mapped corners) decides which old words
        // the new reading replaces.
        let (a, b, c, d) = (
            info.view_to_user(region_view[0], region_view[1]),
            info.view_to_user(region_view[2], region_view[3]),
            info.view_to_user(region_view[0], region_view[3]),
            info.view_to_user(region_view[2], region_view[1]),
        );
        let xs = [a[0] as f64, b[0] as f64, c[0] as f64, d[0] as f64];
        let ys = [a[1] as f64, b[1] as f64, c[1] as f64, d[1] as f64];
        let bbox = [
            xs.iter().cloned().fold(f64::INFINITY, f64::min),
            ys.iter().cloned().fold(f64::INFINITY, f64::min),
            xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        ];
        let Some(pg) = self.page_mut(page) else { return Err("the page is gone".into()) };
        let before = pg.words.clone();
        let center = |w: &PlacedWord| [w.origin[0] + w.across[0] / 2.0 + w.up[0] / 2.0, w.origin[1] + w.across[1] / 2.0 + w.up[1] / 2.0];
        // The old words outside the rectangle stay, in order; `moved` maps each kept word's old
        // position to its new one, so its review marks follow it.
        let mut moved = std::collections::BTreeMap::new();
        let mut after: Vec<PlacedWord> = Vec::with_capacity(before.len().saturating_add(words.len()));
        for (i, w) in before.iter().enumerate() {
            let c = center(w);
            if !(c[0] >= bbox[0] && c[0] <= bbox[2] && c[1] >= bbox[1] && c[1] <= bbox[3]) {
                moved.insert(i, after.len());
                after.push(w.clone());
            }
        }
        // The new words follow the kept ones, so the first of them is at the kept words' count;
        // selecting it shows the reviewer what the re-read produced.
        let first_new = after.len();
        after.extend(words.iter().cloned());
        pg.words = after;
        let keys = self.take_page_keys(page);
        let remap = |old: &[usize]| old.iter().filter_map(|i| moved.get(i).map(|&j| (page, j))).collect::<Vec<_>>();
        self.review.removed.extend(remap(&keys.removed));
        self.review.verified.extend(remap(&keys.verified));
        self.review.history.push(Op::Region { page, before, keys });
        self.select((page, first_new));
        Ok(())
    }

    /// Store a finished run's pages (the worker's result) and move to ReviewReady. A cancel is
    /// honest: the pages already read are kept, and the status says the rest was not read.
    pub fn finished(&mut self, found: Vec<OcrPage>, cancelled: bool) {
        let read = found.iter().filter(|p| p.skipped.is_none()).count();
        let found_words: usize = found.iter().map(|p| p.words.len()).sum();
        self.pages = found;
        self.review = Review::default();
        self.selected = None;
        self.region = None;
        self.show_a_read_page();
        let total = self.pages.len();
        self.message = if cancelled {
            crate::i18n::fmt(
                tl!("Cancelled — {n} words were read on {m} of {t} pages; the rest were not read"),
                &[("n", &found_words.to_string()), ("m", &read.to_string()), ("t", &total.to_string())],
            )
        } else if found_words == 0 {
            tl!("OCR complete — no text recognized on this page.").to_string()
        } else {
            crate::i18n::fmt(
                tl!("Read {n} words on {m} of {t} pages"),
                &[("n", &found_words.to_string()), ("m", &read.to_string()), ("t", &total.to_string())],
            )
        };
        self.phase = Phase::ReviewReady;
        self.load_dictionary_pref();
    }

    /// A run that failed: the cause is named and Run is enabled again.
    pub fn failed(&mut self, why: &str) {
        self.phase = Phase::RecoverableError;
        self.message = why.to_string();
    }

    pub(crate) fn start_run(&mut self, app: &mut PdfCraftApp) {
        let Some((_, id)) = app.active_ids() else { return };
        if !self.can_run() || self.phase == Phase::Running {
            return;
        }
        if self.reread.is_some() {
            return; // the page re-read in flight replaces these words; one worker at a time
        }
        // A review of another document stays until it is accepted or rejected: a run here would
        // replace its corrections.
        if self.has_reading() && self.doc != Some(id) {
            self.message = tl!("Accept or reject the OCR review of the other document before running OCR here").to_string();
            return;
        }
        if let Some(why) = app.session.get(id).and_then(|d| d.read_only_reason.clone()) {
            self.message = why;
            self.phase = Phase::RecoverableError;
            return;
        }
        let d = &app.ocr_draft;
        let pages: Vec<usize> = match d.pages {
            super::ocr_ui::OcrPages::All => Vec::new(),
            super::ocr_ui::OcrPages::Current => vec![app.active.map_or(0, |i| app.views.get(i).map_or(0, |v| v.current()))],
            super::ocr_ui::OcrPages::Range => (d.from.saturating_sub(1)..d.to).collect(),
        };
        let settings = d.settings();
        #[cfg(target_arch = "wasm32")]
        if app.web_models_missing(settings.engine) {
            self.wait_for_models(AwaitingModels::Run);
            return;
        }
        let Some(job) = app.session.ocr_job(id, &pages, settings.clone()) else { return };
        let language = job.settings.language.clone();
        let guard = job.guard.clone();
        // The source pane shows the first page this run reads (the current page, for that scope).
        let first = job.pages.first().copied().unwrap_or(0);
        let count = Arc::new(JobCount::default());
        count.set(0, job.pages.len());
        let reporting = count.clone();
        let wake = app.wake.clone();
        self.ticket = self.ticket.wrapping_add(1);
        let ticket = self.ticket;
        app.verify_jobs.submit(VerifyJob::Run(ticket), Priority::default(), move |_| {
            let result = pdfcraft_engine::ocr::recognizers(&settings).map(|r| {
                job.run(&r, |done, total| {
                    reporting.set(done, total);
                    wake.wake();
                    !reporting.cancelled()
                })
            });
            wake.wake();
            VerifyDone::Run(result)
        });
        self.dictionary_language = language;
        self.doc = Some(id);
        self.guard = Some(guard);
        self.current = first;
        self.run = Some((ticket, count));
        self.phase = Phase::Running;
        self.message = String::new();
    }

    /// Take what the queue finished, once per frame: a result lands only when its ticket is the
    /// current one. A lane the host dropped is handed back (`resume`) while something is out,
    /// and the web build, which has no lanes, runs one job a frame here.
    pub(crate) fn poll(&mut self, app: &mut PdfCraftApp) {
        #[cfg(target_arch = "wasm32")]
        app.verify_jobs.run_inline(1);
        while let Some(done) = app.verify_jobs.try_recv() {
            match (done.key, done.result) {
                (VerifyJob::Run(ticket), VerifyDone::Run(result)) => self.land_run(ticket, result),
                (VerifyJob::Reread(ticket), VerifyDone::Reread(result)) => self.land_reread(app, ticket, result),
                _ => {}
            }
        }
        // Waiting work with no lane alive means the host dropped the lane; `resume` gives it one.
        if (self.run.is_some() || self.reread.is_some()) && {
            let counts = app.verify_jobs.counts();
            counts.queued > 0 && counts.running == 0
        } {
            app.verify_jobs.resume();
        }
        if self.phase == Phase::Running
            && let Some((_, count)) = &self.run
        {
            let (done, total) = count.read();
            self.message = crate::i18n::fmt(
                tl!("Recognizing text… page {d} of {t}"),
                &[("d", &(done + 1).min(total.max(1)).to_string()), ("t", &total.max(1).to_string())],
            );
        }
    }

    /// Store a finished run, unless it is not the run in flight. A cancel keeps the pages the
    /// run had read.
    fn land_run(&mut self, ticket: u64, result: Result<Vec<OcrPage>, String>) {
        let Some((current, count)) = &self.run else { return };
        if *current != ticket {
            return;
        }
        let cancelled = count.cancelled();
        self.run = None;
        match result {
            Ok(found) => self.finished(found, cancelled),
            Err(why) => self.failed(&why),
        }
    }
}

/// Run `f` with the screen's state moved out of the app (a step that needs both the state and
/// the rest of the app would otherwise borrow it twice). The state is put back afterwards.
pub(crate) fn with_state(app: &mut PdfCraftApp, f: impl FnOnce(&mut OcrVerify, &mut PdfCraftApp)) {
    let mut v = std::mem::take(&mut app.ocr_verify);
    f(&mut v, app);
    app.ocr_verify = v;
}

/// Apply what the frame's controls asked for. The guards live in the [`OcrVerify`] methods:
/// outside ReviewReady, Accept/Reject/Re-OCR and edits are ignored, not acted on.
fn apply(app: &mut PdfCraftApp, ctx: &egui::Context, n: Intents) {
    with_state(app, |v, app| {
        if n.exit {
            v.open = false;
        }
        if let Some(p) = n.select_page {
            v.current = p;
            if v.selected.is_some_and(|k| k.0 != p) {
                v.selected = None;
            }
        }
        if let Some(k) = n.select_word {
            v.select(k);
        }
        if n.toggle_page_verified {
            let on = !v.review.verified_pages.contains(&v.current);
            v.set_page_verified(v.current, on);
        }
        if n.next_uncertain {
            v.goto_uncertain(true);
        }
        if n.prev_uncertain {
            v.goto_uncertain(false);
        }
        if n.undo {
            v.undo();
        }
        if let Some(text) = n.correct
            && let Some(k) = v.selected
        {
            v.correct(k, &text);
            v.correction = v.text_of(k).unwrap_or_default().to_owned();
        }
        if let Some(removed) = n.set_removed
            && let Some(k) = v.selected
        {
            v.set_removed(k, removed);
        }
        if let Some(on) = n.set_word_verified
            && let Some(k) = v.selected
        {
            v.set_word_verified(k, on);
        }
        if n.dictionary_add
            && let Some(k) = v.selected
        {
            let word = v.text_of(k).unwrap_or_default().to_owned();
            let language = v.dictionary_language.clone();
            v.add_to_dictionary(&word, &language);
        }
        if n.reocr_page {
            let page = v.current;
            v.start_reread(app, page, None);
        }
        if n.reocr_region {
            let (page, region) = (v.current, v.region);
            if let Some(region) = region {
                v.start_reread(app, page, Some(region));
            }
        }
        if n.cancel_reread {
            v.cancel_reread(app);
        }
        if n.accept_page {
            let page = v.current;
            match v.accept_page(&mut app.session, page) {
                Ok(count) if count > 0 => {
                    if let Some(doc) = v.doc {
                        refresh_view(app, ctx, doc);
                    }
                    app.notify_fmt(tl!("The page's words were applied as one undoable step ({n} words)"), &[("n", &count.to_string())]);
                }
                Ok(_) => {}
                Err(why) => app.notify_error(why),
            }
        }
        if n.reject_page {
            let page = v.current;
            if v.reject_page(page) {
                app.notify_tr("The page's words were cleared (undo in the panel)");
            }
        }
        if n.run {
            v.start_run(app);
        }
        if n.cancel {
            v.cancel();
        }
        if n.accept {
            do_accept(v, app, ctx);
        }
        if n.reject && v.reject() {
            app.notify_tr("OCR results rejected.");
        }
    });
}

/// Accept: searchable mode applies the review to the open document as one undoable step;
/// editable mode asks where to write the new `<name>_ocr.pdf`. Stale sessions and empty
/// reviews are typed failures the user sees (the edits are kept).
fn do_accept(v: &mut OcrVerify, app: &mut PdfCraftApp, ctx: &egui::Context) {
    let doc = v.doc;
    match app.ocr_draft.output_mode {
        OutputMode::Searchable => match v.accept(&mut app.session) {
            Ok(count) => {
                if let Some(doc) = doc {
                    refresh_view(app, ctx, doc);
                }
                if count > 0 {
                    app.notify_fmt(tl!("Recognized text applied ({n} words, one undoable step)"), &[("n", &count.to_string())]);
                }
            }
            Err(why) => app.notify_error(why),
        },
        OutputMode::EditableText => match v.editable_output(&app.session) {
            Ok((bytes, name)) => save_editable(app, bytes, name, doc),
            Err(why) => app.notify_error(why),
        },
    }
}

/// Write the editable-text output: ask where (the default name is `<name>_ocr.pdf`). The source is never overwritten; a
/// cancelled dialog keeps the review. The write is the [`Phase::Saving`] state: it ends the
/// review when the file lands, and returns to ReviewReady when it does not, so a failed or
/// cancelled save can simply be tried again.
fn save_editable(app: &mut PdfCraftApp, bytes: Arc<Vec<u8>>, name: String, doc: Option<DocId>) {
    let source = doc.and_then(|id| app.session.get(id)).and_then(|d| d.path.clone());
    // The chosen file is the open source when both resolve to the same path (a new file does
    // not exist yet, so fall back to its folder and name).
    let is_source = |picked: &std::path::Path, source: &Option<String>| {
        let Some(src) = source else { return false };
        if let (Ok(a), Ok(b)) = (std::fs::canonicalize(picked), std::fs::canonicalize(src)) {
            return a == b;
        }
        let same_name = picked.file_name().is_some_and(|n| n.to_string_lossy() == src.as_str());
        let same_folder = std::fs::canonicalize(src)
            .ok()
            .zip(picked.parent().map(std::path::Path::to_path_buf))
            .is_some_and(|(s, parent)| s.parent().is_some_and(|p| p == parent));
        same_name && same_folder
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let request = craft::host::PickRequest::save("export").filter("PDF", &["pdf"]).file_name(name.clone());
        app.pick_file(request, None, move |app, _, picked| {
            // A cancelled or unavailable picker leaves the review as it was.
            let Some(picked) = picked.first_path() else { return };
            if is_source(&picked, &source) {
                app.notify_tr("That is the source file; choose another destination");
                return;
            }
            write_editable(app, &picked, bytes);
        });
    }
    // The web build has no file system to write into: the new document downloads under its
    // `<name>_ocr.pdf` name, so it never lands on the source either.
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (&source, doc, is_source);
        // The host's file service hands a write to the page or the browser's downloads.
        let saved = app.host.files().write(std::path::Path::new(&name), &bytes, craft::host::WriteIntent::SaveAs);
        with_state(app, |v, _| {
            if saved.is_ok() {
                v.finish();
            } else {
                v.phase = Phase::ReviewReady;
            }
        });
        if let Err(e) = saved {
            app.notify_error(e);
        }
    }
}

/// The editable write itself: [`Phase::Saving`] while it runs, Idle once the file lands (the
/// review is done), back to ReviewReady when it fails — the reviewer sees the error and can
/// accept again without re-reading the document. Desktop only: the web build downloads instead.
#[cfg(not(target_arch = "wasm32"))]
fn write_editable(app: &mut PdfCraftApp, target: &std::path::Path, bytes: Arc<Vec<u8>>) {
    with_state(app, |v, _| v.phase = Phase::Saving);
    // PLAN: assumed `PdfCraftApp::host: Arc<Host>` (pdf-hub-w1).
    match app.host.files().write(target, &bytes, craft::host::WriteIntent::SaveAs) {
        Ok(_) => {
            with_state(app, |v, _| v.finish());
            app.notify(crate::i18n::fmt(tl!("Wrote {name}"), &[("name", &target.to_string_lossy())]));
        }
        Err(e) => {
            with_state(app, |v, _| v.phase = Phase::ReviewReady);
            app.notify_error(e);
        }
    }
}

/// Pull the document's changed appearance into its view after an apply.
fn refresh_view(app: &mut PdfCraftApp, ctx: &egui::Context, doc: DocId) {
    if let Some(info) = app.session.get(doc).map(|d| d.info.clone())
        && let Some(view) = app.views.iter_mut().find(|view| view.id == doc)
    {
        view.document_changed(ctx, &info);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft::host::Picked;
    use pdfcraft_engine::Session;
    use pdfcraft_engine::ocr::EngineChoice;

    /// A word box that sits where a real placed word would (high on the page, reading left
    /// to right).
    fn word(text: &str, conf: Option<f32>) -> PlacedWord {
        PlacedWord { text: text.into(), origin: [72.0, 700.0], across: [40.0, 0.0], up: [0.0, 12.0], confidence: conf, source: "ocrs".into() }
    }

    /// One page of words with the given confidences, and a review loaded with it (the guard
    /// comes from a real job, as it does on screen).
    fn loaded(session: &mut Session, confs: &[Option<f32>]) -> (DocId, OcrVerify) {
        let text = session.create_from_text("t", "Recognize these scanned words").expect("fixture");
        let id = session.open("t.pdf", None, text, None).expect("fixture");
        let job = session.ocr_job(id, &[0], OcrSettings::default()).expect("fixture");
        let page = OcrPage { already_on_page: 0, page: 0, words: confs.iter().map(|c| word("w", *c)).collect(), skipped: None, notes: Vec::new() };
        let mut v = OcrVerify::default();
        v.load(id, job.guard.clone(), vec![page]);
        (id, v)
    }

    /// F8/Shift+F8 walk the uncertain words: wrap around, skip removed words, and a page
    /// marked verified drops out entirely. With nothing to show, the walk is a no-op.
    #[test]
    fn uncertain_walk_wraps_and_skips_removed_and_verified() {
        let mut s = Session::new();
        // 95 is fine; 40 and 50 are Low; the third word is corrected into the dictionary.
        let (_, mut v) = loaded(&mut s, &[Some(95.0), Some(40.0), Some(50.0), Some(95.0)]);
        assert_eq!(v.uncertain(), vec![(0, 1), (0, 2)], "the Low-band words, in order");
        assert!(v.goto_uncertain(true));
        assert_eq!(v.selected, Some((0, 1)));
        assert!(v.goto_uncertain(true));
        assert_eq!(v.selected, Some((0, 2)));
        assert!(v.goto_uncertain(true), "wraps around");
        assert_eq!(v.selected, Some((0, 1)));
        // A removed word leaves the walk; a corrected word stays by its original confidence.
        v.set_removed((0, 1), true);
        assert!(v.goto_uncertain(true));
        assert_eq!(v.selected, Some((0, 2)));
        assert!(v.correct((0, 2), "fixed"), "the correction commits");
        assert!(v.uncertain().contains(&(0, 2)), "a corrected word stays uncertain by its original confidence");
        v.set_word_verified((0, 2), true);
        assert!(v.uncertain().is_empty(), "a vouched-for word leaves the walk");
        assert!(!v.goto_uncertain(true), "nothing left: a no-op");
        // A page marked verified drops all its words from the walk.
        v.set_word_verified((0, 2), false);
        v.set_removed((0, 1), false);
        v.set_page_verified(0, true);
        assert!(v.uncertain().is_empty());
    }

    /// The review tools are guarded: outside ReviewReady they are ignored, not acted on.
    #[test]
    fn guards_ignore_review_actions_outside_reviewready() {
        let mut v = OcrVerify::default();
        assert_eq!(v.phase, Phase::Idle);
        assert!(v.can_run(), "Run is enabled in Idle");
        assert!(!v.reject(), "Reject is ignored in Idle");
        assert!(!v.set_removed((0, 0), true));
        assert!(!v.correct((0, 0), "x"));
        assert!(!v.set_page_verified(0, true));
        assert!(!v.reject_page(0));
        let mut s = Session::new();
        assert_eq!(v.accept(&mut s).expect("ignored, not an error"), 0);
        v.phase = Phase::Saving;
        assert!(!v.can_run(), "Run is disabled while saving");
        v.phase = Phase::RecoverableError;
        assert!(v.can_run(), "Run is enabled again after a recoverable error");
    }

    /// Accept: zero words left is a typed failure; a real review is applied as ONE undoable
    /// step; a stale session is refused with its reason and the edits are kept.
    #[test]
    fn accept_zero_words_fails_and_a_live_review_applies_once() {
        let mut s = Session::new();
        let (id, mut v) = loaded(&mut s, &[Some(95.0), Some(40.0)]);
        v.set_removed((0, 0), true);
        v.set_removed((0, 1), true);
        assert!(v.accept(&mut s).is_err(), "zero words: a typed failure");
        assert_eq!(v.phase, Phase::ReviewReady, "the failure keeps the review");
        let live = v.live_pages();
        assert!(live[0].words.is_empty(), "removed words are not written");
        // Restore one word: the accept applies the whole review in one step.
        v.set_removed((0, 0), false);
        let n = v.accept(&mut s).expect("the accept applies");
        assert_eq!(n, 1);
        assert_eq!(s.get(id).expect("open").can_undo(), Some("Recognize text"), "one undoable step");
        assert_eq!(v.phase, Phase::Idle, "a successful accept ends the review");
        // A second accept of the same (now spent) review has nothing to write.
        let mut v2 = OcrVerify::default();
        let job = s.ocr_job(id, &[0], OcrSettings::default()).expect("fixture");
        v2.load(
            id,
            job.guard.clone(),
            vec![OcrPage { already_on_page: 0, page: 0, words: vec![word("w", Some(95.0))], skipped: None, notes: Vec::new() }],
        );
        v2.set_removed((0, 0), true);
        assert!(v2.accept(&mut s).is_err());
    }

    /// Accept refuses a stale session: the document was edited since the job started, the
    /// refusal names the reason, and the review edits survive.
    #[test]
    fn accept_refuses_a_stale_session_with_a_named_reason() {
        let mut s = Session::new();
        let (id, mut v) = loaded(&mut s, &[Some(95.0)]);
        assert!(v.correct((0, 0), "Corrected"), "the correction commits");
        // The document changed since the job started: the result no longer applies.
        s.apply(id, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).expect("fixture");
        let why = v.accept(&mut s).expect_err("the stale result is refused");
        assert!(why.contains("changed"), "{why}");
        assert_eq!(v.phase, Phase::RecoverableError, "Run is enabled again");
        assert_eq!(v.text_of((0, 0)), Some("Corrected"), "the edits are kept");
    }

    /// "Accept entire page" applies just that page and a later whole-review Accept never
    /// writes it twice; "Reject entire page" clears the page (undoable in the panel).
    #[test]
    fn accept_page_then_accept_does_not_double_write() {
        let mut s = Session::new();
        let (_, mut v) = loaded(&mut s, &[Some(95.0), Some(95.0)]);
        v.pages = vec![OcrPage {
            already_on_page: 0,
            page: 0,
            words: vec![word("one", Some(95.0)), word("two", Some(95.0))],
            skipped: None,
            notes: Vec::new(),
        }];
        let n = v.accept_page(&mut s, 0).expect("the page applies");
        assert_eq!(n, 2);
        assert!(v.review.applied_pages.contains(&0));
        // Reject entire page: the words clear (undoable), then come back.
        assert!(v.reject_page(0));
        assert!(v.page(0).expect("page").words.is_empty());
        assert!(v.undo(), "the clear is undoable");
        assert_eq!(v.page(0).expect("page").words.len(), 2);
        // A zero-word page cannot be accepted.
        v.reject_page(0);
        assert!(v.accept_page(&mut s, 0).is_err(), "zero words on the page: a typed failure");
    }

    /// The text the document's page `page` carries now, as the renderer extracts it.
    fn page_text(s: &Session, id: DocId, page: usize) -> String {
        let doc = s.get(id).expect("open");
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), pdfcraft_render::RenderConfig::default());
        let request = pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() };
        r.render(request).text.map(|t| t.plain_text()).unwrap_or_default()
    }

    /// A blank document of `pages` pages, open, with a review of one word list per page loaded
    /// against a real job's guard.
    fn blank_review(s: &mut Session, words: Vec<Vec<PlacedWord>>) -> (DocId, OcrVerify) {
        let blank = s.create_blank(612.0, 792.0, words.len()).expect("fixture");
        let id = s.open("blank.pdf", None, blank, None).expect("fixture");
        let job = s.ocr_job(id, &[], OcrSettings::default()).expect("fixture");
        let pages = words
            .into_iter()
            .enumerate()
            .map(|(page, words)| OcrPage { already_on_page: 0, page, words, skipped: None, notes: Vec::new() })
            .collect();
        let mut v = OcrVerify::default();
        v.load(id, job.guard.clone(), pages);
        (id, v)
    }

    /// Pages accepted one at a time, then the whole review: each page-accept is the document's
    /// newest edit, so the next one (and the final Accept) still applies, and every page is
    /// written exactly once. Before the guard was refreshed, the second page-accept was refused
    /// as "the document changed since recognition started" and the review was stuck.
    #[test]
    fn accepting_pages_one_by_one_then_the_review_applies_every_page_once() {
        let mut s = Session::new();
        let (id, mut v) = blank_review(&mut s, vec![vec![word("one", Some(95.0))], vec![word("two", Some(95.0))], vec![word("three", Some(95.0))]]);
        assert_eq!(v.accept_page(&mut s, 0), Ok(1));
        assert_eq!(v.accept_page(&mut s, 1), Ok(1), "the second page applies after the first");
        assert_eq!(v.phase, Phase::ReviewReady);
        assert_eq!(v.accept(&mut s), Ok(1), "the final Accept writes only the page not yet applied");
        assert_eq!(v.phase, Phase::Idle, "the review ended");
        for (page, text) in ["one", "two", "three"].into_iter().enumerate() {
            let on_page = page_text(&s, id, page);
            assert_eq!(on_page.matches(text).count(), 1, "page {page} written exactly once: {on_page:?}");
        }
    }

    /// A page already applied ("Accept entire page") is never applied again: a second
    /// "Accept entire page" on it writes nothing (the OCR layer is not appended twice), and
    /// corrections made after the page-accept are kept in the review.
    #[test]
    fn accepting_a_page_twice_writes_it_once_and_keeps_later_corrections() {
        let mut s = Session::new();
        let (id, mut v) = blank_review(&mut s, vec![vec![word("one", Some(95.0))], vec![word("two", Some(95.0))]]);
        assert_eq!(v.accept_page(&mut s, 0), Ok(1), "the first accept applies the page");
        // The reviewer corrects a word on the applied page afterwards.
        assert!(v.correct((0, 0), "uno"), "the correction commits");
        // A second "Accept entire page" on the same page writes nothing.
        assert_eq!(v.accept_page(&mut s, 0), Ok(0), "already applied: ignored, not applied twice");
        let text = page_text(&s, id, 0);
        assert_eq!(text.matches("one").count(), 1, "the page was written exactly once: {text:?}");
        // The later correction is kept (not lost by the refusal), and the final Accept writes
        // only the page not yet applied.
        assert_eq!(v.text_of((0, 0)), Some("uno"));
        assert_eq!(v.accept(&mut s), Ok(1), "the final Accept writes only page 2");
        let text = page_text(&s, id, 0);
        assert_eq!(text.matches("one").count() + text.matches("uno").count(), 1, "still written once: {text:?}");
        assert_eq!(page_text(&s, id, 1).matches("two").count(), 1);
    }

    /// The refreshed guard still catches an edit made elsewhere between two page-accepts: the
    /// next page is refused with the reason, nothing is written, and the review is kept.
    #[test]
    fn an_outside_edit_between_page_accepts_is_still_refused() {
        let mut s = Session::new();
        let (id, mut v) = blank_review(&mut s, vec![vec![word("one", Some(95.0))], vec![word("two", Some(95.0))]]);
        assert_eq!(v.accept_page(&mut s, 0), Ok(1));
        s.apply(id, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).expect("an unrelated edit");
        let why = v.accept_page(&mut s, 1).expect_err("stale after an outside edit");
        assert!(why.contains("changed since recognition started"), "{why}");
        assert_eq!(v.phase, Phase::RecoverableError);
        assert_eq!(v.page(1).expect("page").words.len(), 1, "the review is kept");
        assert!(page_text(&s, id, 1).trim().is_empty(), "nothing was written to page 2");
    }

    /// A word at `x` on the first line of a US Letter page, with a Low confidence.
    fn word_at(text: &str, x: f64) -> PlacedWord {
        PlacedWord { text: text.into(), origin: [x, 700.0], across: [40.0, 0.0], up: [0.0, 12.0], confidence: Some(40.0), source: "ocrs".into() }
    }

    /// Whole-page re-read: review marks are positional, so the page's marks go with the old
    /// words (a struck-out slot must not strike the new word there), a correction of an old
    /// word no longer shows on the new one, and the undo restores words and marks exactly.
    #[test]
    fn rereading_a_page_clears_its_marks_and_undo_restores_them() {
        let mut s = Session::new();
        let (_, mut v) = blank_review(&mut s, vec![vec![word_at("keep", 72.0), word_at("gone", 300.0), word_at("struck", 400.0)]]);
        assert!(v.set_removed((0, 2), true));
        assert!(v.set_word_verified((0, 0), true));
        assert!(v.correct((0, 1), "fixed"));
        let marks = (v.review.removed.clone(), v.review.verified.clone());
        assert!(v.replace_page_words(0, vec![word_at("a", 72.0), word_at("b", 300.0), word_at("c", 400.0)]));
        assert!(v.review.removed.is_empty() && v.review.verified.is_empty(), "the old marks are gone");
        assert!(!v.is_corrected((0, 1)), "the correction belonged to the old word list");
        let texts: Vec<_> = v.live_pages()[0].words.iter().map(|w| w.text.clone()).collect();
        assert_eq!(texts, ["a", "b", "c"], "no new word is struck out by an old mark");
        assert!(v.undo());
        assert_eq!((v.review.removed.clone(), v.review.verified.clone()), marks, "the undo restores the marks");
        assert!(v.is_corrected((0, 1)));
        let texts: Vec<_> = v.live_pages()[0].words.iter().map(|w| w.text.clone()).collect();
        assert_eq!(texts, ["keep", "fixed"], "the struck word stays out");
    }

    /// A blank two-page-shaped review inside a real app (the document open), one word list per
    /// page, against a real job's guard.
    fn app_review(words: Vec<Vec<PlacedWord>>) -> (PdfCraftApp, DocId) {
        let blank = Session::new().create_blank(612.0, 792.0, words.len()).expect("fixture").to_vec();
        let mut app = PdfCraftApp::for_unit_tests();
        app.open_bytes(&egui::Context::default(), "blank.pdf", None, blank).expect("fixture");
        let id = app.active_ids().expect("open").1;
        let job = app.session.ocr_job(id, &[], OcrSettings::default()).expect("fixture");
        let pages = words
            .into_iter()
            .enumerate()
            .map(|(page, words)| OcrPage { already_on_page: 0, page, words, skipped: None, notes: Vec::new() })
            .collect();
        app.ocr_verify.load(id, job.guard.clone(), pages);
        (app, id)
    }

    /// Runs what the app's queue holds, here, and lands the results as a frame's poll would.
    fn run_jobs(app: &mut PdfCraftApp) {
        assert!(app.verify_jobs.finish(std::time::Duration::from_secs(60)), "the queue ran dry");
        with_state(app, OcrVerify::poll);
    }

    /// What the app said to the user, from the notices it posted.
    fn said(app: &mut PdfCraftApp) -> String {
        crate::notices::texts(app, &egui::Context::default()).join("\n")
    }

    /// A re-read of page 1 in flight under ticket 1.
    fn reading(region: Option<[f32; 4]>) -> Reread {
        Reread { page: 0, region, ticket: 1 }
    }

    /// A page re-read runs in the app's queue: start_reread returns with the reading in flight
    /// and the words as they were (Run is refused while it is out), and the result is applied by
    /// the poll. No OCR models are needed: the engine fails fast here, which still walks the
    /// whole queue path — and a failed reading keeps the words.
    #[test]
    fn a_page_reread_runs_in_the_queue_and_poll_applies_it() {
        let (mut app, _) = app_review(vec![vec![word("one", Some(95.0))]]);
        app.ocr_draft.engine = EngineChoice::Ocrs; // an ocrs-only job never probes tesseract
        with_state(&mut app, |v, app| v.start_reread(app, 0, None));
        assert!(app.ocr_verify.reread.is_some(), "the reading is in flight");
        assert!(app.ocr_verify.message.contains("Re-reading"), "{}", app.ocr_verify.message);
        assert_eq!(app.ocr_verify.page(0).expect("page").words.len(), 1, "the words are as they were until it lands");
        // A full run waits for the re-read.
        with_state(&mut app, OcrVerify::start_run);
        assert_eq!(app.ocr_verify.phase, Phase::ReviewReady, "Run is refused while a re-read is out");
        run_jobs(&mut app);
        assert!(app.ocr_verify.reread.is_none(), "the re-read is over");
        assert_eq!(app.ocr_verify.phase, Phase::ReviewReady, "the review survives a failed re-read");
        assert_eq!(app.ocr_verify.page(0).expect("page").words.len(), 1, "the words survive");
        assert!(!app.ocr_verify.message.contains("Re-reading"), "the failure is said: {}", app.ocr_verify.message);
    }

    /// The web build: a Run OCR asked for while the models download runs once they land, rather
    /// than failing at once with the desktop's "set PDFCRAFT_MODELS" message.
    #[test]
    fn a_run_that_waited_for_the_models_starts_when_they_land() {
        let (mut app, _) = app_review(vec![vec![word("one", Some(95.0))]]);
        app.ocr_verify.open = true;
        app.ocr_verify.awaiting_models = Some(AwaitingModels::Run);
        with_state(&mut app, |v, app| v.models_landed(app, Ok(())));
        assert_eq!(app.ocr_verify.awaiting_models, None);
        assert_eq!(app.ocr_verify.phase, Phase::Running, "the run was submitted");
        run_jobs(&mut app);
        // The run either read the blank page (no words) or failed for want of models on this
        // machine. Either way the old review was replaced by a run.
        let ran = app.ocr_verify.phase == Phase::RecoverableError || app.ocr_verify.page(0).is_some_and(|p| p.words.is_empty());
        assert!(ran, "the run started: {:?} {}", app.ocr_verify.phase, app.ocr_verify.message);
    }

    /// The same for a page or region re-read.
    #[test]
    fn a_reread_that_waited_for_the_models_starts_when_they_land() {
        let (mut app, _) = app_review(vec![vec![word("one", Some(95.0))]]);
        app.ocr_verify.open = true;
        app.ocr_verify.awaiting_models = Some(AwaitingModels::Reread { page: 0, region: None });
        with_state(&mut app, |v, app| v.models_landed(app, Ok(())));
        assert_eq!(app.ocr_verify.awaiting_models, None);
        assert!(app.ocr_verify.reread.is_some(), "the re-read started");
        run_jobs(&mut app);
        assert!(app.ocr_verify.reread.is_none(), "and landed");
        assert_eq!(app.ocr_verify.phase, Phase::ReviewReady);
    }

    /// A failed download is said in the status line, and nothing runs or changes.
    #[test]
    fn a_failed_model_download_is_said_and_nothing_runs() {
        let (mut app, _) = app_review(vec![vec![word("one", Some(95.0))]]);
        app.ocr_verify.open = true;
        app.ocr_verify.awaiting_models = Some(AwaitingModels::Run);
        with_state(&mut app, |v, app| v.models_landed(app, Err("models/text-detection.onnx: HTTP 404".into())));
        assert_eq!(app.ocr_verify.awaiting_models, None);
        assert!(app.ocr_verify.message.contains("Couldn't download") && app.ocr_verify.message.contains("HTTP 404"), "{}", app.ocr_verify.message);
        assert_eq!(app.ocr_verify.phase, Phase::ReviewReady);
        assert_eq!(app.ocr_verify.page(0).expect("page").words.len(), 1, "the words are untouched");
    }

    /// Closing the screen while the models download drops what waited: nothing runs behind it.
    #[test]
    fn nothing_waiting_for_the_models_runs_once_the_screen_is_closed() {
        let (mut app, _) = app_review(vec![vec![word("one", Some(95.0))]]);
        app.ocr_verify.open = false;
        app.ocr_verify.awaiting_models = Some(AwaitingModels::Reread { page: 0, region: None });
        with_state(&mut app, |v, app| v.models_landed(app, Ok(())));
        assert_eq!(app.ocr_verify.awaiting_models, None);
        assert!(app.ocr_verify.reread.is_none());
        assert_eq!(app.ocr_verify.page(0).expect("page").words.len(), 1);
    }

    /// A re-read that finds nothing is a typed failure: the words stay, the review stays.
    #[test]
    fn a_reread_that_finds_nothing_keeps_the_words() {
        let (mut app, _) = app_review(vec![vec![word("one", Some(95.0))]]);
        app.ocr_verify.reread = Some(reading(None));
        let nothing = Err("the re-read found no text; the existing words are unchanged".to_string());
        with_state(&mut app, |v, app| v.land_reread(app, 1, nothing));
        assert!(app.ocr_verify.reread.is_none(), "the re-read is over");
        assert_eq!(app.ocr_verify.phase, Phase::ReviewReady);
        assert_eq!(app.ocr_verify.page(0).expect("page").words.len(), 1, "the words survive the failed re-read");
        assert!(app.ocr_verify.message.contains("found no text"), "{}", app.ocr_verify.message);
    }

    /// Cancel drops the re-read: whatever the worker makes of it is discarded when it lands,
    /// and the page's words stay as they were.
    #[test]
    fn a_cancelled_reread_is_discarded_and_keeps_the_words() {
        let (mut app, _) = app_review(vec![vec![word("one", Some(95.0))]]);
        app.ocr_draft.engine = EngineChoice::Ocrs;
        with_state(&mut app, |v, app| v.start_reread(app, 0, None));
        with_state(&mut app, |v, app| v.cancel_reread(app));
        assert!(app.ocr_verify.reread.is_none());
        assert!(app.ocr_verify.message.contains("cancelled"), "{}", app.ocr_verify.message);
        run_jobs(&mut app);
        let words: Vec<_> = app.ocr_verify.page(0).expect("page").words.iter().map(|w| w.text.clone()).collect();
        assert_eq!(words, ["one"], "nothing landed");
        assert!(app.ocr_verify.message.contains("cancelled"), "{}", app.ocr_verify.message);
    }

    /// A result of an earlier re-read than the one in flight is dropped.
    #[test]
    fn a_late_result_of_a_superseded_reread_is_dropped() {
        let (mut app, _) = app_review(vec![vec![word("one", Some(95.0))]]);
        app.ocr_verify.reread = Some(Reread { page: 0, region: None, ticket: 2 });
        with_state(&mut app, |v, app| v.land_reread(app, 1, Ok(vec![word("late", Some(95.0))])));
        assert!(app.ocr_verify.reread.is_some(), "the re-read in flight is still out");
        let words: Vec<_> = app.ocr_verify.page(0).expect("page").words.iter().map(|w| w.text.clone()).collect();
        assert_eq!(words, ["one"]);
    }

    /// A re-read of a document that changed since the review began is refused: the reading may
    /// no longer fit the page it was read from, so the words stay and the reason is said.
    #[test]
    fn a_reread_of_a_changed_document_is_refused() {
        let (mut app, id) = app_review(vec![vec![word("one", Some(95.0))], vec![word("two", Some(95.0))]]);
        app.ocr_verify.reread = Some(Reread { page: 1, region: None, ticket: 1 });
        app.session.apply(id, pdfcraft_engine::Edit::RotatePages { pages: vec![1], degrees: 90 }).expect("an edit");
        with_state(&mut app, |v, app| v.land_reread(app, 1, Ok(vec![word("late", Some(95.0))])));
        let words: Vec<_> = app.ocr_verify.page(1).expect("page").words.iter().map(|w| w.text.clone()).collect();
        assert_eq!(words, ["two"], "the late reading is not applied");
        assert!(app.ocr_verify.message.contains("changed"), "the refusal names its reason: {}", app.ocr_verify.message);
    }

    /// Region re-read (the worker's reading applied by the poll): the words outside the
    /// rectangle stay and their marks follow them to their new positions; the replaced word's
    /// marks go; the first NEW word is the one selected (it follows the kept words); undo
    /// restores everything.
    #[test]
    fn rereading_a_region_remaps_marks_and_selects_the_first_new_word() {
        let (mut app, _) = app_review(vec![vec![word_at("keep", 72.0), word_at("gone", 300.0), word_at("struck", 400.0)]]);
        // The drawn region and the worker's reading of it: one word, "new".
        let region = [290.0, 70.0, 350.0, 100.0];
        let new_word = PlacedWord {
            text: "new".into(),
            origin: [300.0, 700.0],
            across: [40.0, 0.0],
            up: [0.0, 12.0],
            confidence: Some(95.0),
            source: "ocrs".into(),
        };
        {
            let v = &mut app.ocr_verify;
            assert!(v.set_removed((0, 2), true));
            assert!(v.set_word_verified((0, 0), true));
            assert!(v.set_word_verified((0, 1), true));
            v.region = Some(region);
            v.reread = Some(reading(Some(region)));
        }
        let marks = {
            let v = &app.ocr_verify;
            (v.review.removed.clone(), v.review.verified.clone())
        };
        // "gone" spans x 300..340 and y 700..712 in user space: view y 80..92 on the 792 pt page.
        with_state(&mut app, |v, app| v.land_reread(app, 1, Ok(vec![new_word])));
        let v = &mut app.ocr_verify;
        assert!(v.reread.is_none(), "the re-read is over");
        let texts: Vec<_> = v.page(0).expect("page").words.iter().map(|w| w.text.clone()).collect();
        assert_eq!(texts, ["keep", "struck", "new"]);
        assert_eq!(v.selected, Some((0, 2)), "the first new word is selected");
        assert!(v.is_removed((0, 1)), "the struck word's mark moved with it");
        assert!(!v.is_removed((0, 2)), "the new word is not struck out");
        assert!(v.is_verified((0, 0)) && !v.is_verified((0, 1)) && !v.is_verified((0, 2)), "{:?}", v.review.verified);
        let live: Vec<_> = v.live_pages()[0].words.iter().map(|w| w.text.clone()).collect();
        assert_eq!(live, ["keep", "new"], "the struck word is never written, the new one is");
        assert!(v.undo());
        assert_eq!((v.review.removed.clone(), v.review.verified.clone()), marks);
        let texts: Vec<_> = v.page(0).expect("page").words.iter().map(|w| w.text.clone()).collect();
        assert_eq!(texts, ["keep", "gone", "struck"]);
    }

    /// The editable output: refuses zero words, keeps the open document untouched, and names
    /// the new file `<name>_ocr.pdf`.
    #[test]
    fn editable_output_refuses_zero_words_and_never_touches_the_source() {
        let mut s = Session::new();
        let (id, mut v) = loaded(&mut s, &[Some(95.0)]);
        let before = s.get(id).expect("open").bytes.clone();
        let (bytes, name) = v.editable_output(&s).expect("the output builds");
        assert!(name.ends_with("_ocr.pdf"), "{name}");
        assert!(std::sync::Arc::ptr_eq(&s.get(id).expect("open").bytes, &before), "the source is untouched");
        assert!(!bytes.is_empty());
        v.set_removed((0, 0), true);
        assert!(v.editable_output(&s).is_err(), "zero words: refused");
        assert_eq!(v.phase, Phase::ReviewReady);
    }

    /// The editable output is a new file made from words read earlier: when the document changed
    /// since (a redaction, say), it is refused with the reason, as Accept in searchable mode is.
    #[test]
    fn editable_output_refuses_a_document_that_changed_since_the_review_started() {
        let mut s = Session::new();
        let (id, v) = loaded(&mut s, &[Some(95.0)]);
        assert!(v.editable_output(&s).is_ok(), "an untouched document is fine");
        s.apply(id, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).expect("an edit");
        let err = v.editable_output(&s).expect_err("a changed document is refused");
        assert!(err.contains("changed since recognition started"), "{err}");
    }

    /// The editable save is the Saving state: the file lands and the review ends (Idle); a
    /// failed write returns to ReviewReady with the edits kept, and a cancelled or unanswered
    /// save dialog never touches the review either — Accept can simply be tried again.
    #[test]
    fn editable_save_lands_and_a_failed_or_cancelled_one_keeps_the_review() {
        let files = Arc::new(craft::host::ScriptedFiles::new());
        let host = craft::host::Host::builder(crate::APP_INFO).files(files.clone()).build();
        let mut app = PdfCraftApp::for_unit_tests_on(Arc::new(host));
        let text = app.session.create_from_text("t", "Recognize these scanned words").expect("fixture");
        // The source is a file on disk, which the destination is compared with.
        let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-source-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp folder");
        let source = dir.join("t.pdf");
        std::fs::write(&source, text.as_slice()).expect("source on disk");
        let id = app.session.open("t.pdf", Some(source.to_string_lossy().into_owned()), text, None).expect("fixture");
        let job = app.session.ocr_job(id, &[0], OcrSettings::default()).expect("fixture");
        app.ocr_verify.load(
            id,
            job.guard.clone(),
            vec![OcrPage { already_on_page: 0, page: 0, words: vec![word("w", Some(95.0))], skipped: None, notes: Vec::new() }],
        );
        assert!(app.ocr_verify.correct((0, 0), "Edited"), "the correction commits");
        app.ocr_draft.output_mode = OutputMode::EditableText;
        // The source file as the destination is refused: nothing is written, the review stays
        // in ReviewReady with the edit, Accept is enabled again.
        use craft::host::Files as _;
        // Each answer is queued for the save dialog the next Accept opens.
        files.answer_next(Picked::Paths(vec![source.clone()])).expect("queued");
        with_state(&mut app, |v, app| do_accept(v, app, &egui::Context::default()));
        app.process_picks(&egui::Context::default());
        assert_eq!(app.ocr_verify.phase, Phase::ReviewReady, "a refused destination keeps the review");
        assert_eq!(app.ocr_verify.text_of((0, 0)), Some("Edited"), "the edits are kept");
        // A cancelled save dialog: nothing is written, nothing changes.
        files.answer_next(Picked::Cancelled).expect("queued");
        with_state(&mut app, |v, app| do_accept(v, app, &egui::Context::default()));
        app.process_picks(&egui::Context::default());
        assert_eq!(app.ocr_verify.phase, Phase::ReviewReady, "a cancelled save keeps the review");
        assert_eq!(app.ocr_verify.text_of((0, 0)), Some("Edited"));
        assert_eq!(app.ocr_verify.doc, Some(id), "the review still belongs to its document");
        // A real destination: the file lands under `<name>_ocr.pdf` and the review ends.
        let target = PathBuf::from("out/t_ocr.pdf");
        files.answer_next(Picked::Paths(vec![target.clone()])).expect("queued");
        with_state(&mut app, |v, app| do_accept(v, app, &egui::Context::default()));
        app.process_picks(&egui::Context::default());
        assert!(files.file(&target).is_some(), "the editable copy is written");
        assert_eq!(app.ocr_verify.phase, Phase::Idle, "a successful save ends the review");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Region re-OCR refuses an empty, inverted or out-of-page rectangle BEFORE running: no
    /// job goes out, no engine is probed, and the existing words are intact.
    #[test]
    fn reread_refuses_an_empty_region_before_running() {
        let (mut app, id) = app_review(vec![vec![word("one", Some(95.0))]]);
        app.ocr_draft.dpi = 72; // one pixel per point, so the arithmetic below is exact
        // An empty rectangle never reaches the engine.
        with_state(&mut app, |v, app| v.start_reread(app, 0, Some([10.0, 10.0, 10.0, 60.0])));
        assert!(app.ocr_verify.reread.is_none(), "nothing runs");
        assert_eq!(app.ocr_verify.page(0).expect("page").words.len(), 1, "the existing words are intact");
        assert!(said(&mut app).contains("empty or outside"));
        // Nor does one wholly outside the page.
        with_state(&mut app, |v, app| v.start_reread(app, 0, Some([-50.0, -50.0, -1.0, -1.0])));
        assert!(app.ocr_verify.reread.is_none());
        assert_eq!(id, app.ocr_verify.doc.expect("the review keeps its document"));
    }

    /// Review edits are undoable within the panel, in order.
    #[test]
    fn review_edits_undo_in_order() {
        let mut s = Session::new();
        let (_, mut v) = loaded(&mut s, &[Some(40.0)]);
        assert!(v.correct((0, 0), "Fixed"), "commits");
        assert_eq!(v.text_of((0, 0)), Some("Fixed"));
        assert!(v.is_corrected((0, 0)));
        assert!(v.add_to_dictionary("Fixed", "en"));
        assert!(v.review.dictionary.contains("fixed"));
        assert!(v.set_removed((0, 0), true));
        assert!(v.is_removed((0, 0)));
        // Undo in order: the removal, then the dictionary add, then the correction.
        assert!(v.undo());
        assert!(!v.is_removed((0, 0)));
        assert!(v.undo());
        assert!(!v.review.dictionary.contains("fixed"));
        assert!(v.undo());
        assert_eq!(v.text_of((0, 0)), Some("w"));
        assert!(!v.undo(), "nothing left to undo");
    }

    /// Corrections commit on Enter; an empty (after trim) commit removes the word; the box
    /// never moves.
    #[test]
    fn correction_commits_and_clearing_removes() {
        let mut s = Session::new();
        let (_, mut v) = loaded(&mut s, &[Some(95.0)]);
        let box_before = v.word((0, 0)).map(|w| (w.origin, w.across, w.up));
        assert!(v.correct((0, 0), "  Fixed  "), "commits");
        assert_eq!(v.text_of((0, 0)), Some("  Fixed  "), "the reviewer's text is kept verbatim");
        assert_eq!(v.word((0, 0)).map(|w| (w.origin, w.across, w.up)), box_before, "the box never moves");
        assert!(v.correct((0, 0), "   "), "an empty commit removes");
        assert!(v.is_removed((0, 0)), "clearing the text removes the word");
    }

    /// Suggestions: dictionary words and the page's own words within edit distance 2, nearest
    /// first, at most 5, never the word itself.
    #[test]
    fn suggestions_are_near_dictionary_and_page_words() {
        let mut dictionary = BTreeSet::new();
        dictionary.insert("recognize".into());
        dictionary.insert("these".into());
        let page_words = vec![word("recognise", None), word("scanned", None), word("words", None), word("completely", None)];
        let sugg = suggestions("recgnise", &dictionary, &page_words);
        assert_eq!(sugg.first().map(String::as_str), Some("recognise"), "distance 1 beats distance 2");
        assert!(sugg.contains(&"recognize".to_string()), "distance 2 candidates are in");
        assert!(!sugg.contains(&"recgnise".to_string()), "never the word itself");
        assert!(sugg.len() <= 5);
        assert!(suggestions("xyzzyx", &dictionary, &page_words).is_empty(), "nothing within distance 2");
        assert!(edit_distance_within_2("kitten", "sitting", 2).is_none(), "distance 3 is out");
        assert_eq!(edit_distance_within_2("kitten", "mitten", 2), Some(1));
    }

    /// The page list's data: a page marked verified, a skipped page, and the verified
    /// fraction the info strip shows.
    #[test]
    fn verified_fraction_and_word_count_follow_the_review() {
        let mut s = Session::new();
        let (_, mut v) = loaded(&mut s, &[Some(95.0), Some(95.0), Some(95.0)]);
        assert!((v.verified_fraction() - 0.0).abs() < f32::EPSILON);
        v.set_word_verified((0, 0), true);
        assert!((v.verified_fraction() - 1.0 / 3.0).abs() < 0.01);
        v.set_page_verified(0, true);
        assert!((v.verified_fraction() - 1.0).abs() < f32::EPSILON, "a verified page vouches for all its words");
        assert_eq!(v.word_count(), 3);
        v.set_removed((0, 1), true);
        assert_eq!(v.word_count(), 2, "removed words are not counted");
        // Reject clears the words, the session, the selection and the flags; back to Idle.
        assert!(v.reject());
        assert_eq!(v.phase, Phase::Idle);
        assert!(v.pages.is_empty() && v.selected.is_none() && v.review.verified_pages.is_empty());
        assert!(!v.reject(), "Reject outside ReviewReady is ignored");
    }

    /// The suspects' count the info strip shows: live Low-band words that are not in the
    /// dictionary.
    #[test]
    fn suspect_count_follows_bands_and_dictionary() {
        let mut s = Session::new();
        let (_, mut v) = loaded(&mut s, &[Some(95.0), Some(40.0), Some(69.0)]);
        assert_eq!(v.suspect_count(), 2, "69 is Low, 40 is Low, 95 is High");
        v.add_to_dictionary("w", "en");
        assert_eq!(v.suspect_count(), 0, "dictionary words do not ask for review");
    }

    #[test]
    fn dictionary_uses_recognition_language_instead_of_interface_locale() {
        let mut session = Session::new();
        let (_, mut v) = loaded(&mut session, &[Some(40.0)]);
        let locale = crate::i18n::current().code();
        let language = if locale == "en" { "fr" } else { "en" };
        let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-dictionary-language-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("ocr-dictionary-{locale}.txt")), "wrong\n").unwrap();
        let path = dir.join(format!("ocr-dictionary-{language}.txt"));
        std::fs::write(&path, "recognized\n").unwrap();
        v.dictionary_dir = Some(dir.clone());
        v.dictionary_language = language.into();
        v.load_dictionary_pref();
        assert!(v.review.dictionary.contains("recognized"));
        assert!(!v.review.dictionary.contains("wrong"));
        assert!(v.add_to_dictionary("Added", language));
        assert!(std::fs::read_to_string(path).unwrap().contains("added\n"));
        assert_eq!(std::fs::read_to_string(dir.join(format!("ocr-dictionary-{locale}.txt"))).unwrap(), "wrong\n");
    }

    #[test]
    fn dictionary_limit_and_normalization_failures_preserve_file_and_review() {
        let mut session = Session::new();
        let (_, mut v) = loaded(&mut session, &[Some(40.0)]);
        let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-dictionary-limits-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ocr-dictionary-en.txt");
        let full: String = (0..65536).map(|i| format!("word{i}\n")).collect();
        std::fs::write(&path, &full).unwrap();
        v.dictionary_dir = Some(dir);
        v.load_dictionary_pref();
        let before = v.review.dictionary.clone();
        assert!(!v.add_to_dictionary("extra", "en"));
        assert!(v.message.contains("65536"), "{}", v.message);
        assert_eq!(v.review.dictionary, before);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), full);
        assert!(!v.add_to_dictionary(&"İ".repeat(512), "en"));
        assert!(v.message.contains("normalized"), "{}", v.message);
        assert_eq!(v.review.dictionary, before);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), full);
        let full_bytes: String = (0..8192).map(|i| format!("{i:05}{}\n", "a".repeat(506))).collect();
        assert_eq!(full_bytes.len(), pdfcraft_engine::ocr::dictionary::MAX_BYTES);
        std::fs::write(&path, &full_bytes).unwrap();
        v.load_dictionary_pref();
        let before = v.review.dictionary.clone();
        assert!(!v.add_to_dictionary("extra", "en"));
        assert!(v.message.contains("4 MiB"), "{}", v.message);
        assert_eq!(v.review.dictionary, before);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), full_bytes);
        // A corrupt file is not silently treated as a fresh, empty dictionary.
        std::fs::write(&path, [0xff]).unwrap();
        v.load_dictionary_pref();
        assert_eq!(v.review.dictionary, before);
        assert!(v.message.contains("OCR dictionary"));
    }

    #[test]
    fn dictionary_undo_persists_and_failed_undo_keeps_history() {
        let mut session = Session::new();
        let (_, mut v) = loaded(&mut session, &[Some(40.0)]);
        let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-dictionary-undo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ocr-dictionary-en.txt");
        std::fs::write(&path, "original\n").unwrap();
        v.dictionary_dir = Some(dir);
        v.load_dictionary_pref();
        std::fs::write(&path, "original\nexternal\n").unwrap();
        assert!(!v.add_to_dictionary("external", "en"), "an externally added word creates no undo entry");
        assert!(v.review.dictionary.contains("external"));
        assert!(v.add_to_dictionary("temporary", "en"));
        assert!(v.undo());
        assert!(!std::fs::read_to_string(&path).unwrap().contains("temporary"));
        assert!(v.add_to_dictionary("later", "en"));
        assert!(!v.review.dictionary.contains("temporary"));
        let before = v.review.dictionary.clone();
        std::fs::write(&path, [0xff]).unwrap();
        assert!(!v.undo());
        assert_eq!(v.review.dictionary, before);
        assert_eq!(std::fs::read(&path).unwrap(), [0xff]);
        std::fs::write(&path, "original\nexternal\nlater\n").unwrap();
        assert!(v.undo(), "failed undo keeps its operation for retry");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external\noriginal\n");
        assert_eq!(v.review.dictionary, std::collections::BTreeSet::from(["external".into(), "original".into()]));
    }

    #[test]
    fn rereading_keeps_captured_language_when_the_draft_changes() {
        let mut session = Session::new();
        let (_, mut v) = loaded(&mut session, &[Some(40.0)]);
        v.dictionary_language = "ja".into();
        let draft = super::super::ocr_ui::OcrDraft { language: "fr".into(), ..Default::default() };
        let settings = v.reread_settings(&draft);
        assert_eq!(settings.language, "ja");
        assert_eq!(settings.dpi, draft.settings().dpi);
        assert!(dictionary_path(std::path::Path::new("prefs"), "../outside").is_err());
    }

    #[test]
    fn finishing_or_rejecting_a_review_keeps_dictionary_configuration() {
        let mut session = Session::new();
        let (_, mut v) = loaded(&mut session, &[Some(40.0)]);
        let dir = PathBuf::from("configured-dictionaries");
        v.dictionary_dir = Some(dir.clone());
        assert!(v.reject());
        assert_eq!(v.dictionary_dir.as_ref(), Some(&dir));
        v.finish();
        assert_eq!(v.dictionary_dir.as_ref(), Some(&dir));
    }

    /// After a run, the source pane shows a page with recognized words: a page skipped because
    /// it already has text is passed over, unless every page was skipped.
    #[test]
    fn a_finished_run_shows_a_read_page_over_a_skipped_one() {
        let skipped = |page| OcrPage { already_on_page: 0, page, words: Vec::new(), skipped: Some("has text".into()), notes: Vec::new() };
        let read = |page| OcrPage { already_on_page: 0, page, words: vec![word("w", Some(95.0))], skipped: None, notes: Vec::new() };
        let mut v = OcrVerify { current: 0, ..Default::default() };
        v.finished(vec![skipped(0), read(1), read(2)], false);
        assert_eq!(v.current, 1, "the first page read");
        let mut v = OcrVerify { current: 2, ..Default::default() };
        v.finished(vec![skipped(0), read(1), read(2)], false);
        assert_eq!(v.current, 2, "the current page, when it was read");
        let mut v = OcrVerify { current: 1, ..Default::default() };
        v.finished(vec![skipped(0), skipped(1)], false);
        assert_eq!(v.current, 1, "every page skipped: the current page stays");
        let mut v = OcrVerify { current: 5, ..Default::default() };
        v.finished(vec![skipped(0), skipped(1)], false);
        assert_eq!(v.current, 0, "or the reading's first page, when it lacks the current one");
    }

    /// A current-page run cancelled before its page was read keeps showing that page.
    #[test]
    fn a_run_that_read_no_pages_keeps_its_page() {
        let mut v = OcrVerify { current: 2, ..Default::default() };
        v.finished(Vec::new(), true);
        assert_eq!(v.current, 2);
        assert_eq!(v.phase, Phase::ReviewReady);
    }

    /// Closing a document under OCR review, from another tab, Close all or its own tab, keeps it
    /// open and brings its review forward; other documents close as usual, and once the review
    /// is rejected the document closes.
    #[test]
    fn closing_the_reviewed_document_brings_its_review_forward() {
        let mut app = PdfCraftApp::for_unit_tests();
        let bytes = Session::new().create_from_text("t", "Recognize these scanned words").expect("fixture").to_vec();
        for name in ["a.pdf", "b.pdf", "c.pdf"] {
            app.open_bytes(&egui::Context::default(), name, None, bytes.clone()).expect("fixture");
        }
        let a = app.views.first().expect("a is open").id;
        let job = app.session.ocr_job(a, &[0], OcrSettings::default()).expect("fixture");
        let page = OcrPage { already_on_page: 0, page: 0, words: vec![word("w", Some(95.0))], skipped: None, notes: Vec::new() };
        app.ocr_verify.load(a, job.guard.clone(), vec![page]);
        assert!(app.ocr_verify.correct((0, 0), "Edited"));
        app.active = Some(2);
        app.request_close_tab(&egui::Context::default(), 0);
        assert_eq!(app.views.len(), 3, "the reviewed document stays open");
        assert_eq!(app.active, Some(0), "and comes to the front");
        assert!(app.ocr_verify.open, "with its review");
        assert_eq!(app.ocr_verify.text_of((0, 0)), Some("Edited"), "as it was");
        assert!(!said(&mut app).is_empty(), "a notice says to accept or reject it first");
        app.ocr_verify.open = false;
        app.request_close_tab(&egui::Context::default(), 2);
        assert_eq!(app.views.len(), 2, "another document closes as usual");
        app.active = Some(1);
        app.close_all(&egui::Context::default());
        assert_eq!(app.views.len(), 2, "Close all closes nothing while the review is open");
        assert_eq!(app.active, Some(0));
        assert!(app.ocr_verify.reject());
        app.request_close_tab(&egui::Context::default(), 0);
        assert_eq!(app.views.len(), 1, "after Reject the document closes");
        assert!(app.views.iter().all(|v| v.id != a));
    }

    /// Three open documents with a review of the first one, a word of it corrected.
    fn reviewing_the_first_of_three() -> (PdfCraftApp, DocId) {
        let mut app = PdfCraftApp::for_unit_tests();
        let bytes = Session::new().create_from_text("t", "Recognize these scanned words").expect("fixture").to_vec();
        for name in ["a.pdf", "b.pdf", "c.pdf"] {
            app.open_bytes(&egui::Context::default(), name, None, bytes.clone()).expect("fixture");
        }
        let a = app.views.first().expect("a is open").id;
        let job = app.session.ocr_job(a, &[0], OcrSettings::default()).expect("fixture");
        let page = OcrPage { already_on_page: 0, page: 0, words: vec![word("w", Some(95.0))], skipped: None, notes: Vec::new() };
        app.ocr_verify.load(a, job.guard.clone(), vec![page]);
        assert!(app.ocr_verify.correct((0, 0), "Edited"));
        (app, a)
    }

    /// Run OCR on another document while a review is held leaves the review alone and says why.
    #[test]
    fn a_run_on_another_document_keeps_the_review() {
        let (mut app, a) = reviewing_the_first_of_three();
        app.active = Some(1);
        with_state(&mut app, |v, app| v.start_run(app));
        assert!(app.ocr_verify.message.contains("other document"), "{}", app.ocr_verify.message);
        assert!(app.ocr_verify.holds_reading_of(a), "the review is still of the first document");
        assert_eq!(app.ocr_verify.phase, Phase::ReviewReady);
        assert_eq!(app.ocr_verify.text_of((0, 0)), Some("Edited"));
    }

    /// The embedding page hears an open review as work it would lose, as the reload warning does,
    /// and hears it go once the review is rejected.
    #[test]
    fn the_embedding_page_hears_an_open_review() {
        let (mut app, _) = reviewing_the_first_of_three();
        let reports = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = reports.clone();
        app.host_dirty = Some(Box::new(move |dirty| sink.lock().expect("not poisoned").push(dirty)));
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            app
        });
        h.run_steps(2);
        assert_eq!(reports.lock().expect("not poisoned").last(), Some(&true), "an open review is work at risk");
        assert!(h.state_mut().ocr_verify.reject());
        h.run_steps(2);
        assert_eq!(reports.lock().expect("not poisoned").last(), Some(&false), "rejected, nothing is at risk");
    }

    /// A run of one page shows that page when its reading lands, not page 1.
    #[test]
    fn a_finished_run_shows_a_page_it_read() {
        let mut v = OcrVerify { current: 0, ..Default::default() };
        let page = OcrPage { already_on_page: 0, page: 2, words: vec![word("w", Some(95.0))], skipped: None, notes: Vec::new() };
        v.finished(vec![page], false);
        assert_eq!(v.current, 2);
        assert!(v.page(v.current).is_some());
    }

    /// A failed Accept keeps the reading (Run is enabled again), and Reject still throws it
    /// away without a new run; a failed run that read nothing has nothing to reject.
    #[test]
    fn reject_throws_away_a_reading_whose_accept_failed() {
        let mut s = Session::new();
        let (id, mut v) = loaded(&mut s, &[Some(95.0)]);
        assert!(v.correct((0, 0), "Corrected"));
        s.apply(id, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).expect("fixture");
        assert!(v.accept(&mut s).is_err(), "the stale reading is refused");
        assert_eq!(v.phase, Phase::RecoverableError);
        assert!(v.holds_reading_of(id), "its document stays open");
        assert!(v.can_reject());
        assert!(v.reject());
        assert_eq!(v.phase, Phase::Idle);
        assert!(v.pages.is_empty());
        assert!(!v.holds_reading_of(id), "and can close");
        let mut failed = OcrVerify::default();
        failed.failed("no engine");
        assert!(!failed.can_reject());
        assert!(!failed.reject(), "a failed run that read nothing has nothing to reject");
    }

    /// Quitting, from Quit or by closing the window, while a reading is under review keeps the
    /// application open and brings the review forward, even when no document has unsaved
    /// changes; once the review is rejected, the application quits.
    #[test]
    fn quitting_brings_an_open_review_forward() {
        // PLAN: assumed `PdfCraftApp::close_requested(&mut self) -> CloseDecision` (pdf-shell-w15) and the shell's `CloseDecision` at this path.
        use craft::ui::shell::embed::CloseDecision;
        let mut app = PdfCraftApp::for_unit_tests();
        let bytes = Session::new().create_from_text("t", "Recognize these scanned words").expect("fixture").to_vec();
        for name in ["a.pdf", "b.pdf"] {
            app.open_bytes(&egui::Context::default(), name, None, bytes.clone()).expect("fixture");
        }
        let a = app.views[0].id;
        let job = app.session.ocr_job(a, &[0], OcrSettings::default()).expect("fixture");
        let page = OcrPage { already_on_page: 0, page: 0, words: vec![word("w", Some(95.0))], skipped: None, notes: Vec::new() };
        app.ocr_verify.load(a, job.guard.clone(), vec![page]);
        assert!(app.first_dirty().is_none(), "nothing is unsaved");
        app.active = Some(1);
        assert!(matches!(app.close_requested(&egui::Context::default()), CloseDecision::Hold), "the close is held");
        assert_eq!(app.active, Some(0), "the reviewed document comes to the front");
        assert!(app.ocr_verify.open, "with its review");
        assert!(!said(&mut app).is_empty(), "a notice says to accept or reject it first");
        assert_eq!(app.views.len(), 2, "nothing closed");
        assert!(app.ocr_verify.reject());
        assert!(matches!(app.close_requested(&egui::Context::default()), CloseDecision::Close), "after Reject, the close goes ahead");
    }
}
