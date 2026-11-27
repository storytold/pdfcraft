//! Flatten Layers: what the default optional-content configuration shows becomes plain content,
//! and the layers go.
//!
//! [`flatten_layers`] reads each layer's state as the document opens on screen, as the renderer
//! shows it: the default configuration's `/BaseState`, `/ON` and `/OFF`, then its `/AS` usage
//! rules for the View event in the View category, which give each layer they name its
//! `/Usage /View /ViewState`, ON or OFF, whatever the lists say (ISO 32000-1 §8.11.4.4). Zoom,
//! User and Language usage give no state at open, and Print and Export usage is not read, so a
//! visible layer set not to print prints once merged. Then:
//!
//! - hidden layers' content is removed, as Remove Hidden Information removes it
//!   ([`crate::sanitize`]), and their marked blocks in every other content stream (forms and
//!   annotation appearances) go too;
//! - a visible layer's `/OC` marked block keeps what it draws and loses its `BDC` and `EMC`;
//! - an XObject in a hidden layer becomes an empty form; one in a visible layer loses `/OC`;
//! - an annotation in a hidden layer is taken off its page with its pop-up and the replies to it
//!   (`/IRT`), and a kept annotation's `/Popup` or `/IRT` naming one that left is dropped (a
//!   widget stays, flagged Hidden, so its field keeps working); one in a visible layer loses `/OC`;
//! - layers (dictionaries of `/Type /OCG`, and anything `/OCGs` lists) leave every resource
//!   `/Properties`, and the catalog loses `/OCProperties`, alternate configurations (`/Configs`)
//!   included.
//!
//! A hidden layer's block goes whole, graphics-state operators (`cm`, `gs`, colours) inside it
//! included, as Remove Hidden Information removes it. Content in no layer is left as it is, and a
//! layer `/OCGs` does not list is no layer (the reader ignores it): its blocks are unwrapped.
//!
//! Refusals leave the document as it was (the work runs on a copy that replaces it on success):
//!
//! - a membership dictionary (`/Type /OCMD`, and with it any `/VE` visibility expression), held
//!   by reference or inline in a property list or an annotation's `/OC`, makes visibility depend
//!   on several layers at once: [`RedactError::LayerMembership`];
//! - a page whose content can't be read whole: [`RedactError::Unreadable`] (its page number);
//! - a form or tiling pattern that can't be read whole while its resources can name a layer (its
//!   `/Properties` holds one, or it has no `/Resources` and may use its page's):
//!   [`RedactError::LayerContentUnreadable`] (its object number). One that can name no layer is
//!   left as it is, and still drawn;
//! - a form or tiling pattern without `/Resources` reads its layer names in the resources of what
//!   draws it (each resource dictionary whose `/XObject` or `/Pattern` names it). One drawn as an
//!   annotation's appearance, where no such resources are given, or whose names mean a hidden
//!   layer in one place and not in another, is refused with the same error.

use std::collections::{HashMap, HashSet};

use pdfcraft_content::{Op, Pieces, serialize_ops};
use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object, Stream};

use crate::sanitize::{LayerPass, ListedStates, catalog, content_pass_with, listed_states, sub, view_states};
use crate::{RedactError, annots_of};

/// What [`flatten_layers`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flattened {
    /// Layers listed in `/OCGs` that were visible and are now plain content.
    pub visible: usize,
    /// Layers that were hidden, whose content was removed.
    pub hidden: usize,
}

/// The annotation flag that hides a widget kept from a hidden layer (ISO 32000-2 §12.5.3).
const HIDDEN_FLAG: i64 = 2;

/// Merges the visible layers into plain content and removes the hidden ones (module docs).
/// A document without `/OCProperties` is left as it is.
///
/// # Errors
///
/// On any error `doc` is left as it was: the work runs on a copy (cheap: unchanged objects are
/// shared) that replaces `doc` only on success.
pub fn flatten_layers(doc: &mut Document) -> Result<Flattened, RedactError> {
    let mut work = doc.clone();
    let done = flatten(&mut work)?;
    *doc = work;
    Ok(done)
}

fn flatten(doc: &mut Document) -> Result<Flattened, RedactError> {
    let Some((root, cat)) = catalog(doc) else { return Ok(Flattened::default()) };
    let Some(oc) = sub(doc, &cat, b"OCProperties") else { return Ok(Flattened::default()) };
    refuse_memberships(doc)?;
    let hidden = hidden_at_open(doc);
    let listed: HashSet<ObjRef> =
        oc.get(b"OCGs").and_then(|g| doc.resolve(g).as_array().map(|a| a.iter().filter_map(Object::as_ref).collect())).unwrap_or_default();
    let done = Flattened { visible: listed.difference(&hidden).count(), hidden: listed.intersection(&hidden).count() };
    if !hidden.is_empty() {
        // The forms it can't read stay drawn: `unwrap_forms` refuses those that could name a layer.
        content_pass_with(doc, false, Some(LayerPass { off: hidden.iter().copied().collect(), keep_unreadable_forms: true }), true)?;
    }
    clear_xobjects(doc, &hidden)?;
    unwrap_pages(doc, &hidden)?;
    unwrap_forms(doc, &hidden, &listed)?;
    clear_annotations(doc, &hidden)?;
    drop_layer_properties(doc, &listed)?;
    doc.update_dict(root, |d| {
        d.remove(b"OCProperties");
    })?;
    doc.require_full_save_with_new_id();
    Ok(done)
}

/// The layers hidden as the document opens (module docs): those the lists leave off, then each
/// one a View usage rule names takes its ViewState.
fn hidden_at_open(doc: &Document) -> HashSet<ObjRef> {
    let Some(ListedStates { all, hidden, d }) = listed_states(doc) else { return HashSet::new() };
    let mut hidden: HashSet<ObjRef> = hidden.into_iter().collect();
    for (g, on) in view_states(doc, &d, &all, |_| true) {
        if on {
            hidden.remove(&g);
        } else {
            hidden.insert(g);
        }
    }
    hidden
}

/// Is `o`, followed through references, a dictionary whose `/Type` is `name`?
fn is_type(doc: &Document, o: &Object, name: &[u8]) -> bool {
    let o = crate::resolve_chain(doc, o);
    let d = match &*o {
        Object::Dict(d) => d,
        Object::Stream(s) => &s.dict,
        _ => return false,
    };
    d.get(b"Type").is_some_and(|t| crate::resolve_chain(doc, t).as_name() == Some(name))
}

/// Is `o` a layer: a dictionary of `/Type /OCG`, or one `/OCGs` lists?
fn is_layer(doc: &Document, o: &Object, listed: &HashSet<ObjRef>) -> bool {
    o.as_ref().is_some_and(|r| listed.contains(&r)) || is_type(doc, o, b"OCG")
}

/// Refuses a document holding a membership dictionary as an object of its own, early (direct
/// ones are refused where they are met).
fn refuse_memberships(doc: &Document) -> Result<(), RedactError> {
    for n in doc.object_numbers() {
        let o = doc.get(ObjRef::new(n, doc.generation(n)));
        if matches!(&*o, Object::Dict(_)) && is_type(doc, &o, b"OCMD") {
            return Err(RedactError::LayerMembership);
        }
    }
    Ok(())
}

/// Whether an `/OC` value names a hidden layer; a membership dictionary is refused.
fn layer_state(doc: &Document, o: &Object, hidden: &HashSet<ObjRef>) -> Result<bool, RedactError> {
    if is_type(doc, o, b"OCMD") {
        return Err(RedactError::LayerMembership);
    }
    Ok(o.as_ref().is_some_and(|r| hidden.contains(&r)))
}

/// What a marked-content operator opens.
enum Block {
    /// A layer that is shown: the operator and its `EMC` go, what it draws stays.
    Shown,
    /// A layer that is hidden: everything up to its `EMC` goes.
    Hidden,
    /// Any other marked content, kept.
    Other,
}

fn block(doc: &Document, op: &Op, props: &Dict, hidden: &HashSet<ObjRef>) -> Result<Block, RedactError> {
    // The tag and property list are the last two operands, as the renderer reads them.
    if !op.is("BDC") || op.name_back(1) != Some(b"OC") {
        return Ok(Block::Other);
    }
    let target = match op.back(0) {
        Some(Object::Name(n)) => props.get(n).cloned(),
        o => o.cloned(),
    };
    // A name no `/Properties` holds names no layer a reader can find: it shows its content.
    let Some(target) = target else { return Ok(Block::Shown) };
    Ok(if layer_state(doc, &target, hidden)? { Block::Hidden } else { Block::Shown })
}

/// `ops` without their layers' marked blocks: shown ones unwrapped, hidden ones removed. `None`
/// when they hold no layer's block.
fn unwrap_ops(doc: &Document, ops: Vec<Op>, props: &Dict, hidden: &HashSet<ObjRef>) -> Result<Option<Vec<Op>>, RedactError> {
    let mut out = Vec::with_capacity(ops.len());
    // For each open block: is it a shown layer's, whose EMC goes with it?
    let mut open: Vec<bool> = Vec::new();
    // How deep inside a hidden layer's block the walk is.
    let mut dropping = 0usize;
    let mut changed = false;
    for op in ops {
        let begins = op.is("BDC") || op.is("BMC");
        if dropping > 0 {
            if begins {
                dropping = dropping.saturating_add(1);
            } else if op.is("EMC") {
                dropping -= 1;
            }
            continue;
        }
        if begins {
            match block(doc, &op, props, hidden)? {
                Block::Hidden => {
                    dropping = 1;
                    changed = true;
                    continue;
                }
                Block::Shown => {
                    open.push(true);
                    changed = true;
                    continue;
                }
                Block::Other => open.push(false),
            }
        } else if op.is("EMC") && open.pop() == Some(true) {
            continue;
        }
        out.push(op);
    }
    Ok(changed.then_some(out))
}

/// A resource dictionary's `/Properties`.
fn properties(doc: &Document, resources: Option<&Object>) -> Dict {
    resources.and_then(|r| doc.resolve(r).as_dict().cloned()).and_then(|r| sub(doc, &r, b"Properties")).unwrap_or_default()
}

/// Each page's content with its layers' blocks unwrapped or removed, stored as one new stream.
fn unwrap_pages(doc: &mut Document, hidden: &HashSet<ObjRef>) -> Result<(), RedactError> {
    for (pi, page) in pdfcraft_model::pages(doc).into_iter().enumerate() {
        let (_, data) = crate::page_streams(doc, &page.dict, pi)?;
        // The pieces are one content stream, which may be split between any two tokens.
        let parsed = Pieces::join(&data).parse();
        if parsed.skipped > 0 || parsed.ambiguous_images > 0 {
            return Err(RedactError::Unreadable(pi + 1));
        }
        let props = properties(doc, page.dict.get(b"Resources"));
        let Some(ops) = unwrap_ops(doc, parsed.ops, &props, hidden)? else { continue };
        let content = doc.add(Object::Stream(Stream::flate(Dict::new(), &serialize_ops(&ops))));
        doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Ref(content)))?;
    }
    Ok(())
}

/// Is this stream content of its own: a form XObject (annotation appearances included) or a
/// tiling pattern?
fn is_content_stream(doc: &Document, d: &Dict) -> bool {
    let name = |k: &[u8]| d.get(k).map(|v| crate::resolve_chain(doc, v));
    name(b"Subtype").is_some_and(|s| s.as_name() == Some(b"Form")) || name(b"PatternType").is_some_and(|p| p.as_int() == Some(1))
}

/// Can this form's content name a layer? Only through its `/Properties`; a form without
/// `/Resources` may use its page's, so it can.
fn may_name_layers(doc: &Document, d: &Dict, listed: &HashSet<ObjRef>) -> bool {
    let Some(res) = d.get(b"Resources") else { return true };
    properties(doc, Some(res)).iter().any(|(_, v)| is_layer(doc, v, listed) || is_type(doc, v, b"OCMD"))
}

/// Every form and tiling pattern with its layers' blocks unwrapped or removed.
fn unwrap_forms(doc: &mut Document, hidden: &HashSet<ObjRef>, listed: &HashSet<ObjRef>) -> Result<(), RedactError> {
    let mut drawers = Drawers::read(doc);
    for n in doc.object_numbers() {
        let r = ObjRef::new(n, doc.generation(n));
        let Object::Stream(s) = &*doc.get(r) else { continue };
        if !is_content_stream(doc, &s.dict) {
            continue;
        }
        let parsed = DecodeBudget::page().decode_strict(doc, s).ok().map(|data| pdfcraft_content::parse(&data));
        let Some(parsed) = parsed.filter(|p| p.skipped == 0 && p.ambiguous_images == 0) else {
            if may_name_layers(doc, &s.dict, listed) {
                return Err(RedactError::LayerContentUnreadable(n));
            }
            continue;
        };
        let props = match s.dict.get(b"Resources") {
            Some(res) => properties(doc, Some(res)),
            None => drawers.properties(doc, r, &parsed.ops, hidden)?,
        };
        let Some(ops) = unwrap_ops(doc, parsed.ops, &props, hidden)? else { continue };
        let mut dict = s.dict.clone();
        dict.remove(b"Length");
        doc.set_existing(r, Object::Stream(Stream::flate(dict, &serialize_ops(&ops))))?;
    }
    Ok(())
}

/// Most layer names [`Drawers::properties`] looks up, over every form it reads, before it refuses.
const MAX_NAME_LOOKUPS: usize = 1 << 20;

/// What draws the forms and tiling patterns that have no `/Resources` of their own.
struct Drawers {
    /// The `/Properties` of each resource dictionary that names one, by the one it names.
    by: HashMap<ObjRef, Vec<Dict>>,
    /// Those drawn as an annotation's appearance.
    appearances: HashSet<ObjRef>,
    lookups: usize,
}

impl Drawers {
    fn read(doc: &Document) -> Self {
        let resourceless = |r: ObjRef| matches!(&*doc.get(r), Object::Stream(s) if !s.dict.contains(b"Resources") && is_content_stream(doc, &s.dict));
        let mut by: HashMap<ObjRef, Vec<Dict>> = HashMap::new();
        let mut seen = HashSet::new();
        for n in doc.object_numbers() {
            let o = doc.get(ObjRef::new(n, doc.generation(n)));
            let held = match &*o {
                Object::Dict(d) => d.get(b"Resources"),
                Object::Stream(s) => s.dict.get(b"Resources"),
                _ => None,
            };
            // A resource dictionary many owners share is read once.
            let Some(res) = held.filter(|r| r.as_ref().is_none_or(|r| seen.insert(r))) else { continue };
            let Some(res) = doc.resolve(res).as_dict().cloned() else { continue };
            let props = sub(doc, &res, b"Properties").unwrap_or_default();
            for key in [&b"XObject"[..], b"Pattern"] {
                for (_, v) in sub(doc, &res, key).unwrap_or_default().iter() {
                    if let Some(t) = v.as_ref().filter(|&t| resourceless(t)) {
                        by.entry(t).or_default().push(props.clone());
                    }
                }
            }
        }
        Drawers { by, appearances: appearances(doc), lookups: 0 }
    }

    /// The `/Properties` the layer names in `ops`, the content of `r` (which has no `/Resources`),
    /// mean where it is drawn: a name means a hidden layer if it does in every resource
    /// dictionary naming `r`, and a shown one (absent here) if it does in none. A form nothing
    /// names, which nothing draws, reads none.
    fn properties(&mut self, doc: &Document, r: ObjRef, ops: &[Op], hidden: &HashSet<ObjRef>) -> Result<Dict, RedactError> {
        let refused = RedactError::LayerContentUnreadable(r.num);
        let names: HashSet<&[u8]> =
            ops.iter().filter(|op| op.is("BDC") && op.name_back(1) == Some(b"OC")).filter_map(|op| op.back(0).and_then(Object::as_name)).collect();
        if names.is_empty() {
            return Ok(Dict::new());
        }
        if self.appearances.contains(&r) {
            return Err(refused);
        }
        let callers = self.by.get(&r).map_or(&[][..], Vec::as_slice);
        let mut out = Dict::new();
        for name in names {
            let mut first: Option<(bool, Option<&Object>)> = None;
            for props in callers {
                self.lookups = self.lookups.saturating_add(1);
                if self.lookups > MAX_NAME_LOOKUPS {
                    return Err(refused);
                }
                let target = props.get(name);
                let off = match target {
                    Some(t) => layer_state(doc, t, hidden)?,
                    None => false,
                };
                match first {
                    None => first = Some((off, target)),
                    Some((was, _)) if was != off => return Err(refused),
                    Some(_) => {}
                }
            }
            if let Some((true, Some(t))) = first {
                out.set(name.to_vec(), t.clone());
            }
        }
        Ok(out)
    }
}

/// The streams the pages' annotations draw as appearances (`/AP` entries, and their states).
fn appearances(doc: &Document) -> HashSet<ObjRef> {
    let mut out = HashSet::new();
    for p in pdfcraft_model::pages(doc) {
        for a in annots_of(doc, &p.dict) {
            let ap = doc.resolve(&a).as_dict().and_then(|d| sub(doc, d, b"AP")).unwrap_or_default();
            for (_, v) in ap.iter() {
                match &*doc.resolve(v) {
                    Object::Dict(states) => out.extend(states.iter().filter_map(|(_, s)| s.as_ref())),
                    _ => out.extend(v.as_ref()),
                }
            }
        }
    }
    out
}

/// XObjects in a hidden layer become empty forms, wherever they are drawn; the others lose `/OC`.
fn clear_xobjects(doc: &mut Document, hidden: &HashSet<ObjRef>) -> Result<(), RedactError> {
    for n in doc.object_numbers() {
        let r = ObjRef::new(n, doc.generation(n));
        let Object::Stream(s) = &*doc.get(r) else { continue };
        let Some(oc) = s.dict.get(b"OC") else { continue };
        if layer_state(doc, oc, hidden)? {
            let empty = Dict::from_iter([
                (b"Type".to_vec(), Object::name("XObject")),
                (b"Subtype".to_vec(), Object::name("Form")),
                (b"BBox".to_vec(), Object::Array(vec![Object::Int(0); 4])),
            ]);
            doc.set_existing(r, Object::Stream(Stream::flate(empty, b"")))?;
        } else {
            let mut s = s.clone();
            s.dict.remove(b"OC");
            doc.set_existing(r, Object::Stream(s))?;
        }
    }
    Ok(())
}

/// What an annotation hangs from: the one it replies to (`/IRT`) and, for a pop-up, its `/Parent`.
fn hangs_from(d: &Dict) -> impl Iterator<Item = ObjRef> + '_ {
    let parent = (d.name(b"Subtype") == Some(b"Popup")).then(|| d.get(b"Parent")).flatten();
    [d.get(b"IRT"), parent].into_iter().flatten().filter_map(Object::as_ref)
}

/// The annotations, by reference, that leave their pages: those in a hidden layer (widgets
/// excepted), and the pop-ups and replies hanging from one that leaves, however deep.
fn leaving(doc: &Document, pages: &[pdfcraft_model::Page], hidden: &HashSet<ObjRef>) -> Result<HashSet<ObjRef>, RedactError> {
    let mut gone = HashSet::new();
    let mut hanging: HashMap<ObjRef, Vec<ObjRef>> = HashMap::new();
    for p in pages {
        for a in annots_of(doc, &p.dict) {
            let Some(r) = a.as_ref() else { continue };
            let Some(d) = doc.get(r).as_dict().cloned() else { continue };
            if d.name(b"Subtype") == Some(b"Widget") {
                continue;
            }
            if let Some(oc) = d.get(b"OC")
                && layer_state(doc, oc, hidden)?
            {
                gone.insert(r);
            }
            for t in hangs_from(&d) {
                hanging.entry(t).or_default().push(r);
            }
        }
    }
    let mut todo: Vec<ObjRef> = gone.iter().copied().collect();
    while let Some(t) = todo.pop() {
        for r in hanging.remove(&t).unwrap_or_default() {
            if gone.insert(r) {
                todo.push(r);
            }
        }
    }
    Ok(gone)
}

/// Annotations in a hidden layer leave their page (a widget stays, flagged Hidden), with the
/// pop-ups and replies of those that leave; the others lose `/OC`, and any `/Popup` or `/IRT`
/// naming one that left.
fn clear_annotations(doc: &mut Document, hidden: &HashSet<ObjRef>) -> Result<(), RedactError> {
    let pages = pdfcraft_model::pages(doc);
    let gone = leaving(doc, &pages, hidden)?;
    for p in pages {
        let mut kept = Vec::new();
        for a in annots_of(doc, &p.dict) {
            let Some(mut d) = doc.resolve(&a).as_dict().cloned() else {
                kept.push(a);
                continue;
            };
            let widget = d.name(b"Subtype") == Some(b"Widget");
            if a.as_ref().is_some_and(|r| gone.contains(&r)) || (!widget && hangs_from(&d).any(|t| gone.contains(&t))) {
                continue;
            }
            let mut changed = false;
            for key in [&b"Popup"[..], b"IRT"] {
                if d.get(key).and_then(Object::as_ref).is_some_and(|t| gone.contains(&t)) {
                    d.remove(key);
                    changed = true;
                }
            }
            if let Some(oc) = d.remove(b"OC") {
                changed = true;
                if layer_state(doc, &oc, hidden)? {
                    // A direct annotation has no reference to leave by: a hidden one goes here.
                    if !widget {
                        continue;
                    }
                    d.set(b"F".to_vec(), Object::Int(d.int(b"F").unwrap_or(0) | HIDDEN_FLAG));
                }
            }
            match a.as_ref() {
                Some(r) if changed => {
                    doc.set_existing(r, Object::Dict(d))?;
                    kept.push(a);
                }
                Some(_) => kept.push(a),
                None => kept.push(Object::Dict(d)),
            }
        }
        doc.update_dict(p.obj, |d| {
            if kept.is_empty() {
                d.remove(b"Annots");
            } else {
                d.set(b"Annots".to_vec(), Object::Array(kept));
            }
        })?;
    }
    // Tags must not keep what left the pages.
    crate::tags::drop_object_refs(doc, &gone)?;
    Ok(())
}

/// `props` without its layers, or `None` when it names none.
fn without_layers(doc: &Document, props: &Dict, listed: &HashSet<ObjRef>) -> Option<Dict> {
    let kept: Dict = props.iter().filter(|(_, v)| !is_layer(doc, v, listed)).map(|(k, v)| (k.clone(), v.clone())).collect();
    (kept.len() != props.len()).then_some(kept)
}

/// `res` (a resource dictionary, direct) without layers in its `/Properties`: a `/Properties` held
/// by reference is cleaned where it is, and `None` says `res` itself needs no change.
fn resources_without_layers(
    doc: &mut Document,
    res: &Dict,
    listed: &HashSet<ObjRef>,
    done: &mut HashSet<ObjRef>,
) -> Result<Option<Dict>, RedactError> {
    match res.get(b"Properties") {
        Some(Object::Ref(p)) => {
            let p = *p;
            if done.insert(p)
                && let Some(kept) = doc.get(p).as_dict().and_then(|d| without_layers(doc, d, listed))
            {
                doc.set_existing(p, Object::Dict(kept))?;
            }
            Ok(None)
        }
        Some(Object::Dict(props)) => Ok(without_layers(doc, props, listed).map(|kept| {
            let mut res = res.clone();
            res.set(b"Properties".to_vec(), Object::Dict(kept));
            res
        })),
        _ => Ok(None),
    }
}

/// Every resource dictionary without the layers its `/Properties` names: pages, page-tree nodes,
/// forms, patterns, fonts, whatever holds `/Resources`.
fn drop_layer_properties(doc: &mut Document, listed: &HashSet<ObjRef>) -> Result<(), RedactError> {
    let mut done = HashSet::new();
    for n in doc.object_numbers() {
        let r = ObjRef::new(n, doc.generation(n));
        let mut owner = (*doc.get(r)).clone();
        let dict = match &mut owner {
            Object::Dict(d) => d,
            Object::Stream(s) => &mut s.dict,
            _ => continue,
        };
        match dict.get(b"Resources").cloned() {
            Some(Object::Ref(res)) => {
                if !done.insert(res) {
                    continue;
                }
                let Some(held) = doc.get(res).as_dict().cloned() else { continue };
                if let Some(new) = resources_without_layers(doc, &held, listed, &mut done)? {
                    doc.set_existing(res, Object::Dict(new))?;
                }
            }
            Some(Object::Dict(held)) => {
                if let Some(new) = resources_without_layers(doc, &held, listed, &mut done)? {
                    dict.set(b"Resources".to_vec(), Object::Dict(new));
                    doc.set_existing(r, owner)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
