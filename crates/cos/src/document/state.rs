use super::xref::XrefEntry;
use super::*;

thread_local! {
    /// The objects being loaded on this thread, innermost last (see `Document::try_get`).
    static LOADING: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) };
    /// Indirect `/Length` values resolved during the current outermost load: a damaged file
    /// retries a parse several ways, and each retry would resolve the same lengths again
    /// (exponential in the chain of streams whose lengths point at further streams).
    pub(super) static LENGTHS: std::cell::RefCell<HashMap<u32, Option<i64>>> = std::cell::RefCell::new(HashMap::new());
}

/// Marks an object as being loaded on this thread. Loading can re-enter (indirect stream
/// `/Length`, object streams): an object already being loaded is a cycle, and the nesting is
/// bounded as well.
pub(super) struct LoadGuard;

impl LoadGuard {
    const MAX: usize = 32;

    pub(super) fn enter(num: u32) -> Option<Self> {
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

#[derive(Clone, Debug)]
pub(super) struct ObjStm {
    /// One slot per member in stream order, since cross-reference entries name a member by its
    /// position: (object number, byte offset within `data`), or `None` for a member whose number
    /// or offset is unusable ([`objstm_member`]).
    pub(super) index: Vec<Option<(u32, usize)>>,
    pub(super) data: Arc<Vec<u8>>,
}

/// Where member `num` of an object stream starts: its `offset` past `/First` (`first`), when the
/// number is an object number and the start lies inside the decoded `len` bytes. Checked on every
/// target, so a 32-bit build neither truncates nor wraps a large `/First` or offset (#1815).
pub(super) fn objstm_member(num: i64, offset: i64, first: Option<usize>, len: usize) -> Option<(u32, usize)> {
    let num = u32::try_from(num).ok()?;
    let start = first?.checked_add(usize::try_from(offset).ok()?)?;
    (start < len).then_some((num, start))
}

/// Where an overlay slot came from.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Slot {
    Set(u16, Arc<Object>),
    Freed(u16),
}

/// An immutable input/decode context shared by document clones. Holding this identity
/// retains neither the input bytes nor the parsed document. Weak ownership prevents an
/// expired identity from matching a later allocation at the same address.
#[derive(Clone, Debug)]
pub struct SourceIdentity(pub(super) Weak<()>);

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
    pub(super) source: SourceIdentity,
    pub(super) trailer: Dict,
    pub(super) overlay: BTreeMap<u32, Slot>,
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

/// A temporary reader for whole-document work. Its caches never populate the editor or
/// undo snapshots. Returned objects survive cache eviction; a stream read from the file is a
/// view into the input bytes (it keeps the whole input alive while held, not a copy).
pub(crate) struct ObjectReader {
    pub(crate) document: Document,
    pub(super) decoded_count: usize,
}

impl ObjectReader {
    pub(super) const OBJECTS: usize = 128;
    pub(super) const STREAM_BYTES: usize = 8 * 1024 * 1024;

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
    pub(super) static SECTION_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Nesting [`owned_bytes`] walks: past the parser's own nesting limit, so every parsed object
/// is measured in full.
const OWNED_BYTES_DEPTH: usize = 256;

/// Bytes `object` holds of its own: string and stream data that is not a view into `data`
/// (the input file). An object nested deeper than [`OWNED_BYTES_DEPTH`] cannot be measured
/// and counts as too large to keep.
pub(super) fn owned_bytes(object: &Object, data: &Arc<Vec<u8>>, depth: usize) -> usize {
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

/// How an encrypted document is opened: with a password, or with the file key of a document
/// that has the same encryption (an earlier revision of the same file).
pub(super) enum Credentials<'a> {
    Password(Option<&'a str>),
    Handler { handler: &'a Arc<pdfcraft_crypt::SecurityHandler>, id0: Vec<u8> },
}
