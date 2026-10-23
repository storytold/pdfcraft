//! Document: cross-reference data, lazy object loading, and a copy-on-write overlay of edits.
//!
//! `Document` is cheap to clone (the original bytes, xref index and parse caches are shared), so
//! a clone is a snapshot: undo/redo and background jobs hold clones while the UI edits another.
//! Edits never touch the original bytes; the writer appends them (incremental save) or rewrites
//! the file (full save).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};

use crate::CosError;
use crate::object::{Dict, ObjRef, Object};
use crate::parser::{EndstreamIndex, Lexer, is_whitespace, parse_indirect, parse_indirect_shared};

thread_local! {
    /// The objects being loaded on this thread, innermost last (see `Document::try_get`).
    static LOADING: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) };
    /// Indirect `/Length` values resolved during the current outermost load: a damaged file
    /// retries a parse several ways, and each retry would resolve the same lengths again
    /// (exponential in the chain of streams whose lengths point at further streams).
    static LENGTHS: std::cell::RefCell<HashMap<u32, Option<i64>>> = std::cell::RefCell::new(HashMap::new());
}

/// Marks an object as being loaded on this thread. Loading can re-enter (indirect stream
/// `/Length`, object streams): an object already being loaded is a cycle, and the nesting is
/// bounded as well.
struct LoadGuard;

impl LoadGuard {
    const MAX: usize = 32;

    fn enter(num: u32) -> Option<Self> {
        LOADING.with(|l| {
            let mut l = l.borrow_mut();
            if l.len() >= Self::MAX || l.contains(&num) {
                return None;
            }
            l.push(num);
            Some(LoadGuard)
        })
    }
}

impl Drop for LoadGuard {
    fn drop(&mut self) {
        let outermost = LOADING.with(|l| {
            let mut l = l.borrow_mut();
            l.pop();
            l.is_empty()
        });
        if outermost {
            LENGTHS.with(|m| m.borrow_mut().clear());
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XrefEntry {
    Free { next_generation: u16 },
    InFile { offset: u64, generation: u16 },
    InStream { stream: u32, index: u32 },
}

#[derive(Clone, Copy, Debug)]
struct XrefRange {
    start: u32,
    count: u32,
    offset: usize,
    stride: usize,
}

#[derive(Clone, Debug)]
enum XrefSectionData {
    Table,
    Stream { bytes: Arc<Vec<u8>>, widths: [usize; 3] },
}

#[derive(Clone, Debug)]
struct XrefSection {
    data: XrefSectionData,
    ranges: Vec<XrefRange>,
    row_count: usize,
    max_num: u32,
    has_entries: bool,
    /// A table subsection was read one number lower than it says ([`subsection_start`]).
    renumbered: bool,
}

#[derive(Clone, Debug)]
struct XrefRevisionSection {
    primary: XrefSection,
    hybrid: Option<XrefSection>,
}

#[derive(Clone, Debug)]
enum XrefIndex {
    Compact { revisions: Vec<XrefRevisionSection>, max_num: u32, has_entries: bool },
    Materialized(Arc<BTreeMap<u32, XrefEntry>>),
}

const MAX_COMPACT_XREF_ROWS: usize = 2_000_000;
const MAX_COMPACT_XREF_BYTES: usize = 64 * 1024 * 1024;

impl XrefSection {
    fn has_any_entry(&self) -> bool {
        match &self.data {
            XrefSectionData::Table => self.has_entries,
            XrefSectionData::Stream { bytes, widths } => {
                let row_width = widths.iter().sum::<usize>();
                if row_width == 0 {
                    return false;
                }
                for range in &self.ranges {
                    let available = bytes.len().saturating_sub(range.offset) / row_width;
                    for i in 0..available.min(range.count as usize) {
                        let Some(offset) = i.checked_mul(row_width).and_then(|delta| range.offset.checked_add(delta)) else { continue };
                        let Some(row) = bytes.get(offset..offset.saturating_add(row_width)) else { continue };
                        let read_field = |from: usize, width: usize, default: u64| -> Option<u64> {
                            if width == 0 {
                                return Some(default);
                            }
                            let end = from.checked_add(width)?;
                            Some(row.get(from..end)?.iter().fold(0u64, |acc, byte| acc.wrapping_shl(8) | u64::from(*byte)))
                        };
                        let Some(kind) = read_field(0, widths[0], 1) else { continue };
                        if kind == 0 || kind == 2 {
                            return true;
                        }
                        if kind == 1 && read_field(widths[0], widths[1], 0).is_some_and(|offset| offset != 0) {
                            return true;
                        }
                    }
                }
                false
            }
        }
    }

    fn retained_bytes(&self) -> usize {
        match &self.data {
            XrefSectionData::Table => 0,
            XrefSectionData::Stream { bytes, .. } => bytes.capacity(),
        }
    }

    fn lookup(&self, num: u32, file: &[u8]) -> Option<XrefEntry> {
        let after = self.ranges.partition_point(|range| range.start <= num);
        let range = self.ranges.get(after.checked_sub(1)?)?;
        let end = u64::from(range.start).checked_add(u64::from(range.count))?;
        if u64::from(num) >= end {
            return None;
        }
        let row = usize::try_from(num.checked_sub(range.start)?).ok()?;
        let offset = range.offset.checked_add(row.checked_mul(range.stride)?)?;
        match &self.data {
            XrefSectionData::Table => {
                let mut lexer = Lexer::new(file, offset);
                lexer.skip_ws();
                let object_offset = lexer.token();
                lexer.skip_ws();
                let generation = lexer.token();
                lexer.skip_ws();
                let kind = lexer.token();
                match kind {
                    b"n" => {
                        let object_offset = std::str::from_utf8(object_offset).ok()?.parse::<u64>().ok()?;
                        if object_offset == 0 {
                            return None;
                        }
                        let generation = std::str::from_utf8(generation).ok()?.parse::<u32>().unwrap_or(0).min(u16::MAX as u32) as u16;
                        Some(XrefEntry::InFile { offset: object_offset, generation })
                    }
                    b"f" => {
                        let generation = std::str::from_utf8(generation).ok()?.parse::<u32>().unwrap_or(0).min(u16::MAX as u32) as u16;
                        Some(XrefEntry::Free { next_generation: generation })
                    }
                    _ => None,
                }
            }
            XrefSectionData::Stream { bytes, widths } => {
                let row_width = widths.iter().sum::<usize>();
                let end = offset.checked_add(row_width)?;
                let row = bytes.get(offset..end)?;
                let field = |from: usize, width: usize, default: u64| -> Option<u64> {
                    if width == 0 {
                        return Some(default);
                    }
                    let end = from.checked_add(width)?;
                    Some(row.get(from..end)?.iter().fold(0u64, |acc, byte| acc.wrapping_shl(8) | u64::from(*byte)))
                };
                let kind = field(0, widths[0], 1)?;
                let a = field(widths[0], widths[1], 0)?;
                let b = field(widths[0] + widths[1], widths[2], 0)?;
                let entry = match kind {
                    0 => XrefEntry::Free { next_generation: b.min(u16::MAX as u64) as u16 },
                    1 => XrefEntry::InFile { offset: a, generation: b.min(u16::MAX as u64) as u16 },
                    2 => XrefEntry::InStream { stream: a.min(u32::MAX as u64) as u32, index: b.min(u32::MAX as u64) as u32 },
                    _ => return None,
                };
                (!matches!(entry, XrefEntry::InFile { offset: 0, .. })).then_some(entry)
            }
        }
    }

    fn extend_numbers(&self, numbers: &mut Vec<u32>) {
        for range in &self.ranges {
            let available = match &self.data {
                XrefSectionData::Table => range.count as usize,
                XrefSectionData::Stream { bytes, widths } => {
                    let row_width = widths.iter().sum::<usize>();
                    bytes.len().saturating_sub(range.offset).checked_div(row_width).unwrap_or(0)
                }
            };
            for i in 0..available.min(range.count as usize) {
                if let Ok(i) = u32::try_from(i)
                    && let Some(num) = range.start.checked_add(i)
                {
                    numbers.push(num);
                }
            }
        }
    }
}

impl XrefRevisionSection {
    fn lookup(&self, num: u32, file: &[u8]) -> Option<XrefEntry> {
        let primary = self.primary.lookup(num, file);
        let hybrid = self.hybrid.as_ref().and_then(|section| section.lookup(num, file));
        match primary {
            None | Some(XrefEntry::Free { .. }) => hybrid.or(primary),
            entry => entry,
        }
    }

    fn extend_numbers(&self, numbers: &mut Vec<u32>) {
        self.primary.extend_numbers(numbers);
        if let Some(hybrid) = &self.hybrid {
            hybrid.extend_numbers(numbers);
        }
    }
}

impl XrefIndex {
    fn empty() -> Self {
        Self::Materialized(Arc::new(BTreeMap::new()))
    }

    fn lookup(&self, num: u32, file: &[u8]) -> Option<XrefEntry> {
        match self {
            Self::Compact { revisions, .. } => revisions.iter().find_map(|revision| revision.lookup(num, file)),
            Self::Materialized(entries) => entries.get(&num).copied(),
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::Compact { has_entries, .. } => !has_entries,
            Self::Materialized(entries) => entries.is_empty(),
        }
    }

    /// Drop an entry for object number 4294967295, which no incremental `/Size` can cover.
    /// Only a materialized index can hold one: the compact reader requires `max_num < /Size`.
    fn remove_exhausted(&mut self) -> bool {
        match self {
            Self::Compact { .. } => false,
            Self::Materialized(entries) => Arc::make_mut(entries).remove(&u32::MAX).is_some(),
        }
    }

    /// Every entry, newest section first, as the materialized reader keeps them.
    fn to_map(&self, file: &[u8]) -> BTreeMap<u32, XrefEntry> {
        match self {
            Self::Materialized(entries) => entries.as_ref().clone(),
            Self::Compact { revisions, .. } => {
                let mut numbers = Vec::new();
                for revision in revisions {
                    revision.extend_numbers(&mut numbers);
                }
                numbers.into_iter().filter_map(|num| self.lookup(num, file).map(|entry| (num, entry))).collect()
            }
        }
    }

    fn max_num(&self) -> u32 {
        match self {
            Self::Compact { max_num, .. } => *max_num,
            Self::Materialized(entries) => entries.keys().next_back().copied().unwrap_or(0),
        }
    }

    fn object_numbers(&self, file: &[u8]) -> Vec<u32> {
        match self {
            Self::Materialized(entries) => {
                entries.iter().filter_map(|(&num, entry)| (!matches!(entry, XrefEntry::Free { .. })).then_some(num)).collect()
            }
            Self::Compact { revisions, .. } => {
                if let [revision] = revisions.as_slice()
                    && revision.hybrid.is_none()
                {
                    let mut numbers = Vec::with_capacity(revision.primary.row_count);
                    revision.primary.extend_numbers(&mut numbers);
                    numbers.retain(|num| self.lookup(*num, file).is_some_and(|entry| !matches!(entry, XrefEntry::Free { .. })));
                    return numbers;
                }
                let mut numbers = Vec::new();
                for revision in revisions {
                    revision.extend_numbers(&mut numbers);
                }
                numbers.sort_unstable();
                numbers.dedup();
                numbers.retain(|num| self.lookup(*num, file).is_some_and(|entry| !matches!(entry, XrefEntry::Free { .. })));
                numbers
            }
        }
    }
}

/// One cross-reference section of the file (one per save; the first is the original).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revision {
    /// Offset of the xref section (`startxref` value).
    pub xref_offset: u64,
    /// `true` if the section is a cross-reference stream.
    pub is_stream: bool,
    /// `true` if `xref_offset` counts from the `%PDF-` header rather than from the start of the
    /// file. The two differ only in a file with bytes before its header, and an update appended
    /// to such a file must count the same way as the section it points back to.
    pub header_relative: bool,
}

#[derive(Clone, Debug)]
struct ObjStm {
    /// One slot per member in stream order, since cross-reference entries name a member by its
    /// position: (object number, byte offset within `data`), or `None` for a member whose number
    /// or offset is unusable ([`objstm_member`]).
    index: Vec<Option<(u32, usize)>>,
    data: Arc<Vec<u8>>,
}

/// Where member `num` of an object stream starts: its `offset` past `/First` (`first`), when the
/// number is an object number and the start lies inside the decoded `len` bytes. Checked on every
/// target, so a 32-bit build neither truncates nor wraps a large `/First` or offset (#1815).
fn objstm_member(num: i64, offset: i64, first: Option<usize>, len: usize) -> Option<(u32, usize)> {
    let num = u32::try_from(num).ok()?;
    let start = first?.checked_add(usize::try_from(offset).ok()?)?;
    (start < len).then_some((num, start))
}

/// Where an overlay slot came from.
#[derive(Clone, Debug, PartialEq)]
enum Slot {
    Set(u16, Arc<Object>),
    Freed(u16),
}

/// An immutable input/decode context shared by document clones. Holding this identity
/// retains neither the input bytes nor the parsed document. Weak ownership prevents an
/// expired identity from matching a later allocation at the same address.
#[derive(Clone, Debug)]
pub struct SourceIdentity(Weak<()>);

impl PartialEq for SourceIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.0.ptr_eq(&other.0)
    }
}

impl Eq for SourceIdentity {}

/// What a document reads as: its source ([`SourceIdentity`]), its trailer and every object changed
/// since it was opened. A clone, or a snapshot that undo restores, has the same state, and every
/// change makes a different one. Changed objects are compared as allocations, which the state
/// keeps alive, so a later object can't take an address one of them had.
#[derive(Clone)]
pub struct ReadState {
    source: SourceIdentity,
    trailer: Dict,
    overlay: BTreeMap<u32, Slot>,
}

impl ReadState {
    /// Whether `doc` reads as this state.
    pub fn matches(&self, doc: &Document) -> bool {
        self.source == doc.source_identity()
            && self.trailer == doc.trailer
            && self.overlay.len() == doc.overlay.len()
            && self.overlay.iter().zip(&doc.overlay).all(|((a, x), (b, y))| {
                a == b
                    && match (x, y) {
                        (Slot::Set(g, o), Slot::Set(h, p)) => g == h && Arc::ptr_eq(o, p),
                        (Slot::Freed(g), Slot::Freed(h)) => g == h,
                        _ => false,
                    }
            })
    }
}

impl std::fmt::Debug for ReadState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadState").field("changed_objects", &self.overlay.len()).finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct Document {
    data: Arc<Vec<u8>>,
    /// Where `endstream` occurs in `data`; see [`EndstreamIndex`].
    endstreams: Arc<EndstreamIndex>,
    source_identity: Arc<()>,
    entries: Arc<XrefIndex>,
    trailer: Dict,
    revisions: Arc<Vec<Revision>>,
    /// See [`Self::xref_oddities`].
    xref_oddities: Arc<Vec<(u64, u32)>>,
    /// Where the file's object headers are, as (offset, generation) of the last header of each
    /// number. Filled only when the cross-reference data proved inconsistent; see
    /// `fill_xref_gaps`.
    located: Arc<BTreeMap<u32, (u64, u16)>>,
    repair_log: Arc<Vec<String>>,
    cache: Arc<Mutex<HashMap<u32, Arc<Object>>>>,
    objstms: Arc<Mutex<HashMap<u32, Arc<ObjStm>>>>,
    stream_limit: usize,
    overlay: BTreeMap<u32, Slot>,
    next_num: u32,
    /// Position of the `%PDF-` header (some files have junk before it).
    header_offset: usize,
    version: String,
    /// The authenticated security handler of an encrypted document.
    security: Option<Arc<pdfcraft_crypt::SecurityHandler>>,
    /// Object number of the `/Encrypt` dictionary (never encrypted itself).
    encrypt_num: Option<u32>,
    /// Encryption was added, changed or removed since opening: only a full save can apply it.
    encryption_changed: bool,
    /// Set by edits that must not leave earlier revisions in the file (redaction).
    full_save: bool,
    /// The handler and `/Encrypt` object number that saves use, when encryption changed.
    out_security: Option<Arc<pdfcraft_crypt::SecurityHandler>>,
    out_encrypt_num: Option<u32>,
}

/// A temporary reader for whole-document work. Its caches never populate the editor or
/// undo snapshots. Returned objects survive cache eviction; a stream read from the file is a
/// view into the input bytes (it keeps the whole input alive while held, not a copy).
pub(crate) struct ObjectReader {
    pub(crate) document: Document,
    decoded_count: usize,
}

impl ObjectReader {
    const OBJECTS: usize = 128;
    const STREAM_BYTES: usize = 8 * 1024 * 1024;

    pub(crate) fn get(&mut self, reference: ObjRef) -> Arc<Object> {
        self.try_get(reference).unwrap_or_else(|_| Arc::new(Object::Null))
    }

    pub(crate) fn try_get(&mut self, reference: ObjRef) -> Result<Arc<Object>, CosError> {
        let object = self.document.try_get(reference.num);
        // A missed eviction here only costs a re-parse, so never fail the read on a poisoned
        // lock. The editing-cache evictors propagate instead; they guard user edits.
        if let Ok(mut streams) = self.document.objstms.lock() {
            // Only insertion changes this private cache between reads. Most neighboring
            // objects reuse a decoded stream: do not sum the whole store for every object.
            if streams.len() != self.decoded_count {
                let bytes = streams.values().fold(0usize, |bytes, stream| {
                    bytes
                        .saturating_add(stream.data.capacity())
                        .saturating_add(stream.index.capacity().saturating_mul(std::mem::size_of::<Option<(u32, usize)>>()))
                });
                if bytes > Self::STREAM_BYTES || streams.len() >= Self::OBJECTS {
                    // Retain one oversized active stream so its next object does not require
                    // decoding it again. That stream still obeys the document's decode cap.
                    let current = match self.document.xref_entry(reference.num) {
                        Some(XrefEntry::InStream { stream, .. }) => Some(stream),
                        _ => None,
                    };
                    streams.retain(|num, _| Some(*num) == current);
                }
                self.decoded_count = streams.len();
            }
        }
        // Parsed objects stay cached (an indirect /Length or a dictionary read again costs a
        // lookup, not a parse) until the cache passes its object count or holds more than
        // STREAM_BYTES of data of its own. Streams read from the file are views into the
        // input bytes, which the document keeps anyway, so they don't count; decrypted
        // streams and strings do. Checked after failed loads too: resolving a /Length may
        // have cached large objects.
        if let Ok(mut cache) = self.document.cache.lock() {
            let data = &self.document.data;
            if cache.len() >= Self::OBJECTS || cache.values().fold(0usize, |n, o| n.saturating_add(owned_bytes(o, data, 0))) > Self::STREAM_BYTES {
                cache.clear();
            }
        }
        object
    }
}

#[cfg(test)]
thread_local! {
    /// Cross-reference sections read on this thread.
    static SECTION_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Nesting [`owned_bytes`] walks: past the parser's own nesting limit, so every parsed object
/// is measured in full.
const OWNED_BYTES_DEPTH: usize = 256;

/// Bytes `object` holds of its own: string and stream data that is not a view into `data`
/// (the input file). An object nested deeper than [`OWNED_BYTES_DEPTH`] cannot be measured
/// and counts as too large to keep.
fn owned_bytes(object: &Object, data: &Arc<Vec<u8>>, depth: usize) -> usize {
    if depth > OWNED_BYTES_DEPTH {
        return usize::MAX;
    }
    match object {
        Object::String(s) => s.bytes.len(),
        Object::Stream(s) => {
            let own = if s.raw.shares(data) { 0 } else { s.raw.len() };
            s.dict.iter().fold(own, |n, (_, v)| n.saturating_add(owned_bytes(v, data, depth + 1)))
        }
        Object::Array(a) => a.iter().fold(0usize, |n, v| n.saturating_add(owned_bytes(v, data, depth + 1))),
        Object::Dict(d) => d.iter().fold(0usize, |n, (_, v)| n.saturating_add(owned_bytes(v, data, depth + 1))),
        _ => 0,
    }
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document").field("bytes", &self.data.len()).field("xref", &self.entries).field("edits", &self.overlay.len()).finish()
    }
}

/// How an encrypted document is opened: with a password, or with the file key of a document
/// that has the same encryption (an earlier revision of the same file).
enum Credentials<'a> {
    Password(Option<&'a str>),
    Handler { handler: &'a Arc<pdfcraft_crypt::SecurityHandler>, id0: Vec<u8> },
}

impl Document {
    /// A new document with an empty page tree (for split / extract / combine). It has no
    /// original bytes, so saving always writes a full file.
    pub fn new_empty() -> Self {
        let mut doc = Document {
            data: Arc::new(Vec::new()),
            endstreams: Arc::default(),
            source_identity: Arc::new(()),
            entries: Arc::new(XrefIndex::empty()),
            trailer: Dict::new(),
            revisions: Arc::new(Vec::new()),
            xref_oddities: Arc::default(),
            located: Arc::default(),
            repair_log: Arc::new(Vec::new()),
            cache: Arc::default(),
            objstms: Arc::default(),
            stream_limit: crate::object::MAX_DECODED,
            overlay: BTreeMap::new(),
            next_num: 1,
            header_offset: 0,
            version: "1.7".into(),
            security: None,
            encrypt_num: None,
            encryption_changed: false,
            full_save: false,
            out_security: None,
            out_encrypt_num: None,
        };
        let mut pages = Dict::new();
        pages.set(b"Type".to_vec(), Object::name("Pages"));
        pages.set(b"Kids".to_vec(), Object::Array(Vec::new()));
        pages.set(b"Count".to_vec(), Object::Int(0));
        let pages = doc.add(pages);
        let mut catalog = Dict::new();
        catalog.set(b"Type".to_vec(), Object::name("Catalog"));
        catalog.set(b"Pages".to_vec(), Object::Ref(pages));
        let root = doc.add(catalog);
        doc.trailer.set(b"Root".to_vec(), Object::Ref(root));
        doc
    }

    /// Parse a document. Damaged cross-reference data is reconstructed; see `repair_log`.
    /// Encrypted documents open only if their user password is empty; see `open_with_password`.
    pub fn open(data: Arc<Vec<u8>>) -> Result<Self, CosError> {
        Self::open_with_password(data, None)
    }

    /// Parse a document, authenticating encrypted ones with `password` (owner or user; `None`
    /// tries the empty password). Fails with `NeedsPassword` / `WrongPassword` as appropriate.
    pub fn open_with_password(data: Arc<Vec<u8>>, password: Option<&str>) -> Result<Self, CosError> {
        Self::open_with_stream_limit(data, password, crate::object::MAX_DECODED)
    }

    /// Open with a tighter decompression bound for object and cross-reference streams.
    /// Inspection can use this without changing the editing reader's default limit. Such a
    /// document is read-only: objects in a larger object stream read as missing, so the
    /// writers refuse to save it ([`CosError::ReadOnlyLimit`]).
    pub fn open_with_stream_limit(data: Arc<Vec<u8>>, password: Option<&str>, stream_limit: usize) -> Result<Self, CosError> {
        Self::open_as(data, Credentials::Password(password), stream_limit)
    }

    /// Open `data`, an earlier revision of this file (the bytes a signature covers) or another
    /// save of it, with this document's credentials: an encrypted one must have the same
    /// `/Encrypt` dictionary and `/ID` as this document, whose file key is then reused, so no
    /// password is needed or kept; unencrypted data opens as [`open`](Self::open) does. Data
    /// encrypted differently fails with `NeedsPassword`.
    pub fn open_with_same_credentials(&self, data: Arc<Vec<u8>>) -> Result<Self, CosError> {
        let credentials = match &self.security {
            Some(handler) => Credentials::Handler { handler, id0: self.first_id() },
            None => Credentials::Password(None),
        };
        Self::open_as(data, credentials, self.stream_limit)
    }

    fn open_as(data: Arc<Vec<u8>>, credentials: Credentials<'_>, stream_limit: usize) -> Result<Self, CosError> {
        // Viewers accept files whose header is missing or damaged as long as the body looks
        // like PDF; so do we (a note goes to the repair log).
        let header = find(&data, b"%PDF-", 0, 1024);
        let headerless = header.is_none();
        if headerless && find(&data, b" obj", 0, 4096).is_none() {
            return Err(CosError::NotPdf);
        }
        let header_offset = header.unwrap_or(0);
        let version = match header {
            Some(h) => String::from_utf8_lossy(&data[(h + 5).min(data.len())..(h + 8).min(data.len())]).trim().to_string(),
            None => "1.4".to_string(),
        };
        let mut doc = Document {
            data,
            endstreams: Arc::default(),
            source_identity: Arc::new(()),
            entries: Arc::new(XrefIndex::empty()),
            trailer: Dict::new(),
            revisions: Arc::new(Vec::new()),
            xref_oddities: Arc::default(),
            located: Arc::default(),
            repair_log: Arc::new(Vec::new()),
            cache: Arc::default(),
            objstms: Arc::default(),
            stream_limit: stream_limit.min(crate::object::MAX_DECODED),
            overlay: BTreeMap::new(),
            next_num: 1,
            header_offset,
            version,
            security: None,
            encrypt_num: None,
            encryption_changed: false,
            full_save: false,
            out_security: None,
            out_encrypt_num: None,
        };
        let mut log = Vec::new();
        if headerless {
            log.push("the %PDF- header is missing; reading the file as PDF 1.4".into());
        }
        let parsed = doc.read_xref_chain(&mut log);
        let mut authenticated = false;
        let ok = match parsed {
            Ok((entries, trailer, revisions, oddities)) => {
                doc.entries = Arc::new(entries);
                doc.trailer = trailer;
                doc.revisions = Arc::new(revisions);
                doc.xref_oddities = Arc::new(oddities);
                doc.fill_xref_gaps(&mut log);
                // Authenticate before checking the catalog: it may sit in an encrypted object
                // stream, which reads as garbage until the key is known.
                if let Some(enc) = doc.trailer.get(b"Encrypt").cloned() {
                    doc.authenticate(&enc, &credentials)?;
                    authenticated = true;
                }
                doc.root_is_catalog()
            }
            Err(e) => {
                log.push(format!("cross-reference data unreadable ({e}); rebuilding from object headers"));
                false
            }
        };
        if !ok {
            if !doc.entries.is_empty() {
                log.push("cross-reference data does not lead to a valid catalog; rebuilding from object headers".into());
            }
            doc.reconstruct(&mut log)?;
        }
        doc.repair_log = Arc::new(log);
        // Reconstruction may have found a different trailer: (re)authenticate against it.
        if (!ok || !authenticated)
            && let Some(enc) = doc.trailer.get(b"Encrypt").cloned()
        {
            doc.authenticate(&enc, &credentials)?;
        }
        if Arc::make_mut(&mut doc.entries).remove_exhausted() {
            Arc::make_mut(&mut doc.repair_log).push("skipped exhausted xref object number 4294967295".into());
        }
        let max = doc.entries.max_num();
        let size = u32::try_from(doc.trailer.int(b"Size").unwrap_or(0).max(0)).unwrap_or(u32::MAX);
        if size == u32::MAX {
            Arc::make_mut(&mut doc.repair_log).push("exhausted trailer /Size; new objects use unused object numbers".into());
        }
        doc.next_num = max.saturating_add(1).max(size).max(1);
        Ok(doc)
    }

    fn authenticate(&mut self, enc: &Object, credentials: &Credentials<'_>) -> Result<(), CosError> {
        // Authentication can change how the same input is decoded.
        self.source_identity = Arc::new(());
        let (num, dict) = match enc {
            Object::Ref(r) => (Some(r.num), self.get(*r).as_dict().cloned()),
            Object::Dict(d) => (None, Some(d.clone())),
            _ => (None, None),
        };
        let Some(dict) = dict else {
            // A dangling /Encrypt: viewers treat the file as unencrypted, and so do we.
            let mut log = self.repair_log.as_ref().clone();
            log.push("the trailer names an /Encrypt dictionary that does not exist; reading the file as unencrypted".into());
            self.repair_log = Arc::new(log);
            self.trailer.remove(b"Encrypt");
            return Ok(());
        };
        let params = crate::security::encrypt_dict(self, &dict);
        let id0 = self.first_id();
        let handler = match credentials {
            Credentials::Password(password) => Arc::new(pdfcraft_crypt::SecurityHandler::open(params, &id0, *password).map_err(|e| match e {
                pdfcraft_crypt::CryptError::WrongPassword if password.is_none() => CosError::NeedsPassword,
                pdfcraft_crypt::CryptError::WrongPassword => CosError::WrongPassword,
                other => CosError::Security(other.to_string()),
            })?),
            // The same encryption as the document whose key this is: that key opens it.
            Credentials::Handler { handler, id0: known } if *handler.dict() == params && *known == id0 => Arc::clone(handler),
            Credentials::Handler { .. } => return Err(CosError::NeedsPassword),
        };
        self.security = Some(handler);
        self.encrypt_num = num;
        // Objects read while locating the catalog were read before decryption was possible.
        self.cache = Arc::default();
        self.objstms = Arc::default();
        Ok(())
    }

    /// The first file identifier, part of the encryption key (empty when there is none).
    fn first_id(&self) -> Vec<u8> {
        match self.trailer.get(b"ID") {
            Some(Object::Array(a)) => a.first().and_then(|s| s.as_string()).map(|s| s.bytes.clone()).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Lift the restrictions of a document opened with its user password by entering the
    /// owner (permissions) password (§7.6.4). Fails with `WrongPassword` for any other
    /// password. Does nothing when the document is unencrypted or already opened as owner.
    /// Both passwords unlock the same file key, so nothing read so far is decoded again.
    pub fn unlock(&mut self, password: &str) -> Result<(), CosError> {
        let Some(current) = self.security.as_deref() else { return Ok(()) };
        if current.auth() == pdfcraft_crypt::Auth::Owner {
            return Ok(());
        }
        let owner = current.authenticate_owner(&self.first_id(), password).map_err(|e| match e {
            pdfcraft_crypt::CryptError::WrongPassword => CosError::WrongPassword,
            other => CosError::Security(other.to_string()),
        })?;
        self.security = Some(Arc::new(owner));
        Ok(())
    }

    /// The security handler, when the document is encrypted.
    pub fn security(&self) -> Option<&pdfcraft_crypt::SecurityHandler> {
        self.security.as_deref()
    }

    /// What the opening password allows (`None` for unencrypted documents: everything).
    pub fn permissions(&self) -> Option<pdfcraft_crypt::Permissions> {
        self.security().map(|s| s.permissions())
    }

    /// Make the next save a full rewrite: earlier revisions (which still hold what an edit
    /// removed) must not stay in the file. Redaction requires this.
    pub fn require_full_save(&mut self) {
        self.full_save = true;
    }

    /// The next save must rewrite the whole file (see [`Document::require_full_save`]).
    pub fn full_save_required(&self) -> bool {
        self.full_save
    }

    /// Security was set or removed since the document was opened (the next save applies it).
    pub fn encryption_changed(&self) -> bool {
        self.encryption_changed
    }

    /// The handler used to write: the new one after `set_encryption` / `remove_encryption`,
    /// otherwise the one the document was opened with.
    pub(crate) fn output_security(&self) -> (Option<&pdfcraft_crypt::SecurityHandler>, Option<u32>) {
        if self.encryption_changed { (self.out_security.as_deref(), self.out_encrypt_num) } else { (self.security.as_deref(), self.encrypt_num) }
    }

    /// The security the next save writes: protection applied with `set_encryption`, none after
    /// `remove_encryption`, otherwise the security the document was opened with.
    pub fn output_handler(&self) -> Option<&pdfcraft_crypt::SecurityHandler> {
        self.output_security().0
    }

    /// Protect the document with a password (§7.6.4). Takes effect on the next save, which is
    /// always a full rewrite. Returns the handler (authenticated as owner).
    pub fn set_encryption(&mut self, params: &pdfcraft_crypt::NewEncryption) -> Result<(), CosError> {
        // The file identifier is part of the key; make sure it exists and keep it.
        let id0 = match self.trailer.get(b"ID") {
            Some(Object::Array(a)) if a.len() == 2 => a[0].as_string().map(|s| s.bytes.clone()).unwrap_or_default(),
            _ => {
                let mut id = generated_id(&params.seed);
                id.truncate(16);
                let s = Object::String(crate::PdfString { bytes: id.clone(), hex: true });
                self.trailer.set(b"ID".to_vec(), Object::Array(vec![s.clone(), s]));
                id
            }
        };
        let h = pdfcraft_crypt::create(params, &id0).map_err(|e| CosError::Security(e.to_string()))?;
        let d = h.dict();
        let s = |b: &[u8]| Object::String(crate::PdfString { bytes: b.to_vec(), hex: true });
        let mut e = Dict::new();
        e.set(b"Filter".to_vec(), Object::name("Standard"));
        e.set(b"V".to_vec(), Object::Int(d.v));
        e.set(b"R".to_vec(), Object::Int(d.r));
        e.set(b"Length".to_vec(), Object::Int(d.length_bits));
        e.set(b"O".to_vec(), s(&d.o));
        e.set(b"U".to_vec(), s(&d.u));
        e.set(b"P".to_vec(), Object::Int(i64::from(d.p)));
        if d.v >= 4 {
            let mut cf = Dict::new();
            let mut std_cf = Dict::new();
            std_cf.set(b"Type".to_vec(), Object::name("CryptFilter"));
            let (cfm, len) = if d.v >= 5 { ("AESV3", 32) } else { ("AESV2", 16) };
            std_cf.set(b"CFM".to_vec(), Object::name(cfm));
            std_cf.set(b"Length".to_vec(), Object::Int(len));
            std_cf.set(b"AuthEvent".to_vec(), Object::name("DocOpen"));
            cf.set(b"StdCF".to_vec(), Object::Dict(std_cf));
            e.set(b"CF".to_vec(), Object::Dict(cf));
            e.set(b"StmF".to_vec(), Object::name("StdCF"));
            e.set(b"StrF".to_vec(), Object::name("StdCF"));
            if !d.encrypt_metadata {
                e.set(b"EncryptMetadata".to_vec(), Object::Bool(false));
            }
        }
        if d.v >= 5 {
            e.set(b"OE".to_vec(), s(&d.oe));
            e.set(b"UE".to_vec(), s(&d.ue));
            e.set(b"Perms".to_vec(), s(&d.perms));
        }
        if let Some(Object::Ref(old)) = self.trailer.get(b"Encrypt").cloned() {
            self.free(old);
        }
        let r = self.add(e);
        self.trailer.set(b"Encrypt".to_vec(), Object::Ref(r));
        // Objects are still read with the original handler; saves use the new one.
        self.out_security = Some(Arc::new(h));
        self.out_encrypt_num = Some(r.num);
        self.encryption_changed = true;
        Ok(())
    }

    /// Remove password protection (requires the document to be open — any authenticated
    /// password; callers should require the owner password, §7.6.4). Applied by a full save.
    pub fn remove_encryption(&mut self) {
        if let Some(Object::Ref(r)) = self.trailer.get(b"Encrypt").cloned() {
            self.free(r);
        }
        self.trailer.remove(b"Encrypt");
        self.out_security = None;
        self.out_encrypt_num = None;
        self.encryption_changed = true;
    }

    /// Decrypt a freshly parsed indirect object if the document is encrypted.
    fn decrypted(&self, id: ObjRef, o: Object) -> Object {
        match &self.security {
            Some(h) if Some(id.num) != self.encrypt_num => crate::security::transform(h, &o, id.num, id.generation, crate::security::Pass::Decrypt),
            _ => o,
        }
    }

    pub fn bytes(&self) -> &Arc<Vec<u8>> {
        &self.data
    }

    /// Identity of the original bytes, xref and authenticated decode context. Edits and
    /// clones preserve it; reopening (including after any save) produces a new identity.
    pub fn source_identity(&self) -> SourceIdentity {
        SourceIdentity(Arc::downgrade(&self.source_identity))
    }

    /// What the document reads as now (see [`ReadState`]): unlike [`Self::source_identity`], it
    /// changes with every edit, so a value derived from the document can be cached against it.
    pub fn read_state(&self) -> ReadState {
        ReadState { source: self.source_identity(), trailer: self.trailer.clone(), overlay: self.overlay.clone() }
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn trailer(&self) -> &Dict {
        &self.trailer
    }

    pub fn trailer_mut(&mut self) -> &mut Dict {
        &mut self.trailer
    }

    pub fn revisions(&self) -> &[Revision] {
        &self.revisions
    }

    /// Position of the `%PDF-` header in the file: 0 unless bytes come before it.
    pub fn header_offset(&self) -> usize {
        self.header_offset
    }

    /// There is no cross-reference chain to append to: the index was rebuilt from object
    /// headers because the file's own was unusable (see [`Self::repair_log`]), or the document
    /// was built from scratch ([`Self::new_empty`]). A save writes such a document in full, even
    /// when nothing was edited: for a repaired file that is what puts the repair on disk.
    pub fn reconstructed(&self) -> bool {
        self.revisions.is_empty()
    }

    /// Where each revision of the file ends (oldest first): the byte after the `%%EOF` (and its
    /// end-of-line) that closes its cross-reference section. `data[..end]` is that revision as it
    /// was saved. A linearized file's first-page section belongs to the revision it precedes, so
    /// ends only ever increase. Empty for a reconstructed file.
    pub fn revision_ends(&self) -> Vec<usize> {
        let d = &self.data[..];
        let mut ends: Vec<usize> = Vec::with_capacity(self.revisions.len());
        for r in self.revisions.iter() {
            let from = (r.xref_offset as usize).saturating_add(self.header_offset).min(d.len());
            let Some(at) = d[from..].windows(5).position(|w| w == b"%%EOF") else { continue };
            let mut end = from + at + 5;
            while end < d.len() && matches!(d[end], b'\r' | b'\n') && end - (from + at + 5) < 2 {
                end += 1;
            }
            match ends.last_mut() {
                Some(last) if end <= *last => {}
                _ => ends.push(end),
            }
        }
        ends
    }

    pub fn repair_log(&self) -> &[String] {
        &self.repair_log
    }

    /// Add a line to [`Self::repair_log`]: a leniency applied while reading that the user should
    /// know about (fidelity: never silent).
    pub fn note_repair(&mut self, line: String) {
        let mut log = self.repair_log.as_ref().clone();
        log.push(line);
        self.repair_log = Arc::new(log);
    }

    /// `true` when there are unsaved edits. Removing an `/Encrypt` dictionary written inline in
    /// the trailer (as MuPDF writes it) changes no object, and is an edit all the same.
    pub fn is_modified(&self) -> bool {
        !self.overlay.is_empty() || self.encryption_changed
    }

    /// Object numbers changed since the document was opened (or last saved).
    pub fn modified_objects(&self) -> Vec<u32> {
        self.overlay.keys().copied().collect()
    }

    /// Where object `num` is stored in the file (ignoring unsaved edits).
    pub fn xref_entry(&self, num: u32) -> Option<XrefEntry> {
        self.entries.lookup(num, &self.data)
    }

    /// Cross-reference entries this reader did not take at face value, as (byte offset of the
    /// section, object number): an in-use entry at offset 0, a second and different entry for
    /// one object in the same section, a reserved cross-reference stream entry type, or a
    /// hybrid /XRefStm entry that disagrees with the table. The reader sets such an entry aside
    /// and keeps what it already had for the object. Another reader, the page renderer among
    /// them, may follow it instead, so the two can then show different objects for one number.
    /// Code that compares revisions (signature validation) must not call such an object
    /// unchanged.
    pub fn xref_oddities(&self) -> &[(u64, u32)] {
        &self.xref_oddities
    }

    /// Whether the cross-reference entry of `num` locates that object itself: its `num G obj`
    /// header at exactly the recorded offset, or the member with that number at the recorded
    /// position of its object stream. When it does not, [`Self::get`] still finds the object by
    /// searching, but another reader may resolve the number differently. Ignores unsaved edits.
    pub fn xref_entry_is_exact(&self, num: u32) -> bool {
        match self.xref_entry(num) {
            None | Some(XrefEntry::Free { .. }) => true,
            Some(XrefEntry::InFile { offset, .. }) => {
                let at = |off: usize| matches!(parse_indirect_shared(&self.data, off, &|_| None, &self.endstreams), Ok((id, _)) if id.num == num);
                // Offsets may count from a header that does not start the file.
                usize::try_from(offset).is_ok_and(|off| at(off) || (self.header_offset != 0 && at(off.saturating_add(self.header_offset))))
            }
            Some(XrefEntry::InStream { stream, index }) => self
                .objstm(stream)
                .is_ok_and(|stm| usize::try_from(index).ok().and_then(|i| stm.index.get(i)).is_some_and(|m| m.is_some_and(|(n, _)| n == num))),
        }
    }

    /// Object `num` has an unsaved edit.
    pub fn is_edited(&self, num: u32) -> bool {
        self.overlay.contains_key(&num)
    }

    pub fn root(&self) -> Option<ObjRef> {
        self.trailer.reference(b"Root")
    }

    // ── object access ───────────────────────────────────────────────────────────────────────

    /// Fetch an object by reference. Missing objects are `Null` (ISO 32000-2 §7.3.10).
    pub fn get(&self, r: ObjRef) -> Arc<Object> {
        self.try_get(r.num).unwrap_or_else(|_| Arc::new(Object::Null))
    }

    /// Follow a reference if `o` is one; otherwise return `o` itself.
    pub fn resolve(&self, o: &Object) -> Arc<Object> {
        match o {
            Object::Ref(r) => self.get(*r),
            other => Arc::new(other.clone()),
        }
    }

    /// Resolve and return a dictionary (a stream's dictionary counts).
    pub fn dict(&self, o: &Object) -> Option<Dict> {
        self.resolve(o).as_dict().cloned()
    }

    pub fn try_get(&self, num: u32) -> Result<Arc<Object>, CosError> {
        match self.overlay.get(&num) {
            Some(Slot::Set(_, o)) => return Ok(o.clone()),
            Some(Slot::Freed(_)) => return Ok(Arc::new(Object::Null)),
            None => {}
        }
        if let Some(o) = self.cache.lock().map_err(|_| CosError::Poisoned)?.get(&num) {
            return Ok(o.clone());
        }
        // Loading can re-enter (indirect stream `/Length`, object streams); damaged files make
        // cycles such as `6 0 obj << /Length 6 0 R >>`. Bound the nesting on every path.
        let _guard =
            LoadGuard::enter(num).ok_or_else(|| CosError::Syntax { offset: 0, detail: format!("reference cycle while loading object {num}") })?;
        let obj = self.load(num, 0)?;
        let obj = Arc::new(obj);
        // The editing cache is bounded like the scan reader's: re-parsing a file-backed view
        // is cheap, and returned objects outlive eviction.
        self.cache.lock().map_err(|_| CosError::Poisoned)?.insert(num, obj.clone());
        self.evict_editor_cache_if_over_budget(num, &obj)?;
        Ok(obj)
    }

    /// Bounds for the editing cache: roomier than the scan reader's (128 objects / 8 MB)
    /// so interactive editing does not thrash, but still capped so touching a large
    /// document cannot pin its whole object graph. Only owned (decrypted/copied) bytes
    /// count; file-backed views re-parse for free.
    ///
    /// Worst case per document: input bytes + up to `EDITOR_CACHE_OBJECTS` ×
    /// `EDITOR_LARGE_OBJECT` (1024 × 256 KB = 256 MB) of small objects between full
    /// accounting passes + `EDITOR_OBJSTM_BYTES` (32 MB) + unsaved `overlay` edits.
    /// The count cap always holds; byte accounting between passes is approximate.
    /// Clones share these caches, so one clone's eviction drops another's hot set;
    /// that only costs re-parsing, never correctness. Eviction keeps just the newcomer:
    /// file-backed views re-parse cheaply and editor working sets stay far below 1024.
    const EDITOR_CACHE_OBJECTS: usize = 1024;
    const EDITOR_CACHE_BYTES: usize = 32 * 1024 * 1024;
    /// Object streams decode to megabytes each, so fewer are kept than plain objects.
    const EDITOR_OBJSTM_OBJECTS: usize = 256;
    const EDITOR_OBJSTM_BYTES: usize = 32 * 1024 * 1024;
    /// Newcomers above this force a full accounting pass even below the count cap:
    /// `EDITOR_CACHE_BYTES / 128`, so 128 large newcomers always trigger a pass before the
    /// byte budget can double.
    const EDITOR_LARGE_OBJECT: usize = 256 * 1024;

    fn evict_editor_cache_if_over_budget(&self, fresh_num: u32, fresh: &Object) -> Result<(), CosError> {
        // Fast path: the full fold is O(cache) per insert, so skip it for a small newcomer
        // below the count cap. `len` is copied out first; nothing here holds the lock.
        let len = self.cache.lock().map_err(|_| CosError::Poisoned)?.len();
        if len <= Self::EDITOR_CACHE_OBJECTS && owned_bytes(fresh, &self.data, 0) <= Self::EDITOR_LARGE_OBJECT {
            return Ok(());
        }
        // Walk a snapshot off-lock; re-lock only to evict.
        let live: Vec<(u32, Arc<Object>)> = self.cache.lock().map_err(|_| CosError::Poisoned)?.iter().map(|(n, o)| (*n, o.clone())).collect();
        let data = &self.data;
        let owned: usize = live.iter().fold(0usize, |n, (_, o)| n.saturating_add(owned_bytes(o, data, 0)));
        if live.len() <= Self::EDITOR_CACHE_OBJECTS && owned <= Self::EDITOR_CACHE_BYTES {
            return Ok(());
        }
        // Re-check under the final lock: another thread may have inserted or evicted since
        // the snapshot. Keep its newcomers, and skip the eviction if back within budget.
        let snapshot: std::collections::HashSet<u32> = live.into_iter().map(|(n, _)| n).collect();
        let mut cache = self.cache.lock().map_err(|_| CosError::Poisoned)?;
        if cache.len() <= Self::EDITOR_CACHE_OBJECTS {
            let data = &self.data;
            let owned: usize = cache.values().fold(0usize, |n, o| n.saturating_add(owned_bytes(o, data, 0)));
            if owned <= Self::EDITOR_CACHE_BYTES {
                return Ok(());
            }
        }
        cache.retain(|n, _| *n == fresh_num || !snapshot.contains(n));
        Ok(())
    }

    fn evict_editor_streams_if_over_budget(&self, current: u32) -> Result<(), CosError> {
        // The caller inserts `current` first, so retaining it keeps a live entry.
        let over = {
            let streams = self.objstms.lock().map_err(|_| CosError::Poisoned)?;
            let bytes = streams.values().fold(0usize, |bytes, stream| {
                bytes
                    .saturating_add(stream.data.capacity())
                    .saturating_add(stream.index.capacity().saturating_mul(std::mem::size_of::<Option<(u32, usize)>>()))
            });
            bytes > Self::EDITOR_OBJSTM_BYTES || streams.len() > Self::EDITOR_OBJSTM_OBJECTS
        };
        if over {
            self.objstms.lock().map_err(|_| CosError::Poisoned)?.retain(|n, _| *n == current);
        }
        Ok(())
    }

    /// Visit every current object without filling this document's editing cache.
    ///
    /// Full-document searches (for example, standalone timestamps) need to inspect objects
    /// that may never be used again. Keep their parse caches private and release them in
    /// batches. Returned objects own their data and remain valid after the next batch.
    pub fn scan_objects(&self) -> impl Iterator<Item = (ObjRef, Arc<Object>)> + use<> {
        let numbers = self.object_numbers();
        let mut reader = self.object_reader();
        numbers.into_iter().map(move |num| {
            let reference = ObjRef::new(num, reader.document.generation(num));
            (reference, reader.get(reference))
        })
    }

    /// Like [`Self::scan_objects`], preserving read errors for callers that must distinguish
    /// incomplete discovery from an actual null object. Caches remain private and bounded.
    pub fn scan_objects_checked(&self) -> impl Iterator<Item = (ObjRef, Result<Arc<Object>, CosError>)> + use<> {
        self.scan_objects_subset(self.object_numbers())
    }

    /// Inspect selected current objects with the same bounded cache as a whole-file scan.
    /// Missing/freed numbers yield `Null`; malformed objects return their read error.
    pub fn scan_objects_subset(&self, numbers: Vec<u32>) -> impl Iterator<Item = (ObjRef, Result<Arc<Object>, CosError>)> + use<> {
        let mut reader = self.object_reader();
        numbers.into_iter().map(move |num| {
            let reference = ObjRef::new(num, reader.document.generation(num));
            (reference, reader.try_get(reference))
        })
    }

    /// Inspect selected objects in the immutable source, ignoring all unsaved edits. This
    /// allows a consumer to check an edit's dependencies without retaining an old document.
    pub fn scan_original_objects_subset(&self, numbers: Vec<u32>) -> impl Iterator<Item = (ObjRef, Result<Arc<Object>, CosError>)> + use<> {
        let mut reader = self.object_reader();
        reader.document.overlay.clear();
        numbers.into_iter().map(move |num| {
            let reference = ObjRef::new(num, reader.document.generation(num));
            (reference, reader.try_get(reference))
        })
    }

    /// Opened with a tighter stream limit than editing uses ([`Document::open_with_stream_limit`]).
    pub(crate) fn stream_limited(&self) -> bool {
        self.stream_limit < crate::object::MAX_DECODED
    }

    pub(crate) fn object_reader(&self) -> ObjectReader {
        let mut document = self.clone();
        document.cache = Arc::default();
        document.objstms = Arc::default();
        ObjectReader { document, decoded_count: 0 }
    }

    fn load(&self, num: u32, depth: u32) -> Result<Object, CosError> {
        if depth > 16 {
            return Err(CosError::Syntax { offset: 0, detail: "reference cycle while loading".into() });
        }
        match self.entries.lookup(num, &self.data) {
            None | Some(XrefEntry::Free { .. }) => Ok(Object::Null),
            Some(XrefEntry::InFile { offset, .. }) => {
                let resolve = |r: ObjRef| {
                    if let Some(known) = LENGTHS.with(|m| m.borrow().get(&r.num).copied()) {
                        return known;
                    }
                    let length = self.try_get(r.num).ok().and_then(|o| o.as_int());
                    LENGTHS.with(|m| m.borrow_mut().insert(r.num, length));
                    length
                };
                let off = offset as usize;
                let at = |off: usize| match parse_indirect_shared(&self.data, off, &resolve, &self.endstreams) {
                    Ok((id, o)) if id.num == num => Some(self.decrypted(id, o)),
                    _ => None,
                };
                // Offsets relative to a shifted header, or simply wrong: try both fixes.
                at(off)
                    .or_else(|| if self.header_offset == 0 { None } else { at(off.saturating_add(self.header_offset)) })
                    .or_else(|| self.located.get(&num).and_then(|(o, _)| usize::try_from(*o).ok()).and_then(at))
                    .or_else(|| self.scan_for(num).map(|(id, o)| self.decrypted(id, o)))
                    .ok_or(CosError::MissingObject(num))
            }
            Some(XrefEntry::InStream { stream, index }) => {
                let stm = self.objstm(stream)?;
                // The member at the entry's position, else wherever the stream lists `num`.
                let at = usize::try_from(index).ok().and_then(|i| stm.index.get(i)).copied().flatten().filter(|(n, _)| *n == num);
                let (_, off) = at.or_else(|| stm.index.iter().flatten().find(|(n, _)| *n == num).copied()).ok_or(CosError::MissingObject(num))?;
                Lexer::new(&stm.data, off).object()
            }
        }
    }

    fn objstm(&self, num: u32) -> Result<Arc<ObjStm>, CosError> {
        if let Some(s) = self.objstms.lock().map_err(|_| CosError::Poisoned)?.get(&num) {
            return Ok(s.clone());
        }
        // A file entry that cannot place the stream (one that says it is itself compressed
        // inside another stream, as in #2367, or points nowhere) is tried by the stream's own
        // header, as a plain object with a wrong offset is. An edited or freed stream, in the
        // overlay or in a saved revision's table, is what that says it is: the file's copy
        // must not come back.
        let loaded = match self.try_get(num) {
            Ok(o) if matches!(*o, Object::Stream(_)) => o,
            _ if self.overlay.contains_key(&num) || matches!(self.xref_entry(num), Some(XrefEntry::Free { .. })) => {
                return Err(CosError::MissingObject(num));
            }
            _ => Arc::new(self.scan_for(num).map(|(id, o)| self.decrypted(id, o)).ok_or(CosError::MissingObject(num))?),
        };
        let Object::Stream(s) = &*loaded else { return Err(CosError::MissingObject(num)) };
        let n = s.dict.int(b"N").unwrap_or(0).clamp(0, 1_000_000) as usize;
        let first = usize::try_from(s.dict.int(b"First").unwrap_or(0).max(0)).ok();
        let data = s.decoded_within(self.stream_limit)?;
        let mut lx = Lexer::new(&data, 0);
        let mut index = Vec::with_capacity(n);
        for _ in 0..n {
            let (Some(Object::Int(onum)), Some(Object::Int(off))) = (lx.object().ok(), lx.object().ok()) else { break };
            index.push(objstm_member(onum, off, first, data.len()));
        }
        let stm = Arc::new(ObjStm { index, data: Arc::new(data) });
        // Same bound as the object cache: decoded object streams can be megabytes each.
        // Insert first so the retained `num` is actually in the map.
        self.objstms.lock().map_err(|_| CosError::Poisoned)?.insert(num, stm.clone());
        self.evict_editor_streams_if_over_budget(num)?;
        Ok(stm)
    }

    /// Last-resort lookup: scan the file for `num G obj`.
    fn scan_for(&self, num: u32) -> Option<(ObjRef, Object)> {
        let needle = format!("{num} ");
        let data = &self.data;
        let mut found = None;
        let mut i = 0;
        while let Some(p) = find(data, needle.as_bytes(), i, data.len()) {
            i = p + 1;
            if p > 0 && !is_whitespace(data[p - 1]) {
                continue;
            }
            if let Ok((id, o)) = parse_indirect_shared(data, p, &|_| None, &self.endstreams)
                && id.num == num
            {
                found = Some((id, o)); // keep the last (newest) definition
            }
        }
        found
    }

    // ── editing ─────────────────────────────────────────────────────────────────────────────

    /// Stream data still pointing into another document's file would keep that whole file
    /// alive for as long as this edit exists: give it a buffer of its own (just its bytes).
    fn adopt(&self, obj: Object) -> Object {
        match obj {
            Object::Stream(mut s) if s.raw.borrows_other_than(&self.data) => {
                s.raw = s.raw.detached();
                Object::Stream(s)
            }
            other => other,
        }
    }

    /// Replace an object (keeping its generation).
    pub fn set(&mut self, r: ObjRef, obj: impl Into<Object>) {
        let obj = self.adopt(obj.into());
        self.overlay.insert(r.num, Slot::Set(r.generation, Arc::new(obj)));
        if r.num >= self.next_num {
            self.next_num = r.num.saturating_add(1);
        }
    }

    /// Add a new indirect object and return its reference.
    pub fn add(&mut self, obj: impl Into<Object>) -> ObjRef {
        let r = ObjRef::new(self.allocate_num(), 0);
        let obj = self.adopt(obj.into());
        self.overlay.insert(r.num, Slot::Set(0, Arc::new(obj)));
        r
    }

    /// Find a never-used number, retaining the high-water mark for incremental /Size.
    /// Exhausted hints fall back to gaps without replacing source or overlay objects.
    fn allocate_num(&mut self) -> u32 {
        let mut num = if self.next_num == u32::MAX { 1 } else { self.next_num.max(1) };
        while self.entries.lookup(num, &self.data).is_some() || self.overlay.contains_key(&num) {
            num = if num == u32::MAX - 1 { 1 } else { num + 1 };
        }
        self.next_num = self.next_num.max(num.saturating_add(1));
        num
    }

    /// Delete an object (its number becomes free with the next generation).
    pub fn free(&mut self, r: ObjRef) {
        self.overlay.insert(r.num, Slot::Freed(r.generation.saturating_add(1)));
    }

    /// Record `current`'s object graph as unsaved edits of this document.
    ///
    /// Used when the working snapshot was restored from an earlier revision and the next
    /// incremental save must still append to this file (the last saved revision). Objects
    /// that already match are left untouched. Reads go through private readers so neither
    /// document's editing cache is filled.
    pub fn overlay_changes_from(&mut self, current: &Document) -> Result<(), CosError> {
        let saved_nums = self.object_numbers();
        let current_nums = current.object_numbers();
        let mut freed = Vec::new();
        let mut changed = Vec::new();
        {
            let mut saved_reader = self.object_reader();
            let mut current_reader = current.object_reader();
            for &num in &saved_nums {
                if current_nums.binary_search(&num).is_err() {
                    freed.push(num);
                }
            }
            for &num in &current_nums {
                let generation = current.generation(num);
                let same = saved_nums.binary_search(&num).is_ok()
                    && self.generation(num) == generation
                    && saved_reader.try_get(ObjRef::new(num, 0))? == current_reader.try_get(ObjRef::new(num, 0))?;
                if !same {
                    changed.push(num);
                }
            }
        }
        for num in freed {
            let generation = self.generation(num);
            self.free(ObjRef::new(num, generation));
        }
        let mut current_reader = current.object_reader();
        for num in changed {
            let generation = current.generation(num);
            let object = current_reader.try_get(ObjRef::new(num, 0))?;
            self.set(ObjRef::new(num, generation), (*object).clone());
        }
        *self.trailer_mut() = current.trailer().clone();
        Ok(())
    }

    /// Edit a dictionary (or stream dictionary) object in place.
    pub fn update_dict(&mut self, r: ObjRef, f: impl FnOnce(&mut Dict)) -> Result<(), CosError> {
        let mut obj = (*self.get(r)).clone();
        let d = obj.as_dict_mut().ok_or(CosError::NotADictionary(r))?;
        f(d);
        self.set(r, obj);
        Ok(())
    }

    /// Current generation of an object number.
    pub fn generation(&self, num: u32) -> u16 {
        match self.overlay.get(&num) {
            Some(Slot::Set(g, _)) => *g,
            Some(Slot::Freed(g)) => *g,
            None => match self.entries.lookup(num, &self.data) {
                Some(XrefEntry::InFile { generation, .. }) => generation,
                Some(XrefEntry::Free { next_generation }) => next_generation,
                _ => 0,
            },
        }
    }

    pub(crate) fn overlay_entries(&self) -> impl Iterator<Item = (u32, u16, Option<Arc<Object>>)> + '_ {
        self.overlay.iter().map(|(n, s)| match s {
            Slot::Set(g, o) => (*n, *g, Some(o.clone())),
            Slot::Freed(g) => (*n, *g, None),
        })
    }

    pub(crate) fn next_num(&self) -> u32 {
        self.next_num
    }

    /// All object numbers known (file and overlay), excluding freed ones.
    pub fn object_numbers(&self) -> Vec<u32> {
        let source_numbers = self.entries.object_numbers(&self.data);
        let mut numbers = Vec::with_capacity(source_numbers.len().max(self.overlay.len()));
        let mut source = source_numbers.into_iter().peekable();
        for (&num, slot) in &self.overlay {
            while source.peek().is_some_and(|n| *n < num) {
                if let Some(n) = source.next() {
                    numbers.push(n);
                }
            }
            if source.peek().is_some_and(|n| *n == num) {
                source.next();
            }
            if matches!(slot, Slot::Set(..)) {
                numbers.push(num);
            }
        }
        numbers.extend(source);
        numbers
    }

    /// Rebase this document on bytes written by the writer (after a save): edits become part of
    /// the file and the overlay is cleared, so the next incremental save appends only new edits.
    pub fn reopen_after_save(&self, bytes: Arc<Vec<u8>>) -> Result<Self, CosError> {
        Document::open(bytes)
    }

    // ── cross-reference parsing ─────────────────────────────────────────────────────────────

    fn root_is_catalog(&self) -> bool {
        let Some(r) = self.root() else { return false };
        matches!(self.try_get(r.num).as_deref(), Ok(Object::Dict(d)) if d.name(b"Type") == Some(b"Catalog") || d.contains(b"Pages"))
    }

    #[allow(clippy::type_complexity)]
    fn read_xref_chain(&self, log: &mut Vec<String>) -> Result<(XrefIndex, Dict, Vec<Revision>, Vec<(u64, u32)>), CosError> {
        let mut compact_log = Vec::new();
        if let Ok(Some((index, trailer, revisions))) = self.read_compact_xref_chain(&mut compact_log) {
            let size = trailer.int(b"Size").unwrap_or(0).max(0);
            if size > 0 && size <= i64::from(u32::MAX) && u64::from(index.max_num()) < size as u64 {
                log.extend(compact_log);
                // The compact reader declines any section with set-aside entries, so there are none here.
                return Ok((index, trailer, revisions, Vec::new()));
            }
        }
        let (entries, trailer, revisions, oddities) = self.read_xref_chain_materialized(log)?;
        Ok((XrefIndex::Materialized(Arc::new(entries)), trailer, revisions, oddities))
    }

    fn read_compact_xref_chain(&self, log: &mut Vec<String>) -> Result<Option<(XrefIndex, Dict, Vec<Revision>)>, CosError> {
        let data = &self.data;
        let tail_start = data.len().saturating_sub(4096);
        let sx = rfind(data, b"startxref", tail_start).ok_or(CosError::Syntax { offset: data.len(), detail: "no startxref".into() })?;
        let mut lx = Lexer::new(data, sx + 9);
        let first = match lx.object() {
            Ok(Object::Int(n)) if n >= 0 => n as u64,
            _ => return Err(CosError::Syntax { offset: sx, detail: "bad startxref value".into() }),
        };
        let mut sections = Vec::new();
        let mut retained_rows = 0usize;
        let mut retained_bytes = 0usize;
        let mut trailer: Option<Dict> = None;
        let mut revisions = Vec::new();
        let mut visited = HashSet::new();
        let mut next = Some(first);
        while let Some(off) = next.take() {
            if !visited.insert(off) || visited.len() > 10_000 {
                log.push(format!("cross-reference chain loops at offset {off}; stopped"));
                break;
            }
            let ((primary, sect_trailer, is_stream), header_relative) = match self.read_compact_section(off as usize) {
                Ok(Some(v)) => (v, false),
                Ok(None) => return Ok(None),
                Err(e) if self.header_offset > 0 => {
                    log.push(format!("xref at {off} unreadable ({e}); retrying relative to the header"));
                    match self.read_compact_section(off as usize + self.header_offset)? {
                        Some(v) => (v, true),
                        None => return Ok(None),
                    }
                }
                // An unreadable older section loses every object only it locates: the
                // materialized reader logs it and the file is rebuilt from object headers (#1656).
                Err(e) => return Err(e),
            };
            revisions.push(Revision { xref_offset: off, is_stream, header_relative });
            if primary.renumbered {
                log.push(format!("xref subsection at {off} is numbered from 1 but begins with object 0's entry; read from 0"));
            }
            let Some(rows) = retained_rows.checked_add(primary.row_count).filter(|rows| *rows <= MAX_COMPACT_XREF_ROWS) else { return Ok(None) };
            let Some(bytes) = retained_bytes.checked_add(primary.retained_bytes()).filter(|bytes| *bytes <= MAX_COMPACT_XREF_BYTES) else {
                return Ok(None);
            };
            retained_rows = rows;
            retained_bytes = bytes;
            let hybrid = if let Some(x) = sect_trailer.int(b"XRefStm").filter(|x| *x >= 0) {
                match self.read_compact_section(x as usize) {
                    Ok(Some((section, _, _))) => {
                        let Some(rows) = retained_rows.checked_add(section.row_count).filter(|rows| *rows <= MAX_COMPACT_XREF_ROWS) else {
                            return Ok(None);
                        };
                        let Some(bytes) = retained_bytes.checked_add(section.retained_bytes()).filter(|bytes| *bytes <= MAX_COMPACT_XREF_BYTES)
                        else {
                            return Ok(None);
                        };
                        retained_rows = rows;
                        retained_bytes = bytes;
                        Some(section)
                    }
                    Ok(None) => return Ok(None),
                    Err(e) => {
                        log.push(format!("hybrid /XRefStm at {x} unreadable: {e}"));
                        None
                    }
                }
            } else {
                None
            };
            if let Some(hybrid) = &hybrid {
                // A hybrid entry that disagrees with the table is set aside by the materialized reader.
                let mut numbers = Vec::new();
                hybrid.extend_numbers(&mut numbers);
                for num in numbers {
                    let Some(entry) = hybrid.lookup(num, &self.data) else { continue };
                    if primary.lookup(num, &self.data).is_some_and(|old| !matches!(old, XrefEntry::Free { .. }) && old != entry) {
                        return Ok(None);
                    }
                }
            }
            sections.push(XrefRevisionSection { primary, hybrid });
            next = prev_offset(data, &sect_trailer, off, |num| sections.iter().find_map(|s| s.lookup(num, data)), log);
            if trailer.is_none() {
                trailer = Some(sect_trailer);
            }
        }
        revisions.reverse();
        let mut trailer = trailer.ok_or(CosError::Syntax { offset: 0, detail: "no trailer".into() })?;
        for k in [&b"Prev"[..], b"XRefStm", b"Type", b"W", b"Index", b"Filter", b"DecodeParms", b"Length"] {
            trailer.remove(k);
        }
        let mut max_num = 0;
        let mut has_entries = false;
        for revision in &sections {
            max_num = max_num.max(revision.primary.max_num);
            has_entries |= revision.primary.has_entries;
            if let Some(hybrid) = &revision.hybrid {
                max_num = max_num.max(hybrid.max_num);
                has_entries |= hybrid.has_entries;
            }
        }
        Ok(Some((XrefIndex::Compact { revisions: sections, max_num, has_entries }, trailer, revisions)))
    }

    fn read_compact_section(&self, off: usize) -> Result<Option<(XrefSection, Dict, bool)>, CosError> {
        let data = &self.data;
        let mut lx = Lexer::new(data, off);
        if lx.eat_keyword(b"xref") {
            let mut ranges = Vec::new();
            let mut layout_ok = true;
            let mut max_num = 0;
            let mut has_entries = false;
            let mut previous_end = None;
            let mut row_count = 0usize;
            let mut renumbered = false;
            loop {
                lx.skip_ws();
                if lx.eat_keyword(b"trailer") {
                    let t = lx.object()?;
                    let d = t.as_dict().cloned().ok_or(CosError::Syntax { offset: lx.pos, detail: "trailer is not a dictionary".into() })?;
                    if !layout_ok {
                        return Ok(None);
                    }
                    return Ok(Some((XrefSection { data: XrefSectionData::Table, ranges, row_count, max_num, has_entries, renumbered }, d, false)));
                }
                let (Ok(Object::Int(start)), Ok(Object::Int(count))) = (lx.object(), lx.object()) else {
                    return Err(CosError::Syntax { offset: lx.pos, detail: "bad xref subsection header".into() });
                };
                if !valid_xref_range(start, count) || count > 10_000_000 {
                    return Err(CosError::Syntax { offset: lx.pos, detail: "invalid xref object range".into() });
                }
                let shifted = subsection_start(data, self.header_offset, lx.pos, start, count);
                renumbered |= shifted != start;
                let start = shifted;
                let Some(next_rows) = usize::try_from(count).ok().and_then(|count| row_count.checked_add(count)) else { return Ok(None) };
                if next_rows > MAX_COMPACT_XREF_ROWS {
                    return Ok(None);
                }
                row_count = next_rows;
                let mut range_offset = None;
                let mut stride = None;
                let mut previous_record = None;
                for i in 0..count {
                    lx.skip_ws();
                    let record = lx.pos;
                    if range_offset.is_none() {
                        range_offset = Some(record);
                    }
                    if let Some(previous) = previous_record {
                        let Some(delta) = record.checked_sub(previous) else {
                            layout_ok = false;
                            continue;
                        };
                        if stride.is_some_and(|known| known != delta) {
                            layout_ok = false;
                        }
                        stride.get_or_insert(delta);
                    }
                    previous_record = Some(record);
                    let o = lx.token();
                    lx.skip_ws();
                    let _generation = lx.token();
                    lx.skip_ws();
                    let kind = lx.token();
                    let num = (start + i) as u32;
                    let offset = std::str::from_utf8(o).ok().and_then(|s| s.parse::<u64>().ok());
                    match (kind, offset) {
                        (b"n", Some(offset)) if offset != 0 => {
                            has_entries = true;
                            max_num = max_num.max(num);
                        }
                        (b"n", Some(_)) => {
                            // Set aside by the materialized reader, which records it.
                            if num != 0 {
                                return Ok(None);
                            }
                        }
                        (b"f", _) => {
                            has_entries = true;
                            max_num = max_num.max(num);
                        }
                        _ => return Err(CosError::Syntax { offset: lx.pos, detail: "bad xref entry".into() }),
                    }
                }
                if count > 0 {
                    let range_start = start as u32;
                    let range_count = count as u32;
                    let end = u64::from(range_start) + u64::from(range_count);
                    if previous_end.is_some_and(|previous| u64::from(range_start) < previous) {
                        layout_ok = false;
                    }
                    previous_end = Some(end);
                    ranges.push(XrefRange {
                        start: range_start,
                        count: range_count,
                        offset: range_offset.unwrap_or(lx.pos),
                        stride: stride.unwrap_or(1),
                    });
                }
            }
        }
        let (_, obj) = parse_indirect(data, off, &|_| None)?;
        let Object::Stream(stream) = obj else { return Err(CosError::Syntax { offset: off, detail: "expected xref table or stream".into() }) };
        if stream.dict.name(b"Type") != Some(b"XRef") {
            return Err(CosError::Syntax { offset: off, detail: "stream at startxref is not /Type /XRef".into() });
        }
        let w: Vec<usize> = stream
            .dict
            .get(b"W")
            .and_then(Object::as_array)
            .map(|a| a.iter().map(|x| x.as_int().unwrap_or(0).clamp(0, 8) as usize).collect())
            .unwrap_or_default();
        if w.len() < 3 {
            return Err(CosError::Syntax { offset: off, detail: "xref stream /W invalid".into() });
        }
        let widths = [w[0], w[1], w[2]];
        let row = widths.iter().sum::<usize>();
        if row == 0 {
            return Err(CosError::Syntax { offset: off, detail: "xref stream row width 0".into() });
        }
        let size = stream.dict.int(b"Size").unwrap_or(0).max(0);
        let index: Vec<i64> = match stream.dict.get(b"Index").and_then(Object::as_array) {
            Some(a) => a.iter().filter_map(Object::as_int).collect(),
            None => vec![0, size],
        };
        let mut requested_rows = 0usize;
        let mut previous_end = None;
        for pair in index.chunks(2) {
            let [start, count] = pair else { break };
            if !valid_xref_range(*start, *count) {
                return Err(CosError::Syntax { offset: off, detail: "invalid xref object range".into() });
            }
            let Ok(count) = usize::try_from(*count) else { return Ok(None) };
            let Some(next) = requested_rows.checked_add(count).filter(|rows| *rows <= MAX_COMPACT_XREF_ROWS) else { return Ok(None) };
            if count > 0 {
                let end = (*start as u64).saturating_add(count as u64);
                if previous_end.is_some_and(|previous| (*start as u64) < previous) {
                    return Ok(None);
                }
                previous_end = Some(end);
            }
            requested_rows = next;
        }
        if requested_rows.checked_mul(row).is_none_or(|bytes| bytes > MAX_COMPACT_XREF_BYTES) {
            return Ok(None);
        }
        let raw = stream.decoded_within(MAX_XREF_OBJECTS.saturating_mul(row).min(self.stream_limit).min(MAX_COMPACT_XREF_BYTES))?;
        if raw.capacity() > MAX_COMPACT_XREF_BYTES {
            return Ok(None);
        }
        let mut ranges = Vec::new();
        let mut row_start = 0usize;
        let available_rows = raw.len() / row;
        for pair in index.chunks(2) {
            let [start, count] = pair else { break };
            let range_start = *start as u32;
            let Ok(requested) = usize::try_from(*count) else { return Ok(None) };
            let range_count = requested.min(available_rows.saturating_sub(row_start));
            if range_count > 0 {
                let Ok(range_count_u32) = u32::try_from(range_count) else { return Ok(None) };
                let Some(byte_offset) = row_start.checked_mul(row) else { return Ok(None) };
                ranges.push(XrefRange { start: range_start, count: range_count_u32, offset: byte_offset, stride: row });
            }
            let Some(next) = row_start.checked_add(range_count) else { return Ok(None) };
            row_start = next;
        }
        // Rows the materialized reader sets aside (reserved types, offset 0) are left to it.
        for range in &ranges {
            for i in 0..range.count as usize {
                let at = range.offset + i * row;
                let Some(r) = raw.get(at..at + row) else { break };
                let field = |from: usize, width: usize, default: u64| -> u64 {
                    if width == 0 {
                        return default;
                    }
                    r[from..from + width].iter().fold(0u64, |acc, byte| acc << 8 | u64::from(*byte))
                };
                let kind = field(0, widths[0], 1);
                let offset = field(widths[0], widths[1], 0);
                if kind > 2 || (kind == 1 && offset == 0 && range.start + i as u32 != 0) {
                    return Ok(None);
                }
            }
        }
        let max_num = ranges.iter().filter_map(|range| range.start.checked_add(range.count.saturating_sub(1))).max().unwrap_or(0);
        let row_count = row_start;
        let section = XrefSection {
            data: XrefSectionData::Stream { bytes: Arc::new(raw), widths },
            ranges,
            row_count,
            max_num,
            has_entries: false,
            renumbered: false,
        };
        let has_entries = section.has_any_entry();
        let section = XrefSection { has_entries, ..section };
        Ok(Some((section, stream.dict.clone(), true)))
    }

    #[allow(clippy::type_complexity)]
    fn read_xref_chain_materialized(
        &self,
        log: &mut Vec<String>,
    ) -> Result<(BTreeMap<u32, XrefEntry>, Dict, Vec<Revision>, Vec<(u64, u32)>), CosError> {
        let data = &self.data;
        let tail_start = data.len().saturating_sub(4096);
        let sx = rfind(data, b"startxref", tail_start).ok_or(CosError::Syntax { offset: data.len(), detail: "no startxref".into() })?;
        let mut lx = Lexer::new(data, sx + 9);
        let first = match lx.object() {
            Ok(Object::Int(n)) if n >= 0 => n as u64,
            _ => return Err(CosError::Syntax { offset: sx, detail: "bad startxref value".into() }),
        };
        let mut entries = BTreeMap::new();
        let mut trailer: Option<Dict> = None;
        let mut revisions = Vec::new();
        let mut oddities = Vec::new();
        let mut visited = HashSet::new();
        // Hybrid streams already read, by offset: their entries, or `None` when unreadable. A newer
        // section has merged every entry of one into `entries`, so an older section that names it
        // again only has its own table compared with them, without decoding the stream again.
        // At most MAX_XREF_OBJECTS entries are kept in all; a stream past that is read again.
        let mut hybrids: HashMap<i64, Option<BTreeMap<u32, XrefEntry>>> = HashMap::new();
        let (mut hybrid_rows, mut noted) = (0usize, HashSet::new());
        let mut next = Some(first);
        while let Some(off) = next.take() {
            if !visited.insert(off) || visited.len() > 10_000 {
                log.push(format!("cross-reference chain loops at offset {off}; stopped"));
                break;
            }
            // This section's own entries; older sections only fill what newer ones left out.
            let mut sect = BTreeMap::new();
            // Object numbers whose entry in this section was set aside ([`Self::xref_oddities`]).
            let mut odd = Vec::new();
            let ((sect_trailer, is_stream), header_relative) = match self.read_section(off as usize, &mut sect, &mut odd, log) {
                Ok(v) => (v, false),
                Err(e) if self.header_offset > 0 => {
                    log.push(format!("xref at {off} unreadable ({e}); retrying relative to the header"));
                    sect.clear();
                    odd.clear();
                    (self.read_section(off as usize + self.header_offset, &mut sect, &mut odd, log)?, true)
                }
                Err(e) => {
                    // An unreadable older section loses every object only it locates (the page
                    // tree, often): rebuild from object headers, as readers do (#1656).
                    if !revisions.is_empty() {
                        log.push(format!("older cross-reference section at {off} is unreadable: {e}"));
                    }
                    return Err(e);
                }
            };
            revisions.push(Revision { xref_offset: off, is_stream, header_relative });
            // Hybrid files (§7.5.8.4): the /XRefStm entries supplement the table and replace the
            // free entries it gives objects only a cross-reference stream can locate.
            let hybrid = sect_trailer.int(b"XRefStm").filter(|x| *x >= 0);
            if let Some((x, read)) = hybrid.and_then(|x| hybrids.get(&x).map(|read| (x, read))) {
                if noted.insert(x) {
                    log.push(format!("hybrid /XRefStm at {x} named again by the section at {off}; read once"));
                }
                if let Some(stm) = read {
                    for (num, old) in &sect {
                        // The table places the object somewhere other than the stream does.
                        if !matches!(old, XrefEntry::Free { .. }) && stm.get(num).is_some_and(|e| e != old) {
                            odd.push(*num);
                        }
                    }
                }
            } else if let Some(x) = hybrid {
                let mut stm = BTreeMap::new();
                let read = match self.read_section(x as usize, &mut stm, &mut odd, log) {
                    Ok(_) => {
                        for (&num, &e) in &stm {
                            match sect.get(&num) {
                                None | Some(XrefEntry::Free { .. }) => {
                                    sect.insert(num, e);
                                }
                                // The table already places the object somewhere else.
                                Some(old) if *old != e => odd.push(num),
                                Some(_) => {}
                            }
                        }
                        Some(stm)
                    }
                    Err(e) => {
                        log.push(format!("hybrid /XRefStm at {x} unreadable: {e}"));
                        None
                    }
                };
                let rows = read.as_ref().map_or(0, BTreeMap::len);
                if let Some(total) = hybrid_rows.checked_add(rows).filter(|total| *total <= MAX_XREF_OBJECTS) {
                    hybrid_rows = total;
                    hybrids.insert(x, read);
                }
            }
            oddities.extend(odd.into_iter().map(|num| (off, num)));
            for (num, e) in sect {
                entries.entry(num).or_insert(e);
            }
            next = prev_offset(data, &sect_trailer, off, |num| entries.get(&num).copied(), log);
            if trailer.is_none() {
                trailer = Some(sect_trailer);
            }
        }
        revisions.reverse();
        let mut trailer = trailer.ok_or(CosError::Syntax { offset: 0, detail: "no trailer".into() })?;
        for k in [&b"Prev"[..], b"XRefStm", b"Type", b"W", b"Index", b"Filter", b"DecodeParms", b"Length"] {
            trailer.remove(k);
        }
        Ok((entries, trailer, revisions, oddities))
    }

    /// Read one section at `off`, adding entries not already known (newer sections win). `odd`
    /// collects the object numbers whose entry here is set aside ([`Self::xref_oddities`]).
    fn read_section(
        &self,
        off: usize,
        entries: &mut BTreeMap<u32, XrefEntry>,
        odd: &mut Vec<u32>,
        log: &mut Vec<String>,
    ) -> Result<(Dict, bool), CosError> {
        #[cfg(test)]
        SECTION_READS.with(|c| c.set(c.get() + 1));
        let data = &self.data;
        let mut lx = Lexer::new(data, off);
        if lx.eat_keyword(b"xref") {
            loop {
                lx.skip_ws();
                if lx.eat_keyword(b"trailer") {
                    let t = lx.object()?;
                    let d = t.as_dict().cloned().ok_or(CosError::Syntax { offset: lx.pos, detail: "trailer is not a dictionary".into() })?;
                    return Ok((d, false));
                }
                let (Ok(Object::Int(start)), Ok(Object::Int(count))) = (lx.object(), lx.object()) else {
                    return Err(CosError::Syntax { offset: lx.pos, detail: "bad xref subsection header".into() });
                };
                if !valid_xref_range(start, count) || count > 10_000_000 {
                    return Err(CosError::Syntax { offset: lx.pos, detail: "invalid xref object range".into() });
                }
                let shifted = subsection_start(data, self.header_offset, lx.pos, start, count);
                if shifted != start {
                    log.push(format!("xref subsection at {off} is numbered from 1 but begins with object 0's entry; read from 0"));
                }
                let start = shifted;
                for i in 0..count {
                    lx.skip_ws();
                    let o = lx.token();
                    lx.skip_ws();
                    let g = lx.token();
                    lx.skip_ws();
                    let kind = lx.token();
                    let num = (start + i) as u32;
                    let offset = std::str::from_utf8(o).ok().and_then(|s| s.parse::<u64>().ok());
                    let generation = std::str::from_utf8(g).ok().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0).min(u16::MAX as u32) as u16;
                    let entry = match (kind, offset) {
                        (b"n", Some(offset)) => XrefEntry::InFile { offset, generation },
                        (b"f", _) => XrefEntry::Free { next_generation: generation },
                        _ => return Err(CosError::Syntax { offset: lx.pos, detail: "bad xref entry".into() }),
                    };
                    // Offset 0 "in use" entries are a known producer bug: ignore them. (Object 0
                    // is never a real object, so its entry cannot mislead anyone.)
                    if matches!(entry, XrefEntry::InFile { offset: 0, .. }) {
                        if num != 0 {
                            odd.push(num);
                        }
                        continue;
                    }
                    if *entries.entry(num).or_insert(entry) != entry {
                        odd.push(num);
                    }
                }
            }
        }
        // Cross-reference stream.
        let (_, obj) = parse_indirect(data, off, &|_| None)?;
        let Object::Stream(s) = obj else { return Err(CosError::Syntax { offset: off, detail: "expected xref table or stream".into() }) };
        if s.dict.name(b"Type") != Some(b"XRef") {
            return Err(CosError::Syntax { offset: off, detail: "stream at startxref is not /Type /XRef".into() });
        }
        let w: Vec<usize> = s
            .dict
            .get(b"W")
            .and_then(Object::as_array)
            .map(|a| a.iter().map(|x| x.as_int().unwrap_or(0).clamp(0, 8) as usize).collect())
            .unwrap_or_default();
        if w.len() < 3 {
            return Err(CosError::Syntax { offset: off, detail: "xref stream /W invalid".into() });
        }
        let size = s.dict.int(b"Size").unwrap_or(0).max(0);
        let index: Vec<i64> = match s.dict.get(b"Index").and_then(Object::as_array) {
            Some(a) => a.iter().filter_map(Object::as_int).collect(),
            None => vec![0, size],
        };
        let row = w[0] + w[1] + w[2];
        if row == 0 {
            return Err(CosError::Syntax { offset: off, detail: "xref stream row width 0".into() });
        }
        // One row per object, and a document has at most 8,388,607 indirect objects (the classic
        // implementation limit): decoding more than that (about 200 MB at the widest rows) is a
        // decompression bomb, not cross-reference data. The section is then reconstructed.
        let raw = s.decoded_within(MAX_XREF_OBJECTS.saturating_mul(row).min(self.stream_limit))?;
        let field = |r: &[u8], from: usize, len: usize, default: u64| -> u64 {
            if len == 0 {
                return default;
            }
            r[from..from + len].iter().fold(0u64, |acc, b| acc << 8 | *b as u64)
        };
        let mut rows = raw.chunks_exact(row);
        for pair in index.chunks(2) {
            let [start, count] = pair else { break };
            if !valid_xref_range(*start, *count) {
                return Err(CosError::Syntax { offset: off, detail: "invalid xref object range".into() });
            }
            for i in 0..*count {
                let Some(r) = rows.next() else { break };
                let t = field(r, 0, w[0], 1);
                let a = field(r, w[0], w[1], 0);
                let b = field(r, w[0] + w[1], w[2], 0);
                let num = (*start + i) as u32;
                let entry = match t {
                    0 => XrefEntry::Free { next_generation: b.min(u16::MAX as u64) as u16 },
                    1 => XrefEntry::InFile { offset: a, generation: b.min(u16::MAX as u64) as u16 },
                    2 => XrefEntry::InStream { stream: a.min(u32::MAX as u64) as u32, index: b.min(u32::MAX as u64) as u32 },
                    // Reserved types are treated as null references (§7.5.8.3).
                    _ => {
                        odd.push(num);
                        continue;
                    }
                };
                if matches!(entry, XrefEntry::InFile { offset: 0, .. }) {
                    if num != 0 {
                        odd.push(num);
                    }
                    continue;
                }
                if *entries.entry(num).or_insert(entry) != entry {
                    odd.push(num);
                }
            }
        }
        Ok((s.dict.clone(), true))
    }

    /// Cross-reference data that leaves out objects the file contains: a subsection numbered one
    /// too low (object 0 in use, the last object without an entry), or an in-use entry at offset 0
    /// that nothing else replaces. (A number no section lists is not evidence: it is a null
    /// object, §7.3.10, and valid files have them below an oversized `/Size`.) Viewers rebuild
    /// the table and show these objects; reading them as null would drop them from every copy.
    /// When one of those shows, the file is scanned once for object headers, and wrong offsets
    /// are looked up there (`load`). Only the numbers the damage leaves out take the last
    /// header's location: one whose entry was set aside, or the last object of a subsection
    /// numbered one too low. Any other unlisted number stays null, and existing
    /// entries are never replaced: an object freed by an update stays freed.
    fn fill_xref_gaps(&mut self, log: &mut Vec<String>) {
        let entries = &self.entries;
        let shifted = matches!(entries.lookup(0, &self.data), Some(XrefEntry::InFile { .. }));
        let set_aside = self.xref_oddities.iter().any(|(_, num)| entries.lookup(*num, &self.data).is_none());
        if !(shifted || set_aside) {
            return;
        }
        let mut located = BTreeMap::new();
        for (num, offset, generation, _) in object_headers(&self.data) {
            located.insert(num, (offset, generation));
        }
        let mut filled = entries.to_map(&self.data);
        // A subsection `0 N` whose rows are those of objects 1 to N leaves out object N, whatever
        // newer sections say about its neighbours.
        let past_shift: HashSet<u32> = if shifted {
            self.revisions
                .iter()
                .filter(|r| !r.is_stream)
                .filter_map(|r| usize::try_from(r.xref_offset).ok().map(|o| if r.header_relative { o.saturating_add(self.header_offset) } else { o }))
                .filter_map(|at| shifted_subsection_end(&self.data, at))
                .collect()
        } else {
            HashSet::new()
        };
        let odd: HashSet<u32> = self.xref_oddities.iter().map(|(_, n)| *n).collect();
        let mut found = 0usize;
        for (&num, &(offset, generation)) in &located {
            let left_out = past_shift.contains(&num) || odd.contains(&num);
            if num != 0 && !filled.contains_key(&num) && left_out {
                filled.insert(num, XrefEntry::InFile { offset, generation });
                found += 1;
            }
        }
        if found > 0 {
            log.push(format!("the cross-reference data leaves out {found} objects the file contains; located them by their object headers"));
            self.entries = Arc::new(XrefIndex::Materialized(Arc::new(filled)));
        }
        self.located = Arc::new(located);
    }

    /// Rebuild the cross-reference index by scanning for `N G obj` headers (§7.5.4 recovery).
    fn reconstruct(&mut self, log: &mut Vec<String>) -> Result<(), CosError> {
        let data = self.data.clone();
        let mut entries: BTreeMap<u32, XrefEntry> = BTreeMap::new();
        let mut objstms = Vec::new();
        // Offsets of objects that may be cross-reference streams: their dictionaries are the
        // trailers of files that have no `trailer` keyword (§7.5.8).
        let mut xref_streams = Vec::new();
        for (num, offset, generation, body) in object_headers(&data) {
            entries.insert(num, XrefEntry::InFile { offset, generation });
            objstms.push(num);
            if find(&data, b"/XRef", body, body.saturating_add(1024)).is_some()
                && let Ok(at) = usize::try_from(offset)
            {
                xref_streams.push(at);
            }
        }
        if entries.is_empty() {
            return Err(CosError::Syntax { offset: 0, detail: "no objects found".into() });
        }
        self.entries = Arc::new(XrefIndex::Materialized(Arc::new(entries.clone())));
        self.cache.lock().map_err(|_| CosError::Poisoned)?.clear();
        // Objects inside object streams.
        for num in objstms {
            if let Ok(o) = self.try_get(num)
                && let Object::Stream(s) = &*o
                && s.dict.name(b"Type") == Some(b"ObjStm")
                && let Ok(stm) = self.objstm(num)
            {
                for (idx, member) in stm.index.iter().enumerate() {
                    if let Some((inner, _)) = member {
                        entries.entry(*inner).or_insert(XrefEntry::InStream { stream: num, index: idx as u32 });
                    }
                }
            }
        }
        let object_count = entries.len();
        self.entries = Arc::new(XrefIndex::Materialized(Arc::new(entries)));
        self.cache.lock().map_err(|_| CosError::Poisoned)?.clear();
        // Trailer: the last `trailer` dictionary or cross-reference stream dictionary with a
        // usable /Root (last in the file, whichever kind), else the catalog itself. One whose
        // /Root is a catalog (as `root_is_catalog` judges it) beats one whose /Root is another
        // dictionary, such as /Info, wherever it is; an FDF's /Root (no /Type, no /Pages) still
        // counts when nothing better is there.
        // A file whose only trailers are cross-reference streams (and whose `startxref` is gone
        // or points nowhere) keeps its /Info, /ID and /Encrypt this way.
        let rank = |at: usize, d: &Dict| {
            let root = self.try_get(d.reference(b"Root")?.num).ok()?;
            let Object::Dict(c) = &*root else { return None };
            Some((c.name(b"Type") == Some(b"Catalog") || c.contains(b"Pages"), at))
        };
        let mut trailer: Option<((bool, usize), Dict)> = None;
        let mut from = 0;
        while let Some(p) = find(&data, b"trailer", from, data.len()) {
            from = p + 7;
            let mut lx = Lexer::new(&data, from);
            if let Ok(Object::Dict(d)) = lx.object()
                && let Some(r) = rank(p, &d)
                && trailer.as_ref().is_none_or(|(best, _)| r > *best)
            {
                trailer = Some((r, d));
            }
        }
        for at in xref_streams {
            if trailer.as_ref().is_some_and(|((catalog, p), _)| *catalog && *p > at) {
                continue;
            }
            if let Ok((_, Object::Stream(s))) = parse_indirect_shared(&data, at, &|_| None, &self.endstreams)
                && s.dict.name(b"Type") == Some(b"XRef")
                && let Some(r) = rank(at, &s.dict)
                && trailer.as_ref().is_none_or(|(best, _)| r > *best)
            {
                trailer = Some((r, s.dict));
            }
        }
        let trailer = match trailer {
            Some((_, t)) => t,
            None => {
                let catalog = self
                    .entries
                    .object_numbers(&self.data)
                    .into_iter()
                    .find(|n| matches!(self.try_get(*n).as_deref(), Ok(Object::Dict(d)) if d.name(b"Type") == Some(b"Catalog")));
                let Some(c) = catalog else { return Err(CosError::Syntax { offset: 0, detail: "no document catalog found".into() }) };
                let mut t = Dict::new();
                t.set(b"Root".to_vec(), Object::Ref(ObjRef::new(c, self.generation(c))));
                t
            }
        };
        let mut trailer = trailer;
        // What a cross-reference stream's dictionary carries for itself goes, as the chain
        // readers drop it: a save writes its own.
        for k in [&b"Prev"[..], b"XRefStm", b"Type", b"W", b"Index", b"Filter", b"DecodeParms", b"Length"] {
            trailer.remove(k);
        }
        self.trailer = trailer;
        self.revisions = Arc::new(Vec::new());
        log.push(format!("rebuilt cross-reference index from {object_count} object headers"));
        Ok(())
    }
}

/// For a classic table at `at` whose first subsection is `0 N` and opens with an in-use row
/// (the rows of objects 1 to N), the number N its rows really end at.
fn shifted_subsection_end(data: &[u8], at: usize) -> Option<u32> {
    let mut lx = Lexer::new(data, at);
    if !lx.eat_keyword(b"xref") {
        return None;
    }
    let (Ok(Object::Int(0)), Ok(Object::Int(count))) = (lx.object(), lx.object()) else { return None };
    lx.skip_ws();
    let offset = std::str::from_utf8(lx.token()).ok().and_then(|s| s.parse::<u64>().ok());
    lx.skip_ws();
    lx.token();
    lx.skip_ws();
    match (offset, lx.token()) {
        (Some(offset), b"n") if offset != 0 => u32::try_from(count).ok().filter(|n| *n > 0),
        _ => None,
    }
}

/// Every `N G obj` header in `data`, in file order, as (number, offset, generation, position
/// after `obj`).
fn object_headers(data: &[u8]) -> Vec<(u32, u64, u16, usize)> {
    let mut headers = Vec::new();
    let mut i = 0;
    while i < data.len() {
        // Object headers start at a line start (or after whitespace).
        let at_start = i == 0 || is_whitespace(data[i - 1]) || data[i - 1] == b'>';
        if at_start && data[i].is_ascii_digit() {
            let mut lx = Lexer::new(data, i);
            let n = lx.token();
            lx.skip_ws();
            let g = lx.token();
            if !n.is_empty() && !g.is_empty() && n.iter().all(u8::is_ascii_digit) && g.iter().all(u8::is_ascii_digit) && lx.eat_keyword(b"obj") {
                let num = std::str::from_utf8(n).ok().and_then(|s| s.parse::<u32>().ok());
                let generation = std::str::from_utf8(g).ok().and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
                if let Some(num) = num {
                    headers.push((num, i as u64, generation, lx.pos));
                }
                i = lx.pos;
                continue;
            }
        }
        i += 1;
    }
    headers
}

/// The offset of the section before the one whose trailer this is (`/Prev`), if any. Some writers
/// give it as a reference, `N 0 R`. When the sections read so far (`lookup`) place that object
/// and it holds an offset (iText's `indirectPrev.pdf`), that is the offset; otherwise, as the
/// tiff2pdf files in JHOVE's PDF-HUL-157 cases need, the sections before are at N. Either way the
/// reference is noted. Any other value that is not an offset is noted and ends the chain.
fn prev_offset(data: &[u8], trailer: &Dict, off: u64, lookup: impl Fn(u32) -> Option<XrefEntry>, log: &mut Vec<String>) -> Option<u64> {
    match trailer.get(b"Prev") {
        Some(Object::Int(p)) if *p >= 0 => Some(*p as u64),
        Some(Object::Ref(r)) => {
            let held = match lookup(r.num) {
                Some(XrefEntry::InFile { offset, generation }) if generation == r.generation => {
                    usize::try_from(offset).ok().and_then(|at| parse_indirect(data, at, &|_| None).ok()).and_then(|(at, value)| match value {
                        Object::Int(p) if at == *r && p >= 0 => Some(p as u64),
                        _ => None,
                    })
                }
                _ => None,
            };
            match held {
                Some(p) => {
                    log.push(format!(
                        "the trailer at {off} gives /Prev as the reference {} {} R; reading the offset {p} it holds",
                        r.num, r.generation
                    ));
                    Some(p)
                }
                None => {
                    log.push(format!(
                        "the trailer at {off} gives /Prev as the reference {} {} R; reading it as offset {}",
                        r.num, r.generation, r.num
                    ));
                    Some(u64::from(r.num))
                }
            }
        }
        Some(_) => {
            log.push(format!("the trailer at {off} has a /Prev that is not an offset; the sections before it are unreachable"));
            None
        }
        None => None,
    }
}

pub(crate) fn find(hay: &[u8], needle: &[u8], from: usize, to: usize) -> Option<usize> {
    let to = to.min(hay.len());
    if from >= to || needle.is_empty() || to - from < needle.len() {
        return None;
    }
    hay[from..to].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

fn rfind(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if hay.len() < needle.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).rev().find(|&i| &hay[i..i + needle.len()] == needle)
}

/// A file identifier derived from entropy supplied by the caller.
fn generated_id(seed: &[u8; 32]) -> Vec<u8> {
    use std::hash::{Hash, Hasher};
    let mut out = Vec::new();
    for i in 0..2u8 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (seed, i, b"pdfcraft id").hash(&mut h);
        out.extend_from_slice(&h.finish().to_be_bytes());
    }
    out
}

/// The object number a classic xref subsection really starts at. A common producer bug heads the
/// table `1 n` and then writes object 0's free entry (`0000000000 65535 f`) as its first row, so
/// every row would be read one number too high and object 1 as free. Such a subsection is read
/// from 0 when its second row, read that way, locates an object that is indeed numbered 1, at
/// that offset or, as `load` reads offsets, relative to a header shifted by `header_offset`.
/// `rows` is the position just after the subsection header.
fn subsection_start(data: &[u8], header_offset: usize, rows: usize, start: i64, count: i64) -> i64 {
    if start != 1 || count < 2 {
        return start;
    }
    let mut lx = Lexer::new(data, rows);
    let mut row = || {
        lx.skip_ws();
        let offset = std::str::from_utf8(lx.token()).ok().and_then(|s| s.parse::<u64>().ok());
        lx.skip_ws();
        let generation = std::str::from_utf8(lx.token()).ok().and_then(|s| s.parse::<u32>().ok());
        lx.skip_ws();
        (offset, generation, lx.token() == b"f")
    };
    let (Some(0), Some(65535), true) = row() else { return start };
    let (Some(second), _, false) = row() else { return start };
    let object_one = |at: usize| {
        if at >= data.len() {
            return false;
        }
        let mut obj = Lexer::new(data, at);
        matches!((obj.object(), obj.object()), (Ok(Object::Int(1)), Ok(Object::Int(_)))) && obj.eat_keyword(b"obj")
    };
    let Ok(second) = usize::try_from(second) else { return start };
    if object_one(second) || (header_offset != 0 && object_one(second.saturating_add(header_offset))) { 0 } else { start }
}

/// Object numbers must fit the representation used by references and the xref map.
/// Validate the whole subsection before adding entries, so neither arithmetic overflow nor
/// truncation can turn a damaged range into entries for unrelated objects.
fn valid_xref_range(start: i64, count: i64) -> bool {
    u32::try_from(start).is_ok() && count >= 0 && start.checked_add(count).is_some_and(|end| end <= i64::from(u32::MAX) + 1)
}

/// The most indirect objects a document can have (the classic implementation limit, ISO
/// 32000-1 Annex C): it bounds how much data a cross-reference stream can hold.
const MAX_XREF_OBJECTS: usize = 8_388_607;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_state_matches_a_clone_and_no_changed_document() {
        let mut doc = Document::new_empty();
        let a = doc.add(Object::Int(1));
        let state = doc.read_state();
        let snapshot = doc.clone();
        assert!(state.matches(&doc) && state.matches(&snapshot));
        // The same value set again is still a change: another allocation.
        doc.set(a, Object::Int(1));
        assert!(!state.matches(&doc) && state.matches(&snapshot));
        let edited = doc.read_state();
        doc.free(a);
        assert!(!edited.matches(&doc));
        let mut trailer_changed = snapshot.clone();
        trailer_changed.trailer_mut().set(b"Info".to_vec(), Object::Null);
        assert!(!state.matches(&trailer_changed));
        // Another document's state never matches, even with the same changes.
        let mut other = Document::new_empty();
        other.add(Object::Int(1));
        assert!(!other.read_state().matches(&snapshot));
    }

    /// Build a classic-xref file from object bodies (object i+1 = bodies[i]).
    pub(crate) fn build(bodies: &[&str], trailer: &str) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, b) in bodies.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{b}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} {trailer} >>\nstartxref\n{xref}\n%%EOF\n", bodies.len() + 1).as_bytes());
        out
    }

    fn assert_compact_matches_materialized(doc: &Document) {
        assert!(matches!(doc.entries.as_ref(), XrefIndex::Compact { .. }), "valid xrefs should not retain a per-entry map");
        let (entries, _, revisions, _) = doc.read_xref_chain_materialized(&mut Vec::new()).unwrap();
        assert_eq!(revisions.as_slice(), doc.revisions());
        let max = entries.keys().next_back().copied().unwrap_or(0);
        assert_eq!(doc.entries.max_num(), max);
        for num in 0..=max {
            assert_eq!(doc.xref_entry(num), entries.get(&num).copied(), "object {num}");
        }
        let expected: Vec<_> = entries.iter().filter_map(|(&num, entry)| (!matches!(entry, XrefEntry::Free { .. })).then_some(num)).collect();
        assert_eq!(doc.object_numbers(), expected);
    }

    #[test]
    fn compact_xref_tables_streams_and_revisions_match_the_materialized_reader() {
        let classic = Document::open(Arc::new(build(
            &["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", "<< /Value 9 >>"],
            "/Root 1 0 R",
        )))
        .unwrap();
        assert_compact_matches_materialized(&classic);

        let mut sparse = b"%PDF-1.7\n".to_vec();
        let root_offset = sparse.len();
        sparse.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 5 0 R >>\nendobj\n");
        let pages_offset = sparse.len();
        sparse.extend_from_slice(b"5 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
        let xref = sparse.len();
        sparse.extend_from_slice(
            format!(
                "xref\n0 1\n0000000000 65535 f \n1 1\n{root_offset:010} 00000 n \n5 1\n{pages_offset:010} 00000 n \ntrailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        let sparse = Document::open(Arc::new(sparse)).unwrap();
        assert_eq!(sparse.object_numbers(), vec![1, 5]);
        assert_compact_matches_materialized(&sparse);

        let mut sparse_stream = b"%PDF-1.7\n".to_vec();
        let mut offsets = [0u32; 8];
        offsets[1] = sparse_stream.len() as u32;
        sparse_stream.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 5 0 R >>\nendobj\n");
        offsets[5] = sparse_stream.len() as u32;
        sparse_stream.extend_from_slice(b"5 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
        offsets[6] = sparse_stream.len() as u32;
        sparse_stream.extend_from_slice(b"6 0 obj\n<< /Unused true >>\nendobj\n");
        offsets[7] = sparse_stream.len() as u32;
        let xref_row = |kind: u8, offset: u32, generation: u16| {
            let mut row = Vec::with_capacity(7);
            row.push(kind);
            row.extend_from_slice(&offset.to_be_bytes());
            row.extend_from_slice(&generation.to_be_bytes());
            row
        };
        let mut rows = Vec::new();
        rows.extend(xref_row(0, 0, u16::MAX));
        rows.extend(xref_row(1, offsets[1], 0));
        rows.extend(xref_row(1, offsets[5], 0));
        rows.extend(xref_row(1, offsets[6], 0));
        rows.extend(xref_row(1, offsets[7], 0));
        sparse_stream.extend_from_slice(
            format!("7 0 obj\n<< /Type /XRef /Size 8 /Root 1 0 R /W [1 4 2] /Index [0 2 5 3] /Length {} >>\nstream\n", rows.len()).as_bytes(),
        );
        sparse_stream.extend_from_slice(&rows);
        sparse_stream.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{}\n%%EOF\n", offsets[7]).as_bytes());
        let sparse_stream = Document::open(Arc::new(sparse_stream)).unwrap();
        assert_eq!(sparse_stream.object_numbers(), vec![1, 5, 6, 7]);
        assert_compact_matches_materialized(&sparse_stream);

        let stream_bytes = crate::write_full(&classic, &crate::SaveOptions::default()).unwrap();
        let stream = Document::open(Arc::new(stream_bytes)).unwrap();
        assert!(stream.revisions().last().is_some_and(|revision| revision.is_stream));
        assert_compact_matches_materialized(&stream);

        let mut edited = classic.clone();
        edited.update_dict(ObjRef::new(1, 0), |dict| dict.set(b"Lang".to_vec(), Object::String(crate::PdfString::literal("en")))).unwrap();
        edited.free(ObjRef::new(3, 0));
        let incremental = crate::write_incremental(&edited, &crate::SaveOptions::default()).unwrap();
        let updated = Document::open(Arc::new(incremental)).unwrap();
        assert_eq!(updated.revisions().len(), 2);
        assert_compact_matches_materialized(&updated);
    }

    #[test]
    fn malformed_size_uses_the_materialized_compatibility_path() {
        let mut bytes = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", "<< /Value 9 >>"], "/Root 1 0 R");
        let marker = b"/Size 4";
        let at = bytes.windows(marker.len()).position(|window| window == marker).unwrap();
        bytes[at..at + marker.len()].copy_from_slice(b"/Size 2");
        let doc = Document::open(Arc::new(bytes)).unwrap();
        assert!(matches!(doc.entries.as_ref(), XrefIndex::Materialized(_)));
        assert_eq!(doc.next_num, 4);
        assert_eq!(doc.object_numbers(), vec![1, 2, 3]);
    }

    /// Entries the reader sets aside are remembered, so a caller comparing revisions can tell
    /// that another reader may resolve the object differently.
    #[test]
    fn cross_reference_entries_that_are_set_aside_are_recorded() {
        let base = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", "(three)"], "/Root 1 0 R");
        let plain = Document::open(Arc::new(base.clone())).unwrap();
        assert!(plain.xref_oddities().is_empty());
        assert!((1..=3).all(|n| plain.xref_entry_is_exact(n)));
        let prev = String::from_utf8_lossy(&base).rfind("xref\n0 4").unwrap();
        let third = plain.xref_entry(3).unwrap();

        // A later section: object 3 "in use" at offset 0, object 2 listed twice with different
        // offsets, and object 0 at offset 0 (the usual producer slip, which names no object).
        let mut bytes = base.clone();
        let section = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 1\n0000000000 65535 n \n3 1\n0000000000 00000 n \n2 1\n0000000058 00000 n \n2 1\n0000000009 00000 n \ntrailer\n<< /Size 4 /Root 1 0 R /Prev {prev} >>\nstartxref\n{section}\n%%EOF\n").as_bytes(),
        );
        let doc = Document::open(Arc::new(bytes)).unwrap();
        assert_eq!(doc.xref_oddities(), [(section as u64, 3), (section as u64, 2)]);
        // What the reader resolves is what it resolved before: the first entry it kept.
        assert_eq!(doc.xref_entry(3), Some(third));
        assert_eq!(doc.xref_entry(2), Some(XrefEntry::InFile { offset: 58, generation: 0 }));
    }

    /// An entry that misses its object still resolves (by searching) but is not exact.
    #[test]
    fn an_entry_that_misses_its_object_is_found_by_searching_but_is_not_exact() {
        let base = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", "(three)"], "/Root 1 0 R");
        let text = String::from_utf8_lossy(&base).into_owned();
        let at = text.find("3 0 obj").unwrap();
        let bytes = text.replacen(&format!("{at:010} 00000 n"), &format!("{:010} 00000 n", at + 2), 1).into_bytes();
        let doc = Document::open(Arc::new(bytes)).unwrap();
        assert_eq!(doc.get(ObjRef { num: 3, generation: 0 }).as_string().map(|s| s.bytes.clone()), Some(b"three".to_vec()));
        assert!(!doc.xref_entry_is_exact(3));
        assert!(doc.xref_entry_is_exact(1) && doc.xref_entry_is_exact(2));
    }

    /// A classic table with `/Size` rows and object 0's free entry first, its subsection numbered
    /// `1 n` instead of `0 n` (#2676): every row is one number off.
    fn misnumbered_table(start: u32) -> Vec<u8> {
        let bodies = ["<< /Marker (first object) >>", "<< /Type /Pages /Kids [] /Count 0 >>", "<< /Type /Catalog /Pages 2 0 R >>"];
        let mut out = b"%PDF-1.7
"
        .to_vec();
        let mut offsets = Vec::new();
        for (i, body) in bodies.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(
                format!(
                    "{} 0 obj
{body}
endobj
",
                    i + 1
                )
                .as_bytes(),
            );
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n{start} 4\n0000000000 65535 f \n").as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size 4 /Root 3 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
        out
    }

    #[test]
    fn a_table_numbered_from_1_that_begins_with_object_0_is_read_from_0() {
        let doc = Document::open(Arc::new(misnumbered_table(1))).unwrap();
        let first = doc.get(ObjRef::new(1, 0)).as_dict().cloned().expect("object 1 is not free");
        assert!(first.contains(b"Marker"));
        assert_eq!(doc.root(), Some(ObjRef::new(3, 0)));
        assert!(doc.repair_log().iter().any(|l| l.contains("numbered from 1")), "{:?}", doc.repair_log());
        // A correct table reads the same and logs nothing.
        let fine = Document::open(Arc::new(misnumbered_table(0))).unwrap();
        assert!(fine.get(ObjRef::new(1, 0)).as_dict().is_some_and(|d| d.contains(b"Marker")));
        assert!(!fine.repair_log().iter().any(|l| l.contains("numbered from 1")));
    }

    #[test]
    fn a_table_that_really_starts_at_1_keeps_its_numbers() {
        // Object 1 deleted for good (generation 65535) in a subsection that does start at 1: its
        // second row locates object 2, so nothing is shifted.
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let pages = bytes.len();
        bytes.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
        let catalog = bytes.len();
        bytes.extend_from_slice(b"3 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!(
                "xref\n0 1\n0000000000 65535 f \n1 3\n0000000000 65535 f \n{pages:010} 00000 n \n{catalog:010} 00000 n \ntrailer\n<< /Size 4 /Root 3 0 R >>\nstartxref\n{xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        let doc = Document::open(Arc::new(bytes)).unwrap();
        assert!(doc.get(ObjRef::new(2, 0)).as_dict().is_some_and(|d| d.name(b"Type") == Some(b"Pages")));
        assert!(doc.get(ObjRef::new(1, 0)).as_dict().is_none(), "object 1 stays free");
        assert!(!doc.repair_log().iter().any(|l| l.contains("numbered from 1")));
    }

    /// A one-page scan: the page draws image 5 with content stream 4.
    fn scanned_page() -> Vec<u8> {
        build(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>",
                "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>",
                "<< /Length 21 >>\nstream\n10 0 0 10 0 0 cm /Im0 Do\nendstream",
                "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\nA\nendstream",
            ],
            "/Root 1 0 R",
        )
    }

    /// `bytes` with its cross-reference table replaced by `table` (the lines after `xref`).
    fn with_table(bytes: &[u8], table: &[String], size: usize) -> Vec<u8> {
        let text = String::from_utf8_lossy(bytes).into_owned();
        let at = text.rfind("\nxref\n").unwrap() + 1;
        let mut out = bytes[..at].to_vec();
        out.extend_from_slice(format!("xref\n{}\ntrailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{at}\n%%EOF\n", table.join("\n")).as_bytes());
        out
    }

    /// The table's entry lines, object 0 first.
    fn table_entries(bytes: &[u8]) -> Vec<String> {
        let text = String::from_utf8_lossy(bytes).into_owned();
        let at = text.rfind("\nxref\n").unwrap() + 1;
        text[at..].lines().skip(2).take_while(|l| !l.starts_with("trailer")).map(str::to_string).collect()
    }

    /// The classic off-by-one: the subsection says it starts at 0 but lists objects 1 to 5, so
    /// the last object has no entry. Viewers rebuild the table and show the page; a reader that
    /// took the missing entry for a null object dropped the image when pages were copied (#1628).
    #[test]
    fn an_off_by_one_xref_subsection_still_finds_the_last_object() {
        let base = scanned_page();
        let entries = table_entries(&base);
        let mut table = vec!["0 5".to_string()];
        table.extend(entries[1..6].iter().cloned());
        let doc = Document::open(Arc::new(with_table(&base, &table, 5))).unwrap();
        let image = doc.get(ObjRef::new(5, 0));
        assert_eq!(image.as_dict().and_then(|d| d.name(b"Subtype")), Some(&b"Image"[..]), "{image:?}");
        assert!(matches!(&*image, Object::Stream(_)));
        assert!(matches!(&*doc.get(ObjRef::new(4, 0)), Object::Stream(s) if &s.raw[..] == b"10 0 0 10 0 0 cm /Im0 Do"));
        assert!(doc.repair_log().iter().any(|l| l.contains("object headers")), "{:?}", doc.repair_log());
    }

    /// An "in use" entry at offset 0 says nothing about where the object is: the object is
    /// found by its header, and the repair is recorded.
    #[test]
    fn an_object_listed_at_offset_zero_is_found_by_its_header() {
        let base = scanned_page();
        for num in [4usize, 5] {
            let mut table = vec!["0 6".to_string()];
            table.extend(table_entries(&base));
            table[num + 1] = "0000000000 00000 n ".into();
            let doc = Document::open(Arc::new(with_table(&base, &table, 6))).unwrap();
            assert!(matches!(&*doc.get(ObjRef::new(num as u32, 0)), Object::Stream(_)), "object {num}");
            assert!(doc.repair_log().iter().any(|l| l.contains("object headers")), "object {num}: {:?}", doc.repair_log());
        }
    }

    /// A valid table that simply does not list every number below `/Size` is not repaired: an
    /// unlisted number is a null object (§7.3.10), even when an old body with that number is
    /// still in the file.
    #[test]
    fn a_consistent_table_with_unlisted_numbers_is_taken_as_it_is() {
        let base = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", "(three)"], "/Root 1 0 R");
        let at = String::from_utf8_lossy(&base).rfind("\nxref\n").unwrap() + 1;
        let mut bytes = base[..at].to_vec();
        bytes.extend_from_slice(b"4 0 obj\n(stale)\nendobj\n");
        let mut table = vec!["0 4".to_string()];
        table.extend(table_entries(&base));
        let doc = Document::open(Arc::new(with_table(&[&bytes[..], &base[at..]].concat(), &table, 6))).unwrap();
        assert!(doc.repair_log().is_empty(), "{:?}", doc.repair_log());
        assert_eq!(*doc.get(ObjRef::new(4, 0)), Object::Null);
        assert!(matches!(&*doc.get(ObjRef::new(3, 0)), Object::String(_)));
    }

    /// Filling the gaps of a damaged table never brings back an object that an incremental
    /// update freed: its body is still in the file, but the newer section says it is gone.
    #[test]
    fn an_object_freed_by_an_update_stays_freed_when_the_table_is_repaired() {
        let base = scanned_page();
        // The original table lists the content stream at offset 0, so the gaps get filled.
        let mut table = vec!["0 6".to_string()];
        table.extend(table_entries(&base));
        table[5] = "0000000000 00000 n ".into();
        let mut bytes = with_table(&base, &table, 6);
        let prev = String::from_utf8_lossy(&bytes).rfind("\nxref\n").unwrap() + 1;
        let section = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n5 1\n0000000000 00001 f \ntrailer\n<< /Size 6 /Root 1 0 R /Prev {prev} >>\nstartxref\n{section}\n%%EOF\n").as_bytes(),
        );
        let doc = Document::open(Arc::new(bytes)).unwrap();
        assert!(matches!(&*doc.get(ObjRef::new(4, 0)), Object::Stream(_)), "the gap is filled");
        assert_eq!(*doc.get(ObjRef::new(5, 0)), Object::Null, "the freed image stays freed");
        assert_eq!(doc.xref_entry(5), Some(XrefEntry::Free { next_generation: 1 }));
    }

    /// Repairing a damaged table fills only the numbers the damage leaves out: an old body of a
    /// number no section lists, under an oversized `/Size`, stays a null object.
    #[test]
    fn a_repaired_table_leaves_an_unlisted_number_null() {
        let base = scanned_page();
        let at = String::from_utf8_lossy(&base).rfind("\nxref\n").unwrap() + 1;
        let entries = table_entries(&base);
        let mut offset_zero = vec!["0 6".to_string()];
        offset_zero.extend(entries.iter().cloned());
        offset_zero[5] = "0000000000 00000 n ".into();
        let mut off_by_one = vec!["0 5".to_string()];
        off_by_one.extend(entries[1..6].iter().cloned());
        for (name, table, found) in [("offset 0", offset_zero, 4), ("off by one", off_by_one, 5)] {
            let bytes = [&base[..at], b"7 0 obj\n(stale)\nendobj\n", &base[at..]].concat();
            let doc = Document::open(Arc::new(with_table(&bytes, &table, 9))).unwrap();
            assert!(matches!(&*doc.get(ObjRef::new(found, 0)), Object::Stream(_)), "{name}: the gap is filled");
            assert_eq!(*doc.get(ObjRef::new(7, 0)), Object::Null, "{name}: {:?}", doc.repair_log());
            assert_eq!(doc.xref_entry(7), None, "{name}");
        }
    }

    /// The last object of a subsection numbered one too low is filled from that subsection,
    /// whatever later happens to the row before it: an incremental save that rewrites object 4,
    /// or object 4's own row at offset 0.
    #[test]
    fn the_last_object_of_a_shifted_subsection_is_filled_whatever_its_neighbour_says() {
        let base = scanned_page();
        let entries = table_entries(&base);
        let mut off_by_one = vec!["0 5".to_string()];
        off_by_one.extend(entries[1..6].iter().cloned());
        let shifted = with_table(&base, &off_by_one, 5);
        let prev = String::from_utf8_lossy(&shifted).rfind("\nxref\n").unwrap() + 1;
        let mut updated = shifted.clone();
        let body = updated.len();
        updated.extend_from_slice(b"4 0 obj\n<< /Length 21 >>\nstream\n10 0 0 10 0 0 cm /Im0 Do\nendstream\nendobj\n");
        let section = updated.len();
        updated.extend_from_slice(
            format!("xref\n4 1\n{body:010} 00000 n \ntrailer\n<< /Size 6 /Root 1 0 R /Prev {prev} >>\nstartxref\n{section}\n%%EOF\n").as_bytes(),
        );
        let mut neighbour_at_zero = off_by_one.clone();
        neighbour_at_zero[5] = "0000000000 00000 n ".into();
        for (name, bytes) in [("updated", updated), ("row 4 at offset 0", with_table(&base, &neighbour_at_zero, 5))] {
            let doc = Document::open(Arc::new(bytes)).unwrap();
            let image = doc.get(ObjRef::new(5, 0));
            assert_eq!(image.as_dict().and_then(|d| d.name(b"Subtype")), Some(&b"Image"[..]), "{name}: {:?}", doc.repair_log());
        }
    }

    /// The image the first page draws as `/Im1`, followed through the page tree.
    fn first_page_image(doc: &Document) -> Arc<Object> {
        let catalog = doc.get(doc.root().unwrap());
        let pages = doc.resolve(catalog.as_dict().unwrap().get(b"Pages").unwrap());
        let page = doc.resolve(&pages.as_dict().unwrap().get(b"Kids").unwrap().as_array().unwrap()[0]);
        let resources = doc.resolve(page.as_dict().unwrap().get(b"Resources").unwrap());
        let xobjects = doc.resolve(resources.as_dict().unwrap().get(b"XObject").unwrap());
        doc.resolve(xobjects.as_dict().unwrap().get(b"Im1").unwrap())
    }

    /// The opposite off-by-one (#1628): the subsection says it starts at 1, but its first entry
    /// is object 0's free-list head, so every entry lands one number too high and object 1, the
    /// image, reads as free. The offset the table gives object 2 holds `1 0 obj`, so the
    /// subsection is read from 0, and the image survives a full save.
    #[test]
    fn a_subsection_from_one_that_opens_with_the_free_list_head_is_read_from_zero() {
        let base = build(
            &[
                "<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 12 >>\nstream\nABCDEFGHIJKL\nendstream",
                "<< /Length 29 >>\nstream\nq 72 0 0 72 0 0 cm /Im1 Do Q\nendstream",
                "<< /Type /Page /Parent 4 0 R /MediaBox [0 0 72 72] /Resources << /XObject << /Im1 1 0 R >> >> /Contents 2 0 R >>",
                "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
                "<< /Type /Catalog /Pages 4 0 R >>",
                "<< /Subject (synthetic) >>",
            ],
            "/Root 5 0 R /Info 6 0 R",
        );
        // /Size 7 is one short of the shifted numbers; /Size 8 covers them all. Junk before the
        // header moves every object past the offsets the table gives; it is not a comment, which
        // the lexer would skip on its way to `1 0 obj` anyway.
        for (size, junk) in [(7, ""), (8, ""), (7, "Content-Type: application/pdf\n\n")] {
            let text = String::from_utf8_lossy(&base).replace("\nxref\n0 7\n", "\nxref\n1 7\n").replace("/Size 7 ", &format!("/Size {size} "));
            let doc = Document::open(Arc::new(format!("{junk}{text}").into_bytes())).unwrap();
            let image = first_page_image(&doc);
            assert_eq!(image.as_dict().and_then(|d| d.name(b"Subtype")), Some(&b"Image"[..]), "/Size {size} {junk:?}: {image:?}");
            assert_eq!(doc.xref_entry(0), Some(XrefEntry::Free { next_generation: 65535 }), "/Size {size} {junk:?}");
            assert!(doc.repair_log().iter().any(|l| l.contains("from 0")), "/Size {size} {junk:?}: {:?}", doc.repair_log());
            let saved = Document::open(Arc::new(crate::write_full(&doc, &crate::SaveOptions::default()).unwrap())).unwrap();
            let image = first_page_image(&saved);
            assert!(matches!(&*image, Object::Stream(s) if &s.raw[..] == b"ABCDEFGHIJKL"), "/Size {size} {junk:?}: {image:?}");
        }
    }

    /// Object 1 really freed: the subsection from 1 opens with a free entry, but the next entry's
    /// offset holds object 2, not object 1. The table is taken as it is, and the old body of
    /// object 1 still in the file stays freed.
    #[test]
    fn a_subsection_from_one_whose_next_offset_holds_object_two_is_taken_as_it_is() {
        let base = build(&["(freed)", "<< /Type /Catalog /Pages 3 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>"], "/Root 2 0 R");
        let entries = table_entries(&base);
        let mut table = vec!["1 3".to_string(), "0000000000 65535 f ".to_string()];
        table.extend(entries[2..].iter().cloned());
        let text = String::from_utf8_lossy(&base).into_owned();
        let at = text.rfind("\nxref\n").unwrap() + 1;
        for junk in ["", "Content-Type: application/pdf\n\n"] {
            let mut bytes = junk.as_bytes().to_vec();
            bytes.extend_from_slice(&base[..at]);
            bytes.extend_from_slice(format!("xref\n{}\ntrailer\n<< /Size 4 /Root 2 0 R >>\nstartxref\n{at}\n%%EOF\n", table.join("\n")).as_bytes());
            let doc = Document::open(Arc::new(bytes)).unwrap();
            assert_eq!(doc.xref_entry(1), Some(XrefEntry::Free { next_generation: 65535 }), "{junk:?}");
            assert_eq!(*doc.get(ObjRef::new(1, 0)), Object::Null, "{junk:?}: the freed object stays freed");
            assert!(matches!(&*doc.get(ObjRef::new(2, 0)), Object::Dict(d) if d.name(b"Type") == Some(&b"Catalog"[..])), "{junk:?}");
            assert!(!doc.repair_log().iter().any(|l| l.contains("from 0")), "{junk:?}: {:?}", doc.repair_log());
            assert!(!junk.is_empty() || doc.repair_log().is_empty(), "{:?}", doc.repair_log());
        }
    }

    #[test]
    fn opening_an_exhausted_classic_xref_repairs_and_allocates() {
        // The extra in-use row need not be loaded to exhaust the next object number.
        let bytes = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\nxref\n0 3\n0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n4294967295 1\n0000000058 00000 n \ntrailer\n<< /Size 3 /Root 1 0 R >>\nstartxref\n110\n%%EOF\n";
        assert_eq!(bytes.len(), 266);
        let mut doc = Document::open(Arc::new(bytes.to_vec())).unwrap();
        assert_eq!(doc.root(), Some(ObjRef::new(1, 0)));
        assert!(doc.repair_log().iter().any(|line| line.contains("exhausted xref")));
        assert_eq!(doc.add(Object::Null), ObjRef::new(3, 0));
    }

    #[test]
    fn opening_an_exhausted_xref_stream_repairs_and_allocates() {
        let mut bytes =
            b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n".to_vec();
        let offset = bytes.len();
        bytes.extend_from_slice(b"3 0 obj\n<< /Type /XRef /Size 4 /Root 1 0 R /W [1 4 2] /Index [0 3 4294967295 1] /Length 28 >>\nstream\n");
        for (kind, position, generation) in [(0_u8, 0_u32, 65535_u16), (1, 9, 0), (1, 58, 0), (1, 58, 0)] {
            bytes.push(kind);
            bytes.extend_from_slice(&position.to_be_bytes());
            bytes.extend_from_slice(&generation.to_be_bytes());
        }
        bytes.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{offset}\n%%EOF\n").as_bytes());
        let mut doc = Document::open(Arc::new(bytes)).unwrap();
        assert_eq!(doc.root(), Some(ObjRef::new(1, 0)));
        assert!(doc.repair_log().iter().any(|line| line.contains("exhausted xref")));
        assert_eq!(doc.add(Object::Null), ObjRef::new(4, 0));
    }

    #[test]
    fn exhausted_size_and_high_edits_allocate_without_overwriting_objects() {
        let ordinary = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>"], "/Root 1 0 R");
        for size in ["4294967295", "4294967296", "9223372036854775807"] {
            let bytes = String::from_utf8(ordinary.clone()).unwrap().replace("/Size 3", &format!("/Size {size}"));
            let mut doc = Document::open(Arc::new(bytes.into_bytes())).unwrap();
            assert!(doc.repair_log().iter().any(|line| line.contains("exhausted trailer")));
            assert_eq!(doc.add(Object::Int(42)), ObjRef::new(3, 0));
            doc.set(ObjRef::new(u32::MAX, 0), Object::Int(99));
            doc.set(ObjRef::new(u32::MAX - 1, 0), Object::Int(98));
            assert_eq!(doc.add(Object::Int(43)), ObjRef::new(4, 0));
            assert_eq!(doc.get(ObjRef::new(3, 0)).as_int(), Some(42));
            assert_eq!(doc.get(ObjRef::new(u32::MAX, 0)).as_int(), Some(99));
            assert_eq!(doc.get(ObjRef::new(1, 0)).as_dict().unwrap().name(b"Type"), Some(b"Catalog".as_slice()));
        }
        let mut doc = Document::new_empty();
        doc.set(ObjRef::new(u32::MAX - 2, 0), Object::Null);
        assert_eq!(doc.add(Object::Null), ObjRef::new(u32::MAX - 1, 0));
        assert_eq!(doc.add(Object::Null), ObjRef::new(3, 0));
        assert_eq!(doc.add(Object::Null), ObjRef::new(4, 0));
    }

    #[test]
    fn exhausted_xref_controls_preserve_ordinary_ignored_and_repaired_entries() {
        let ordinary = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>"], "/Root 1 0 R");
        for (extra, repaired) in
            [("", false), ("4294967295 1\n0000000000 00000 n \n", false), ("4294967295 2\n0000000058 00000 n \n0000000058 00000 n \n", true)]
        {
            let bytes = String::from_utf8(ordinary.clone()).unwrap().replacen("trailer\n", &format!("{extra}trailer\n"), 1);
            let mut doc = Document::open(Arc::new(bytes.into_bytes())).unwrap();
            assert_eq!(doc.root(), Some(ObjRef::new(1, 0)));
            assert_eq!(doc.get(ObjRef::new(1, 0)).as_dict().unwrap().name(b"Type"), Some(b"Catalog".as_slice()));
            assert_eq!(!doc.repair_log().is_empty(), repaired);
            if repaired {
                assert!(doc.repair_log().iter().any(|line| line.contains("xref") && line.contains("range")));
            }
            assert_eq!(doc.add(Object::Null), ObjRef::new(3, 0));
        }
    }

    #[test]
    fn invalid_xref_object_ranges_are_reconstructed() {
        // Original in-memory PDF: an intact catalog followed by a damaged xref section.
        for stream in [false, true] {
            for (start, count) in [(i64::MAX, 2), (-1, 2), (4_294_967_296, 1), (4_294_967_295, 2)] {
                let mut bytes =
                    b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n".to_vec();
                let offset = bytes.len();
                if stream {
                    bytes.extend_from_slice(
                        format!("3 0 obj\n<< /Type /XRef /Root 1 0 R /Size 4 /W [1 1 1] /Index [{start} {count}] /Length 6 >>\nstream\n").as_bytes(),
                    );
                    bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
                    bytes.extend_from_slice(b"\nendstream\nendobj\n");
                } else {
                    bytes.extend_from_slice(
                        format!("xref\n{start} {count}\n0000000000 65535 f \n0000000000 65535 f \ntrailer\n<< /Root 1 0 R /Size 3 >>\n").as_bytes(),
                    );
                }
                bytes.extend_from_slice(format!("startxref\n{offset}\n%%EOF\n").as_bytes());
                let doc = Document::open(Arc::new(bytes)).unwrap();
                assert!(
                    doc.repair_log().iter().any(|line| line.contains("xref") && line.contains("range")),
                    "stream={stream}, start={start}, count={count}: {:?}",
                    doc.repair_log()
                );
                assert_eq!(doc.get(ObjRef::new(1, 0)).as_dict().unwrap().name(b"Type"), Some(b"Catalog".as_slice()));
            }
        }
    }

    #[test]
    fn objects_only_an_unreadable_older_section_locates_are_found_by_scanning() {
        // The newest section lists the catalog only; its /Prev points past the end of the file,
        // so the page tree is reachable only by scanning object headers, as readers do (#1656).
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (num, body) in
            [(1, "<< /Type /Catalog /Pages 2 0 R >>"), (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"), (3, "<< /Type /Page /Parent 2 0 R >>")]
        {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{num} 0 obj\n{body}\nendobj\n").as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!(
                "xref\n0 2\n0000000000 65535 f \n{:010} 00000 n \ntrailer\n<< /Root 1 0 R /Size 4 /Prev 999999 >>\nstartxref\n{xref}\n%%EOF\n",
                offsets[0]
            )
            .as_bytes(),
        );
        let doc = Document::open(Arc::new(bytes)).unwrap();
        assert_eq!(doc.get(ObjRef::new(2, 0)).as_dict().and_then(|d| d.name(b"Type")), Some(b"Pages".as_slice()));
        assert_eq!(doc.get(ObjRef::new(3, 0)).as_dict().and_then(|d| d.name(b"Type")), Some(b"Page".as_slice()));
        assert!(doc.repair_log().iter().any(|l| l.contains("999999")), "{:?}", doc.repair_log());
    }

    #[test]
    fn self_referential_length_does_not_overflow() {
        let bytes = build(
            &["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", "<< /Length 3 0 R >>\nstream\nabc\nendstream"],
            "/Root 1 0 R",
        );
        let doc = Document::open(Arc::new(bytes)).unwrap();
        // Either recovered via the endstream search or reported — but never a stack overflow.
        let o = doc.get(ObjRef::new(3, 0));
        if let Object::Stream(s) = o.as_ref() {
            assert_eq!(&s.raw[..], b"abc");
        }
    }

    #[test]
    fn mutually_referential_lengths_do_not_overflow() {
        let bytes = build(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>",
                "<< /Type /Pages /Kids [] /Count 0 >>",
                "<< /Length 4 0 R >>\nstream\nabc\nendstream",
                "<< /Length 3 0 R >>\nstream\nxyz\nendstream",
            ],
            "/Root 1 0 R",
        );
        let doc = Document::open(Arc::new(bytes)).unwrap();
        let _ = doc.get(ObjRef::new(3, 0));
        let _ = doc.get(ObjRef::new(4, 0));
    }

    #[test]
    fn reads_classic_revision_and_edits_are_copy_on_write() {
        let bytes = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>"], "/Root 1 0 R");
        let doc = Document::open(Arc::new(bytes)).unwrap();
        assert_eq!(doc.revisions().len(), 1);
        assert!(doc.repair_log().is_empty());
        let mut edited = doc.clone();
        edited.update_dict(ObjRef::new(1, 0), |d| d.set(b"Lang".to_vec(), Object::String(crate::PdfString::literal("en")))).unwrap();
        assert!(edited.is_modified() && !doc.is_modified(), "clone is an independent snapshot");
        assert!(doc.get(ObjRef::new(1, 0)).as_dict().unwrap().get(b"Lang").is_none());
    }

    #[test]
    fn object_numbers_merge_source_and_overlay_in_order() {
        let mut doc = Document::new_empty();
        doc.overlay.clear();
        doc.entries = Arc::new(XrefIndex::Materialized(Arc::new(BTreeMap::from([
            (0, XrefEntry::Free { next_generation: 65535 }),
            (2, XrefEntry::InFile { offset: 10, generation: 7 }),
            (4, XrefEntry::Free { next_generation: 9 }),
            (6, XrefEntry::InStream { stream: 80, index: 1 }),
            (8, XrefEntry::InFile { offset: 20, generation: 5 }),
            (10, XrefEntry::InFile { offset: 30, generation: 3 }),
            (u32::MAX, XrefEntry::InFile { offset: 40, generation: 1 }),
        ]))));
        doc.set(ObjRef::new(1, 2), Object::Int(1)); // before the first live source slot
        doc.set(ObjRef::new(2, 8), Object::Int(2)); // override, without duplication
        doc.set(ObjRef::new(4, 9), Object::Int(4)); // reuse a source free slot
        doc.free(ObjRef::new(6, 0)); // remove a compressed source object
        doc.free(ObjRef::new(7, 2)); // free a number absent from the source
        doc.free(ObjRef::new(10, 3));
        doc.set(ObjRef::new(11, 4), Object::Int(11));
        assert_eq!(doc.object_numbers(), vec![1, 2, 4, 8, 11, u32::MAX]);
        assert_eq!(doc.generation(2), 8);
        assert_eq!(doc.generation(4), 9);
        assert_eq!(doc.generation(6), 1);
        assert_eq!(doc.generation(8), 5);
        assert_eq!(doc.generation(10), 4);
        doc.overlay.clear();
        assert_eq!(doc.object_numbers(), vec![2, 6, 8, 10, u32::MAX]);
        doc.entries = Arc::new(XrefIndex::empty());
        assert!(doc.object_numbers().is_empty());
        doc.set(ObjRef::new(5, 7), Object::Int(5));
        assert_eq!(doc.object_numbers(), vec![5]);
    }

    #[test]
    fn full_object_scans_preserve_edits_without_retaining_the_scan() {
        let mut bodies = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_owned(), "<< /Type /Pages /Kids [] /Count 0 >>".to_owned()];
        bodies.extend((0..400).map(|i| format!("<< /Value {i} >>")));
        let refs: Vec<_> = bodies.iter().map(String::as_str).collect();
        let mut doc = Document::open(Arc::new(build(&refs, "/Root 1 0 R"))).unwrap();
        doc.set(ObjRef::new(3, 0), Object::Int(900));
        doc.free(ObjRef::new(4, 0));
        let added = doc.add(Object::Int(901));
        let cached = doc.cache.lock().unwrap().len();
        let held: Vec<_> = doc.scan_objects().collect();
        assert_eq!(held.len(), 402);
        assert_eq!(doc.cache.lock().unwrap().len(), cached, "scanning must not populate the editor cache");
        assert!(doc.objstms.lock().unwrap().is_empty());
        assert!(!held.iter().any(|(r, _)| r.num == 4));
        assert_eq!(held.iter().find(|(r, _)| r.num == 3).unwrap().1.as_int(), Some(900));
        assert_eq!(held.iter().find(|(r, _)| *r == added).unwrap().1.as_int(), Some(901));
        assert_eq!(held.iter().find(|(r, _)| r.num == 400).unwrap().1.as_dict().unwrap().int(b"Value"), Some(397));
        // Previously returned objects survive cache eviction and dropping the source.
        drop(doc);
        assert_eq!(held.iter().find(|(r, _)| r.num == 128).unwrap().1.as_dict().unwrap().int(b"Value"), Some(125));
    }

    #[test]
    fn temporary_reader_keeps_file_views_and_evicts_data_of_its_own() {
        // A stream read from the file is a view into the input: caching it costs no copy, so
        // even a large one stays cached (its /Length object too) instead of being re-parsed.
        let body = "x".repeat(ObjectReader::STREAM_BYTES + 1);
        let stream = format!("<< /Length 4 0 R >>\nstream\n{body}\nendstream");
        let length = body.len().to_string();
        let bytes = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", &stream, &length], "/Root 1 0 R");
        let doc = Document::open(Arc::new(bytes)).unwrap();
        let mut reader = doc.object_reader();
        let read = reader.get(ObjRef::new(3, 0));
        let Object::Stream(s) = &*read else { panic!("stream") };
        assert!(s.raw.shares(&reader.document.data), "no copy of the file's bytes");
        assert!(reader.document.cache.lock().unwrap().contains_key(&3), "a view costs nothing to keep");
        // Data of its own (decrypted streams, strings) counts: past the budget the cache is
        // cleared after the next read, including a failed one.
        let owned = Object::Stream(crate::Stream::from_raw(Dict::new(), vec![b'y'; ObjectReader::STREAM_BYTES + 1]));
        reader.document.cache.lock().unwrap().insert(3, Arc::new(owned));
        assert_eq!(*reader.get(ObjRef::new(99, 0)), Object::Null);
        assert!(reader.document.cache.lock().unwrap().is_empty(), "owned data over budget is evicted");
    }

    #[test]
    fn temporary_reader_and_full_save_do_not_fill_snapshot_caches() {
        let mut source = Document::new_empty();
        let objects: Vec<_> = (0..260)
            .map(|i| {
                let mut value = vec![b'x'; 48 * 1024];
                value[..4].copy_from_slice(&(i as u32).to_le_bytes());
                Object::Ref(source.add(Object::String(crate::PdfString::literal(value))))
            })
            .collect();
        source.update_dict(source.root().unwrap(), |d| d.set(b"Extension".to_vec(), Object::Array(objects))).unwrap();
        let bytes = Arc::new(crate::write_full(&source, &crate::SaveOptions::default()).unwrap());
        let doc = Document::open(bytes).unwrap();
        let root = doc.get(doc.root().unwrap());
        let references = root.as_dict().unwrap().get(b"Extension").unwrap().as_array().unwrap();
        let snapshot = doc.clone();
        let cached = doc.cache.lock().unwrap().len();
        let streams = doc.objstms.lock().unwrap().len();
        let mut reader = doc.object_reader();
        let mut held = None;
        for (i, reference) in references.iter().enumerate() {
            let Object::Ref(reference) = reference else { panic!("indirect object") };
            let object = reader.get(*reference);
            assert_eq!(&object.as_string().unwrap().bytes[..4], &(i as u32).to_le_bytes());
            if i == 0 {
                held = Some(object);
            }
            assert!(reader.document.cache.lock().unwrap().len() < ObjectReader::OBJECTS);
            let decoded = reader.document.objstms.lock().unwrap();
            assert!(decoded.len() < ObjectReader::OBJECTS);
            let bytes: usize = decoded.values().map(|s| s.data.capacity() + s.index.capacity() * std::mem::size_of::<Option<(u32, usize)>>()).sum();
            assert!(bytes <= ObjectReader::STREAM_BYTES || decoded.len() == 1, "only one oversized stream may remain");
        }
        drop(reader);
        assert_eq!(&held.unwrap().as_string().unwrap().bytes[..4], &0u32.to_le_bytes(), "owned results survive evictions");
        for object_streams in [false, true] {
            let saved = crate::write_full(&doc, &crate::SaveOptions { object_streams, ..Default::default() }).unwrap();
            assert!(Document::open(Arc::new(saved)).unwrap().repair_log().is_empty());
            assert_eq!(doc.cache.lock().unwrap().len(), cached, "full saves must not fill the editor cache");
            assert_eq!(doc.objstms.lock().unwrap().len(), streams);
            assert!(Arc::ptr_eq(&doc.cache, &snapshot.cache), "do not detach or clear caller snapshots");
        }
    }

    #[test]
    fn streams_without_length_share_one_endstream_search() {
        let n = 2_000;
        let mut file = b"%PDF-1.7\n".to_vec();
        for k in 1..=n {
            file.extend_from_slice(format!("{k} 0 obj\n<< >>\nstream\nxx\n").as_bytes());
        }
        file.extend_from_slice(b"endstream\nendobj\n");
        file.extend_from_slice(
            format!(
                "{0} 0 obj\n<< /Type /Catalog /Pages {1} 0 R >>\nendobj\n{1} 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n",
                n + 1,
                n + 2
            )
            .as_bytes(),
        );
        file.extend_from_slice(format!("trailer\n<< /Root {} 0 R /Size {} >>\n%%EOF\n", n + 1, n + 3).as_bytes());
        let len = file.len();
        crate::parser::ENDSTREAM_SCAN.with(|c| c.set(0));
        let doc = Document::open(Arc::new(file)).unwrap();
        for k in [1, n / 2, n] {
            assert!(matches!(doc.try_get(k).as_deref(), Ok(Object::Stream(_))), "object {k}");
        }
        let scanned = crate::parser::ENDSTREAM_SCAN.with(|c| c.get());
        assert!(scanned <= 4 * len, "{scanned} bytes searched in a {len}-byte file");
    }

    #[test]
    fn editor_cache_counts_deeply_nested_strings() {
        let mut nested = Object::String(crate::PdfString::literal(vec![b'x'; 1 << 20]));
        for _ in 0..8 {
            nested = Object::Array(vec![nested]);
        }
        let mut source = Document::new_empty();
        let objects: Vec<_> = (0..48).map(|_| Object::Ref(source.add(nested.clone()))).collect();
        source.update_dict(source.root().unwrap(), |d| d.set(b"Extension".to_vec(), Object::Array(objects))).unwrap();
        let bytes = Arc::new(crate::write_full(&source, &crate::SaveOptions::default()).unwrap());
        let doc = Document::open(bytes).unwrap();
        assert_eq!(owned_bytes(&nested, &doc.data, 0), 1 << 20);
        let root = doc.get(doc.root().unwrap());
        for reference in root.as_dict().unwrap().get(b"Extension").unwrap().as_array().unwrap() {
            let Object::Ref(reference) = reference else { panic!("indirect object") };
            drop(doc.get(*reference));
            let cache = doc.cache.lock().unwrap();
            let owned = cache.values().fold(0usize, |n, o| n.saturating_add(owned_bytes(o, &doc.data, 0)));
            let strings = cache.values().filter(|o| o.as_array().is_some()).count();
            assert!(strings << 20 <= Document::EDITOR_CACHE_BYTES + (1 << 20), "{strings} MiB strings cached, {owned} bytes counted");
        }
    }

    #[test]
    fn editor_cache_stays_bounded() {
        let mut source = Document::new_empty();
        let objects: Vec<_> = (0..(Document::EDITOR_CACHE_OBJECTS + 64))
            .map(|i| {
                let mut value = vec![b'x'; 1024];
                value[..4].copy_from_slice(&(i as u32).to_le_bytes());
                Object::Ref(source.add(Object::String(crate::PdfString::literal(value))))
            })
            .collect();
        source.update_dict(source.root().unwrap(), |d| d.set(b"Extension".to_vec(), Object::Array(objects))).unwrap();
        let bytes = Arc::new(crate::write_full(&source, &crate::SaveOptions::default()).unwrap());
        let doc = Document::open(bytes).unwrap();
        let root = doc.get(doc.root().unwrap());
        let references = root.as_dict().unwrap().get(b"Extension").unwrap().as_array().unwrap().clone();
        let mut held = None;
        for (i, reference) in references.iter().enumerate() {
            let Object::Ref(reference) = reference else { panic!("indirect object") };
            let object = doc.get(*reference);
            assert_eq!(&object.as_string().unwrap().bytes[..4], &(i as u32).to_le_bytes());
            if i == 0 {
                held = Some(object);
            }
            assert!(doc.cache.lock().unwrap().len() <= Document::EDITOR_CACHE_OBJECTS, "editing must not pin the whole object graph");
            // Eviction keeps the newcomer.
            assert!(doc.cache.lock().unwrap().contains_key(&reference.num), "eviction must keep the newcomer");
        }
        // Returned objects survive eviction, through both handles of the shared cache.
        assert_eq!(&held.unwrap().as_string().unwrap().bytes[..4], &0u32.to_le_bytes());
        let snapshot = doc.clone();
        let first = references[0].as_ref().unwrap();
        assert_eq!(&snapshot.get(first).as_string().unwrap().bytes[..4], &0u32.to_le_bytes());
    }

    #[test]
    fn editor_cache_byte_bound_fires() {
        // 40 × 1 MB strings: far below the count cap, above the 32 MB byte budget. Each
        // newcomer exceeds EDITOR_LARGE_OBJECT, so every insert takes a full accounting pass.
        let mut source = Document::new_empty();
        let objects: Vec<_> = (0..40)
            .map(|i| {
                let mut value = vec![b'y'; 1024 * 1024];
                value[..4].copy_from_slice(&(i as u32).to_le_bytes());
                Object::Ref(source.add(Object::String(crate::PdfString::literal(value))))
            })
            .collect();
        source.update_dict(source.root().unwrap(), |d| d.set(b"Extension".to_vec(), Object::Array(objects))).unwrap();
        let bytes = Arc::new(crate::write_full(&source, &crate::SaveOptions::default()).unwrap());
        let doc = Document::open(bytes).unwrap();
        let root = doc.get(doc.root().unwrap());
        let references = root.as_dict().unwrap().get(b"Extension").unwrap().as_array().unwrap().clone();
        let mut held = None;
        for (i, reference) in references.iter().enumerate() {
            let Object::Ref(reference) = reference else { panic!("indirect object") };
            let object = doc.get(*reference);
            assert_eq!(&object.as_string().unwrap().bytes[..4], &(i as u32).to_le_bytes());
            if i == 0 {
                held = Some(object);
            }
            // Eviction keeps the newcomer.
            assert!(doc.cache.lock().unwrap().contains_key(&reference.num), "eviction must keep the newcomer");
        }
        assert!(doc.cache.lock().unwrap().len() < references.len(), "the byte budget must evict even below the count cap");
        let last = references[references.len() - 1].as_ref().unwrap();
        assert!(doc.cache.lock().unwrap().contains_key(&last.num), "eviction must keep the newcomer");
        assert_eq!(&held.unwrap().as_string().unwrap().bytes[..4], &0u32.to_le_bytes());
    }

    #[test]
    fn a_stream_limited_document_cannot_be_saved() {
        let bytes = Arc::new(crate::write_full(&Document::new_empty(), &crate::SaveOptions::default()).unwrap());
        let doc = Document::open_with_stream_limit(bytes.clone(), None, 1 << 20).unwrap();
        for saved in [crate::write_full(&doc, &crate::SaveOptions::default()), crate::write_incremental(&doc, &crate::SaveOptions::default())] {
            assert!(matches!(saved, Err(CosError::ReadOnlyLimit)), "{saved:?}");
        }
        let doc = Document::open(bytes).unwrap();
        assert!(crate::write_full(&doc, &crate::SaveOptions::default()).is_ok());
    }

    #[test]
    fn inspection_stream_limit_applies_before_loading_compressed_objects() {
        let mut doc = Document::new_empty();
        let large = doc.add(Object::String(crate::PdfString::literal(vec![b'x'; 4096])));
        doc.update_dict(doc.root().unwrap(), |catalog| catalog.set(b"Extension".to_vec(), Object::Ref(large))).unwrap();
        let bytes = Arc::new(crate::write_full(&doc, &crate::SaveOptions::default()).unwrap());
        assert!(Document::open(bytes.clone()).is_ok());
        assert!(Document::open_with_stream_limit(bytes, None, 512).is_err());
    }

    /// A /Prev chain that loops back on itself stops at the loop (and says so).
    #[test]
    fn looping_revision_chains_stop() {
        let mut bytes = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>"], "/Root 1 0 R");
        // An update whose /Prev points at itself.
        let at = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 1\n0000000000 65535 f \ntrailer << /Size 3 /Root 1 0 R /Prev {at} >>\nstartxref\n{at}\n%%EOF\n").as_bytes(),
        );
        let doc = Document::open(Arc::new(bytes)).unwrap();
        assert_eq!(doc.root(), Some(ObjRef::new(1, 0)));
        assert!(doc.repair_log().iter().any(|l| l.contains("loops")), "{:?}", doc.repair_log());
    }

    /// A hybrid-reference file: the classic table leaves object 3 out; the `/XRefStm` stream
    /// says it lives in object stream 4.
    #[test]
    fn hybrid_reference_files_read_the_xref_stream_too() {
        let doc = Document::open(Arc::new(object_stream_file("3 0 ", "<< /Hidden (yes) >>", 0))).unwrap();
        assert!(doc.repair_log().is_empty(), "{:?}", doc.repair_log());
        assert_compact_matches_materialized(&doc);
        assert_eq!(text_of(&doc, 3, b"Hidden").as_deref(), Some("yes"));
    }

    /// A hybrid-reference file whose classic table leaves object 3 out, and whose `/XRefStm`
    /// stream puts it at position `member` of object stream 4, which holds `header` (the number
    /// and offset pairs) and then `members`.
    fn object_stream_file(header: &str, members: &str, member: u8) -> Vec<u8> {
        let mut out = b"%PDF-1.5\n".to_vec();
        let mut offs = [0usize; 6];
        let mut obj = |out: &mut Vec<u8>, n: usize, body: &[u8]| {
            offs[n] = out.len();
            out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        };
        obj(&mut out, 1, b"<< /Type /Catalog /Pages 2 0 R >>");
        obj(&mut out, 2, b"<< /Type /Pages /Kids [] /Count 0 /Extra 3 0 R >>");
        let count = header.split_whitespace().count() / 2;
        let inner = format!("{header}{members}");
        let mut stm = format!("<< /Type /ObjStm /N {count} /First {} /Length {} >>\nstream\n", header.len(), inner.len()).into_bytes();
        stm.extend_from_slice(inner.as_bytes());
        stm.extend_from_slice(b"\nendstream");
        obj(&mut out, 4, &stm);
        let data = [2u8, 0, 4, member];
        let mut xs = format!("<< /Type /XRef /Size 6 /W [1 2 1] /Index [3 1] /Length {} >>\nstream\n", data.len()).into_bytes();
        xs.extend_from_slice(&data);
        xs.extend_from_slice(b"\nendstream");
        obj(&mut out, 5, &xs);
        let table = out.len();
        out.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
        for (n, off) in offs.iter().enumerate().skip(1) {
            let line = if n == 3 { "0000000000 65535 f \n".to_string() } else { format!("{off:010} 00000 n \n") };
            out.extend_from_slice(line.as_bytes());
        }
        out.extend_from_slice(format!("trailer << /Size 6 /Root 1 0 R /XRefStm {} >>\nstartxref\n{table}\n%%EOF\n", offs[5]).as_bytes());
        out
    }

    /// Many revisions naming one hybrid /XRefStm read it once.
    #[test]
    fn a_hybrid_stream_named_by_many_revisions_is_read_once() {
        let mut out = object_stream_file("3 0 ", "<< /Hidden (yes) >>", 0);
        let stream = find(&out, b"5 0 obj", 0, out.len()).unwrap();
        let mut prev = rfind(&out, b"\nxref\n", 0).unwrap() + 1;
        let revisions = 200;
        for _ in 0..revisions {
            let at = out.len();
            out.extend_from_slice(
                format!("xref\ntrailer << /Size 6 /Root 1 0 R /XRefStm {stream} /Prev {prev} >>\nstartxref\n{at}\n%%EOF\n").as_bytes(),
            );
            prev = at;
        }
        let doc = Document::open(Arc::new(out)).unwrap();
        SECTION_READS.with(|c| c.set(0));
        let (entries, ..) = doc.read_xref_chain_materialized(&mut Vec::new()).unwrap();
        let reads = SECTION_READS.with(|c| c.get());
        assert!(reads <= revisions + 2, "{reads} sections read for {} revisions", revisions + 1);
        assert!(matches!(entries.get(&3), Some(XrefEntry::InStream { stream: 4, .. })), "{:?}", entries.get(&3));
        assert_eq!(text_of(&doc, 3, b"Hidden").as_deref(), Some("yes"));
        assert_compact_matches_materialized(&doc);
    }

    /// An older section that names an /XRefStm a newer section already read still has its own
    /// table compared with that stream, so an entry it sets aside is recorded for it.
    #[test]
    fn a_repeated_hybrid_stream_records_each_sections_conflicts() {
        let mut out = object_stream_file("3 0 ", "<< /Hidden (yes) >>", 0);
        let stream = find(&out, b"5 0 obj", 0, out.len()).unwrap();
        let catalog = find(&out, b"1 0 obj", 0, out.len()).unwrap();
        let base = rfind(&out, b"\nxref\n", 0).unwrap() + 1;
        // This section's table places object 3 at the catalog; the stream puts it in object stream 4.
        let older = out.len();
        out.extend_from_slice(
            format!(
                "xref\n3 1\n{catalog:010} 00000 n \ntrailer << /Size 6 /Root 1 0 R /XRefStm {stream} /Prev {base} >>\nstartxref\n{older}\n%%EOF\n"
            )
            .as_bytes(),
        );
        let newer = out.len();
        out.extend_from_slice(
            format!("xref\ntrailer << /Size 6 /Root 1 0 R /XRefStm {stream} /Prev {older} >>\nstartxref\n{newer}\n%%EOF\n").as_bytes(),
        );
        let doc = Document::open(Arc::new(out)).unwrap();
        assert_eq!(doc.xref_oddities(), [(older as u64, 3)]);
        assert_eq!(text_of(&doc, 3, b"Hidden").as_deref(), Some("yes"));
    }

    fn text_of(doc: &Document, num: u32, key: &[u8]) -> Option<String> {
        doc.get(ObjRef::new(num, 0)).as_dict().and_then(|d| d.get(key).and_then(|v| v.as_string().map(|s| s.to_text())))
    }

    /// An object stream member numbered 2^32 + 3 is not object 3 (#1815): the number used to be
    /// cast to `u32`, which dropped the high bits.
    #[test]
    fn object_stream_numbers_beyond_u32_are_not_truncated() {
        let control = Document::open(Arc::new(object_stream_file("3 0 ", "<< /Member (yes) >>", 0))).unwrap();
        assert_eq!(text_of(&control, 3, b"Member").as_deref(), Some("yes"));
        let doc = Document::open(Arc::new(object_stream_file("4294967299 0 ", "<< /Member (yes) >>", 0))).unwrap();
        assert!(matches!(*doc.get(ObjRef::new(3, 0)), Object::Null), "{:?}", doc.get(ObjRef::new(3, 0)));
        assert!(!doc.xref_entry_is_exact(3));
    }

    /// Member starts are checked rather than cast, so they cannot wrap on any target (#1815).
    #[test]
    fn object_stream_member_starts_are_checked() {
        assert_eq!(objstm_member(3, 4, Some(10), 20), Some((3, 14)));
        assert_eq!(objstm_member(3, 9, Some(10), 20), Some((3, 19)), "the last byte");
        assert_eq!(objstm_member(3, 10, Some(10), 20), None, "at the end of the data");
        assert_eq!(objstm_member(3, 1, Some(usize::MAX), 20), None, "/First plus the offset overflows");
        assert_eq!(objstm_member(3, i64::MAX, Some(10), 20), None);
        assert_eq!(objstm_member(3, -1, Some(10), 20), None);
        assert_eq!(objstm_member(3, 0, None, 20), None, "/First does not fit in usize");
        assert_eq!(objstm_member(i64::from(u32::MAX), 0, Some(0), 1), Some((u32::MAX, 0)));
        assert_eq!(objstm_member(i64::from(u32::MAX) + 1, 0, Some(0), 1), None);
        assert_eq!(objstm_member(-1, 0, Some(0), 1), None);
    }

    /// A member at a negative offset is unusable, not the stream's first member (#1815): the
    /// offset used to be clamped to zero, which read the object at `/First`.
    #[test]
    fn object_stream_negative_offsets_are_refused() {
        let doc = Document::open(Arc::new(object_stream_file("7 0 3 -1 ", "<< /First (yes) >>", 1))).unwrap();
        assert!(matches!(*doc.get(ObjRef::new(3, 0)), Object::Null), "{:?}", doc.get(ObjRef::new(3, 0)));
        let control = Document::open(Arc::new(object_stream_file("7 0 3 19 ", "<< /First (yes) >> << /Second (yes) >>", 1))).unwrap();
        assert_eq!(text_of(&control, 3, b"Second").as_deref(), Some("yes"));
    }

    #[test]
    fn revision_ends_split_incremental_updates() {
        let mut bytes = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>"], "/Root 1 0 R");
        let first = bytes.len();
        let doc = Document::open(Arc::new(bytes.clone())).unwrap();
        assert_eq!(doc.revision_ends(), vec![first]);
        let mut edited = doc.clone();
        edited.update_dict(ObjRef::new(1, 0), |d| d.set(b"Lang".to_vec(), Object::String(crate::PdfString::literal("en")))).unwrap();
        bytes = crate::write_incremental(&edited, &crate::SaveOptions::default()).unwrap();
        let doc = Document::open(Arc::new(bytes.clone())).unwrap();
        let ends = doc.revision_ends();
        assert_eq!(ends.len(), 2, "{ends:?}");
        assert!(ends[0] >= first && ends[0] < bytes.len() && ends[1] == bytes.len(), "{ends:?} of {}", bytes.len());
        // The first revision opens on its own, without the update.
        let old = Document::open(Arc::new(bytes[..ends[0]].to_vec())).unwrap();
        assert!(old.get(ObjRef::new(1, 0)).as_dict().unwrap().get(b"Lang").is_none());
    }

    /// qpdf's `xref-compressed-in-compressed.pdf` in miniature: object 3 is an object stream
    /// holding object 4, and the cross-reference stream says object 3 itself lives in stream 99.
    fn compressed_in_compressed_fixture() -> Document {
        let mut out = b"%PDF-1.5\n".to_vec();
        let at = |out: &mut Vec<u8>, num: u32, body: &str| {
            let off = out.len();
            out.extend_from_slice(format!("{num} 0 obj\n{body}\nendobj\n").as_bytes());
            off as u16
        };
        let o1 = at(&mut out, 1, "<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R >>");
        let o2 = at(&mut out, 2, "<< /Type /Pages /Kids [] /Count 0 >>");
        let inner = "<< /Type /Outlines /Count 0 >>";
        at(&mut out, 3, &format!("<< /Type /ObjStm /N 1 /First 4 /Length {} >>\nstream\n4 0 {inner}\nendstream", 4 + inner.len()));
        // /W [1 2 1]: object 3 is said to live in stream 99 (which does not exist), 4 in stream 3.
        let rows: Vec<[u8; 4]> = vec![
            [0, 0, 0, 255],
            [1, 0, o1 as u8, 0],
            [1, 0, o2 as u8, 0],
            [2, 0, 99, 0],
            [2, 0, 3, 0],
            [1, (out.len() >> 8) as u8, out.len() as u8, 0],
        ];
        let rows: Vec<u8> = rows.concat();
        let xref = out.len();
        out.extend_from_slice(
            format!("5 0 obj\n<< /Type /XRef /W [1 2 1] /Index [0 6] /Size 6 /Root 1 0 R /Length {} >>\nstream\n", rows.len()).as_bytes(),
        );
        out.extend_from_slice(&rows);
        out.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{xref}\n%%EOF\n").as_bytes());
        Document::open(Arc::new(out)).unwrap()
    }

    /// An object stream whose own cross-reference entry says it is compressed inside another
    /// stream (which the format forbids, and which does not exist here: qpdf's
    /// `xref-compressed-in-compressed.pdf`, #2367) could not be loaded, so every object in it
    /// read as null and a full save wrote `/Outlines null`. The stream is now found by its own
    /// header, as a plain object with a wrong offset is.
    #[test]
    fn an_object_stream_marked_as_compressed_is_found_by_its_header() {
        let doc = compressed_in_compressed_fixture();
        assert_eq!(doc.revisions().len(), 1, "the chain itself reads: {:?}", doc.repair_log());
        let outlines = doc.get(ObjRef::new(4, 0));
        assert_eq!(outlines.as_dict().and_then(|d| d.name(b"Type")), Some(&b"Outlines"[..]), "{outlines:?}");
        let saved = crate::write_full(&doc, &crate::SaveOptions::default()).unwrap();
        let back = Document::open(Arc::new(saved)).unwrap();
        let catalog = back.get(back.root().unwrap());
        let outlines = back.resolve(catalog.as_dict().unwrap().get(b"Outlines").unwrap());
        assert_eq!(outlines.as_dict().and_then(|d| d.name(b"Type")), Some(&b"Outlines"[..]), "the full save keeps it: {outlines:?}");
    }

    /// The header fallback is for file entries only: once the stream is freed (or replaced) in
    /// the overlay, its objects must not come back from the file's copy of it.
    #[test]
    fn a_freed_object_stream_is_not_found_by_its_header() {
        let doc = compressed_in_compressed_fixture();
        assert!(doc.get(ObjRef::new(4, 0)).as_dict().is_some());
        let mut edited = doc.clone();
        edited.free(ObjRef::new(3, 0));
        let fresh = edited.object_reader().document;
        assert!(matches!(*fresh.get(ObjRef::new(4, 0)), Object::Null), "{:?}", fresh.get(ObjRef::new(4, 0)));
        let mut replaced = doc.clone();
        replaced.set(ObjRef::new(3, 0), Object::Int(0));
        let fresh = replaced.object_reader().document;
        assert!(matches!(*fresh.get(ObjRef::new(4, 0)), Object::Null), "{:?}", fresh.get(ObjRef::new(4, 0)));
    }

    /// The same once the free is saved: the revision's table says the stream is free, and
    /// neither reading the file back nor rewriting it in full brings its objects back.
    #[test]
    fn a_container_freed_in_a_saved_revision_is_not_found_by_its_header() {
        let mut edited = compressed_in_compressed_fixture();
        edited.free(ObjRef::new(3, 0));
        let incremental = crate::write_incremental(&edited, &crate::SaveOptions::default()).unwrap();
        let reopened = Document::open(Arc::new(incremental)).unwrap();
        assert_eq!(reopened.revisions().len(), 2);
        assert!(matches!(reopened.xref_entry(3), Some(XrefEntry::Free { .. })));
        assert!(matches!(*reopened.get(ObjRef::new(4, 0)), Object::Null), "{:?}", reopened.get(ObjRef::new(4, 0)));
        for object_streams in [true, false] {
            let opts = crate::SaveOptions { object_streams, ..Default::default() };
            let full = crate::write_full(&reopened, &opts).unwrap();
            let back = Document::open(Arc::new(full)).unwrap();
            let catalog = back.get(back.root().unwrap());
            let outlines = catalog.as_dict().unwrap().get(b"Outlines").map(|o| back.resolve(o));
            assert!(outlines.as_ref().is_none_or(|o| matches!(**o, Object::Null)), "the full save keeps it out: {outlines:?}");
        }
    }

    /// A trailer whose /Prev is written as the reference `N 0 R` (tiff2pdf did this; the files
    /// are JHOVE's PDF-HUL-157 cases, #2366) used to end the cross-reference chain silently, so
    /// every object of the earlier sections read as null: a four-page file showed one page, and
    /// a full save wrote the three missing kids as null. The offset is read from the reference.
    #[test]
    fn a_prev_written_as_a_reference_still_reaches_the_earlier_sections() {
        let base = build(
            &["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 >>", "<< /Type /Page /Parent 2 0 R >>"],
            "/Root 1 0 R",
        );
        let prev = String::from_utf8_lossy(&base).rfind("xref\n0 4").unwrap();
        let update = |prev_entry: &str| {
            let mut bytes = base.clone();
            let at = bytes.len();
            bytes.extend_from_slice(b"4 0 obj\n<< /Title (Update) >>\nendobj\n");
            let xref = bytes.len();
            bytes.extend_from_slice(
                format!(
                    "xref\n4 1\n{at:010} 00000 n \ntrailer\n<< /Size 5 /Root 1 0 R /Info 4 0 R /Prev {prev_entry} >>\nstartxref\n{xref}\n%%EOF\n"
                )
                .as_bytes(),
            );
            Document::open(Arc::new(bytes)).unwrap()
        };
        let good = update(&prev.to_string());
        let bad = update(&format!("{prev} 0 R"));
        assert!(good.repair_log().is_empty(), "{:?}", good.repair_log());
        assert_eq!(bad.revisions().len(), 2, "{:?}", bad.repair_log());
        assert_eq!(bad.repair_log().len(), 1, "a note about the /Prev: {:?}", bad.repair_log());
        assert_eq!(bad.xref_entry(3), good.xref_entry(3), "the page of the first section is reached");
        assert_eq!(bad.get(ObjRef::new(3, 0)).as_dict().and_then(|d| d.name(b"Type")), Some(&b"Page"[..]));
        assert_eq!(
            bad.get(ObjRef::new(4, 0)).as_dict().and_then(|d| d.get(b"Title").and_then(|t| t.as_string().map(|s| s.to_text()))),
            Some("Update".into())
        );
    }

    /// iText's `indirectPrev.pdf` in miniature: the update's trailer gives `/Prev 4 0 R`, and
    /// object 4, listed in that update, holds the offset of the first section. Read as offset 4
    /// (inside the header), the chain broke and the file was rebuilt without its revisions.
    #[test]
    fn a_prev_reference_to_an_object_holding_the_offset_reads_that_offset() {
        let mut bytes = build(
            &["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 >>", "<< /Type /Page /Parent 2 0 R >>"],
            "/Root 1 0 R",
        );
        let prev = String::from_utf8_lossy(&bytes).rfind("xref\n0 4").unwrap();
        let mut at = Vec::new();
        for (num, body) in
            [(2, "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_string()), (4, prev.to_string()), (5, "<< /Type /Page /Parent 2 0 R >>".into())]
        {
            at.push(bytes.len());
            bytes.extend_from_slice(format!("{num} 0 obj\n{body}\nendobj\n").as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!(
                "xref\n2 1\n{:010} 00000 n \n4 2\n{:010} 00000 n \n{:010} 00000 n \ntrailer\n<< /Size 6 /Root 1 0 R /Prev 4 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                at[0], at[1], at[2]
            )
            .as_bytes(),
        );
        let doc = Document::open(Arc::new(bytes)).unwrap();
        assert_eq!(doc.revisions().len(), 2, "{:?}", doc.repair_log());
        assert_eq!(doc.repair_log().len(), 1, "a note about the /Prev, and no rebuild: {:?}", doc.repair_log());
        assert!(doc.repair_log()[0].contains(&format!("the offset {prev}")), "{:?}", doc.repair_log());
        assert_eq!(doc.get(ObjRef::new(3, 0)).as_dict().and_then(|d| d.name(b"Type")), Some(&b"Page"[..]));
        assert_compact_matches_materialized(&doc);
    }

    /// A file whose only trailers are cross-reference stream dictionaries, with its `startxref`
    /// out of reach (here a second file appended after it, cut off before its own
    /// cross-reference data, as in #2365), is rebuilt from its object headers. The rebuild used
    /// to look only for the `trailer` keyword, so /Info and /ID were lost, and a full save after
    /// editing one Info field dropped every other.
    #[test]
    fn reconstruction_takes_the_trailer_from_a_cross_reference_stream() {
        let table = build(
            &["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", "<< /Title (Kept) /Creator (Me) >>"],
            "/Root 1 0 R /Info 3 0 R",
        );
        let packed = crate::write_full(&Document::open(Arc::new(table)).unwrap(), &crate::SaveOptions::default()).unwrap();
        let text = String::from_utf8_lossy(&packed);
        assert!(text.contains("/XRef") && !text.contains("trailer"), "a cross-reference stream is the only trailer");
        let mut bytes = packed.clone();
        let appended = format!("%PDF-1.3\n90 0 obj\n<< /Length 5000 >>\nstream\n{}\nendstream\nendobj\n", "x".repeat(5000));
        bytes.extend_from_slice(appended.as_bytes());
        let doc = Document::open(Arc::new(bytes)).unwrap();
        assert!(doc.revisions().is_empty(), "rebuilt: {:?}", doc.repair_log());
        let info = doc.trailer().reference(b"Info").expect("the cross-reference stream's /Info");
        let field = |d: &Document, key: &[u8]| d.get(info).as_dict().unwrap().get(key).unwrap().as_string().unwrap().bytes.clone();
        assert_eq!(field(&doc, b"Title"), b"Kept");
        assert!(doc.trailer().get(b"ID").is_some(), "and its /ID");
        let mut edited = doc.clone();
        edited.update_dict(info, |d| d.set(b"Subject".to_vec(), Object::String(crate::PdfString::literal("new")))).unwrap();
        let saved = crate::write_full(&edited, &crate::SaveOptions::default()).unwrap();
        let back = Document::open(Arc::new(saved)).unwrap();
        assert!(back.repair_log().is_empty(), "{:?}", back.repair_log());
        let info = back.trailer().reference(b"Info").expect("the full save keeps /Info");
        let field = |key: &[u8]| back.get(info).as_dict().unwrap().get(key).unwrap().as_string().unwrap().bytes.clone();
        assert_eq!((field(b"Title"), field(b"Creator"), field(b"Subject")), (b"Kept".to_vec(), b"Me".to_vec(), b"new".to_vec()));
    }

    /// A rebuild prefers a trailer whose /Root is the catalog (as `root_is_catalog` judges it): a
    /// later cross-reference stream whose /Root points at the Info dictionary does not replace an
    /// earlier good trailer, or a full save would write the Info dictionary as /Root.
    #[test]
    fn reconstruction_skips_a_trailer_whose_root_is_not_the_catalog() {
        let mut bytes =
            build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", "<< /Title (Kept) >>"], "/Root 1 0 R /Info 3 0 R");
        bytes.extend_from_slice(b"4 0 obj\n<< /Type /XRef /Size 5 /W [1 1 1] /Root 3 0 R /Length 3 >>\nstream\nabc\nendstream\nendobj\n");
        let appended = format!("%PDF-1.3\n90 0 obj\n<< /Length 5000 >>\nstream\n{}\nendstream\nendobj\n", "x".repeat(5000));
        bytes.extend_from_slice(appended.as_bytes());
        let doc = Document::open(Arc::new(bytes)).unwrap();
        assert!(doc.repair_log().iter().any(|l| l.contains("rebuilt")), "{:?}", doc.repair_log());
        assert_eq!(doc.trailer().reference(b"Root").map(|r| r.num), Some(1), "the catalog, not the Info dictionary");
        assert_eq!(doc.trailer().reference(b"Info").map(|r| r.num), Some(3));
    }

    #[test]
    fn damaged_xref_is_reconstructed() {
        let mut bytes = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>"], "/Root 1 0 R");
        let n = bytes.len();
        bytes[n - 12..n - 6].copy_from_slice(b"999999"); // point startxref into nowhere
        let doc = Document::open(Arc::new(bytes)).unwrap();
        assert!(doc.revisions().is_empty());
        assert!(!doc.repair_log().is_empty());
        assert_eq!(doc.root(), Some(ObjRef::new(1, 0)));
    }

    #[test]
    fn a_decompression_bomb_in_an_xref_stream_is_refused_and_the_file_reconstructed() {
        // 32 MiB of rows behind two FlateDecode filters (a few hundred bytes in the file) for
        // /W [1 1 1]: at 3 bytes a row that is more than 8,388,607 objects' worth, so it is not
        // decoded (in full: the limit applies while inflating) and the objects are found by
        // reconstruction instead.
        let bomb = pdfcraft_filters::encode_flate(&pdfcraft_filters::encode_flate(&vec![0u8; 32 << 20]));
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let o1 = bytes.len();
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        let o2 = bytes.len();
        bytes.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
        let x = bytes.len();
        bytes.extend_from_slice(
            format!("3 0 obj\n<< /Type /XRef /Size 4 /W [1 1 1] /Root 1 0 R /Filter [/FlateDecode /FlateDecode] /Length {} >>\nstream\n", bomb.len())
                .as_bytes(),
        );
        bytes.extend_from_slice(&bomb);
        bytes.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{x}\n%%EOF\n").as_bytes());
        assert!(o1 < o2 && o2 < x);
        let doc = Document::open(Arc::new(bytes)).unwrap();
        assert_eq!(doc.root(), Some(ObjRef::new(1, 0)));
        assert!(doc.repair_log().iter().any(|l| l.contains("limit")), "{:?}", doc.repair_log());
    }

    #[test]
    fn a_stream_whose_length_is_itself_and_never_ends_loads_quickly() {
        // From the nightly fuzz job: `6 0 obj << /Length 6 0 R >> stream` with no `endstream`
        // after it. Every parse attempt failed, and each attempt resolved the length by loading
        // object 6 again, retrying the same ways down to the nesting limit: about 2^32 parses.
        // Objects being loaded are now a seen-set, lengths are memoised, and the second parse
        // only runs when the header is shifted.
        let mut bytes = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n".to_vec();
        bytes.extend_from_slice(b"6 0 obj\n<< /Length 6 0 R >>\nstream\nno end marker follows\n");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Ok(doc) = Document::open(Arc::new(bytes)) {
                let _ = doc.try_get(6);
            }
            let _ = tx.send(());
        });
        rx.recv_timeout(std::time::Duration::from_secs(20)).expect("loading must not retry exponentially");
    }
}
