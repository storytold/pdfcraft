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
//! number is written wrong (`0.0.75`, `Infinity`), what follows may be the rest of the damage, so
//! it is neither judged nor followed to the content it would draw. A page part that can't be read
//! ends the page's content there, as it may leave a comment or a string open.
//!
//! The content read is what the pages draw: their contents, and through them the form XObjects
//! `Do` paints, the glyph procedures of the Type 3 fonts `Tf` selects, or the graphics states `gs`
//! sets select with `/Font` (ISO 32000-2 Table 57), the tiling patterns `scn`/`SCN` select, and the
//! soft mask groups of the graphics states `gs` sets; and the
//! appearance streams of the pages' annotations. A form XObject that no content paints isn't
//! read, as it is never drawn. Names are looked up as readers look them up: in the stream's own
//! resources, or, when it has none, in those of the content that drew it. Content with resources
//! of its own is read once; content without them once for each resource dictionary it is drawn
//! with, since what its names find, and so what it is reported for and what it draws, depends on
//! that dictionary. Content first reached so deep that what it draws lies past [`MAX_DEPTH`] is
//! read again when it is reached less deep.

use std::collections::HashMap;
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
/// How many glyph procedures the Type 3 fonts list may be passed over in all as read already with
/// the same resources. Fonts sharing one `/CharProcs` dictionary would otherwise go through it once
/// each: past this many, no more glyphs are read.
const MAX_PASSED: usize = 1 << 16;
/// The bytes of content read in all, a stream read again with other resources counted again:
/// twice [`MAX_DECODED`], so content read once always fits.
const MAX_READ: usize = 2 * MAX_DECODED;

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
    let mut walk = Walk {
        doc,
        seen: HashMap::new(),
        procs: HashMap::new(),
        decoded: HashMap::new(),
        streams: 0,
        passed: 0,
        read: 0,
        budget: DecodeBudget::new(MAX_DECODED, usize::MAX),
        found: Findings::default(),
    };
    for p in pdfcraft_model::pages(doc) {
        // The page's own dictionary, to tell its own resources from inherited ones.
        let own = doc.get(p.obj);
        let explicit = own.as_dict().is_some_and(|d| d.get(b"Resources").is_some());
        let resources = p.dict.get(b"Resources").map(|r| doc.resolve(r));
        // Which resource dictionary that is: its object, or the page's when it is written there.
        let context = Some(p.dict.get(b"Resources").and_then(Object::as_ref).unwrap_or(p.obj));
        let contents = p.dict.get(b"Contents").map(|c| doc.resolve(c));
        let parts: Vec<Object> = match contents.as_deref() {
            Some(Object::Array(a)) => a.clone(),
            Some(Object::Stream(_)) => p.dict.get(b"Contents").cloned().into_iter().collect(),
            _ => Vec::new(),
        };
        // The parts of a page's contents are one stream cut in pieces (ISO 32000-2 7.8.2).
        let mut data = Vec::new();
        for part in &parts {
            // Each part is read on every page and every time it is listed, as the page draws it.
            let Some(bytes) = walk.data(part) else {
                // A part that isn't a stream (a reference to a missing object is null, ISO 32000-2
                // 7.3.10) holds nothing. One that is but can't be read may end inside a comment or
                // a string that the next part goes on with, so what follows isn't read.
                if matches!(&*doc.resolve(part), Object::Stream(_)) {
                    break;
                }
                continue;
            };
            // A part read again isn't decoded again, so the read budget bounds what is joined.
            if walk.read.saturating_add(data.len()).saturating_add(bytes.len()) > MAX_READ {
                break;
            }
            data.extend_from_slice(&bytes);
            // At least one white-space character between parts (ISO 32000-2 Table 31, /Contents):
            // a space, as the content library joins them, so a comment a part ends in goes on
            // into the next.
            data.push(b' ');
        }
        walk.run(&data, resources.as_deref(), context, explicit, Holder::Page, 0);
        let annots = p.dict.get(b"Annots").map(|a| doc.resolve(a));
        for a in annots.as_deref().and_then(Object::as_array).into_iter().flatten() {
            let a = doc.resolve(a);
            let Some(ap) = a.as_dict().and_then(|a| a.get(b"AP")).map(|ap| doc.resolve(ap)) else { continue };
            for (_, v) in ap.as_dict().into_iter().flat_map(|d| d.iter()) {
                match &*doc.resolve(v) {
                    Object::Stream(_) => walk.form(v, None, None, 0),
                    Object::Dict(states) => {
                        for (_, s) in states.iter() {
                            walk.form(s, None, None, 0);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    walk.found
}

/// Which resource dictionary content is read with: the object that holds it (the dictionary
/// itself, or the page or stream it is written in), `None` when none is known.
type Context = Option<ObjRef>;

/// A content stream or Type 3 font as it is read: the object, the resource dictionary it is read
/// with when it has none of its own (`None` when it has), and whether it counts as having its own
/// (a glyph without them does when its font has them).
type Visit = (ObjRef, Context, bool);

struct Walk<'a> {
    doc: &'a Document,
    /// The content streams and Type 3 fonts read, each with the shallowest depth it was read at.
    seen: HashMap<Visit, usize>,
    /// The Type 3 `/CharProcs` dictionaries gone through, as [`Self::seen`]: each with the
    /// resource dictionary and `explicit` its glyphs were read with.
    procs: HashMap<Visit, usize>,
    /// The data of each content stream decoded, so one read again with other resources isn't
    /// decoded again (`None` when it can't be).
    decoded: HashMap<ObjRef, Option<Arc<[u8]>>>,
    streams: usize,
    /// The glyph procedures passed over as read already (see [`MAX_PASSED`]).
    passed: usize,
    /// The bytes of content read so far (see [`MAX_READ`]).
    read: usize,
    budget: DecodeBudget,
    found: Findings,
}

impl Walk<'_> {
    /// Whether to read `o` with the resources of `context` (`None` when it has its own), as
    /// content with resources of its own when `explicit` ([`Visit`]), at `depth`: the first time,
    /// or again when it is reached less deep than before, as content cut off below it at the
    /// deeper visit is then read. Never at the same depth or deeper, so a cycle ends.
    fn visit(&mut self, o: &Object, context: Context, explicit: bool, depth: usize) -> bool {
        let Object::Ref(r) = o else { return true };
        first_visit(&mut self.seen, (*r, context, explicit), depth)
    }

    /// The decoded data of the stream `o` refers to, read as [`Self::visit`] says: once per stream,
    /// context and `explicit`, unless it is reached less deep than before; decoded once, within
    /// the budgets.
    fn decode(&mut self, o: &Object, context: Context, explicit: bool, depth: usize) -> Option<Arc<[u8]>> {
        if !self.visit(o, context, explicit, depth) {
            return None;
        }
        self.data(o)
    }

    /// The decoded data of the stream `o` refers to, decoded once and kept, within the budgets,
    /// however often it is read.
    fn data(&mut self, o: &Object) -> Option<Arc<[u8]>> {
        if self.streams >= MAX_STREAMS {
            return None;
        }
        self.streams = self.streams.saturating_add(1);
        if let Object::Ref(r) = o
            && let Some(done) = self.decoded.get(r)
        {
            return done.clone();
        }
        let data = self.decode_new(o);
        if let Object::Ref(r) = o {
            self.decoded.insert(*r, data.clone());
        }
        data
    }

    /// The decoded data of the stream `o` refers to, when its filters are ones a stream may name
    /// and it decodes within the budget.
    fn decode_new(&mut self, o: &Object) -> Option<Arc<[u8]>> {
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
    /// `outer`, those of `context`.
    fn form(&mut self, o: &Object, outer: Option<&Object>, context: Context, depth: usize) {
        self.stream_content(o, outer, context, Holder::Form, depth);
    }

    /// Read the content stream `o` with its own resources, or `outer` (those of `context`) when it
    /// has none.
    fn stream_content(&mut self, o: &Object, outer: Option<&Object>, context: Context, holder: Holder, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let held = self.doc.resolve(o);
        let Object::Stream(s) = &*held else { return };
        if holder == Holder::Form && !is_form(self.doc, s) {
            return;
        }
        let own_entry = s.dict.get(b"Resources");
        let own = own_entry.map(|r| self.doc.resolve(r));
        let explicit = own.is_some();
        let Some(data) = self.decode(o, if explicit { None } else { context }, explicit, depth) else { return };
        let context = match own_entry {
            Some(entry) => entry.as_ref().or_else(|| o.as_ref()),
            None => context,
        };
        self.run(&data, own.as_deref().or(outer), context, explicit, holder, depth);
    }

    /// Read the operators of `data`, drawn with `resources`, those of `context`. `explicit` says
    /// whether the stream has resources of its own.
    fn run(&mut self, data: &[u8], resources: Option<&Object>, context: Context, explicit: bool, holder: Holder, depth: usize) {
        self.read = self.read.saturating_add(data.len());
        if self.read > MAX_READ {
            return;
        }
        let doc = self.doc;
        let lookup = |category: &[u8], name: &[u8]| -> Option<Object> {
            let res = resources?.as_dict()?;
            let entries = doc.resolve(res.get(category)?);
            entries.as_dict()?.get(name).cloned()
        };
        let mut unassociated = false;
        let mut ops = pdfcraft_content::ops(data);
        while let Some(op) = ops.next() {
            let kw = op.op.as_slice();
            // Once the stream turns out damaged (bytes the parser had to skip, a number written
            // wrong, a keyword that isn't printable ASCII, which is data rather than content),
            // what follows may be the rest of a damaged operand rather than an operator: the
            // stream isn't judged on it, nor is the content it would draw read from here.
            if ops.skipped() > 0 || damaged_numbers(data, &op) || !kw.iter().all(u8::is_ascii_graphic) {
                break;
            }
            if !OPERATORS.contains(&kw) && self.found.undefined.is_none() {
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
                        self.form(&x, resources, context, next);
                    }
                }
                b"Tf" => {
                    if let Some(f) = op.name_back(1).and_then(|n| lookup(b"Font", n)) {
                        self.type3(&f, resources, context, next);
                    }
                }
                b"scn" | b"SCN" => {
                    if let Some(p) = op.last_name().and_then(|n| lookup(b"Pattern", n))
                        && let Object::Stream(s) = &*doc.resolve(&p)
                        && s.dict.get(b"PatternType").and_then(|t| doc.resolve(t).as_int()) == Some(1)
                    {
                        self.stream_content(&p, resources, context, Holder::Pattern, next);
                    }
                }
                b"gs" => {
                    let gs = op.last_name().and_then(|n| lookup(b"ExtGState", n)).map(|g| doc.resolve(&g));
                    let mask = gs.as_deref().and_then(Object::as_dict).and_then(|g| g.get(b"SMask")).map(|m| doc.resolve(m));
                    if let Some(group) = mask.as_deref().and_then(Object::as_dict).and_then(|m| m.get(b"G")) {
                        self.form(group, resources, context, next);
                    }
                    // A graphics state can select a font as `Tf` does, by reference (Table 57).
                    let font = gs.as_deref().and_then(Object::as_dict).and_then(|g| g.get(b"Font")).map(|f| doc.resolve(f));
                    if let Some(f) = font.as_deref().and_then(Object::as_array).and_then(|a| a.first()) {
                        self.type3(f, resources, context, next);
                    }
                }
                _ => {}
            }
        }
    }

    /// The glyph procedures of the Type 3 font `f`, with its resources or `outer`, those of
    /// `context`.
    fn type3(&mut self, f: &Object, outer: Option<&Object>, context: Context, depth: usize) {
        let doc = self.doc;
        let font = doc.resolve(f);
        let Some(d) = font.as_dict() else { return };
        if d.get(b"Subtype").map(|s| doc.resolve(s)).as_deref().and_then(Object::as_name) != Some(b"Type3") {
            return;
        }
        let own_entry = d.get(b"Resources");
        let own = own_entry.map(|r| doc.resolve(r));
        let explicit = own.is_some();
        // A font with resources of its own is read once, wherever it is selected; one without them
        // once for each resource dictionary it is selected with.
        if !self.visit(f, if explicit { None } else { context }, explicit, depth) {
            return;
        }
        let context = match own_entry {
            Some(entry) => entry.as_ref().or_else(|| f.as_ref()),
            None => context,
        };
        let resources = own.as_deref().or(outer);
        if depth > MAX_DEPTH {
            return;
        }
        // A `/CharProcs` dictionary fonts share is gone through once for each resource dictionary
        // and `explicit` its glyphs are read with, as its glyphs are: another font with the same
        // ones would find each of them read already. Its object stands for it when it is written
        // in the font.
        let procs_entry = d.get(b"CharProcs");
        let procs_key = procs_entry.and_then(Object::as_ref).or_else(|| f.as_ref());
        if let Some(r) = procs_key
            && !first_visit(&mut self.procs, (r, context, explicit), depth)
        {
            return;
        }
        let procs = procs_entry.map(|c| doc.resolve(c));
        for (_, p) in procs.as_deref().and_then(Object::as_dict).into_iter().flat_map(|d| d.iter()) {
            // Past the stream limit nothing more is read; past the passed limit, no more glyphs.
            if self.streams >= MAX_STREAMS || self.passed >= MAX_PASSED {
                return;
            }
            // A glyph's own stream may carry the resources it uses, as readers accept.
            let glyph = doc.resolve(p);
            let glyph_entry = match &*glyph {
                Object::Stream(g) => g.dict.get(b"Resources").cloned(),
                _ => None,
            };
            let glyph_own = glyph_entry.as_ref().map(|r| doc.resolve(r));
            // A glyph without resources of its own is read once for each dictionary it is read
            // with, the font's own among them, and for whether its font has them: two fonts may
            // share it.
            let glyph_explicit = explicit || glyph_own.is_some();
            if !self.visit(p, if glyph_own.is_some() { None } else { context }, glyph_explicit, depth) {
                self.passed = self.passed.saturating_add(1);
                continue;
            }
            let Some(data) = self.data(p) else { continue };
            let context = match &glyph_entry {
                Some(entry) => entry.as_ref().or_else(|| p.as_ref()),
                None => context,
            };
            self.run(&data, glyph_own.as_deref().or(resources), context, glyph_explicit, Holder::Type3, depth);
        }
    }
}

/// Whether `key` is read at `depth` as [`Walk::visit`] says, noting it in `seen` when it is.
fn first_visit(seen: &mut HashMap<Visit, usize>, key: Visit, depth: usize) -> bool {
    match seen.get(&key) {
        Some(&before) if before <= depth => false,
        _ => {
            seen.insert(key, depth);
            true
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
