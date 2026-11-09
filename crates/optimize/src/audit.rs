//! PDF Optimizer ▸ Audit space usage: how many bytes each kind of content takes.
//!
//! Every standalone object is sized as it would be written (dictionary plus stream data), while
//! objects packed in compressed object streams share their object stream's compressed bytes in
//! proportion to their uncompressed size. Resources go by kind first, wherever they are used: a
//! font or image a form field or comment draws with is still a font or image. Every other object
//! goes to the first category that reaches it; what remains (the catalog, page tree, info,
//! metadata, cross-reference data and anything unreachable) is document overhead.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, MAX_DECODED, Object, XrefEntry};

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

fn serialized_len(doc: &Document, num: u32) -> Option<u64> {
    let obj = doc.try_get(num).ok()?;
    let mut out = Vec::new();
    pdfcraft_cos::serialize(&obj, &mut out);
    Some(out.len() as u64)
}

fn size(doc: &Document, num: u32) -> u64 {
    // "n g obj\n … \nendobj\n" and its cross-reference entry.
    serialized_len(doc, num).map_or(0, |len| len + 20 + 20)
}

/// The unedited object stream `num` is packed in, when `num` itself has no unsaved edit.
fn object_stream(doc: &Document, num: u32) -> Option<u32> {
    if doc.is_edited(num) {
        return None;
    }
    match doc.xref_entry(num) {
        Some(XrefEntry::InStream { stream, .. }) if !doc.is_edited(stream) => Some(stream),
        _ => None,
    }
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
    let mut streams: HashMap<u32, ([u64; SpaceCategory::ALL.len()], u64, usize)> = HashMap::new();
    for &num in a.owner.keys() {
        if let Some(stream) = object_stream(doc, num)
            && matches!(doc.try_get(stream).as_deref(), Ok(Object::Stream(_)))
        {
            streams.entry(stream).or_insert(([0; SpaceCategory::ALL.len()], 0, 0));
        }
    }
    for num in doc.object_numbers() {
        if let Some(stream) = object_stream(doc, num)
            && let Some((by_cat, total_u, count)) = streams.get_mut(&stream)
        {
            if let Some(len) = serialized_len(doc, num) {
                let u = len + 1;
                *total_u += u;
                *count += 1;
                if let Some(&c) = a.owner.get(&num) {
                    by_cat[c as usize] += u;
                }
            }
        } else if !streams.contains_key(&num)
            && let Some(&c) = a.owner.get(&num)
        {
            *bytes.entry(c).or_default() += size(doc, num);
        }
    }
    for (stream, (by_cat, mut total_u, count)) in streams {
        let Ok(stm_obj) = doc.try_get(stream) else { continue };
        let Object::Stream(stm) = &*stm_obj else { continue };
        let declared_n = usize::try_from(stm.dict.int(b"N").unwrap_or(0).max(0)).unwrap_or(0);
        if count < declared_n
            && let Ok(decoded) = stm.decoded_within(MAX_DECODED)
        {
            let first = usize::try_from(stm.dict.int(b"First").unwrap_or(0).max(0)).unwrap_or(0);
            total_u = total_u.max(decoded.len().saturating_sub(first) as u64);
        }
        if total_u == 0 {
            continue;
        }
        let raw_len = u128::from(stm.raw.len() as u64);
        let total_u = u128::from(total_u);
        let mut acc: u128 = 0;
        for c in SpaceCategory::ALL {
            let cat_u = u128::from(by_cat[c as usize]);
            if cat_u == 0 {
                continue;
            }
            let prev = (acc * raw_len) / total_u;
            acc += cat_u;
            let next = (acc * raw_len) / total_u;
            *bytes.entry(c).or_default() += (next - prev) as u64;
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
    use std::sync::Arc;

    use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, SaveOptions, Stream, write_full};

    use super::*;

    fn one_page_doc() -> (Document, ObjRef) {
        let mut doc = Document::new_empty();
        let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
        let content = doc.add(Object::Stream(Stream::flate(Dict::new(), b"BT ET")));
        let mut page = Dict::new();
        page.set(b"Type".to_vec(), Object::name("Page"));
        page.set(b"Parent".to_vec(), Object::Ref(pages));
        page.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]));
        page.set(b"Contents".to_vec(), Object::Ref(content));
        let page = doc.add(Object::Dict(page));
        doc.update_dict(pages, |d| {
            d.set(b"Count".to_vec(), Object::Int(1));
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        })
        .unwrap();
        (doc, page)
    }

    #[test]
    fn space_audit_sizes_objects_packed_in_compressed_object_streams_as_compressed() {
        let (mut doc, page) = one_page_doc();
        let outlines = doc.add(Object::Null);
        let mut items = Vec::new();
        for i in 0..80 {
            let mut item = Dict::new();
            item.set(b"Title".to_vec(), Object::String(PdfString::literal(format!("Chapter {i}: Section heading with repeated text").into_bytes())));
            item.set(b"Parent".to_vec(), Object::Ref(outlines));
            item.set(b"Dest".to_vec(), Object::Array(vec![Object::Ref(page), Object::name("Fit")]));
            items.push(doc.add(Object::Dict(item)));
        }
        let mut root_outlines = Dict::new();
        root_outlines.set(b"First".to_vec(), Object::Ref(items[0]));
        root_outlines.set(b"Last".to_vec(), Object::Ref(*items.last().unwrap()));
        root_outlines.set(b"Count".to_vec(), Object::Int(items.len() as i64));
        doc.set(outlines, Object::Dict(root_outlines));
        for pair in items.windows(2) {
            doc.update_dict(pair[0], |d| d.set(b"Next".to_vec(), Object::Ref(pair[1]))).unwrap();
            doc.update_dict(pair[1], |d| d.set(b"Prev".to_vec(), Object::Ref(pair[0]))).unwrap();
        }
        let root = doc.root().unwrap();
        doc.update_dict(root, |c| c.set(b"Outlines".to_vec(), Object::Ref(outlines))).unwrap();

        let unpacked = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
        let packed = write_full(&doc, &SaveOptions::default()).unwrap();
        assert!(packed.len() * 3 < unpacked.len(), "{} vs {}", packed.len(), unpacked.len());

        let unpacked_doc = Document::open(Arc::new(unpacked.clone())).unwrap();
        let unpacked_bookmarks =
            audit_space(&unpacked_doc, unpacked.len() as u64).into_iter().find(|u| u.category == SpaceCategory::Bookmarks).unwrap().bytes;

        let reopened = Document::open(Arc::new(packed.clone())).unwrap();
        let audit = audit_space(&reopened, packed.len() as u64);
        let get = |c: SpaceCategory| audit.iter().find(|u| u.category == c).unwrap().clone();
        let bookmarks = get(SpaceCategory::Bookmarks);
        let overhead = get(SpaceCategory::DocumentOverhead);

        assert!(bookmarks.bytes > 0 && bookmarks.bytes < packed.len() as u64, "{audit:?}");
        assert!(bookmarks.bytes * 3 < unpacked_bookmarks, "{} vs {}", bookmarks.bytes, unpacked_bookmarks);
        assert!(overhead.bytes > 0, "packed bookmarks must not swallow the file's overhead: {audit:?}");
        assert_eq!(audit.iter().map(|u| u.bytes).sum::<u64>(), packed.len() as u64);
    }

    #[test]
    fn space_audit_shares_an_object_stream_across_categories_and_survives_freed_or_edited_members() {
        let (mut doc, page) = one_page_doc();
        // A small bookmark and a large structure tree packed into the same object stream.
        let mut item = Dict::new();
        item.set(b"Title".to_vec(), Object::String(PdfString::literal(b"Intro".to_vec())));
        item.set(b"Dest".to_vec(), Object::Array(vec![Object::Ref(page), Object::name("Fit")]));
        let item = doc.add(Object::Dict(item));
        let mut outlines = Dict::new();
        outlines.set(b"First".to_vec(), Object::Ref(item));
        outlines.set(b"Last".to_vec(), Object::Ref(item));
        outlines.set(b"Count".to_vec(), Object::Int(1));
        let outlines = doc.add(Object::Dict(outlines));
        doc.update_dict(item, |d| d.set(b"Parent".to_vec(), Object::Ref(outlines))).unwrap();

        let mut kids = Vec::new();
        for i in 0..40 {
            let mut elem = Dict::new();
            elem.set(b"Type".to_vec(), Object::name("StructElem"));
            elem.set(b"S".to_vec(), Object::name("P"));
            elem.set(b"Alt".to_vec(), Object::String(PdfString::literal(format!("Paragraph {i} with descriptive alternate text").into_bytes())));
            kids.push(Object::Ref(doc.add(Object::Dict(elem))));
        }
        let mut struct_root = Dict::new();
        struct_root.set(b"Type".to_vec(), Object::name("StructTreeRoot"));
        struct_root.set(b"K".to_vec(), Object::Array(kids.clone()));
        let struct_root = doc.add(Object::Dict(struct_root));

        let root = doc.root().unwrap();
        doc.update_dict(root, |c| {
            c.set(b"Outlines".to_vec(), Object::Ref(outlines));
            c.set(b"StructTreeRoot".to_vec(), Object::Ref(struct_root));
        })
        .unwrap();

        let packed = write_full(&doc, &SaveOptions::default()).unwrap();
        let reopened = Document::open(Arc::new(packed.clone())).unwrap();
        let before = audit_space(&reopened, packed.len() as u64);
        let bytes_of = |rows: &[SpaceUse], c: SpaceCategory| rows.iter().find(|u| u.category == c).unwrap().bytes;
        let bookmarks_before = bytes_of(&before, SpaceCategory::Bookmarks);
        let struct_before = bytes_of(&before, SpaceCategory::StructureInfo);
        assert!(bookmarks_before > 0 && struct_before > bookmarks_before * 5, "{before:?}");

        // Freeing the structure elements in memory leaves their dead bytes in the object stream on
        // disk as overhead; the surviving bookmark keeps its own small share.
        let mut freed_struct = reopened.clone();
        let cat_ref = freed_struct.root().unwrap();
        let sr = freed_struct.get(cat_ref).as_dict().unwrap().reference(b"StructTreeRoot").unwrap();
        let elem_refs: Vec<ObjRef> =
            freed_struct.get(sr).as_dict().unwrap().get(b"K").unwrap().as_array().unwrap().iter().filter_map(Object::as_ref).collect();
        freed_struct
            .update_dict(cat_ref, |c| {
                c.remove(b"StructTreeRoot");
            })
            .unwrap();
        freed_struct.free(sr);
        for r in elem_refs {
            freed_struct.free(r);
        }
        let after_free = audit_space(&freed_struct, packed.len() as u64);
        assert_eq!(bytes_of(&after_free, SpaceCategory::StructureInfo), 0);
        assert_eq!(bytes_of(&after_free, SpaceCategory::Bookmarks), bookmarks_before, "bookmark share stays unchanged after freeing other members");

        // Editing the bookmark in memory moves it out of the object stream and sizes its new value.
        let mut edited = reopened;
        let outlines_ref = edited.get(cat_ref).as_dict().unwrap().reference(b"Outlines").unwrap();
        let first_ref = edited.get(outlines_ref).as_dict().unwrap().reference(b"First").unwrap();
        edited
            .update_dict(first_ref, |d| {
                d.set(b"Title".to_vec(), Object::String(PdfString::literal(vec![b'X'; 400])));
            })
            .unwrap();
        let after_edit = audit_space(&edited, packed.len() as u64);
        assert!(bytes_of(&after_edit, SpaceCategory::Bookmarks) > bookmarks_before + 400);
        assert_eq!(bytes_of(&after_edit, SpaceCategory::StructureInfo), struct_before);

        // A malformed reference pointing directly at the object stream itself must not count the
        // stream a second time alongside its members.
        let Some(XrefEntry::InStream { stream: stm_num, .. }) = edited.xref_entry(outlines_ref.num) else {
            panic!("expected outlines in an object stream");
        };
        let mut self_ref = edited.clone();
        self_ref
            .update_dict(outlines_ref, |d| {
                d.set(b"Last".to_vec(), Object::Ref(ObjRef::new(stm_num, 0)));
            })
            .unwrap();
        let after_self_ref = audit_space(&self_ref, packed.len() as u64);
        assert_eq!(bytes_of(&after_self_ref, SpaceCategory::StructureInfo), struct_before);

        // Editing the object stream itself detaches its members from compressed stream sizing.
        edited
            .update_dict(ObjRef::new(stm_num, 0), |d| {
                d.set(b"First".to_vec(), Object::Int(999_999));
            })
            .unwrap();
        let after_stream_edit = audit_space(&edited, 0);
        assert!(bytes_of(&after_stream_edit, SpaceCategory::StructureInfo) > struct_before);
    }

    #[test]
    fn space_audit_skips_corrupted_object_streams() {
        let (mut doc, page) = one_page_doc();
        // Fill the first object stream (100 objects) with structure elements so the bookmark item
        // lands in the second object stream while the catalog stays readable in the first.
        let mut kids = Vec::new();
        for i in 0..105 {
            let mut elem = Dict::new();
            elem.set(b"Type".to_vec(), Object::name("StructElem"));
            elem.set(b"S".to_vec(), Object::name("P"));
            elem.set(b"Alt".to_vec(), Object::String(PdfString::literal(format!("Item {i}").into_bytes())));
            kids.push(Object::Ref(doc.add(Object::Dict(elem))));
        }
        let mut struct_root = Dict::new();
        struct_root.set(b"Type".to_vec(), Object::name("StructTreeRoot"));
        struct_root.set(b"K".to_vec(), Object::Array(kids));
        let struct_root = doc.add(Object::Dict(struct_root));

        let mut item = Dict::new();
        item.set(b"Title".to_vec(), Object::String(PdfString::literal(b"Broken".to_vec())));
        item.set(b"Dest".to_vec(), Object::Array(vec![Object::Ref(page), Object::name("Fit")]));
        let item = doc.add(Object::Dict(item));
        let mut outlines = Dict::new();
        outlines.set(b"First".to_vec(), Object::Ref(item));
        outlines.set(b"Last".to_vec(), Object::Ref(item));
        let outlines = doc.add(Object::Dict(outlines));
        let root = doc.root().unwrap();
        doc.update_dict(root, |c| {
            c.set(b"StructTreeRoot".to_vec(), Object::Ref(struct_root));
            c.set(b"Outlines".to_vec(), Object::Ref(outlines));
        })
        .unwrap();

        let mut packed = write_full(&doc, &SaveOptions::default()).unwrap();
        // Corrupt the last object stream's Flate payload while keeping the catalog's stream intact.
        let marker = b"/Type /ObjStm";
        let pos = packed.windows(marker.len()).rposition(|w| w == marker).unwrap();
        let stream_kw = packed[pos..].windows(8).position(|w| w == b"\nstream\n").unwrap() + pos + 8;
        for b in &mut packed[stream_kw..stream_kw + 8] {
            *b ^= 0xFF;
        }
        let corrupted = Document::open(Arc::new(packed.clone())).unwrap();
        let audit = audit_space(&corrupted, packed.len() as u64);
        assert_eq!(audit.iter().map(|u| u.bytes).sum::<u64>(), packed.len() as u64);
    }
}
