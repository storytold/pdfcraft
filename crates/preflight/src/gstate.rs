//! PDF/A-2 6.2.5, the extended graphics state: device-dependent settings a conforming file may not
//! rely on.
//!
//! - no transfer function: no `/TR` entry, and `/TR2` only as `/Default` (ISO 32000-1 10.4);
//! - no `/HTP` (halftone phase) entry, a PDF 1.2 key ISO 32000-1 no longer defines;
//! - halftones of type 1 or 5 only, without a `/HalftoneName` (ISO 32000-1 10.5.5); the halftones
//!   a type 5 halftone holds for its colorants are checked the same way;
//! - a halftone's `/TransferFunction` only where one is needed: a type 5 halftone's component for
//!   a non-primary colorant has one, and no other halftone does. The primary colorants are those
//!   of DeviceCMYK; the `Default` component is left as it is (ISO 32000-1 10.5.5.6, Table 130);
//! - a rendering intent (`/RI`) that is one of the four of ISO 32000-1 8.6.5.8, Table 70.
//!
//! Graphics state dictionaries are found as the `/ExtGState` entries of resource dictionaries
//! (`/Resources` of pages, forms, Type 3 fonts, patterns and annotation appearances, and an
//! interactive form's `/DR`), and as the `/ExtGState` of a shading pattern (ISO 32000-1 8.7.4.3,
//! Table 76). They are looked for in what the document reaches from its trailer, following
//! references, but not into private application data (`/PieceInfo`), so an object only that data
//! refers to isn't read. A map shared by reference is read once.

use std::collections::{BTreeSet, HashSet, VecDeque};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

/// How deep a direct object is followed: as deep as the parser nests them.
const MAX_DEPTH: u8 = 128;
/// The rendering intents of ISO 32000-1 Table 70.
const INTENTS: [&[u8]; 4] = [b"AbsoluteColorimetric", b"RelativeColorimetric", b"Saturation", b"Perceptual"];
/// The entries of a type 5 halftone that aren't colorant halftones.
const HALFTONE_KEYS: [&[u8]; 3] = [b"Type", b"HalftoneType", b"HalftoneName"];
/// The primary colorants, whose halftones take the graphics state's transfer function.
const PRIMARY: [&[u8]; 4] = [b"Cyan", b"Magenta", b"Yellow", b"Black"];
/// The entries that hold resource dictionaries.
const RESOURCES: [&[u8]; 2] = [b"Resources", b"DR"];

/// What the graphics states of a document break of 6.2.5.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Findings {
    /// A graphics state has a `/TR` entry.
    pub(crate) transfer: bool,
    /// A graphics state has a `/TR2` other than `/Default`.
    pub(crate) transfer2: bool,
    /// A graphics state has an `/HTP`.
    pub(crate) phase: bool,
    /// Halftone types other than 1 and 5, in order.
    pub(crate) halftone_types: BTreeSet<i64>,
    /// A halftone's `/HalftoneType` is missing or isn't an integer.
    pub(crate) untyped: bool,
    /// A halftone has a `/HalftoneName`.
    pub(crate) halftone_name: bool,
    /// A halftone that may not have a `/TransferFunction` has one.
    pub(crate) halftone_transfer: bool,
    /// Non-primary colorants whose halftone has no `/TransferFunction`, in name order.
    pub(crate) missing_transfer: BTreeSet<String>,
    /// Rendering intents outside Table 70, in name order.
    pub(crate) intents: BTreeSet<String>,
    /// A rendering intent isn't a name.
    pub(crate) intent_not_name: bool,
}

/// The 6.2.5 findings over what `doc` reaches from its trailer.
pub(crate) fn check(doc: &Document) -> Findings {
    scan(doc).findings
}

/// How many objects [`check`] reads.
#[cfg(test)]
pub(crate) fn reached(doc: &Document) -> usize {
    scan(doc).reached.len()
}

fn scan(doc: &Document) -> Scan<'_> {
    let mut scan = Scan { doc, read: HashSet::new(), reached: HashSet::new(), queue: VecDeque::new(), findings: Findings::default() };
    scan.walk(&Object::Dict(doc.trailer().clone()), 0);
    // References are followed from a queue, not by recursion: a chain of them can be as long as
    // the file.
    while let Some(r) = scan.queue.pop_front() {
        let o = doc.get(r);
        scan.walk(&o, 0);
    }
    scan
}

struct Scan<'a> {
    doc: &'a Document,
    /// Resource dictionaries, `/ExtGState` maps and graphics states read so far, by object number.
    read: HashSet<u32>,
    /// Object numbers reached so far, queued or walked. Both sets go by number, as
    /// [`Document::get`] does: references differing only in generation reach the same object.
    reached: HashSet<u32>,
    queue: VecDeque<ObjRef>,
    findings: Findings,
}

impl Scan<'_> {
    fn walk(&mut self, o: &Object, depth: u8) {
        if depth > MAX_DEPTH {
            return;
        }
        let d = match o {
            Object::Ref(r) => {
                if self.reached.insert(r.num) {
                    self.queue.push_back(*r);
                }
                return;
            }
            Object::Array(a) => {
                for v in a {
                    self.walk(v, depth.saturating_add(1));
                }
                return;
            }
            Object::Dict(d) => d,
            Object::Stream(s) => &s.dict,
            _ => return,
        };
        self.holder(d);
        for (k, v) in d.iter() {
            if k.as_slice() != b"PieceInfo" {
                self.walk(v, depth.saturating_add(1));
            }
        }
    }

    /// Whether `o` is a reference read before; marks it read.
    fn read_before(&mut self, o: &Object) -> bool {
        matches!(o, Object::Ref(r) if !self.read.insert(r.num))
    }

    /// A dictionary that holds graphics states: through its resources, or as a shading pattern.
    fn holder(&mut self, d: &Dict) {
        for key in RESOURCES {
            if let Some(resources) = d.get(key) {
                self.resources(resources);
            }
        }
        if d.get(b"PatternType").is_some()
            && let Some(gs) = d.get(b"ExtGState")
            && !self.read_before(gs)
            && let Some(gs) = self.doc.resolve(gs).as_dict()
        {
            self.gstate(gs);
        }
    }

    fn resources(&mut self, resources: &Object) {
        if self.read_before(resources) {
            return;
        }
        let resources = self.doc.resolve(resources);
        let Some(states) = resources.as_dict().and_then(|r| r.get(b"ExtGState")) else { return };
        if self.read_before(states) {
            return;
        }
        let states = self.doc.resolve(states);
        let Some(states) = states.as_dict() else { return };
        for (_, gs) in states.iter() {
            if !self.read_before(gs)
                && let Some(gs) = self.doc.resolve(gs).as_dict()
            {
                self.gstate(gs);
            }
        }
    }

    /// `key` of `d`, resolved, unless it is missing or null (ISO 32000-1 7.3.9).
    fn present(&self, d: &Dict, key: &[u8]) -> Option<std::sync::Arc<Object>> {
        d.get(key).map(|v| self.doc.resolve(v)).filter(|v| !matches!(**v, Object::Null))
    }

    fn gstate(&mut self, gs: &Dict) {
        if self.present(gs, b"TR").is_some() {
            self.findings.transfer = true;
        }
        if self.present(gs, b"TR2").is_some_and(|f| f.as_name() != Some(b"Default")) {
            self.findings.transfer2 = true;
        }
        if self.present(gs, b"HTP").is_some() {
            self.findings.phase = true;
        }
        if let Some(ht) = self.present(gs, b"HT")
            && let Some(ht) = ht.as_dict()
        {
            self.halftone(ht, None);
        }
        if let Some(ri) = self.present(gs, b"RI") {
            match ri.as_name() {
                Some(ri) if !INTENTS.contains(&ri) => {
                    self.findings.intents.insert(String::from_utf8_lossy(ri).into_owned());
                }
                Some(_) => {}
                None => self.findings.intent_not_name = true,
            }
        }
    }

    /// A halftone dictionary or stream: the graphics state's own (`colorant` is `None`), or the
    /// one a type 5 halftone holds for `colorant`.
    fn halftone(&mut self, ht: &Dict, colorant: Option<&[u8]>) {
        // An integer (ISO 32000-1 Table 128), not a real of the same value.
        let kind = self.present(ht, b"HalftoneType").and_then(|t| if let Object::Int(t) = *t { Some(t) } else { None });
        match kind {
            None => self.findings.untyped = true,
            Some(1 | 5) => {}
            Some(kind) => {
                self.findings.halftone_types.insert(kind);
            }
        }
        if self.present(ht, b"HalftoneName").is_some() {
            self.findings.halftone_name = true;
        }
        let transfer = self.present(ht, b"TransferFunction").is_some();
        match colorant {
            Some(b"Default") => {}
            Some(c) if !PRIMARY.contains(&c) => {
                if !transfer {
                    self.findings.missing_transfer.insert(String::from_utf8_lossy(c).into_owned());
                }
            }
            _ => self.findings.halftone_transfer |= transfer,
        }
        if colorant.is_none() && kind == Some(5) {
            for (k, v) in ht.iter() {
                if HALFTONE_KEYS.contains(&k.as_slice()) {
                    continue;
                }
                if let Some(component) = self.doc.resolve(v).as_dict() {
                    self.halftone(component, Some(k));
                }
            }
        }
    }
}
