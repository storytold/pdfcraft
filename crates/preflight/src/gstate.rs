//! PDF/A-2 6.2.5, the extended graphics state: device-dependent settings a conforming file may not
//! rely on.
//!
//! - no transfer function: no `/TR` entry, and `/TR2` only as `/Default` (ISO 32000-1 10.4);
//! - no `/HTP` (halftone phase) entry, a PDF 1.2 key ISO 32000-1 no longer defines;
//! - halftones of type 1 or 5 only, without a `/HalftoneName` (ISO 32000-1 10.5.5); the halftones
//!   a type 5 halftone holds for its colorants are checked the same way;
//! - a rendering intent (`/RI`) that is one of the four of ISO 32000-1 8.6.5.8, Table 70.
//!
//! Graphics state dictionaries are found as the entries of every `/ExtGState` resource
//! dictionary, and as the `/ExtGState` of a shading pattern (ISO 32000-1 8.7.4.3, Table 76),
//! direct or by reference.

use std::collections::BTreeSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

/// How deep a direct object is followed: as deep as the parser nests them.
const MAX_DEPTH: u8 = 128;
/// The rendering intents of ISO 32000-1 Table 70.
const INTENTS: [&[u8]; 4] = [b"AbsoluteColorimetric", b"RelativeColorimetric", b"Saturation", b"Perceptual"];
/// The entries of a type 5 halftone that aren't colorant halftones.
const HALFTONE_KEYS: [&[u8]; 3] = [b"Type", b"HalftoneType", b"HalftoneName"];

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
    /// A halftone has a `/HalftoneName`.
    pub(crate) halftone_name: bool,
    /// Rendering intents outside Table 70, in name order.
    pub(crate) intents: BTreeSet<String>,
}

/// The 6.2.5 findings over the indirect objects `objs`.
pub(crate) fn check(doc: &Document, objs: &[(ObjRef, std::sync::Arc<Object>)]) -> Findings {
    let mut scan = Scan { doc, findings: Findings::default() };
    for (_, o) in objs {
        scan.walk(o, 0);
    }
    scan.findings
}

struct Scan<'a> {
    doc: &'a Document,
    findings: Findings,
}

impl Scan<'_> {
    fn walk(&mut self, o: &Object, depth: u8) {
        if depth > MAX_DEPTH {
            return;
        }
        let entries: Box<dyn Iterator<Item = &Object>> = match o {
            Object::Array(a) => Box::new(a.iter()),
            Object::Dict(d) => {
                self.dict(d);
                Box::new(d.iter().map(|(_, v)| v))
            }
            Object::Stream(s) => {
                self.dict(&s.dict);
                Box::new(s.dict.iter().map(|(_, v)| v))
            }
            _ => return,
        };
        for v in entries {
            self.walk(v, depth.saturating_add(1));
        }
    }

    /// A dictionary that holds graphics states: a shading pattern its one, resources by name.
    fn dict(&mut self, d: &Dict) {
        let Some(states) = d.get(b"ExtGState").map(|s| self.doc.resolve(s)) else { return };
        let Some(states) = states.as_dict() else { return };
        if d.get(b"PatternType").is_some() {
            self.gstate(states);
            return;
        }
        for (_, gs) in states.iter() {
            if let Some(gs) = self.doc.resolve(gs).as_dict() {
                self.gstate(gs);
            }
        }
    }

    fn gstate(&mut self, gs: &Dict) {
        let doc = self.doc;
        let present = |k: &[u8]| gs.get(k).map(|v| doc.resolve(v)).filter(|v| !matches!(**v, Object::Null));
        if present(b"TR").is_some() {
            self.findings.transfer = true;
        }
        if present(b"TR2").is_some_and(|f| f.as_name() != Some(b"Default")) {
            self.findings.transfer2 = true;
        }
        if present(b"HTP").is_some() {
            self.findings.phase = true;
        }
        if let Some(ht) = present(b"HT")
            && let Some(ht) = ht.as_dict()
        {
            self.halftone(ht, true);
        }
        if let Some(ri) = present(b"RI")
            && let Some(ri) = ri.as_name()
            && !INTENTS.contains(&ri)
        {
            self.findings.intents.insert(String::from_utf8_lossy(ri).into_owned());
        }
    }

    /// A halftone dictionary or stream; `top` when it isn't one a type 5 halftone holds, whose
    /// own entries are halftones in turn.
    fn halftone(&mut self, ht: &Dict, top: bool) {
        let kind = ht.get(b"HalftoneType").and_then(|t| self.doc.resolve(t).as_int());
        if let Some(kind) = kind
            && kind != 1
            && kind != 5
        {
            self.findings.halftone_types.insert(kind);
        }
        if ht.get(b"HalftoneName").is_some() {
            self.findings.halftone_name = true;
        }
        if top && kind == Some(5) {
            for (k, v) in ht.iter() {
                if HALFTONE_KEYS.contains(&k.as_slice()) {
                    continue;
                }
                if let Some(component) = self.doc.resolve(v).as_dict() {
                    self.halftone(component, false);
                }
            }
        }
    }
}
