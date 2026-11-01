//! PDF Optimizer ▸ Audit space usage: how many bytes each kind of content takes.
//!
//! Every object is sized as it would be written (dictionary plus stream data). Resources go by
//! kind first, wherever they are used: a font or image a form field or comment draws with is
//! still a font or image. Every other object goes to the first category that reaches it; what
//! remains (the catalog, page tree, info, metadata, cross-reference data and anything
//! unreachable) is document overhead.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

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
                    if obj.as_dict().is_some_and(|d| matches!(d.name(b"Type"), Some(b"Page" | b"Pages"))) {
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
        o.map(|x| self.doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default()
    }

    /// A resource dictionary, by kind; `depth` counts the form XObjects it is nested in.
    fn resources(&mut self, res: &Dict, depth: usize) {
        let xobjects = self.resolve_dict(res.get(b"XObject"));
        for (_, x) in xobjects.iter() {
            let Some(r) = x.as_ref() else { continue };
            match &*self.doc.get(r) {
                Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image") => self.claim(x, SpaceCategory::Images),
                Object::Stream(s) if depth < MAX_FORM_DEPTH && !self.owner.contains_key(&r.num) => {
                    // A form: its own resources first, by kind, then the rest of it. Owning it
                    // first stops a form that draws itself.
                    self.owner.insert(r.num, SpaceCategory::XObjectForms);
                    let inner = self.resolve_dict(s.dict.get(b"Resources"));
                    self.resources(&inner, depth + 1);
                    self.claim(&Object::Dict(s.dict.clone()), SpaceCategory::XObjectForms);
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
            let streams = match &*self.doc.resolve(v) {
                Object::Stream(s) => vec![s.dict.clone()],
                Object::Dict(states) => states
                    .iter()
                    .filter_map(|(_, s)| match &*self.doc.resolve(s) {
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
    page.get(b"Annots").and_then(|a| doc.resolve(a).as_array().cloned()).unwrap_or_default()
}

/// Form fields' widgets belong to the form; every other annotation is a comment.
fn annotation_category(doc: &Document, annot: &Object) -> SpaceCategory {
    let widget = annot.as_ref().is_some_and(|r| doc.get(r).as_dict().is_some_and(|d| d.name(b"Subtype") == Some(b"Widget")));
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
