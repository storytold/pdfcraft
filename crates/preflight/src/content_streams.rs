//! PDF/A-2 and -3 6.2.2 (PDF/A-4 6.2.2): content streams.
//!
//! ISO 19005-2/-3 6.2.2 and ISO 19005-4 6.2.2 ask two things of the content a page draws:
//!
//! - it uses no operator that ISO 32000 doesn't define (ISO 32000-2 Annex A, Table A.1), even
//!   between `BX` and `EX`, which otherwise let a reader skip operators it doesn't know (8.2);
//! - a content stream that refers to resources by name has a resource dictionary explicitly
//!   associated with it (7.8.3): a page's own `/Resources`, not one inherited from the page tree;
//!   a form XObject's, a tiling pattern's or a Type 3 font's own `/Resources` (or a glyph
//!   stream's own).
//!
//! A stream is reported for the second only when a name it uses is found in resources it took
//! from elsewhere: a name no dictionary defines is a missing resource, not a missing dictionary.
//! An operator is judged only while the stream reads cleanly: once the parser skips bytes, or a
//! number is written wrong (`0.0.75`, `Infinity`), what follows may be the rest of the damage.
//!
//! The content read is what the pages draw: their contents, and through them the form XObjects
//! `Do` paints, the glyph procedures of the Type 3 fonts `Tf` selects, or the graphics states `gs`
//! sets select with `/Font` (ISO 32000-2 Table 57), the tiling patterns `scn`/`SCN` select, and the
//! soft mask groups of the graphics states `gs` sets; and the
//! appearance streams of the pages' annotations. A form XObject that no content paints isn't
//! read, as it is never drawn. Names are looked up as readers look them up: in the stream's own
//! resources, or, when it has none, in those of the content that drew it.

use std::collections::HashSet;
use std::sync::Arc;

use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object, Stream};

/// The operators of ISO 32000-2 Table A.1. `ID` and `EI` are read as part of `BI`.
const OPERATORS: &[&[u8]] = &[
    b"b", b"B", b"b*", b"B*", b"BDC", b"BI", b"BMC", b"BT", b"BX", b"c", b"cm", b"CS", b"cs", b"d", b"d0", b"d1", b"Do", b"DP", b"EI", b"EMC", b"ET",
    b"EX", b"f", b"F", b"f*", b"G", b"g", b"gs", b"h", b"i", b"ID", b"j", b"J", b"K", b"k", b"l", b"m", b"M", b"MP", b"n", b"q", b"Q", b"re", b"RG",
    b"rg", b"ri", b"s", b"S", b"SC", b"sc", b"SCN", b"scn", b"sh", b"T*", b"Tc", b"Td", b"TD", b"Tf", b"Tj", b"TJ", b"TL", b"Tm", b"Tr", b"Ts",
    b"Tw", b"Tz", b"v", b"w", b"W", b"W*", b"y", b"'", b"\"",
];
/// The stream filters of ISO 32000-2 Table 6.
const STREAM_FILTERS: &[&[u8]] = &[
    b"ASCIIHexDecode",
    b"ASCII85Decode",
    b"LZWDecode",
    b"FlateDecode",
    b"RunLengthDecode",
    b"CCITTFaxDecode",
    b"JBIG2Decode",
    b"DCTDecode",
    b"JPXDecode",
    b"Crypt",
];
/// The decoded bytes the content of one document may take in all. Past it, no more is read.
const MAX_DECODED: usize = 1 << 28;
/// How deep content draws other content (a form in a form, ...).
const MAX_DEPTH: usize = 32;
/// How many content streams are read in all.
const MAX_STREAMS: usize = 1 << 16;

/// What kind of content stream refers to resources without its own resource dictionary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Holder {
    Page,
    Form,
    Pattern,
    Type3,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Findings {
    /// The first operator that ISO 32000 doesn't define, its other bytes `#`-escaped.
    pub(crate) undefined: Option<String>,
    /// Each kind of content that refers to resources by name without its own dictionary.
    pub(crate) unassociated: Vec<Holder>,
}

pub(crate) fn check(doc: &Document) -> Findings {
    let mut walk = Walk { doc, seen: HashSet::new(), streams: 0, budget: DecodeBudget::new(MAX_DECODED, usize::MAX), found: Findings::default() };
    for p in pdfcraft_model::pages(doc) {
        // The page's own dictionary, to tell its own resources from inherited ones.
        let own = doc.get(p.obj);
        let explicit = own.as_dict().is_some_and(|d| d.get(b"Resources").is_some());
        let resources = p.dict.get(b"Resources").map(|r| doc.resolve(r));
        let contents = p.dict.get(b"Contents").map(|c| doc.resolve(c));
        let parts: Vec<Object> = match contents.as_deref() {
            Some(Object::Array(a)) => a.clone(),
            Some(Object::Stream(_)) => p.dict.get(b"Contents").cloned().into_iter().collect(),
            _ => Vec::new(),
        };
        // The parts of a page's contents are one stream cut in pieces (ISO 32000-2 7.8.2).
        let mut data = Vec::new();
        for part in &parts {
            let Some(bytes) = walk.decode(part) else { continue };
            data.extend_from_slice(&bytes);
            data.push(b'\n');
        }
        walk.run(&data, resources.as_deref(), explicit, Holder::Page, 0);
        let annots = p.dict.get(b"Annots").map(|a| doc.resolve(a));
        for a in annots.as_deref().and_then(Object::as_array).into_iter().flatten() {
            let a = doc.resolve(a);
            let Some(ap) = a.as_dict().and_then(|a| a.get(b"AP")).map(|ap| doc.resolve(ap)) else { continue };
            for (_, v) in ap.as_dict().into_iter().flat_map(|d| d.iter()) {
                match &*doc.resolve(v) {
                    Object::Stream(_) => walk.form(v, None, 0),
                    Object::Dict(states) => {
                        for (_, s) in states.iter() {
                            walk.form(s, None, 0);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    walk.found
}

struct Walk<'a> {
    doc: &'a Document,
    /// The content streams read, each once.
    seen: HashSet<ObjRef>,
    streams: usize,
    budget: DecodeBudget,
    found: Findings,
}

impl Walk<'_> {
    /// The decoded data of the stream `o` refers to, once per stream and within the budgets.
    fn decode(&mut self, o: &Object) -> Option<Arc<[u8]>> {
        if let Object::Ref(r) = o
            && !self.seen.insert(*r)
        {
            return None;
        }
        if self.streams >= MAX_STREAMS {
            return None;
        }
        self.streams = self.streams.saturating_add(1);
        let held = self.doc.resolve(o);
        let Object::Stream(s) = &*held else { return None };
        // A filter a stream can't name (the abbreviations are for inline images only, ISO 32000-2
        // 8.9.7) leaves its data undefined, so it isn't read as content.
        let filters = s.dict.get(b"Filter").map(|f| self.doc.resolve(f));
        let named = |f: &Object| f.as_name().is_some_and(|n| STREAM_FILTERS.contains(&n));
        let defined = match filters.as_deref() {
            None => true,
            Some(Object::Array(a)) => a.iter().all(|f| named(&self.doc.resolve(f))),
            Some(f) => named(f),
        };
        if !defined {
            return None;
        }
        self.budget.decode(s).ok().map(|d| Arc::from(&d[..]))
    }

    /// Read a form XObject (or an appearance stream) `o`, drawn from content whose resources are
    /// `outer`.
    fn form(&mut self, o: &Object, outer: Option<&Object>, depth: usize) {
        self.stream_content(o, outer, Holder::Form, depth);
    }

    /// Read the content stream `o` with its own resources, or `outer` when it has none.
    fn stream_content(&mut self, o: &Object, outer: Option<&Object>, holder: Holder, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let held = self.doc.resolve(o);
        let Object::Stream(s) = &*held else { return };
        if holder == Holder::Form && !is_form(self.doc, s) {
            return;
        }
        let own = s.dict.get(b"Resources").map(|r| self.doc.resolve(r));
        let Some(data) = self.decode(o) else { return };
        let explicit = own.is_some();
        let resources = own.or_else(|| outer.map(|r| Arc::new(r.clone())));
        self.run(&data, resources.as_deref(), explicit, holder, depth);
    }

    /// Read the operators of `data`, drawn with `resources`. `explicit` says whether the stream
    /// has resources of its own.
    fn run(&mut self, data: &[u8], resources: Option<&Object>, explicit: bool, holder: Holder, depth: usize) {
        let doc = self.doc;
        let lookup = |category: &[u8], name: &[u8]| -> Option<Object> {
            let res = resources?.as_dict()?;
            let entries = doc.resolve(res.get(category)?);
            entries.as_dict()?.get(name).cloned()
        };
        let mut unassociated = false;
        // Once the stream turns out damaged (bytes the parser had to skip, a number written
        // wrong), what follows may be the rest of a damaged operand rather than an operator: the
        // stream isn't judged on it.
        let mut damaged = false;
        let mut ops = pdfcraft_content::ops(data);
        while let Some(op) = ops.next() {
            let kw = op.op.as_slice();
            // An operator is written in printable ASCII; other bytes are data, not content.
            damaged = damaged || ops.skipped() > 0 || damaged_numbers(data, &op) || !kw.iter().all(u8::is_ascii_graphic);
            if !OPERATORS.contains(&kw) && self.found.undefined.is_none() && !damaged {
                self.found.undefined = Some(escaped(kw));
            }
            // A name the stream's own resources would have to define, found only in those of the
            // page tree or of the content that drew it.
            if !explicit && !unassociated && named_resource(&op).is_some_and(|(category, name)| lookup(category, name).is_some()) {
                unassociated = true;
                if !self.found.unassociated.contains(&holder) {
                    self.found.unassociated.push(holder);
                }
            }
            let next = depth.saturating_add(1);
            match kw {
                b"Do" => {
                    if let Some(x) = op.last_name().and_then(|n| lookup(b"XObject", n)) {
                        self.form(&x, resources, next);
                    }
                }
                b"Tf" => {
                    if let Some(f) = op.name_back(1).and_then(|n| lookup(b"Font", n)) {
                        self.type3(&f, resources, next);
                    }
                }
                b"scn" | b"SCN" => {
                    if let Some(p) = op.last_name().and_then(|n| lookup(b"Pattern", n))
                        && let Object::Stream(s) = &*doc.resolve(&p)
                        && s.dict.get(b"PatternType").and_then(|t| doc.resolve(t).as_int()) == Some(1)
                    {
                        self.stream_content(&p, resources, Holder::Pattern, next);
                    }
                }
                b"gs" => {
                    let gs = op.last_name().and_then(|n| lookup(b"ExtGState", n)).map(|g| doc.resolve(&g));
                    let mask = gs.as_deref().and_then(Object::as_dict).and_then(|g| g.get(b"SMask")).map(|m| doc.resolve(m));
                    if let Some(group) = mask.as_deref().and_then(Object::as_dict).and_then(|m| m.get(b"G")) {
                        self.form(group, resources, next);
                    }
                    // A graphics state can select a font as `Tf` does, by reference (Table 57).
                    let font = gs.as_deref().and_then(Object::as_dict).and_then(|g| g.get(b"Font")).map(|f| doc.resolve(f));
                    if let Some(f) = font.as_deref().and_then(Object::as_array).and_then(|a| a.first()) {
                        self.type3(f, resources, next);
                    }
                }
                _ => {}
            }
        }
    }

    /// The glyph procedures of the Type 3 font `f`, with its resources or `outer`.
    fn type3(&mut self, f: &Object, outer: Option<&Object>, depth: usize) {
        let doc = self.doc;
        let font = doc.resolve(f);
        let Some(d) = font.as_dict() else { return };
        if d.get(b"Subtype").map(|s| doc.resolve(s)).as_deref().and_then(Object::as_name) != Some(b"Type3") {
            return;
        }
        // A font is read once, wherever it is selected.
        if let Object::Ref(r) = f
            && !self.seen.insert(*r)
        {
            return;
        }
        let own = d.get(b"Resources").map(|r| doc.resolve(r));
        let explicit = own.is_some();
        let resources = own.or_else(|| outer.map(|r| Arc::new(r.clone())));
        let procs = d.get(b"CharProcs").map(|c| doc.resolve(c));
        for (_, p) in procs.as_deref().and_then(Object::as_dict).into_iter().flat_map(|d| d.iter()) {
            if depth > MAX_DEPTH {
                return;
            }
            // A glyph's own stream may carry the resources it uses, as readers accept.
            let glyph = doc.resolve(p);
            let glyph_own = match &*glyph {
                Object::Stream(g) => g.dict.get(b"Resources").map(|r| doc.resolve(r)),
                _ => None,
            };
            let Some(data) = self.decode(p) else { continue };
            let explicit = explicit || glyph_own.is_some();
            self.run(&data, glyph_own.as_deref().or(resources.as_deref()), explicit, Holder::Type3, depth);
        }
    }
}

/// Whether `s` is a form XObject: `/Subtype /Form`, or no `/Subtype` at all, as appearance streams
/// may leave it out. An image painted with `Do` holds no content.
fn is_form(doc: &Document, s: &Stream) -> bool {
    match s.dict.get(b"Subtype").map(|t| doc.resolve(t)) {
        Some(t) => t.as_name() == Some(b"Form"),
        None => true,
    }
}

/// The resource `op` refers to by name (ISO 32000-2 Table 34), as its category and name: an
/// XObject, a font, a graphics state, a colour space other than a device space or `Pattern`, a
/// pattern, a shading or a property list.
fn named_resource(op: &pdfcraft_content::Op) -> Option<(&'static [u8], &[u8])> {
    match op.op.as_slice() {
        b"Do" => Some((b"XObject", op.last_name()?)),
        b"gs" => Some((b"ExtGState", op.last_name()?)),
        b"sh" => Some((b"Shading", op.last_name()?)),
        b"Tf" => Some((b"Font", op.name_back(1)?)),
        b"cs" | b"CS" => op.last_name().filter(|n| !is_device_space(n) && *n != b"Pattern").map(|n| (b"ColorSpace".as_slice(), n)),
        b"scn" | b"SCN" => Some((b"Pattern", op.last_name()?)),
        b"BDC" | b"DP" if op.operands.len() >= 2 => Some((b"Properties", op.last_name()?)),
        b"BI" => op.inline.as_ref().and_then(|(d, _)| inline_space_name(d)).map(|n| (b"ColorSpace".as_slice(), n)),
        _ => None,
    }
}

fn is_device_space(n: &[u8]) -> bool {
    matches!(n, b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK")
}

/// The colour space resource an inline image names: a name other than a device space or its
/// abbreviation (ISO 32000-2 Table 91), or the base of an Indexed space when it is such a name.
fn inline_space_name(d: &Dict) -> Option<&[u8]> {
    let named = |n: &&[u8]| !is_device_space(n) && !matches!(*n, b"G" | b"RGB" | b"CMYK" | b"Indexed" | b"I");
    match d.get(b"CS").or_else(|| d.get(b"ColorSpace"))? {
        Object::Name(n) => Some(n.as_slice()).filter(named),
        Object::Array(a) => a.get(1).and_then(Object::as_name).filter(named),
        _ => None,
    }
}

/// Whether the operands of `op` were written as ISO 32000-2 7.3.3 writes numbers. A number such as
/// `0.0.75` or a word such as `Infinity` where a number goes is damage, not an operand or operator
/// to judge: strings, hexadecimal strings, names and comments are stepped over.
fn damaged_numbers(data: &[u8], op: &pdfcraft_content::Op) -> bool {
    if op.inline.is_some() {
        return false;
    }
    let Some(span) = data.get(op.span.clone()) else { return false };
    let mut i = 0;
    while let Some(&b) = span.get(i) {
        match b {
            b'(' => {
                // A literal string, with its escapes and balanced parentheses.
                let mut depth = 0usize;
                while let Some(&c) = span.get(i) {
                    match c {
                        b'\\' => i = i.saturating_add(1),
                        b'(' => depth = depth.saturating_add(1),
                        b')' => {
                            depth = depth.saturating_sub(1);
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    i = i.saturating_add(1);
                }
                i = i.saturating_add(1);
            }
            b'<' if span.get(i.saturating_add(1)) != Some(&b'<') => {
                i = i.saturating_add(span.get(i..).and_then(|r| r.iter().position(|&c| c == b'>')).unwrap_or(span.len()));
                i = i.saturating_add(1);
            }
            b'%' => {
                i = i.saturating_add(span.get(i..).and_then(|r| r.iter().position(|&c| matches!(c, b'\r' | b'\n'))).unwrap_or(span.len()));
            }
            b'/' => {
                i = i.saturating_add(1);
                i = i.saturating_add(span.get(i..).map_or(0, |r| r.iter().take_while(|&&c| is_regular(c)).count()));
            }
            _ if is_regular(b) => {
                let len = span.get(i..).map_or(0, |r| r.iter().take_while(|&&c| is_regular(c)).count()).max(1);
                let token = span.get(i..i.saturating_add(len)).unwrap_or_default();
                let numeric = matches!(b, b'0'..=b'9' | b'+' | b'-' | b'.');
                if (numeric && !is_number(token)) || matches!(token, b"Infinity" | b"-Infinity" | b"NaN") {
                    return true;
                }
                i = i.saturating_add(len);
            }
            _ => i = i.saturating_add(1),
        }
    }
    false
}

fn is_regular(b: u8) -> bool {
    !pdfcraft_cos::is_whitespace(b) && !matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

/// A number as ISO 32000-2 7.3.3 writes it: a sign, digits, and at most one period among them.
fn is_number(t: &[u8]) -> bool {
    let digits = t.strip_prefix(b"+").or_else(|| t.strip_prefix(b"-")).unwrap_or(t);
    digits.iter().any(u8::is_ascii_digit)
        && digits.iter().all(|c| c.is_ascii_digit() || *c == b'.')
        && digits.iter().filter(|&&c| c == b'.').count() <= 1
}

/// An operator as written: printable ASCII as it is, any other byte as `#XX`, cut to 32 bytes.
fn escaped(kw: &[u8]) -> String {
    let mut out = String::new();
    for &b in kw.iter().take(32) {
        if b.is_ascii_graphic() && b != b'#' {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("#{b:02X}"));
        }
    }
    out
}
