//! PDF Optimizer ▸ Audit space usage: how many bytes each kind of content takes.
//!
//! Every object is sized as it would be written (dictionary plus stream data) and put in the
//! first category that reaches it; what remains (the catalog, page tree, info, metadata,
//! cross-reference data and anything unreachable) is document overhead.

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

    /// A page's resources, by kind.
    fn resources(&mut self, res: &Dict) {
        let xobjects = self.resolve_dict(res.get(b"XObject"));
        for (_, x) in xobjects.iter() {
            let Some(r) = x.as_ref() else { continue };
            let is_image = matches!(&*self.doc.get(r), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image"));
            if is_image {
                self.claim(x, SpaceCategory::Images);
            } else {
                // A form: its own resources first, by kind, then the form itself.
                if let Object::Stream(s) = &*self.doc.get(r) {
                    let inner = self.resolve_dict(s.dict.get(b"Resources"));
                    if let std::collections::hash_map::Entry::Vacant(e) = self.owner.entry(r.num) {
                        e.insert(SpaceCategory::XObjectForms);
                        self.resources(&inner);
                    }
                }
                self.claim(x, SpaceCategory::XObjectForms);
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
    // Order matters: the most specific owners first.
    if let Some(s) = cat.get(b"StructTreeRoot") {
        // Structure elements point at pages and annotations; those stay out (claim skips pages,
        // and annotations are claimed first below).
        for p in &pages {
            if let Some(annots) = p.dict.get(b"Annots") {
                for x in doc.resolve(annots).as_array().cloned().unwrap_or_default() {
                    let widget = x.as_ref().and_then(|r| doc.get(r).as_dict().map(|d| d.name(b"Subtype") == Some(b"Widget"))).unwrap_or(false);
                    a.claim(&x, if widget { SpaceCategory::Forms } else { SpaceCategory::Comments });
                }
            }
        }
        a.claim(s, SpaceCategory::StructureInfo);
    }
    if let Some(f) = cat.get(b"AcroForm") {
        a.claim(f, SpaceCategory::Forms);
    }
    for p in &pages {
        if let Some(annots) = p.dict.get(b"Annots") {
            for x in doc.resolve(annots).as_array().cloned().unwrap_or_default() {
                let widget = x.as_ref().and_then(|r| doc.get(r).as_dict().map(|d| d.name(b"Subtype") == Some(b"Widget"))).unwrap_or(false);
                a.claim(&x, if widget { SpaceCategory::Forms } else { SpaceCategory::Comments });
            }
        }
        // Resources before the content: a content stream's dictionary may point at what it
        // draws (PdfCraft's /PCAdded records an added image), which stays an image or font.
        let res = a.resolve_dict(p.dict.get(b"Resources"));
        a.resources(&res);
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
