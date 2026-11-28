//! Whether a document asks for overprint (ISO 32000-2 §8.6.7, §11.7.4).
//!
//! Overprint is asked for when content that is drawn invokes, with `gs`, a graphics state whose
//! `/OP` or `/op` is true (§8.4.5, Table 57), or paints with a shading pattern whose own graphics
//! state has one (§8.7.4.3). The content walked is each page's, the form XObjects, tiling
//! patterns, soft-mask groups and Type 3 glyph procedures it draws, and every appearance stream of
//! the page's annotations, in any state and whatever their flags (§12.5.5), so an appearance that
//! is never shown can still be reported. A graphics state that is listed in the resources but never
//! invoked asks for nothing. The walk is bounded; where it stops short the answer is "not proven",
//! never "no".

use std::collections::HashSet;
use std::sync::Arc;

use pdfcraft_content::Pieces;
use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object};
use pdfcraft_model::Page;

/// How far one walk goes before it stops and says the answer isn't proven.
#[derive(Clone, Copy, Debug)]
pub(super) struct Limits {
    /// Operators read, plus annotations and glyph procedures looked at, across the document.
    pub(super) operators: u64,
    /// Streams entered across the document (page content excluded).
    pub(super) streams: usize,
    /// Decoded content bytes across the document.
    pub(super) bytes: usize,
    /// How deep drawings nest: a form in a form, a pattern in a glyph procedure, ...
    pub(super) depth: usize,
}

impl Limits {
    pub(super) const DOCUMENT: Limits = Limits { operators: 20_000_000, streams: 20_000, bytes: 512 << 20, depth: 28 };
}

/// `q` nesting tracked for the font in force; deeper content is not read on.
const MAX_SAVES: usize = 256;
/// Reasons kept for a walk that didn't reach everything.
const MAX_REASONS: usize = 8;

/// Whether the drawn content of a document asks for overprint.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overprint {
    /// The first page, in page order (1-based), whose drawing turns overprint on.
    pub page: Option<u32>,
    /// The stream on that page the request is in (a form XObject, pattern, soft-mask group,
    /// glyph procedure or appearance stream), as `n g R`. `None` for the page's own content.
    pub object: Option<String>,
    /// What the walk could not read. Empty when it read everything it was looking for.
    pub unwalked: Vec<String>,
}

impl Overprint {
    /// Something drawn turns overprint on.
    pub fn asks(&self) -> bool {
        self.page.is_some()
    }

    /// The answer is known: a request was found, or nothing was left unread.
    pub fn proven(&self) -> bool {
        self.asks() || self.unwalked.is_empty()
    }
}

/// [`overprint_use_within`] at [`Limits::DOCUMENT`].
pub(super) fn overprint_use(doc: &Document, pages: &[Page]) -> Overprint {
    overprint_use_within(doc, pages, Limits::DOCUMENT)
}

/// Walks `pages` in order and stops at the first request for overprint, or at `limits`.
pub(super) fn overprint_use_within(doc: &Document, pages: &[Page], limits: Limits) -> Overprint {
    let mut walk = Walk {
        doc,
        limits,
        budget: DecodeBudget::new(limits.bytes, limits.streams),
        page: 0,
        path: Vec::new(),
        seen: HashSet::new(),
        type3_seen: HashSet::new(),
        operators: 0,
        stopped: false,
        found: None,
        unwalked: Vec::new(),
    };
    for (index, page) in pages.iter().enumerate() {
        walk.page = u32::try_from(index).unwrap_or(u32::MAX).saturating_add(1);
        walk.page_content(page);
        walk.appearances(&page.dict);
        if walk.done() {
            break;
        }
    }
    let (page, object) = match walk.found {
        Some((page, object)) => (Some(page), object.map(|r| r.to_string())),
        None => (None, None),
    };
    Overprint { page, object, unwalked: walk.unwalked }
}

struct Walk<'a> {
    doc: &'a Document,
    limits: Limits,
    budget: DecodeBudget,
    /// The page being walked, 1-based.
    page: u32,
    /// Streams on the current drawing path (the cycle guard; the last is where an operator is).
    path: Vec<ObjRef>,
    /// `(stream, object its resources come from)` pairs already walked.
    seen: HashSet<(ObjRef, ObjRef)>,
    /// `(font, object its fallback resources come from)` pairs whose glyph procedures were walked.
    type3_seen: HashSet<(ObjRef, ObjRef)>,
    operators: u64,
    /// A document-wide limit was reached: nothing more is read.
    stopped: bool,
    found: Option<(u32, Option<ObjRef>)>,
    unwalked: Vec<String>,
}

impl Walk<'_> {
    fn done(&self) -> bool {
        self.stopped || self.found.is_some()
    }

    fn short(&mut self, why: String) {
        if self.unwalked.len() < MAX_REASONS && !self.unwalked.contains(&why) {
            self.unwalked.push(why);
        }
    }

    fn stop(&mut self, why: String) {
        self.stopped = true;
        self.short(why);
    }

    /// Counts one unit of work; `false` once the walk is over.
    fn tick(&mut self) -> bool {
        if self.done() {
            return false;
        }
        self.operators += 1;
        if self.operators > self.limits.operators {
            self.stop(format!("the walk stopped after {} operators, on page {}", self.limits.operators, self.page));
            return false;
        }
        true
    }

    fn note(&mut self) {
        if self.found.is_none() {
            self.found = Some((self.page, self.path.last().copied()));
        }
    }

    /// Whether a decode that failed did so because the byte allowance ran out, which ends the walk.
    fn out_of_bytes(&mut self) -> bool {
        if self.budget.refused() > 0 || self.budget.remaining() == 0 {
            self.stop(format!("the walk stopped after {} MB of decoded content, on page {}", self.limits.bytes >> 20, self.page));
            return true;
        }
        false
    }

    fn page_content(&mut self, page: &Page) {
        let Some(contents) = page.dict.get(b"Contents") else { return };
        let mut pieces = Vec::new();
        for read in self.budget.decode_contents_with_damage(self.doc, contents) {
            match read {
                Ok((data, damaged)) => {
                    if damaged {
                        self.short(format!("a content stream of page {} is damaged partway", self.page));
                    }
                    pieces.push(data);
                }
                Err(_) if self.out_of_bytes() => return,
                Err(_) => self.short(format!("a content stream of page {} can't be decoded", self.page)),
            }
        }
        let resources = resolved_dict(self.doc, page.dict.get(b"Resources"));
        self.content(Pieces::join(&pieces).data(), &resources, page.obj);
    }

    /// Every appearance stream of the page's annotations: normal, rollover and down, each a
    /// stream or a dictionary of streams by state (§12.5.5).
    fn appearances(&mut self, page: &Dict) {
        let Some(annots) = page.get(b"Annots") else { return };
        let annots = self.doc.resolve(annots);
        for annot in annots.as_array().into_iter().flatten() {
            if !self.tick() {
                return;
            }
            let annot = self.doc.resolve(annot);
            let Some(ap) = annot.as_dict().and_then(|a| a.get(b"AP")).map(|ap| self.doc.resolve(ap)) else { continue };
            let mut streams = Vec::new();
            for (_, v) in ap.as_dict().into_iter().flat_map(Dict::iter) {
                match &*self.doc.resolve(v) {
                    Object::Stream(_) => streams.extend(v.as_ref()),
                    Object::Dict(states) => streams.extend(states.iter().filter_map(|(_, s)| s.as_ref())),
                    _ => {}
                }
            }
            for r in streams {
                if !self.tick() {
                    return;
                }
                // An appearance stream draws with its own resources only.
                let own = own_resources(self.doc, &resolved_dict(self.doc, Some(&Object::Ref(r))));
                self.drawing(r, own, &Dict::default(), r);
            }
        }
    }

    /// Reads the operators of `data` against `resources`, which come from `holder`.
    fn content(&mut self, data: &[u8], resources: &Dict, holder: ObjRef) {
        let mut reader = pdfcraft_content::ops(data);
        let mut font: Option<Font> = None;
        let mut saved: Vec<Option<Font>> = Vec::new();
        for op in reader.by_ref() {
            if !self.tick() {
                return;
            }
            match op.op.as_slice() {
                b"q" if saved.len() >= MAX_SAVES => {
                    self.short(format!("graphics states on page {} nest deeper than {MAX_SAVES}", self.page));
                    return;
                }
                b"q" => saved.push(font.clone()),
                b"Q" => font = saved.pop().unwrap_or(font),
                b"Tf" => font = op.name(0).and_then(|n| entry(self.doc, resources, b"Font", n)),
                b"Tj" | b"TJ" | b"'" | b"\"" => {
                    if let Some(f) = &font {
                        self.type3(f, resources, holder);
                    }
                }
                b"Do" => {
                    if let Some(n) = op.name(0) {
                        self.xobject(n, resources, holder);
                    }
                }
                b"scn" | b"SCN" => {
                    if let Some(n) = op.last_name() {
                        self.pattern(n, resources, holder);
                    }
                }
                b"gs" => {
                    if let Some(n) = op.name(0) {
                        self.ext_gstate(n, resources, holder);
                    }
                }
                _ => {}
            }
        }
        if !self.done() && reader.ambiguous_images() > 0 {
            self.short(format!("an inline image on page {} leaves the operators after it uncertain", self.page));
        }
    }

    /// The stream `r`, against its own resources `own` or, lacking them, `fallback` from `holder`.
    fn drawing(&mut self, r: ObjRef, own: Option<Dict>, fallback: &Dict, holder: ObjRef) {
        let holder = if own.is_some() { r } else { holder };
        if self.done() || self.path.contains(&r) || !self.seen.insert((r, holder)) {
            return;
        }
        if self.path.len() >= self.limits.depth {
            self.short(format!("drawing on page {} nests deeper than {} at {r}", self.page, self.limits.depth));
            return;
        }
        if !self.budget.enter_form() {
            self.stop(format!("the walk stopped after {} streams, on page {}", self.limits.streams, self.page));
            return;
        }
        let object = self.doc.get(r);
        let Object::Stream(stream) = &*object else { return };
        let data = match self.budget.decode_with_damage(self.doc, stream, usize::MAX) {
            Ok((data, damaged)) => {
                if damaged {
                    self.short(format!("{r} on page {} is damaged partway", self.page));
                }
                data
            }
            Err(_) => {
                if !self.out_of_bytes() {
                    self.short(format!("{r} on page {} can't be decoded", self.page));
                }
                return;
            }
        };
        let resources = own.unwrap_or_else(|| fallback.clone());
        self.path.push(r);
        self.content(&data, &resources, holder);
        self.path.pop();
    }

    fn xobject(&mut self, name: &[u8], resources: &Dict, holder: ObjRef) {
        let Some((Some(r), x)) = entry(self.doc, resources, b"XObject", name) else { return };
        let Some(d) = x.as_dict() else { return };
        if d.name(b"Subtype") == Some(b"Form") {
            let own = own_resources(self.doc, d);
            self.drawing(r, own, resources, holder);
        }
    }

    fn pattern(&mut self, name: &[u8], resources: &Dict, holder: ObjRef) {
        let Some((r, p)) = entry(self.doc, resources, b"Pattern", name) else { return };
        match &*p {
            Object::Stream(tiling) => {
                if let Some(r) = r {
                    let own = own_resources(self.doc, &tiling.dict);
                    self.drawing(r, own, resources, holder);
                }
            }
            // A shading pattern's own graphics state (§8.7.4.1).
            Object::Dict(shading) => {
                let gs = resolved_dict(self.doc, shading.get(b"ExtGState"));
                if asks_overprint(self.doc, &gs) {
                    self.note();
                }
            }
            _ => {}
        }
    }

    /// A `gs`: its overprint flags, and the group its soft mask draws.
    fn ext_gstate(&mut self, name: &[u8], resources: &Dict, holder: ObjRef) {
        let Some((_, gs)) = entry(self.doc, resources, b"ExtGState", name) else { return };
        let Some(gs) = gs.as_dict() else { return };
        if asks_overprint(self.doc, gs) {
            self.note();
            return;
        }
        let mask = resolved_dict(self.doc, gs.get(b"SMask"));
        let Some(group) = mask.get(b"G").and_then(Object::as_ref) else { return };
        let own = own_resources(self.doc, &resolved_dict(self.doc, Some(&Object::Ref(group))));
        self.drawing(group, own, resources, holder);
    }

    /// Every glyph procedure of `font` when it is a Type 3 font, against the font's resources or,
    /// lacking them, the resources it is shown with (§9.6.5).
    fn type3(&mut self, font: &Font, resources: &Dict, holder: ObjRef) {
        let (r, font) = font;
        let Some(dict) = font.as_dict() else { return };
        if dict.name(b"Subtype") != Some(b"Type3") {
            return;
        }
        if let Some(r) = r
            && !self.type3_seen.insert((*r, holder))
        {
            return;
        }
        let own = own_resources(self.doc, dict);
        let procs = resolved_dict(self.doc, dict.get(b"CharProcs"));
        for (_, glyph) in procs.iter() {
            if !self.tick() {
                return;
            }
            if let Some(g) = glyph.as_ref() {
                self.drawing(g, own.clone(), resources, holder);
            }
        }
    }
}

/// A font resource: the reference it was named by, when indirect, and the font dictionary.
type Font = (Option<ObjRef>, Arc<Object>);

/// `/OP` or `/op` true. `/OP` alone sets both, so either asks for overprint on some painting
/// (§8.6.7, Table 57).
fn asks_overprint(doc: &Document, gs: &Dict) -> bool {
    [b"OP".as_slice(), b"op"].iter().any(|k| gs.get(k).is_some_and(|v| matches!(*doc.resolve(v), Object::Bool(true))))
}

/// The resource `name` in the `category` subdictionary of `resources`.
fn entry(doc: &Document, resources: &Dict, category: &[u8], name: &[u8]) -> Option<(Option<ObjRef>, Arc<Object>)> {
    let held = doc.resolve(resources.get(category)?);
    let o = held.as_dict()?.get(name)?;
    Some((o.as_ref(), doc.resolve(o)))
}

/// A stream's or font's own `/Resources`, when it has a dictionary there.
fn own_resources(doc: &Document, d: &Dict) -> Option<Dict> {
    let r = doc.resolve(d.get(b"Resources")?);
    r.as_dict().cloned()
}

/// The dictionary `o` is or names; empty when it is neither.
fn resolved_dict(doc: &Document, o: Option<&Object>) -> Dict {
    o.and_then(|o| doc.resolve(o).as_dict().cloned()).unwrap_or_default()
}

#[cfg(test)]
#[path = "overprint_tests.rs"]
mod tests;
