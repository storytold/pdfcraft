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
//! - hidden layers' content is removed by Remove Hidden Information's page pass
//!   ([`crate::sanitize`]), and their marked blocks in every other content stream (forms,
//!   annotation appearances, tiling patterns, soft-mask groups and Type 3 glyphs) go too;
//! - a visible layer's `/OC` marked block keeps what it draws and loses its `BDC` and `EMC`;
//! - an XObject in a hidden layer becomes an empty form; one in a visible layer loses `/OC`;
//! - an annotation in a hidden layer is taken off its page with its pop-up and the replies to it
//!   (`/IRT`), and a kept annotation's `/Popup` or `/IRT` naming one that left is dropped (a
//!   widget stays, flagged Hidden, so its field keeps working); one in a visible layer loses `/OC`;
//! - layers (dictionaries of `/Type /OCG`, and anything `/OCGs` lists) leave every resource
//!   `/Properties`, and the catalog loses `/OCProperties`, alternate configurations (`/Configs`)
//!   included.
//!
//! A hidden layer's block loses what it draws and keeps what outlives it: graphics and text
//! state, clipping and text position (ISO 32000-1 §8.11.3.1, [`crate::hidden_block`]). Remove
//! Hidden Information itself still removes such a block whole; only this pass keeps the rest. On a page
//! the text a hidden block showed becomes the move its glyphs made; elsewhere, where fonts aren't
//! read, a document whose hidden text sets where later text on the line goes is refused. Content
//! in no layer is left as it is, and a layer `/OCGs` does not list is no layer (the reader
//! ignores it): its blocks are unwrapped.
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
//! - a content stream without `/Resources` reads its layer names in the resources of what draws
//!   it ([`Drawers`]): each resource dictionary whose `/XObject` or `/Pattern` names it, whose
//!   `/ExtGState` (or a shading pattern's own `/ExtGState`) holds it as a soft mask's group
//!   (`/SMask /G`), or, for a Type 3 glyph whose font has no `/Resources`, whose `/Font` names
//!   that font, and any resource dictionary at all that holds the name (text state, and so the
//!   font, passes into what is drawn after `Tf`). A name none of these holds is refused with the
//!   same error, never read as shown: the stream may be drawn some other way. So is one drawn as
//!   an annotation's appearance, where no such resources are given, and one whose names mean a
//!   hidden layer in one place and not in another;
//! - last, the result is checked whatever drew it: a stream a full save writes that may still hold
//!   a layer's block, read as far as a reader reads it, is refused with the same error, since the
//!   layers are gone and a reader would show it; so is one that can't be read at all, unless what
//!   it is says no reader draws it ([`refuse_layer_blocks_left`]).

use std::collections::{HashMap, HashSet};

use pdfcraft_content::{Op, Pieces, serialize_ops};
use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object, Stream};

use left::refuse_layer_blocks_left;

use crate::hidden_block::{Residue, is_show};
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
    // Read before the page pass, which drops the resources a page's own content no longer names.
    let drawers = Drawers::read(doc);
    if !hidden.is_empty() {
        // The forms it can't read stay drawn: `unwrap_forms` refuses those that could name a layer.
        content_pass_with(
            doc,
            false,
            Some(LayerPass { off: hidden.iter().copied().collect(), keep_unreadable_forms: true, keep_hidden_state: true }),
            true,
        )?;
    }
    clear_xobjects(doc, &hidden)?;
    unwrap_pages(doc, &hidden)?;
    unwrap_forms(doc, drawers, &hidden, &listed)?;
    clear_annotations(doc, &hidden)?;
    drop_layer_properties(doc, &listed)?;
    doc.update_dict(root, |d| {
        d.remove(b"OCProperties");
    })?;
    refuse_layer_blocks_left(doc)?;
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
    match &*o {
        Object::Dict(d) => has_name(doc, d, b"Type", name),
        Object::Stream(s) => has_name(doc, &s.dict, b"Type", name),
        _ => false,
    }
}

/// Is `d`'s `key`, followed through references, the name `name`?
fn has_name(doc: &Document, d: &Dict, key: &[u8], name: &[u8]) -> bool {
    d.get(key).is_some_and(|t| crate::resolve_chain(doc, t).as_name() == Some(name))
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

/// Is `op` a layer's marked-content block: `/OC` with a name or property list, then `BDC`?
fn opens_layer_block(op: &Op) -> bool {
    op.is("BDC") && op.name_back(1) == Some(b"OC") && matches!(op.back(0), Some(Object::Name(_) | Object::Dict(_)))
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

/// What [`unwrap_ops`] made of a stream.
enum Unwrapped {
    /// It holds no layer's block.
    Same,
    New(Vec<Op>),
    /// Text a hidden block showed sets where a string after it is shown, a move this pass can't
    /// measure (it doesn't read fonts).
    MovesText,
}

/// Whether a string a hidden block showed still sets where the next one goes: a string moves the
/// text position, and only a new line or text object starts it afresh. Inside a text object `q`
/// and `Q` save and restore the text matrices (ISO 32000-2 §9.4.1 and §8.4.2, as amended by
/// erratum 368), so `Q` brings back a debt the `q` saved, and keeps any debt incurred since its
/// `q`. `saved` is not cleared at `BT` or `ET`, so a `Q` after `ET` popping a `q` from inside the
/// text object may set a debt that isn't there: it over-refuses, never under.
#[derive(Default)]
struct TextRun {
    open: bool,
    owed: bool,
    saved: Vec<bool>,
}

impl TextRun {
    /// Takes the next operator, in a hidden block or not; `false` for a shown string that would
    /// land where a removed one left the text position.
    fn step(&mut self, op: &Op, hidden: bool) -> bool {
        match op.op.as_slice() {
            b"BT" | b"ET" => {
                self.open = op.is("BT");
                self.owed = false;
            }
            b"Td" | b"TD" | b"Tm" | b"T*" => self.owed = false,
            b"q" => self.saved.push(self.owed),
            b"Q" => self.owed |= self.saved.pop().unwrap_or(false),
            b"Tj" | b"TJ" if !hidden => return !self.owed,
            b"'" | b"\"" if !hidden => self.owed = false,
            o if hidden && is_show(o) => self.owed = self.open,
            _ => {}
        }
        true
    }
}

/// `ops` without their layers' marked blocks: shown ones unwrapped, hidden ones reduced to what
/// outlives them ([`crate::hidden_block`]).
fn unwrap_ops(doc: &Document, ops: Vec<Op>, props: &Dict, hidden: &HashSet<ObjRef>) -> Result<Unwrapped, RedactError> {
    let mut out = Vec::with_capacity(ops.len());
    // For each open block: is it a shown layer's, whose EMC goes with it?
    let mut open: Vec<bool> = Vec::new();
    // How deep inside a hidden layer's block the walk is.
    let mut dropping = 0usize;
    let mut residue = Residue::default();
    let mut text = TextRun::default();
    let mut changed = false;
    for op in ops {
        if !text.step(&op, dropping > 0) {
            return Ok(Unwrapped::MovesText);
        }
        let begins = op.is("BDC") || op.is("BMC");
        if dropping > 0 {
            if begins {
                dropping = dropping.saturating_add(1);
            } else if op.is("EMC") {
                dropping -= 1;
                if dropping == 0 {
                    out.append(&mut std::mem::take(&mut residue).finish());
                }
            } else {
                residue.push(op, false);
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
    // A hidden block the stream doesn't close.
    out.append(&mut residue.finish());
    Ok(if changed { Unwrapped::New(out) } else { Unwrapped::Same })
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
        let ops = match unwrap_ops(doc, parsed.ops, &props, hidden)? {
            Unwrapped::Same => continue,
            Unwrapped::New(ops) => ops,
            Unwrapped::MovesText => return Err(RedactError::LayerContentUnreadable(page.obj.num)),
        };
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

/// Every form, tiling pattern and Type 3 glyph with its layers' blocks unwrapped or removed.
fn unwrap_forms(doc: &mut Document, mut drawers: Drawers, hidden: &HashSet<ObjRef>, listed: &HashSet<ObjRef>) -> Result<(), RedactError> {
    for n in doc.object_numbers() {
        let r = ObjRef::new(n, doc.generation(n));
        let Object::Stream(s) = &*doc.get(r) else { continue };
        if !is_content_stream(doc, &s.dict) && !drawers.glyphs.contains(&r) {
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
        let ops = match unwrap_ops(doc, parsed.ops, &props, hidden)? {
            Unwrapped::Same => continue,
            Unwrapped::New(ops) => ops,
            Unwrapped::MovesText => return Err(RedactError::LayerContentUnreadable(n)),
        };
        let mut dict = s.dict.clone();
        dict.remove(b"Length");
        doc.set_existing(r, Object::Stream(Stream::flate(dict, &serialize_ops(&ops))))?;
    }
    Ok(())
}

/// Most layer names [`Drawers::properties`] looks up, over every form it reads, before it refuses.
const MAX_NAME_LOOKUPS: usize = 1 << 20;

/// What draws each content stream that has no `/Resources` of its own (module docs): one
/// mechanism for every way a stream is drawn, so a name in it is read where it is drawn. A name
/// no indexed caller holds is refused, never read as shown: a stream may be drawn some way the
/// index doesn't follow.
struct Drawers {
    /// The resource dictionaries each one is drawn with, by the one drawn.
    by: HashMap<ObjRef, Vec<Caller>>,
    /// Type 3 glyph programs: content streams their dictionaries don't mark as such.
    glyphs: HashSet<ObjRef>,
    /// Glyphs of a Type 3 font without `/Resources`: text state passes into the forms and
    /// patterns drawn after `Tf`, so one may be shown under any resource dictionary.
    inherits: HashSet<ObjRef>,
    /// The `/Properties` of every resource dictionary that has one, for [`Drawers::inherits`].
    anywhere: Vec<Dict>,
    /// Those drawn as an annotation's appearance.
    appearances: HashSet<ObjRef>,
    lookups: usize,
}

/// A resource dictionary's `/Properties`, as a stream without `/Resources` drawn with it reads them.
struct Caller {
    props: Dict,
    /// The `/Resources` of the glyph's own font, which it always reads: a name absent there names
    /// no layer, and shows.
    own: bool,
}

/// Is `r` a form or tiling pattern without `/Resources`?
fn resourceless(doc: &Document, r: ObjRef) -> bool {
    matches!(&*doc.get(r), Object::Stream(s) if !s.dict.contains(b"Resources") && is_content_stream(doc, &s.dict))
}

/// The group (`/G`) of the soft mask an ExtGState `gs` sets, if any.
fn soft_mask_group(doc: &Document, gs: &Object) -> Option<ObjRef> {
    let gs = doc.resolve(gs).as_dict()?.clone();
    sub(doc, &gs, b"SMask")?.get(b"G")?.as_ref()
}

/// The soft-mask group a shading pattern's own `/ExtGState` sets, if any.
fn shading_mask_group(doc: &Document, pattern: &Object) -> Option<ObjRef> {
    let p = doc.resolve(pattern).as_dict()?.clone();
    if p.get(b"PatternType").map(|t| crate::resolve_chain(doc, t).as_int()) != Some(Some(2)) {
        return None;
    }
    soft_mask_group(doc, p.get(b"ExtGState")?)
}

impl Drawers {
    fn read(doc: &Document) -> Self {
        let mut drawers = Drawers {
            by: HashMap::new(),
            glyphs: HashSet::new(),
            inherits: HashSet::new(),
            anywhere: Vec::new(),
            appearances: appearances(doc),
            lookups: 0,
        };
        let (mut seen, mut fonts) = (HashSet::new(), HashSet::new());
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
            drawers.index(doc, &res, &mut fonts);
        }
        drawers
    }

    /// Records what `res` draws: the forms and tiling patterns its `/XObject` and `/Pattern` name,
    /// the soft-mask groups its `/ExtGState` and its shading patterns' own `/ExtGState` set, and
    /// its Type 3 fonts' glyphs (each font with `/Resources` of its own, in `fonts`, once).
    fn index(&mut self, doc: &Document, res: &Dict, fonts: &mut HashSet<ObjRef>) {
        let props = sub(doc, res, b"Properties").unwrap_or_default();
        let entries = |key: &[u8]| sub(doc, res, key).unwrap_or_default();
        let mut drawn: Vec<ObjRef> = Vec::new();
        for key in [&b"XObject"[..], b"Pattern"] {
            drawn.extend(entries(key).iter().filter_map(|(_, v)| v.as_ref()));
        }
        drawn.extend(entries(b"ExtGState").iter().filter_map(|(_, gs)| soft_mask_group(doc, gs)));
        drawn.extend(entries(b"Pattern").iter().filter_map(|(_, p)| shading_mask_group(doc, p)));
        for t in drawn.into_iter().filter(|&t| resourceless(doc, t)) {
            self.by.entry(t).or_default().push(Caller { props: props.clone(), own: false });
        }
        for (_, f) in entries(b"Font").iter() {
            let Some(font) = doc.resolve(f).as_dict().cloned().filter(|d| has_name(doc, d, b"Subtype", b"Type3")) else { continue };
            // Its glyphs read names in its own `/Resources`, or else in what shows them.
            let (reads, own) = match font.get(b"Resources") {
                Some(_) if f.as_ref().is_some_and(|r| !fonts.insert(r)) => continue,
                Some(own) => (properties(doc, Some(own)), true),
                None => (props.clone(), false),
            };
            for g in sub(doc, &font, b"CharProcs").unwrap_or_default().iter().filter_map(|(_, g)| g.as_ref()) {
                self.glyphs.insert(g);
                if !own {
                    self.inherits.insert(g);
                }
                self.by.entry(g).or_default().push(Caller { props: reads.clone(), own });
            }
        }
        if !props.is_empty() {
            self.anywhere.push(props);
        }
    }

    /// The `/Properties` the layer names in `ops`, the content of `r` (which has no `/Resources`),
    /// mean where it is drawn ([`Drawers::hidden_target`]). A stream drawn as an annotation's
    /// appearance, where no resources are given, is refused.
    fn properties(&mut self, doc: &Document, r: ObjRef, ops: &[Op], hidden: &HashSet<ObjRef>) -> Result<Dict, RedactError> {
        let names: HashSet<&[u8]> =
            ops.iter().filter(|op| op.is("BDC") && op.name_back(1) == Some(b"OC")).filter_map(|op| op.back(0).and_then(Object::as_name)).collect();
        if names.is_empty() {
            return Ok(Dict::new());
        }
        if self.appearances.contains(&r) {
            return Err(RedactError::LayerContentUnreadable(r.num));
        }
        let mut out = Dict::new();
        for name in names {
            if let Some(t) = self.hidden_target(doc, r, name, hidden)? {
                out.set(name.to_vec(), t);
            }
        }
        Ok(out)
    }

    /// The hidden layer `name` means in `r`, or `None` for a shown one. Each caller answers: a
    /// name it holds means what it names, and one it lacks shows, except in a resource dictionary
    /// a glyph may only inherit, where a lack says nothing. Callers that disagree are refused, and
    /// so is a name none holds, unless every caller is the glyph's own font resources.
    fn hidden_target(&mut self, doc: &Document, r: ObjRef, name: &[u8], hidden: &HashSet<ObjRef>) -> Result<Option<Object>, RedactError> {
        let refused = RedactError::LayerContentUnreadable(r.num);
        let callers = self.by.get(&r).map_or(&[][..], Vec::as_slice);
        let anywhere = if self.inherits.contains(&r) { self.anywhere.as_slice() } else { &[][..] };
        let answers = callers.iter().map(|c| (&c.props, true)).chain(anywhere.iter().map(|p| (p, false)));
        let (mut state, mut found) = (None, None);
        for (props, lack_shows) in answers {
            self.lookups = self.lookups.saturating_add(1);
            if self.lookups > MAX_NAME_LOOKUPS {
                return Err(refused);
            }
            let target = props.get(name);
            let off = match target {
                Some(t) => layer_state(doc, t, hidden)?,
                None if lack_shows => false,
                None => continue,
            };
            if state.is_some_and(|was| was != off) {
                return Err(refused);
            }
            state = Some(off);
            found = found.or(target);
        }
        let own = !callers.is_empty() && callers.iter().all(|c| c.own);
        match (state, found) {
            (Some(true), Some(t)) => Ok(Some(t.clone())),
            (_, None) if !own => Err(refused),
            _ => Ok(None),
        }
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

mod left;

#[cfg(test)]
mod tests;
