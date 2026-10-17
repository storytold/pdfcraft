//! The content interpreter that applies (or verifies) redaction on one content scope: a page's
//! streams or a form XObject's.
//!
//! It tracks just enough graphics state to place every glyph, image and path in user space:
//! the CTM stack, the text state (font, size, spacing, scaling, rise, leading) and the text
//! matrices. In apply mode it rewrites the operators:
//! - text: glyphs whose boxes overlap a region are cut out of `Tj`/`TJ`/`'`/`"`, replaced by a
//!   `TJ` displacement of the same width so the remaining text stays exactly where it was;
//! - images: fully covered → the `Do` is removed; partly covered → the covered pixels are
//!   cleared in a copy of the image (or the image is removed when its codec can't be re-encoded);
//!   inline images under a region are removed;
//! - vectors: fully covered paths are removed; partly covered ones are clipped so nothing shows
//!   inside the regions; shadings are clipped the same way;
//! - form XObjects under a region are rewritten recursively into new objects (copy-on-write, so
//!   other pages using the original are untouched).
//!
//! In verify mode nothing changes; glyphs and inline images that still overlap a region are
//! counted.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use pdfcraft_content::{Matrix, Op, Pieces, contains, num, overlaps, serialize_ops, string};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::{RedactError, Report, image, page_streams, tags};
use pdfcraft_fonts::pdf::Metrics;

const MAX_DEPTH: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Apply,
    Verify,
}

#[derive(Clone)]
struct Gs {
    ctm: Matrix,
    font: Rc<Metrics>,
    /// The current font writes vertically (a Type0 font with a predefined vertical CMap or an
    /// embedded CMap whose `/WMode` is 1, see [`is_vertical`]). Glyph layout here is horizontal,
    /// so such text cannot be placed under a region; apply fails closed when it meets any (see
    /// `show`).
    vertical: bool,
    size: f64,
    char_spacing: f64,
    word_spacing: f64,
    render_mode: i64,
    scale: f64,
    leading: f64,
    rise: f64,
}

impl Gs {
    /// Make `font` the current font, as `Tf` and an ExtGState's `/Font` do.
    fn select(&mut self, font: CachedFont) {
        self.font = font.metrics;
        self.vertical = font.vertical;
    }
}

/// The resource category whose name selected a font. The two are separate name spaces (one name
/// can be a font in one and a graphics state in the other), so a scope's font cache keys on both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum FontSource {
    /// A `/Font` resource, selected by `Tf`.
    Resource,
    /// An `/ExtGState` resource whose `/Font` entry selects the font, set by `gs`.
    ExtGState,
}

/// A font as a scope caches it.
#[derive(Clone)]
struct CachedFont {
    metrics: Rc<Metrics>,
    /// The font writes vertically (see [`is_vertical`]).
    vertical: bool,
}

/// What processing a scope produced.
#[derive(Default)]
pub(crate) struct Output {
    /// The rewritten streams (`None` = unchanged).
    pub streams: Vec<Option<Vec<u8>>>,
    /// New XObjects the rewritten content refers to: (resource name, object).
    pub xobjects: Vec<(Vec<u8>, ObjRef)>,
    /// Resource names whose `Do` this scope dropped or pointed at a cleaned copy, so the caller
    /// can unlink the original from the resource dictionary (a full save keeps whatever is still
    /// referenced, which would otherwise retain the removed image or form bytes).
    pub replaced: Vec<Vec<u8>>,
    /// The original objects behind `replaced`, nested scopes included. Other resource
    /// dictionaries may name them too (an inherited or shared /Resources, or the caller's when a
    /// form has none of its own): [`unlink_doomed`] removes those entries.
    pub doomed: Vec<ObjRef>,
    /// Glyphs / inline images still overlapping a region (verify mode).
    pub residue: usize,
    /// Apply mode met vertical-writing text (which it cannot remove) while a redaction region was
    /// in force: the operation must fail closed rather than leave that text under the mark.
    pub vertical: bool,
    /// The content could not be read the way a viewer draws it (an inline image without an `EI`
    /// swallowed the operators after it, in this scope or a nested form), so what was processed
    /// is not everything the page shows: apply must fail closed.
    pub unreadable: bool,
    pub tag_changes: Vec<(ObjRef, ObjRef, HashSet<i64>, HashSet<i64>)>,
    /// Property lists rewritten without their replacement text and added under a fresh name,
    /// because the named list held indirect references and so could not be written inline.
    pub properties: Vec<(Vec<u8>, Dict)>,
    /// `/Properties` names whose `BDC` this scope rewrote (inline, or under a fresh name): the
    /// caller drops them unless a surviving `BDC` still names them, so the shared original with
    /// its replacement text does not stay in the file.
    pub replaced_properties: Vec<Vec<u8>>,
}

/// The keys of a property list, or a structure element, whose value stands for the content: text
/// a reader may show, search or read out in place of the glyphs.
pub(crate) const REPLACEMENT_KEYS: [&[u8]; 3] = [b"ActualText", b"Alt", b"E"];

pub(crate) struct Scope<'a> {
    pub rects: &'a [[f64; 4]],
    pub mode: Mode,
    pub report: &'a mut Report,
    /// Sanitize: remove hidden text (invisible render modes, or wholly outside this page box).
    pub hidden_text: Option<[f64; 4]>,
    /// Sanitize: optional content groups (and membership dictionaries) that are off; content
    /// marked with them is removed.
    pub hidden_layers: Vec<ObjRef>,
    /// Content removed from hidden layers (blocks and XObjects).
    pub layer_blocks: usize,
    /// The fonts this scope has selected, by the resource that named each.
    fonts: HashMap<(FontSource, Vec<u8>), CachedFont>,
    used_names: Vec<Vec<u8>>,
    depth: usize,
}

impl<'a> Scope<'a> {
    pub fn new(rects: &'a [[f64; 4]], mode: Mode, report: &'a mut Report) -> Self {
        Scope {
            rects,
            mode,
            report,
            hidden_text: None,
            hidden_layers: Vec::new(),
            layer_blocks: 0,
            fonts: HashMap::new(),
            used_names: Vec::new(),
            depth: 0,
        }
    }

    /// Sanitizing visits every form XObject (not only those under a region).
    fn everywhere(&self) -> bool {
        self.hidden_text.is_some() || !self.hidden_layers.is_empty()
    }

    /// Is this optional-content reference (an OCG or OCMD) hidden?
    fn layer_hidden(&self, doc: &Document, o: &Object) -> bool {
        layer_hidden(doc, &self.hidden_layers, o)
    }

    fn hits(&self, b: [f64; 4]) -> bool {
        self.rects.iter().any(|r| overlaps(*r, b, 0.0))
    }

    fn covered(&self, b: [f64; 4]) -> bool {
        self.rects.iter().any(|r| contains(*r, b, 0.01))
    }

    /// Does a glyph box count as under a region? Any real overlap does (more than a fifth of
    /// the glyph or a point, whichever is less, in both directions), so neighbours that only
    /// touch an edge survive while partly covered glyphs go. Degenerate boxes (zero-size text)
    /// count when their origin is inside.
    fn glyph_hit(&self, b: [f64; 4], origin: (f64, f64)) -> bool {
        let (gw, gh) = (b[2] - b[0], b[3] - b[1]);
        if gw < 0.01 || gh < 0.01 {
            return self.rects.iter().any(|r| origin.0 >= r[0] && origin.0 <= r[2] && origin.1 >= r[1] && origin.1 <= r[3]);
        }
        let (mx, my) = ((gw * 0.2).min(1.0), (gh * 0.2).min(1.0));
        self.rects.iter().any(|r| {
            let ox = b[2].min(r[2]) - b[0].max(r[0]);
            let oy = b[3].min(r[3]) - b[1].max(r[1]);
            ox > mx && oy > my
        })
    }
}

fn res_dict(doc: &Document, res: &Dict, key: &[u8]) -> Dict {
    res.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default()
}

/// Is this optional-content reference (an OCG or OCMD) one of the `hidden` layers?
fn layer_hidden(doc: &Document, hidden: &[ObjRef], o: &Object) -> bool {
    if hidden.is_empty() {
        return false;
    }
    if let Some(r) = o.as_ref()
        && hidden.contains(&r)
    {
        return true;
    }
    // A membership dictionary (default policy AnyOn): hidden when all its groups are.
    let d = doc.resolve(o);
    let Some(d) = d.as_dict() else { return false };
    if d.name(b"Type") != Some(b"OCMD") {
        return false;
    }
    let groups: Vec<ObjRef> = match d.get(b"OCGs").map(|g| (*doc.resolve(g)).clone()) {
        Some(Object::Array(a)) => a.iter().filter_map(Object::as_ref).collect(),
        Some(_) => d.get(b"OCGs").and_then(Object::as_ref).into_iter().collect(),
        None => Vec::new(),
    };
    !groups.is_empty() && groups.iter().all(|g| hidden.contains(g))
}

/// Does this font write vertically? Only a Type0 (composite) font can: its `/Encoding` is a
/// predefined vertical CMap name (ending in `-V`, Identity-V and the like, or the one named
/// plainly `V`; ISO 32000-2 Table 118), or an embedded CMap stream whose `/WMode` is 1, in its
/// stream dictionary or, as a viewer reads it, in its program (§9.7.5.4). An embedded CMap that
/// cannot be decoded counts as vertical, so apply fails closed.
fn is_vertical(doc: &Document, font: &Dict) -> bool {
    if font.name(b"Subtype") != Some(b"Type0".as_slice()) {
        return false;
    }
    match font.get(b"Encoding").map(|e| doc.resolve(e)).as_deref() {
        Some(Object::Name(n)) => n == b"V" || n.ends_with(b"-V"),
        Some(Object::Stream(s)) => {
            s.dict.int(b"WMode") == Some(1)
                || match s.decoded() {
                    Ok(program) => pdfcraft_fonts::pdf::cmap_is_vertical(&program),
                    Err(_) => true,
                }
        }
        _ => false,
    }
}

/// The prefix of the `/XObject` names given to the cleaned copies of images and forms.
const XOBJECT_PREFIX: &str = "PCRedacted";
/// The prefix of the `/Properties` names given to the cleaned copies of property lists.
const PROPERTY_PREFIX: &str = "PCRedactedP";

/// A resource name, `prefix` and a number, that is neither in `taken` (the resource dictionary it
/// is for) nor handed out before; it is recorded in the scope so it is not handed out again.
fn fresh_name(scope: &mut Scope<'_>, taken: &Dict, prefix: &str) -> Vec<u8> {
    let mut i = scope.used_names.len() + 1;
    loop {
        let n = format!("{prefix}{i}").into_bytes();
        if !taken.contains(&n) && !scope.used_names.contains(&n) {
            scope.used_names.push(n.clone());
            return n;
        }
        i += 1;
    }
}

/// The parts of a scope's [`Output`] that its resource dictionary has to follow once the content
/// is rewritten.
pub(crate) struct Rewrite<'a> {
    /// New XObjects the rewritten content draws: (resource name, object).
    pub xobjects: &'a [(Vec<u8>, ObjRef)],
    /// XObject names whose `Do` was dropped or pointed at a cleaned copy.
    pub replaced: &'a [Vec<u8>],
    /// Cleaned property lists added under fresh names.
    pub properties: &'a [(Vec<u8>, Dict)],
    /// Property-list names whose `BDC` was rewritten.
    pub replaced_properties: &'a [Vec<u8>],
}

impl Rewrite<'_> {
    /// Did the rewrite drop, replace or add any resource name?
    fn names_resources(&self) -> bool {
        !self.xobjects.is_empty() || !self.replaced.is_empty() || !self.properties.is_empty() || !self.replaced_properties.is_empty()
    }
}

impl Output {
    /// What the scope's resources have to follow (see [`rebuild_resources`]).
    pub(crate) fn rewrite(&self) -> Rewrite<'_> {
        Rewrite { xobjects: &self.xobjects, replaced: &self.replaced, properties: &self.properties, replaced_properties: &self.replaced_properties }
    }
}

/// Bring a rewritten scope's resource dictionary `res` in line with its new `content` (`None`
/// when the content itself did not change: then nothing is pruned, only added), so a full-rewrite
/// save keeps only what the scope still uses:
/// - `/XObject`: an original whose `Do` was dropped or replaced goes once no surviving `Do` draws
///   it; the cleaned copies come in;
/// - `/Properties`: the lists of `hidden_layers` and the originals of the cleaned lists go once
///   no surviving `BDC` names them; the cleaned copies with references come in.
///
/// Each dictionary is written back into `res` (inline) only when an entry went or came. Returns
/// whether `res` changed.
pub(crate) fn rebuild_resources(
    doc: &Document,
    res: &mut Dict,
    rewrite: &Rewrite<'_>,
    content: Option<&[Vec<u8>]>,
    hidden_layers: &[ObjRef],
) -> bool {
    let mut xo = res_dict(doc, res, b"XObject");
    let before = xo.len();
    if let Some(content) = content {
        prune(&mut xo, rewrite.replaced, content);
    }
    for (n, r) in rewrite.xobjects {
        xo.set(n.clone(), Object::Ref(*r));
    }
    let xobjects_changed = xo.len() != before || !rewrite.xobjects.is_empty();
    if xobjects_changed {
        res.set(b"XObject".to_vec(), Object::Dict(xo));
    }
    let mut pr = res_dict(doc, res, b"Properties");
    let before = pr.len();
    if let Some(content) = content {
        prune_properties(doc, hidden_layers, &mut pr, content);
        prune_names(&mut pr, rewrite.replaced_properties, content, "BDC", 1);
    }
    for (n, d) in rewrite.properties {
        pr.set(n.clone(), Object::Dict(d.clone()));
    }
    let properties_changed = pr.len() != before || !rewrite.properties.is_empty();
    if properties_changed {
        res.set(b"Properties".to_vec(), Object::Dict(pr));
    }
    xobjects_changed || properties_changed
}

/// The resources the tag pass reads for a rewritten scope: `res` as it was before
/// [`rebuild_resources`], plus the property lists the rewrite added, so the names in the content
/// before (the originals) and after (the fresh ones) both resolve.
pub(crate) fn tag_resources(doc: &Document, res: &Dict, rewrite: &Rewrite<'_>) -> Dict {
    let mut tags_res = res.clone();
    if !rewrite.properties.is_empty() {
        let mut pr = res_dict(doc, res, b"Properties");
        for (n, d) in rewrite.properties {
            pr.set(n.clone(), Object::Dict(d.clone()));
        }
        tags_res.set(b"Properties".to_vec(), Object::Dict(pr));
    }
    tags_res
}

/// The data of a page's content streams as listed in `/Contents`, decoded leniently (an entry
/// that is not a stream, or does not decode, counts as empty): what a rewritten page's resources
/// are pruned against.
pub(crate) fn decoded_contents(doc: &Document, list: &[Object]) -> Vec<Vec<u8>> {
    list.iter()
        .map(|o| match &*doc.resolve(o) {
            Object::Stream(s) => s.decoded().unwrap_or_default(),
            _ => Vec::new(),
        })
        .collect()
}

/// Drop every `replaced` resource name whose `Do` no longer appears in the rewritten `content`,
/// so a full-rewrite save stops reaching the original image or form it named. A name that some
/// surviving `Do` still draws (the same image shown again outside a mark) is kept.
fn prune(xo: &mut Dict, replaced: &[Vec<u8>], content: &[Vec<u8>]) {
    prune_names(xo, replaced, content, "Do", 0);
}

/// Drop every `replaced` name of a resource dictionary that no surviving `operator` in `content`
/// names as its operand `operand` (`Do` and an XObject, `BDC` and a property list), so a
/// full-rewrite save stops reaching what it named.
fn prune_names(dict: &mut Dict, replaced: &[Vec<u8>], content: &[Vec<u8>], operator: &str, operand: usize) {
    if replaced.is_empty() {
        return;
    }
    let joined = Pieces::join(content);
    let ops = joined.parse().ops;
    let still_named = |n: &[u8]| ops.iter().any(|op| op.is(operator) && op.name(operand) == Some(n));
    for n in replaced {
        if !still_named(n) {
            dict.remove(n);
        }
    }
}

/// Does this value hold an indirect reference anywhere (which a content stream cannot carry)?
fn has_ref(o: &Object, depth: usize) -> bool {
    if depth > 64 {
        return true; // too deep to tell: treat as not inlinable
    }
    match o {
        Object::Ref(_) => true,
        Object::Array(a) => a.iter().any(|x| has_ref(x, depth + 1)),
        Object::Dict(d) => d.iter().any(|(_, v)| has_ref(v, depth + 1)),
        _ => false,
    }
}

/// Strip the replacement text (`REPLACEMENT_KEYS`) from a `BDC`'s property list. An inline
/// dictionary is edited in place. A named one is resolved and written inline, leaving the shared
/// original for sequences that were not touched (the caller drops it once nothing names it); one
/// that holds indirect references is added to `out.properties` under a fresh name instead.
/// Returns whether the operator changed.
fn clean_property_list(doc: &Document, scope: &mut Scope<'_>, op: &mut Op, props: &Dict, out: &mut Output) -> bool {
    let has_text = |d: &Dict| REPLACEMENT_KEYS.iter().any(|k| d.contains(k));
    let cleaned = |d: &Dict| {
        let mut d = d.clone();
        for k in REPLACEMENT_KEYS {
            d.remove(k);
        }
        d
    };
    let new = match op.operands.get(1) {
        Some(Object::Dict(d)) if has_text(d) => Object::Dict(cleaned(d)),
        Some(Object::Name(n)) => {
            let Some(list) = props.get(n).map(|o| doc.resolve(o)) else { return false };
            let Some(d) = list.as_dict() else { return false };
            if !has_text(d) {
                return false;
            }
            out.replaced_properties.push(n.clone());
            let d = cleaned(d);
            if d.iter().any(|(_, v)| has_ref(v, 0)) {
                let fresh = fresh_name(scope, props, PROPERTY_PREFIX);
                out.properties.push((fresh.clone(), d));
                Object::Name(fresh)
            } else {
                Object::Dict(d)
            }
        }
        _ => return false,
    };
    match op.operands.get_mut(1) {
        Some(slot) => {
            *slot = new;
            true
        }
        None => false,
    }
}

/// The pieces of a content stream rewritten so far, and the marked-content sequences still open
/// across them.
#[derive(Default)]
struct Built {
    /// Each finished piece's operators and whether it changed; the piece being built is the next.
    done: Vec<(Vec<Op>, bool)>,
    /// The open sequences: where each `BDC` went (piece, index in that piece), `None` for a `BMC`
    /// or a list cleaned already.
    open: Vec<Option<(usize, usize)>>,
}

/// Something inside the open marked-content sequences was removed or cut: the text that stood
/// for it goes from every enclosing `BDC`'s property list, whether that `BDC` is in the piece
/// being built (`ops`) or an earlier one. An earlier piece whose `BDC` changed is flagged as
/// changed here; the piece being built is not, since the caller, which has just removed or cut
/// something in it, flags it itself. A sequence cleaned once is forgotten (`None`).
fn clean_enclosing_sequences(doc: &Document, scope: &mut Scope<'_>, built: &mut Built, ops: &mut [Op], props: &Dict, out: &mut Output) {
    let current = built.done.len();
    for slot in built.open.iter_mut() {
        let Some((piece, index)) = *slot else { continue };
        *slot = None;
        let op = if piece == current { ops.get_mut(index) } else { built.done.get_mut(piece).and_then(|(v, _)| v.get_mut(index)) };
        let Some(op) = op else { continue };
        if clean_property_list(doc, scope, op, props, out)
            && piece != current
            && let Some((_, changed)) = built.done.get_mut(piece)
        {
            *changed = true;
        }
    }
}

/// Whether `content` draws the XObject `name` of the resource dictionary `xo`: directly, or
/// through a form that has no /Resources of its own (such a form draws through its caller's).
/// `None` when that cannot be told (a form that will not decode, or nesting past the limit).
fn content_draws(doc: &Document, content: &[Vec<u8>], xo: &Dict, name: &[u8], depth: usize, seen: &mut HashSet<ObjRef>) -> Option<bool> {
    let joined = Pieces::join(content);
    let ops = joined.parse().ops;
    for op in ops.iter().filter(|op| op.is("Do")) {
        let Some(n) = op.name(0) else { continue };
        if n == name {
            return Some(true);
        }
        let Some(r) = xo.get(n).and_then(Object::as_ref) else { continue };
        let obj = doc.get(r);
        let Object::Stream(s) = &*obj else { continue };
        if s.dict.name(b"Subtype") != Some(b"Form") || s.dict.contains(b"Resources") || !seen.insert(r) {
            continue;
        }
        if depth >= MAX_DEPTH {
            return None;
        }
        let Ok(data) = s.decoded_strict_in(doc) else { return None };
        match content_draws(doc, std::slice::from_ref(&data), xo, name, depth + 1, seen) {
            Some(false) => {}
            other => return other,
        }
    }
    Some(false)
}

/// Unlink the XObjects a rewrite dropped or replaced (`doomed`) from every page-tree resource
/// dictionary that still names them. The rewritten page has its own pruned /Resources, but the
/// originals stay reachable, and so stay in a full-rewrite save, when:
/// - the page inherited its /Resources from a /Pages node, which still lists them;
/// - several pages share one resource dictionary, and another page still refers to it;
/// - a form without /Resources drew them through the page's dictionary.
///
/// An entry goes unless a page that draws through that dictionary still draws the name (the same
/// image shown on another page, or again outside a mark). A dictionary that loses an entry is
/// replaced by a private copy on its node, so a dictionary object shared with other nodes is
/// left as it is for them. Fails closed when a page's use of a name cannot be told.
pub(crate) fn unlink_doomed(doc: &mut Document, doomed: &[ObjRef]) -> Result<(), RedactError> {
    if doomed.is_empty() {
        return Ok(());
    }
    let doomed: HashSet<ObjRef> = doomed.iter().copied().collect();
    let Some(top) = doc.root().and_then(|root| doc.get(root).as_dict().and_then(|d| d.reference(b"Pages"))) else { return Ok(()) };
    // Every page-tree node with a /Resources of its own, and the pages that draw through it
    // (the nearest such node above a page, or the page itself). Cycles are cut as in
    // `pdfcraft_model::pages`.
    let mut holders: Vec<(ObjRef, Vec<ObjRef>)> = Vec::new();
    let mut seen = HashSet::new();
    let mut stack: Vec<(ObjRef, Option<usize>)> = vec![(top, None)];
    while let Some((node, mut holder)) = stack.pop() {
        if !seen.insert(node) || seen.len() > 1_000_000 {
            continue;
        }
        let obj = doc.get(node);
        let Some(d) = obj.as_dict() else { continue };
        if d.contains(b"Resources") {
            holders.push((node, Vec::new()));
            holder = Some(holders.len() - 1);
        }
        let is_pages = d.name(b"Type") == Some(b"Pages") || (d.contains(b"Kids") && d.name(b"Type") != Some(b"Page"));
        if is_pages {
            let kids = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default();
            stack.extend(kids.iter().filter_map(Object::as_ref).map(|k| (k, holder)));
        } else if let Some(h) = holder.and_then(|i| holders.get_mut(i)) {
            h.1.push(node);
        }
    }
    let index: HashMap<ObjRef, usize> = pdfcraft_model::pages(doc).iter().enumerate().map(|(i, p)| (p.obj, i)).collect();
    for (node, users) in holders {
        let Some(res) = doc.get(node).as_dict().and_then(|d| d.get(b"Resources")).and_then(|r| doc.resolve(r).as_dict().cloned()) else { continue };
        let mut xo = res_dict(doc, &res, b"XObject");
        let names: Vec<Vec<u8>> = xo.iter().filter(|(_, v)| v.as_ref().is_some_and(|r| doomed.contains(&r))).map(|(n, _)| n.to_vec()).collect();
        if names.is_empty() {
            continue;
        }
        let mut contents = Vec::with_capacity(users.len());
        for user in &users {
            let page = doc.get(*user).as_dict().cloned().unwrap_or_default();
            // A page whose content cannot be read may still draw the name: fail closed.
            contents.push(page_streams(doc, &page, index.get(user).copied().unwrap_or(0))?.1);
        }
        let mut pruned = false;
        for n in names {
            let mut drawn = Some(false);
            for content in &contents {
                drawn = content_draws(doc, content, &xo, &n, 0, &mut HashSet::new());
                if drawn != Some(false) {
                    break;
                }
            }
            match drawn {
                Some(true) => {}
                Some(false) => {
                    xo.remove(&n);
                    pruned = true;
                }
                None => return Err(RedactError::Residue(1)),
            }
        }
        if pruned {
            let mut res = res;
            res.set(b"XObject".to_vec(), Object::Dict(xo));
            doc.update_dict(node, |d| d.set(b"Resources".to_vec(), Object::Dict(res)))?;
        }
    }
    Ok(())
}

/// The clip that hides everything inside each region: one even-odd clip per region (a big
/// rectangle with the region cut out), in the current user space. Returns `None` when the CTM
/// can't be inverted (nothing is painted then anyway).
fn clip_out(rects: &[[f64; 4]], ctm: &Matrix, around: [f64; 4]) -> Option<Vec<Op>> {
    let inv = ctm.invert()?;
    let mut ops = Vec::new();
    let big = [around[0] - 10.0, around[1] - 10.0, around[2] + 10.0, around[3] + 10.0];
    for r in rects.iter().filter(|r| overlaps(**r, around, 0.0)) {
        let outer = [(big[0], big[1]), (big[2], big[1]), (big[2], big[3]), (big[0], big[3])];
        let inner = [(r[0], r[1]), (r[0], r[3]), (r[2], r[3]), (r[2], r[1])];
        for poly in [outer, inner] {
            for (k, (x, y)) in poly.iter().enumerate() {
                let (u, v) = inv.apply(*x, *y);
                ops.push(Op::new(if k == 0 { "m" } else { "l" }, vec![num(u), num(v)]));
            }
            ops.push(Op::new("h", vec![]));
        }
        ops.push(Op::new("W*", vec![]));
        ops.push(Op::new("n", vec![]));
    }
    Some(ops)
}

/// One glyph of a shown string.
struct Glyph {
    bytes: std::ops::Range<usize>,
    /// Advance in unscaled text space (before `Th`).
    advance: f64,
    hit: bool,
}

/// The font `font_dict` as the layout needs it, cached in the scope under the resource that
/// selected it (`source` and `name`), so its metrics are read and its writing mode worked out
/// once per scope; the fallback, written horizontally, when there is no dictionary. Within a
/// scope a resource name stands for one font dictionary, so the cached answer is the answer.
fn cached_font(
    scope: &mut Scope<'_>,
    doc: &Document,
    source: FontSource,
    name: &[u8],
    font_dict: Option<&Dict>,
    fallback: &Rc<Metrics>,
) -> CachedFont {
    let font = scope.fonts.entry((source, name.to_vec())).or_insert_with(|| CachedFont {
        metrics: font_dict.map(|d| Rc::new(Metrics::from_dict(doc, d))).unwrap_or_else(|| fallback.clone()),
        vertical: font_dict.is_some_and(|d| is_vertical(doc, d)),
    });
    font.clone()
}

/// Process a page's content streams (state flows from one stream to the next) from the initial
/// graphics state.
pub(crate) fn process(doc: &mut Document, scope: &mut Scope<'_>, streams: &[Vec<u8>], resources: &Dict, ctm: Matrix) -> Output {
    let gs = Gs {
        ctm,
        font: Rc::new(Metrics::fallback()),
        vertical: false,
        size: 0.0,
        char_spacing: 0.0,
        word_spacing: 0.0,
        render_mode: 0,
        scale: 1.0,
        leading: 0.0,
        rise: 0.0,
    };
    process_with(doc, scope, streams, resources, gs)
}

/// Process a scope's content streams starting from `gs`: a form XObject inherits its caller's
/// graphics state, text state included (ISO 32000-2 §8.10.1), so text it shows without a `Tf` of
/// its own is in the caller's font and size.
fn process_with(doc: &mut Document, scope: &mut Scope<'_>, streams: &[Vec<u8>], resources: &Dict, mut gs: Gs) -> Output {
    let mut out = Output::default();
    let fallback = Rc::new(Metrics::fallback());
    let mut stack: Vec<Gs> = Vec::new();
    let (mut tm, mut tlm) = (Matrix::IDENTITY, Matrix::IDENTITY);
    let fonts_res = res_dict(doc, resources, b"Font");
    let ext_res = res_dict(doc, resources, b"ExtGState");
    let mut xobjects = res_dict(doc, resources, b"XObject");
    // The current path: its operators, its bounding box in user space, and whether it clips.
    let mut path: Vec<Op> = Vec::new();
    let mut path_box: Option<[f64; 4]> = None;
    let mut clip = false;
    let mut forced_change = false;

    // The streams are one content stream in pieces, which may be split between any two tokens
    // (ISO 32000-2 §7.8.2): an operator's operands can end one piece and the operator start the
    // next. Parse them joined, then give each operator back to the piece its keyword is in.
    let joined = Pieces::join(streams);
    let mut parsed = joined.parse();
    out.unreadable |= parsed.unterminated_inline_image;
    let props = res_dict(doc, resources, b"Properties");
    if !scope.hidden_layers.is_empty() {
        let stripped = strip_hidden_layers(doc, scope, parsed.ops, &props, &mut out);
        parsed.ops = stripped.ops;
        if stripped.blocks > 0 {
            scope.layer_blocks += stripped.blocks;
            forced_change = true;
        }
        // The XObjects the removed blocks drew: unlinked by the caller unless a surviving `Do`
        // still draws them, so their bytes do not stay reachable in the saved file.
        for name in stripped.dropped {
            if let Some(r) = xobjects.get(&name).and_then(Object::as_ref) {
                out.doomed.push(r);
            }
            out.replaced.push(name);
        }
    }
    let mut pieces: Vec<Vec<Op>> = streams.iter().map(|_| Vec::new()).collect();
    let mut rewrite: Vec<bool> = vec![forced_change; streams.len()];
    for op in parsed.ops {
        let (first, last) = joined.pieces_of(&op);
        // An operator split across pieces is written whole into its keyword's piece, so every
        // piece it spans is rewritten.
        if first < last {
            for r in rewrite.iter_mut().take(last + 1).skip(first) {
                *r = true;
            }
        }
        if let Some(p) = pieces.get_mut(last) {
            p.push(op);
        }
    }

    // When something inside an open marked-content sequence is removed or cut, the enclosing
    // property lists lose the text that stood for it (`clean_enclosing_sequences`).
    let mut built = Built { done: Vec::with_capacity(streams.len()), open: Vec::new() };
    for (piece, mut changed) in pieces.into_iter().zip(rewrite) {
        let mut ops: Vec<Op> = Vec::with_capacity(piece.len());
        for op in piece {
            let o = op.op.as_slice();
            // Path construction.
            if matches!(o, b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re" | b"W" | b"W*") {
                let pts: Vec<(f64, f64)> = match o {
                    b"re" => op.nums::<4>().map(|[x, y, w, h]| vec![(x, y), (x + w, y), (x, y + h), (x + w, y + h)]).unwrap_or_default(),
                    b"h" | b"W" | b"W*" => Vec::new(),
                    _ => op.operands.iter().filter_map(Object::as_f64).collect::<Vec<_>>().as_chunks::<2>().0.iter().map(|p| (p[0], p[1])).collect(),
                };
                for (x, y) in pts {
                    let (u, v) = gs.ctm.apply(x, y);
                    path_box = Some(match path_box {
                        None => [u, v, u, v],
                        Some(b) => [b[0].min(u), b[1].min(v), b[2].max(u), b[3].max(v)],
                    });
                }
                clip |= matches!(o, b"W" | b"W*");
                path.push(op);
                continue;
            }
            if matches!(o, b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n") {
                let body = std::mem::take(&mut path);
                let bbox = path_box.take();
                let clips = std::mem::replace(&mut clip, false);
                let hit = scope.mode == Mode::Apply && o != b"n" && bbox.is_some_and(|b| scope.hits(b));
                if !hit {
                    ops.extend(body);
                    ops.push(op);
                    continue;
                }
                let b = bbox.unwrap_or_default();
                changed = true;
                clean_enclosing_sequences(doc, scope, &mut built, &mut ops, &props, &mut out);
                if scope.covered(b) {
                    scope.report.paths_removed += 1;
                    if clips {
                        // Keep the clipping, drop the painting.
                        ops.extend(body);
                        ops.push(Op::new("n", vec![]));
                    }
                } else if clips {
                    // A clip-and-paint path can't be wrapped in q/Q without losing its clip:
                    // paint it clipped, then set its clip with a no-op path.
                    scope.report.paths_clipped += 1;
                    if let Some(c) = clip_out(scope.rects, &gs.ctm, b) {
                        let plain: Vec<Op> = body.iter().filter(|p| !p.is("W") && !p.is("W*")).cloned().collect();
                        ops.push(Op::new("q", vec![]));
                        ops.extend(c);
                        ops.extend(plain);
                        ops.push(op);
                        ops.push(Op::new("Q", vec![]));
                    }
                    ops.extend(body);
                    ops.push(Op::new("n", vec![]));
                } else {
                    scope.report.paths_clipped += 1;
                    if let Some(c) = clip_out(scope.rects, &gs.ctm, b) {
                        ops.push(Op::new("q", vec![]));
                        ops.extend(c);
                        ops.extend(body);
                        ops.push(op);
                        ops.push(Op::new("Q", vec![]));
                    }
                }
                continue;
            }
            if !path.is_empty() {
                // A path not ended by a painting operator: pass it through.
                ops.append(&mut path);
                path_box = None;
                clip = false;
            }
            match o {
                b"q" => stack.push(gs.clone()),
                b"Q" => {
                    if let Some(g) = stack.pop() {
                        gs = g;
                    }
                }
                b"cm" => {
                    if let Some(m) = op.nums::<6>() {
                        gs.ctm = Matrix(m).then(&gs.ctm);
                    }
                }
                b"BT" => {
                    tm = Matrix::IDENTITY;
                    tlm = Matrix::IDENTITY;
                }
                b"Tf" => {
                    gs.size = op.num(1).unwrap_or(gs.size);
                    if let Some(name) = op.name(0) {
                        let font_dict = fonts_res.get(name).and_then(|f| doc.resolve(f).as_dict().cloned());
                        gs.select(cached_font(scope, doc, FontSource::Resource, name, font_dict.as_ref(), &fallback));
                    }
                }
                b"gs" => {
                    // An ExtGState's /Font entry, `[font size]`, selects the font and size as
                    // `Tf` does (§8.4.5, Table 57); a viewer honours it, so must the layout here.
                    if let Some(name) = op.name(0)
                        && let Some(g) = ext_res.get(name).map(|g| doc.resolve(g))
                        && let Some(pair) = g.as_dict().and_then(|g| g.get(b"Font")).map(|f| doc.resolve(f))
                        && let Some(pair) = pair.as_array()
                    {
                        if let Some(size) = pair.get(1).and_then(|s| doc.resolve(s).as_f64()) {
                            gs.size = size;
                        }
                        if let Some(f) = pair.first() {
                            let font_dict = doc.resolve(f).as_dict().cloned();
                            gs.select(cached_font(scope, doc, FontSource::ExtGState, name, font_dict.as_ref(), &fallback));
                        }
                    }
                }
                b"Tc" => gs.char_spacing = op.num(0).unwrap_or(0.0),
                b"Tw" => gs.word_spacing = op.num(0).unwrap_or(0.0),
                b"Tz" => gs.scale = op.num(0).unwrap_or(100.0) / 100.0,
                b"TL" => gs.leading = op.num(0).unwrap_or(0.0),
                b"Ts" => gs.rise = op.num(0).unwrap_or(0.0),
                b"Tr" => gs.render_mode = op.num(0).unwrap_or(0.0) as i64,
                b"Td" | b"TD" => {
                    if let Some([x, y]) = op.nums::<2>() {
                        if o == b"TD" {
                            gs.leading = -y;
                        }
                        tlm = Matrix::translate(x, y).then(&tlm);
                        tm = tlm;
                    }
                }
                b"Tm" => {
                    if let Some(m) = op.nums::<6>() {
                        tlm = Matrix(m);
                        tm = tlm;
                    }
                }
                b"T*" => {
                    tlm = Matrix::translate(0.0, -gs.leading).then(&tlm);
                    tm = tlm;
                }
                b"Tj" | b"TJ" | b"'" | b"\"" => {
                    if o == b"\"" {
                        gs.word_spacing = op.num(0).unwrap_or(gs.word_spacing);
                        gs.char_spacing = op.num(1).unwrap_or(gs.char_spacing);
                    }
                    if matches!(o, b"'" | b"\"") {
                        tlm = Matrix::translate(0.0, -gs.leading).then(&tlm);
                        tm = tlm;
                    }
                    let items: Vec<Object> = match o {
                        b"TJ" => op.operands.first().and_then(Object::as_array).cloned().unwrap_or_default(),
                        _ => op.operands.last().cloned().into_iter().collect(),
                    };
                    // Vertical-writing text is laid out horizontally by `show`, so its glyphs are
                    // never matched against a region. Rather than ship a document whose vertical
                    // text was silently left under a mark, record it so apply can fail closed.
                    // (Full vertical-text removal is a future enhancement.)
                    if gs.vertical
                        && scope.mode == Mode::Apply
                        && scope.hidden_text.is_none()
                        && !scope.rects.is_empty()
                        && items.iter().any(|o| matches!(o, Object::String(s) if !s.bytes.is_empty()))
                    {
                        out.vertical = true;
                    }
                    let (new_items, removed) = show(scope, &gs, &mut tm, &items);
                    if removed == 0 {
                        ops.push(op);
                        continue;
                    }
                    if scope.mode == Mode::Verify {
                        out.residue += removed;
                        ops.push(op);
                        continue;
                    }
                    changed = true;
                    scope.report.glyphs += removed;
                    clean_enclosing_sequences(doc, scope, &mut built, &mut ops, &props, &mut out);
                    match o {
                        b"\"" => {
                            ops.push(Op::new("Tw", vec![num(gs.word_spacing)]));
                            ops.push(Op::new("Tc", vec![num(gs.char_spacing)]));
                            ops.push(Op::new("T*", vec![]));
                        }
                        b"'" => ops.push(Op::new("T*", vec![])),
                        _ => {}
                    }
                    ops.push(Op::new("TJ", vec![Object::Array(new_items)]));
                    continue;
                }
                b"BI" => {
                    let b = gs.ctm.bbox([0.0, 0.0, 1.0, 1.0]);
                    if scope.hits(b) {
                        if scope.mode == Mode::Verify {
                            out.residue += 1;
                        } else {
                            scope.report.images_removed += 1;
                            changed = true;
                            clean_enclosing_sequences(doc, scope, &mut built, &mut ops, &props, &mut out);
                            continue;
                        }
                    }
                }
                b"sh" if scope.mode == Mode::Apply => {
                    // A shading fills the current clip: clip the regions out.
                    if let Some(c) = clip_out(scope.rects, &gs.ctm, [-1e6, -1e6, 1e6, 1e6]) {
                        changed = true;
                        clean_enclosing_sequences(doc, scope, &mut built, &mut ops, &props, &mut out);
                        ops.push(Op::new("q", vec![]));
                        ops.extend(c);
                        ops.push(op);
                        ops.push(Op::new("Q", vec![]));
                        continue;
                    }
                }
                b"Do" => {
                    if let Some(new) = xobject(doc, scope, &op, &gs, resources, &mut xobjects, &mut out) {
                        changed = true;
                        clean_enclosing_sequences(doc, scope, &mut built, &mut ops, &props, &mut out);
                        if let Some(n) = new {
                            ops.push(n);
                        }
                        continue;
                    }
                }
                b"BDC" => built.open.push(Some((built.done.len(), ops.len()))),
                b"BMC" => built.open.push(None),
                b"EMC" => {
                    built.open.pop();
                }
                _ => {}
            }
            ops.push(op);
        }
        if !path.is_empty() {
            ops.append(&mut path);
        }
        built.done.push((ops, changed));
    }
    for (ops, changed) in built.done {
        out.streams.push(changed.then(|| serialize_ops(&ops)));
    }
    out
}

/// Lay out a shown string (or `TJ` array) glyph by glyph, advancing `tm`. Returns the `TJ`
/// items with the glyphs under a region replaced by displacements, and how many were removed.
fn show(scope: &Scope<'_>, gs: &Gs, tm: &mut Matrix, items: &[Object]) -> (Vec<Object>, usize) {
    let f = &gs.font;
    let size = gs.size;
    let mut out: Vec<Object> = Vec::new();
    let mut removed = 0;
    let push_num = |out: &mut Vec<Object>, v: f64| {
        if v == 0.0 {
            return;
        }
        if let Some(last) = out.last_mut()
            && let Some(prev) = last.as_f64()
        {
            *last = num(prev + v);
            return;
        }
        out.push(num(v));
    };
    for item in items {
        match item {
            Object::String(s) => {
                let bytes = &s.bytes;
                let mut glyphs = Vec::new();
                let mut pos = 0;
                for (code, len) in f.codes(bytes) {
                    let w0 = f.width(code);
                    let spacing = gs.char_spacing + if f.is_space(code, len) { gs.word_spacing } else { 0.0 };
                    let trm = Matrix([size * gs.scale, 0.0, 0.0, size, 0.0, gs.rise]).then(tm).then(&gs.ctm);
                    let b = trm.bbox([0.0, f.descent, w0, f.ascent]);
                    let origin = trm.apply(0.0, 0.0);
                    let advance = w0 * size + spacing;
                    let hit = match scope.hidden_text {
                        // Invisible (Tr 3) and clip-only (Tr 7) text, or text wholly off the page.
                        Some(page) => {
                            let degenerate = b[2] - b[0] < 0.01 && b[3] - b[1] < 0.01;
                            matches!(gs.render_mode, 3 | 7) || !(overlaps(page, b, 0.0) || degenerate)
                        }
                        None => scope.glyph_hit(b, origin),
                    };
                    glyphs.push(Glyph { bytes: pos..pos + len, advance, hit });
                    *tm = Matrix::translate(advance * gs.scale, 0.0).then(tm);
                    pos += len;
                }
                let mut run: Vec<u8> = Vec::new();
                for g in &glyphs {
                    if g.hit {
                        removed += 1;
                        if !run.is_empty() {
                            out.push(string(std::mem::take(&mut run)));
                        }
                        // A TJ number moves by -n/1000 × size (× Th): the removed advance.
                        if size != 0.0 {
                            push_num(&mut out, -g.advance * 1000.0 / size);
                        }
                    } else {
                        run.extend_from_slice(&bytes[g.bytes.clone()]);
                    }
                }
                if !run.is_empty() {
                    out.push(string(run));
                }
            }
            other => {
                if let Some(v) = other.as_f64() {
                    *tm = Matrix::translate(-v / 1000.0 * size * gs.scale, 0.0).then(tm);
                    push_num(&mut out, v);
                }
            }
        }
    }
    (out, removed)
}

/// `Do`: `None` = leave the operator alone; `Some(None)` = drop it; `Some(Some(op))` = replace it.
fn xobject(
    doc: &mut Document,
    scope: &mut Scope<'_>,
    op: &Op,
    gs: &Gs,
    resources: &Dict,
    xobjects: &mut Dict,
    out: &mut Output,
) -> Option<Option<Op>> {
    let name = op.name(0)?.to_vec();
    let r = xobjects.get(&name)?.as_ref()?;
    let obj = doc.get(r);
    let Object::Stream(s) = &*obj else { return None };
    match s.dict.name(b"Subtype") {
        _ if s.dict.get(b"OC").is_some_and(|oc| scope.layer_hidden(doc, oc)) => {
            if scope.mode == Mode::Verify {
                return None;
            }
            scope.layer_blocks += 1;
            out.replaced.push(name);
            out.doomed.push(r);
            Some(None)
        }
        Some(b"Image") => {
            let b = gs.ctm.bbox([0.0, 0.0, 1.0, 1.0]);
            if !scope.hits(b) || scope.mode == Mode::Verify {
                return None;
            }
            if scope.covered(b) {
                scope.report.images_removed += 1;
                out.replaced.push(name);
                out.doomed.push(r);
                return Some(None);
            }
            match image::clear(doc, s, &gs.ctm, scope.rects) {
                Ok(None) => None,
                Ok(Some(new)) => {
                    let nr = doc.add(Object::Stream(new));
                    let n = fresh_name(scope, xobjects, XOBJECT_PREFIX);
                    xobjects.set(n.clone(), Object::Ref(nr));
                    out.xobjects.push((n.clone(), nr));
                    out.replaced.push(name);
                    out.doomed.push(r);
                    scope.report.images_cleared += 1;
                    Some(Some(Op::new("Do", vec![Object::Name(n)])))
                }
                Err(()) => {
                    scope.report.images_removed += 1;
                    out.replaced.push(name);
                    out.doomed.push(r);
                    Some(None)
                }
            }
        }
        Some(b"Form") => {
            let m = s.dict.get(b"Matrix").and_then(|m| Matrix::from_operands(doc.resolve(m).as_array()?)).unwrap_or_default();
            let fctm = m.then(&gs.ctm);
            let bbox = s.dict.get(b"BBox").and_then(|b| {
                let v: Vec<f64> = doc.resolve(b).as_array()?.iter().filter_map(Object::as_f64).collect();
                (v.len() == 4).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
            });
            if let Some(bb) = bbox
                && !scope.everywhere()
                && !scope.hits(fctm.bbox(bb))
            {
                return None;
            }
            if scope.depth >= MAX_DEPTH {
                if scope.mode == Mode::Verify {
                    return None;
                }
                scope.report.forms_removed += 1;
                out.replaced.push(name);
                out.doomed.push(r);
                return Some(None);
            }
            // Strict, with the chain resolved through the document: a form that does not decode
            // as a viewer shows it is removed rather than rewritten from other bytes.
            let data = match s.decoded_strict_in(doc) {
                Ok(d) => d,
                Err(_) if scope.mode == Mode::Apply => {
                    scope.report.forms_removed += 1;
                    out.replaced.push(name);
                    out.doomed.push(r);
                    return Some(None);
                }
                Err(_) => return None,
            };
            let own = s.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned());
            let res = own.clone().unwrap_or_else(|| resources.clone());
            let saved_fonts = std::mem::take(&mut scope.fonts);
            scope.depth += 1;
            // The form inherits the caller's graphics state (its font and size included) under
            // its own matrix.
            let mut inner = process_with(doc, scope, std::slice::from_ref(&data), &res, Gs { ctm: fctm, ..gs.clone() });
            scope.depth -= 1;
            scope.fonts = saved_fonts;
            out.residue += inner.residue;
            out.vertical |= inner.vertical;
            out.unreadable |= inner.unreadable;
            out.doomed.extend_from_slice(&inner.doomed);
            out.tag_changes.append(&mut inner.tag_changes);
            if scope.mode == Mode::Verify {
                return None;
            }
            let new_data = inner.streams.drain(..).next().flatten();
            let rewrite = inner.rewrite();
            if new_data.is_none() && rewrite.xobjects.is_empty() && rewrite.replaced.is_empty() {
                return None;
            }
            let mut dict = s.dict.clone();
            dict.remove(b"Length");
            let tags_res = tag_resources(doc, &res, &rewrite);
            // The copy gets /Resources of its own whenever they changed, so a `Do` or `BDC` the copy
            // no longer makes stops keeping the original reachable, and whenever the rewrite named
            // a resource at all, even if they came out the same: a form without /Resources of its
            // own would otherwise draw through its caller's, which the caller prunes for its own
            // content only.
            let mut res = res;
            let resources_changed = rebuild_resources(doc, &mut res, &rewrite, new_data.as_ref().map(std::slice::from_ref), &scope.hidden_layers);
            if resources_changed || rewrite.names_resources() {
                dict.set(b"Resources".to_vec(), Object::Dict(res));
            }
            let (changed, empty) = tags::touched(doc, &tags_res, &data, new_data.as_deref().unwrap_or(&data));
            let stream = match new_data {
                Some(d) => Stream::flate(dict, &d),
                None => Stream { dict, raw: s.raw.clone() },
            };
            let nr = doc.add(Object::Stream(stream));
            out.tag_changes.push((r, nr, changed, empty));
            let n = fresh_name(scope, xobjects, XOBJECT_PREFIX);
            xobjects.set(n.clone(), Object::Ref(nr));
            out.xobjects.push((n.clone(), nr));
            out.replaced.push(name);
            out.doomed.push(r);
            scope.report.forms_rewritten += 1;
            Some(Some(Op::new("Do", vec![Object::Name(n)])))
        }
        _ => None,
    }
}

/// A marked-content block that is open while the hidden layers are stripped.
struct Open {
    /// Its contents go: it is a hidden layer's block, or lies inside one.
    hidden: bool,
    /// Where its `BDC` went in the kept operators (when it was kept and not cleaned yet).
    bdc: Option<usize>,
}

/// What [`strip_hidden_layers`] left of a scope's operators, and what it took.
struct Stripped {
    /// The remaining operators.
    ops: Vec<Op>,
    /// How many hidden blocks went (one nested in another counts with it).
    blocks: usize,
    /// The XObject names of the `Do` operators that went with them, each once.
    dropped: Vec<Vec<u8>>,
}

/// Remove marked-content blocks of hidden layers (`/OC /name BDC … EMC`, nested blocks
/// included). A kept sequence around a removed block loses the text that stood for its content
/// (`clean_property_list`).
fn strip_hidden_layers(doc: &Document, scope: &mut Scope<'_>, ops: Vec<Op>, props: &Dict, out: &mut Output) -> Stripped {
    let mut kept = Vec::with_capacity(ops.len());
    let mut stack: Vec<Open> = Vec::new();
    let mut blocks = 0;
    let mut dropped: Vec<Vec<u8>> = Vec::new();
    for op in ops {
        let hidden_now = stack.last().is_some_and(|open| open.hidden);
        match op.op.as_slice() {
            b"BDC" | b"BMC" => {
                let this = op.is("BDC")
                    && op.name(0) == Some(b"OC")
                    && match op.operands.get(1) {
                        Some(Object::Name(n)) => props.get(n).is_some_and(|o| scope.layer_hidden(doc, o)),
                        Some(o) => scope.layer_hidden(doc, o),
                        None => false,
                    };
                if this && !hidden_now {
                    blocks += 1;
                    if scope.mode == Mode::Apply {
                        for open in stack.iter_mut() {
                            let Some(i) = open.bdc.take() else { continue };
                            if let Some(bdc) = kept.get_mut(i) {
                                clean_property_list(doc, scope, bdc, props, out);
                            }
                        }
                    }
                }
                let keep = !(hidden_now || this);
                stack.push(Open { hidden: hidden_now || this, bdc: (keep && op.is("BDC")).then_some(kept.len()) });
                if keep {
                    kept.push(op);
                }
            }
            b"EMC" => {
                let was_hidden = stack.pop().is_some_and(|open| open.hidden);
                if !was_hidden {
                    kept.push(op);
                }
            }
            _ => {
                if !hidden_now {
                    kept.push(op);
                } else if op.is("Do")
                    && let Some(n) = op.name(0)
                    && !dropped.iter().any(|d| d == n)
                {
                    dropped.push(n.to_vec());
                }
            }
        }
    }
    Stripped { ops: kept, blocks, dropped }
}

/// Drop the `/Properties` entries that name a hidden layer (an off OCG, or an OCMD all of whose
/// groups are off) once no surviving `BDC` in `content` names them, so the group dictionary and
/// its name leave the file with the content it hid. Entries a surviving `BDC` still names are
/// kept (a block that was not hidden can name the same property list).
fn prune_properties(doc: &Document, hidden_layers: &[ObjRef], props: &mut Dict, content: &[Vec<u8>]) {
    if hidden_layers.is_empty() {
        return;
    }
    let hidden: Vec<Vec<u8>> = props.iter().filter(|(_, v)| layer_hidden(doc, hidden_layers, v)).map(|(n, _)| n.to_vec()).collect();
    prune_names(props, &hidden, content, "BDC", 1);
}
