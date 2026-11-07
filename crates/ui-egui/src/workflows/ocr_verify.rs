//! Scan & OCR ▸ Correct recognized text: the OCR Verify screen. A full-workspace surface with
//! four panes (page list, source page, recognized text, zoom + word inspector) where a
//! recognition is reviewed before it is applied: words are colored by confidence, corrected,
//! removed (and restored), sent to a per-language user dictionary, and navigated by their
//! uncertainty. Accept applies the review — as one undoable step on the open document
//! (searchable), or as a new `<name>_ocr.pdf` of visible text (editable). Reject throws the
//! reading away.
//!
//! The state machine is [`Phase`]: recognition runs on a worker thread ([`Phase::Running`], a
//! real Cancel keeps the pages already read), results land in [`Phase::ReviewReady`], failures
//! land in [`Phase::RecoverableError`] with the cause named and Run enabled again, and
//! Accept/Reject/Re-OCR are ignored outside ReviewReady instead of acting on nothing.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use pdfcraft_engine::ocr::{
    ConfidenceBand, EngineChoice, MergeStrategy, OcrGuard, OcrPage, OcrSettings, OutputMode, PlacedWord, band_for, clamp_region,
};
use pdfcraft_engine::{DocId, Session};

use super::ocr_ui;
use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// Where the screen stands. Run is enabled in Idle, ReviewReady and RecoverableError; the
/// review tools (Accept, Reject, navigation, the inspector, re-OCR) only in ReviewReady.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Running,
    ReviewReady,
    Saving,
    RecoverableError,
}

/// The four panes; Ctrl+Tab moves the focus ring.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pane {
    #[default]
    Pages,
    Source,
    Text,
    Inspector,
}

impl Pane {
    fn next(self) -> Pane {
        match self {
            Pane::Pages => Pane::Source,
            Pane::Source => Pane::Text,
            Pane::Text => Pane::Inspector,
            Pane::Inspector => Pane::Pages,
        }
    }
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

/// The page preview render: cached until the document or page changes.
pub struct Preview {
    pub doc: DocId,
    pub page: usize,
    /// Pixels per point the page was rendered at.
    pub scale: f32,
    pub width: u32,
    pub height: u32,
    pub(crate) texture: Option<egui::TextureHandle>,
}

/// A page or region re-read in flight ([`OcrVerify::start_reread`]): the page being read
/// again (and the drawn region, in view-space points, for a region re-read), with the
/// worker's progress.
pub struct Reread {
    pub page: usize,
    pub region: Option<[f32; 4]>,
    pub(crate) progress: Arc<Mutex<RereadProgress>>,
}

/// A re-read's progress: the replacement words once the worker is done, and a cancel request.
/// A cancelled re-read is discarded when it lands: the page's words stay as they were.
#[derive(Default)]
pub struct RereadProgress {
    pub cancel: bool,
    pub(crate) result: Option<Result<Vec<PlacedWord>, String>>,
}

/// The OCR Verify screen. One instance per app; lives across frames.
pub struct OcrVerify {
    pub open: bool,
    pub phase: Phase,
    /// The document being reviewed; `None` until a run starts.
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
    /// Where a source-pane drag started (the live region rectangle is drawn from here).
    pub(crate) drag: Option<egui::Pos2>,
    pub focus: Pane,
    /// The status line: where the run stands, why something was refused.
    pub message: String,
    cancelled: bool,
    pub progress: Option<Arc<Mutex<ocr_ui::OcrProgress>>>,
    /// A page or region re-read running on a worker thread: the screen stays live (the review
    /// edits and navigation work) and the reading is applied by [`Self::poll_reread`].
    pub reread: Option<Reread>,
    /// The correction text field's current contents.
    pub correction: String,
    /// Where the per-language dictionary files live (`None`: off, e.g. the web build).
    pub dictionary_dir: Option<PathBuf>,
    /// Language of the captured recognition, independent of the interface locale.
    pub dictionary_language: String,
    pub(crate) preview: Option<Preview>,
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
            drag: None,
            focus: Pane::Pages,
            message: String::new(),
            cancelled: false,
            progress: None,
            reread: None,
            correction: String::new(),
            dictionary_dir: None,
            dictionary_language: "en".into(),
            preview: None,
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
        self.current = self.current.min(self.pages.len().saturating_sub(1));
        self.phase = Phase::ReviewReady;
        self.load_dictionary_pref();
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
            crate::editing::write_atomically(&path.to_string_lossy(), text.as_bytes()).map_err(|e| e.to_string())?;
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
    /// Ignored outside ReviewReady.
    pub fn reject(&mut self) -> bool {
        if !self.in_review() {
            return false;
        }
        *self = OcrVerify { open: self.open, dictionary_dir: self.dictionary_dir.take(), ..Default::default() };
        true
    }

    /// The state after a successful accept (or a saved editable copy): the review is done.
    pub(crate) fn finish(&mut self) {
        *self = OcrVerify { open: self.open, phase: Phase::Idle, doc: self.doc, dictionary_dir: self.dictionary_dir.take(), ..Default::default() };
    }

    /// Ask the running recognition to stop. The pages already read are kept; the rest of the
    /// document is not read.
    pub fn cancel(&mut self) {
        if self.phase == Phase::Running {
            self.cancelled = true;
            if let Some(p) = &self.progress
                && let Ok(mut s) = p.lock()
            {
                s.cancel = true;
            }
        }
    }

    pub fn can_run(&self) -> bool {
        matches!(self.phase, Phase::Idle | Phase::ReviewReady | Phase::RecoverableError)
    }

    /// Start re-reading a whole page (`region` `None`) or a drawn region on a WORKER THREAD:
    /// the UI thread only builds the job and refuses an empty or out-of-page region BEFORE
    /// anything runs — the render, the recognition and the engine probe (a `--list-langs`
    /// process for Tesseract) happen off it, with the status line and a Cancel while they run.
    /// The reading is applied by [`Self::poll_reread`] when it lands.
    pub fn start_reread(&mut self, app: &mut PdfCraftApp, page: usize, region: Option<[f32; 4]>) {
        if self.phase != Phase::ReviewReady || self.reread.is_some() {
            return; // one re-read at a time, and never under a run
        }
        let Some(doc) = self.doc else { return };
        let settings = self.reread_settings(&app.ocr_draft);
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
        let progress = Arc::new(Mutex::new(RereadProgress::default()));
        self.message = if is_region { tl!("Re-reading the region…").to_string() } else { tl!("Re-reading the page…").to_string() };
        let p = progress.clone();
        let work = move || {
            let result = pdfcraft_engine::ocr::recognizers(&job.settings).and_then(|r| {
                let found = job.run(&r, |_, _| !p.lock().map(|s| s.cancel).unwrap_or(false));
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
            if let Ok(mut s) = p.lock() {
                s.result = Some(result);
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        if app.run_inline {
            work();
        } else {
            std::thread::Builder::new().name("pdfcraft-ocr-reread".into()).spawn(work).ok();
        }
        #[cfg(target_arch = "wasm32")]
        work();
        self.reread = Some(Reread { page, region, progress });
    }

    /// Ask a running re-read to stop and be discarded: when the worker lands, its reading is
    /// thrown away and the page's words stay as they were.
    pub fn cancel_reread(&mut self) {
        if let Some(r) = &self.reread
            && let Ok(mut s) = r.progress.lock()
        {
            s.cancel = true;
        }
    }

    /// Apply a finished re-read, once the worker is done: a page re-read replaces the page's
    /// words, a region re-read the words inside the drawn rectangle. A cancel discards the
    /// reading, and so does a refusal — a document changed since the review began no longer
    /// matches the page the words were read from. Called every frame while a re-read is out.
    pub(crate) fn poll_reread(&mut self, app: &mut PdfCraftApp) {
        if self.phase != Phase::ReviewReady {
            return;
        }
        let Some(reread) = &self.reread else { return };
        let (cancelled, result) = match reread.progress.lock() {
            Ok(mut s) => (s.cancel, s.result.take()),
            Err(_) => return,
        };
        let Some(result) = result else {
            // Still reading: keep the status line live and ask for another frame.
            if let Some(ctx) = &app.ctx {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
            return;
        };
        let (page, region) = (reread.page, reread.region);
        self.reread = None;
        if cancelled {
            self.message = tl!("The re-read was cancelled; the words are unchanged").to_string();
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
        self.current = self.current.min(self.pages.len().saturating_sub(1));
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
        self.progress = None;
    }

    pub(crate) fn start_run(&mut self, app: &mut PdfCraftApp) {
        let Some((_, id)) = app.active_ids() else { return };
        if !self.can_run() || self.phase == Phase::Running {
            return;
        }
        if self.reread.is_some() {
            return; // the page re-read in flight replaces these words; one worker at a time
        }
        if let Some(why) = app.session.get(id).and_then(|d| d.read_only_reason.clone()) {
            self.message = why;
            self.phase = Phase::RecoverableError;
            return;
        }
        let d = &app.ocr_draft;
        let pages: Vec<usize> = match d.pages {
            super::ocr_ui::OcrPages::All => Vec::new(),
            super::ocr_ui::OcrPages::Current => vec![app.active.map_or(0, |i| app.views.get(i).map_or(0, |v| v.current))],
            super::ocr_ui::OcrPages::Range => (d.from.saturating_sub(1)..d.to).collect(),
        };
        let settings = d.settings();
        let Some(job) = app.session.ocr_job(id, &pages, settings.clone()) else { return };
        self.dictionary_language = job.settings.language.clone();
        let progress = Arc::new(Mutex::new(ocr_ui::OcrProgress { total: job.pages.len(), ..Default::default() }));
        self.doc = Some(id);
        self.guard = Some(job.guard.clone());
        self.current = 0;
        self.cancelled = false;
        self.phase = Phase::Running;
        self.message = String::new();
        let p = progress.clone();
        let work = move || {
            let result = pdfcraft_engine::ocr::recognizers(&settings).map(|r| {
                job.run(&r, |done, total| {
                    let Ok(mut s) = p.lock() else { return false };
                    s.done = done;
                    s.total = total;
                    !s.cancel
                })
            });
            if let Ok(mut s) = p.lock() {
                s.result = Some(result);
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        if app.run_inline {
            work();
        } else {
            std::thread::Builder::new().name("pdfcraft-ocr-verify".into()).spawn(work).ok();
        }
        #[cfg(target_arch = "wasm32")]
        work();
        self.progress = Some(progress);
        self.poll(app);
    }

    /// Take the worker's result, once it is done.
    pub(crate) fn poll(&mut self, app: &mut PdfCraftApp) {
        if self.phase != Phase::Running {
            return;
        }
        let Some(progress) = self.progress.clone() else { return };
        let result = progress.lock().ok().and_then(|mut s| s.result.take());
        let Some(result) = result else {
            // Still running: refresh the status line and ask for another frame.
            if let Ok(s) = progress.lock() {
                self.message = crate::i18n::fmt(
                    tl!("Recognizing text… page {d} of {t}"),
                    &[("d", &(s.done + 1).min(s.total.max(1)).to_string()), ("t", &s.total.max(1).to_string())],
                );
            }
            if let Some(ctx) = &app.ctx {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
            return;
        };
        self.progress = None;
        match result {
            Ok(found) => self.finished(found, self.cancelled),
            Err(why) => self.failed(&why),
        }
    }
}

/// What a frame's controls asked for, applied after the drawing (the panes read `app` while
/// the controls write it, so nothing mutates through an open borrow).
#[derive(Default)]
struct Intents {
    run: bool,
    cancel: bool,
    cancel_reread: bool,
    accept: bool,
    reject: bool,
    exit: bool,
    undo: bool,
    next_uncertain: bool,
    prev_uncertain: bool,
    toggle_page_verified: bool,
    reocr_region: bool,
    reocr_page: bool,
    accept_page: bool,
    reject_page: bool,
    cycle_focus: bool,
    select_page: Option<usize>,
    select_word: Option<(usize, usize)>,
    correct: Option<String>,
    set_removed: Option<bool>,
    set_word_verified: Option<bool>,
    dictionary_add: bool,
}

/// Band colours: High green, Medium yellow, Low red (the fill at ~20% opacity, the outline
/// at ~60%; the legend carries the same thresholds as text).
fn band_color(band: ConfidenceBand) -> egui::Color32 {
    match band {
        ConfidenceBand::High => egui::Color32::from_rgb(34, 139, 74),
        ConfidenceBand::Medium => egui::Color32::from_rgb(196, 148, 20),
        ConfidenceBand::Low => egui::Color32::from_rgb(204, 61, 47),
    }
}

/// The whole-window OCR Verify screen (Scan & OCR ▸ Correct recognized text).
pub(crate) fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    use egui::{Key, Modifiers};
    let ctx = ui.ctx().clone();
    let t = Tokens::get(&ctx);
    let wants_text = ctx.egui_wants_keyboard_input();
    let (next_u, prev_u, toggle_verified, cycle, undo_key) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::F8),
            i.consume_key(Modifiers::SHIFT, Key::F8),
            (!wants_text && i.consume_key(Modifiers::COMMAND, Key::T)),
            (!wants_text && i.consume_key(Modifiers::COMMAND, Key::Tab)),
            (!wants_text && i.consume_key(Modifiers::COMMAND, Key::Z)),
        )
    });

    let mut n = Intents {
        next_uncertain: next_u,
        prev_uncertain: prev_u,
        toggle_page_verified: toggle_verified,
        cycle_focus: cycle,
        undo: undo_key,
        ..Default::default()
    };

    egui::Panel::left("ocr-verify-pages").resizable(false).default_size(190.0).frame(egui::Frame::new().fill(t.panel).inner_margin(8.0)).show(
        ui,
        |ui| {
            pane_title(ui, &t, tl!("Pages"), app.ocr_verify.focus == Pane::Pages);
            pages_pane(app, ui, &mut n);
        },
    );

    egui::Panel::right("ocr-verify-right").resizable(false).default_size(340.0).frame(egui::Frame::new().fill(t.panel).inner_margin(8.0)).show(
        ui,
        |ui| {
            pane_title(ui, &t, tl!("Recognized text"), app.ocr_verify.focus == Pane::Text);
            text_pane(app, ui, &t, &mut n);
            ui.add_space(10.0);
            pane_title(ui, &t, tl!("Word"), app.ocr_verify.focus == Pane::Inspector);
            inspector(app, ui, &t, &mut n);
        },
    );

    egui::CentralPanel::default().frame(egui::Frame::new().fill(t.pasteboard).inner_margin(egui::Margin::same(8))).show(ui, |ui| {
        toolbar(app, ui, &mut n);
        ui.add_space(4.0);
        info_strip(app, ui, &t);
        ui.add_space(6.0);
        pane_title(ui, &t, tl!("Source page"), app.ocr_verify.focus == Pane::Source);
        source_pane(app, ui, &t, &mut n);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&app.ocr_verify.message).small().color(if app.ocr_verify.phase == Phase::RecoverableError {
                t.accent
            } else {
                t.text_muted
            }));
        });
    });

    if n.cycle_focus {
        app.ocr_verify.focus = app.ocr_verify.focus.next();
    }
    apply(app, n);
}

/// A pane's caption; the focused pane's name is underlined (Ctrl+Tab moves the focus).
fn pane_title(ui: &mut egui::Ui, t: &Tokens, name: &str, focused: bool) {
    ui.add_space(2.0);
    let font = theme::semibold(11.5);
    let w = ui.fonts_mut(|f| f.layout_no_wrap(name.to_owned(), font.clone(), t.text).size().x);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 14.0), egui::Sense::hover());
    ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, name.to_uppercase(), font, t.text_muted);
    if focused {
        let r = egui::Rect::from_min_max(rect.left_bottom() + egui::vec2(0.0, 1.0), rect.right_bottom() + egui::vec2(0.0, 2.0));
        ui.painter().rect_filled(r, egui::CornerRadius::same(1), t.accent);
    }
    ui.add_space(4.0);
}

/// Toolbar: every control is live — the choices feed the next run (and are persisted), Run
/// starts a recognition, Accept/Reject/navigate/verify only act in ReviewReady.
fn toolbar(app: &mut PdfCraftApp, ui: &mut egui::Ui, n: &mut Intents) {
    let v = &app.ocr_verify;
    let d = &mut app.ocr_draft;
    let review = v.phase == Phase::ReviewReady;
    let run_enabled = v.can_run() && v.reread.is_none();
    let accept_enabled = review && v.word_count() + v.review.applied_pages.len() > 0;

    // Row 1: the settings (each written back into the persisted draft).
    ui.horizontal_wrapped(|ui| {
        craft_ui::ComboBox::from_id_salt("ocr-verify-language").width(150.0).selected_text(language_name(&d.language)).show_ui(ui, |ui| {
            for (code, name) in pdfcraft_engine::ocr::LANGUAGES {
                ui.selectable_value(&mut d.language, (*code).to_string(), *name);
            }
        });
        engine_combo(ui, &mut d.engine, &mut d.strategy);
        strategy_combo(ui, &mut d.strategy);
        output_combo(ui, &mut d.output_mode);
        for (flag, label, why) in [
            (&mut d.auto_rotate, tl!("Auto-rotate"), tl!("Turns pages scanned sideways or upside down upright before reading")),
            (&mut d.deskew, tl!("Deskew"), tl!("Straightens skewed text lines before reading")),
            (&mut d.denoise, tl!("Denoise"), tl!("A 3×3 median filter against scan speckle")),
            (&mut d.binarize, tl!("Binarize"), tl!("Sauvola binarization: ink to black, paper to white")),
        ] {
            ui.add(crate::widgets::checkbox(flag, label)).on_hover_text(format!("{label} \u{2014} {why}"));
        }
    });

    // Row 2: scope and actions.
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        crate::widgets::radio_value(ui, &mut d.pages, super::ocr_ui::OcrPages::All, tl!("All pages"));
        crate::widgets::radio_value(ui, &mut d.pages, super::ocr_ui::OcrPages::Current, tl!("Current page"));
        crate::widgets::radio_value(ui, &mut d.pages, super::ocr_ui::OcrPages::Range, tl!("From"));
        let on = d.pages == super::ocr_ui::OcrPages::Range;
        ui.add_enabled(on, egui::DragValue::new(&mut d.from).range(1..=9999));
        ui.label(tl!("to"));
        ui.add_enabled(on, egui::DragValue::new(&mut d.to).range(1..=9999));

        ui.separator();
        if ui.add_enabled_ui(run_enabled, |ui| widgets::pill_button(ui, tl!("Run OCR"), true)).inner.clicked() {
            n.run = true;
        }
        if v.phase == Phase::Running && widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            n.cancel = true;
        }
        if v.reread.is_some() && widgets::pill_button(ui, tl!("Cancel re-read"), false).clicked() {
            n.cancel_reread = true;
        }
        if ui.add_enabled_ui(accept_enabled, |ui| widgets::pill_button(ui, tl!("Accept"), false)).inner.clicked() {
            n.accept = true;
        }
        if ui.add_enabled_ui(review, |ui| widgets::pill_button(ui, tl!("Reject"), false)).inner.clicked() {
            n.reject = true;
        }
        ui.separator();
        let page_done = review && v.review.verified_pages.contains(&v.current);
        let mut verified = page_done;
        if ui
            .add_enabled(review, crate::widgets::checkbox(&mut verified, tl!("Page verified")))
            .on_hover_text(tl!("Mark this page as reviewed (Ctrl+T)"))
            .changed()
        {
            n.toggle_page_verified = true;
        }
        if ui
            .add_enabled_ui(review, |ui| widgets::ghost_button(ui, "chevron-right", tl!("Next uncertain")))
            .inner
            .on_hover_text(tl!("F8: next word to review; Shift+F8 goes back"))
            .clicked()
        {
            n.next_uncertain = true;
        }
        if ui
            .add_enabled_ui(review, |ui| widgets::ghost_button(ui, "chevron-left", tl!("Previous uncertain")))
            .inner
            .on_hover_text(tl!("Shift+F8: the previous word to review"))
            .clicked()
        {
            n.prev_uncertain = true;
        }
        if widgets::ghost_button(ui, "x", tl!("Exit")).clicked() {
            n.exit = true;
        }
    });
}

/// A language code's dropdown text: its name and its code ("English (en)").
fn language_name(code: &str) -> String {
    pdfcraft_engine::ocr::LANGUAGES.iter().find(|l| l.0 == code).map_or_else(|| code.to_owned(), |l| format!("{} ({})", l.1, l.0))
}

/// The Engine dropdown's text.
fn engine_label(engine: EngineChoice) -> &'static str {
    match engine {
        EngineChoice::Auto => tl!("Automatic"),
        EngineChoice::Ocrs => "ocrs",
        EngineChoice::Tesseract => "Tesseract",
    }
}

/// The Strategy dropdown's text.
fn strategy_label(strategy: MergeStrategy) -> &'static str {
    match strategy {
        MergeStrategy::PrimaryOnly => tl!("Primary only"),
        MergeStrategy::ConfidenceWeighted => tl!("Confidence weighted"),
        MergeStrategy::RoverVote => tl!("ROVER vote"),
    }
}

/// The Engine dropdown: Automatic, ocrs, Tesseract. An engine that cannot run is disabled and
/// says why (a tooltip names the missing models or program). The choice feeds the next run.
/// Choosing Automatic while the strategy is "Primary only" (which would never run the second
/// engine) moves the strategy to Automatic's own default, Confidence weighted.
fn engine_combo(ui: &mut egui::Ui, engine: &mut EngineChoice, strategy: &mut MergeStrategy) {
    ocr_ui::keep_asking_while_probing(ui.ctx());
    craft_ui::ComboBox::from_id_salt("ocr-verify-engine").width(120.0).selected_text(engine_label(*engine)).show_ui(ui, |ui| {
        for (choice, label) in [(EngineChoice::Auto, tl!("Automatic")), (EngineChoice::Ocrs, "ocrs"), (EngineChoice::Tesseract, "Tesseract")] {
            let why = pdfcraft_engine::ocr::engine_unavailable_reason(choice);
            let resp = ui.add_enabled(why.is_none(), egui::Button::selectable(*engine == choice, label));
            let clicked = resp.clicked();
            match why {
                Some(reason) => {
                    resp.on_disabled_hover_text(crate::i18n::t(&reason));
                }
                None => {
                    resp.on_hover_text(match choice {
                        EngineChoice::Auto => tl!("Both engines, combined as the Strategy says"),
                        EngineChoice::Ocrs => tl!("The built-in engine (pure Rust)"),
                        EngineChoice::Tesseract => tl!("The tesseract program, run as an external process"),
                    });
                }
            }
            if clicked {
                *engine = choice;
                if choice == EngineChoice::Auto && *strategy == MergeStrategy::PrimaryOnly {
                    *strategy = choice.default_strategy();
                }
            }
        }
    });
}

/// The Strategy dropdown: how two engines' readings combine. With a single named engine the
/// ensemble strategies still run — they just degrade to the primary reading (the tooltip says
/// so), because only one engine was asked to read.
fn strategy_combo(ui: &mut egui::Ui, strategy: &mut MergeStrategy) {
    craft_ui::ComboBox::from_id_salt("ocr-verify-strategy").width(150.0).selected_text(strategy_label(*strategy)).show_ui(ui, |ui| {
        for (s, label, hint) in [
            (MergeStrategy::PrimaryOnly, tl!("Primary only"), tl!("Use the chosen engine's reading alone")),
            (MergeStrategy::ConfidenceWeighted, tl!("Confidence weighted"), tl!("Add a second engine's reading when the first is unsure")),
            (MergeStrategy::RoverVote, tl!("ROVER vote"), tl!("Always read with both engines and merge the words")),
        ] {
            let resp = ui.add(egui::Button::selectable(*strategy == s, label)).on_hover_text(hint);
            if resp.clicked() {
                *strategy = s;
            }
        }
    });
}

/// The Output mode dropdown: what Accept produces, with tooltips that say what each mode
/// changes (searchable: the open page gains an invisible text layer; editable: a new file).
fn output_combo(ui: &mut egui::Ui, mode: &mut OutputMode) {
    let name = match *mode {
        OutputMode::Searchable => tl!("Searchable Image (Exact)"),
        OutputMode::EditableText => tl!("Editable text"),
    };
    craft_ui::ComboBox::from_id_salt("ocr-verify-output").width(180.0).selected_text(name).show_ui(ui, |ui| {
        let s = ui
            .selectable_label(*mode == OutputMode::Searchable, tl!("Searchable Image (Exact)"))
            .on_hover_text(tl!("Adds invisible text over each word; the page image is not changed"));
        if s.clicked() {
            *mode = OutputMode::Searchable;
        }
        let e = ui
            .selectable_label(*mode == OutputMode::EditableText, tl!("Editable text"))
            .on_hover_text(tl!("Writes a new document of visible text; the scan is not changed"));
        if e.clicked() {
            *mode = OutputMode::EditableText;
        }
    });
}

/// The info strip: where the review stands, in facts — page, average confidence, how many
/// words ask for review, the share verified, and the engine · strategy that read it.
fn info_strip(app: &PdfCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let v = &app.ocr_verify;
    let read = v.pages.iter().filter(|p| p.skipped.is_none()).count();
    let avg = v.page(v.current).and_then(pdfcraft_engine::ocr::OcrPage::mean_confidence);
    let cells = [
        (tl!("PAGE"), if v.pages.is_empty() { "—".into() } else { format!("{} / {}", v.current + 1, v.pages.len()) }),
        (tl!("AVG CONFIDENCE"), avg.map_or_else(|| "—".into(), |c| format!("{c:.0}%"))),
        (tl!("LOW-CONFIDENCE WORDS"), v.suspect_count().to_string()),
        (tl!("VERIFIED %"), format!("{}%", (v.verified_fraction() * 100.0).round() as i64)),
        (
            tl!("ENGINE · STRATEGY"),
            if read == 0 { "—".into() } else { format!("{} · {}", engine_label(app.ocr_draft.engine), strategy_label(app.ocr_draft.strategy)) },
        ),
    ];
    ui.horizontal(|ui| {
        for (k, val) in cells {
            ui.label(egui::RichText::new(k.to_string()).small().color(t.text_faint));
            ui.label(egui::RichText::new(val).small());
            ui.separator();
        }
    });
}

/// The page list: one row per read page, with its live word count and a flag for the words
/// that still ask for review. Clicking shows the page.
fn pages_pane(app: &mut PdfCraftApp, ui: &mut egui::Ui, n: &mut Intents) {
    let v = &app.ocr_verify;
    egui::ScrollArea::vertical().show(ui, |ui| {
        for pg in &v.pages {
            let p = pg.page;
            let (live, suspects) = (
                pg.words.iter().filter(|w| !w.text.trim().is_empty()).count(),
                pg.words.iter().filter(|w| !w.text.trim().is_empty() && pdfcraft_engine::ocr::is_suspect(w.confidence)).count(),
            );
            let marked = if pg.skipped.is_some() {
                format!(" · {}", tl!("skipped"))
            } else if v.review.verified_pages.contains(&p) {
                format!(" · {}", tl!("verified"))
            } else {
                String::new()
            };
            let text = format!("{} {}{} · {live}", tl!("Page"), p + 1, marked);
            let label = ui.selectable_label(p == v.current, text);
            let label = if suspects > 0 {
                label.on_hover_text(crate::i18n::fmt(tl!("{n} word(s) to review"), &[("n", &suspects.to_string())]))
            } else {
                label
            };
            if label.clicked() {
                n.select_page = Some(p);
            }
        }
    });
}

/// The SOURCE PAGE pane: the rendered page with one box per recognized word. A click selects
/// the word under the pointer; a drag draws the re-OCR region (in view-space points, clamped
/// to the page). Words are coloured by their confidence band; removed words are struck out and
/// dimmed, corrected words outlined. The context menu re-reads a region or the page, and
/// accepts or rejects the page.
fn source_pane(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens, n: &mut Intents) {
    let page = app.ocr_verify.current;
    let review = app.ocr_verify.phase == Phase::ReviewReady;
    ensure_preview(app, page);
    let Some(doc) = app.ocr_verify.doc else { return };
    let Some(info) = app.session.get(doc).and_then(|d| d.info.pages.get(page)).cloned() else { return };
    // Reserve the room the confidence legend (at the end of this pane) and the message row
    // (after it) need, so the page cannot push them off the panel.
    let size = (ui.available_size() - egui::vec2(0.0, 40.0)).max(egui::vec2(120.0, 160.0));
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    // The page fits the pane; the pasteboard shows around it.
    let s = (rect.width() / info.width.max(1.0)).min(rect.height() / info.height.max(1.0)).max(0.01);
    let size = egui::vec2(info.width * s, info.height * s);
    let img_rect = egui::Rect::from_min_size(rect.min + (rect.size() - size) * 0.5, size);
    painter.rect_filled(img_rect.expand(2.0), egui::CornerRadius::same(2), t.hover);
    if let Some(preview) = &app.ocr_verify.preview
        && let Some(tex) = &preview.texture
    {
        painter.image(tex.id(), img_rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
    }
    // A word's box: its user-space corners, through the page's rotation, into pane pixels.
    let to_pane = |p: [f64; 2]| {
        let v = info.user_to_view(p[0] as f32, p[1] as f32);
        egui::pos2(img_rect.left() + v[0] * s, img_rect.top() + v[1] * s)
    };
    let corners = |w: &PlacedWord| {
        [
            to_pane(w.origin),
            to_pane([w.origin[0] + w.across[0], w.origin[1] + w.across[1]]),
            to_pane([w.origin[0] + w.across[0] + w.up[0], w.origin[1] + w.across[1] + w.up[1]]),
            to_pane([w.origin[0] + w.up[0], w.origin[1] + w.up[1]]),
        ]
    };
    if let Some(pg) = app.ocr_verify.page(page) {
        for (wi, w) in pg.words.iter().enumerate() {
            let key = (pg.page, wi);
            let quad = corners(w);
            if app.ocr_verify.is_removed(key) {
                // Removed: dimmed, struck through — still there, still restorable.
                painter.add(egui::Shape::convex_polygon(quad.to_vec(), egui::Color32::from_black_alpha(32), egui::Stroke::NONE));
                painter.line_segment([quad[0], quad[2]], egui::Stroke::new(1.0, t.text_faint));
                continue;
            }
            if let Some(band) = w.confidence.map(band_for) {
                let (fill, _) = band_colors(band);
                painter.add(egui::Shape::convex_polygon(quad.to_vec(), fill, egui::Stroke::NONE));
            }
            let outline = if app.ocr_verify.selected == Some(key) {
                egui::Stroke::new(2.0, egui::Color32::from_rgb(43, 122, 236))
            } else if app.ocr_verify.is_corrected(key) {
                egui::Stroke::new(1.5, t.text)
            } else {
                // The band's outline; a word without a confidence reads as unmeasured.
                w.confidence.map(|c| band_colors(band_for(c)).1).unwrap_or_else(|| egui::Stroke::new(0.5, t.text_faint))
            };
            painter.add(egui::Shape::convex_polygon(quad.to_vec(), egui::Color32::TRANSPARENT, outline));
        }
    }
    // The chosen region and the one being drawn right now.
    let drag_now = resp.interact_pointer_pos().zip(app.ocr_verify.drag).map(|(p, start)| egui::Rect::from_two_pos(start, p).intersect(img_rect));
    let chosen = app.ocr_verify.region.and_then(|region| pane_region(region, &info, img_rect));
    for r in [chosen, drag_now].into_iter().flatten() {
        painter.rect_stroke(r, 0.0, egui::Stroke::new(1.5, t.accent), egui::StrokeKind::Middle);
    }
    // A click selects the word under the pointer; dragging away from a click draws the region.
    if resp.drag_started() {
        app.ocr_verify.drag = resp.interact_pointer_pos();
    }
    if resp.drag_stopped() {
        let (start, end) = (app.ocr_verify.drag.take(), resp.interact_pointer_pos());
        if let (Some(a), Some(b)) = (start, end)
            && egui::Rect::from_two_pos(a, b).intersect(img_rect).area() > 36.0
        {
            // A region: normalize it into view-space points, clamped to the page.
            let r = egui::Rect::from_two_pos(a, b).intersect(img_rect);
            let region = [
                (r.left() - img_rect.left()) / s,
                (r.top() - img_rect.top()) / s,
                (r.right() - img_rect.left()) / s,
                (r.bottom() - img_rect.top()) / s,
            ];
            app.ocr_verify.region = (region[2] > region[0] && region[3] > region[1]).then_some(region);
        }
    }
    if resp.clicked()
        && let Some(at) = resp.interact_pointer_pos()
        && let Some(pg) = app.ocr_verify.page(page)
    {
        n.select_word = pg
            .words
            .iter()
            .enumerate()
            .find(|(wi, w)| !app.ocr_verify.is_removed((pg.page, *wi)) && point_in_quad(at, &corners(w)))
            .map(|(wi, _)| (pg.page, wi));
    }
    resp.context_menu(|ui| {
        let rereading = app.ocr_verify.reread.is_some();
        if ui.add_enabled(review && !rereading && app.ocr_verify.region.is_some(), egui::Button::new(tl!("Re-OCR this region"))).clicked() {
            n.reocr_region = true;
            ui.close();
        }
        if ui.add_enabled(review && !rereading, egui::Button::new(tl!("Re-OCR entire page"))).clicked() {
            n.reocr_page = true;
            ui.close();
        }
        ui.separator();
        if ui.add_enabled(review, egui::Button::new(tl!("Accept entire page"))).clicked() {
            n.accept_page = true;
            ui.close();
        }
        if ui.add_enabled(review, egui::Button::new(tl!("Reject entire page"))).clicked() {
            n.reject_page = true;
            ui.close();
        }
        ui.separator();
        ui.label(egui::RichText::new(tl!("Click a word to inspect it; drag a rectangle to re-read part of the page")).small().color(t.text_faint));
    });
    legend(ui, t);
}

/// A region given in view-space points, back into pane pixels on `img_rect` (None when it
/// does not lie on the page). The region is already in view space (the drag stored it so), so
/// it is only scaled: mapping it through `user_to_view` again would mirror it vertically (and
/// misplace it on rotated or cropped pages).
fn pane_region(region: [f32; 4], info: &pdfcraft_render::PageInfo, img_rect: egui::Rect) -> Option<egui::Rect> {
    let scale = img_rect.width() / info.width.max(1.0);
    let to_pane = |v: [f32; 2]| egui::pos2(img_rect.left() + v[0] * scale, img_rect.top() + v[1] * scale);
    let a = to_pane([region[0], region[1]]);
    let b = to_pane([region[2], region[3]]);
    let r = egui::Rect::from_two_pos(a, b).intersect(img_rect);
    (r.area() > 1.0).then_some(r)
}

/// Point in a convex quad (corners in order): the pointer is on the same side of all four
/// edges.
fn point_in_quad(p: egui::Pos2, quad: &[egui::Pos2; 4]) -> bool {
    (0..4).all(|i| {
        let (a, b) = (quad[i], quad[(i + 1) % 4]);
        (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x) <= 0.0
    })
}

/// Band colours: the fill at ~20% opacity, the outline at ~60% — the legend names the bands
/// as text, so meaning never rests on colour alone.
fn band_colors(band: ConfidenceBand) -> (egui::Color32, egui::Stroke) {
    let c = band_color(band);
    (c.gamma_multiply(0.2), egui::Stroke::new(1.0, c.gamma_multiply(0.6)))
}

/// The overlay legend: the classifier's bands, with their thresholds spelled out.
fn legend(ui: &mut egui::Ui, t: &Tokens) {
    ui.add_space(2.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(tl!("CONFIDENCE")).small().strong());
        for (band, text) in legend_rows() {
            let (fill, stroke) = band_colors(band);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 2.0, fill);
            ui.painter().rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
            ui.label(egui::RichText::new(text).small().color(t.text_muted));
        }
    });
}

/// The legend's rows: the same [`ConfidenceBand`] values the overlay, the counts and the
/// navigation use, with the classifier's thresholds as text (a test holds the drawing to
/// these numbers).
pub fn legend_rows() -> [(ConfidenceBand, String); 3] {
    [
        (ConfidenceBand::High, tl!("HIGH (>= 90%)").to_string()),
        (ConfidenceBand::Medium, tl!("MEDIUM (70-89%)").to_string()),
        (ConfidenceBand::Low, tl!("LOW (< 70%)").to_string()),
    ]
}

/// Render (or re-render) the source pane's page preview when the shown page changed. One
/// synchronous render per page; the texture is cached until then.
fn ensure_preview(app: &mut PdfCraftApp, page: usize) {
    let Some(doc) = app.ocr_verify.doc else { return };
    if app.ocr_verify.preview.as_ref().is_some_and(|p| p.doc == doc && p.page == page && p.texture.is_some()) {
        return;
    }
    let Some(d) = app.session.get(doc) else { return };
    let Some(info) = d.info.pages.get(page) else { return };
    // Screen scale (the boxes are drawn on top of it); the recognition dpi is irrelevant here.
    let scale = 2.0_f32.min((900.0 / info.width.max(1.0)).min(1200.0 / info.height.max(1.0))).max(0.2);
    let config = pdfcraft_render::RenderConfig { password: d.password.as_deref().map(std::sync::Arc::from), ..Default::default() };
    let mut r = pdfcraft_render::PageRenderer::new(d.bytes.clone(), config);
    let shot = r.render(pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Pixels, scale, ..Default::default() });
    if shot.error.is_some() || shot.width == 0 || shot.height == 0 {
        return; // keep any earlier preview; a render failure names itself elsewhere
    }
    let scale = shot.width as f32 / info.width.max(1.0);
    let texture = app.ctx.as_ref().map(|ctx| {
        ctx.load_texture(
            "ocr-verify-page",
            egui::ColorImage::from_rgba_premultiplied([shot.width as usize, shot.height as usize], &shot.rgba),
            egui::TextureOptions::LINEAR,
        )
    });
    app.ocr_verify.preview = Some(Preview { doc, page, scale, width: shot.width, height: shot.height, texture });
}

/// The RECOGNIZED pane: the page's words as the engine read them — a preview, not
/// authoritative (the source pane's boxes and the inspector are where corrections happen).
/// Removed words show struck out; clicking a word selects it in the inspector.
fn text_pane(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens, n: &mut Intents) {
    let v = &app.ocr_verify;
    let Some(pg) = v.page(v.current) else { return };
    if let Some(why) = &pg.skipped {
        ui.label(egui::RichText::new(why.as_str()).small().color(t.text_muted));
        return;
    }
    if pg.words.is_empty() {
        ui.label(egui::RichText::new(tl!("OCR complete — no text recognized on this page.")).small().color(t.text_muted));
        return;
    }
    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (wi, w) in pg.words.iter().enumerate() {
                let key = (pg.page, wi);
                let mut label = egui::RichText::new(w.text.as_str()).small();
                if v.is_removed(key) {
                    label = label.strikethrough().color(t.text_faint);
                } else if v.is_corrected(key) {
                    label = label.underline();
                }
                if ui.selectable_label(v.selected == Some(key), label).clicked() {
                    n.select_word = Some(key);
                }
            }
        });
    });
}

/// The ZOOM + word inspector: a magnified crop of the selected word, its facts (id,
/// confidence, engine, review state), the correction field (Enter commits; clearing removes
/// the word; the original box never moves), Remove/Restore, + Dictionary, the per-word
/// Verified toggle, and suggestions within edit distance 2.
fn inspector(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens, n: &mut Intents) {
    let v = &app.ocr_verify;
    let Some(key) = v.selected else {
        ui.label(egui::RichText::new(tl!("Select a word on the page to inspect it")).small().color(t.text_muted));
        return;
    };
    if v.word(key).is_none() {
        ui.label(egui::RichText::new(tl!("The selected word is gone; choose another")).small().color(t.text_muted));
        return;
    }
    zoom_crop(app, ui, key, t);
    ui.add_space(4.0);
    let (id, conf, source) = {
        let w = v.word(key);
        (
            key.1 + 1,
            w.and_then(|w| w.confidence).map_or_else(|| "—".to_owned(), |c| format!("{c:.0}%")),
            w.map(|w| w.source.clone()).unwrap_or_default(),
        )
    };
    let state = if v.is_removed(key) {
        tl!("removed")
    } else if v.is_corrected(key) {
        tl!("corrected")
    } else if v.is_verified(key) {
        tl!("verified")
    } else {
        tl!("unreviewed")
    };
    ui.label(egui::RichText::new(format!("{id} · {conf} · {source} · {state}")).small().color(t.text_muted));
    ui.add_space(4.0);
    // The correction field: Enter commits, an empty commit removes, the box never moves.
    let (mut correction, verified) = (app.ocr_verify.correction.clone(), app.ocr_verify.is_verified(key));
    let field = ui.add(egui::TextEdit::singleline(&mut correction).hint_text(tl!("Corrected text (Enter commits)")));
    let commit = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    ui.horizontal(|ui| {
        if ui.add(egui::Button::new(tl!("Remove")).small()).clicked() {
            n.set_removed = Some(true);
        }
        if ui.add(egui::Button::new(tl!("Restore")).small()).clicked() {
            n.set_removed = Some(false);
        }
        if ui.add(egui::Button::new(tl!("+ Dictionary")).small()).clicked() {
            n.dictionary_add = true;
        }
        let mut on = verified;
        if ui.add(crate::widgets::checkbox(&mut on, tl!("Verified"))).changed() {
            n.set_word_verified = Some(on);
        }
        if ui.add(egui::Button::new(tl!("Undo")).small()).on_hover_text(tl!("Undo the last review change (Ctrl+Z)")).clicked() {
            n.undo = true;
        }
    });
    app.ocr_verify.correction = correction;
    if commit {
        n.correct = Some(app.ocr_verify.correction.clone());
    }
    // Suggestions: dictionary words and this page's own words within edit distance 2.
    let (word_text, page_words) = {
        let v = &app.ocr_verify;
        (v.text_of(key).unwrap_or_default().to_owned(), v.page(key.0).map(|p| p.words.clone()).unwrap_or_default())
    };
    let sugg = suggestions(&word_text, &app.ocr_verify.review.dictionary, &page_words);
    if !sugg.is_empty() {
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(tl!("Suggestions")).small().color(t.text_muted));
            for s in sugg {
                if ui.add(egui::Button::new(egui::RichText::new(s.clone()).small())).clicked() {
                    n.correct = Some(s);
                }
            }
        });
    }
}

/// The magnified crop of the selected word, drawn out of the page preview's texture.
fn zoom_crop(app: &PdfCraftApp, ui: &mut egui::Ui, key: (usize, usize), t: &Tokens) {
    let Some(preview) = &app.ocr_verify.preview else { return };
    let Some(tex) = &preview.texture else { return };
    let Some(w) = app.ocr_verify.word(key) else { return };
    let Some(info) = app.ocr_verify.doc.and_then(|doc| app.session.get(doc)).and_then(|d| d.info.pages.get(preview.page)).cloned() else {
        return;
    };
    // The word's box in preview pixels (user → view → × preview scale), plus a margin.
    let to_px = |p: [f64; 2]| {
        let v = info.user_to_view(p[0] as f32, p[1] as f32);
        [v[0] * preview.scale, v[1] * preview.scale]
    };
    let a = to_px(w.origin);
    let b = to_px([w.origin[0] + w.across[0] + w.up[0], w.origin[1] + w.across[1] + w.up[1]]);
    let (x0, y0, x1, y1) = (a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1]));
    let (mw, mh) = ((x1 - x0).max(8.0) * 0.5, (y1 - y0).max(8.0) * 0.6);
    let crop = egui::Rect::from_min_max(
        egui::pos2((x0 - mw).max(0.0), (y0 - mh).max(0.0)),
        egui::pos2((x1 + mw).min(preview.width as f32), (y1 + mh).min(preview.height as f32)),
    );
    if crop.width() < 1.0 || crop.height() < 1.0 {
        return;
    }
    // At most 300 × 90 points, at most 3× the preview's own resolution.
    let k = (300.0 / crop.width()).min(90.0 / crop.height()).clamp(0.1, 3.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(crop.width() * k, crop.height() * k), egui::Sense::hover());
    let uv = egui::Rect::from_min_max(
        egui::pos2(crop.left() / preview.width as f32, crop.top() / preview.height as f32),
        egui::pos2(crop.right() / preview.width as f32, crop.bottom() / preview.height as f32),
    );
    ui.painter().image(tex.id(), rect, uv, egui::Color32::WHITE);
    ui.painter().rect_stroke(rect, 0.0, egui::Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
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
fn apply(app: &mut PdfCraftApp, n: Intents) {
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
            v.cancel_reread();
        }
        if n.accept_page {
            let page = v.current;
            match v.accept_page(&mut app.session, page) {
                Ok(count) if count > 0 => {
                    if let Some(doc) = v.doc {
                        refresh_view(app, doc);
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
            do_accept(v, app);
        }
        if n.reject && v.reject() {
            app.notify_tr("OCR results rejected.");
        }
    });
}

/// Accept: searchable mode applies the review to the open document as one undoable step;
/// editable mode asks where to write the new `<name>_ocr.pdf`. Stale sessions and empty
/// reviews are typed failures the user sees (the edits are kept).
fn do_accept(v: &mut OcrVerify, app: &mut PdfCraftApp) {
    let doc = v.doc;
    match app.ocr_draft.output_mode {
        OutputMode::Searchable => match v.accept(&mut app.session) {
            Ok(count) => {
                if let Some(doc) = doc {
                    refresh_view(app, doc);
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

/// Write the editable-text output: into the tests' export folder when one is set, otherwise
/// ask where (the default name is `<name>_ocr.pdf`). The source is never overwritten; a
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
    if let Some(dir) = app.export_dir_override.clone() {
        let target = std::path::Path::new(&dir).join(&name);
        if is_source(&target, &source) {
            app.notify_tr("That is the source file; choose another destination");
            return;
        }
        write_editable(app, &target, bytes);
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let dialog = rfd::AsyncFileDialog::new().add_filter("PDF", &["pdf"]).set_file_name(&name);
        app.ask_one(crate::pickers::Ask::Save(dialog), None, move |app, picked: std::path::PathBuf| {
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
        let saved = crate::editing::download(&name, &bytes);
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
    match crate::editing::write_atomically(&target.to_string_lossy(), &bytes) {
        Ok(()) => {
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
fn refresh_view(app: &mut PdfCraftApp, doc: DocId) {
    if let Some(info) = app.session.get(doc).map(|d| d.info.clone())
        && let Some(view) = app.views.iter_mut().find(|view| view.id == doc)
    {
        view.document_changed(&info);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_engine::Session;

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

    /// The spec's status wording is fixed: a finished run that found nothing says so, and a
    /// cancelled one never claims a complete run.
    #[test]
    fn finished_states_keep_the_spec_wording() {
        let mut v = OcrVerify::default();
        v.finished(vec![OcrPage { already_on_page: 0, page: 0, words: vec![], skipped: None, notes: Vec::new() }], false);
        assert_eq!(v.phase, Phase::ReviewReady);
        assert_eq!(v.message, "OCR complete — no text recognized on this page.");
        v.finished(
            vec![OcrPage {
                already_on_page: 0,
                page: 0,
                words: vec![word("Hi", Some(95.0)), word("there", Some(40.0))],
                skipped: None,
                notes: Vec::new(),
            }],
            true,
        );
        assert!(v.message.starts_with("Cancelled — 2 words were read on 1 of 1 pages"), "{}", v.message);
        assert_eq!(v.phase, Phase::ReviewReady, "a cancel keeps the pages already read, in review");
        assert_eq!(v.page(0).expect("the cancelled run's page is kept").words.len(), 2);
    }

    /// The legend's text is the classifier's thresholds, spelled out — a drift between the
    /// two would put the wrong words in front of the reviewer.
    #[test]
    fn legend_rows_carry_the_classifier_thresholds() {
        let rows = legend_rows();
        assert_eq!(rows[0].0, ConfidenceBand::High);
        assert_eq!(rows[1].0, ConfidenceBand::Medium);
        assert_eq!(rows[2].0, ConfidenceBand::Low);
        assert_eq!(rows[0].1, "HIGH (>= 90%)");
        assert_eq!(rows[1].1, "MEDIUM (70-89%)");
        assert_eq!(rows[2].1, "LOW (< 70%)");
        // The same classifier drives the overlay: 90 is High, 89 is not; 70 is Medium, 69 is
        // not; the bands clamp out-of-range values.
        assert_eq!(band_for(90.0), ConfidenceBand::High);
        assert_eq!(band_for(89.0), ConfidenceBand::Medium);
        assert_eq!(band_for(70.0), ConfidenceBand::Medium);
        assert_eq!(band_for(69.0), ConfidenceBand::Low);
        assert_eq!(band_for(-1.0), ConfidenceBand::Low);
        assert_eq!(band_for(140.0), ConfidenceBand::High);
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

    /// A blank two-page-shaped review inside a real app (the document open, Run set to go
    /// inline), one word list per page, against a real job's guard.
    fn app_review(words: Vec<Vec<PlacedWord>>) -> (PdfCraftApp, DocId) {
        let blank = Session::new().create_blank(612.0, 792.0, words.len()).expect("fixture").to_vec();
        let mut app = PdfCraftApp::new();
        app.run_inline = true;
        app.open_bytes("blank.pdf", None, blank).expect("fixture");
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

    /// The status the tests see, from the app's last toast.
    fn toast(app: &mut PdfCraftApp) -> String {
        app.toast.take().map(|t| t.0).unwrap_or_default()
    }

    /// A region re-read whose worker has finished with `result`, already asked for.
    fn finished_reread(region: Option<[f32; 4]>, result: Result<Vec<PlacedWord>, String>) -> Reread {
        Reread { page: 0, region, progress: std::sync::Arc::new(Mutex::new(RereadProgress { cancel: false, result: Some(result) })) }
    }

    /// A page re-read runs OFF the UI thread: start_reread returns with the reading in flight
    /// (Run is refused while it is out), and the worker's result is applied by the next poll.
    /// No OCR models are needed: the engine fails fast here, which still walks the whole
    /// off-thread path — and a failed reading keeps the words.
    #[test]
    fn a_page_reread_runs_off_the_ui_thread_and_poll_applies_it() {
        let (mut app, _) = app_review(vec![vec![word("one", Some(95.0))]]);
        app.ocr_draft.engine = EngineChoice::Ocrs; // an ocrs-only job never probes tesseract
        app.run_inline = false; // the point of the test: a real worker thread
        with_state(&mut app, |v, app| v.start_reread(app, 0, None));
        assert!(app.ocr_verify.reread.is_some(), "the reading is in flight");
        assert!(app.ocr_verify.message.contains("Re-reading"), "{}", app.ocr_verify.message);
        // A full run waits for the re-read.
        with_state(&mut app, OcrVerify::start_run);
        assert_eq!(app.ocr_verify.phase, Phase::ReviewReady, "Run is refused while a re-read is out");
        // Wait for the worker (it fails fast without models), then apply its result.
        let progress = app.ocr_verify.reread.as_ref().expect("in flight").progress.clone();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while progress.lock().expect("progress").result.is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(progress.lock().expect("progress").result.is_some(), "the worker finished");
        with_state(&mut app, OcrVerify::poll_reread);
        assert!(app.ocr_verify.reread.is_none(), "the re-read is over");
        assert_eq!(app.ocr_verify.phase, Phase::ReviewReady, "the review survives a failed re-read");
        assert_eq!(app.ocr_verify.page(0).expect("page").words.len(), 1, "the words survive");
        assert!(!toast(&mut app).is_empty(), "the failure is said");
    }

    /// A re-read that finds nothing is a typed failure: the words stay, the review stays.
    #[test]
    fn a_reread_that_finds_nothing_keeps_the_words() {
        let (mut app, _) = app_review(vec![vec![word("one", Some(95.0))]]);
        app.ocr_verify.reread = Some(finished_reread(None, Err("the re-read found no text; the existing words are unchanged".into())));
        with_state(&mut app, OcrVerify::poll_reread);
        assert!(app.ocr_verify.reread.is_none(), "the re-read is over");
        assert_eq!(app.ocr_verify.phase, Phase::ReviewReady);
        assert_eq!(app.ocr_verify.page(0).expect("page").words.len(), 1, "the words survive the failed re-read");
        assert!(toast(&mut app).contains("found no text"));
    }

    /// Cancel asks the worker to stop and discards the reading when it lands anyway: the
    /// page's words stay as they were.
    #[test]
    fn a_cancelled_reread_is_discarded_and_keeps_the_words() {
        let (mut app, _) = app_review(vec![vec![word("one", Some(95.0))]]);
        app.ocr_verify.reread = Some(Reread { page: 0, region: None, progress: std::sync::Arc::new(Mutex::new(RereadProgress::default())) });
        with_state(&mut app, |v, _| v.cancel_reread());
        assert!(app.ocr_verify.reread.as_ref().expect("in flight").progress.lock().expect("progress").cancel, "the cancel reached the worker");
        // The worker finishes anyway; its (late) reading is discarded.
        app.ocr_verify.reread.as_ref().expect("in flight").progress.lock().expect("progress").result = Some(Ok(vec![word("late", Some(95.0))]));
        with_state(&mut app, OcrVerify::poll_reread);
        assert!(app.ocr_verify.reread.is_none());
        let words: Vec<_> = app.ocr_verify.page(0).expect("page").words.iter().map(|w| w.text.clone()).collect();
        assert_eq!(words, ["one"], "the late reading is discarded");
        assert!(app.ocr_verify.message.contains("cancelled"), "{}", app.ocr_verify.message);
    }

    /// A re-read of a document that changed since the review began is refused: the reading may
    /// no longer fit the page it was read from, so the words stay and the reason is said.
    #[test]
    fn a_reread_of_a_changed_document_is_refused() {
        let (mut app, id) = app_review(vec![vec![word("one", Some(95.0))], vec![word("two", Some(95.0))]]);
        app.ocr_verify.reread = Some(finished_reread(None, Ok(vec![word("late", Some(95.0))])));
        app.ocr_verify.current = 1;
        app.session.apply(id, pdfcraft_engine::Edit::RotatePages { pages: vec![1], degrees: 90 }).expect("an edit");
        with_state(&mut app, OcrVerify::poll_reread);
        let words: Vec<_> = app.ocr_verify.page(1).expect("page").words.iter().map(|w| w.text.clone()).collect();
        assert_eq!(words, ["two"], "the late reading is not applied");
        assert!(toast(&mut app).contains("changed"), "the refusal names its reason");
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
            v.reread = Some(finished_reread(Some(region), Ok(vec![new_word])));
        }
        let marks = {
            let v = &app.ocr_verify;
            (v.review.removed.clone(), v.review.verified.clone())
        };
        // "gone" spans x 300..340 and y 700..712 in user space: view y 80..92 on the 792 pt page.
        with_state(&mut app, OcrVerify::poll_reread);
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
        let mut app = PdfCraftApp::new();
        app.run_inline = true;
        let text = app.session.create_from_text("t", "Recognize these scanned words").expect("fixture");
        let id = app.session.open("t.pdf", None, text, None).expect("fixture");
        let job = app.session.ocr_job(id, &[0], OcrSettings::default()).expect("fixture");
        app.ocr_verify.load(
            id,
            job.guard.clone(),
            vec![OcrPage { already_on_page: 0, page: 0, words: vec![word("w", Some(95.0))], skipped: None, notes: Vec::new() }],
        );
        assert!(app.ocr_verify.correct((0, 0), "Edited"), "the correction commits");
        app.ocr_draft.output_mode = OutputMode::EditableText;
        // A destination that cannot be written (the "folder" is a regular file): the write
        // fails, the review stays in ReviewReady with the edit, Accept is enabled again.
        let blocker = std::env::temp_dir().join(format!("pdfcraft-ocr-save-block-{}", std::process::id()));
        std::fs::write(&blocker, b"not a folder").expect("fixture");
        app.export_dir_override = Some(blocker.to_string_lossy().into_owned());
        with_state(&mut app, do_accept);
        assert_eq!(app.ocr_verify.phase, Phase::ReviewReady, "a failed write keeps the review");
        assert_eq!(app.ocr_verify.text_of((0, 0)), Some("Edited"), "the edits are kept");
        // A cancelled save dialog (the override answers with nothing): nothing is written,
        // nothing changes.
        app.export_dir_override = None;
        app.pick_override = Some(Vec::new());
        with_state(&mut app, do_accept);
        app.process_picked();
        assert_eq!(app.ocr_verify.phase, Phase::ReviewReady, "a cancelled save keeps the review");
        assert_eq!(app.ocr_verify.text_of((0, 0)), Some("Edited"));
        assert_eq!(app.ocr_verify.doc, Some(id), "the review still belongs to its document");
        // A real destination: the file lands under `<name>_ocr.pdf` and the review ends.
        let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-save-out-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("fixture");
        app.pick_override = Some(vec![dir.join("t_ocr.pdf").to_string_lossy().into_owned()]);
        with_state(&mut app, do_accept);
        app.process_picked();
        assert!(dir.join("t_ocr.pdf").exists(), "the editable copy is written");
        assert_eq!(app.ocr_verify.phase, Phase::Idle, "a successful save ends the review");
        let _ = std::fs::remove_file(&blocker);
        let _ = std::fs::remove_dir_all(&dir);
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
        assert!(toast(&mut app).contains("empty or outside"));
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
        let draft = super::ocr_ui::OcrDraft { language: "fr".into(), ..Default::default() };
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

    /// The drawn region is stored in view space, so its outline is that rectangle scaled onto
    /// the pane: not mirrored on an upright page, and in place on a rotated, cropped one.
    #[test]
    fn the_region_outline_is_where_it_was_drawn() {
        let img = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(306.0, 396.0));
        let upright = pdfcraft_render::PageInfo { width: 612.0, height: 792.0, label: "1".into(), crop: [0.0, 0.0, 612.0, 792.0], rotation: 0 };
        let r = pane_region([100.0, 50.0, 200.0, 150.0], &upright, img).expect("on the page");
        assert_eq!((r.left(), r.top(), r.right(), r.bottom()), (60.0, 45.0, 110.0, 95.0), "top region stays at the top");
        let turned = pdfcraft_render::PageInfo { width: 792.0, height: 612.0, label: "1".into(), crop: [36.0, 36.0, 648.0, 828.0], rotation: 90 };
        let img = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(396.0, 306.0));
        let r = pane_region([100.0, 50.0, 200.0, 150.0], &turned, img).expect("on the page");
        assert_eq!((r.left(), r.top(), r.right(), r.bottom()), (50.0, 25.0, 100.0, 75.0));
    }
}
