//! Tagging: the structural edits of the Tags panel and the Reading Order tool.
//!
//! Every operation keeps the tagged structure consistent (ISO 32000-2 §14.7): each element's
//! `/P` names the element or root that lists it, every marked-content item and object reference
//! an element owns has its parent-tree entry pointing back at that element, MCIDs stay unique on
//! their page, and new structure types are standard or defined by the role map. Content is never
//! dropped: page content changes only by marked-content operators spliced in or renamed, so the
//! page draws exactly as before.
//!
//! Operations work on a copy and replace the document only when every step succeeded. Cases
//! that can't be done safely (an element listed twice, a parent stored inline, content in a form
//! XObject, a page whose content can't be decoded exactly) are refused with a reason.

mod check;
mod content;
mod fields;
mod parent_tree;
mod tree;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap, HashSet};

use pdfcraft_cos::{CosError, Dict, Document, ObjRef, Object, PdfString};

pub use check::structure_problems;
pub use content::UnitKind;
use content::{Mark, PageContent, Source, Unit};
pub use fields::tag_form_fields;
use parent_tree::ParentTree;
use tree::{Container, Kid, Node, Owner, Tree, classify, kid_items};

/// How a unit of page content is marked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitMark {
    /// Marked content owned by exactly one structure tag.
    Tagged {
        mcid: i64,
        tag: ObjRef,
    },
    /// Marked content with an MCID no tag owns: untagged for a reader.
    Orphan {
        mcid: i64,
    },
    /// Marked content owned by an inline tag or by several tags: not editable here.
    Unaddressable {
        mcid: i64,
    },
    Artifact,
    /// Content outside any tagged or artifact marked content.
    Untagged,
    /// A form XObject with structure of its own.
    FormStructure,
}

/// One unit of a page's content: a tagged or artifact marked-content sequence, or one object
/// drawn outside both.
#[derive(Clone, Debug, PartialEq)]
pub struct ContentUnit {
    /// Position in the page's list (what [`Selection::Units`] takes).
    pub index: usize,
    pub kind: UnitKind,
    pub mark: UnitMark,
    /// Where it draws, in user space.
    pub bbox: Option<[f64; 4]>,
    /// The text it shows (shortened).
    pub text: String,
    /// It holds another tagged sequence, so it can't be retagged as a whole.
    pub nested: bool,
}

impl ContentUnit {
    /// Content a reader doesn't reach through the tags: untagged, or marked with an MCID no tag owns.
    pub fn is_untagged(&self) -> bool {
        matches!(self.mark, UnitMark::Untagged | UnitMark::Orphan { .. })
    }
}

/// An annotation on the page and the tag that owns it.
#[derive(Clone, Debug, PartialEq)]
pub struct AnnotationItem {
    pub object: ObjRef,
    pub subtype: String,
    pub rect: Option<[f64; 4]>,
    pub tag: Option<ObjRef>,
}

/// A numbered region of the Reading Order tool: a tag that owns content on the page, in
/// structure order.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    /// 1-based position in reading order on this page.
    pub order: usize,
    pub tag: ObjRef,
    pub kind: String,
    pub bbox: Option<[f64; 4]>,
    /// Units it owns (indices into [`PageTagging::units`]).
    pub units: Vec<usize>,
    pub annotations: Vec<ObjRef>,
    pub text: String,
}

/// A page as the Reading Order tool and the Tags panel see it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageTagging {
    pub units: Vec<ContentUnit>,
    pub annotations: Vec<AnnotationItem>,
    pub regions: Vec<Region>,
    /// Why the page's content can't be rewritten (tags can still be moved and edited).
    pub read_only: Option<String>,
}

/// Which content of a page an operation takes.
#[derive(Clone, Debug, PartialEq)]
pub enum Selection {
    /// Units by index (from [`page_tagging`]).
    Units(Vec<usize>),
    /// The units drawn mostly inside a rectangle (user space).
    Rect([f64; 4]),
}

/// Where a tag goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination {
    /// Just before this tag, under the same parent.
    Before(ObjRef),
    /// Just after this tag, under the same parent.
    After(ObjRef),
    /// The last child of this tag (`None`: of the structure tree root).
    Into(Option<ObjRef>),
    /// Child number `index` (0-based) of this tag or the root.
    At(Option<ObjRef>, usize),
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum TaggingError {
    #[error("the PDF has no document catalog")]
    NoCatalog,
    #[error("the document has no tags")]
    Untagged,
    #[error("object {0} is not a structure tag in the tag tree")]
    NotATag(ObjRef),
    #[error("tag {0} is listed more than once in the tag tree, so it can't be edited safely")]
    Shared(ObjRef),
    #[error("tag {0} is stored inside an inline tag, which can't be edited")]
    InlineParent(ObjRef),
    #[error("the parent lists tag {0} more than once")]
    Ambiguous(ObjRef),
    #[error("a tag can't be moved into itself or a tag inside it")]
    Cycle,
    #[error("the tag is already at that position")]
    AlreadyThere,
    #[error("position {0} is past the end of the parent's tags")]
    BadIndex(usize),
    #[error("{0:?} is not a standard tag type or one the role map defines; add it to the role map first")]
    InvalidType(String),
    #[error("content can't be placed directly under the tag root; delete the tag with its contents or move them first")]
    RootContent,
    #[error("page {0} doesn't exist")]
    NoPage(usize),
    #[error("page {page}'s content can't be rewritten safely: {reason}")]
    PageNotEditable { page: usize, reason: String },
    #[error("no content is selected")]
    NothingSelected,
    #[error("the page has no content item {0}")]
    BadUnit(usize),
    #[error("content item {unit} can't be changed: {reason}")]
    UnitNotEditable { unit: usize, reason: String },
    #[error("the marked content {mcid} of this tag was not found on page {page}")]
    ContentNotFound { page: usize, mcid: i64 },
    #[error("the tag's content is inside a form XObject, which this tool can't rewrite")]
    FormContent,
    #[error("the tag holds an annotation, which can't become an artifact; move or delete its tag first")]
    Annotation,
    #[error("the tag has marked content without a page")]
    NoContentPage,
    #[error("other tagged content is nested inside marked content {0}")]
    NestedContent(i64),
    #[error("the structure tree root is not a dictionary")]
    BadRoot,
    #[error("the annotation is not on page {0}")]
    NotAnAnnotation(usize),
    #[error("the annotation already has a tag")]
    AlreadyTagged,
    #[error("the PDF has no form fields")]
    NoForm,
    #[error("the form has no untagged fields on its pages")]
    NoUntaggedFields,
    #[error("{0} form field(s) are referenced by an inline tag or by more than one tag, so they can't be linked safely; every other field has a tag")]
    FieldsNotLinkable(usize),
    #[error("the structure parent tree can't be updated: {0}")]
    ParentTree(String),
    #[error("the tag tree is too large to edit")]
    TooLarge,
    #[error("text is too long (maximum 65536 bytes)")]
    TooLong,
    #[error(transparent)]
    Cos(#[from] CosError),
}

fn pt_error(e: String) -> TaggingError {
    TaggingError::ParentTree(e)
}

// ---------------------------------------------------------------------------------------------
// Reading

/// The page's content units, annotations and numbered reading-order regions.
pub fn page_tagging(doc: &Document, page: usize) -> Result<PageTagging, TaggingError> {
    let pages = pdfcraft_model::pages(doc);
    let p = pages.get(page).ok_or(TaggingError::NoPage(page.saturating_add(1)))?;
    let tree = Tree::read(doc);
    let content = PageContent::read(doc, p);
    let units = content.units();
    Ok(describe(doc, &tree, p, &content, &units))
}

fn mark_of(tree: &Tree, content: &PageContent, page: ObjRef, unit: &Unit) -> UnitMark {
    match unit.source {
        Source::Sequence(s) => match content.sequences.get(s).map(|q| q.mark) {
            Some(Mark::Tagged(mcid)) => match tree.mcid_owners.get(&(page, mcid)).map(Vec::as_slice) {
                None | Some([]) => UnitMark::Orphan { mcid },
                Some([Owner::Element(tag)]) if tree.node(*tag).is_some() => UnitMark::Tagged { mcid, tag: *tag },
                _ => UnitMark::Unaddressable { mcid },
            },
            _ => UnitMark::Artifact,
        },
        Source::Atom(a) => {
            if content.atoms.get(a).is_some_and(|x| x.structured) {
                UnitMark::FormStructure
            } else {
                UnitMark::Untagged
            }
        }
    }
}

fn annotations(doc: &Document, tree: &Tree, page: &pdfcraft_model::Page) -> Vec<AnnotationItem> {
    let annots = page.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
    annots
        .iter()
        .filter_map(|a| {
            let object = a.as_ref()?;
            let d = doc.get(object).as_dict().cloned()?;
            let subtype = String::from_utf8_lossy(d.name(b"Subtype").unwrap_or(b"Annot")).into_owned();
            if subtype == "Popup" {
                return None;
            }
            let rect = d.get(b"Rect").map(|r| doc.resolve(r)).and_then(|r| {
                let v: Vec<f64> = r.as_array()?.iter().filter_map(Object::as_f64).collect();
                match v[..] {
                    [a, b, c, d] if v.iter().all(|x| x.is_finite()) => Some([a.min(c), b.min(d), a.max(c), b.max(d)]),
                    _ => None,
                }
            });
            Some(AnnotationItem { object, subtype, rect, tag: tree.object_owner(object).filter(|t| tree.node(*t).is_some()) })
        })
        .collect()
}

fn describe(doc: &Document, tree: &Tree, page: &pdfcraft_model::Page, content: &PageContent, units: &[Unit]) -> PageTagging {
    let listed: Vec<ContentUnit> = units
        .iter()
        .enumerate()
        .map(|(index, u)| ContentUnit {
            index,
            kind: u.kind,
            mark: mark_of(tree, content, page.obj, u),
            bbox: u.bbox,
            text: u.text.clone(),
            nested: u.nested,
        })
        .collect();
    let annots = annotations(doc, tree, page);
    let mut by_tag: HashMap<ObjRef, Vec<usize>> = HashMap::new();
    for u in &listed {
        if let UnitMark::Tagged { tag, .. } = u.mark {
            by_tag.entry(tag).or_default().push(u.index);
        }
    }
    let mut annots_by_tag: HashMap<ObjRef, Vec<&AnnotationItem>> = HashMap::new();
    for a in &annots {
        if let Some(tag) = a.tag {
            annots_by_tag.entry(tag).or_default().push(a);
        }
    }
    let mut regions = Vec::new();
    for node in &tree.nodes {
        let owned = by_tag.get(&node.object);
        let owned_annots = annots_by_tag.get(&node.object);
        if owned.is_none() && owned_annots.is_none() {
            continue;
        }
        let mut bbox: Option<[f64; 4]> = None;
        let mut text = String::new();
        for &u in owned.map(Vec::as_slice).unwrap_or_default() {
            if let Some(unit) = listed.get(u) {
                if let Some(b) = unit.bbox {
                    bbox = Some(bbox.map_or(b, |a| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]));
                }
                if text.len() < 200 && !unit.text.is_empty() {
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(&unit.text);
                }
            }
        }
        for a in owned_annots.map(Vec::as_slice).unwrap_or_default() {
            if let Some(b) = a.rect {
                bbox = Some(bbox.map_or(b, |x| [x[0].min(b[0]), x[1].min(b[1]), x[2].max(b[2]), x[3].max(b[3])]));
            }
        }
        regions.push(Region {
            order: regions.len().saturating_add(1),
            tag: node.object,
            kind: String::from_utf8_lossy(node.dict.name(b"S").unwrap_or_default()).into_owned(),
            bbox,
            units: owned.cloned().unwrap_or_default(),
            annotations: owned_annots.map(|v| v.iter().map(|a| a.object).collect()).unwrap_or_default(),
            text,
        });
    }
    PageTagging { units: listed, annotations: annots, regions, read_only: content.read_only.clone() }
}

/// Where a tag's content (and its descendants') is drawn on a page, in user space: what the
/// Tags panel highlights.
pub fn tag_boxes(doc: &Document, tag: ObjRef, page: usize) -> Vec<[f64; 4]> {
    let tree = Tree::read(doc);
    let below: HashSet<ObjRef> = tree.subtree(tag).into_iter().collect();
    if below.is_empty() {
        return Vec::new();
    }
    let Ok(listing) = page_tagging(doc, page) else { return Vec::new() };
    let mut boxes: Vec<[f64; 4]> = listing
        .units
        .iter()
        .filter_map(|u| match u.mark {
            UnitMark::Tagged { tag, .. } if below.contains(&tag) => u.bbox,
            _ => None,
        })
        .collect();
    boxes.extend(listing.annotations.iter().filter(|a| a.tag.is_some_and(|t| below.contains(&t))).filter_map(|a| a.rect));
    boxes
}

/// The tag whose content is drawn at a point of a page (user space), the smallest one first.
pub fn tag_at(doc: &Document, page: usize, x: f64, y: f64) -> Option<ObjRef> {
    let listing = page_tagging(doc, page).ok()?;
    let inside = |b: &[f64; 4]| b[0] <= x && x <= b[2] && b[1] <= y && y <= b[3];
    let area = |b: &[f64; 4]| (b[2] - b[0]).abs() * (b[3] - b[1]).abs();
    let units = listing.units.iter().filter_map(|u| match (u.mark, u.bbox) {
        (UnitMark::Tagged { tag, .. }, Some(b)) if inside(&b) => Some((area(&b), tag)),
        _ => None,
    });
    let annots = listing.annotations.iter().filter_map(|a| match (a.tag, a.rect) {
        (Some(tag), Some(b)) if inside(&b) => Some((area(&b), tag)),
        _ => None,
    });
    units.chain(annots).min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, tag)| tag)
}

// ---------------------------------------------------------------------------------------------
// Shared machinery

/// The structure tree root, made an indirect object with an indirect parent tree. With
/// `create`, an untagged document gets a new tree (a `/Document` element) and `/MarkInfo`.
fn ensure_structure(doc: &mut Document, create: bool) -> Result<ObjRef, TaggingError> {
    let catalog_ref = doc.root().ok_or(TaggingError::NoCatalog)?;
    let catalog = doc.get(catalog_ref).as_dict().cloned().ok_or(TaggingError::NoCatalog)?;
    let root = match catalog.get(b"StructTreeRoot") {
        Some(Object::Ref(r)) if doc.get(*r).as_dict().is_some() => *r,
        Some(Object::Dict(d)) => {
            // Nothing could name a root stored inside the catalog as its parent: once it is an
            // object of its own, its top tags do.
            let r = doc.add(Object::Dict(d.clone()));
            doc.update_dict(catalog_ref, |c| c.set(b"StructTreeRoot".to_vec(), Object::Ref(r)))?;
            let mut named = 0usize;
            for item in kid_items(doc, d.get(b"K")) {
                if let Some(top) = item.as_ref()
                    && doc.get(top).as_dict().is_some_and(|e| e.contains(b"S") && e.get(b"P").and_then(Object::as_ref) != Some(r))
                {
                    doc.update_dict(top, |e| e.set(b"P".to_vec(), Object::Ref(r)))?;
                    named = named.saturating_add(1);
                }
            }
            if named > 0 {
                doc.note_repair(format!(
                    "the structure tree root was stored inside the catalog; it is now an object of its own, and {named} top tag(s) name it as their parent"
                ));
            }
            r
        }
        Some(_) => return Err(TaggingError::BadRoot),
        None if !create => return Err(TaggingError::Untagged),
        None => {
            let root = doc.add(Object::Null);
            let mut document = Dict::new();
            document.set(b"Type".to_vec(), Object::name("StructElem"));
            document.set(b"S".to_vec(), Object::name("Document"));
            document.set(b"P".to_vec(), Object::Ref(root));
            document.set(b"K".to_vec(), Object::Array(Vec::new()));
            let document = doc.add(Object::Dict(document));
            let mut tree = Dict::new();
            tree.set(b"Nums".to_vec(), Object::Array(Vec::new()));
            let parent_tree = doc.add(Object::Dict(tree));
            let mut structure = Dict::new();
            structure.set(b"Type".to_vec(), Object::name("StructTreeRoot"));
            structure.set(b"K".to_vec(), Object::Array(vec![Object::Ref(document)]));
            structure.set(b"ParentTree".to_vec(), Object::Ref(parent_tree));
            structure.set(b"ParentTreeNextKey".to_vec(), Object::Int(0));
            doc.set(root, Object::Dict(structure));
            let mut mark_info = catalog.get(b"MarkInfo").map(|m| doc.resolve(m)).and_then(|m| m.as_dict().cloned()).unwrap_or_default();
            mark_info.set(b"Marked".to_vec(), Object::Bool(true));
            doc.update_dict(catalog_ref, |c| {
                c.set(b"StructTreeRoot".to_vec(), Object::Ref(root));
                c.set(b"MarkInfo".to_vec(), Object::Dict(mark_info));
            })?;
            root
        }
    };
    let dict = doc.get(root).as_dict().cloned().ok_or(TaggingError::Untagged)?;
    match dict.get(b"ParentTree") {
        Some(Object::Ref(r)) if doc.get(*r).as_dict().is_some() => {}
        Some(Object::Dict(d)) => {
            let r = doc.add(Object::Dict(d.clone()));
            doc.update_dict(root, |s| s.set(b"ParentTree".to_vec(), Object::Ref(r)))?;
        }
        None => {
            let mut tree = Dict::new();
            tree.set(b"Nums".to_vec(), Object::Array(Vec::new()));
            let r = doc.add(Object::Dict(tree));
            doc.update_dict(root, |s| s.set(b"ParentTree".to_vec(), Object::Ref(r)))?;
        }
        Some(_) => return Err(TaggingError::ParentTree("it is not a dictionary".into())),
    }
    Ok(root)
}

/// The parent tree, handing out no key a page or annotation already uses.
fn parent_tree(doc: &Document, root: ObjRef) -> Result<ParentTree, TaggingError> {
    let dict = doc.get(root).as_dict().cloned().ok_or(TaggingError::Untagged)?;
    let pt = dict.get(b"ParentTree").and_then(Object::as_ref).ok_or_else(|| pt_error("it is missing".into()))?;
    let mut tree = ParentTree::read(doc, pt, dict.get(b"ParentTreeNextKey").and_then(Object::as_int));
    tree.avoid(keys_in_use(doc));
    Ok(tree)
}

/// The parent-tree keys of the pages (`/StructParents`) and their annotations (`/StructParent`),
/// listed in the tree or not.
fn keys_in_use(doc: &Document) -> Vec<i64> {
    let mut keys = Vec::new();
    for page in pdfcraft_model::pages(doc) {
        keys.extend(page.dict.get(b"StructParents").and_then(Object::as_int));
        let annots = page.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        for annot in &annots {
            keys.extend(doc.resolve(annot).as_dict().and_then(|d| d.get(b"StructParent").and_then(Object::as_int)));
        }
    }
    keys
}

fn save_next_key(doc: &mut Document, root: ObjRef, pt: &ParentTree) -> Result<(), TaggingError> {
    let current = doc.get(root).as_dict().and_then(|d| d.int(b"ParentTreeNextKey"));
    if current != Some(pt.next_key) {
        let next = pt.next_key;
        doc.update_dict(root, |d| d.set(b"ParentTreeNextKey".to_vec(), Object::Int(next)))?;
    }
    Ok(())
}

fn read_tree(doc: &Document) -> Result<Tree, TaggingError> {
    let tree = Tree::read(doc);
    if tree.truncated {
        return Err(TaggingError::TooLarge);
    }
    Ok(tree)
}

fn editable(tree: &Tree, r: ObjRef) -> Result<&Node, TaggingError> {
    let node = tree.node(r).ok_or(TaggingError::NotATag(r))?;
    if node.shared {
        return Err(TaggingError::Shared(r));
    }
    if node.container == Container::Inline {
        return Err(TaggingError::InlineParent(r));
    }
    Ok(node)
}

/// Does a type name a standard structure type, directly or through the role map?
fn check_type(doc: &Document, kind: &str) -> Result<(), TaggingError> {
    let valid = !kind.is_empty() && kind.len() <= 127 && kind.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'.' | b'_' | b'-'));
    if !valid {
        return Err(TaggingError::InvalidType(kind.to_owned()));
    }
    if crate::structure::is_standard(kind.as_bytes()) {
        return Ok(());
    }
    let role_map =
        tree::root_of(doc).and_then(|(_, d)| d.get(b"RoleMap").map(|m| doc.resolve(m)).and_then(|m| m.as_dict().cloned())).unwrap_or_default();
    if crate::structure::is_standard(&crate::alt::standard(doc, &role_map, kind.as_bytes())) {
        Ok(())
    } else {
        Err(TaggingError::InvalidType(kind.to_owned()))
    }
}

fn container_ref(root: ObjRef, c: Container) -> Option<ObjRef> {
    match c {
        Container::Root => Some(root),
        Container::Element(r) => Some(r),
        Container::Inline => None,
    }
}

fn kids_raw(doc: &Document, root: ObjRef, c: Container) -> Result<Vec<Object>, TaggingError> {
    let holder = container_ref(root, c).ok_or(TaggingError::InlineParent(root))?;
    Ok(kid_items(doc, doc.get(holder).as_dict().and_then(|d| d.get(b"K"))))
}

fn set_kids(doc: &mut Document, root: ObjRef, c: Container, items: Vec<Object>) -> Result<(), TaggingError> {
    let holder = container_ref(root, c).ok_or(TaggingError::InlineParent(root))?;
    doc.update_dict(holder, |d| d.set(b"K".to_vec(), Object::Array(items)))?;
    Ok(())
}

fn position(items: &[Object], r: ObjRef) -> Result<usize, TaggingError> {
    let mut found = items.iter().enumerate().filter(|(_, o)| o.as_ref() == Some(r)).map(|(i, _)| i);
    match (found.next(), found.next()) {
        (Some(i), None) => Ok(i),
        (Some(_), Some(_)) => Err(TaggingError::Ambiguous(r)),
        (None, _) => Err(TaggingError::NotATag(r)),
    }
}

fn page_of(tree: &Tree, c: Container) -> Option<ObjRef> {
    match c {
        Container::Element(r) => tree.node(r).and_then(|n| n.page),
        _ => None,
    }
}

/// The container and index a destination names, as if `moving` were already out of the tree.
fn resolve_destination(
    tree: &Tree,
    doc: &Document,
    root: ObjRef,
    dest: Destination,
    moving: Option<ObjRef>,
) -> Result<(Container, usize), TaggingError> {
    let without = |c: Container| -> Result<Vec<Object>, TaggingError> {
        let mut items = kids_raw(doc, root, c)?;
        if let Some(m) = moving {
            items.retain(|o| o.as_ref() != Some(m));
        }
        Ok(items)
    };
    let (container, index) = match dest {
        Destination::Before(anchor) | Destination::After(anchor) => {
            if Some(anchor) == moving {
                return Err(TaggingError::AlreadyThere);
            }
            let node = editable(tree, anchor)?;
            let items = without(node.container)?;
            let at = position(&items, anchor)?;
            (node.container, if matches!(dest, Destination::After(_)) { at.saturating_add(1) } else { at })
        }
        Destination::Into(parent) | Destination::At(parent, _) => {
            let container = match parent {
                Some(p) => {
                    let node = tree.node(p).ok_or(TaggingError::NotATag(p))?;
                    if node.shared {
                        return Err(TaggingError::Shared(p));
                    }
                    Container::Element(p)
                }
                None => Container::Root,
            };
            let len = without(container)?.len();
            let index = match dest {
                Destination::At(_, i) if i > len => return Err(TaggingError::BadIndex(i)),
                Destination::At(_, i) => i,
                _ => len,
            };
            (container, index)
        }
    };
    if let (Some(m), Container::Element(p)) = (moving, container)
        && tree.is_within(p, m)
    {
        return Err(TaggingError::Cycle);
    }
    Ok((container, index))
}

fn insert_kid(doc: &mut Document, root: ObjRef, c: Container, index: usize, item: Object) -> Result<(), TaggingError> {
    let mut items = kids_raw(doc, root, c)?;
    if index > items.len() {
        return Err(TaggingError::BadIndex(index));
    }
    items.insert(index, item);
    set_kids(doc, root, c, items)
}

fn remove_kid(doc: &mut Document, root: ObjRef, c: Container, r: ObjRef) -> Result<usize, TaggingError> {
    let mut items = kids_raw(doc, root, c)?;
    let at = position(&items, r)?;
    items.remove(at);
    set_kids(doc, root, c, items)?;
    Ok(at)
}

fn element(kind: &str, parent: ObjRef, page: Option<ObjRef>, kids: Vec<Object>) -> Dict {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("StructElem"));
    d.set(b"S".to_vec(), Object::name(kind));
    d.set(b"P".to_vec(), Object::Ref(parent));
    if let Some(p) = page {
        d.set(b"Pg".to_vec(), Object::Ref(p));
    }
    d.set(b"K".to_vec(), Object::Array(kids));
    d
}

fn struct_parents(doc: &Document, page: ObjRef) -> Option<i64> {
    doc.get(page).as_dict().and_then(|d| d.get(b"StructParents").and_then(Object::as_int))
}

/// Everything below a tag, found by walking its `/K` (inline tags included).
#[derive(Default)]
struct Below {
    /// (page, MCID, form stream)
    mcids: Vec<(Option<ObjRef>, i64, Option<ObjRef>)>,
    objects: Vec<ObjRef>,
}

fn collect_below(doc: &Document, tree: &Tree, tag: ObjRef) -> Result<Below, TaggingError> {
    let mut below = Below::default();
    let mut seen = HashSet::new();
    let page = tree.node(tag).and_then(|n| n.page);
    let mut stack: Vec<(Object, Option<ObjRef>, usize)> = vec![(Object::Ref(tag), page, 0)];
    while let Some((item, page, depth)) = stack.pop() {
        if depth > tree::MAX_DEPTH || seen.len() > tree::MAX_ELEMENTS {
            return Err(TaggingError::TooLarge);
        }
        match classify(doc, &item, page) {
            Kid::Element(r) => {
                let node = tree.node(r).ok_or(TaggingError::NotATag(r))?;
                if node.shared || !seen.insert(r) {
                    return Err(TaggingError::Shared(r));
                }
                for k in kid_items(doc, node.dict.get(b"K")) {
                    stack.push((k, node.page, depth.saturating_add(1)));
                }
            }
            Kid::Inline => {
                let d = doc.resolve(&item).as_dict().cloned().unwrap_or_default();
                let page = d.get(b"Pg").and_then(Object::as_ref).or(page);
                for k in kid_items(doc, d.get(b"K")) {
                    stack.push((k, page, depth.saturating_add(1)));
                }
            }
            Kid::Mcid { mcid, page, stream } => below.mcids.push((page, mcid, stream)),
            Kid::Object { object, .. } => below.objects.push(object),
            Kid::Other => {}
        }
    }
    Ok(below)
}

/// Is a `/K` entry the marked content `mcid` of `page` (drawn directly in the page content)?
fn is_mcid(doc: &Document, item: &Object, owner_page: Option<ObjRef>, page: ObjRef, mcids: &HashSet<i64>) -> bool {
    matches!(classify(doc, item, owner_page), Kid::Mcid { mcid, page: Some(p), stream: None } if p == page && mcids.contains(&mcid))
}

/// Remove marked-content entries from a tag. Returns whether the tag is left with no children.
/// `owner_page` is the page the tag's integer MCIDs are on (its own or inherited `/Pg`).
fn remove_mcids(doc: &mut Document, owner: ObjRef, owner_page: Option<ObjRef>, page: ObjRef, mcids: &HashSet<i64>) -> Result<bool, TaggingError> {
    let dict = doc.get(owner).as_dict().cloned().ok_or(TaggingError::NotATag(owner))?;
    let items = kid_items(doc, dict.get(b"K"));
    let kept: Vec<Object> = items.iter().filter(|o| !is_mcid(doc, o, owner_page, page, mcids)).cloned().collect();
    let empty = kept.is_empty();
    if kept.len() != items.len() {
        doc.update_dict(owner, |d| d.set(b"K".to_vec(), Object::Array(kept)))?;
    }
    Ok(empty)
}

/// Remove tags left with no children, when they can be removed safely.
fn drop_empty(doc: &mut Document, root: ObjRef, emptied: &[ObjRef]) -> Result<(), TaggingError> {
    let tree = read_tree(doc)?;
    for &r in emptied {
        let Ok(node) = editable(&tree, r) else { continue };
        if !kid_items(doc, doc.get(r).as_dict().and_then(|d| d.get(b"K"))).is_empty() {
            continue;
        }
        remove_kid(doc, root, node.container, r)?;
    }
    Ok(())
}

/// Resolve a selection to unit indices, in content order.
fn select(units: &[Unit], selection: &Selection) -> Result<Vec<usize>, TaggingError> {
    let mut chosen: Vec<usize> = match selection {
        Selection::Units(list) => {
            for &u in list {
                if u >= units.len() {
                    return Err(TaggingError::BadUnit(u));
                }
            }
            list.clone()
        }
        Selection::Rect(r) => {
            let r = [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])];
            units
                .iter()
                .enumerate()
                .filter(|(_, u)| {
                    u.bbox.is_some_and(|b| {
                        let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
                        let centre = r[0] <= cx && cx <= r[2] && r[1] <= cy && cy <= r[3];
                        let w = (b[2].min(r[2]) - b[0].max(r[0])).max(0.0);
                        let h = (b[3].min(r[3]) - b[1].max(r[1])).max(0.0);
                        let area = (b[2] - b[0]) * (b[3] - b[1]);
                        centre || (area > 0.0 && w * h >= area * 0.5)
                    })
                })
                .map(|(i, _)| i)
                .collect()
        }
    };
    chosen.sort_unstable();
    chosen.dedup();
    if chosen.is_empty() {
        return Err(TaggingError::NothingSelected);
    }
    Ok(chosen)
}

fn page_index(pages: &[pdfcraft_model::Page]) -> HashMap<ObjRef, usize> {
    pages.iter().enumerate().map(|(i, p)| (p.obj, i)).collect()
}

const CELL_PARTS: &[&[u8]] = &[b"TR", b"TH", b"TD", b"THead", b"TBody", b"TFoot", b"LI", b"Lbl", b"LBody"];

/// Climb out of table and list internals, so new content goes next to the table or list.
fn climb_cells(doc: &Document, tree: &Tree, r: ObjRef) -> ObjRef {
    let role_map =
        tree::root_of(doc).and_then(|(_, d)| d.get(b"RoleMap").map(|m| doc.resolve(m)).and_then(|m| m.as_dict().cloned())).unwrap_or_default();
    let mut at = r;
    for _ in 0..tree::MAX_DEPTH {
        let Some(node) = tree.node(at) else { break };
        let kind = crate::alt::standard(doc, &role_map, node.dict.name(b"S").unwrap_or_default());
        match node.container {
            Container::Element(parent) if CELL_PARTS.contains(&kind.as_slice()) => at = parent,
            _ => break,
        }
    }
    at
}

/// Climb to the child of the top element (or of the root) that holds `r`.
fn climb_block(tree: &Tree, r: ObjRef) -> ObjRef {
    let top = tree.top();
    let mut at = r;
    for _ in 0..tree::MAX_DEPTH {
        match tree.node(at).map(|n| n.container) {
            Some(Container::Element(parent)) if Some(parent) != top => at = parent,
            _ => break,
        }
    }
    at
}

/// Where new structure for content on `page` goes when the caller doesn't say: next to the
/// tags of neighbouring content on the page, else after the structure of earlier pages, else
/// into the top element.
fn auto_destination(doc: &Document, tree: &Tree, pages: &[pdfcraft_model::Page], page: usize, marks: &[UnitMark], chosen: &[usize]) -> Destination {
    let usable = |r: ObjRef| editable(tree, r).is_ok();
    let owner = |u: usize| match marks.get(u) {
        Some(UnitMark::Tagged { tag, .. }) => Some(*tag),
        _ => None,
    };
    for &u in chosen {
        if let Some(tag) = owner(u) {
            let at = climb_cells(doc, tree, tag);
            if usable(at) {
                return Destination::Before(at);
            }
        }
    }
    let first = chosen.first().copied().unwrap_or(0);
    let picked: HashSet<usize> = chosen.iter().copied().collect();
    if let Some(tag) = (0..first).rev().filter(|u| !picked.contains(u)).filter_map(owner).map(|t| climb_cells(doc, tree, t)).find(|t| usable(*t)) {
        return Destination::After(tag);
    }
    if let Some(tag) = (first..marks.len()).filter(|u| !picked.contains(u)).filter_map(owner).map(|t| climb_cells(doc, tree, t)).find(|t| usable(*t))
    {
        return Destination::Before(tag);
    }
    let index = page_index(pages);
    let page_of = |n: &Node| n.page.and_then(|p| index.get(&p).copied());
    if let Some(node) = tree.nodes.iter().rev().find(|n| page_of(n).is_some_and(|p| p < page) && n.container != Container::Inline) {
        let at = climb_block(tree, node.object);
        if usable(at) {
            return Destination::After(at);
        }
    }
    if let Some(node) = tree.nodes.iter().find(|n| page_of(n).is_some_and(|p| p > page) && n.container != Container::Inline) {
        let at = climb_block(tree, node.object);
        if usable(at) {
            return Destination::Before(at);
        }
    }
    Destination::Into(tree.top())
}

// ---------------------------------------------------------------------------------------------
// Tags panel

/// New tag: an empty tag of `kind` at `dest`. An untagged document gets a tag tree first.
pub fn new_tag(doc: &mut Document, kind: &str, dest: Destination) -> Result<ObjRef, TaggingError> {
    check_type(doc, kind)?;
    let mut next = doc.clone();
    let root = ensure_structure(&mut next, true)?;
    let tree = read_tree(&next)?;
    let (container, index) = resolve_destination(&tree, &next, root, dest, None)?;
    let parent = container_ref(root, container).ok_or(TaggingError::InlineParent(root))?;
    let tag = next.add(Object::Dict(element(kind, parent, None, Vec::new())));
    insert_kid(&mut next, root, container, index, Object::Ref(tag))?;
    *doc = next;
    Ok(tag)
}

/// Move a tag (with everything below it) to another place in the tree, under any parent.
/// Content and parent-tree entries stay; the tag keeps the page it had.
pub fn move_tag_to(doc: &mut Document, tag: ObjRef, dest: Destination) -> Result<(), TaggingError> {
    let mut next = doc.clone();
    let root = ensure_structure(&mut next, false)?;
    let tree = read_tree(&next)?;
    let node = editable(&tree, tag)?.clone();
    let (to, index) = resolve_destination(&tree, &next, root, dest, Some(tag))?;
    let from = node.container;
    let old = position(&kids_raw(&next, root, from)?, tag)?;
    if from == to && old == index {
        return Err(TaggingError::AlreadyThere);
    }
    remove_kid(&mut next, root, from, tag)?;
    insert_kid(&mut next, root, to, index, Object::Ref(tag))?;
    let parent = container_ref(root, to).ok_or(TaggingError::InlineParent(root))?;
    let inherited = page_of(&tree, to);
    next.update_dict(tag, |d| {
        d.set(b"P".to_vec(), Object::Ref(parent));
        // Keep the page the tag (and what inherits from it) was on.
        if !d.contains(b"Pg")
            && let Some(page) = node.page
            && inherited != Some(page)
        {
            d.set(b"Pg".to_vec(), Object::Ref(page));
        }
    })?;
    *doc = next;
    Ok(())
}

/// Delete a tag. With `keep_contents`, its children (tags and content) move up into its parent
/// in its place; otherwise everything below it leaves the tree and its content becomes
/// untagged (the page still draws it, and it can be tagged again).
pub fn delete_tag(doc: &mut Document, tag: ObjRef, keep_contents: bool) -> Result<(), TaggingError> {
    let mut next = doc.clone();
    let root = ensure_structure(&mut next, false)?;
    let tree = read_tree(&next)?;
    let node = editable(&tree, tag)?.clone();
    let container = node.container;
    let mut items = kids_raw(&next, root, container)?;
    let at = position(&items, tag)?;
    let mut pt = parent_tree(&next, root)?;
    if !keep_contents {
        let below = collect_below(&next, &tree, tag)?;
        let mut by_key: BTreeMap<i64, Vec<(i64, Object)>> = BTreeMap::new();
        for (page, mcid, stream) in below.mcids {
            if let Some(key) = stream.or(page).and_then(|holder| struct_parents(&next, holder)) {
                by_key.entry(key).or_default().push((mcid, Object::Null));
            }
        }
        for (key, changes) in by_key {
            pt.set_array(&mut next, key, &changes).map_err(pt_error)?;
        }
        for object in below.objects {
            if let Some(key) = next.get(object).as_dict().and_then(|d| d.int(b"StructParent")) {
                pt.set_single(&mut next, key, Object::Null).map_err(pt_error)?;
                next.update_dict(object, |d| {
                    d.remove(b"StructParent");
                })?;
            }
        }
        items.remove(at);
        set_kids(&mut next, root, container, items)?;
        *doc = next;
        return Ok(());
    }
    let Container::Element(parent) = container else {
        // The root lists only tags: lifting is possible only when the tag has nothing else.
        let lifted = kid_items(&next, node.dict.get(b"K"));
        if lifted.iter().any(|k| !matches!(classify(&next, k, node.page), Kid::Element(_) | Kid::Inline | Kid::Other)) {
            return Err(TaggingError::RootContent);
        }
        for k in &lifted {
            if let Kid::Element(child) = classify(&next, k, node.page) {
                lift_element(&mut next, &tree, child, root, node.page, None)?;
            }
        }
        items.splice(at..=at, lifted);
        set_kids(&mut next, root, container, items)?;
        *doc = next;
        return Ok(());
    };
    let new_page = page_of(&tree, container);
    let mut lifted = Vec::new();
    let mut by_key: BTreeMap<i64, Vec<(i64, Object)>> = BTreeMap::new();
    for item in kid_items(&next, node.dict.get(b"K")) {
        match classify(&next, &item, node.page) {
            Kid::Element(child) => {
                lift_element(&mut next, &tree, child, parent, node.page, new_page)?;
                lifted.push(item);
            }
            Kid::Inline => {
                let mut d = next.resolve(&item).as_dict().cloned().unwrap_or_default();
                if d.contains(b"P") {
                    d.set(b"P".to_vec(), Object::Ref(parent));
                }
                if !d.contains(b"Pg")
                    && let Some(p) = node.page
                    && new_page != Some(p)
                {
                    d.set(b"Pg".to_vec(), Object::Ref(p));
                }
                lifted.push(Object::Dict(d));
            }
            Kid::Mcid { mcid, page, stream } => {
                let page = page.ok_or(TaggingError::NoContentPage)?;
                let key = stream.or(Some(page)).and_then(|holder| struct_parents(&next, holder));
                if let Some(key) = key {
                    by_key.entry(key).or_default().push((mcid, Object::Ref(parent)));
                }
                lifted.push(with_page(&mut next, item, page, new_page, Some(mcid))?);
            }
            Kid::Object { object, page } => {
                if let Some(key) = next.get(object).as_dict().and_then(|d| d.int(b"StructParent")) {
                    pt.set_single(&mut next, key, Object::Ref(parent)).map_err(pt_error)?;
                }
                lifted.push(match page {
                    Some(p) => with_page(&mut next, item, p, new_page, None)?,
                    None => item,
                });
            }
            Kid::Other => lifted.push(item),
        }
    }
    for (key, changes) in by_key {
        pt.set_array(&mut next, key, &changes).map_err(pt_error)?;
    }
    items.splice(at..=at, lifted);
    set_kids(&mut next, root, container, items)?;
    save_next_key(&mut next, root, &pt)?;
    *doc = next;
    Ok(())
}

/// A child tag moving up a level: its `/P` changes and it keeps the page it inherited.
fn lift_element(
    doc: &mut Document,
    tree: &Tree,
    child: ObjRef,
    parent: ObjRef,
    page: Option<ObjRef>,
    new_page: Option<ObjRef>,
) -> Result<(), TaggingError> {
    let node = tree.node(child).ok_or(TaggingError::NotATag(child))?;
    if node.shared {
        return Err(TaggingError::Shared(child));
    }
    doc.update_dict(child, |d| {
        d.set(b"P".to_vec(), Object::Ref(parent));
        if !d.contains(b"Pg")
            && let Some(p) = page
            && new_page != Some(p)
        {
            d.set(b"Pg".to_vec(), Object::Ref(p));
        }
    })?;
    Ok(())
}

/// A content item moving to a tag on `new_page`: an integer MCID becomes a marked-content
/// reference naming its page, and a reference without `/Pg` gets one.
fn with_page(doc: &mut Document, item: Object, page: ObjRef, new_page: Option<ObjRef>, mcid: Option<i64>) -> Result<Object, TaggingError> {
    if new_page == Some(page) {
        return Ok(item);
    }
    match item {
        Object::Int(m) => {
            let mut mcr = Dict::new();
            mcr.set(b"Type".to_vec(), Object::name("MCR"));
            mcr.set(b"Pg".to_vec(), Object::Ref(page));
            mcr.set(b"MCID".to_vec(), Object::Int(mcid.unwrap_or(m)));
            Ok(Object::Dict(mcr))
        }
        Object::Dict(mut d) => {
            if !d.contains(b"Pg") {
                d.set(b"Pg".to_vec(), Object::Ref(page));
            }
            Ok(Object::Dict(d))
        }
        Object::Ref(r) => {
            if doc.get(r).as_dict().is_some_and(|d| !d.contains(b"Pg")) {
                doc.update_dict(r, |d| d.set(b"Pg".to_vec(), Object::Ref(page)))?;
            }
            Ok(Object::Ref(r))
        }
        other => Ok(other),
    }
}

/// Change Tag to Artifact: every piece of marked content below the tag becomes an artifact on
/// its page and the tag leaves the tree. Returns how many marked-content sequences changed.
pub fn tag_to_artifact(doc: &mut Document, tag: ObjRef) -> Result<usize, TaggingError> {
    let mut next = doc.clone();
    let root = ensure_structure(&mut next, false)?;
    let tree = read_tree(&next)?;
    let node = editable(&tree, tag)?.clone();
    let below = collect_below(&next, &tree, tag)?;
    if !below.objects.is_empty() {
        return Err(TaggingError::Annotation);
    }
    let mut by_page: BTreeMap<ObjRef, HashSet<i64>> = BTreeMap::new();
    for (page, mcid, stream) in below.mcids {
        if stream.is_some() {
            return Err(TaggingError::FormContent);
        }
        by_page.entry(page.ok_or(TaggingError::NoContentPage)?).or_default().insert(mcid);
    }
    let pages = pdfcraft_model::pages(&next);
    let index = page_index(&pages);
    let uses = content::stream_uses(&next, &pages);
    let mut pt = parent_tree(&next, root)?;
    let mut changed = 0usize;
    for (page_ref, mcids) in by_page {
        let page_no = index.get(&page_ref).copied().ok_or(TaggingError::NoContentPage)?;
        let page = pages.get(page_no).ok_or(TaggingError::NoContentPage)?;
        let content = PageContent::read(&next, page);
        if let Some(reason) = &content.read_only {
            return Err(TaggingError::PageNotEditable { page: page_no.saturating_add(1), reason: reason.clone() });
        }
        let converting: HashSet<usize> =
            content.sequences.iter().enumerate().filter(|(_, s)| matches!(s.mark, Mark::Tagged(m) if mcids.contains(&m))).map(|(i, _)| i).collect();
        for &m in &mcids {
            if !content.sequences.iter().any(|s| s.mark == Mark::Tagged(m)) {
                return Err(TaggingError::ContentNotFound { page: page_no.saturating_add(1), mcid: m });
            }
        }
        // Tagged content of other tags inside what becomes an artifact would vanish from reading.
        for seq in &content.sequences {
            if let Mark::Tagged(m) = seq.mark
                && !mcids.contains(&m)
                && ancestors(&content, seq.parent).any(|a| converting.contains(&a))
            {
                return Err(TaggingError::NestedContent(m));
            }
        }
        let mut edits = Vec::new();
        for &s in &converting {
            if let Some(seq) = content.sequences.get(s)
                && let Some(edit) = content.replace(seq.open, b"/Artifact BMC".to_vec())
            {
                edits.push(edit);
            }
        }
        changed = changed.saturating_add(edits.len());
        content.commit(&mut next, edits, &uses).map_err(|reason| TaggingError::PageNotEditable { page: page_no.saturating_add(1), reason })?;
        if let Some(key) = struct_parents(&next, page_ref) {
            let changes: Vec<(i64, Object)> = mcids.iter().map(|m| (*m, Object::Null)).collect();
            pt.set_array(&mut next, key, &changes).map_err(pt_error)?;
        }
    }
    remove_kid(&mut next, root, node.container, tag)?;
    *doc = next;
    Ok(changed)
}

fn ancestors(content: &PageContent, start: Option<usize>) -> impl Iterator<Item = usize> + '_ {
    let mut at = start;
    let mut steps = 0usize;
    std::iter::from_fn(move || {
        let s = at?;
        steps = steps.saturating_add(1);
        if steps > content.sequences.len() {
            return None;
        }
        at = content.sequences.get(s).and_then(|q| q.parent);
        Some(s)
    })
}

/// Tag an annotation: a new `/Link`, `/Form` (widgets) or `/Annot` tag that references it, with
/// its parent-tree entry.
pub fn tag_annotation(doc: &mut Document, page: usize, annotation: ObjRef, dest: Option<Destination>) -> Result<ObjRef, TaggingError> {
    let mut next = doc.clone();
    let root = ensure_structure(&mut next, true)?;
    let tree = read_tree(&next)?;
    let pages = pdfcraft_model::pages(&next);
    let p = pages.get(page).ok_or(TaggingError::NoPage(page.saturating_add(1)))?;
    let listed = annotations(&next, &tree, p);
    let item = listed.iter().find(|a| a.object == annotation).ok_or(TaggingError::NotAnAnnotation(page.saturating_add(1)))?;
    if tree.object_owners.contains_key(&annotation) {
        return Err(TaggingError::AlreadyTagged);
    }
    let kind = match item.subtype.as_str() {
        "Link" => "Link",
        "Widget" => "Form",
        _ => "Annot",
    };
    let dest = match dest {
        Some(d) => d,
        None => after_page_content(&next, &tree, p).unwrap_or_else(|| auto_destination(&next, &tree, &pages, page, &[], &[])),
    };
    let (container, index) = resolve_destination(&tree, &next, root, dest, None)?;
    let parent = container_ref(root, container).ok_or(TaggingError::InlineParent(root))?;
    let mut pt = parent_tree(&next, root)?;
    let key = pt.allocate().map_err(pt_error)?;
    let tag = annotation_tag(&mut next, kind, parent, p.obj, annotation, key)?;
    pt.set_single(&mut next, key, Object::Ref(tag)).map_err(pt_error)?;
    insert_kid(&mut next, root, container, index, Object::Ref(tag))?;
    save_next_key(&mut next, root, &pt)?;
    *doc = next;
    Ok(tag)
}

/// Just after the last tag that owns content on the page (out of table and list internals), when
/// that tag can be edited: where a new annotation tag goes by default.
fn after_page_content(doc: &Document, tree: &Tree, page: &pdfcraft_model::Page) -> Option<Destination> {
    let content = PageContent::read(doc, page);
    let listing = describe(doc, tree, page, &content, &content.units());
    listing.regions.last().map(|r| climb_cells(doc, tree, r.tag)).filter(|t| editable(tree, *t).is_ok()).map(Destination::After)
}

/// A new tag of `kind` under `parent` holding an object reference to `annotation` on `page`,
/// with the annotation's `/StructParent` set to `key`. The caller lists the tag in its parent
/// and points `key` at it in the parent tree.
fn annotation_tag(doc: &mut Document, kind: &str, parent: ObjRef, page: ObjRef, annotation: ObjRef, key: i64) -> Result<ObjRef, TaggingError> {
    let mut objr = Dict::new();
    objr.set(b"Type".to_vec(), Object::name("OBJR"));
    objr.set(b"Obj".to_vec(), Object::Ref(annotation));
    objr.set(b"Pg".to_vec(), Object::Ref(page));
    let tag = doc.add(Object::Dict(element(kind, parent, Some(page), vec![Object::Dict(objr)])));
    doc.update_dict(annotation, |d| d.set(b"StructParent".to_vec(), Object::Int(key)))?;
    Ok(tag)
}

// ---------------------------------------------------------------------------------------------
// Reading Order tool

/// The structure a selection becomes.
enum Plan {
    /// A tag owning the units directly.
    Leaf { kind: &'static str, units: Vec<usize> },
    /// A tag holding other tags.
    Group { kind: &'static str, children: Vec<Plan> },
}

/// A row's vertical band and its units with their boxes.
type Row = ([f64; 2], Vec<(usize, [f64; 4])>);

/// Group units into rows (top to bottom) and, in each row, cells split at wide gaps.
fn rows_and_cells(units: &[Unit], chosen: &[usize]) -> Vec<Vec<Vec<usize>>> {
    let mut placed: Vec<(usize, [f64; 4])> = chosen.iter().filter_map(|&u| units.get(u).and_then(|x| x.bbox).map(|b| (u, b))).collect();
    let unplaced: Vec<usize> = chosen.iter().copied().filter(|&u| units.get(u).and_then(|x| x.bbox).is_none()).collect();
    placed.sort_by(|a, b| b.1[3].total_cmp(&a.1[3]).then(a.1[0].total_cmp(&b.1[0])));
    let mut rows: Vec<Row> = Vec::new();
    for (u, b) in placed {
        let centre = (b[1] + b[3]) / 2.0;
        match rows.iter_mut().find(|(band, _)| band[0] <= centre && centre <= band[1]) {
            Some((band, members)) => {
                band[0] = band[0].min(b[1]);
                band[1] = band[1].max(b[3]);
                members.push((u, b));
            }
            None => rows.push(([b[1], b[3]], vec![(u, b)])),
        }
    }
    let mut out: Vec<Vec<Vec<usize>>> = rows
        .into_iter()
        .map(|(band, mut members)| {
            members.sort_by(|a, b| a.1[0].total_cmp(&b.1[0]));
            let gap = ((band[1] - band[0]) * 0.8).max(4.0);
            let mut cells: Vec<(f64, Vec<usize>)> = Vec::new();
            for (u, b) in members {
                match cells.last_mut() {
                    Some((right, cell)) if b[0] - *right <= gap => {
                        *right = right.max(b[2]);
                        cell.push(u);
                    }
                    _ => cells.push((b[2], vec![u])),
                }
            }
            cells
                .into_iter()
                .map(|(_, mut cell)| {
                    cell.sort_unstable_by_key(|&u| units.get(u).map_or(0, |x| x.first));
                    cell
                })
                .collect()
        })
        .collect();
    if !unplaced.is_empty() {
        out.push(vec![unplaced]);
    }
    out
}

fn is_label(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() || t.chars().count() > 4 {
        return false;
    }
    let bullets = ['•', '◦', '▪', '‣', '–', '-', '*', '·', '○', '■', '□', '►', '✓', '➢'];
    if t.chars().count() == 1 && t.chars().all(|c| bullets.contains(&c)) {
        return true;
    }
    let body = t.trim_start_matches('(').trim_end_matches(['.', ')']);
    t != body.trim() && !body.is_empty() && body.chars().all(|c| c.is_ascii_alphanumeric())
}

fn plan_for(kind: &str, units: &[Unit], chosen: &[usize]) -> Plan {
    match kind {
        "Table" => Plan::Group {
            kind: "Table",
            children: rows_and_cells(units, chosen)
                .into_iter()
                .map(|row| Plan::Group { kind: "TR", children: row.into_iter().map(|cell| Plan::Leaf { kind: "TD", units: cell }).collect() })
                .collect(),
        },
        "L" => Plan::Group {
            kind: "L",
            children: rows_and_cells(units, chosen)
                .into_iter()
                .map(|row| {
                    let label = row.len() > 1
                        && row.first().is_some_and(|c| c.len() == 1 && c.iter().all(|&u| units.get(u).is_some_and(|x| is_label(&x.text))));
                    let mut cells = row.into_iter();
                    let mut children = Vec::new();
                    if label && let Some(first) = cells.next() {
                        children.push(Plan::Leaf { kind: "Lbl", units: first });
                    }
                    let mut body: Vec<usize> = cells.flatten().collect();
                    body.sort_unstable_by_key(|&u| units.get(u).map_or(0, |x| x.first));
                    if !body.is_empty() {
                        children.push(Plan::Leaf { kind: "LBody", units: body });
                    }
                    Plan::Group { kind: "LI", children }
                })
                .collect(),
        },
        _ => Plan::Leaf { kind: "", units: chosen.to_vec() },
    }
}

/// Everything one tagging operation changes, gathered before anything is written.
struct Tagging<'a> {
    content: &'a PageContent,
    units: &'a [Unit],
    page: ObjRef,
    next_mcid: i64,
    edits: Vec<(std::ops::Range<usize>, Vec<u8>)>,
    /// (tag, MCID) parent-tree entries.
    entries: Vec<(i64, ObjRef)>,
    /// Owner → MCIDs it gives up.
    taken: BTreeMap<ObjRef, HashSet<i64>>,
}

impl Tagging<'_> {
    fn fresh(&mut self) -> Result<i64, TaggingError> {
        let m = self.next_mcid;
        if m > parent_tree::MAX_MCID {
            return Err(TaggingError::ParentTree("the page has too many marked-content ids".into()));
        }
        self.next_mcid = m.saturating_add(1);
        Ok(m)
    }

    /// Mark `units` as content of `tag` (structure type `kind`); returns its MCIDs in content order.
    fn take(&mut self, tree: &Tree, tag: ObjRef, kind: &str, units: &[usize]) -> Result<Vec<i64>, TaggingError> {
        let mut items: Vec<(usize, i64)> = Vec::new();
        let mut atoms: Vec<usize> = Vec::new();
        for &u in units {
            let unit = self.units.get(u).ok_or(TaggingError::BadUnit(u))?;
            match unit.source {
                Source::Sequence(s) => {
                    let seq = self.content.sequences.get(s).ok_or(TaggingError::BadUnit(u))?;
                    match seq.mark {
                        Mark::Tagged(m) => {
                            if let Some(owner) = tree.mcid_owner(self.page, m) {
                                self.taken.entry(owner).or_default().insert(m);
                            }
                            items.push((unit.first, m));
                        }
                        _ => {
                            let m = self.fresh()?;
                            let edit = self.content.replace(seq.open, content::open_tagged(kind, m)).ok_or(TaggingError::BadUnit(u))?;
                            self.edits.push(edit);
                            items.push((unit.first, m));
                        }
                    }
                }
                Source::Atom(a) => atoms.push(a),
            }
        }
        atoms.sort_unstable_by_key(|&a| self.content.atoms.get(a).map_or(0, |x| x.first));
        let runs = self.content.runs(&atoms).map_err(|reason| TaggingError::UnitNotEditable { unit: units.first().copied().unwrap_or(0), reason })?;
        for (first, last) in runs {
            let m = self.fresh()?;
            let open = self.content.before(first, content::open_tagged(kind, m)).ok_or(TaggingError::NothingSelected)?;
            let close = self.content.after(last, content::close()).ok_or(TaggingError::NothingSelected)?;
            self.edits.push(open);
            self.edits.push(close);
            items.push((first, m));
        }
        items.sort_unstable();
        let mcids: Vec<i64> = items.into_iter().map(|(_, m)| m).collect();
        self.entries.extend(mcids.iter().map(|m| (*m, tag)));
        Ok(mcids)
    }

    /// Write `plan` as tags under `parent`; returns the top tag.
    fn build(&mut self, doc: &mut Document, tree: &Tree, plan: &Plan, kind: &str, parent: ObjRef, alt: Option<&str>) -> Result<ObjRef, TaggingError> {
        let tag = doc.add(Object::Null);
        match plan {
            Plan::Leaf { kind: k, units } => {
                let kind = if k.is_empty() { kind } else { k };
                let mcids = self.take(tree, tag, kind, units)?;
                let mut d = element(kind, parent, Some(self.page), mcids.into_iter().map(Object::Int).collect());
                if let Some(alt) = alt {
                    d.set(b"Alt".to_vec(), Object::String(PdfString::text(alt)));
                }
                doc.set(tag, Object::Dict(d));
            }
            Plan::Group { kind: k, children } => {
                let mut kids = Vec::with_capacity(children.len());
                for child in children {
                    kids.push(Object::Ref(self.build(doc, tree, child, kind, tag, None)?));
                }
                let mut d = element(k, parent, Some(self.page), kids);
                if let Some(alt) = alt {
                    d.set(b"Alt".to_vec(), Object::String(PdfString::text(alt)));
                }
                doc.set(tag, Object::Dict(d));
            }
        }
        Ok(tag)
    }
}

/// Tag the selected content of a page as `kind` (`P`, `H1`…`H6`, `Figure`, `Table`, `L` or any
/// type the role map defines): Reading Order tool and Tags panel ▸ Create Tag from Selection.
///
/// Untagged content is wrapped in new marked content, artifacts become tagged content, and
/// tagged content moves to the new tag from the tags that owned it (a tag left empty is
/// removed). `Table` builds rows and cells and `L` list items from the layout, for review.
/// `alt` sets alternate text (for a `Figure`). Without `dest`, the new tag goes next to the
/// tags of neighbouring content.
pub fn tag_selection(
    doc: &mut Document,
    page: usize,
    selection: &Selection,
    kind: &str,
    alt: Option<&str>,
    dest: Option<Destination>,
) -> Result<ObjRef, TaggingError> {
    check_type(doc, kind)?;
    let alt = alt.map(str::trim).filter(|a| !a.is_empty());
    if alt.is_some_and(|a| a.len() > 65_536) {
        return Err(TaggingError::TooLong);
    }
    let mut next = doc.clone();
    let root = ensure_structure(&mut next, true)?;
    let tree = read_tree(&next)?;
    let pages = pdfcraft_model::pages(&next);
    let p = pages.get(page).ok_or(TaggingError::NoPage(page.saturating_add(1)))?;
    let content = PageContent::read(&next, p);
    if let Some(reason) = &content.read_only {
        return Err(TaggingError::PageNotEditable { page: page.saturating_add(1), reason: reason.clone() });
    }
    let units = content.units();
    let chosen = select(&units, selection)?;
    let marks: Vec<UnitMark> = units.iter().map(|u| mark_of(&tree, &content, p.obj, u)).collect();
    for &u in &chosen {
        let unit = units.get(u).ok_or(TaggingError::BadUnit(u))?;
        let refuse = |reason: &str| Err(TaggingError::UnitNotEditable { unit: u, reason: reason.into() });
        if unit.nested {
            return refuse("other tagged content is nested inside it");
        }
        match marks.get(u) {
            Some(UnitMark::Tagged { tag, .. }) => {
                if let Err(e) = editable(&tree, *tag) {
                    return refuse(&e.to_string());
                }
            }
            Some(UnitMark::Unaddressable { .. }) => return refuse("its tag is inline or listed more than once"),
            Some(UnitMark::FormStructure) => return refuse("it is a form XObject with structure of its own"),
            _ => {}
        }
    }
    let dest = dest.unwrap_or_else(|| auto_destination(&next, &tree, &pages, page, &marks, &chosen));
    let (container, index) = resolve_destination(&tree, &next, root, dest, None)?;
    let parent = container_ref(root, container).ok_or(TaggingError::InlineParent(root))?;
    let mut pt = parent_tree(&next, root)?;
    let key = match struct_parents(&next, p.obj) {
        Some(k) => k,
        None => {
            let k = pt.allocate().map_err(pt_error)?;
            next.update_dict(p.obj, |d| d.set(b"StructParents".to_vec(), Object::Int(k)))?;
            k
        }
    };
    // A page whose key maps nothing gets its entry rebuilt from the tags that own its content.
    let mut rebuilt: Vec<(i64, ObjRef)> = Vec::new();
    if pt.get(&next, key).is_none() {
        rebuilt = tree.mcid_owners.keys().filter(|(pg, _)| *pg == p.obj).filter_map(|(pg, m)| tree.mcid_owner(*pg, *m).map(|t| (*m, t))).collect();
        rebuilt.sort_unstable();
        if !rebuilt.is_empty() {
            next.note_repair(format!(
                "the structure parent tree had no entry for page {} (key {key}); it was rebuilt from the {} tagged item(s) the tags list",
                page.saturating_add(1),
                rebuilt.len()
            ));
        }
    }
    let in_tree = tree.mcid_owners.keys().filter(|(pg, _)| *pg == p.obj).map(|(_, m)| *m).max();
    let in_pt = match pt.get(&next, key) {
        Some(parent_tree::Value::Array(a)) => i64::try_from(a.len()).ok().map(|n| n.saturating_sub(1)),
        _ => None,
    };
    let next_mcid = [content.max_mcid, in_tree, in_pt].into_iter().flatten().max().map_or(0, |m| m.saturating_add(1)).max(0);
    let mut work =
        Tagging { content: &content, units: &units, page: p.obj, next_mcid, edits: Vec::new(), entries: Vec::new(), taken: BTreeMap::new() };
    let plan = plan_for(kind, &units, &chosen);
    let tag = work.build(&mut next, &tree, &plan, kind, parent, alt)?;
    insert_kid(&mut next, root, container, index, Object::Ref(tag))?;
    let mut emptied = Vec::new();
    for (owner, mcids) in &work.taken {
        if remove_mcids(&mut next, *owner, tree.node(*owner).and_then(|n| n.page), p.obj, mcids)? {
            emptied.push(*owner);
        }
    }
    drop_empty(&mut next, root, &emptied)?;
    let taken: HashSet<i64> = work.taken.values().flatten().copied().collect();
    let changes: Vec<(i64, Object)> =
        rebuilt.iter().filter(|(m, _)| !taken.contains(m)).chain(work.entries.iter()).map(|(m, t)| (*m, Object::Ref(*t))).collect();
    pt.set_array(&mut next, key, &changes).map_err(pt_error)?;
    save_next_key(&mut next, root, &pt)?;
    let uses = content::stream_uses(&next, &pages);
    content.commit(&mut next, work.edits, &uses).map_err(|reason| TaggingError::PageNotEditable { page: page.saturating_add(1), reason })?;
    *doc = next;
    Ok(tag)
}

/// Mark the selected content of a page as an artifact (Reading Order tool ▸ Background/Artifact).
/// Tagged content leaves its tag (a tag left empty is removed). Returns how many units changed.
pub fn content_to_artifact(doc: &mut Document, page: usize, selection: &Selection) -> Result<usize, TaggingError> {
    let mut next = doc.clone();
    let tree = read_tree(&next)?;
    let pages = pdfcraft_model::pages(&next);
    let p = pages.get(page).ok_or(TaggingError::NoPage(page.saturating_add(1)))?;
    let content = PageContent::read(&next, p);
    if let Some(reason) = &content.read_only {
        return Err(TaggingError::PageNotEditable { page: page.saturating_add(1), reason: reason.clone() });
    }
    let units = content.units();
    let chosen = select(&units, selection)?;
    let mut edits = Vec::new();
    let mut atoms = Vec::new();
    let mut taken: BTreeMap<ObjRef, HashSet<i64>> = BTreeMap::new();
    let mut nulls: Vec<(i64, Object)> = Vec::new();
    let mut changed = 0usize;
    for &u in &chosen {
        let unit = units.get(u).ok_or(TaggingError::BadUnit(u))?;
        let refuse = |reason: &str| Err(TaggingError::UnitNotEditable { unit: u, reason: reason.into() });
        match (unit.source, mark_of(&tree, &content, p.obj, unit)) {
            (_, UnitMark::Artifact) => {}
            (_, UnitMark::Unaddressable { .. }) => return refuse("its tag is inline or listed more than once"),
            (_, UnitMark::FormStructure) => return refuse("it is a form XObject with structure of its own"),
            (Source::Sequence(s), mark) => {
                if unit.nested {
                    return refuse("other tagged content is nested inside it");
                }
                if let UnitMark::Tagged { mcid, tag } = mark {
                    if let Err(e) = editable(&tree, tag) {
                        return refuse(&e.to_string());
                    }
                    taken.entry(tag).or_default().insert(mcid);
                }
                if let UnitMark::Tagged { mcid, .. } | UnitMark::Orphan { mcid } = mark {
                    nulls.push((mcid, Object::Null));
                }
                let open = content.sequences.get(s).map(|q| q.open).ok_or(TaggingError::BadUnit(u))?;
                edits.push(content.replace(open, b"/Artifact BMC".to_vec()).ok_or(TaggingError::BadUnit(u))?);
                changed = changed.saturating_add(1);
            }
            (Source::Atom(a), _) => {
                atoms.push(a);
                changed = changed.saturating_add(1);
            }
        }
    }
    let runs = content.runs(&atoms).map_err(|reason| TaggingError::UnitNotEditable { unit: chosen.first().copied().unwrap_or(0), reason })?;
    for (first, last) in runs {
        edits.push(content.before(first, content::open_artifact()).ok_or(TaggingError::NothingSelected)?);
        edits.push(content.after(last, content::close()).ok_or(TaggingError::NothingSelected)?);
    }
    if !nulls.is_empty() {
        let root = ensure_structure(&mut next, false)?;
        let mut emptied = Vec::new();
        for (owner, mcids) in &taken {
            if remove_mcids(&mut next, *owner, tree.node(*owner).and_then(|n| n.page), p.obj, mcids)? {
                emptied.push(*owner);
            }
        }
        drop_empty(&mut next, root, &emptied)?;
        if let Some(key) = struct_parents(&next, p.obj) {
            let mut pt = parent_tree(&next, root)?;
            pt.set_array(&mut next, key, &nulls).map_err(pt_error)?;
        }
    }
    let uses = content::stream_uses(&next, &pages);
    content.commit(&mut next, edits, &uses).map_err(|reason| TaggingError::PageNotEditable { page: page.saturating_add(1), reason })?;
    *doc = next;
    Ok(changed)
}
