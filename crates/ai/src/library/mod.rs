//! The personal PDF library: documents the user added, split into passages and searched with
//! BM25, entirely on this machine. Only the passages a request shows in its preview ever leave it.
//!
//! Documents are known by [`DocRef`] (the SHA-256 of their bytes), so a moved or renamed file is
//! recognised when it is added again. The caller opens files and extracts text (`engine`); this
//! module never touches the file system. [`Library::to_json`] and [`Library::from_json`] are the
//! saved form, which the caller writes atomically.

pub mod passage;
pub mod tokenize;

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::source::{DocRef, Sha256};
use crate::text;

/// Documents in the library.
pub const MAX_DOCUMENTS: usize = 10_000;
/// Pages indexed per document; later pages are left out and counted.
pub const MAX_PAGES: usize = 20_000;
/// Text indexed per page; the rest of a page is cut.
pub const MAX_PAGE_TEXT_BYTES: usize = 256 * 1024;
/// Passages in the whole library.
pub const MAX_PASSAGES: usize = 500_000;
/// Passage text in the whole library.
pub const MAX_TEXT_BYTES: usize = 256 << 20;
/// Largest saved library read back.
pub const MAX_FILE_BYTES: usize = 512 << 20;
/// Longest query searched; the rest is ignored.
pub const MAX_QUERY_BYTES: usize = 4 * 1024;
/// Most results one search returns.
pub const MAX_RESULTS: usize = 50;
/// Results a question retrieves unless told otherwise.
pub const DEFAULT_RESULTS: usize = 6;
/// Last-known locations kept per document.
pub const MAX_PATHS: usize = 8;
const MAX_PATH_BYTES: usize = 4_096;
const MAX_TITLE_BYTES: usize = 512;

/// BM25 parameters (Robertson and Zaragoza's usual values).
const K1: f64 = 1.2;
const B: f64 = 0.75;

const FORMAT: &str = "pdfcraft-ai-library";
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LibraryError {
    #[error("the library is full ({0})")]
    Full(&'static str),
    #[error("the saved library is larger than {MAX_FILE_BYTES} bytes")]
    FileTooLarge,
    #[error("the saved library isn't one PdfCraft can read: {0}")]
    Unreadable(String),
    #[error("the saved library is version {0}; this PdfCraft reads version {FORMAT_VERSION}")]
    WrongVersion(u32),
}

/// Why a document is in the library without being searchable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Indexed,
    /// Encrypted, and no password was given. Opening it in PdfCraft and adding it from there
    /// indexes it with that session's access.
    NeedsPassword,
    /// Its permissions forbid copying text, so it is never indexed.
    NotPermitted,
    /// No page has a text layer (a scan): Scan & OCR first.
    NoText,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub doc: DocRef,
    /// Where the file was last seen, most recent first.
    pub paths: Vec<String>,
    pub title: String,
    pub pages: u32,
    /// Pages with no text (scanned, or past [`MAX_PAGES`]).
    pub pages_without_text: u32,
    pub passages: u32,
    /// When it was added, in Unix seconds (the caller's clock).
    pub added_at: i64,
    pub status: Status,
}

/// The text of one page, by page index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageText {
    pub page: u32,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Pages(Vec<PageText>),
    NeedsPassword,
    NotPermitted,
}

/// A document to add, opened and read by the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewDocument {
    pub doc: DocRef,
    pub path: String,
    pub title: String,
    pub pages: u32,
    pub added_at: i64,
    pub content: Content,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum Added {
    New {
        status: Status,
        passages: u32,
    },
    /// Already in the library (same bytes): its location was updated, or it was indexed now
    /// that its text could be read.
    Known {
        status: Status,
        passages: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Hit {
    pub doc: Sha256,
    pub page: u32,
    pub text: String,
    pub score: f64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct StoredPassage {
    doc: Sha256,
    page: u32,
    text: String,
}

#[derive(Default)]
struct Index {
    /// Token → (passage, term frequency).
    postings: HashMap<String, Vec<(u32, u32)>>,
    /// Tokens per passage.
    lengths: Vec<u32>,
    total_length: u64,
}

impl Index {
    fn add(&mut self, text: &str) {
        let id = u32::try_from(self.lengths.len()).unwrap_or(u32::MAX);
        let mut tokens = Vec::new();
        tokenize::tokens(text, &mut tokens);
        let length = u32::try_from(tokens.len()).unwrap_or(u32::MAX);
        let mut counts: HashMap<String, u32> = HashMap::new();
        for t in tokens {
            let n = counts.entry(t).or_insert(0);
            *n = n.saturating_add(1);
        }
        for (token, tf) in counts {
            self.postings.entry(token).or_default().push((id, tf));
        }
        self.lengths.push(length);
        self.total_length = self.total_length.saturating_add(u64::from(length));
    }
}

#[derive(Default)]
pub struct Library {
    entries: Vec<Entry>,
    passages: Vec<StoredPassage>,
    text_bytes: usize,
    index: Index,
}

impl Library {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn entry(&self, sha256: &Sha256) -> Option<&Entry> {
        self.entries.iter().find(|e| e.doc.sha256 == *sha256)
    }

    pub fn passage_count(&self) -> usize {
        self.passages.len()
    }

    /// Add a document, or update the one with the same bytes. Nothing changes on an error.
    pub fn add(&mut self, new: NewDocument) -> Result<Added, LibraryError> {
        let (status, passages, pages_without_text) = match &new.content {
            Content::Pages(pages) => {
                let (passages, without) = split_pages(new.doc.sha256, pages, new.pages);
                let status = if passages.is_empty() { Status::NoText } else { Status::Indexed };
                (status, passages, without)
            }
            Content::NeedsPassword => (Status::NeedsPassword, Vec::new(), 0),
            Content::NotPermitted => (Status::NotPermitted, Vec::new(), 0),
        };
        let path = text::capped(&new.path, MAX_PATH_BYTES);
        if let Some(pos) = self.entries.iter().position(|e| e.doc.sha256 == new.doc.sha256) {
            let reindex = self.entries.get(pos).is_some_and(|e| e.status != Status::Indexed && status == Status::Indexed);
            if reindex {
                self.check_room(&passages, 0)?;
            }
            let Some(entry) = self.entries.get_mut(pos) else { return Err(LibraryError::Unreadable("entry vanished".into())) };
            entry.paths.retain(|p| *p != path);
            entry.paths.insert(0, path);
            entry.paths.truncate(MAX_PATHS);
            if reindex {
                entry.status = status;
                entry.passages = count(&passages);
                entry.pages_without_text = pages_without_text;
                self.push_passages(passages);
            }
            let entry = self.entries.get(pos).ok_or_else(|| LibraryError::Unreadable("entry vanished".into()))?;
            return Ok(Added::Known { status: entry.status, passages: entry.passages });
        }
        self.check_room(&passages, 1)?;
        let passage_count = count(&passages);
        self.entries.push(Entry {
            doc: new.doc,
            paths: vec![path],
            title: text::capped(new.title.trim(), MAX_TITLE_BYTES),
            pages: new.pages,
            pages_without_text,
            passages: passage_count,
            added_at: new.added_at,
            status,
        });
        self.push_passages(passages);
        Ok(Added::New { status, passages: passage_count })
    }

    fn check_room(&self, passages: &[StoredPassage], documents: usize) -> Result<(), LibraryError> {
        if self.entries.len().saturating_add(documents) > MAX_DOCUMENTS {
            return Err(LibraryError::Full("documents"));
        }
        if self.passages.len().saturating_add(passages.len()) > MAX_PASSAGES {
            return Err(LibraryError::Full("passages"));
        }
        let bytes = passages.iter().fold(0usize, |n, p| n.saturating_add(p.text.len()));
        if self.text_bytes.saturating_add(bytes) > MAX_TEXT_BYTES {
            return Err(LibraryError::Full("text"));
        }
        Ok(())
    }

    fn push_passages(&mut self, passages: Vec<StoredPassage>) {
        for p in passages {
            self.index.add(&p.text);
            self.text_bytes = self.text_bytes.saturating_add(p.text.len());
            self.passages.push(p);
        }
    }

    /// Remove a document and its passages. Returns whether it was there.
    pub fn remove(&mut self, sha256: &Sha256) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.doc.sha256 != *sha256);
        if self.entries.len() == before {
            return false;
        }
        let passages: Vec<StoredPassage> = std::mem::take(&mut self.passages).into_iter().filter(|p| p.doc != *sha256).collect();
        self.reindex(passages);
        true
    }

    fn reindex(&mut self, passages: Vec<StoredPassage>) {
        self.index = Index::default();
        self.text_bytes = 0;
        self.passages = Vec::with_capacity(passages.len());
        self.push_passages(passages);
    }

    /// The `k` best passages for `query`, at most one per page and, while other documents have
    /// matches, at most half of them from one document. `exclude` leaves out a document (the one
    /// being read, whose own text is sent anyway).
    pub fn search(&self, query: &str, k: usize, exclude: Option<&Sha256>) -> Vec<Hit> {
        let k = k.min(MAX_RESULTS);
        let mut terms = Vec::new();
        tokenize::tokens(text::prefix(query, MAX_QUERY_BYTES), &mut terms);
        terms.sort_unstable();
        terms.dedup();
        if k == 0 || terms.is_empty() || self.passages.is_empty() {
            return Vec::new();
        }
        let n = self.index.lengths.len() as f64;
        let average = (self.index.total_length as f64 / n).max(1.0);
        let mut scores: HashMap<u32, f64> = HashMap::new();
        for term in &terms {
            let Some(postings) = self.index.postings.get(term) else { continue };
            let df = postings.len() as f64;
            let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
            for &(id, tf) in postings {
                let length = self.index.lengths.get(id as usize).copied().unwrap_or(0) as f64;
                let tf = f64::from(tf);
                let score = idf * tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * length / average));
                if score.is_finite() {
                    *scores.entry(id).or_insert(0.0) += score;
                }
            }
        }
        let mut ranked: Vec<(u32, f64)> = scores.into_iter().filter(|(_, s)| *s > 0.0).collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let ranked: Vec<(&StoredPassage, f64)> =
            ranked.into_iter().filter_map(|(id, s)| Some((self.passages.get(id as usize)?, s))).filter(|(p, _)| exclude != Some(&p.doc)).collect();
        pick(&ranked, k)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, LibraryError> {
        let saved = SavedRef { format: FORMAT, version: FORMAT_VERSION, entries: &self.entries, passages: &self.passages };
        serde_json::to_vec(&saved).map_err(|e| LibraryError::Unreadable(e.to_string()))
    }

    /// Read a saved library. A wrong format or version is an error (the caller starts a new
    /// library and says so); damaged parts of a readable file are dropped, each with a warning.
    pub fn from_json(bytes: &[u8]) -> Result<(Self, Vec<String>), LibraryError> {
        Self::from_json_within(bytes, MAX_FILE_BYTES)
    }

    fn from_json_within(bytes: &[u8], max_bytes: usize) -> Result<(Self, Vec<String>), LibraryError> {
        if bytes.len() > max_bytes {
            return Err(LibraryError::FileTooLarge);
        }
        let header: Header = serde_json::from_slice(bytes).map_err(|e| LibraryError::Unreadable(e.to_string()))?;
        if header.format != FORMAT {
            return Err(LibraryError::Unreadable("not a PdfCraft library".into()));
        }
        if header.version != FORMAT_VERSION {
            return Err(LibraryError::WrongVersion(header.version));
        }
        let saved: Saved = serde_json::from_slice(bytes).map_err(|e| LibraryError::Unreadable(e.to_string()))?;
        let mut warnings = Vec::new();
        let mut library = Self::new();
        let mut seen = HashSet::new();
        for mut entry in saved.entries {
            if library.entries.len() >= MAX_DOCUMENTS {
                warnings.push(format!("more than {MAX_DOCUMENTS} documents; the rest were left out"));
                break;
            }
            if !seen.insert(entry.doc.sha256) {
                warnings.push(format!("document {} was listed twice", entry.doc.sha256));
                continue;
            }
            entry.paths = entry.paths.iter().take(MAX_PATHS).map(|p| text::capped(p, MAX_PATH_BYTES)).collect();
            text::truncate(&mut entry.title, MAX_TITLE_BYTES);
            entry.passages = 0;
            library.entries.push(entry);
        }
        let indexed: HashSet<Sha256> = library.entries.iter().filter(|e| e.status == Status::Indexed).map(|e| e.doc.sha256).collect();
        let mut passages = Vec::new();
        let mut dropped = 0usize;
        let mut bytes_kept = 0usize;
        for mut p in saved.passages {
            if !indexed.contains(&p.doc) || passages.len() >= MAX_PASSAGES || bytes_kept >= MAX_TEXT_BYTES {
                dropped = dropped.saturating_add(1);
                continue;
            }
            text::truncate(&mut p.text, MAX_PAGE_TEXT_BYTES);
            bytes_kept = bytes_kept.saturating_add(p.text.len());
            passages.push(p);
        }
        if dropped > 0 {
            warnings.push(format!("{dropped} passages belonged to no indexed document or passed the limits and were dropped"));
        }
        for entry in &mut library.entries {
            entry.passages = count_of(&passages, &entry.doc.sha256);
        }
        library.reindex(passages);
        Ok((library, warnings))
    }
}

fn count(passages: &[StoredPassage]) -> u32 {
    u32::try_from(passages.len()).unwrap_or(u32::MAX)
}

fn count_of(passages: &[StoredPassage], doc: &Sha256) -> u32 {
    u32::try_from(passages.iter().filter(|p| p.doc == *doc).count()).unwrap_or(u32::MAX)
}

/// Passages for the pages given, and how many of the document's pages had no text.
fn split_pages(doc: Sha256, pages: &[PageText], page_count: u32) -> (Vec<StoredPassage>, u32) {
    let mut out = Vec::new();
    let mut with_text = HashSet::new();
    for page in pages.iter().take(MAX_PAGES) {
        let texts = passage::split(text::prefix(&page.text, MAX_PAGE_TEXT_BYTES));
        if !texts.is_empty() {
            with_text.insert(page.page);
        }
        out.extend(texts.into_iter().map(|text| StoredPassage { doc, page: page.page, text }));
    }
    let without = page_count.saturating_sub(u32::try_from(with_text.len()).unwrap_or(u32::MAX));
    (out, without)
}

/// The top `k` of `ranked` (best first), one per page, spreading across documents first.
fn pick(ranked: &[(&StoredPassage, f64)], k: usize) -> Vec<Hit> {
    let documents: HashSet<&Sha256> = ranked.iter().map(|(p, _)| &p.doc).collect();
    let per_document = if documents.len() > 1 { k.div_ceil(2) } else { k };
    let mut pages = HashSet::new();
    let mut taken = HashSet::new();
    let mut from: HashMap<&Sha256, usize> = HashMap::new();
    let mut hits = Vec::new();
    for spread in [true, false] {
        for (i, (p, score)) in ranked.iter().enumerate() {
            if hits.len() >= k {
                return hits;
            }
            if taken.contains(&i) || pages.contains(&(p.doc, p.page)) {
                continue;
            }
            let n = from.entry(&p.doc).or_insert(0);
            if spread && *n >= per_document {
                continue;
            }
            *n += 1;
            taken.insert(i);
            pages.insert((p.doc, p.page));
            hits.push(Hit { doc: p.doc, page: p.page, text: p.text.clone(), score: *score });
        }
    }
    hits
}

#[derive(Deserialize)]
struct Header {
    format: String,
    version: u32,
}

#[derive(Deserialize)]
struct Saved {
    entries: Vec<Entry>,
    passages: Vec<StoredPassage>,
}

#[derive(Serialize)]
struct SavedRef<'a> {
    format: &'static str,
    version: u32,
    entries: &'a [Entry],
    passages: &'a [StoredPassage],
}

#[cfg(test)]
mod tests;
