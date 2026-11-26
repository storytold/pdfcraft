//! PDF/A-2 6.1.13: a file stays within the implementation limits of ISO 32000-1 Annex C (Table C.1
//! and the page size limits after it): integers that fit in 32 bits, strings of at most 32767
//! bytes, names of at most 127 bytes, and page boundaries between 3 and 14400 units wide and high.
//!
//! The values are read from the trailer, the indirect objects and the content streams, as parsed:
//! a name written with `#xx` escapes counts its decoded bytes. A page's `/Contents` streams are read
//! joined, as one content stream (an operator's operands may end one stream and the operator start
//! the next, ISO 32000-1 7.8.2). Content is decoded within one allowance for the whole document.
//! Real numbers beyond ±3.403 × 10³⁸ are reported, and so are nonzero ones nearer zero than
//! ±1.175 × 10⁻³⁸ (ISO 32000-1 Table C.1 lets a reader round them to zero; PDF/A-2 does not allow
//! them, PDF Association TechNote 0010 A018), and so is an embedded CMap of a Type 0 font
//! (or one it uses through `/UseCMap`) that maps a code to a CID above 65535. An integer token too
//! long even for a real reads as one past that range, and is reported as a real.

use std::collections::HashSet;

use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object};

/// Table C.1: the smallest and largest integer values.
const MIN_INT: i64 = i32::MIN as i64;
const MAX_INT: i64 = i32::MAX as i64;
/// Table C.1: the largest real value (approximate).
const MAX_REAL: f64 = 3.403e38;
/// Table C.1: the nonzero real value closest to zero (approximate).
const MIN_REAL: f64 = 1.175e-38;
/// Table C.1: the largest character identifier.
const MAX_CID: u64 = 65535;
/// The decoded CMap data, across all the document's embedded CMaps, the scan reads.
const MAX_CMAP_BYTES: usize = 16 << 20;
/// How many CMaps a `/UseCMap` chain is followed through.
const MAX_CMAP_CHAIN: usize = 8;
/// Table C.1: the longest string, in bytes.
const MAX_STRING: usize = 32767;
/// Table C.1: the longest name, in bytes.
const MAX_NAME: usize = 127;
/// The smallest and largest page boundary, in default user space units.
const MIN_PAGE: f64 = 3.0;
const MAX_PAGE: f64 = 14400.0;
/// How deep a direct object is followed (arrays and dictionaries inside one another): as deep as
/// the parser nests them.
const MAX_DEPTH: u8 = 128;
/// The decoded content, across all the document's content streams, the scan reads.
const MAX_CONTENT_BYTES: usize = 256 << 20;
/// The page boundaries, of which only the media and crop boxes are inherited.
const BOXES: [&[u8]; 5] = [b"MediaBox", b"CropBox", b"BleedBox", b"TrimBox", b"ArtBox"];

/// Which limits a value went past.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Exceeded {
    pub(crate) int: bool,
    pub(crate) real: bool,
    pub(crate) string: bool,
    pub(crate) name: bool,
}

impl Exceeded {
    fn all(self) -> bool {
        self.int && self.real && self.string && self.name
    }

    fn name(&mut self, n: &[u8]) {
        self.name |= n.len() > MAX_NAME;
    }

    /// One value and, within [`MAX_DEPTH`], what it holds: a dictionary's keys count as names.
    fn value(&mut self, o: &Object, depth: u8) {
        if self.all() || depth > MAX_DEPTH {
            return;
        }
        match o {
            Object::Int(i) => self.int |= !(MIN_INT..=MAX_INT).contains(i),
            Object::Real(r) => self.real |= !r.is_finite() || r.abs() > MAX_REAL || (*r != 0.0 && r.abs() < MIN_REAL),
            Object::String(s) => self.string |= s.bytes.len() > MAX_STRING,
            Object::Name(n) => self.name(n),
            Object::Array(a) => {
                for v in a {
                    self.value(v, depth.saturating_add(1));
                }
            }
            Object::Dict(d) => self.dict(d, depth),
            Object::Stream(s) => self.dict(&s.dict, depth),
            Object::Null | Object::Bool(_) | Object::Ref(_) => {}
        }
    }

    /// The operands of one content stream, inline images' dictionaries included.
    fn content(&mut self, data: &[u8]) {
        for op in pdfcraft_content::ops(data) {
            if self.all() {
                return;
            }
            for v in &op.operands {
                self.value(v, 0);
            }
            if let Some((image, _)) = &op.inline {
                self.dict(image, 0);
            }
        }
    }

    fn dict(&mut self, d: &Dict, depth: u8) {
        for (k, v) in d.iter() {
            self.name(k);
            self.value(v, depth.saturating_add(1));
        }
    }
}

/// The limits that the trailer, the indirect objects `objs` and the content streams go past.
/// `content` holds the content streams only a reference identifies (a page's `/Contents`, Type 3
/// glyph procedures), besides form XObjects and appearance streams, which say so themselves.
pub(crate) fn exceeded(doc: &Document, objs: &[(ObjRef, std::sync::Arc<Object>)], content: &HashSet<ObjRef>) -> Exceeded {
    let mut found = Exceeded::default();
    found.dict(doc.trailer(), 0);
    for (_, o) in objs {
        if found.all() {
            return found;
        }
        found.value(o, 0);
    }
    let mut budget = DecodeBudget::new(MAX_CONTENT_BYTES, 0);
    let mut read: HashSet<ObjRef> = HashSet::new();
    for page in pdfcraft_model::pages(doc) {
        if found.all() || budget.remaining() == 0 {
            return found;
        }
        let refs = contents(doc, &page.dict);
        let mut joined = Vec::new();
        for r in &refs {
            if let Object::Stream(s) = &*doc.get(*r)
                && let Ok(data) = budget.decode(s)
            {
                joined.extend_from_slice(&data);
                joined.push(b' ');
            }
        }
        read.extend(refs);
        found.content(&joined);
    }
    for (r, o) in objs {
        if found.all() || budget.remaining() == 0 {
            break;
        }
        if let Object::Stream(s) = &**o
            && !read.contains(r)
            && (content.contains(r) || crate::is_content(&s.dict))
            && let Ok(data) = budget.decode(s)
        {
            found.content(&data);
        }
    }
    found
}

/// A page's `/Contents` streams, in order.
fn contents(doc: &Document, page: &Dict) -> Vec<ObjRef> {
    match page.get(b"Contents") {
        Some(Object::Ref(r)) => match &*doc.get(*r) {
            Object::Array(a) => a.iter().filter_map(Object::as_ref).collect(),
            _ => vec![*r],
        },
        Some(Object::Array(a)) => a.iter().filter_map(Object::as_ref).collect(),
        _ => Vec::new(),
    }
}

/// Whether one of a page's boundaries (with the media and crop boxes it inherits) is narrower or
/// lower than 3 units, or wider or higher than 14400.
pub(crate) fn page_box_out_of_range(doc: &Document, page: &Dict) -> bool {
    BOXES.iter().filter_map(|k| page.get(k)).any(|b| {
        let b = doc.resolve(b);
        let v: Vec<f64> = b.as_array().map(|a| a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect()).unwrap_or_default();
        let [x0, y0, x1, y1] = v.as_slice() else { return false };
        let (w, h) = ((x1 - x0).abs(), (y1 - y0).abs());
        // A size past `f64` (corners near ±1e308) is as much too large as any other.
        [w, h].iter().any(|s| !(MIN_PAGE..=MAX_PAGE).contains(s))
    })
}

/// Whether an embedded CMap of a Type 0 font among `objs`, or one it uses through `/UseCMap`, maps
/// a code to a CID above 65535: a `begincidrange` entry `<low> <high> cid` reaches
/// `cid + high − low`, a `begincidchar` entry `<code> cid` reaches `cid` (ISO 32000-1 9.7.5.4). A
/// CMap named rather than embedded is a predefined one, within the limit. The CMaps are decoded
/// within one allowance for the document.
pub(crate) fn cid_over_limit(doc: &Document, objs: &[(ObjRef, std::sync::Arc<Object>)]) -> bool {
    let mut budget = DecodeBudget::new(MAX_CMAP_BYTES, 0);
    let mut read: HashSet<ObjRef> = HashSet::new();
    for (_, o) in objs {
        let Some(d) = o.as_dict() else { continue };
        if d.name(b"Subtype") != Some(b"Type0") {
            continue;
        }
        let mut next = d.get(b"Encoding").and_then(Object::as_ref);
        for _ in 0..MAX_CMAP_CHAIN {
            let Some(r) = next.filter(|r| read.insert(*r)) else { break };
            if budget.remaining() == 0 {
                return false;
            }
            let Object::Stream(cmap) = &*doc.get(r) else { break };
            if budget.decode(cmap).is_ok_and(|data| maps_past_limit(&data)) {
                return true;
            }
            next = cmap.dict.get(b"UseCMap").and_then(Object::as_ref);
        }
    }
    false
}

/// Whether the CMap program `data` maps a code to a CID above [`MAX_CID`]. The entries of a
/// `begincidrange` … `endcidrange` block are the operands of its `endcidrange`, read as content is.
fn maps_past_limit(data: &[u8]) -> bool {
    let value = |o: &Object| -> Option<u64> {
        match o {
            Object::String(s) if s.bytes.len() <= 4 => Some(s.bytes.iter().fold(0u64, |v, b| (v << 8) | u64::from(*b))),
            Object::Int(i) => u64::try_from(*i).ok(),
            _ => None,
        }
    };
    pdfcraft_content::ops(data).any(|op| match op.op.as_slice() {
        b"endcidrange" => op.operands.as_chunks::<3>().0.iter().any(|[low, high, cid]| match (value(low), value(high), value(cid)) {
            (Some(low), Some(high), Some(cid)) => cid.saturating_add(high.saturating_sub(low)) > MAX_CID,
            _ => false,
        }),
        b"endcidchar" => op.operands.as_chunks::<2>().0.iter().any(|[_, cid]| value(cid).is_some_and(|cid| cid > MAX_CID)),
        _ => false,
    })
}
