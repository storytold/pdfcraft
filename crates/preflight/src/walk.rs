//! Every content stream a document draws, with the resources it draws with: the pages, the form
//! XObjects, tiling patterns, Type 3 glyphs and soft-mask groups their resources list, the
//! annotations' appearances, and content streams nothing reaches (a verifier reads those too).
//! Conversions that rewrite content, and checks that read the text it shows, walk these.

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

/// How deep forms inside forms are followed.
const MAX_DEPTH: u8 = 24;

/// One content stream, or a page's content streams, with its resources.
#[derive(Clone, Debug)]
pub(crate) struct Content {
    /// The streams that make it up, in order (a page's `/Contents` may be several).
    pub streams: Vec<ObjRef>,
    /// The resources it draws with.
    pub resources: Dict,
    /// The page it is on or drawn on, when that is known.
    pub page: Option<usize>,
}

pub(crate) fn contents(doc: &Document) -> Vec<Content> {
    let mut w = Walk { doc, out: Vec::new(), own: HashSet::new(), borrowed: Vec::new() };
    for (i, p) in pdfcraft_model::pages(doc).iter().enumerate() {
        let resources = dict_of(doc, p.dict.get(b"Resources"));
        w.borrowing(refs(doc, p.dict.get(b"Contents")), &resources, Some(i), 0);
        for r in appearances(doc, &p.dict) {
            // An appearance stream has resources of its own or none: it never takes the page's.
            w.stream(r, &Dict::new(), Some(i), 0);
        }
    }
    for n in doc.object_numbers() {
        let r = ObjRef::new(n, doc.generation(n));
        if let Object::Stream(s) = &*doc.get(r)
            && crate::is_content(&s.dict)
        {
            w.stream(r, &Dict::new(), None, 0);
        }
    }
    w.out
}

struct Walk<'a> {
    doc: &'a Document,
    out: Vec<Content>,
    /// Streams with resources of their own, already listed.
    own: HashSet<ObjRef>,
    /// Streams that take their caller's resources, with the resources already listed for them: a
    /// stream drawn with different resources is listed once for each.
    borrowed: Vec<(Vec<ObjRef>, Dict)>,
}

impl Walk<'_> {
    /// A stream drawn by content that uses `inherited`: its own resources, if it has them.
    fn stream(&mut self, r: ObjRef, inherited: &Dict, page: Option<usize>, depth: u8) {
        if depth > MAX_DEPTH {
            return;
        }
        let o = self.doc.get(r);
        let Object::Stream(s) = &*o else { return };
        match s.dict.get(b"Resources") {
            Some(res) => {
                if !self.own.insert(r) {
                    return;
                }
                let resources = dict_of(self.doc, Some(res));
                self.out.push(Content { streams: vec![r], resources: resources.clone(), page });
                self.within(&resources, page, depth.saturating_add(1));
            }
            None => self.borrowing(vec![r], inherited, page, depth),
        }
    }

    /// Streams drawn with resources that aren't their own.
    fn borrowing(&mut self, streams: Vec<ObjRef>, resources: &Dict, page: Option<usize>, depth: u8) {
        if depth > MAX_DEPTH || self.borrowed.iter().any(|(s, r)| *s == streams && r == resources) {
            return;
        }
        self.borrowed.push((streams.clone(), resources.clone()));
        self.out.push(Content { streams, resources: resources.clone(), page });
        self.within(resources, page, depth.saturating_add(1));
    }

    /// The content `resources` lists: forms, tiling patterns, Type 3 glyphs, soft-mask groups.
    fn within(&mut self, resources: &Dict, page: Option<usize>, depth: u8) {
        let doc = self.doc;
        let list = |key: &[u8]| dict_of(doc, resources.get(key));
        for (_, x) in list(b"XObject").iter() {
            if let Some(r) = x.as_ref()
                && matches!(&*doc.get(r), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Form"))
            {
                self.stream(r, resources, page, depth);
            }
        }
        for (_, p) in list(b"Pattern").iter() {
            if let Some(r) = p.as_ref()
                && matches!(&*doc.get(r), Object::Stream(s) if s.dict.int(b"PatternType") == Some(1))
            {
                self.stream(r, resources, page, depth);
            }
        }
        for (_, f) in list(b"Font").iter() {
            let font = doc.resolve(f);
            let Some(font) = font.as_dict().filter(|d| d.name(b"Subtype") == Some(b"Type3")) else { continue };
            // Glyph procedures use the font's resources, or (an older form) the page's.
            let glyph_resources = match font.get(b"Resources") {
                Some(r) => dict_of(doc, Some(r)),
                None => resources.clone(),
            };
            let procs: Vec<ObjRef> = dict_of(doc, font.get(b"CharProcs")).iter().filter_map(|(_, p)| p.as_ref()).collect();
            for r in procs {
                self.borrowing(vec![r], &glyph_resources, page, depth);
            }
        }
        for (_, g) in list(b"ExtGState").iter() {
            let state = doc.resolve(g);
            if let Some(mask) = state.as_dict().and_then(|s| s.get(b"SMask")).map(|m| doc.resolve(m))
                && let Some(group) = mask.as_dict().and_then(|m| m.get(b"G")).and_then(Object::as_ref)
            {
                self.stream(group, resources, page, depth);
            }
        }
    }
}

/// A dictionary, resolved; empty when `o` isn't one.
pub(crate) fn dict_of(doc: &Document, o: Option<&Object>) -> Dict {
    o.map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default()
}

/// The streams a `/Contents` entry names: one, or an array of them.
fn refs(doc: &Document, contents: Option<&Object>) -> Vec<ObjRef> {
    match contents {
        Some(Object::Ref(r)) => match &*doc.get(*r) {
            Object::Array(a) => a.iter().filter_map(Object::as_ref).collect(),
            _ => vec![*r],
        },
        Some(Object::Array(a)) => a.iter().filter_map(Object::as_ref).collect(),
        _ => Vec::new(),
    }
}

/// The appearance streams of a page's annotations: normal, rollover and down, each one stream
/// or one per appearance state.
pub(crate) fn appearances(doc: &Document, page: &Dict) -> Vec<ObjRef> {
    let mut out = Vec::new();
    let annots = page.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
    for a in &annots {
        let ap = dict_of(doc, doc.resolve(a).as_dict().and_then(|d| d.get(b"AP")));
        for key in [b"N".as_slice(), b"R", b"D"] {
            match ap.get(key) {
                Some(Object::Ref(r)) => match &*doc.get(*r) {
                    Object::Stream(_) => out.push(*r),
                    Object::Dict(states) => out.extend(states.iter().filter_map(|(_, s)| s.as_ref())),
                    _ => {}
                },
                Some(Object::Dict(states)) => out.extend(states.iter().filter_map(|(_, s)| s.as_ref())),
                _ => {}
            }
        }
    }
    out
}
