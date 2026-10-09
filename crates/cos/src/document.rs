//! Document: cross-reference data, lazy object loading, and a copy-on-write overlay of edits.
//!
//! `Document` is cheap to clone (the original bytes, xref index and parse caches are shared), so
//! a clone is a snapshot: undo/redo and background jobs hold clones while the UI edits another.
//! Edits never touch the original bytes; the writer appends them (incremental save) or rewrites
//! the file (full save).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::CosError;
use crate::object::{Dict, ObjRef, Object};
use crate::parser::{Lexer, is_whitespace, parse_indirect, parse_indirect_shared};

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

/// One cross-reference section of the file (one per save; the first is the original).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revision {
    /// Offset of the xref section (`startxref` value).
    pub xref_offset: u64,
    /// `true` if the section is a cross-reference stream.
    pub is_stream: bool,
}

#[derive(Clone, Debug)]
struct ObjStm {
    /// (object number, byte offset within `data`).
    index: Vec<(u32, usize)>,
    data: Arc<Vec<u8>>,
}

/// Where an overlay slot came from.
#[derive(Clone, Debug, PartialEq)]
enum Slot {
    Set(u16, Arc<Object>),
    Freed(u16),
}

#[derive(Clone)]
pub struct Document {
    data: Arc<Vec<u8>>,
    entries: Arc<BTreeMap<u32, XrefEntry>>,
    trailer: Dict,
    revisions: Arc<Vec<Revision>>,
    repair_log: Arc<Vec<String>>,
    cache: Arc<Mutex<HashMap<u32, Arc<Object>>>>,
    objstms: Arc<Mutex<HashMap<u32, Arc<ObjStm>>>>,
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

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document").field("bytes", &self.data.len()).field("objects", &self.entries.len()).field("edits", &self.overlay.len()).finish()
    }
}

impl Document {
    /// A new document with an empty page tree (for split / extract / combine). It has no
    /// original bytes, so saving always writes a full file.
    pub fn new_empty() -> Self {
        let mut doc = Document {
            data: Arc::new(Vec::new()),
            entries: Arc::new(BTreeMap::new()),
            trailer: Dict::new(),
            revisions: Arc::new(Vec::new()),
            repair_log: Arc::new(Vec::new()),
            cache: Arc::default(),
            objstms: Arc::default(),
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
            entries: Arc::new(BTreeMap::new()),
            trailer: Dict::new(),
            revisions: Arc::new(Vec::new()),
            repair_log: Arc::new(Vec::new()),
            cache: Arc::default(),
            objstms: Arc::default(),
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
            Ok((entries, trailer, revisions)) => {
                doc.entries = Arc::new(entries);
                doc.trailer = trailer;
                doc.revisions = Arc::new(revisions);
                // Authenticate before checking the catalog: it may sit in an encrypted object
                // stream, which reads as garbage until the key is known.
                if let Some(enc) = doc.trailer.get(b"Encrypt").cloned() {
                    doc.authenticate(&enc, password)?;
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
            doc.authenticate(&enc, password)?;
        }
        let max = doc.entries.keys().next_back().copied().unwrap_or(0);
        let size = doc.trailer.int(b"Size").unwrap_or(0).max(0) as u32;
        doc.next_num = max.max(size.saturating_sub(1)) + 1;
        Ok(doc)
    }

    fn authenticate(&mut self, enc: &Object, password: Option<&str>) -> Result<(), CosError> {
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
        let id0 = match self.trailer.get(b"ID") {
            Some(Object::Array(a)) => a.first().and_then(|s| s.as_string()).map(|s| s.bytes.clone()).unwrap_or_default(),
            _ => Vec::new(),
        };
        let handler = pdfcraft_crypt::SecurityHandler::open(params, &id0, password).map_err(|e| match e {
            pdfcraft_crypt::CryptError::WrongPassword if password.is_none() => CosError::NeedsPassword,
            pdfcraft_crypt::CryptError::WrongPassword => CosError::WrongPassword,
            other => CosError::Security(other.to_string()),
        })?;
        self.security = Some(Arc::new(handler));
        self.encrypt_num = num;
        // Objects read while locating the catalog were read before decryption was possible.
        self.cache = Arc::default();
        self.objstms = Arc::default();
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
            Some(h) if Some(id.num) != self.encrypt_num => crate::security::transform(h, &o, id.num, id.generation, true),
            _ => o,
        }
    }

    pub fn bytes(&self) -> &Arc<Vec<u8>> {
        &self.data
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

    /// `true` when there are unsaved edits.
    pub fn is_modified(&self) -> bool {
        !self.overlay.is_empty()
    }

    /// Object numbers changed since the document was opened (or last saved).
    pub fn modified_objects(&self) -> Vec<u32> {
        self.overlay.keys().copied().collect()
    }

    /// Where object `num` is stored in the file (ignoring unsaved edits).
    pub fn xref_entry(&self, num: u32) -> Option<XrefEntry> {
        self.entries.get(&num).cloned()
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
        self.cache.lock().map_err(|_| CosError::Poisoned)?.insert(num, obj.clone());
        Ok(obj)
    }

    fn load(&self, num: u32, depth: u32) -> Result<Object, CosError> {
        if depth > 16 {
            return Err(CosError::Syntax { offset: 0, detail: "reference cycle while loading".into() });
        }
        match self.entries.get(&num) {
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
                let off = *offset as usize;
                let at = |off: usize| match parse_indirect_shared(&self.data, off, &resolve) {
                    Ok((id, o)) if id.num == num => Some(self.decrypted(id, o)),
                    _ => None,
                };
                // Offsets relative to a shifted header, or simply wrong: try both fixes.
                at(off)
                    .or_else(|| if self.header_offset == 0 { None } else { at(off.saturating_add(self.header_offset)) })
                    .or_else(|| self.scan_for(num).map(|(id, o)| self.decrypted(id, o)))
                    .ok_or(CosError::MissingObject(num))
            }
            Some(XrefEntry::InStream { stream, index }) => {
                let stm = self.objstm(*stream)?;
                let (n, off) = stm
                    .index
                    .get(*index as usize)
                    .copied()
                    .or_else(|| stm.index.iter().find(|(n, _)| *n == num).copied())
                    .ok_or(CosError::MissingObject(num))?;
                let off =
                    if n == num { off } else { stm.index.iter().find(|(n, _)| *n == num).map(|(_, o)| *o).ok_or(CosError::MissingObject(num))? };
                Lexer::new(&stm.data, off).object()
            }
        }
    }

    fn objstm(&self, num: u32) -> Result<Arc<ObjStm>, CosError> {
        if let Some(s) = self.objstms.lock().map_err(|_| CosError::Poisoned)?.get(&num) {
            return Ok(s.clone());
        }
        let Object::Stream(s) = &*self.try_get(num)? else { return Err(CosError::MissingObject(num)) };
        let n = s.dict.int(b"N").unwrap_or(0).clamp(0, 1_000_000) as usize;
        let first = s.dict.int(b"First").unwrap_or(0).max(0) as usize;
        let data = s.decoded()?;
        let mut lx = Lexer::new(&data, 0);
        let mut index = Vec::with_capacity(n);
        for _ in 0..n {
            let (Some(Object::Int(onum)), Some(Object::Int(off))) = (lx.object().ok(), lx.object().ok()) else { break };
            index.push((onum.max(0) as u32, first + off.max(0) as usize));
        }
        let stm = Arc::new(ObjStm { index, data: Arc::new(data) });
        self.objstms.lock().map_err(|_| CosError::Poisoned)?.insert(num, stm.clone());
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
            if let Ok((id, o)) = parse_indirect_shared(data, p, &|_| None)
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
            self.next_num = r.num + 1;
        }
    }

    /// Add a new indirect object and return its reference.
    pub fn add(&mut self, obj: impl Into<Object>) -> ObjRef {
        let r = ObjRef::new(self.next_num, 0);
        self.next_num += 1;
        let obj = self.adopt(obj.into());
        self.overlay.insert(r.num, Slot::Set(0, Arc::new(obj)));
        r
    }

    /// Delete an object (its number becomes free with the next generation).
    pub fn free(&mut self, r: ObjRef) {
        self.overlay.insert(r.num, Slot::Freed(r.generation.saturating_add(1)));
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
            None => match self.entries.get(&num) {
                Some(XrefEntry::InFile { generation, .. }) => *generation,
                Some(XrefEntry::Free { next_generation }) => *next_generation,
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
        let mut set: HashSet<u32> = self.entries.iter().filter(|(_, e)| !matches!(e, XrefEntry::Free { .. })).map(|(n, _)| *n).collect();
        for (n, s) in &self.overlay {
            match s {
                Slot::Set(..) => set.insert(*n),
                Slot::Freed(_) => set.remove(n),
            };
        }
        let mut v: Vec<u32> = set.into_iter().collect();
        v.sort_unstable();
        v
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
    fn read_xref_chain(&self, log: &mut Vec<String>) -> Result<(BTreeMap<u32, XrefEntry>, Dict, Vec<Revision>), CosError> {
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
        let mut visited = HashSet::new();
        let mut next = Some(first);
        while let Some(off) = next.take() {
            if !visited.insert(off) || visited.len() > 10_000 {
                log.push(format!("cross-reference chain loops at offset {off}; stopped"));
                break;
            }
            // This section's own entries; older sections only fill what newer ones left out.
            let mut sect = BTreeMap::new();
            let (sect_trailer, is_stream) = match self.read_section(off as usize, &mut sect) {
                Ok(v) => v,
                Err(e) if self.header_offset > 0 => {
                    log.push(format!("xref at {off} unreadable ({e}); retrying relative to the header"));
                    sect.clear();
                    self.read_section(off as usize + self.header_offset, &mut sect)?
                }
                Err(e) => {
                    if revisions.is_empty() {
                        return Err(e);
                    }
                    log.push(format!("older cross-reference section at {off} is unreadable: {e}"));
                    break;
                }
            };
            revisions.push(Revision { xref_offset: off, is_stream });
            // Hybrid files (§7.5.8.4): the /XRefStm entries supplement the table and replace the
            // free entries it gives objects only a cross-reference stream can locate.
            if let Some(x) = sect_trailer.int(b"XRefStm").filter(|x| *x >= 0) {
                let mut stm = BTreeMap::new();
                match self.read_section(x as usize, &mut stm) {
                    Ok(_) => {
                        for (num, e) in stm {
                            if sect.get(&num).is_none_or(|old| matches!(old, XrefEntry::Free { .. })) {
                                sect.insert(num, e);
                            }
                        }
                    }
                    Err(e) => log.push(format!("hybrid /XRefStm at {x} unreadable: {e}")),
                }
            }
            for (num, e) in sect {
                entries.entry(num).or_insert(e);
            }
            next = sect_trailer.int(b"Prev").filter(|p| *p >= 0).map(|p| p as u64);
            if trailer.is_none() {
                trailer = Some(sect_trailer);
            }
        }
        revisions.reverse();
        let mut trailer = trailer.ok_or(CosError::Syntax { offset: 0, detail: "no trailer".into() })?;
        for k in [&b"Prev"[..], b"XRefStm", b"Type", b"W", b"Index", b"Filter", b"DecodeParms", b"Length"] {
            trailer.remove(k);
        }
        Ok((entries, trailer, revisions))
    }

    /// Read one section at `off`, adding entries not already known (newer sections win).
    fn read_section(&self, off: usize, entries: &mut BTreeMap<u32, XrefEntry>) -> Result<(Dict, bool), CosError> {
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
                    // Offset 0 "in use" entries are a known producer bug: ignore them.
                    if matches!(entry, XrefEntry::InFile { offset: 0, .. }) {
                        continue;
                    }
                    entries.entry(num).or_insert(entry);
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
        let raw = s.decoded_within(MAX_XREF_OBJECTS.saturating_mul(row))?;
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
                    _ => continue, // reserved types are treated as null references (§7.5.8.3)
                };
                if matches!(entry, XrefEntry::InFile { offset: 0, .. }) {
                    continue;
                }
                entries.entry(num).or_insert(entry);
            }
        }
        Ok((s.dict.clone(), true))
    }

    /// Rebuild the cross-reference index by scanning for `N G obj` headers (§7.5.4 recovery).
    fn reconstruct(&mut self, log: &mut Vec<String>) -> Result<(), CosError> {
        let data = self.data.clone();
        let mut entries: BTreeMap<u32, XrefEntry> = BTreeMap::new();
        let mut i = 0;
        let mut objstms = Vec::new();
        while i < data.len() {
            // Object headers start at a line start (or after whitespace).
            let at_start = i == 0 || is_whitespace(data[i - 1]) || data[i - 1] == b'>';
            if at_start && data[i].is_ascii_digit() {
                let mut lx = Lexer::new(&data, i);
                let n = lx.token();
                lx.skip_ws();
                let g = lx.token();
                if !n.is_empty() && !g.is_empty() && n.iter().all(u8::is_ascii_digit) && g.iter().all(u8::is_ascii_digit) && lx.eat_keyword(b"obj") {
                    let num = std::str::from_utf8(n).ok().and_then(|s| s.parse::<u32>().ok());
                    let generation = std::str::from_utf8(g).ok().and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
                    if let Some(num) = num {
                        entries.insert(num, XrefEntry::InFile { offset: i as u64, generation });
                        objstms.push(num);
                    }
                    i = lx.pos;
                    continue;
                }
            }
            i += 1;
        }
        if entries.is_empty() {
            return Err(CosError::Syntax { offset: 0, detail: "no objects found".into() });
        }
        self.entries = Arc::new(entries.clone());
        self.cache.lock().map_err(|_| CosError::Poisoned)?.clear();
        // Objects inside object streams.
        for num in objstms {
            if let Ok(o) = self.try_get(num)
                && let Object::Stream(s) = &*o
                && s.dict.name(b"Type") == Some(b"ObjStm")
                && let Ok(stm) = self.objstm(num)
            {
                for (idx, (inner, _)) in stm.index.iter().enumerate() {
                    entries.entry(*inner).or_insert(XrefEntry::InStream { stream: num, index: idx as u32 });
                }
            }
        }
        self.entries = Arc::new(entries);
        self.cache.lock().map_err(|_| CosError::Poisoned)?.clear();
        // Trailer: the last `trailer` dictionary with a usable /Root, else the catalog itself.
        let mut trailer = None;
        let mut from = 0;
        while let Some(p) = find(&data, b"trailer", from, data.len()) {
            from = p + 7;
            let mut lx = Lexer::new(&data, from);
            if let Ok(Object::Dict(d)) = lx.object()
                && d.reference(b"Root").is_some_and(|r| matches!(self.try_get(r.num).as_deref(), Ok(Object::Dict(_))))
            {
                trailer = Some(d);
            }
        }
        let trailer = match trailer {
            Some(t) => t,
            None => {
                let catalog = self
                    .entries
                    .keys()
                    .copied()
                    .find(|n| matches!(self.try_get(*n).as_deref(), Ok(Object::Dict(d)) if d.name(b"Type") == Some(b"Catalog")));
                let Some(c) = catalog else { return Err(CosError::Syntax { offset: 0, detail: "no document catalog found".into() }) };
                let mut t = Dict::new();
                t.set(b"Root".to_vec(), Object::Ref(ObjRef::new(c, self.generation(c))));
                t
            }
        };
        let mut trailer = trailer;
        for k in [&b"Prev"[..], b"XRefStm"] {
            trailer.remove(k);
        }
        self.trailer = trailer;
        self.revisions = Arc::new(Vec::new());
        log.push(format!("rebuilt cross-reference index from {} object headers", self.entries.len()));
        Ok(())
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
        let inner = b"3 0 << /Hidden (yes) >>";
        let mut stm = format!("<< /Type /ObjStm /N 1 /First 4 /Length {} >>\nstream\n", inner.len()).into_bytes();
        stm.extend_from_slice(inner);
        stm.extend_from_slice(b"\nendstream");
        obj(&mut out, 4, &stm);
        let data = [2u8, 0, 4, 0];
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
        let doc = Document::open(Arc::new(out)).unwrap();
        assert!(doc.repair_log().is_empty(), "{:?}", doc.repair_log());
        let hidden = doc.get(ObjRef::new(3, 0));
        assert_eq!(hidden.as_dict().and_then(|d| d.get(b"Hidden").and_then(|h| h.as_string().map(|s| s.to_text()))), Some("yes".into()));
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
