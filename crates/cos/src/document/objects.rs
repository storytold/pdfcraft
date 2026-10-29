use super::reader::find;
use super::state::{LENGTHS, LoadGuard, ObjStm, ObjectReader, ReadState, Slot, SourceIdentity, objstm_member, owned_bytes};
use super::xref::{Revision, XrefEntry};
use super::{Arc, CosError, Dict, Document, Lexer, ObjRef, Object, is_whitespace, parse_indirect_shared};

impl Document {
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

    /// Set the PDF header version (`"1.4"`, `"1.7"`, `"2.0"`, …) written by a full save.
    pub fn set_version(&mut self, version: impl Into<String>) {
        self.version = version.into();
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
    pub(super) const EDITOR_CACHE_OBJECTS: usize = 1024;

    pub(super) const EDITOR_CACHE_BYTES: usize = 32 * 1024 * 1024;

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

    pub(super) fn objstm(&self, num: u32) -> Result<Arc<ObjStm>, CosError> {
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

    /// Free what was added since the file was read and nothing refers to any more: the objects
    /// [`SaveOptions::drop_unlinked`](crate::SaveOptions::drop_unlinked) leaves out of an
    /// incremental save, freed as it writes them (the generation stays). For a writer about to
    /// add an object that nothing refers to either, which that option would leave out too.
    pub fn free_unlinked_additions(&mut self) {
        for num in crate::writer::unlinked_additions(self) {
            let generation = self.generation(num);
            self.overlay.insert(num, Slot::Freed(generation));
        }
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

    pub(super) fn root_is_catalog(&self) -> bool {
        let Some(r) = self.root() else { return false };
        matches!(self.try_get(r.num).as_deref(), Ok(Object::Dict(d)) if d.name(b"Type") == Some(b"Catalog") || d.contains(b"Pages"))
    }
}
