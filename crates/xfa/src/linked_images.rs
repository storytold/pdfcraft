//! Pictures a template links instead of embedding (`<image href="…">`). Since XFA 2.6 a form
//! keeps them in its PDF: each image file is a stream holding the file's bytes, named in the
//! `XFAImages` name tree of the catalog's `Names` dictionary by the URI the template writes (XFA
//! 3.3 pp. 257–258, "Storing Images Inside PDF"). Only that tree is looked in: as the spec warns,
//! a form that isn't trusted must not have its links followed outside the file.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Document, ObjRef, Object};

use crate::model::{Field, Node, Subform, Template, Value};

/// Most nodes of the name tree visited, and most names kept from it.
const MAX_TREE_NODES: usize = 4096;
const MAX_NAMES: usize = 4096;
/// Most name-and-value pairs looked at, a name repeated counted each time (only a new name is
/// kept, so [`MAX_NAMES`] alone doesn't end a `Names` array that repeats one).
const MAX_PAIRS: usize = 4 * MAX_NAMES;
/// Longest name kept, in bytes: a template's link is a short path or URI, and a longer name
/// could only be matched by an equally long link.
const MAX_NAME_LEN: usize = 4096;
/// Deepest the name tree is followed.
const MAX_DEPTH: usize = 32;
/// Largest picture taken from the file, as for a picture written into the template. Small in
/// tests, so a test can pass it.
#[cfg(not(test))]
const MAX_IMAGE: usize = 32 << 20;
#[cfg(test)]
const MAX_IMAGE: usize = 512;
/// Most bytes of pictures taken from the file in all, each place a picture is used counted (a
/// picture is held once per place). Small in tests, so a test can reach it.
#[cfg(not(test))]
const MAX_TOTAL: usize = 128 << 20;
#[cfg(test)]
const MAX_TOTAL: usize = 1 << 10;
/// Deepest the template is walked.
const MAX_TEMPLATE_DEPTH: usize = 256;

/// `f` of `o`, a reference followed. A direct object is lent as it is, not copied: a tree's
/// arrays can be as long as the file allows.
fn with_resolved<R>(doc: &Document, o: &Object, f: impl FnOnce(&Object) -> R) -> R {
    match o {
        Object::Ref(r) => f(&doc.get(*r)),
        other => f(other),
    }
}

/// The `XFAImages` name tree: each name, as text, and the object it names.
fn names(doc: &Document) -> HashMap<String, Object> {
    let mut walk = Walk { doc, out: HashMap::new(), seen: HashSet::new(), visited: 0, pairs: 0 };
    let Some(root) = doc.root() else { return walk.out };
    let catalog = doc.get(root);
    if let Some(names) = catalog.as_dict().and_then(|c| c.get(b"Names")) {
        with_resolved(doc, names, |n| {
            if let Some(tree) = n.as_dict().and_then(|d| d.get(b"XFAImages")) {
                walk.node(tree, 0);
            }
        });
    }
    walk.out
}

/// A walk of the name tree, borrowing each node from its resolved owner: nothing of the tree
/// is copied but the names kept, and only the branch being walked is held.
struct Walk<'a> {
    doc: &'a Document,
    out: HashMap<String, Object>,
    seen: HashSet<ObjRef>,
    visited: usize,
    /// Pairs looked at so far, repeats included.
    pairs: usize,
}

impl Walk<'_> {
    fn full(&self) -> bool {
        self.visited >= MAX_TREE_NODES || self.out.len() >= MAX_NAMES || self.pairs >= MAX_PAIRS
    }

    fn node(&mut self, node: &Object, depth: usize) {
        // Too deep: this branch is left, the others are still walked.
        if self.full() || depth > MAX_DEPTH {
            return;
        }
        self.visited += 1;
        if let Some(r) = node.as_ref()
            && !self.seen.insert(r)
        {
            return;
        }
        let doc = self.doc;
        with_resolved(doc, node, |node| {
            let Some(d) = node.as_dict() else { return };
            if let Some(pairs) = d.get(b"Names") {
                with_resolved(doc, pairs, |pairs| self.pairs(pairs));
            }
            if let Some(kids) = d.get(b"Kids") {
                with_resolved(doc, kids, |kids| {
                    for kid in kids.as_array().map(Vec::as_slice).unwrap_or_default() {
                        if self.full() {
                            break;
                        }
                        self.node(kid, depth + 1);
                    }
                });
            }
        });
    }

    fn pairs(&mut self, pairs: &Object) {
        let doc = self.doc;
        for pair in pairs.as_array().map(Vec::as_slice).unwrap_or_default().chunks(2) {
            // The names and pairs limits only: the node limit ends the walk at the next node, not
            // the names of the node it has just come to.
            if self.out.len() >= MAX_NAMES || self.pairs >= MAX_PAIRS {
                break;
            }
            self.pairs += 1;
            // An over-long name is left before it is turned into text.
            if let [key, value] = pair
                && let Some(key) = with_resolved(doc, key, |k| k.as_string().filter(|s| s.bytes.len() <= MAX_NAME_LEN).map(|s| s.to_text()))
            {
                self.out.entry(key).or_insert_with(|| value.clone());
            }
        }
    }
}

/// Pictures found so far by their link, read from the file once each.
struct Found<'a> {
    doc: &'a Document,
    names: HashMap<String, Object>,
    read: HashMap<String, Option<Vec<u8>>>,
    /// Bytes of the pictures in `read`, and of the copies put in places: each at most
    /// [`MAX_TOTAL`], however many names the tree holds.
    kept: usize,
    total: usize,
    warnings: Vec<String>,
}

impl Found<'_> {
    /// A copy of the picture linked as `href` for one more place, when the file holds it and the
    /// pictures kept and the copies so far leave room for it under [`MAX_TOTAL`].
    fn get(&mut self, href: &str) -> Option<Vec<u8>> {
        if !self.read.contains_key(href) {
            // Past the total, a picture not read yet is neither read nor kept.
            let data = if self.kept >= MAX_TOTAL { None } else { self.load(href) };
            let data = match data {
                Some(d) if self.kept.saturating_add(d.len()) > MAX_TOTAL => {
                    self.over_total();
                    None
                }
                Some(d) => {
                    self.kept = self.kept.saturating_add(d.len());
                    Some(d)
                }
                None => {
                    if self.kept >= MAX_TOTAL && self.names.contains_key(href) {
                        self.over_total();
                    }
                    None
                }
            };
            self.read.insert(href.to_string(), data);
        }
        let len = self.read.get(href).and_then(Option::as_ref).map(Vec::len)?;
        if self.total.saturating_add(len) > MAX_TOTAL {
            self.over_total();
            return None;
        }
        self.total = self.total.saturating_add(len);
        self.read.get(href).cloned().flatten()
    }

    fn over_total(&mut self) {
        self.warnings.push("the pictures the form keeps in the file take too much memory in all; the rest are left out".into());
    }

    /// Bytes of the pictures read so far and kept for reuse.
    #[cfg(test)]
    fn kept(&self) -> usize {
        self.read.values().flatten().map(Vec::len).sum()
    }

    fn load(&mut self, href: &str) -> Option<Vec<u8>> {
        let object = self.names.get(href)?;
        let object = self.doc.resolve(object);
        let Object::Stream(s) = &*object else { return None };
        let data = match s.decoded_within(MAX_IMAGE) {
            Ok(d) => d,
            Err(_) => {
                self.warnings.push(format!("a picture the form keeps in the file ({href}) could not be read; it is left out"));
                return None;
            }
        };
        Some(data)
    }

    fn value(&mut self, v: &mut Value) {
        if let Value::Image { data, href: Some(href), .. } = v
            && data.is_empty()
            && let Some(found) = self.get(href)
        {
            *data = found;
        }
    }

    fn field(&mut self, f: &mut Field) {
        self.value(&mut f.value);
        if let Some(c) = f.caption.as_mut() {
            self.value(&mut c.value);
        }
    }

    fn nodes(&mut self, nodes: &mut [Node], depth: usize) {
        if depth > MAX_TEMPLATE_DEPTH {
            return;
        }
        for n in nodes {
            match n {
                Node::Subform(s) => self.subform(s, depth + 1),
                Node::Area(a) => self.nodes(&mut a.children, depth + 1),
                Node::Field(f) => self.field(f),
                Node::Draw(d) => self.value(&mut d.value),
                Node::ExclGroup(g) => g.fields.iter_mut().for_each(|f| self.field(f)),
            }
        }
    }

    fn subform(&mut self, s: &mut Subform, depth: usize) {
        if let Some(set) = s.page_set.as_mut() {
            for area in &mut set.areas {
                self.nodes(&mut area.items, depth + 1);
            }
        }
        self.nodes(&mut s.children, depth);
    }
}

/// Give the pictures `tpl` links to (and doesn't embed) the data `doc` keeps for them. A link
/// the file holds nothing for stays empty. What couldn't be read is returned as warnings.
pub fn resolve(tpl: &mut Template, doc: &Document) -> Vec<String> {
    resolve_found(tpl, doc).map(|f| f.warnings).unwrap_or_default()
}

fn resolve_found<'a>(tpl: &mut Template, doc: &'a Document) -> Option<Found<'a>> {
    let names = names(doc);
    if names.is_empty() {
        return None;
    }
    let mut found = Found { doc, names, read: HashMap::new(), kept: 0, total: 0, warnings: Vec::new() };
    found.subform(&mut tpl.root, 0);
    found.warnings.dedup();
    Some(found)
}

/// [`resolve`], and the bytes of pictures it held for reuse at the end.
#[cfg(test)]
pub(crate) fn resolve_keeping(tpl: &mut Template, doc: &Document) -> (Vec<String>, usize) {
    resolve_found(tpl, doc).map_or((Vec::new(), 0), |f| (f.warnings.clone(), f.kept()))
}
