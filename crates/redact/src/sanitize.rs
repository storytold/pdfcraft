//! Remove Hidden Information and Sanitize Document (architecture §11.1, execution plan M8.4).
//!
//! [`scan`] counts what each category would remove; [`remove_hidden`] removes the chosen
//! categories; [`sanitize`] removes all of them. Every removal makes the next save a full
//! rewrite, so the earlier revision (which still holds the data) leaves the file.

use pdfcraft_content::Matrix;
use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::interp::{Mode, Scope, process};
use crate::{RedactError, Report, annots_of, listed_annotations, page_streams, page_streams_within};

/// The categories of Acrobat's Remove Hidden Information panel that PdfCraft handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Hidden {
    /// Document information (`/Info`) and XMP metadata streams.
    Metadata,
    /// Embedded files, associated files (`/AF`) and file attachment annotations.
    Attachments,
    /// Comments and markup annotations (with their pop-ups).
    Comments,
    /// Interactive form fields (flattened: their appearance stays as page content).
    FormFields,
    /// Text drawn invisibly (render modes 3 and 7) or wholly off the page.
    HiddenText,
    /// Optional content (layers) that is off: its content, then the layers themselves.
    HiddenLayers,
    Bookmarks,
    /// Links, actions (open action, additional actions) and document JavaScript.
    LinksActionsScripts,
    /// Private data of other applications (`/PieceInfo`).
    PrivateData,
}

pub const HIDDEN: [Hidden; 9] = [
    Hidden::Metadata,
    Hidden::Attachments,
    Hidden::Comments,
    Hidden::FormFields,
    Hidden::HiddenText,
    Hidden::HiddenLayers,
    Hidden::Bookmarks,
    Hidden::LinksActionsScripts,
    Hidden::PrivateData,
];

impl Hidden {
    pub fn label(self) -> &'static str {
        match self {
            Hidden::Metadata => "Metadata",
            Hidden::Attachments => "File attachments",
            Hidden::Comments => "Comments and markups",
            Hidden::FormFields => "Form fields",
            Hidden::HiddenText => "Hidden text",
            Hidden::HiddenLayers => "Hidden layers",
            Hidden::Bookmarks => "Bookmarks",
            Hidden::LinksActionsScripts => "Links, actions and JavaScripts",
            Hidden::PrivateData => "Private data of other applications",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Hidden::Metadata => "metadata",
            Hidden::Attachments => "attachments",
            Hidden::Comments => "comments",
            Hidden::FormFields => "form-fields",
            Hidden::HiddenText => "hidden-text",
            Hidden::HiddenLayers => "hidden-layers",
            Hidden::Bookmarks => "bookmarks",
            Hidden::LinksActionsScripts => "links-actions-scripts",
            Hidden::PrivateData => "private-data",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        HIDDEN.into_iter().find(|h| h.id() == id)
    }
}

fn catalog(doc: &Document) -> Option<(ObjRef, Dict)> {
    let r = doc.root()?;
    Some((r, doc.get(r).as_dict()?.clone()))
}

fn sub(doc: &Document, d: &Dict, key: &[u8]) -> Option<Dict> {
    d.get(key).and_then(|o| doc.resolve(o).as_dict().cloned())
}

/// The most name-tree nodes one count reads.
const NAME_TREE_NODE_BUDGET: usize = 100_000;

/// Entries of a name tree (`/Names` leaves, `/Kids` recursion). Each indirect node is read once,
/// however many `/Kids` arrays list it, so shared or cyclic kids cost one step per node rather
/// than one per path. Every `/Kids` entry the walk does not descend into (a repeated reference or
/// a non-dictionary) costs a step as well, so a long `/Kids` array of repeats cannot outrun the
/// budget. Depth and a total step budget bound the rest.
fn name_tree_len(doc: &Document, node: &Dict) -> usize {
    let (mut seen, mut visited) = (std::collections::HashSet::new(), 0);
    name_tree_len_seen(doc, node, 0, &mut seen, &mut visited)
}

fn name_tree_len_seen(doc: &Document, node: &Dict, depth: usize, seen: &mut std::collections::HashSet<ObjRef>, visited: &mut usize) -> usize {
    *visited = visited.saturating_add(1);
    if depth > 32 || *visited > NAME_TREE_NODE_BUDGET {
        return 0;
    }
    let leaves = node.get(b"Names").and_then(|n| doc.resolve(n).as_array().map(|a| a.len() / 2)).unwrap_or(0);
    let mut n = leaves;
    let kids = node.get(b"Kids").map(|k| doc.resolve(k));
    for k in kids.as_deref().and_then(Object::as_array).map_or(&[][..], Vec::as_slice) {
        if *visited > NAME_TREE_NODE_BUDGET {
            break;
        }
        let held;
        let kid = match k {
            Object::Ref(r) if !seen.insert(*r) => None,
            Object::Ref(r) => {
                held = doc.get(*r);
                held.as_dict()
            }
            other => other.as_dict(),
        };
        match kid {
            Some(kid) => n = n.saturating_add(name_tree_len_seen(doc, kid, depth + 1, seen, visited)),
            None => *visited = visited.saturating_add(1),
        }
    }
    n
}

/// The most distinct outline items one count reads, over all levels together.
const OUTLINE_ITEM_BUDGET: usize = 100_000;

fn outline_len(doc: &Document, first: Option<&Object>, depth: usize) -> usize {
    let (mut seen, mut counted) = (std::collections::HashSet::new(), 0);
    outline_len_seen(doc, first, depth, &mut seen, &mut counted)
}

/// Counts an outline level and everything below it, each item once. `seen` holds the indirect items
/// already counted, and `counted` every item counted so far, inline ones included, which the total
/// budget applies to. An item that two others name (as `/First` or `/Next`) is counted where the walk
/// first reaches it, and a level stops at an item already seen, because the walk that reached that
/// item counts the rest of its level. A hostile outline therefore costs one step per item, not 2^n
/// steps for n levels of shared children. The depth limit of 64 only matters for outlines nested that
/// deep: an item first reached through a deeper path is not visited again from a shallower one, so
/// items nested beyond the limit may stay uncounted.
fn outline_len_seen(
    doc: &Document,
    first: Option<&Object>,
    depth: usize,
    seen: &mut std::collections::HashSet<ObjRef>,
    counted: &mut usize,
) -> usize {
    let mut n = 0;
    let mut cur = first.cloned();
    let mut guard = 0;
    while let Some(o) = cur {
        guard += 1;
        if guard > 100_000 || depth > 64 || *counted >= OUTLINE_ITEM_BUDGET {
            break;
        }
        if let Object::Ref(r) = &o
            && !seen.insert(*r)
        {
            break;
        }
        let Some(d) = doc.resolve(&o).as_dict().cloned() else { break };
        *counted += 1;
        n += 1 + outline_len_seen(doc, d.get(b"First"), depth + 1, seen, counted);
        cur = d.get(b"Next").cloned();
    }
    n
}

/// Entries that hold a category's data wherever they appear, not only on the catalog and pages:
/// XMP on images, fonts and Forms, private data on Forms, associated files on pages and
/// annotations, additional actions on fields.
const OWNED: [(Hidden, &[u8]); 4] =
    [(Hidden::Metadata, b"Metadata"), (Hidden::PrivateData, b"PieceInfo"), (Hidden::Attachments, b"AF"), (Hidden::LinksActionsScripts, b"AA")];

/// Keys whose dictionary values map names a producer chooses to objects (resources, glyph
/// procedures, named destinations, role and class maps, private data by application). A name
/// there may be spelt like an owned entry (a font called `/AF`), so these maps are walked but
/// never stripped.
const NAME_MAPS: [&[u8]; 14] = [
    b"Font",
    b"XObject",
    b"ExtGState",
    b"ColorSpace",
    b"Pattern",
    b"Shading",
    b"Properties",
    b"CharProcs",
    b"Dests",
    b"RoleMap",
    b"ClassMap",
    b"Colorants",
    b"Schema",
    b"PieceInfo",
];

/// How deep the owner sweep follows dictionaries and arrays written in place.
const MAX_DIRECT_DEPTH: usize = 64;

/// What a dictionary is to the owner sweep.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Held {
    /// An object that may own an entry of [`OWNED`].
    Owner,
    /// A [`NAME_MAPS`] map: its keys are names, its values owners.
    Map,
    /// An appearance dictionary (`/AP`): its dictionary values map appearance states.
    Appearances,
}

impl Held {
    fn child(self, key: &[u8]) -> Held {
        match self {
            Held::Owner if NAME_MAPS.contains(&key) => Held::Map,
            Held::Owner if key == b"AP" => Held::Appearances,
            Held::Appearances => Held::Map,
            _ => Held::Owner,
        }
    }
}

/// Indirect objects that hold an owned entry, each with the role it was reached in.
type Holders = Vec<(ObjRef, Held)>;

/// The objects reachable from the catalog that hold one of `keys` outside a name map, and how
/// many dictionaries hold each key. Each object is read at most once per [`Held`] role, so a
/// shared or cyclic graph costs one step per object.
fn owners(doc: &Document, keys: &[&[u8]]) -> Result<(Vec<usize>, Holders), RedactError> {
    let mut held = vec![0; keys.len()];
    let mut holders = Vec::new();
    let Some(root) = doc.root() else { return Ok((held, holders)) };
    let mut next = vec![(root, Held::Owner)];
    let mut seen = std::collections::HashSet::new();
    while let Some((r, role)) = next.pop() {
        if !seen.insert((r, role)) {
            continue;
        }
        if find_owned(&doc.get(r), role, keys, &mut held, &mut next, 0)? {
            holders.push((r, role));
        }
    }
    Ok((held, holders))
}

/// Counts the dictionaries in `o` (and what it holds in place) that hold one of `keys`, and
/// queues the indirect objects it names. Returns whether any key was found.
fn find_owned(o: &Object, role: Held, keys: &[&[u8]], held: &mut [usize], next: &mut Vec<(ObjRef, Held)>, depth: usize) -> Result<bool, RedactError> {
    if depth > MAX_DIRECT_DEPTH {
        return Err(RedactError::TooDeep);
    }
    let (d, role) = match o {
        Object::Ref(r) => {
            next.push((*r, role));
            return Ok(false);
        }
        Object::Array(a) => {
            let mut found = false;
            for v in a {
                found |= find_owned(v, Held::Owner, keys, held, next, depth + 1)?;
            }
            return Ok(found);
        }
        Object::Dict(d) => (d, role),
        // A stream's dictionary is never a name map.
        Object::Stream(s) => (&s.dict, Held::Owner),
        _ => return Ok(false),
    };
    let mut found = false;
    if role != Held::Map {
        for (i, k) in keys.iter().enumerate() {
            if d.contains(k)
                && let Some(n) = held.get_mut(i)
            {
                *n = n.saturating_add(1);
                found = true;
            }
        }
    }
    for (k, v) in d.iter() {
        found |= find_owned(v, role.child(k), keys, held, next, depth + 1)?;
    }
    Ok(found)
}

/// Removes `keys` from `o` and from what it holds in place, outside name maps. [`owners`] has
/// already refused anything nested past the depth limit.
fn strip_owned(o: &mut Object, role: Held, keys: &[&[u8]], depth: usize) {
    if depth > MAX_DIRECT_DEPTH {
        return;
    }
    let (d, role) = match o {
        Object::Array(a) => {
            for v in a {
                strip_owned(v, Held::Owner, keys, depth + 1);
            }
            return;
        }
        Object::Dict(d) => (d, role),
        Object::Stream(s) => (&mut s.dict, Held::Owner),
        _ => return,
    };
    if role != Held::Map {
        for k in keys {
            d.remove(k);
        }
    }
    let names: Vec<Vec<u8>> = d.iter().filter(|(_, v)| matches!(v, Object::Array(_) | Object::Dict(_))).map(|(k, _)| k.clone()).collect();
    for k in names {
        if let Some(v) = d.get_mut(&k) {
            strip_owned(v, role.child(&k), keys, depth + 1);
        }
    }
}

/// A comment as the comments list sees it, less file attachments (removed with Attachments) and
/// redaction marks.
fn is_comment(subtype: &[u8]) -> bool {
    !matches!(subtype, b"FileAttachment" | b"Redact") && pdfcraft_annot::is_comment_subtype(&String::from_utf8_lossy(subtype))
}

/// The layers that are off in the default configuration.
fn hidden_layers(doc: &Document) -> Vec<ObjRef> {
    let Some((_, cat)) = catalog(doc) else { return Vec::new() };
    let Some(oc) = sub(doc, &cat, b"OCProperties") else { return Vec::new() };
    let Some(d) = sub(doc, &oc, b"D") else { return Vec::new() };
    let all: Vec<ObjRef> =
        oc.get(b"OCGs").and_then(|g| doc.resolve(g).as_array().map(|a| a.iter().filter_map(Object::as_ref).collect())).unwrap_or_default();
    let list = |k: &[u8]| -> Vec<ObjRef> {
        d.get(k).and_then(|g| doc.resolve(g).as_array().map(|a| a.iter().filter_map(Object::as_ref).collect())).unwrap_or_default()
    };
    let (on, off) = (list(b"ON"), list(b"OFF"));
    if d.name(b"BaseState") == Some(b"OFF") {
        all.into_iter().filter(|g| !on.contains(g)).collect()
    } else {
        off.into_iter().filter(|g| all.contains(g)).collect()
    }
}

/// Glyphs of hidden text, or content of hidden layers, on every page (`write` = remove them).
fn content_pass(doc: &mut Document, text: bool, layers: bool, write: bool) -> Result<(usize, std::collections::HashSet<ObjRef>), RedactError> {
    let off = if layers { hidden_layers(doc) } else { Vec::new() };
    if !text && !layers {
        return Ok((0, std::collections::HashSet::new()));
    }
    let mut total = 0;
    let mut retained_off_layers = std::collections::HashSet::new();
    // The original image and form XObjects the rewrite drops or replaces, on any page.
    let mut doomed = Vec::new();
    let mut retired_properties = Vec::new();
    let n = pdfcraft_model::pages(doc).len();
    for pi in 0..n {
        let page = pdfcraft_model::pages(doc).swap_remove(pi);
        let mut budget = pdfcraft_cos::DecodeBudget::page();
        let Ok((list, data)) = page_streams_within(doc, &page.dict, pi, &mut budget) else {
            if write {
                return Err(RedactError::Unreadable(pi + 1));
            }
            // Content that can't be read may hide anything: count it so the removal runs and
            // refuses the page, instead of a zero count skipping it.
            total += 1;
            continue;
        };
        let resources = page.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
        let mut report = Report::default();
        let mode = if write { Mode::Apply } else { Mode::Verify };
        let mut scope = Scope::new(&[], mode, &mut report, budget);
        if text {
            let c = page.crop(doc);
            scope.hidden_text = Some([c[0].min(c[2]), c[1].min(c[3]), c[0].max(c[2]), c[1].max(c[3])]);
        }
        scope.hidden_layers = off.iter().copied().collect();
        scope.layers_enabled = layers;
        let out = process(doc, &mut scope, &data, &resources, Matrix::IDENTITY);
        if let Some(error) = out.error {
            return Err(error.into());
        }
        let blocks = scope.layer_blocks;
        let glyphs = scope.report.glyphs;
        let retained_for_page = std::mem::take(&mut scope.retained_off_layers);
        drop(scope);
        total += out.residue + blocks + glyphs;
        retained_off_layers.extend(retained_for_page);
        if !write {
            continue;
        }
        doomed.extend_from_slice(&out.doomed);
        retired_properties.extend(out.retired_properties);
        let mut new_list = list.clone();
        let mut changed = false;
        for (i, new) in out.streams.into_iter().enumerate() {
            let Some(bytes) = new else { continue };
            let mut dict = match &*doc.resolve(&list[i]) {
                Object::Stream(s) => s.dict.clone(),
                _ => Dict::new(),
            };
            dict.remove(b"Length");
            // The source text of an added item (Edit ▸ Add content) would keep what was removed.
            dict.remove(b"PCAdded");
            new_list[i] = Object::Ref(doc.add(Object::Stream(pdfcraft_cos::Stream::flate(dict, &bytes))));
            changed = true;
        }
        if changed || !out.xobjects.is_empty() || !out.replaced.is_empty() {
            // Pin inherited resources before replacing them. When no page still inherits the
            // original holder, retiring it also retires descriptions no longer used anywhere.
            crate::localize_resources(doc, &[pi])?;
            let mut cleaned_page = page.dict.clone();
            cleaned_page.set(b"Contents".to_vec(), Object::Array(new_list.clone()));
            let (_, page_content) = page_streams(doc, &cleaned_page, pi)?;
            let mut res = resources.clone();
            if !out.xobjects.is_empty() || !out.replaced.is_empty() {
                let mut xo = res.get(b"XObject").and_then(|x| doc.resolve(x).as_dict().cloned()).unwrap_or_default();
                // A full save keeps whatever is still referenced: unlink the originals this page no
                // longer draws so the removed form or image bytes cannot survive in the saved file.
                crate::interp::prune(&mut xo, &out.replaced, &page_content);
                for (n, r) in &out.xobjects {
                    xo.set(n.clone(), Object::Ref(*r));
                }
                res.set(b"XObject".to_vec(), Object::Dict(xo));
            }
            retired_properties.extend(crate::interp::prune_properties(doc, &mut res, &page_content)?);
            doc.update_dict(page.obj, |d| {
                d.set(b"Contents".to_vec(), Object::Array(new_list));
                d.set(b"Resources".to_vec(), Object::Dict(res));
            })?;
            // Tags must not describe or point at what the page no longer shows.
            let (changed_ids, empty_ids) = crate::tags::touched(
                doc,
                &resources,
                pdfcraft_content::Pieces::join(&data).data(),
                pdfcraft_content::Pieces::join(&page_content).data(),
            );
            crate::tags::clean(doc, page.obj, None, &changed_ids, &empty_ids)?;
        }
        for (old, new, changed, empty) in out.tag_changes {
            crate::tags::clean(doc, page.obj, Some((old, new)), &changed, &empty)?;
        }
    }
    // What a page no longer draws must not stay reachable through an inherited or shared
    // /Resources either.
    crate::interp::unlink_doomed(doc, &doomed, &retired_properties)?;
    Ok((total, retained_off_layers))
}

/// How many items each category would remove (categories with nothing are included as 0).
pub fn scan(doc: &Document) -> Vec<(Hidden, usize)> {
    let cat = catalog(doc).map(|c| c.1).unwrap_or_default();
    let pages = pdfcraft_model::pages(doc);
    let annots: Vec<Dict> = pages.iter().flat_map(|p| annots_of(doc, &p.dict)).filter_map(|a| doc.resolve(&a).as_dict().cloned()).collect();
    let count_sub = |s: &[u8]| annots.iter().filter(|a| a.name(b"Subtype") == Some(s)).count();
    let names = sub(doc, &cat, b"Names").unwrap_or_default();
    let owned_keys: Vec<&[u8]> = OWNED.iter().map(|o| o.1).collect();
    // A graph nested too deeply to walk counts as holding everything, so the removal runs and
    // refuses it.
    let held = owners(doc, &owned_keys).map_or_else(|_| vec![1; OWNED.len()], |o| o.0);
    let owned = |h: Hidden| OWNED.iter().position(|o| o.0 == h).and_then(|i| held.get(i)).copied().unwrap_or(0);
    let mut doc2 = doc.clone();
    HIDDEN
        .into_iter()
        .map(|h| {
            let n = match h {
                Hidden::Metadata => {
                    let info = doc.trailer().get(b"Info").and_then(|i| doc.resolve(i).as_dict().map(|d| d.len())).unwrap_or(0);
                    info + owned(Hidden::Metadata)
                }
                Hidden::Attachments => {
                    sub(doc, &names, b"EmbeddedFiles").map_or(0, |t| name_tree_len(doc, &t))
                        + count_sub(b"FileAttachment")
                        + owned(Hidden::Attachments)
                }
                Hidden::Comments => annots.iter().filter(|a| is_comment(a.name(b"Subtype").unwrap_or(b""))).count(),
                Hidden::FormFields => {
                    let n = pdfcraft_forms::fields(doc).len();
                    if n > 0 {
                        n
                    } else if cat.contains(b"AcroForm") {
                        1
                    } else {
                        0
                    }
                }
                Hidden::HiddenText => content_pass(&mut doc2, true, false, false).map(|(n, _)| n).unwrap_or(0),
                Hidden::HiddenLayers => {
                    let off = hidden_layers(doc).len();
                    off + content_pass(&mut doc2, false, true, false).map(|(n, _)| n).unwrap_or(0)
                }
                Hidden::Bookmarks => sub(doc, &cat, b"Outlines").map_or(0, |o| outline_len(doc, o.get(b"First"), 0)),
                Hidden::LinksActionsScripts => {
                    count_sub(b"Link")
                        + usize::from(cat.get(b"OpenAction").is_some_and(|a| doc.resolve(a).as_dict().is_some()))
                        + owned(Hidden::LinksActionsScripts)
                        + annots.iter().filter(|a| !a.contains(b"AA") && a.name(b"Subtype") == Some(b"Widget") && a.contains(b"A")).count()
                        + sub(doc, &names, b"JavaScript").map_or(0, |t| name_tree_len(doc, &t))
                }
                Hidden::PrivateData => owned(Hidden::PrivateData),
            };
            (h, n)
        })
        .collect()
}

/// Remove the chosen categories. Returns what was removed per category.
///
/// # Errors
///
/// This operation is not transactional: an error may leave `doc` partially modified, with
/// sensitive content still present. Call this on a working clone and replace the original only
/// on success. On any error, discard the working clone; do not save or reuse it.
pub fn remove_hidden(doc: &mut Document, which: &[Hidden]) -> Result<Vec<(Hidden, usize)>, RedactError> {
    let counts = scan(doc);
    let count = |h: Hidden| counts.iter().find(|c| c.0 == h).map_or(0, |c| c.1);
    let (root, _) = catalog(doc).ok_or(RedactError::NothingToApply)?;
    let mut done = Vec::new();
    // Every annotation the pages list now: whatever is no longer listed at the end (flattened
    // widgets included) leaves the structure tree too.
    let listed_before = listed_annotations(doc);
    let has_acroform = doc.get(root).as_dict().is_some_and(|d| d.contains(b"AcroForm"));
    let fields = which.contains(&Hidden::FormFields) && (count(Hidden::FormFields) > 0 || has_acroform);
    if fields {
        // Fields without appearances get one first, so their values stay visible.
        for f in pdfcraft_forms::fields(doc) {
            if f.widgets.iter().any(|w| !doc.get(w.obj).as_dict().is_some_and(|d| d.contains(b"AP"))) {
                pdfcraft_forms::redraw_field(doc, &f.name)?;
            }
        }
        let n = pdfcraft_model::pages(doc).len();
        pdfcraft_edit::flatten(doc, &(0..n).collect::<Vec<_>>(), false, true)?;
        doc.update_dict(root, |d| {
            d.remove(b"AcroForm");
        })?;
    }
    // Content next (it needs the layers still in place). Flattened appearances are page content
    // by now, so hidden text or layers inside them are removed with the rest.
    let (text, layers) = (which.contains(&Hidden::HiddenText), which.contains(&Hidden::HiddenLayers));
    let retained_off_layers = if (text && (fields || count(Hidden::HiddenText) > 0)) || (layers && (fields || count(Hidden::HiddenLayers) > 0)) {
        content_pass(doc, text, layers, true)?.1
    } else {
        std::collections::HashSet::new()
    };
    // Annotations: comments, attachments, links, actions, and the widgets flattening kept (hidden
    // ones, and ones with nothing to draw), which still hold their field's value and parent.
    let drop_annot = |a: &Dict| -> bool {
        let s = a.name(b"Subtype").unwrap_or(b"");
        (which.contains(&Hidden::Comments) && is_comment(s))
            || (fields && s == b"Widget")
            || (which.contains(&Hidden::Attachments) && s == b"FileAttachment")
            || (which.contains(&Hidden::LinksActionsScripts) && s == b"Link")
    };
    // The annotations to remove, on every page first, so a pop-up listed on another page than
    // its parent goes with it.
    let removed: std::collections::HashSet<ObjRef> = pdfcraft_model::pages(doc)
        .iter()
        .flat_map(|p| annots_of(doc, &p.dict))
        .filter_map(|a| a.as_ref())
        .filter(|&r| doc.get(r).as_dict().is_some_and(drop_annot))
        .collect();
    for p in pdfcraft_model::pages(doc) {
        let list = annots_of(doc, &p.dict);
        let mut kept = Vec::new();
        for a in list {
            let Some(r) = a.as_ref() else {
                if !doc.resolve(&a).as_dict().is_some_and(drop_annot) {
                    kept.push(a);
                }
                continue;
            };
            let d = doc.get(r).as_dict().cloned().unwrap_or_default();
            let parent_gone = d.get(b"Parent").and_then(Object::as_ref).is_some_and(|x| removed.contains(&x));
            if removed.contains(&r) || (d.name(b"Subtype") == Some(b"Popup") && parent_gone) {
                continue;
            }
            if which.contains(&Hidden::LinksActionsScripts) && (d.contains(b"AA") || (d.name(b"Subtype") == Some(b"Widget") && d.contains(b"A"))) {
                doc.update_dict(r, |d| {
                    d.remove(b"AA");
                    d.remove(b"A");
                })?;
            }
            kept.push(a);
        }
        doc.update_dict(p.obj, |d| {
            if kept.is_empty() {
                d.remove(b"Annots");
            } else {
                d.set(b"Annots".to_vec(), Object::Array(kept));
            }
        })?;
    }
    // A tagged PDF's structure tree still points at the removed links, comments and flattened
    // widgets (#2447): drop those references, or the full rewrite keeps the annotations, URLs,
    // text and field values included.
    let still = listed_annotations(doc);
    let gone: std::collections::HashSet<ObjRef> = listed_before.difference(&still).copied().collect();
    crate::tags::drop_object_refs(doc, &gone)?;
    let off = hidden_layers(doc);
    let mut cat = doc.get(root).as_dict().cloned().unwrap_or_default();
    let mut names = sub(doc, &cat, b"Names");
    for &h in which {
        match h {
            Hidden::Metadata => {
                doc.trailer_mut().remove(b"Info");
            }
            Hidden::Attachments => {
                if let Some(n) = names.as_mut() {
                    n.remove(b"EmbeddedFiles");
                }
            }
            Hidden::Bookmarks => {
                cat.remove(b"Outlines");
                if cat.name(b"PageMode") == Some(b"UseOutlines") {
                    cat.remove(b"PageMode");
                }
            }
            Hidden::LinksActionsScripts => {
                if cat.get(b"OpenAction").is_some_and(|a| doc.resolve(a).as_dict().is_some()) {
                    cat.remove(b"OpenAction");
                }
                if let Some(n) = names.as_mut() {
                    n.remove(b"JavaScript");
                }
            }
            Hidden::HiddenLayers if !off.is_empty() => {
                // The off layers leave the configuration (their content is gone already).
                if let Some(mut oc) = sub(doc, &cat, b"OCProperties") {
                    let keep = |o: &Object| !o.as_ref().is_some_and(|r| off.contains(&r) && !retained_off_layers.contains(&r));
                    if let Some(Object::Array(a)) = oc.get(b"OCGs").map(|g| (*doc.resolve(g)).clone()) {
                        oc.set(b"OCGs".to_vec(), Object::Array(a.into_iter().filter(keep).collect()));
                    }
                    if let Some(mut d) = sub(doc, &oc, b"D") {
                        for k in [&b"ON"[..], b"OFF", b"Order", b"Locked"] {
                            if let Some(Object::Array(a)) = d.get(k).map(|g| (*doc.resolve(g)).clone()) {
                                d.set(k.to_vec(), Object::Array(a.into_iter().filter(keep).collect()));
                            }
                        }
                        oc.set(b"D".to_vec(), Object::Dict(d));
                    }
                    cat.set(b"OCProperties".to_vec(), Object::Dict(oc));
                }
            }
            Hidden::FormFields => {
                cat.remove(b"AcroForm");
            }
            _ => {}
        }
        let n = if h == Hidden::FormFields && fields { count(h).max(1) } else { count(h) };
        if n > 0 {
            done.push((h, n));
        }
    }
    if let Some(n) = names {
        cat.set(b"Names".to_vec(), Object::Dict(n));
    }
    doc.set(root, Object::Dict(cat));
    // Metadata, private data, associated files and additional actions on every object that
    // still owns them (the catalog and pages included).
    let keys: Vec<&[u8]> = OWNED.iter().filter(|o| which.contains(&o.0)).map(|o| o.1).collect();
    if !keys.is_empty() {
        for (r, role) in owners(doc, &keys)?.1 {
            let mut o = (*doc.get(r)).clone();
            strip_owned(&mut o, role, &keys, 0);
            doc.set(r, o);
        }
    }
    if done.is_empty() {
        return Err(RedactError::NothingToApply);
    }
    doc.require_full_save();
    Ok(done)
}

/// Sanitize Document: remove every category of hidden information.
pub fn sanitize(doc: &mut Document) -> Result<Vec<(Hidden, usize)>, RedactError> {
    match remove_hidden(doc, &HIDDEN) {
        // Nothing hidden is fine for Sanitize: the full rewrite still drops old revisions.
        Err(RedactError::NothingToApply) => {
            doc.require_full_save();
            Ok(Vec::new())
        }
        r => r,
    }
}
