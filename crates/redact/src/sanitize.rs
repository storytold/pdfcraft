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
    /// Links, actions (open action, additional actions, what bookmarks run) and document JavaScript.
    LinksActionsScripts,
    /// Private data of other applications (`/PieceInfo`), and prepress data on images and Forms
    /// (`/OPI`, `/Alternates`).
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

#[cfg(test)]
thread_local! {
    /// Passes over every page's content ([`content_pass`]) made on this thread.
    static CONTENT_PASSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Walks of the whole object graph for owned entries ([`owners`]) made by [`scan_only`] on
    /// this thread.
    static OWNER_WALKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many content passes and owner walks this thread has made, for tests to count the work.
#[cfg(test)]
pub(crate) fn passes() -> (usize, usize) {
    (CONTENT_PASSES.with(std::cell::Cell::get), OWNER_WALKS.with(std::cell::Cell::get))
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

/// Takes out the action of outline item `d` when it does more than go to a place in this document
/// (a script, a launch, a web or remote link, a form submission); a jump with an action chained
/// after it (`/Next`) keeps only the jump. Items it links to that are written in place are cleaned
/// the same way, and items that are objects of their own go on `next`. Every link counts, not only
/// `/First` and `/Next`: a damaged outline may reach an item only backwards, and a full save keeps
/// that item too. `items` counts every item cleaned, inline ones included. Returns how many actions
/// went, or `None` past the depth limit or past [`OUTLINE_ITEM_BUDGET`] items.
fn clean_outline_item(doc: &Document, d: &mut Dict, next: &mut Vec<ObjRef>, items: &mut usize, depth: usize) -> Option<usize> {
    *items = items.saturating_add(1);
    if depth > MAX_DIRECT_DEPTH || *items > OUTLINE_ITEM_BUDGET {
        return None;
    }
    let mut n = 0;
    if let Some(action) = d.get(b"A").and_then(|a| doc.resolve(a).as_dict().cloned()) {
        match (action.name(b"S"), action.get(b"D")) {
            (Some(b"GoTo"), _) if !action.contains(b"Next") => {}
            (Some(b"GoTo"), Some(place)) => {
                let mut jump = Dict::new();
                jump.set(b"S".to_vec(), Object::name("GoTo"));
                jump.set(b"D".to_vec(), place.clone());
                d.set(b"A".to_vec(), Object::Dict(jump));
                n += 1;
            }
            _ => {
                d.remove(b"A");
                n += 1;
            }
        }
    }
    for key in [&b"First"[..], b"Last", b"Next", b"Prev", b"Parent"] {
        match d.get_mut(key) {
            Some(Object::Ref(r)) => next.push(*r),
            Some(Object::Dict(kid)) => n += clean_outline_item(doc, kid, next, items, depth + 1)?,
            _ => {}
        }
    }
    Some(n)
}

/// [`clean_outline_item`] over the outline from the objects in `next`, after `items` items cleaned
/// already: the outline objects to write back, and how many actions went. Each object is read once;
/// `None` when the outline is larger than [`OUTLINE_ITEM_BUDGET`] items or nests in place past the
/// depth limit, so the removal can't be shown complete.
fn outline_without_actions(doc: &Document, mut next: Vec<ObjRef>, mut items: usize) -> Option<(Vec<(ObjRef, Dict)>, usize)> {
    let (mut out, mut n, mut seen) = (Vec::new(), 0, std::collections::HashSet::new());
    while let Some(r) = next.pop() {
        if !seen.insert(r) {
            continue;
        }
        if seen.len() > OUTLINE_ITEM_BUDGET {
            return None;
        }
        let Some(mut d) = doc.get(r).as_dict().cloned() else { continue };
        let went = clean_outline_item(doc, &mut d, &mut next, &mut items, 0)?;
        if went > 0 {
            out.push((r, d));
            n += went;
        }
    }
    Some((out, n))
}

/// How many outline actions Links, actions and JavaScripts would take out ([`clean_outline_item`]).
/// An outline too large or too deep to follow counts as one, so the removal runs and refuses it.
fn outline_action_count(doc: &Document, cat: &Dict) -> usize {
    let (mut next, mut items) = (Vec::new(), 0);
    let mut n = match cat.get(b"Outlines") {
        Some(Object::Ref(r)) => {
            next.push(*r);
            0
        }
        Some(Object::Dict(o)) => clean_outline_item(doc, &mut o.clone(), &mut next, &mut items, 0).unwrap_or(1),
        _ => return 0,
    };
    n += outline_without_actions(doc, next, items).map_or(1, |(_, went)| went);
    n
}

/// Entries that hold a category's data wherever they appear, not only on the catalog and pages:
/// XMP on images, fonts and Forms, private data on Forms, associated files on pages and
/// annotations, additional actions on fields. Images and Forms also carry prepress data: an OPI
/// proxy's link to the original file, and alternate images, which can be the unedited original.
const OWNED: [(Hidden, &[u8]); 6] = [
    (Hidden::Metadata, b"Metadata"),
    (Hidden::PrivateData, b"PieceInfo"),
    (Hidden::PrivateData, b"OPI"),
    (Hidden::PrivateData, b"Alternates"),
    (Hidden::Attachments, b"AF"),
    (Hidden::LinksActionsScripts, b"AA"),
];

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

/// The layers that are off in the default configuration: listed off (or not listed on, under
/// `/BaseState /OFF`), or turned off on screen by one of its usage rules (`/AS`, ISO 32000-2,
/// 8.11.4.4): a rule for the View event in the View category hides a layer whose
/// `/Usage /View /ViewState` is OFF. Rules that turn a layer on, or apply only when printing or
/// exporting, leave it as the lists say.
fn hidden_layers(doc: &Document) -> Vec<ObjRef> {
    let Some((_, cat)) = catalog(doc) else { return Vec::new() };
    let Some(oc) = sub(doc, &cat, b"OCProperties") else { return Vec::new() };
    let Some(d) = sub(doc, &oc, b"D") else { return Vec::new() };
    let refs = |o: Option<&Object>| -> Vec<ObjRef> {
        o.and_then(|g| doc.resolve(g).as_array().map(|a| a.iter().filter_map(Object::as_ref).collect())).unwrap_or_default()
    };
    let all = refs(oc.get(b"OCGs"));
    let (on, off) = (refs(d.get(b"ON")), refs(d.get(b"OFF")));
    let mut hidden: Vec<ObjRef> = if d.name(b"BaseState") == Some(b"OFF") {
        all.iter().copied().filter(|g| !on.contains(g)).collect()
    } else {
        off.into_iter().filter(|g| all.contains(g)).collect()
    };
    // Each rule, layer list and layer is read once, however often the rules name them: many rules
    // can share one long list.
    let known: std::collections::HashSet<ObjRef> = all.iter().copied().collect();
    let mut looked: std::collections::HashSet<ObjRef> = hidden.iter().copied().collect();
    let mut read = std::collections::HashSet::new();
    let rules = d.get(b"AS").and_then(|a| doc.resolve(a).as_array().cloned()).unwrap_or_default();
    for rule in &rules {
        if let Some(r) = rule.as_ref()
            && !read.insert(r)
        {
            continue;
        }
        let Some(rule) = doc.resolve(rule).as_dict().cloned() else { continue };
        let categories = rule.get(b"Category").and_then(|c| doc.resolve(c).as_array().cloned()).unwrap_or_default();
        if rule.name(b"Event") != Some(b"View") || !categories.iter().any(|c| c.as_name() == Some(b"View")) {
            continue;
        }
        if let Some(r) = rule.get(b"OCGs").and_then(Object::as_ref)
            && !read.insert(r)
        {
            continue;
        }
        for g in refs(rule.get(b"OCGs")) {
            if !known.contains(&g) || !looked.insert(g) {
                continue;
            }
            let usage = doc.get(g).as_dict().and_then(|layer| sub(doc, layer, b"Usage"));
            if usage.and_then(|u| sub(doc, &u, b"View")).is_some_and(|v| v.name(b"ViewState") == Some(b"OFF")) {
                hidden.push(g);
            }
        }
    }
    hidden
}

/// The objects a layer cleanup has reached by reference, and whether the lists naming each keep it.
type Cleaned = std::collections::HashMap<ObjRef, bool>;

/// `o` cleaned by `clean`: in its list when written there, and when written by reference, once and
/// where it is, however many lists name it (they keep the reference, or drop it when `clean` gives
/// `None`). A list that names one object many times costs one step per object, not one per path,
/// and a cycle stops where it began.
fn once(
    doc: &mut Document,
    o: &Object,
    cleaned: &mut Cleaned,
    clean: impl FnOnce(&mut Document, &Object, &mut Cleaned) -> Result<Option<Object>, RedactError>,
) -> Result<Option<Object>, RedactError> {
    let Object::Ref(r) = o else { return clean(doc, o, cleaned) };
    if let Some(&kept) = cleaned.get(r) {
        return Ok(kept.then(|| o.clone()));
    }
    cleaned.insert(*r, true);
    let held: &Object = &doc.resolve(o);
    let Some(new) = clean(doc, held, cleaned)? else {
        cleaned.insert(*r, false);
        return Ok(None);
    };
    if new != *held {
        doc.set_existing(*r, new)?;
    }
    Ok(Some(o.clone()))
}

/// `o` without the entries `keep` refuses, through nested arrays (an `/Order` holds sub-lists, and
/// `/RBGroups` holds groups), each array written by reference cleaned [`once`]. `None` when `keep`
/// refuses `o` itself, or when `o` is a sub-list (`depth` above 0) that named a layer and names
/// none now: it goes, with the heading that labelled it.
fn filtered(
    doc: &mut Document,
    o: &Object,
    keep: &impl Fn(&Object) -> bool,
    depth: usize,
    cleaned: &mut Cleaned,
) -> Result<Option<Object>, RedactError> {
    if depth > MAX_DIRECT_DEPTH {
        return Err(RedactError::TooDeep);
    }
    if !keep(o) {
        return Ok(None);
    }
    once(doc, o, cleaned, |doc, o, cleaned| {
        let Object::Array(a) = o else { return Ok(Some(o.clone())) };
        let mut out = Vec::new();
        for x in a {
            out.extend(filtered(doc, x, keep, depth + 1, cleaned)?);
        }
        // Anything but a text string (a heading) is a layer or a sub-list.
        let names = |l: &[Object]| l.iter().any(|x| !matches!(x, Object::String(_)));
        let emptied = depth > 0 && names(a.as_slice()) && !names(out.as_slice());
        Ok((!emptied).then_some(Object::Array(out)))
    })
}

/// An optional-content configuration without the layers `keep` refuses, in every list that names
/// them: on, off, order, locked, radio-button groups and usage rules (a rule, group or sub-list left
/// with no layer goes). A removed layer still named in any of them would stay in the file, its name
/// included.
fn config_without(doc: &mut Document, mut d: Dict, keep: &impl Fn(&Object) -> bool, cleaned: &mut Cleaned) -> Result<Dict, RedactError> {
    for k in [&b"ON"[..], b"OFF", b"Order", b"Locked", b"RBGroups"] {
        if let Some(v) = d.get(k).cloned() {
            let v = filtered(doc, &v, keep, 0, cleaned)?.unwrap_or_else(|| Object::Array(Vec::new()));
            d.set(k.to_vec(), v);
        }
    }
    let Some(rules) = d.get(b"AS").cloned() else { return Ok(d) };
    let rules = once(doc, &rules, cleaned, |doc, rules, cleaned| {
        let Object::Array(rules) = rules else { return Ok(Some(rules.clone())) };
        let mut kept = Vec::new();
        for rule in rules {
            kept.extend(once(doc, rule, cleaned, |doc, rule, cleaned| {
                let Some(mut rule) = rule.as_dict().cloned() else { return Ok(None) };
                if let Some(layers) = rule.get(b"OCGs").cloned() {
                    let Some(layers) = filtered(doc, &layers, keep, 0, cleaned)? else { return Ok(None) };
                    if doc.resolve(&layers).as_array().is_some_and(Vec::is_empty) {
                        return Ok(None);
                    }
                    rule.set(b"OCGs".to_vec(), layers);
                }
                Ok(Some(Object::Dict(rule)))
            })?);
        }
        Ok(Some(Object::Array(kept)))
    })?;
    if let Some(rules) = rules {
        d.set(b"AS".to_vec(), rules);
    }
    Ok(d)
}

/// Glyphs of hidden text, or content of hidden layers, on every page (`write` = remove them).
fn content_pass(doc: &mut Document, text: bool, layers: bool, write: bool) -> Result<(usize, std::collections::HashSet<ObjRef>), RedactError> {
    let off = if layers { hidden_layers(doc) } else { Vec::new() };
    if !text && !layers {
        return Ok((0, std::collections::HashSet::new()));
    }
    #[cfg(test)]
    CONTENT_PASSES.with(|n| n.set(n.get() + 1));
    let mut total = 0;
    let mut retained_off_layers = std::collections::HashSet::new();
    // The original image and form XObjects the rewrite drops or replaces, on any page.
    let mut doomed = Vec::new();
    let mut retired_properties = Vec::new();
    // The page tree is read once, and again only after a page's rewrite (pinning its inherited
    // resources can change its ancestors), not once per page: a scan of many pages stays linear.
    let mut pages = pdfcraft_model::pages(doc);
    let n = pages.len();
    for pi in 0..n {
        let Some(page) = pages.get(pi).cloned() else { break };
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
        let mut images = pdfcraft_cos::ImageBudget::new(doc);
        let mut scope = Scope::new(&[], mode, &mut report, budget, &mut images);
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
            crate::type3::prune_type3_from_resources(doc, &mut res, &page_content, Some(&page.dict))?;
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
            pages = pdfcraft_model::pages(doc);
        }
        for (old, new, changed, empty) in out.tag_changes {
            crate::tags::clean(doc, page.obj, Some((old, new)), &changed, &empty)?;
        }
    }
    // What a page no longer draws must not stay reachable through an inherited or shared
    // /Resources either.
    // Only a removal rewrites the document; a count on a clone has nothing to unlink, and a
    // cleanup error there would be mistaken for "nothing to remove".
    if write {
        crate::interp::unlink_doomed(doc, &doomed, &retired_properties)?;
    }
    Ok((total, retained_off_layers))
}

/// How many items each category would remove (categories with nothing are included as 0).
pub fn scan(doc: &Document) -> Vec<(Hidden, usize)> {
    scan_only(doc, &HIDDEN)
}

/// [`scan`] for the categories in `which` alone, in [`HIDDEN`] order, each once. A category's
/// count costs what it reads: hidden text and hidden layers read every page's content, and
/// metadata, attachments, links and private data walk every object the catalog reaches, so
/// counting bookmarks alone does neither.
pub fn scan_only(doc: &Document, which: &[Hidden]) -> Vec<(Hidden, usize)> {
    let cat = catalog(doc).map(|c| c.1).unwrap_or_default();
    let annots_read = std::cell::OnceCell::<Vec<Dict>>::new();
    let annots = || {
        annots_read.get_or_init(|| {
            pdfcraft_model::pages(doc).iter().flat_map(|p| annots_of(doc, &p.dict)).filter_map(|a| doc.resolve(&a).as_dict().cloned()).collect()
        })
    };
    let count_sub = |s: &[u8]| annots().iter().filter(|a| a.name(b"Subtype") == Some(s)).count();
    let names_read = std::cell::OnceCell::<Dict>::new();
    let names = || names_read.get_or_init(|| sub(doc, &cat, b"Names").unwrap_or_default());
    let held = std::cell::OnceCell::<Vec<usize>>::new();
    let owned = |h: Hidden| {
        let held = held.get_or_init(|| {
            #[cfg(test)]
            OWNER_WALKS.with(|n| n.set(n.get() + 1));
            let owned_keys: Vec<&[u8]> = OWNED.iter().map(|o| o.1).collect();
            // A graph nested too deeply to walk counts as holding everything, so the removal
            // runs and refuses it.
            owners(doc, &owned_keys).map_or_else(|_| vec![1; OWNED.len()], |o| o.0)
        });
        OWNED.iter().zip(held).filter(|(o, _)| o.0 == h).map(|(_, n)| n).sum::<usize>()
    };
    let mut doc2 = doc.clone();
    HIDDEN
        .into_iter()
        .filter(|h| which.contains(h))
        .map(|h| {
            let n = match h {
                Hidden::Metadata => {
                    let info = doc.trailer().get(b"Info").and_then(|i| doc.resolve(i).as_dict().map(|d| d.len())).unwrap_or(0);
                    info + owned(Hidden::Metadata)
                }
                Hidden::Attachments => {
                    sub(doc, names(), b"EmbeddedFiles").map_or(0, |t| name_tree_len(doc, &t))
                        + count_sub(b"FileAttachment")
                        + owned(Hidden::Attachments)
                }
                Hidden::Comments => annots().iter().filter(|a| is_comment(a.name(b"Subtype").unwrap_or(b""))).count(),
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
                        + annots().iter().filter(|a| !a.contains(b"AA") && a.name(b"Subtype") == Some(b"Widget") && a.contains(b"A")).count()
                        + sub(doc, names(), b"JavaScript").map_or(0, |t| name_tree_len(doc, &t))
                        + outline_action_count(doc, &cat)
                }
                Hidden::PrivateData => owned(Hidden::PrivateData),
            };
            (h, n)
        })
        .collect()
}

/// [`crate::globals::unshare`] for what left without marks: past `images` it is refused as finding
/// what shares the dictionaries is, since the error redaction gives (redact fewer pages at a time)
/// means nothing when no page was chosen.
pub(crate) fn unshare_unmarked(
    doc: &mut Document,
    before: &std::collections::HashSet<u32>,
    images: &mut pdfcraft_cos::ImageBudget,
) -> Result<usize, RedactError> {
    crate::globals::unshare(doc, before, images).map_err(|e| match e {
        RedactError::ImagesPastAllowance => RedactError::SharedJbig2Budget,
        e => e,
    })
}

/// Take from the outline of catalog `cat` every action that does more than go to a place in this
/// document (see [`outline_without_actions`]): bookmarks that stay keep where they go, not what
/// they run. An outline too large or nested too deep to follow is refused, not left half cleaned:
/// neither `cat` nor `doc` changes until the whole outline is cleaned.
fn strip_outline_actions(doc: &mut Document, cat: &mut Dict) -> Result<(), RedactError> {
    let (mut next, mut items) = (Vec::new(), 0);
    let root = match cat.get(b"Outlines") {
        Some(Object::Dict(o)) => {
            let mut o = o.clone();
            clean_outline_item(doc, &mut o, &mut next, &mut items, 0).ok_or(RedactError::TooDeep)?;
            Some(o)
        }
        Some(Object::Ref(r)) => {
            next.push(*r);
            None
        }
        _ => None,
    };
    let (objs, _) = outline_without_actions(doc, next, items).ok_or(RedactError::TooDeep)?;
    if let Some(o) = root {
        cat.set(b"Outlines".to_vec(), Object::Dict(o));
    }
    for (r, d) in objs {
        doc.set(r, Object::Dict(d));
    }
    Ok(())
}

/// Remove the chosen categories. Returns what was removed per category.
///
/// # Errors
///
/// This operation is not transactional: an error may leave `doc` partially modified, with
/// sensitive content still present. Call this on a working clone and replace the original only
/// on success. On any error, discard the working clone; do not save or reuse it.
pub fn remove_hidden(doc: &mut Document, which: &[Hidden]) -> Result<Vec<(Hidden, usize)>, RedactError> {
    // Only what the chosen categories need is counted: a full scan reads every page's content
    // twice and walks the whole object graph, which removing bookmarks alone doesn't need.
    let counts = scan_only(doc, which);
    remove_counted(doc, which, &counts, false)
}

/// Take off every page the annotations the chosen categories remove (with `fields`, the widgets
/// flattening kept), and the actions of those that stay when links, actions and scripts go.
fn drop_page_annotations(doc: &mut Document, which: &[Hidden], fields: bool) -> Result<(), RedactError> {
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
    let pages = pdfcraft_model::pages(doc);
    let removed: std::collections::HashSet<ObjRef> = pages
        .iter()
        .flat_map(|p| annots_of(doc, &p.dict))
        .filter_map(|a| a.as_ref())
        .filter(|&r| doc.get(r).as_dict().is_some_and(drop_annot))
        .collect();
    for p in pages {
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
    Ok(())
}

/// [`remove_hidden`] with `counts` from a scan of at least the categories in `which`. With
/// `every_page`, every page's annotation list is rewritten even when no chosen category changes
/// it, as removal did before it skipped that work; tests compare the two.
pub(crate) fn remove_counted(
    doc: &mut Document,
    which: &[Hidden],
    counts: &[(Hidden, usize)],
    every_page: bool,
) -> Result<Vec<(Hidden, usize)>, RedactError> {
    let count = |h: Hidden| counts.iter().find(|c| c.0 == h).map_or(0, |c| c.1);
    let (root, _) = catalog(doc).ok_or(RedactError::NothingToApply)?;
    let mut done = Vec::new();
    // What a full save keeps before anything is removed: an image that leaves, by whatever route
    // below, takes a shared JBIG2 dictionary with it (`unshare_unmarked`, last).
    let kept_before = crate::globals::kept(doc)?;
    let has_acroform = doc.get(root).as_dict().is_some_and(|d| d.contains(b"AcroForm"));
    let fields = which.contains(&Hidden::FormFields) && (count(Hidden::FormFields) > 0 || has_acroform);
    // Pages' annotation lists change only for these; otherwise every page is left as it is.
    let annotations = every_page || fields || which.iter().any(|h| matches!(h, Hidden::Comments | Hidden::Attachments | Hidden::LinksActionsScripts));
    // Every annotation the pages list now: whatever is no longer listed at the end (flattened
    // widgets included) leaves the structure tree too.
    let listed_before = if annotations { listed_annotations(doc) } else { std::collections::HashSet::new() };
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
    if annotations {
        drop_page_annotations(doc, which, fields)?;
        // A tagged PDF's structure tree still points at the removed links, comments and flattened
        // widgets (#2447): drop those references, or the full rewrite keeps the annotations, URLs,
        // text and field values included.
        let still = listed_annotations(doc);
        let gone: std::collections::HashSet<ObjRef> = listed_before.difference(&still).copied().collect();
        crate::tags::drop_object_refs(doc, &gone)?;
    }
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
                // Bookmarks that stay keep where they go, not what they run.
                if !which.contains(&Hidden::Bookmarks) {
                    strip_outline_actions(doc, &mut cat)?;
                }
            }
            Hidden::HiddenLayers if !off.is_empty() => {
                // The off layers leave the configuration (their content is gone already).
                if let Some(mut oc) = sub(doc, &cat, b"OCProperties") {
                    let keep = |o: &Object| !o.as_ref().is_some_and(|r| off.contains(&r) && !retained_off_layers.contains(&r));
                    if let Some(Object::Array(a)) = oc.get(b"OCGs").map(|g| (*doc.resolve(g)).clone()) {
                        oc.set(b"OCGs".to_vec(), Object::Array(a.into_iter().filter(keep).collect()));
                    }
                    let mut cleaned = Cleaned::new();
                    if let Some(d) = sub(doc, &oc, b"D") {
                        oc.set(b"D".to_vec(), Object::Dict(config_without(doc, d, &keep, &mut cleaned)?));
                    }
                    if let Some(Object::Array(configs)) = oc.get(b"Configs").map(|c| (*doc.resolve(c)).clone()) {
                        let mut kept = Vec::with_capacity(configs.len());
                        for config in configs {
                            kept.extend(once(doc, &config, &mut cleaned, |doc, config, cleaned| match config.as_dict() {
                                Some(c) => Ok(Some(Object::Dict(config_without(doc, c.clone(), &keep, cleaned)?))),
                                None => Ok(None),
                            })?);
                        }
                        oc.set(b"Configs".to_vec(), Object::Array(kept));
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
    // Images taken off their page (Delete Image, Replace Image) are still listed in its
    // resources: the full rewrite would keep them. Unlinking them is a removal of its own.
    let unlinked = crate::interp::unlink_undrawn(doc)?;
    if done.is_empty() && unlinked == 0 {
        return Err(RedactError::NothingToApply);
    }
    // Images stored again without a dictionary, and the dictionaries read to know copies, are
    // decoded within an allowance of their own.
    unshare_unmarked(doc, &kept_before, &mut pdfcraft_cos::ImageBudget::new(doc))?;
    // The rewrite drops the revisions that held the removed content, so it is a different file from
    // its source: it gets a new identifier, whoever asked for the removal (the remove-hidden edit
    // and the optimizer's discard option call this function directly, not through [`sanitize`]).
    doc.require_full_save_with_new_id();
    Ok(done)
}

/// Sanitize Document: remove every category of hidden information.
pub fn sanitize(doc: &mut Document) -> Result<Vec<(Hidden, usize)>, RedactError> {
    match remove_hidden(doc, &HIDDEN) {
        // Nothing hidden is fine for Sanitize: the full rewrite still drops old revisions. The
        // rewrite is a new file either way, so it asks for a new identifier itself (when something
        // was removed, `remove_hidden` has already asked).
        Err(RedactError::NothingToApply) => {
            doc.require_full_save_with_new_id();
            Ok(Vec::new())
        }
        r => r,
    }
}
