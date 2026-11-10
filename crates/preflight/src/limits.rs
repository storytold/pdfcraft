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
    let Some(contents) = page.get(b"Contents") else { return Vec::new() };
    match &*doc.resolve(contents) {
        Object::Array(a) => a.iter().filter_map(|o| resolve_ref(doc, o)).collect(),
        _ => resolve_ref(doc, contents).into_iter().collect(),
    }
}

/// The terminal indirect reference of `o`, when `o` resolves to a non-reference object.
fn resolve_ref(doc: &Document, o: &Object) -> Option<ObjRef> {
    let mut r = o.as_ref()?;
    for _ in 0..=32 {
        let Object::Ref(next) = &*doc.get(r) else { return Some(r) };
        r = *next;
    }
    None
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
/// a code to a CID above 65535: a `begincidrange` or `beginnotdefrange` entry `<low> <high> cid`
/// reaches `cid + high − low` (`begincidrange`) or `cid` (`beginnotdefrange`), and a
/// `begincidchar` or `beginnotdefchar` entry `<code> cid` reaches `cid` (ISO 32000-1 9.7.5.4,
/// 9.7.6.2). A CMap named rather than embedded is a predefined one, within the limit. The CMaps
/// are decoded within one allowance for the document.
pub(crate) fn cid_over_limit(doc: &Document, objs: &[(ObjRef, std::sync::Arc<Object>)]) -> bool {
    let mut budget = DecodeBudget::new(MAX_CMAP_BYTES, 0);
    let mut read: HashSet<ObjRef> = HashSet::new();
    let mut encodings = Vec::new();
    for (_, o) in objs {
        collect_type0_encodings(doc, o, 0, &mut encodings);
    }
    for enc in encodings {
        let mut next = Some(enc);
        for _ in 0..MAX_CMAP_CHAIN {
            let Some(enc) = next else { break };
            if let Some(r) = resolve_ref(doc, &enc)
                && !read.insert(r)
            {
                break;
            }
            if budget.remaining() == 0 {
                return false;
            }
            let resolved = doc.resolve(&enc);
            let Object::Stream(cmap) = &*resolved else { break };
            if budget.decode(cmap).is_ok_and(|data| maps_past_limit(&data)) {
                return true;
            }
            next = cmap.dict.get(b"UseCMap").cloned();
        }
    }
    false
}

/// Collect the `/Encoding` of every Type 0 font in `o`, whether `o` itself or nested directly inside it.
fn collect_type0_encodings(doc: &Document, o: &Object, depth: u8, out: &mut Vec<Object>) {
    if depth > MAX_DEPTH {
        return;
    }
    match o {
        Object::Dict(d) => {
            if d.get(b"Subtype").map(|s| doc.resolve(s)).as_deref().and_then(Object::as_name) == Some(b"Type0")
                && let Some(enc) = d.get(b"Encoding")
            {
                out.push(enc.clone());
            }
            for (_, v) in d.iter() {
                collect_type0_encodings(doc, v, depth.saturating_add(1), out);
            }
        }
        Object::Stream(s) => {
            for (_, v) in s.dict.iter() {
                collect_type0_encodings(doc, v, depth.saturating_add(1), out);
            }
        }
        Object::Array(a) => {
            for v in a {
                collect_type0_encodings(doc, v, depth.saturating_add(1), out);
            }
        }
        _ => {}
    }
}

/// Whether the CMap program `data` maps a code to a CID above [`MAX_CID`]. The entries of a
/// `begincidrange` … `endcidrange` block are the operands of its `endcidrange`, read as content is.
fn maps_past_limit(data: &[u8]) -> bool {
    let value = |o: &Object| -> Option<u64> {
        match o {
            Object::String(s) if !s.bytes.is_empty() && s.bytes.len() <= 4 => Some(s.bytes.iter().fold(0u64, |v, b| (v << 8) | u64::from(*b))),
            Object::Int(i) => u64::try_from(*i).ok(),
            _ => None,
        }
    };
    let valid_codes = |low: &Object, high: &Object| -> Option<(u64, u64)> {
        match (low, high) {
            (Object::String(a), Object::String(b)) if a.bytes.len() == b.bytes.len() => {
                let (low, high) = (value(low)?, value(high)?);
                (low <= high).then_some((low, high))
            }
            _ => None,
        }
    };
    pdfcraft_content::ops(data).any(|op| match op.op.as_slice() {
        b"endcidrange" => op.operands.as_chunks::<3>().0.iter().any(|[low, high, cid]| match (valid_codes(low, high), value(cid)) {
            (Some((low, high)), Some(cid)) => cid.saturating_add(high - low) > MAX_CID,
            _ => false,
        }),
        b"endnotdefrange" => op
            .operands
            .as_chunks::<3>()
            .0
            .iter()
            .any(|[low, high, cid]| valid_codes(low, high).is_some() && value(cid).is_some_and(|cid| cid > MAX_CID)),
        b"endcidchar" | b"endnotdefchar" => {
            op.operands.as_chunks::<2>().0.iter().any(|[code, cid]| valid_codes(code, code).is_some() && value(cid).is_some_and(|cid| cid > MAX_CID))
        }
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use pdfcraft_cos::{Dict, Document, Object, Stream};

    use super::{Exceeded, cid_over_limit, exceeded, maps_past_limit};
    use crate::objects;

    #[test]
    fn cmap_limit_checks_notdef_mappings_and_chained_or_direct_cmaps() {
        assert!(maps_past_limit(b"<0000> 65536 endnotdefchar"));
        assert!(maps_past_limit(b"<0000> <0010> 65536 endnotdefrange"));
        // An inverted range (high < low) or mismatched byte lengths does not map any valid character code.
        assert!(!maps_past_limit(b"<0010> <0000> 65535 endcidrange"));
        assert!(!maps_past_limit(b"<0010> <0000> 65536 endnotdefrange"));
        assert!(!maps_past_limit(b"<00> <0010> 65535 endcidrange"));
        assert!(!maps_past_limit(b"<> 65536 endcidchar"));
        assert!(maps_past_limit(b"<0000> <0002> 65534 endcidrange"));
        assert!(!maps_past_limit(b"<0000> <0002> 65533 endcidrange"));

        let mut doc = Document::new_empty();
        let subtype_ref = doc.add(Object::name("Type0"));
        let base_cmap = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"<0000> 70000 endcidchar".to_vec())));
        let base_alias = doc.add(Object::Ref(base_cmap));
        let mut child_dict = Dict::new();
        child_dict.set(b"UseCMap".to_vec(), Object::Ref(base_alias));
        let child_cmap = doc.add(Object::Stream(Stream::from_raw(child_dict, b"<0000> 1 endcidchar".to_vec())));
        let child_alias = doc.add(Object::Ref(child_cmap));

        // Inline Type 0 font inside a resource dictionary with indirect /Subtype and chained /Encoding + /UseCMap.
        let mut font = Dict::new();
        font.set(b"Type".to_vec(), Object::name("Font"));
        font.set(b"Subtype".to_vec(), Object::Ref(subtype_ref));
        font.set(b"Encoding".to_vec(), Object::Ref(child_alias));
        let mut font_res = Dict::new();
        font_res.set(b"F1".to_vec(), Object::Dict(font));
        let mut res = Dict::new();
        res.set(b"Font".to_vec(), Object::Dict(font_res));
        doc.add(Object::Dict(res));
        assert!(cid_over_limit(&doc, &objects(&doc)));

        let mut direct_doc = Document::new_empty();
        let mut direct_font = Dict::new();
        direct_font.set(b"Subtype".to_vec(), Object::name("Type0"));
        direct_font.set(b"Encoding".to_vec(), Object::Stream(Stream::from_raw(Dict::new(), b"<0000> 70000 endcidchar".to_vec())));
        direct_doc.add(Object::Dict(direct_font));
        assert!(cid_over_limit(&direct_doc, &objects(&direct_doc)));

        // Reference cycles and UseCMap cycles terminate cleanly.
        let mut cycle_doc = Document::new_empty();
        let cycle_ref = cycle_doc.add(Object::Null);
        cycle_doc.set(cycle_ref, Object::Ref(cycle_ref));
        let mut cmap_a_dict = Dict::new();
        let cmap_a = cycle_doc.add(Object::Null);
        cmap_a_dict.set(b"UseCMap".to_vec(), Object::Ref(cmap_a));
        cycle_doc.set(cmap_a, Object::Stream(Stream::from_raw(cmap_a_dict, b"<0000> 1 endcidchar".to_vec())));
        for enc in [cycle_ref, cmap_a] {
            let mut f = Dict::new();
            f.set(b"Subtype".to_vec(), Object::name("Type0"));
            f.set(b"Encoding".to_vec(), Object::Ref(enc));
            cycle_doc.add(Object::Dict(f));
        }
        assert!(!cid_over_limit(&cycle_doc, &objects(&cycle_doc)));
    }

    #[test]
    fn exceeded_joins_chained_indirect_page_contents() {
        let mut doc = Document::new_empty();
        let s1 = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"3000000000 ".to_vec())));
        let mut s1_alias = s1;
        for _ in 0..32 {
            s1_alias = doc.add(Object::Ref(s1_alias));
        }
        let s2 = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"0 Td".to_vec())));
        let s2_alias = doc.add(Object::Ref(s2));
        let arr = doc.add(Object::Array(vec![Object::Ref(s1_alias), Object::Ref(s2_alias)]));
        let arr_alias = doc.add(Object::Ref(arr));

        let mut page = Dict::new();
        page.set(b"Type".to_vec(), Object::name("Page"));
        page.set(b"Contents".to_vec(), Object::Ref(arr_alias));
        let page_ref = doc.add(Object::Dict(page));

        let mut pages = Dict::new();
        pages.set(b"Type".to_vec(), Object::name("Pages"));
        pages.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page_ref)]));
        pages.set(b"Count".to_vec(), Object::Int(1));
        let pages_ref = doc.add(Object::Dict(pages));

        let mut catalog = Dict::new();
        catalog.set(b"Type".to_vec(), Object::name("Catalog"));
        catalog.set(b"Pages".to_vec(), Object::Ref(pages_ref));
        let cat_ref = doc.add(Object::Dict(catalog));
        doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(cat_ref));

        assert_eq!(exceeded(&doc, &objects(&doc), &HashSet::new()), Exceeded { int: true, ..Exceeded::default() });
    }
}
