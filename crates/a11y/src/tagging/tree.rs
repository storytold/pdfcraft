//! The structure tree as an index an edit can trust (ISO 32000-2 §14.7.2): every reachable
//! element in document order, where it is listed, its effective page, and which content items
//! (marked content and object references) it owns.
//!
//! The index never guesses. An element listed twice, a cycle, or an element inside a direct
//! (inline) element dictionary is recorded as such, and the edits refuse to touch it.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

/// How many elements the index reads. Larger trees are refused for editing.
pub(crate) const MAX_ELEMENTS: usize = 500_000;
/// How deep the index reads.
pub(crate) const MAX_DEPTH: usize = 256;

/// Where a structure element is listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Container {
    /// The structure tree root's `/K`.
    Root,
    /// An indirect element's `/K`.
    Element(ObjRef),
    /// Inside a direct element dictionary, which nothing can address.
    Inline,
}

/// One child of a structure element (an entry of its `/K`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Kid {
    Element(ObjRef),
    /// A direct element dictionary.
    Inline,
    /// Marked content: `page` is the reference's `/Pg` or the element's page, `stream` its `/Stm`.
    Mcid {
        mcid: i64,
        page: Option<ObjRef>,
        stream: Option<ObjRef>,
    },
    /// An object reference (an annotation or an XObject).
    Object {
        object: ObjRef,
        page: Option<ObjRef>,
    },
    /// Anything else, kept as it is.
    Other,
}

/// Who owns a content item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Owner {
    Element(ObjRef),
    /// A direct element dictionary.
    Inline,
}

/// A reachable indirect structure element.
#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub object: ObjRef,
    pub dict: Dict,
    pub container: Container,
    /// `/Pg`, else the nearest ancestor's.
    pub page: Option<ObjRef>,
    pub depth: usize,
    /// Listed more than once, or part of a cycle.
    pub shared: bool,
}

/// The structure tree.
#[derive(Clone, Debug, Default)]
pub(crate) struct Tree {
    /// The structure tree root when it is an indirect object.
    pub root: Option<ObjRef>,
    pub root_kids: Vec<Kid>,
    pub nodes: Vec<Node>,
    pub index: HashMap<ObjRef, usize>,
    /// (page, MCID) → owners, for marked content drawn directly in page content.
    pub mcid_owners: HashMap<(ObjRef, i64), Vec<Owner>>,
    /// Annotation or XObject → owners.
    pub object_owners: HashMap<ObjRef, Vec<Owner>>,
    /// More elements or levels than the index reads.
    pub truncated: bool,
}

/// The catalog's structure tree root dictionary, and its reference when indirect.
pub(crate) fn root_of(doc: &Document) -> Option<(Option<ObjRef>, Dict)> {
    let catalog = doc.root().and_then(|r| doc.get(r).as_dict().cloned())?;
    let entry = catalog.get(b"StructTreeRoot")?;
    let dict = doc.resolve(entry).as_dict().cloned()?;
    Some((entry.as_ref(), dict))
}

/// A `/K` value as its list of entries: a single entry, an array, or an indirect array.
pub(crate) fn kid_items(doc: &Document, k: Option<&Object>) -> Vec<Object> {
    match k {
        None => Vec::new(),
        Some(Object::Array(items)) => items.clone(),
        Some(Object::Ref(r)) => match &*doc.get(*r) {
            Object::Array(items) => items.clone(),
            _ => vec![Object::Ref(*r)],
        },
        Some(other) => vec![other.clone()],
    }
}

/// What one `/K` entry is, for an element on `page`.
pub(crate) fn classify(doc: &Document, item: &Object, page: Option<ObjRef>) -> Kid {
    let reference = item.as_ref();
    let resolved = doc.resolve(item);
    match &*resolved {
        Object::Int(mcid) => Kid::Mcid { mcid: *mcid, page, stream: None },
        Object::Dict(d) if d.name(b"Type") == Some(b"MCR") => match d.get(b"MCID").and_then(Object::as_int) {
            Some(mcid) => Kid::Mcid { mcid, page: d.get(b"Pg").and_then(Object::as_ref).or(page), stream: d.get(b"Stm").and_then(Object::as_ref) },
            None => Kid::Other,
        },
        Object::Dict(d) if d.name(b"Type") == Some(b"OBJR") => match d.get(b"Obj").and_then(Object::as_ref) {
            Some(object) => Kid::Object { object, page: d.get(b"Pg").and_then(Object::as_ref).or(page) },
            None => Kid::Other,
        },
        Object::Dict(d) if d.contains(b"S") => match reference {
            Some(r) => Kid::Element(r),
            None => Kid::Inline,
        },
        _ => Kid::Other,
    }
}

impl Tree {
    /// Read the structure tree (an empty tree when there is none).
    pub fn read(doc: &Document) -> Tree {
        let mut tree = Tree::default();
        let Some((root, dict)) = root_of(doc) else { return tree };
        tree.root = root;
        tree.root_kids = kid_items(doc, dict.get(b"K")).iter().map(|k| classify(doc, k, None)).collect();
        let mut seen: HashSet<ObjRef> = HashSet::new();
        // (entry, where it is listed, inherited page, depth)
        let mut stack: Vec<(Object, Container, Option<ObjRef>, usize)> = Vec::new();
        for item in kid_items(doc, dict.get(b"K")).into_iter().rev() {
            stack.push((item, Container::Root, None, 0));
        }
        while let Some((item, container, page, depth)) = stack.pop() {
            if depth > MAX_DEPTH || tree.nodes.len() >= MAX_ELEMENTS {
                tree.truncated = true;
                continue;
            }
            match classify(doc, &item, page) {
                Kid::Element(r) => {
                    if !seen.insert(r) {
                        if let Some(&i) = tree.index.get(&r)
                            && let Some(node) = tree.nodes.get_mut(i)
                        {
                            node.shared = true;
                        }
                        continue;
                    }
                    let Some(d) = doc.get(r).as_dict().cloned() else { continue };
                    let page = d.get(b"Pg").and_then(Object::as_ref).or(page);
                    let items = kid_items(doc, d.get(b"K"));
                    let kids: Vec<Kid> = items.iter().map(|k| classify(doc, k, page)).collect();
                    tree.own(Owner::Element(r), &kids);
                    for k in items.into_iter().rev() {
                        stack.push((k, Container::Element(r), page, depth.saturating_add(1)));
                    }
                    tree.index.insert(r, tree.nodes.len());
                    tree.nodes.push(Node { object: r, dict: d, container, page, depth, shared: false });
                }
                Kid::Inline => {
                    let Some(d) = doc.resolve(&item).as_dict().cloned() else { continue };
                    let page = d.get(b"Pg").and_then(Object::as_ref).or(page);
                    let items = kid_items(doc, d.get(b"K"));
                    let kids: Vec<Kid> = items.iter().map(|k| classify(doc, k, page)).collect();
                    tree.own(Owner::Inline, &kids);
                    for k in items.into_iter().rev() {
                        stack.push((k, Container::Inline, page, depth.saturating_add(1)));
                    }
                }
                _ => {}
            }
        }
        tree
    }

    fn own(&mut self, owner: Owner, kids: &[Kid]) {
        for kid in kids {
            match kid {
                Kid::Mcid { mcid, page: Some(page), stream: None } => self.mcid_owners.entry((*page, *mcid)).or_default().push(owner),
                Kid::Object { object, .. } => self.object_owners.entry(*object).or_default().push(owner),
                _ => {}
            }
        }
    }

    pub fn node(&self, r: ObjRef) -> Option<&Node> {
        self.index.get(&r).and_then(|&i| self.nodes.get(i))
    }

    /// The one element owning marked content `mcid` on `page`, if exactly one indirect element does.
    pub fn mcid_owner(&self, page: ObjRef, mcid: i64) -> Option<ObjRef> {
        match self.mcid_owners.get(&(page, mcid)).map(Vec::as_slice) {
            Some([Owner::Element(r)]) => Some(*r),
            _ => None,
        }
    }

    /// The one element owning an annotation or XObject reference.
    pub fn object_owner(&self, object: ObjRef) -> Option<ObjRef> {
        match self.object_owners.get(&object).map(Vec::as_slice) {
            Some([Owner::Element(r)]) => Some(*r),
            _ => None,
        }
    }

    /// `r` and every element below it, in document order (`r` first).
    pub fn subtree(&self, r: ObjRef) -> Vec<ObjRef> {
        let Some(&start) = self.index.get(&r) else { return Vec::new() };
        let Some(depth) = self.nodes.get(start).map(|n| n.depth) else { return Vec::new() };
        let mut out = vec![r];
        // Document order puts descendants right after the element, deeper than it.
        for node in self.nodes.iter().skip(start.saturating_add(1)) {
            if node.depth <= depth {
                break;
            }
            out.push(node.object);
        }
        out
    }

    /// Is `ancestor` the element `r` or one of its ancestors?
    pub fn is_within(&self, r: ObjRef, ancestor: ObjRef) -> bool {
        let mut at = Some(r);
        for _ in 0..=MAX_DEPTH {
            let Some(current) = at else { return false };
            if current == ancestor {
                return true;
            }
            at = match self.node(current).map(|n| n.container) {
                Some(Container::Element(parent)) => Some(parent),
                _ => None,
            };
        }
        false
    }

    /// The top element new content goes into by default: the root's only element (usually
    /// `/Document`), else the root itself.
    pub fn top(&self) -> Option<ObjRef> {
        let elements: Vec<ObjRef> = self.root_kids.iter().filter_map(|k| if let Kid::Element(r) = k { Some(*r) } else { None }).collect();
        match elements.as_slice() {
            [only] if self.node(*only).is_some_and(|n| !n.shared) => Some(*only),
            _ => None,
        }
    }
}
