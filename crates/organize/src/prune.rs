//! Keeps only the XObjects a part's pages actually draw (Split / Extract Pages, #204).
//!
//! In a real book all pages share one `/Resources` dictionary, so every split part carried all
//! ~2,900 image XObjects while its 1,500 pages drew ~300 of them, and the part saved out at the
//! source's size. This pass follows each part's content streams (`Do`, form XObjects included)
//! and drops the `/XObject` entries no page names; the garbage-collecting rewrite on save then
//! drops the objects themselves.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{CosError, DecodeBudget, Dict, Document, ObjRef, Object};

use crate::OrganizeError;

/// Form XObjects nested deeper than this are a broken file, not a reason to hang.
const MAX_FORM_DEPTH: u8 = 8;
/// Count every resource object and direct child; a graph beyond this is left unpruned.
const MAX_GLYPH_RESOURCE_WORK: usize = 10_000;

fn resolve(doc: &Document, obj: &Object) -> Object {
    let mut cur = obj.clone();
    for _ in 0..16 {
        let Object::Ref(r) = cur else { break };
        cur = (*doc.get(r)).clone();
    }
    cur
}

fn target_ref(doc: &Document, obj: &Object) -> Option<ObjRef> {
    let mut cur = obj.as_ref()?;
    for _ in 0..16 {
        match &*doc.get(cur) {
            Object::Ref(next) => cur = *next,
            _ => return Some(cur),
        }
    }
    None
}

fn dict_name(doc: &Document, d: &Dict, key: &[u8]) -> Option<Vec<u8>> {
    d.get(key).and_then(|v| resolve(doc, v).as_name().map(<[u8]>::to_vec))
}

/// The dictionary a set of drawn names belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum XKey {
    /// An indirect `/XObject` dictionary.
    XObject(ObjRef),
    /// An inline `/XObject` dictionary inside an indirect resources object.
    Resources(ObjRef),
    /// Everything inline on the page itself.
    Page(ObjRef),
}

/// Who owns the resources currently being walked, for the key drawn names are recorded under.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Owner {
    /// An indirect resources object (a page's, or a form XObject's own).
    Resources(ObjRef),
    /// Resources inline on the page.
    Page(ObjRef),
}

struct Pruner<'a> {
    doc: &'a Document,
    used: HashMap<XKey, HashSet<Vec<u8>>>,
    /// Dictionaries whose drawn names can't be known for certain (undecodable content, Type 3
    /// glyphs drawing through them). They are left untouched: pruning must never drop an XObject
    /// something still draws.
    unsure: HashSet<XKey>,
    /// An unresolved resource owner may share names with any dictionary already analysed.
    resource_resolution_uncertain: bool,
    /// An overlarge glyph-resource graph is not safe to prune at all.
    glyph_budget_exhausted: bool,
    /// Nor is a file with forms nested past [`MAX_FORM_DEPTH`]: what the forms there draw isn't
    /// read, and they may draw it through resources shared with content that was.
    too_deep: bool,
    glyph_work_left: usize,
    glyph_seen: HashSet<ObjRef>,
    /// An indirect font's explicit resources have one owner regardless of which page uses it.
    glyph_fonts: HashSet<ObjRef>,
    /// Forms already analysed, with the owner of the resources they drew against: drawing one
    /// again records nothing new, and re-reading it on every `Do` is exponential in the nesting.
    forms_seen: HashSet<(ObjRef, Owner)>,
    /// Owners of resources that content not read whole drew against, whose every form was read
    /// as drawn ([`Pruner::unread`]): once is enough.
    partly_read: HashSet<Owner>,
}

impl Pruner<'_> {
    /// Mark the `/XObject` dictionary of `resources` as one not to prune.
    fn distrust(&mut self, resources: &Dict, owner: Owner) {
        if let Some((_, key)) = self.xobjects(resources, owner) {
            self.unsure.insert(key);
        }
    }

    fn glyph_work(&mut self, count: usize) -> bool {
        if !self.glyph_budget_exhausted
            && let Some(left) = self.glyph_work_left.checked_sub(count)
        {
            self.glyph_work_left = left;
            return true;
        }
        self.glyph_budget_exhausted = true;
        false
    }

    /// Walk one object's direct children without cloning them or recursing. Referenced objects
    /// go on a separate work list, so their borrowed dictionaries stay alive during this pass.
    fn protect_glyph_tree(&mut self, object: &Object, referenced: &mut Vec<ObjRef>) {
        if !self.glyph_work(1) {
            return;
        }
        let mut pending = vec![object];
        while let Some(object) = pending.pop() {
            match object {
                Object::Ref(r) => {
                    // A glyph's forms, patterns and soft masks can share either the XObject
                    // dictionary itself or a resource owner with content we do interpret.
                    self.unsure.insert(XKey::XObject(*r));
                    self.unsure.insert(XKey::Resources(*r));
                    self.unsure.insert(XKey::Page(*r));
                    if self.glyph_seen.insert(*r) {
                        referenced.push(*r);
                    }
                }
                Object::Array(array) => {
                    if !self.glyph_work(array.len()) {
                        return;
                    }
                    pending.extend(array);
                }
                Object::Dict(_) | Object::Stream(_) => {
                    if let Some(dict) = object.as_dict() {
                        if !self.glyph_work(dict.len()) {
                            return;
                        }
                        pending.extend(dict.iter().map(|(_, value)| value));
                    }
                }
                _ => {}
            }
        }
    }

    /// Glyph programs are not interpreted here. Keep every resource owner reachable from their
    /// resource graph, including nested forms, patterns and soft masks. Data bytes are never
    /// decoded; references are visited once and all direct children count against the budget.
    fn protect_glyph_object(&mut self, object: &Object) {
        let mut referenced = Vec::new();
        self.protect_glyph_tree(object, &mut referenced);
        while !self.glyph_budget_exhausted {
            let Some(r) = referenced.pop() else { break };
            let object = self.doc.get(r);
            self.protect_glyph_tree(&object, &mut referenced);
        }
    }

    fn protect_type3_resources(&mut self, resources: &Dict, owner: Owner) {
        if self.glyph_budget_exhausted {
            return;
        }
        let Some(fonts) = resources.get(b"Font").map(|f| resolve(self.doc, f)) else { return };
        let Some(fonts) = fonts.as_dict() else { return };
        // Bound discovery too, but don't spend the glyph-graph budget repeatedly examining an
        // ordinary shared font catalog on every page of a large document.
        if fonts.len() > MAX_GLYPH_RESOURCE_WORK {
            self.glyph_budget_exhausted = true;
            return;
        }
        for (_, font) in fonts.iter() {
            if self.glyph_budget_exhausted {
                return;
            }
            let font_ref = target_ref(self.doc, font);
            let resolved = resolve(self.doc, font);
            let Some(font) = resolved.as_dict().filter(|f| dict_name(self.doc, f, b"Subtype").as_deref() == Some(b"Type3")) else { continue };
            if let Some(glyph_resources) = font.get(b"Resources") {
                if let Some(r) = font_ref
                    && !self.glyph_fonts.insert(r)
                {
                    continue;
                }
                self.protect_glyph_object(glyph_resources);
            } else {
                self.distrust(resources, owner);
                // This root dictionary is already owned by the content caller. Only its
                // referenced descendants can share additional owners elsewhere in the file.
                for (_, value) in resources.iter() {
                    self.protect_glyph_object(value);
                    if self.glyph_budget_exhausted {
                        return;
                    }
                }
            }
        }
    }

    /// The `/XObject` dictionary of `resources`, with the key its drawn names belong to.
    fn xobjects(&mut self, resources: &Dict, owner: Owner) -> Option<(Dict, XKey)> {
        let x = resources.get(b"XObject")?;
        let resolved = resolve(self.doc, x);
        let Some(dict) = resolved.as_dict().cloned() else {
            self.resource_resolution_uncertain = true;
            return None;
        };
        let key = match target_ref(self.doc, x) {
            Some(r) => XKey::XObject(r),
            None => match owner {
                Owner::Resources(r) => XKey::Resources(r),
                Owner::Page(p) => XKey::Page(p),
            },
        };
        Some((dict, key))
    }

    /// Record every XObject name `data` draws with `Do`, descending into form XObjects.
    fn content(&mut self, data: &[u8], resources: &Dict, owner: Owner, depth: u8) {
        self.protect_type3_resources(resources, owner);
        let Some((xobjects, key)) = self.xobjects(resources, owner) else {
            return;
        };
        if depth > MAX_FORM_DEPTH {
            self.too_deep = true;
            return;
        }
        for op in pdfcraft_content::parse(data).ops {
            if op.op.as_slice() != b"Do" {
                continue;
            }
            let Some(name) = op.name(0) else { continue };
            self.used.entry(key).or_default().insert(name.to_vec());
            self.form(&xobjects, name, resources, owner, depth);
        }
    }

    /// For content that can't be read whole (damaged partway, or not decoding at all): what it
    /// draws is unknown, so its `/XObject` dictionary is kept whole, and every form there is read
    /// as drawn, which also reads every form the part before any damage draws. A form whose
    /// resources are shared keeps what it draws from them too.
    fn unread(&mut self, resources: &Dict, owner: Owner, depth: u8) {
        self.protect_type3_resources(resources, owner);
        let Some((xobjects, key)) = self.xobjects(resources, owner) else { return };
        self.unsure.insert(key);
        if depth > MAX_FORM_DEPTH {
            self.too_deep = true;
            return;
        }
        if !self.partly_read.insert(owner) {
            return;
        }
        for (name, _) in xobjects.iter() {
            self.form(&xobjects, name, resources, owner, depth);
        }
    }

    /// Record what the XObject `name` of `xobjects` draws when it is a form, drawn from content
    /// with `resources`.
    fn form(&mut self, xobjects: &Dict, name: &[u8], resources: &Dict, owner: Owner, depth: u8) {
        let Some(object) = xobjects.get(name) else { return };
        let Some(r) = target_ref(self.doc, object) else {
            // A drawn alias may hide a form that uses this shared resource dictionary.
            self.resource_resolution_uncertain = true;
            return;
        };
        let Object::Stream(s) = &*self.doc.get(r) else { return };
        let subtype = s.dict.get(b"Subtype").map(|value| resolve(self.doc, value));
        if matches!(&subtype, Some(Object::Ref(_))) {
            // The hop limit leaves the subtype unknown, not definitively non-Form.
            self.resource_resolution_uncertain = true;
            return;
        }
        if subtype.as_ref().and_then(Object::as_name) != Some(b"Form") {
            return;
        }
        let (inner, inner_owner) = match s.dict.get(b"Resources") {
            Some(res) => match target_ref(self.doc, res) {
                Some(rr) => match self.doc.get(rr).as_dict().cloned() {
                    Some(d) => (d, Owner::Resources(rr)),
                    None => {
                        self.resource_resolution_uncertain = true;
                        return;
                    }
                },
                None => match resolve(self.doc, res).as_dict().cloned() {
                    // The form carries its resources inline: filter them on the form itself.
                    Some(d) => (d, Owner::Resources(r)),
                    None => {
                        self.resource_resolution_uncertain = true;
                        return;
                    }
                },
            },
            // A form without its own resources draws against the ones it was invoked with.
            None => (resources.clone(), owner),
        };
        if !self.forms_seen.insert((r, inner_owner)) {
            return;
        }
        #[cfg(test)]
        crate::work(1);
        match DecodeBudget::new(usize::MAX, 0).decode_with_damage(self.doc, s, usize::MAX) {
            Ok((data, false)) => self.content(&data, &inner, inner_owner, depth + 1),
            Ok((_, true)) | Err(_) => self.unread(&inner, inner_owner, depth + 1),
        }
    }
}

/// The page's content, concatenated (`/Contents` may be one stream or an array) with a space
/// between pieces as the renderer joins them, decoded against one page's [`DecodeBudget`], and
/// whether a stream is damaged partway (its part is then what decoded before the damage). An
/// error when a stream can't be decoded or the budget runs out: the page may draw names that
/// can't be seen.
pub(crate) fn content_bytes(doc: &Document, contents: &Object) -> Result<(Vec<u8>, bool), CosError> {
    let mut out = Vec::new();
    let mut damaged = false;
    for read in DecodeBudget::page().decode_contents_with_damage(doc, contents) {
        let (data, partial) = read?;
        damaged |= partial;
        out.extend(data);
        out.push(b' ');
    }
    Ok((out, damaged))
}

/// The resources of a page (as `walk` reports them) with their owner.
fn resolve_resources(doc: &Document, resources: &Object, page: ObjRef) -> Option<(Dict, Owner)> {
    if let Some(r) = target_ref(doc, resources) {
        return doc.get(r).as_dict().cloned().map(|d| (d, Owner::Resources(r)));
    }
    resolve(doc, resources).as_dict().cloned().map(|d| (d, Owner::Page(page)))
}

/// Drop the entries `names` never draws. Returns how many were removed.
fn drop_unused(xobjects: &mut Dict, names: &HashSet<Vec<u8>>) -> usize {
    let unused: Vec<Vec<u8>> = xobjects.iter().map(|(k, _)| k.clone()).filter(|k| !names.contains(k)).collect();
    for k in &unused {
        xobjects.remove(k);
    }
    unused.len()
}

/// Filter the dictionary `key` names to `names`.
fn filter_at(doc: &mut Document, key: XKey, names: &HashSet<Vec<u8>>) -> Result<usize, OrganizeError> {
    match key {
        XKey::XObject(r) => {
            let mut removed = 0;
            doc.update_dict(r, |d| removed = drop_unused(d, names))?;
            Ok(removed)
        }
        XKey::Resources(r) => {
            let Some(d) = doc.get(r).as_dict().cloned() else { return Ok(0) };
            let Some(x) = d.get(b"XObject") else { return Ok(0) };
            if target_ref(doc, x).is_some() {
                return Ok(0); // recorded as XObject(r) instead
            }
            let Some(mut xd) = resolve(doc, x).as_dict().cloned() else { return Ok(0) };
            let removed = drop_unused(&mut xd, names);
            if removed > 0 {
                doc.update_dict(r, |d| d.set(b"XObject".to_vec(), Object::Dict(xd.clone())))?;
            }
            Ok(removed)
        }
        XKey::Page(p) => {
            let Some(pd) = doc.get(p).as_dict().cloned() else { return Ok(0) };
            let Some(res) = pd.get(b"Resources") else { return Ok(0) };
            if target_ref(doc, res).is_some() {
                return Ok(0); // recorded as Resources(r) instead
            }
            let Some(mut rd) = resolve(doc, res).as_dict().cloned() else { return Ok(0) };
            let Some(x) = rd.get(b"XObject") else { return Ok(0) };
            if target_ref(doc, x).is_some() {
                return Ok(0);
            }
            let Some(mut xd) = resolve(doc, x).as_dict().cloned() else { return Ok(0) };
            let removed = drop_unused(&mut xd, names);
            if removed > 0 {
                rd.set(b"XObject".to_vec(), Object::Dict(xd));
                doc.update_dict(p, |d| d.set(b"Resources".to_vec(), Object::Dict(rd.clone())))?;
            }
            Ok(removed)
        }
    }
}

/// Drop the page-resource entries the document's pages never draw. Returns how many were removed.
pub(crate) fn prune_unused_xobjects(doc: &mut Document) -> Result<usize, OrganizeError> {
    let pages = crate::walk(doc)?;
    let mut pruner = Pruner {
        doc,
        used: HashMap::new(),
        unsure: HashSet::new(),
        resource_resolution_uncertain: false,
        glyph_budget_exhausted: false,
        too_deep: false,
        glyph_work_left: MAX_GLYPH_RESOURCE_WORK,
        glyph_seen: HashSet::new(),
        glyph_fonts: HashSet::new(),
        forms_seen: HashSet::new(),
        partly_read: HashSet::new(),
    };
    for (page, attrs) in &pages {
        let Some(res) = attrs.get(b"Resources") else { continue };
        let Some((res_dict, owner)) = resolve_resources(pruner.doc, res, *page) else {
            pruner.resource_resolution_uncertain = true;
            continue;
        };
        let Some(pd) = pruner.doc.get(*page).as_dict().cloned() else { continue };
        let Some(contents) = pd.get(b"Contents").cloned() else { continue };
        match content_bytes(pruner.doc, &contents) {
            Ok((data, false)) if data.is_empty() => {}
            Ok((data, false)) => pruner.content(&data, &res_dict, owner, 0),
            Ok((_, true)) | Err(_) => pruner.unread(&res_dict, owner, 0),
        }
    }
    let Pruner { used, unsure, glyph_budget_exhausted, too_deep, resource_resolution_uncertain, .. } = pruner;
    // Missing usage behind an unresolved alias could belong to any shared owner. Keeping extra
    // entries is safer than mutating dictionaries based on an incomplete usage census.
    if glyph_budget_exhausted || too_deep || resource_resolution_uncertain {
        return Ok(0);
    }
    let mut removed = 0;
    for (key, names) in used {
        if !unsure.contains(&key) {
            removed += filter_at(doc, key, &names)?;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_cos::Stream;

    /// Two pages draw different names from one XObject dictionary, through unequal aliases.
    fn shared_alias_fixture(xobject_alias: bool, extra_hops: usize) -> (Document, ObjRef) {
        let mut doc = Document::new_empty();
        let mut image = Dict::new();
        image.set(b"Type".to_vec(), Object::name("XObject"));
        image.set(b"Subtype".to_vec(), Object::name("Image"));
        image.set(b"Width".to_vec(), Object::Int(1));
        image.set(b"Height".to_vec(), Object::Int(1));
        image.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
        image.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        let image = doc.add(Object::Stream(Stream::from_raw(image, vec![0])));
        let mut xobjects = Dict::new();
        for name in ["First", "Second", "Unused"] {
            xobjects.set(name.as_bytes().to_vec(), Object::Ref(image));
        }
        let shared = doc.add(xobjects);
        let mut resources = Dict::new();
        resources.set(b"XObject".to_vec(), Object::Ref(shared));
        let resources_ref = doc.add(resources.clone());
        let first_resources = doc.add(Object::Ref(resources_ref));
        let mut second = if xobject_alias { Object::Ref(shared) } else { Object::Ref(resources_ref) };
        for _ in 0..extra_hops {
            second = Object::Ref(doc.add(second));
        }
        if xobject_alias {
            resources.set(b"XObject".to_vec(), second);
            second = Object::Dict(resources);
        }
        let pages_ref = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
        let mut kids = Vec::new();
        for (resource, content) in [(Object::Ref(first_resources), b"/First Do".as_slice()), (second, b"/Second Do".as_slice())] {
            let content = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content.to_vec())));
            let mut page = Dict::new();
            page.set(b"Type".to_vec(), Object::name("Page"));
            page.set(b"Parent".to_vec(), Object::Ref(pages_ref));
            page.set(b"MediaBox".to_vec(), Object::Array([0, 0, 100, 100].map(Object::Int).to_vec()));
            page.set(b"Resources".to_vec(), resource);
            page.set(b"Contents".to_vec(), Object::Ref(content));
            kids.push(Object::Ref(doc.add(page)));
        }
        doc.update_dict(pages_ref, |d| {
            d.set(b"Kids".to_vec(), Object::Array(kids));
            d.set(b"Count".to_vec(), Object::Int(2));
        })
        .unwrap();
        (doc, shared)
    }

    fn first_page_xobjects(doc: &Document) -> Dict {
        let pages = crate::walk(doc).unwrap();
        let resources = resolve(doc, pages[0].1.get(b"Resources").unwrap());
        resolve(doc, resources.as_dict().unwrap().get(b"XObject").unwrap()).as_dict().unwrap().clone()
    }

    fn check_shared_chain_limit(xobject_alias: bool) {
        for extra_hops in [0, 15, 16, 100] {
            let (doc, _) = shared_alias_fixture(xobject_alias, extra_hops);
            let bytes = pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
            let reopened = Document::open(std::sync::Arc::new(bytes)).unwrap();
            for mut doc in [doc, reopened] {
                let uncertain = extra_hops >= 16;
                assert_eq!(prune_unused_xobjects(&mut doc).unwrap(), usize::from(!uncertain), "XObject={xobject_alias}, extra hops={extra_hops}");
                let bytes = pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
                let reopened = Document::open(std::sync::Arc::new(bytes)).unwrap();
                let names = first_page_xobjects(&reopened);
                assert!(names.contains(b"First"));
                assert!(names.contains(b"Second"), "a use hidden beyond the hop limit remains live");
                assert_eq!(names.contains(b"Unused"), uncertain);
            }
        }
    }

    #[test]
    fn shared_resource_chain_limit_preserves_all_entries() {
        check_shared_chain_limit(false);
    }

    #[test]
    fn shared_xobject_chain_limit_preserves_all_entries() {
        check_shared_chain_limit(true);
    }

    fn check_drawn_form_reference_limit(subtype_alias: bool) {
        for extra_hops in [15, 16] {
            let (mut doc, shared) = shared_alias_fixture(false, 0);
            let second_page = crate::walk(&doc).unwrap()[1].0;
            let mut form = Dict::new();
            let mut subtype = Object::name("Form");
            if subtype_alias {
                for _ in 0..=extra_hops {
                    subtype = Object::Ref(doc.add(subtype));
                }
            }
            form.set(b"Subtype".to_vec(), subtype);
            form.set(b"BBox".to_vec(), Object::Array([0, 0, 100, 100].map(Object::Int).to_vec()));
            let mut form = Object::Ref(doc.add(Object::Stream(Stream::from_raw(form, b"/Second Do".to_vec()))));
            if !subtype_alias {
                for _ in 0..extra_hops {
                    form = Object::Ref(doc.add(form));
                }
            }
            doc.update_dict(shared, |d| d.set(b"Fm".to_vec(), form)).unwrap();
            let contents = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"/Fm Do".to_vec())));
            doc.update_dict(second_page, |d| d.set(b"Contents".to_vec(), Object::Ref(contents))).unwrap();
            let bytes = pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
            let reopened = Document::open(std::sync::Arc::new(bytes)).unwrap();
            for mut source in [doc, reopened] {
                let extracted = crate::extract_pages(&source, &[0, 1]).unwrap();
                let bytes = pdfcraft_cos::write_full(&extracted, &pdfcraft_cos::SaveOptions::default()).unwrap();
                let saved = Document::open(std::sync::Arc::new(bytes)).unwrap();
                let names = first_page_xobjects(&saved);
                assert!(names.contains(b"First"));
                assert!(names.contains(b"Fm"));
                assert!(names.contains(b"Second"), "Extract must retain an inheriting form's use behind {extra_hops} aliases");
                assert_eq!(names.contains(b"Unused"), extra_hops == 16);
                assert_eq!(prune_unused_xobjects(&mut source).unwrap(), usize::from(extra_hops == 15));
                assert!(first_page_xobjects(&source).contains(b"Second"));
            }
        }
    }

    #[test]
    fn drawn_form_alias_chain_limit_preserves_inherited_image_use() {
        check_drawn_form_reference_limit(false);
    }

    #[test]
    fn drawn_form_subtype_chain_limit_preserves_inherited_image_use() {
        check_drawn_form_reference_limit(true);
    }

    #[test]
    fn resolved_non_form_subtype_still_allows_pruning() {
        let (mut doc, shared) = shared_alias_fixture(false, 0);
        let image = doc.get(shared).as_dict().unwrap().reference(b"First").unwrap();
        let mut subtype = Object::name("Image");
        for _ in 0..16 {
            subtype = Object::Ref(doc.add(subtype));
        }
        doc.update_dict(image, |d| d.set(b"Subtype".to_vec(), subtype)).unwrap();
        assert_eq!(prune_unused_xobjects(&mut doc).unwrap(), 1);
        assert!(!first_page_xobjects(&doc).contains(b"Unused"));
    }

    #[test]
    fn form_resource_chain_limit_preserves_shared_xobjects() {
        for xobject_alias in [false, true] {
            let (mut doc, _) = shared_alias_fixture(xobject_alias, 16);
            let page = crate::walk(&doc).unwrap()[1].0;
            let mut form = Dict::new();
            form.set(b"Subtype".to_vec(), Object::name("Form"));
            form.set(b"Resources".to_vec(), doc.get(page).as_dict().unwrap().get(b"Resources").unwrap().clone());
            let form = doc.add(Object::Stream(Stream::from_raw(form, b"/Second Do".to_vec())));
            let mut names = Dict::new();
            names.set(b"Fm".to_vec(), Object::Ref(form));
            let mut resources = Dict::new();
            resources.set(b"XObject".to_vec(), Object::Dict(names));
            let content = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"/Fm Do".to_vec())));
            doc.update_dict(page, |d| {
                d.set(b"Resources".to_vec(), Object::Dict(resources));
                d.set(b"Contents".to_vec(), Object::Ref(content));
            })
            .unwrap();
            assert_eq!(prune_unused_xobjects(&mut doc).unwrap(), 0);
            let bytes = pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
            let reopened = Document::open(std::sync::Arc::new(bytes)).unwrap();
            assert!(first_page_xobjects(&reopened).contains(b"Second"));
        }
    }

    #[test]
    fn unresolved_resource_cycles_and_values_are_conservative_but_absence_is_not() {
        for xobject_alias in [false, true] {
            for variant in 0..4 {
                let (mut doc, shared) = shared_alias_fixture(xobject_alias, 0);
                let second = crate::walk(&doc).unwrap()[1].0;
                let value = match variant {
                    0 => {
                        let cycle = doc.add(Object::Null);
                        doc.set(cycle, Object::Ref(cycle));
                        Some(Object::Ref(cycle))
                    }
                    1 => Some(Object::Ref(ObjRef::new(999_999, 0))),
                    2 => Some(Object::Int(42)),
                    _ => None,
                };
                doc.update_dict(second, |d| {
                    if xobject_alias {
                        let mut resources = Dict::new();
                        if let Some(value) = value {
                            resources.set(b"XObject".to_vec(), value);
                        }
                        d.set(b"Resources".to_vec(), Object::Dict(resources));
                    } else if let Some(value) = value {
                        d.set(b"Resources".to_vec(), value);
                    } else {
                        d.remove(b"Resources");
                    }
                })
                .unwrap();
                assert_eq!(prune_unused_xobjects(&mut doc).unwrap(), if variant == 3 { 2 } else { 0 });
                assert_eq!(doc.get(shared).as_dict().unwrap().contains(b"Second"), variant != 3);
            }
        }
    }

    #[test]
    fn extract_preserves_shared_xobjects_beyond_resource_chain_limit() {
        for xobject_alias in [false, true] {
            let (doc, _) = shared_alias_fixture(xobject_alias, 16);
            let extracted = crate::extract_pages(&doc, &[0, 1]).unwrap();
            let bytes = pdfcraft_cos::write_full(&extracted, &pdfcraft_cos::SaveOptions::default()).unwrap();
            let reopened = Document::open(std::sync::Arc::new(bytes)).unwrap();
            let pages = crate::walk(&reopened).unwrap();
            let resources = resolve(&reopened, pages[0].1.get(b"Resources").unwrap());
            let names = resolve(&reopened, resources.as_dict().unwrap().get(b"XObject").unwrap());
            let names = names.as_dict().unwrap();
            assert!(names.contains(b"First"));
            assert!(names.contains(b"Second"), "Extract must preserve a use through an unresolved alias");
            assert!(names.contains(b"Unused"), "uncertain resource usage leaves the shared dictionary intact");
        }
    }

    #[test]
    fn prune_unused_xobjects_follows_form_with_indirect_subtype_and_chained_ref() {
        let mut doc = Document::new_empty();
        let used_img = doc.add(Object::Stream(Stream::from_raw(Dict::new(), Vec::new())));
        let unused_img = doc.add(Object::Stream(Stream::from_raw(Dict::new(), Vec::new())));

        let form_subtype_ref = doc.add(Object::Name(b"Form".to_vec()));
        let mut form_dict = Dict::new();
        form_dict.set(b"Subtype".to_vec(), Object::Ref(form_subtype_ref));
        // Form has no /Resources of its own; it draws /ImUsed from the page's /Resources.
        let form_ref = doc.add(Object::Stream(Stream::from_raw(form_dict, b"/ImUsed Do".to_vec())));
        let form_hop = doc.add(Object::Ref(form_ref));

        let mut xo = Dict::new();
        xo.set(b"Fm1".to_vec(), Object::Ref(form_hop));
        xo.set(b"ImUsed".to_vec(), Object::Ref(used_img));
        xo.set(b"ImUnused".to_vec(), Object::Ref(unused_img));
        let mut res = Dict::new();
        res.set(b"XObject".to_vec(), Object::Dict(xo));

        let contents = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"/Fm1 Do".to_vec())));
        let pages_ref = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).expect("pages");
        let mut page = Dict::new();
        page.set(b"Type".to_vec(), Object::name("Page"));
        page.set(b"Parent".to_vec(), Object::Ref(pages_ref));
        page.set(b"Resources".to_vec(), Object::Dict(res));
        page.set(b"Contents".to_vec(), Object::Ref(contents));
        let page_ref = doc.add(Object::Dict(page));
        doc.update_dict(pages_ref, |d| {
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page_ref)]));
            d.set(b"Count".to_vec(), Object::Int(1));
        })
        .expect("update pages");

        let removed = prune_unused_xobjects(&mut doc).expect("prune unused xobjects");
        assert_eq!(removed, 1);
        let page_obj = doc.get(page_ref);
        let kept_xo = page_obj
            .as_dict()
            .and_then(|d| d.get(b"Resources"))
            .and_then(Object::as_dict)
            .and_then(|r| r.get(b"XObject"))
            .and_then(Object::as_dict)
            .expect("kept xo");
        assert!(kept_xo.contains(b"Fm1"));
        assert!(kept_xo.contains(b"ImUsed"), "image drawn by inheriting form with indirect /Subtype /Form must not be pruned");
        assert!(!kept_xo.contains(b"ImUnused"));
    }

    #[test]
    fn prune_unused_xobjects_distrusts_type3_font_with_indirect_subtype_and_chained_ref() {
        let mut doc = Document::new_empty();
        let img = doc.add(Object::Stream(Stream::from_raw(Dict::new(), Vec::new())));

        let type3_ref = doc.add(Object::Name(b"Type3".to_vec()));
        let mut font_dict = Dict::new();
        font_dict.set(b"Subtype".to_vec(), Object::Ref(type3_ref));
        let font_ref = doc.add(Object::Dict(font_dict));
        let font_hop = doc.add(Object::Ref(font_ref));

        let mut fonts = Dict::new();
        fonts.set(b"F1".to_vec(), Object::Ref(font_hop));
        let mut xo = Dict::new();
        xo.set(b"ImGlyph".to_vec(), Object::Ref(img));
        xo.set(b"ImPage".to_vec(), Object::Ref(img));
        let mut res = Dict::new();
        res.set(b"Font".to_vec(), Object::Dict(fonts));
        res.set(b"XObject".to_vec(), Object::Dict(xo));

        let contents = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"/ImPage Do BT /F1 12 Tf (a) Tj ET".to_vec())));
        let pages_ref = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).expect("pages");
        let mut page = Dict::new();
        page.set(b"Type".to_vec(), Object::name("Page"));
        page.set(b"Parent".to_vec(), Object::Ref(pages_ref));
        page.set(b"Resources".to_vec(), Object::Dict(res));
        page.set(b"Contents".to_vec(), Object::Ref(contents));
        let page_ref = doc.add(Object::Dict(page));
        doc.update_dict(pages_ref, |d| {
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page_ref)]));
            d.set(b"Count".to_vec(), Object::Int(1));
        })
        .expect("update pages");

        let removed = prune_unused_xobjects(&mut doc).expect("prune unused xobjects");
        assert_eq!(removed, 0, "resources shared with a resourceless Type3 font must not be pruned");
    }
}
