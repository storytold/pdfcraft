//! Bounded, read-only ownership of structure-element replacement text.
//!
//! The forward structure tree is the authority here: `/K` names its content, with `/Pg`
//! inherited down the tree and an MCR's `/Stm` identifying a different content context.
//! No content is substituted until this index has finished validating all of its ownership.

use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

const MAX_DEPTH: usize = 128;
const MAX_WORK: usize = 1_000_000;
const MAX_OWNERS: usize = 100_000;
const MAX_KEYS: usize = 100_000;
const MAX_RAW_TEXT: usize = 4 << 20;
const MAX_DECODED_TEXT: usize = 16 << 20;

/// An MCID belongs to the page's combined content, or to a particular form stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct ContentKey {
    pub page: ObjRef,
    pub stream: Option<ObjRef>,
    pub mcid: i64,
}

struct Owner {
    text: String,
    members: Vec<ContentKey>,
    hint: Option<ObjRef>,
    valid: bool,
}

/// Only complete, single-page owners are exposed to the reading pass.
#[derive(Default)]
pub(super) struct StructureTextIndex {
    owners: Vec<Owner>,
    // A declared key remains visible even if it has no usable replacement owner.
    bindings: HashMap<ContentKey, Option<usize>>,
    by_page: HashMap<ObjRef, Vec<usize>>,
    incomplete_pages: HashSet<ObjRef>,
    all_incomplete: bool,
}

impl StructureTextIndex {
    pub(super) fn new(doc: &Document, pages: &[ObjRef]) -> Self {
        Self::with_limits(doc, pages, Limits::DEFAULT)
    }

    fn with_limits(doc: &Document, pages: &[ObjRef], limits: Limits) -> Self {
        let Some(root) = doc.root() else { return Self::default() };
        // A normal catalog needs one bounded object lookup to prove that there is
        // no structure work at all, regardless of the page count or walk budget.
        // Aliased or unreadable catalogs still go through the guarded resolver.
        if let Ok(catalog) = doc.try_get(root.num)
            && let Some(catalog) = catalog.as_dict()
            && !catalog.contains(b"StructTreeRoot")
        {
            return Self::default();
        }
        let mut walk = Walk {
            doc,
            limits,
            remaining: limits.work,
            decoded: 0,
            pages: HashSet::new(),
            path: HashSet::new(),
            owners: Vec::new(),
            keys: HashMap::new(),
            incomplete: HashSet::new(),
            all_incomplete: false,
            stopped: false,
        };
        walk.start(pages);
        walk.finish()
    }

    pub(super) fn owner(&self, key: &ContentKey) -> Option<usize> {
        self.bindings.get(key).copied().flatten()
    }

    /// Declared structure content must not be absorbed by a different active owner.
    pub(super) fn declared(&self, key: &ContentKey) -> bool {
        self.bindings.contains_key(key)
    }

    pub(super) fn text(&self, owner: usize) -> Option<&str> {
        self.owners.get(owner).filter(|owner| owner.valid).map(|owner| owner.text.as_str())
    }

    pub(super) fn members(&self, owner: usize) -> &[ContentKey] {
        self.owners.get(owner).filter(|owner| owner.valid).map_or(&[], |owner| owner.members.as_slice())
    }

    pub(super) fn owners_on_page(&self, page: ObjRef) -> impl Iterator<Item = usize> + '_ {
        self.by_page.get(&page).into_iter().flatten().copied()
    }

    pub(super) fn incomplete(&self, page: ObjRef) -> bool {
        self.all_incomplete || self.incomplete_pages.contains(&page)
    }
}

#[derive(Clone, Copy)]
struct Limits {
    depth: usize,
    work: usize,
    owners: usize,
    keys: usize,
    raw_text: usize,
    decoded_text: usize,
}

impl Limits {
    const DEFAULT: Self =
        Self { depth: MAX_DEPTH, work: MAX_WORK, owners: MAX_OWNERS, keys: MAX_KEYS, raw_text: MAX_RAW_TEXT, decoded_text: MAX_DECODED_TEXT };
}

/// Borrow direct objects and retain indirect objects without cloning a wide `/K` container.
enum Value<'a> {
    Borrowed(&'a Object),
    Shared(Arc<Object>),
}

impl Deref for Value<'_> {
    type Target = Object;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Borrowed(value) => value,
            Self::Shared(value) => value,
        }
    }
}

struct Resolved<'a> {
    value: Value<'a>,
    reference: Option<ObjRef>,
}

#[derive(Clone, Copy)]
struct Scope {
    page: Option<ObjRef>,
    owner: Option<usize>,
    parent: Option<ObjRef>,
}

struct Walk<'a> {
    doc: &'a Document,
    limits: Limits,
    remaining: usize,
    decoded: usize,
    pages: HashSet<ObjRef>,
    path: HashSet<ObjRef>,
    owners: Vec<Owner>,
    // Include content with no replacement too: another owner cannot claim the same key.
    keys: HashMap<ContentKey, Option<usize>>,
    incomplete: HashSet<ObjRef>,
    all_incomplete: bool,
    stopped: bool,
}

impl Walk<'_> {
    fn stop(&mut self) {
        self.stopped = true;
        self.all_incomplete = true;
    }

    fn spend(&mut self) -> bool {
        if self.stopped || self.remaining == 0 {
            self.stop();
            return false;
        }
        self.remaining -= 1;
        true
    }

    /// Resolve scalar/container aliases under the same work cap as the tree walk.
    fn read<'a>(&mut self, object: &'a Object) -> Option<Resolved<'a>> {
        if !self.spend() {
            return None;
        }
        let Object::Ref(first) = object else { return Some(Resolved { value: Value::Borrowed(object), reference: None }) };
        let mut reference = *first;
        let mut seen = HashSet::new();
        loop {
            if seen.len() >= self.limits.depth || !seen.insert(reference) || !self.spend() {
                self.stop();
                return None;
            }
            let value = match self.doc.try_get(reference.num) {
                Ok(value) => value,
                Err(_) => {
                    self.stop();
                    return None;
                }
            };
            if let Object::Ref(next) = &*value {
                reference = *next;
            } else {
                return Some(Resolved { value: Value::Shared(value), reference: Some(reference) });
            }
        }
    }

    fn start(&mut self, pages: &[ObjRef]) {
        if self.stopped {
            return;
        }
        let Some(root) = self.doc.root() else { return };
        let root = Object::Ref(root);
        let Some(catalog) = self.read(&root) else { return };
        let Some(catalog) = catalog.value.as_dict() else {
            self.stop();
            return;
        };
        let Some(tree) = catalog.get(b"StructTreeRoot") else { return };
        let Some(tree) = self.read(tree) else { return };
        let Some(dict) = tree.value.as_dict() else {
            self.stop();
            return;
        };
        if let Some(kids) = dict.get(b"K") {
            for page in pages {
                if !self.spend() {
                    return;
                }
                self.pages.insert(*page);
            }
            self.visit(kids, Scope { page: None, owner: None, parent: tree.reference }, 0);
        }
    }

    fn page(&mut self, dict: &Dict, inherited: Option<ObjRef>) -> Option<ObjRef> {
        let Some(value) = dict.get(b"Pg") else { return inherited };
        let value = self.read(value)?;
        let page = value.reference.filter(|reference| self.pages.contains(reference));
        if page.is_none() {
            // An unreadable page constraint might otherwise authorize another page's MCID.
            self.stop();
        }
        page
    }

    fn bad(&mut self, scope: Scope) {
        if let Some(owner) = scope.owner.and_then(|owner| self.owners.get_mut(owner)) {
            owner.valid = false;
        }
        if let Some(page) = scope.page {
            self.incomplete.insert(page);
        } else {
            self.all_incomplete = true;
        }
    }

    fn visit(&mut self, object: &Object, scope: Scope, depth: usize) {
        if depth > self.limits.depth {
            self.stop();
            return;
        }
        let Some(value) = self.read(object) else { return };
        if let Some(reference) = value.reference
            && !self.path.insert(reference)
        {
            self.stop();
            return;
        }
        match &*value.value {
            Object::Array(items) => {
                // A cursor on the borrowed array, not one cloned task per child.
                for item in items {
                    self.visit(item, scope, depth.saturating_add(1));
                    if self.stopped {
                        break;
                    }
                }
            }
            Object::Int(mcid) => self.content(scope, None, *mcid),
            Object::Dict(dict) if dict.contains(b"S") => self.element(dict, value.reference, scope, depth),
            Object::Dict(dict) if dict.name(b"Type") == Some(b"MCR".as_slice()) => self.marked(dict, scope),
            Object::Dict(dict) if dict.name(b"Type") == Some(b"OBJR".as_slice()) => {
                let scope = Scope { page: self.page(dict, scope.page), ..scope };
                // Graphic-only replacement placement is outside this text reader's scope.
                if scope.owner.is_some() {
                    self.bad(scope);
                }
            }
            _ => {
                self.bad(scope);
                // An unknown container could hide a second claim on any earlier MCID.
                self.stop();
            }
        }
        if let Some(reference) = value.reference {
            self.path.remove(&reference);
        }
    }

    fn element(&mut self, dict: &Dict, reference: Option<ObjRef>, scope: Scope, depth: usize) {
        let mut scope = Scope { page: self.page(dict, scope.page), ..scope };
        if self.stopped {
            return;
        }
        let role_is_name = dict.get(b"S").and_then(|role| self.read(role)).is_some_and(|role| matches!(&*role.value, Object::Name(_)));
        let type_is_element = match dict.get(b"Type") {
            None => true,
            Some(kind) => self.read(kind).is_some_and(|kind| kind.value.as_name() == Some(b"StructElem".as_slice())),
        };
        if !role_is_name || !type_is_element {
            self.bad(scope);
            self.stop();
            return;
        }
        if let Some(parent) = dict.get(b"P") {
            let Some(parent) = self.read(parent) else { return };
            if scope.parent.is_some() && parent.reference != scope.parent {
                self.bad(scope);
                // The forward and reverse parents disagree: do not expose any prefix map.
                self.stop();
                return;
            }
        }
        if scope.owner.is_none()
            && let Some(text) = dict.get(b"ActualText")
        {
            if self.owners.len() >= self.limits.owners {
                self.stop();
                return;
            }
            let text = self.text_value(text);
            let owner = self.owners.len();
            self.owners.push(Owner { valid: text.is_some(), text: text.unwrap_or_default(), members: Vec::new(), hint: scope.page });
            scope.owner = Some(owner);
        }
        scope.parent = reference;
        if let Some(kids) = dict.get(b"K") {
            self.visit(kids, scope, depth.saturating_add(1));
        }
    }

    fn text_value(&mut self, object: &Object) -> Option<String> {
        let value = self.read(object)?;
        let Object::String(text) = &*value.value else { return None };
        let bound = text.bytes.len().checked_mul(3)?;
        if text.bytes.len() > self.limits.raw_text || bound > self.limits.decoded_text.saturating_sub(self.decoded) {
            return None;
        }
        let text = text.to_text();
        let decoded = self.decoded.checked_add(text.len())?;
        if decoded > self.limits.decoded_text {
            return None;
        }
        self.decoded = decoded;
        Some(text)
    }

    fn marked(&mut self, dict: &Dict, scope: Scope) {
        let scope = Scope { page: self.page(dict, scope.page), ..scope };
        if self.stopped {
            return;
        }
        let Some(mcid) = dict.get(b"MCID").and_then(|value| self.read(value)).and_then(|value| value.value.as_int()) else {
            self.bad(scope);
            return;
        };
        let stream = match dict.get(b"Stm") {
            None => None,
            Some(value) => {
                let Some(value) = self.read(value) else { return };
                let form = match &*value.value {
                    Object::Stream(stream) => stream.dict.name(b"Subtype") == Some(b"Form".as_slice()),
                    _ => false,
                };
                if !form || value.reference.is_none() || dict.contains(b"StmOwn") {
                    self.bad(scope);
                    self.stop();
                    return;
                }
                value.reference
            }
        };
        if dict.contains(b"StmOwn") {
            self.bad(scope);
            self.stop();
            return;
        }
        self.content(scope, stream, mcid);
    }

    fn content(&mut self, scope: Scope, stream: Option<ObjRef>, mcid: i64) {
        let Some(page) = scope.page else {
            self.bad(scope);
            self.stop();
            return;
        };
        if mcid < 0 {
            self.bad(scope);
            return;
        }
        let key = ContentKey { page, stream, mcid };
        if let Some(previous) = self.keys.get(&key).copied() {
            if let Some(owner) = previous.and_then(|owner| self.owners.get_mut(owner)) {
                owner.valid = false;
            }
            self.bad(scope);
            return;
        }
        if self.keys.len() >= self.limits.keys {
            self.stop();
            return;
        }
        self.keys.insert(key, scope.owner);
        if let Some(owner) = scope.owner.and_then(|owner| self.owners.get_mut(owner)) {
            if owner.members.first().is_some_and(|member| member.page != page) {
                owner.valid = false;
            }
            owner.members.push(key);
        }
    }

    fn finish(mut self) -> StructureTextIndex {
        for owner in &mut self.owners {
            if owner.members.is_empty() || self.stopped {
                owner.valid = false;
            }
            if !owner.valid {
                if owner.members.is_empty() {
                    match owner.hint {
                        Some(page) => {
                            self.incomplete.insert(page);
                        }
                        None => self.all_incomplete = true,
                    }
                }
                self.incomplete.extend(owner.members.iter().map(|key| key.page));
            }
        }
        let mut bindings = HashMap::new();
        let mut by_page: HashMap<ObjRef, Vec<usize>> = HashMap::new();
        if !self.stopped {
            bindings = self.keys;
            for owner in bindings.values_mut() {
                *owner = (*owner).filter(|owner| self.owners.get(*owner).is_some_and(|owner| owner.valid));
            }
            for (id, owner) in self.owners.iter().enumerate().filter(|(_, owner)| owner.valid) {
                if let Some(member) = owner.members.first() {
                    by_page.entry(member.page).or_default().push(id);
                }
            }
        }
        StructureTextIndex { owners: self.owners, bindings, by_page, incomplete_pages: self.incomplete, all_incomplete: self.all_incomplete }
    }
}

#[cfg(test)]
mod tests;
