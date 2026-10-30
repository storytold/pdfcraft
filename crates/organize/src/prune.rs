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
const MAX_PAINT_RESOURCE_WORK: usize = 10_000;

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
    /// Dictionaries whose drawn names can't be known for certain (undecodable content, nesting
    /// past the depth limit, Type 3 glyphs drawing through them). They are left untouched:
    /// pruning must never drop an XObject something still draws.
    unsure: HashSet<XKey>,
    /// An incomplete unparsed-paint resource graph is not safe to prune at all.
    paint_budget_exhausted: bool,
    paint_work_left: usize,
    /// A resource-less paint can inherit from each caller. Its appearance role also matters:
    /// an AP stream may omit Subtype, unlike opaque font/profile data reached through resources.
    paint_seen: HashSet<(ObjRef, Owner, bool)>,
    /// An owner's resource catalog is immutable during collection. Shared catalogs and the
    /// content/error paths must not repeatedly consume the protection budget for the same owner.
    resources_seen: HashSet<Owner>,
    /// Nested resource-less paint in a font can still depend on each caller's resources.
    glyph_fonts: HashSet<(ObjRef, Owner)>,
    /// Forms already analysed, with the owner of the resources they drew against: drawing one
    /// again records nothing new, and re-reading it on every `Do` is exponential in the nesting.
    forms_seen: HashSet<(ObjRef, Owner)>,
}

impl Pruner<'_> {
    /// Kind entries may reference primitive objects. Inspect direct values in place, and only
    /// fetch an indirect value, so malformed direct containers are never cloned by a probe.
    fn name_is(&self, value: Option<&Object>, expected: &[u8]) -> bool {
        let Some(value) = value else { return false };
        match value.as_ref() {
            Some(reference) => self.doc.get(reference).as_name() == Some(expected),
            None => value.as_name() == Some(expected),
        }
    }

    fn integer_is(&self, value: Option<&Object>, expected: i64) -> bool {
        let Some(value) = value else { return false };
        match value.as_ref() {
            Some(reference) => self.doc.get(reference).as_int() == Some(expected),
            None => value.as_int() == Some(expected),
        }
    }

    /// Inspect direct containers in place; resolving a direct dictionary would clone it before
    /// the protection work budget has charged its children.
    fn usable_resources(&self, object: &Object) -> bool {
        match object.as_ref() {
            Some(reference) => self.doc.get(reference).as_dict().is_some(),
            None => object.as_dict().is_some(),
        }
    }

    /// Mark the `/XObject` dictionary of `resources` as one not to prune.
    fn distrust(&mut self, resources: &Dict, owner: Owner) {
        if let Some(xobjects) = resources.get(b"XObject") {
            let key = match (xobjects.as_ref(), owner) {
                (Some(reference), _) => XKey::XObject(reference),
                (None, Owner::Resources(reference)) => XKey::Resources(reference),
                (None, Owner::Page(reference)) => XKey::Page(reference),
            };
            self.unsure.insert(key);
        }
    }

    fn paint_work(&mut self, count: usize) -> bool {
        if !self.paint_budget_exhausted
            && let Some(left) = self.paint_work_left.checked_sub(count)
        {
            self.paint_work_left = left;
            return true;
        }
        self.paint_budget_exhausted = true;
        false
    }

    /// Walk one object's direct children without cloning them or recursing. Referenced objects
    /// go on a separate work list, so their borrowed dictionaries stay alive during this pass.
    fn protect_paint_tree(&mut self, object: &Object, owner: Owner, appearance: bool, referenced: &mut Vec<(ObjRef, bool)>) -> bool {
        if !self.paint_work(1) {
            return false;
        }
        let mut inherits = false;
        let mut pending = vec![(object, appearance)];
        while let Some((object, appearance)) = pending.pop() {
            if let Some(dict) = object.as_dict() {
                let paint_stream = matches!(object, Object::Stream(_))
                    && (appearance || self.name_is(dict.get(b"Subtype"), b"Form") || self.integer_is(dict.get(b"PatternType"), 1));
                if (paint_stream || self.name_is(dict.get(b"Subtype"), b"Type3"))
                    && !dict.get(b"Resources").is_some_and(|resources| self.usable_resources(resources))
                {
                    inherits = true;
                }
            }
            match object {
                Object::Ref(r) => {
                    // Unparsed paint can share either the XObject dictionary itself or a
                    // resource owner with page/Form content we do interpret.
                    self.unsure.insert(XKey::XObject(*r));
                    self.unsure.insert(XKey::Resources(*r));
                    self.unsure.insert(XKey::Page(*r));
                    if self.paint_seen.insert((*r, owner, appearance)) {
                        referenced.push((*r, appearance));
                    }
                }
                Object::Array(array) => {
                    if !self.paint_work(array.len()) {
                        return inherits;
                    }
                    pending.extend(array.iter().map(|value| (value, appearance)));
                }
                Object::Dict(_) | Object::Stream(_) => {
                    if let Some(dict) = object.as_dict() {
                        if !self.paint_work(dict.len()) {
                            return inherits;
                        }
                        // Appearance dictionaries contain N/R/D and named states. Once a
                        // stream is reached, its dictionary holds ordinary resource/data graphs.
                        let child_appearance = appearance && !matches!(object, Object::Stream(_));
                        let soft_mask = self.name_is(dict.get(b"S"), b"Luminosity") || self.name_is(dict.get(b"S"), b"Alpha");
                        pending.extend(dict.iter().map(|(key, value)| (value, child_appearance || (soft_mask && key.as_slice() == b"G"))));
                    }
                }
                _ => {}
            }
        }
        inherits
    }

    /// Keep every owner reachable from unparsed paint, without decoding any stream bytes.
    /// Returns whether a paint program may also use its caller's resources.
    fn protect_paint_object(&mut self, object: &Object, owner: Owner, appearance: bool) -> bool {
        let mut referenced = Vec::new();
        let mut inherits = self.protect_paint_tree(object, owner, appearance, &mut referenced);
        while !self.paint_budget_exhausted {
            let Some((r, appearance)) = referenced.pop() else { break };
            let object = self.doc.get(r);
            inherits |= self.protect_paint_tree(&object, owner, appearance, &mut referenced);
        }
        inherits
    }

    fn protect_caller(&mut self, resources: &Dict, owner: Owner) {
        self.distrust(resources, owner);
        for (_, value) in resources.iter() {
            self.protect_paint_object(value, owner, false);
            if self.paint_budget_exhausted {
                break;
            }
        }
    }

    /// Patterns and graphics states can paint through soft masks or select Type 3 fonts.
    /// Their content is not interpreted by this pruner, so protect any aliased resource owners.
    fn protect_resources(&mut self, resources: &Dict, owner: Owner) {
        if self.paint_budget_exhausted || !self.resources_seen.insert(owner) {
            return;
        }
        self.protect_type3_resources(resources, owner);
        for key in [b"Pattern".as_slice(), b"ExtGState"] {
            if let Some(root) = resources.get(key)
                && self.protect_paint_object(root, owner, false)
            {
                self.protect_caller(resources, owner);
            }
        }
    }

    fn protect_appearances(&mut self, page: ObjRef, dict: &Dict, caller: Option<&(Dict, Owner)>) {
        let Some(annotations) = dict.get(b"Annots") else { return };
        let resolved = annotations.as_ref().map(|reference| self.doc.get(reference));
        let Some(annotations) = resolved.as_deref().unwrap_or(annotations).as_array() else { return };
        if !self.paint_work(annotations.len()) {
            return;
        }
        let owner = caller.map_or(Owner::Page(page), |(_, owner)| *owner);
        for annotation in annotations {
            let resolved = annotation.as_ref().map(|reference| self.doc.get(reference));
            let annotation = resolved.as_deref().unwrap_or(annotation);
            if let Some(appearance) = annotation.as_dict().and_then(|dict| dict.get(b"AP"))
                && self.protect_paint_object(appearance, owner, true)
                && let Some((resources, owner)) = caller
            {
                self.protect_caller(resources, *owner);
            }
            if self.paint_budget_exhausted {
                break;
            }
        }
    }

    fn protect_type3_resources(&mut self, resources: &Dict, owner: Owner) {
        if self.paint_budget_exhausted {
            return;
        }
        let Some(fonts) = resources.get(b"Font") else { return };
        let resolved = fonts.as_ref().map(|reference| self.doc.get(reference));
        let Some(fonts) = resolved.as_deref().unwrap_or(fonts).as_dict() else { return };
        // Bound discovery too, but don't spend the glyph-graph budget repeatedly examining an
        // ordinary shared font catalog on every page of a large document.
        if fonts.len() > MAX_PAINT_RESOURCE_WORK {
            self.paint_budget_exhausted = true;
            return;
        }
        for (_, font) in fonts.iter() {
            if self.paint_budget_exhausted {
                return;
            }
            let font_ref = font.as_ref();
            let resolved = font_ref.map(|reference| self.doc.get(reference));
            let Some(font) = resolved.as_deref().unwrap_or(font).as_dict().filter(|font| self.name_is(font.get(b"Subtype"), b"Type3")) else {
                continue;
            };
            if let Some(glyph_resources) = font.get(b"Resources").filter(|resources| self.usable_resources(resources)) {
                if let Some(r) = font_ref
                    && !self.glyph_fonts.insert((r, owner))
                {
                    continue;
                }
                if self.protect_paint_object(glyph_resources, owner, false) {
                    self.protect_caller(resources, owner);
                }
            } else {
                self.protect_caller(resources, owner);
            }
        }
    }

    /// The `/XObject` dictionary of `resources`, with the key its drawn names belong to.
    fn xobjects(&self, resources: &Dict, owner: Owner) -> Option<(Dict, XKey)> {
        let x = resources.get(b"XObject")?;
        if let Some(r) = x.as_ref() {
            return Some((self.doc.resolve(x).as_dict()?.clone(), XKey::XObject(r)));
        }
        let key = match owner {
            Owner::Resources(r) => XKey::Resources(r),
            Owner::Page(p) => XKey::Page(p),
        };
        Some((self.doc.resolve(x).as_dict()?.clone(), key))
    }

    /// Record every XObject name `data` draws with `Do`, descending into form XObjects.
    fn content(&mut self, data: &[u8], resources: &Dict, owner: Owner, depth: u8) {
        self.protect_resources(resources, owner);
        let Some((xobjects, key)) = self.xobjects(resources, owner) else {
            return;
        };
        if depth > MAX_FORM_DEPTH {
            self.unsure.insert(key);
            return;
        }
        for op in pdfcraft_content::parse(data).ops {
            if op.op.as_slice() != b"Do" {
                continue;
            }
            let Some(name) = op.name(0) else { continue };
            self.used.entry(key).or_default().insert(name.to_vec());
            let Some(r) = xobjects.get(name).and_then(Object::as_ref) else { continue };
            let Object::Stream(s) = &*self.doc.get(r) else { continue };
            if !self.name_is(s.dict.get(b"Subtype"), b"Form") {
                continue;
            }
            let (inner, inner_owner) = match s.dict.get(b"Resources") {
                Some(res) => match res.as_ref() {
                    Some(rr) => match self.doc.get(rr).as_dict().cloned() {
                        Some(d) => (d, Owner::Resources(rr)),
                        None => continue,
                    },
                    None => match self.doc.resolve(res).as_dict().cloned() {
                        // The form carries its resources inline: filter them on the form itself.
                        Some(d) => (d, Owner::Resources(r)),
                        None => continue,
                    },
                },
                // A form without its own resources draws against the ones it was invoked with.
                None => (resources.clone(), owner),
            };
            if !self.forms_seen.insert((r, inner_owner)) {
                continue;
            }
            #[cfg(test)]
            crate::work(1);
            match s.decoded() {
                Ok(data) => self.content(&data, &inner, inner_owner, depth + 1),
                // What the form draws is unknown: keep everything it could reach.
                Err(_) => {
                    self.protect_resources(&inner, inner_owner);
                    self.distrust(&inner, inner_owner);
                }
            }
        }
    }
}

/// The page's content, concatenated (`/Contents` may be one stream or an array), decoded
/// against one page's [`DecodeBudget`]. An error when a stream can't be decoded or the budget
/// runs out: the page may draw names that can't be seen.
pub(crate) fn content_bytes(doc: &Document, contents: &Object) -> Result<Vec<u8>, CosError> {
    let mut out = Vec::new();
    for data in DecodeBudget::page().decode_contents(doc, contents) {
        out.extend(data?);
        out.push(b'\n');
    }
    Ok(out)
}

/// The resources of a page (as `walk` reports them) with their owner.
fn resolve_resources(doc: &Document, resources: &Object, page: ObjRef) -> Option<(Dict, Owner)> {
    match resources {
        Object::Ref(r) => doc.get(*r).as_dict().cloned().map(|d| (d, Owner::Resources(*r))),
        Object::Dict(d) => Some((d.clone(), Owner::Page(page))),
        _ => None,
    }
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
            if x.as_ref().is_some() {
                return Ok(0); // recorded as XObject(r) instead
            }
            let Some(mut xd) = doc.resolve(x).as_dict().cloned() else { return Ok(0) };
            let removed = drop_unused(&mut xd, names);
            if removed > 0 {
                doc.update_dict(r, |d| d.set(b"XObject".to_vec(), Object::Dict(xd.clone())))?;
            }
            Ok(removed)
        }
        XKey::Page(p) => {
            let Some(pd) = doc.get(p).as_dict().cloned() else { return Ok(0) };
            let Some(res) = pd.get(b"Resources") else { return Ok(0) };
            if res.as_ref().is_some() {
                return Ok(0); // recorded as Resources(r) instead
            }
            let Some(mut rd) = doc.resolve(res).as_dict().cloned() else { return Ok(0) };
            let Some(x) = rd.get(b"XObject") else { return Ok(0) };
            if x.as_ref().is_some() {
                return Ok(0);
            }
            let Some(mut xd) = doc.resolve(x).as_dict().cloned() else { return Ok(0) };
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
        paint_budget_exhausted: false,
        paint_work_left: MAX_PAINT_RESOURCE_WORK,
        paint_seen: HashSet::new(),
        resources_seen: HashSet::new(),
        glyph_fonts: HashSet::new(),
        forms_seen: HashSet::new(),
    };
    for (page, attrs) in &pages {
        let pd = pruner.doc.get(*page);
        let Some(pd) = pd.as_dict() else { continue };
        let caller = attrs.get(b"Resources").and_then(|res| resolve_resources(pruner.doc, res, *page));
        pruner.protect_appearances(*page, pd, caller.as_ref());
        if pruner.paint_budget_exhausted {
            return Ok(0);
        }
        let Some((res_dict, owner)) = caller else { continue };
        pruner.protect_resources(&res_dict, owner);
        if pruner.paint_budget_exhausted {
            return Ok(0);
        }
        let Some(contents) = pd.get(b"Contents") else { continue };
        match content_bytes(pruner.doc, contents) {
            Ok(data) if data.is_empty() => {}
            Ok(data) => pruner.content(&data, &res_dict, owner, 0),
            Err(_) => {
                pruner.distrust(&res_dict, owner);
            }
        }
    }
    let Pruner { used, unsure, paint_budget_exhausted, .. } = pruner;
    if paint_budget_exhausted {
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
