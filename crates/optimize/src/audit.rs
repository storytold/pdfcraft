//! PDF Optimizer ▸ Audit space usage: how many bytes each kind of content takes.
//!
//! Every object is sized as it would be written (dictionary plus stream data). Resources go by
//! kind first, wherever they are used: a font or image a form field or comment draws with is
//! still a font or image. Every other object goes to the first category that reaches it; what
//! remains (the catalog, page tree, info, metadata, cross-reference data and anything
//! unreachable) is document overhead.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

fn resolve(doc: &Document, obj: &Object) -> Object {
    let mut cur = obj.clone();
    for _ in 0..16 {
        match cur {
            Object::Ref(r) => cur = (*doc.get(r)).clone(),
            _ => return cur,
        }
    }
    cur
}

fn dict_name(doc: &Document, d: &Dict, key: &[u8]) -> Option<Vec<u8>> {
    d.get(key).and_then(|v| resolve(doc, v).as_name().map(<[u8]>::to_vec))
}

fn target_ref(doc: &Document, obj: &Object) -> Option<ObjRef> {
    let mut cur = obj.clone();
    let mut last = None;
    for _ in 0..16 {
        match cur {
            Object::Ref(r) => {
                last = Some(r);
                cur = (*doc.get(r)).clone();
            }
            _ => break,
        }
    }
    last
}

/// The categories, in Acrobat's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SpaceCategory {
    Images,
    ContentStreams,
    Fonts,
    XObjectForms,
    StructureInfo,
    Bookmarks,
    Thumbnails,
    Comments,
    Forms,
    NamedDestinations,
    ColorSpaces,
    Shadings,
    Patterns,
    ExtendedGraphicsStates,
    EmbeddedFiles,
    DocumentOverhead,
}

impl SpaceCategory {
    pub const ALL: [SpaceCategory; 16] = [
        SpaceCategory::Images,
        SpaceCategory::ContentStreams,
        SpaceCategory::Fonts,
        SpaceCategory::XObjectForms,
        SpaceCategory::StructureInfo,
        SpaceCategory::Bookmarks,
        SpaceCategory::Thumbnails,
        SpaceCategory::Comments,
        SpaceCategory::Forms,
        SpaceCategory::NamedDestinations,
        SpaceCategory::ColorSpaces,
        SpaceCategory::Shadings,
        SpaceCategory::Patterns,
        SpaceCategory::ExtendedGraphicsStates,
        SpaceCategory::EmbeddedFiles,
        SpaceCategory::DocumentOverhead,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SpaceCategory::Images => "Images",
            SpaceCategory::ContentStreams => "Content Streams",
            SpaceCategory::Fonts => "Fonts",
            SpaceCategory::XObjectForms => "X Object Forms",
            SpaceCategory::StructureInfo => "Structure Info",
            SpaceCategory::Bookmarks => "Bookmarks",
            SpaceCategory::Thumbnails => "Thumbnails",
            SpaceCategory::Comments => "Comments",
            SpaceCategory::Forms => "Forms",
            SpaceCategory::NamedDestinations => "Named Destinations",
            SpaceCategory::ColorSpaces => "Color Spaces",
            SpaceCategory::Shadings => "Shadings",
            SpaceCategory::Patterns => "Patterns",
            SpaceCategory::ExtendedGraphicsStates => "Extended Graphics States",
            SpaceCategory::EmbeddedFiles => "Embedded Files",
            SpaceCategory::DocumentOverhead => "Document Overhead",
        }
    }
}

/// One row of the audit.
#[derive(Clone, Debug, PartialEq)]
pub struct SpaceUse {
    pub category: SpaceCategory,
    pub bytes: u64,
    /// Share of the file (0–100).
    pub percent: f64,
}

/// How deep form XObjects nested in each other's resources are sorted by kind. Deeper forms
/// (and what they use) are still counted, as X Object Forms; the limit keeps a crafted chain
/// of nested forms from overflowing the stack.
const MAX_FORM_DEPTH: usize = 64;

struct Audit<'a> {
    doc: &'a Document,
    owner: HashMap<u32, SpaceCategory>,
}

impl Audit<'_> {
    /// Claim `o` and everything it references for `cat` (objects already claimed keep theirs).
    fn claim(&mut self, o: &Object, cat: SpaceCategory) {
        let mut stack = vec![o.clone()];
        let mut seen = HashSet::new();
        while let Some(o) = stack.pop() {
            match o {
                Object::Ref(r) => {
                    if !seen.insert(r.num) || self.owner.contains_key(&r.num) {
                        continue;
                    }
                    // Pages belong to the page tree, not to whatever links to them.
                    let obj = self.doc.get(r);
                    if obj.as_dict().is_some_and(|d| matches!(dict_name(self.doc, d, b"Type").as_deref(), Some(b"Page" | b"Pages"))) {
                        continue;
                    }
                    self.owner.insert(r.num, cat);
                    stack.push((*obj).clone());
                }
                Object::Array(a) => stack.extend(a),
                Object::Dict(d) => stack.extend(d.iter().filter(|(k, _)| k.as_slice() != b"Parent" && k.as_slice() != b"P").map(|(_, v)| v.clone())),
                Object::Stream(s) => stack.extend(s.dict.iter().filter(|(k, _)| k.as_slice() != b"Parent").map(|(_, v)| v.clone())),
                _ => {}
            }
        }
    }

    fn resolve_dict(&self, o: Option<&Object>) -> Dict {
        o.and_then(|x| resolve(self.doc, x).as_dict().cloned()).unwrap_or_default()
    }

    /// A resource dictionary, by kind; `depth` counts the form XObjects it is nested in.
    fn resources(&mut self, res: &Dict, depth: usize) {
        let xobjects = self.resolve_dict(res.get(b"XObject"));
        for (_, x) in xobjects.iter() {
            let Some(r) = target_ref(self.doc, x) else { continue };
            match &*self.doc.get(r) {
                Object::Stream(s) if dict_name(self.doc, &s.dict, b"Subtype").as_deref() == Some(b"Image") => {
                    self.claim(x, SpaceCategory::Images);
                }
                Object::Stream(s) if depth < MAX_FORM_DEPTH && !self.owner.contains_key(&r.num) => {
                    // A form: its own resources first, by kind, then the rest of it. Owning it
                    // first stops a form that draws itself.
                    self.owner.insert(r.num, SpaceCategory::XObjectForms);
                    let inner = self.resolve_dict(s.dict.get(b"Resources"));
                    self.resources(&inner, depth + 1);
                    self.claim(&Object::Dict(s.dict.clone()), SpaceCategory::XObjectForms);
                    self.claim(x, SpaceCategory::XObjectForms);
                }
                _ => self.claim(x, SpaceCategory::XObjectForms),
            }
        }
        for (key, cat) in [
            (&b"Font"[..], SpaceCategory::Fonts),
            (b"ColorSpace", SpaceCategory::ColorSpaces),
            (b"Shading", SpaceCategory::Shadings),
            (b"Pattern", SpaceCategory::Patterns),
            (b"ExtGState", SpaceCategory::ExtendedGraphicsStates),
        ] {
            if let Some(v) = res.get(key) {
                self.claim(v, cat);
            }
        }
    }

    /// The resources an annotation's appearances draw with, by kind. Each of /N, /R and /D is
    /// one appearance stream or a dictionary of them, one per state.
    fn appearance_resources(&mut self, annot: &Object) {
        let ap = self.resolve_dict(self.resolve_dict(Some(annot)).get(b"AP"));
        for (_, v) in ap.iter() {
            let streams = match resolve(self.doc, v) {
                Object::Stream(s) => vec![s.dict.clone()],
                Object::Dict(states) => states
                    .iter()
                    .filter_map(|(_, s)| match resolve(self.doc, s) {
                        Object::Stream(s) => Some(s.dict.clone()),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            for dict in streams {
                let res = self.resolve_dict(dict.get(b"Resources"));
                self.resources(&res, 1);
            }
        }
    }
}

/// A page's annotations.
fn annotations(doc: &Document, page: &Dict) -> Vec<Object> {
    page.get(b"Annots").and_then(|a| resolve(doc, a).as_array().cloned()).unwrap_or_default()
}

/// Form fields' widgets belong to the form; every other annotation is a comment.
fn annotation_category(doc: &Document, annot: &Object) -> SpaceCategory {
    let widget = resolve(doc, annot).as_dict().is_some_and(|d| dict_name(doc, d, b"Subtype").as_deref() == Some(b"Widget"));
    if widget { SpaceCategory::Forms } else { SpaceCategory::Comments }
}

fn size(doc: &Document, num: u32) -> u64 {
    let obj = doc.get(ObjRef::new(num, doc.generation(num)));
    let mut out = Vec::new();
    pdfcraft_cos::serialize(&obj, &mut out);
    // "n g obj\n … \nendobj\n" and its cross-reference entry.
    out.len() as u64 + 20 + 20
}

/// Audit the space a document uses; `file_len` is the size of the file on disk (the rest is
/// overhead).
pub fn audit_space(doc: &Document, file_len: u64) -> Vec<SpaceUse> {
    let mut a = Audit { doc, owner: HashMap::new() };
    let cat = doc.root().and_then(|r| doc.get(r).as_dict().cloned()).unwrap_or_default();
    let pages = pdfcraft_model::pages(doc);
    // Order matters: the most specific owners first. Resources by kind before anything that
    // uses them: a content stream's dictionary may point at what it draws (PdfCraft's /PCAdded
    // records an added image), and form fields and comments draw with fonts and images too.
    for p in &pages {
        let res = a.resolve_dict(p.dict.get(b"Resources"));
        a.resources(&res, 0);
    }
    let form = a.resolve_dict(cat.get(b"AcroForm"));
    let form_resources = a.resolve_dict(form.get(b"DR"));
    a.resources(&form_resources, 0);
    for p in &pages {
        for x in annotations(doc, &p.dict) {
            a.appearance_resources(&x);
        }
    }
    if let Some(s) = cat.get(b"StructTreeRoot") {
        // Structure elements point at pages and annotations; those stay out (claim skips pages,
        // and annotations are claimed first).
        for p in &pages {
            for x in annotations(doc, &p.dict) {
                a.claim(&x, annotation_category(doc, &x));
            }
        }
        a.claim(s, SpaceCategory::StructureInfo);
    }
    if let Some(f) = cat.get(b"AcroForm") {
        a.claim(f, SpaceCategory::Forms);
    }
    for p in &pages {
        for x in annotations(doc, &p.dict) {
            a.claim(&x, annotation_category(doc, &x));
        }
        if let Some(c) = p.dict.get(b"Contents") {
            a.claim(c, SpaceCategory::ContentStreams);
        }
        if let Some(t) = p.dict.get(b"Thumb") {
            a.claim(t, SpaceCategory::Thumbnails);
        }
    }
    if let Some(o) = cat.get(b"Outlines") {
        a.claim(o, SpaceCategory::Bookmarks);
    }
    let names = a.resolve_dict(cat.get(b"Names"));
    if let Some(d) = names.get(b"Dests").or_else(|| cat.get(b"Dests")) {
        a.claim(d, SpaceCategory::NamedDestinations);
    }
    if let Some(e) = names.get(b"EmbeddedFiles") {
        a.claim(e, SpaceCategory::EmbeddedFiles);
    }
    let mut bytes: HashMap<SpaceCategory, u64> = HashMap::new();
    for num in doc.object_numbers() {
        if let Some(c) = a.owner.get(&num) {
            *bytes.entry(*c).or_default() += size(doc, num);
        }
    }
    let counted: u64 = bytes.values().sum();
    let total = file_len.max(counted).max(1);
    bytes.insert(SpaceCategory::DocumentOverhead, total - counted);
    SpaceCategory::ALL
        .into_iter()
        .map(|c| {
            let b = bytes.get(&c).copied().unwrap_or(0);
            SpaceUse { category: c, bytes: b, percent: b as f64 * 100.0 / total as f64 }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_cos::Stream;

    #[test]
    fn audit_space_handles_indirect_subtypes_and_chained_xobject_refs() {
        let mut doc = Document::new_empty();
        let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();

        let image_sub = doc.add(Object::name("Image"));
        let mut img_d = Dict::new();
        img_d.set(b"Subtype".to_vec(), Object::Ref(image_sub));
        let img = doc.add(Object::Stream(Stream::from_raw(img_d, vec![0; 4096])));
        let img_chain = doc.add(Object::Ref(img));

        let mut xo = Dict::new();
        xo.set(b"Im0".to_vec(), Object::Ref(img_chain));
        let mut res = Dict::new();
        res.set(b"XObject".to_vec(), Object::Dict(xo));

        let widget_sub = doc.add(Object::name("Widget"));
        let mut widget = Dict::new();
        widget.set(b"Subtype".to_vec(), Object::Ref(widget_sub));
        widget.set(b"T".to_vec(), Object::String(pdfcraft_cos::PdfString::literal(vec![b'x'; 1024])));
        let widget_ref = doc.add(Object::Dict(widget));

        let page_ty = doc.add(Object::name("Page"));
        let mut p = Dict::new();
        p.set(b"Type".to_vec(), Object::Ref(page_ty));
        p.set(b"Parent".to_vec(), Object::Ref(pages));
        p.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]));
        p.set(b"Resources".to_vec(), Object::Dict(res));
        p.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(widget_ref)]));
        let page = doc.add(Object::Dict(p));
        doc.update_dict(pages, |d| {
            d.set(b"Count".to_vec(), Object::Int(1));
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        })
        .unwrap();

        let report = audit_space(&doc, 10_000);
        let get = |cat: SpaceCategory| report.iter().find(|r| r.category == cat).unwrap().bytes;
        assert!(get(SpaceCategory::Images) >= 4096, "indirect /Subtype /Image behind chained ref is an Image: {report:?}");
        assert_eq!(get(SpaceCategory::XObjectForms), 0, "not misclassified as form XObject: {report:?}");
        assert!(get(SpaceCategory::Forms) >= 1024, "widget with indirect /Subtype /Widget is counted as Forms: {report:?}");
        assert_eq!(get(SpaceCategory::Comments), 0, "widget is not misclassified as Comments: {report:?}");
    }

    #[test]
    fn audit_space_terminates_on_cyclic_xobject_refs_and_skips_indirect_type_pages() {
        let mut doc = Document::new_empty();
        let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();

        let cyc = doc.add(Object::Null);
        doc.set(cyc, Object::Ref(cyc));
        let mut xo = Dict::new();
        xo.set(b"Cyc".to_vec(), Object::Ref(cyc));
        let mut res = Dict::new();
        res.set(b"XObject".to_vec(), Object::Dict(xo));

        let page_ty = doc.add(Object::name("Page"));
        let mut p = Dict::new();
        p.set(b"Type".to_vec(), Object::Ref(page_ty));
        p.set(b"Parent".to_vec(), Object::Ref(pages));
        p.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]));
        p.set(b"Resources".to_vec(), Object::Dict(res));
        let page = doc.add(Object::Dict(p));

        // Link annotation pointing to `page` (which has an indirect /Type /Page): must not claim `page` as Comments.
        let mut link = Dict::new();
        link.set(b"Subtype".to_vec(), Object::name("Link"));
        link.set(b"Dest".to_vec(), Object::Array(vec![Object::Ref(page), Object::name("Fit")]));
        let link_ref = doc.add(Object::Dict(link));
        doc.update_dict(page, |d| d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(link_ref)]))).unwrap();
        doc.update_dict(pages, |d| {
            d.set(b"Count".to_vec(), Object::Int(1));
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        })
        .unwrap();

        let report = audit_space(&doc, 5_000);
        let get = |cat: SpaceCategory| report.iter().find(|r| r.category == cat).unwrap().bytes;
        assert!(get(SpaceCategory::Comments) < 200, "page with indirect /Type /Page was not claimed by link: {report:?}");
    }
}
