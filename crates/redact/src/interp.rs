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
use pdfcraft_cos::{CosError, Dict, Document, ObjRef, Object, Stream};

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
    /// The current font writes vertically (a Type0 font with an `-V` encoding or a CMap whose
    /// `/WMode` is 1). Glyph layout here is horizontal, so such text cannot be placed under a
    /// region; apply fails closed when it meets any (see `show`).
    vertical: bool,
    size: f64,
    char_spacing: f64,
    word_spacing: f64,
    render_mode: i64,
    scale: f64,
    leading: f64,
    rise: f64,
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
    /// Original property values retired by this scope or a nested form. Other resource holders
    /// may still name them; unlink only entries no surviving content uses.
    pub retired_properties: Vec<Object>,
    /// Glyphs / inline images still overlapping a region (verify mode).
    pub residue: usize,
    /// Apply mode met vertical-writing text (which it cannot remove) while a redaction region was
    /// in force: the operation must fail closed rather than leave that text under the mark.
    pub vertical: bool,
    /// The content holds an inline image the renderer reads no image for
    /// ([`pdfcraft_content::Parsed::ambiguous_images`]): what this scope placed after it may not
    /// be what the page shows, so a redaction must not trust it.
    pub ambiguous: bool,
    pub tag_changes: Vec<(ObjRef, ObjRef, HashSet<i64>, HashSet<i64>)>,
    pub error: Option<CosError>,
}

pub(crate) struct Scope<'a> {
    pub rects: &'a [[f64; 4]],
    pub mode: Mode,
    pub report: &'a mut Report,
    /// Sanitize: remove hidden text (invisible render modes, or wholly outside this page box).
    pub hidden_text: Option<[f64; 4]>,
    /// Sanitize: optional content groups that are off; content marked with them is removed.
    pub hidden_layers: HashSet<ObjRef>,
    /// Whether this pass is evaluating optional-content rules. An empty off set is meaningful:
    /// policies such as `/AllOff` can still hide content when every group is on.
    pub layers_enabled: bool,
    /// Off groups referenced by optional content that remains visible and therefore still needs
    /// its original default state after sanitization.
    pub retained_off_layers: HashSet<ObjRef>,
    /// Cache membership results so repeated blocks cannot repeatedly walk hostile expressions.
    ocmd_cache: HashMap<ObjRef, bool>,
    /// Direct `/OC` dictionaries on XObjects have no object reference of their own. Cache them
    /// by the containing XObject so repeated `Do` calls share the same bounded evaluation.
    xobject_oc_cache: HashMap<ObjRef, bool>,
    /// Content removed from hidden layers (blocks and XObjects).
    pub layer_blocks: usize,
    fonts: HashMap<Vec<u8>, Rc<Metrics>>,
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
            hidden_layers: HashSet::new(),
            layers_enabled: false,
            retained_off_layers: HashSet::new(),
            ocmd_cache: HashMap::new(),
            xobject_oc_cache: HashMap::new(),
            layer_blocks: 0,
            fonts: HashMap::new(),
            used_names: Vec::new(),
            depth: 0,
        }
    }

    /// Sanitizing visits every form XObject (not only those under a region).
    fn everywhere(&self) -> bool {
        self.hidden_text.is_some() || self.layers_enabled
    }

    /// Is this optional-content reference (an OCG or OCMD) hidden?
    fn layer_hidden(&mut self, doc: &Document, o: &Object) -> bool {
        if !self.layers_enabled {
            return false;
        }
        let reference = o.as_ref();
        if let Some(r) = reference {
            if let Some(hidden) = self.ocmd_cache.get(&r) {
                return *hidden;
            }
            if self.hidden_layers.contains(&r) {
                self.ocmd_cache.insert(r, true);
                return true;
            }
        }
        // Evaluate the policy against the same default configuration used to select hidden
        // groups. A membership dictionary's default policy is AnyOn.
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
        let fail_closed = || groups.iter().any(|g| self.hidden_layers.contains(g));
        let hidden = if let Some(expression) = d.get(b"VE") {
            let visible = self.visibility_expression(doc, expression, 0, &mut HashSet::new(), &mut 4096);
            match visible {
                Some(visible) => !visible,
                None => fail_closed() || self.expression_mentions_hidden(doc, expression, 0, &mut HashSet::new(), &mut 4096),
            }
        } else if groups.is_empty() {
            false
        } else {
            let any_on = groups.iter().any(|g| !self.hidden_layers.contains(g));
            let all_on = groups.iter().all(|g| !self.hidden_layers.contains(g));
            let policy = d.get(b"P").map(|p| doc.resolve(p)).and_then(|p| p.as_name().map(|name| name.to_vec()));
            match (d.get(b"P"), policy.as_deref()) {
                (None, _) | (Some(_), Some(b"AnyOn")) => !any_on,
                (Some(_), Some(b"AllOn")) => !all_on,
                (Some(_), Some(b"AnyOff")) => all_on,
                (Some(_), Some(b"AllOff")) => any_on,
                // An invalid policy must not make a candidate hidden block survive sanitization.
                _ => fail_closed(),
            }
        };
        if !hidden {
            self.retained_off_layers.extend(groups.iter().filter(|g| self.hidden_layers.contains(g)).copied());
            if let Some(expression) = d.get(b"VE") {
                let mut referenced = HashSet::new();
                if Self::collect_hidden_expression_groups(doc, expression, &self.hidden_layers, 0, &mut HashSet::new(), &mut 4096, &mut referenced) {
                    self.retained_off_layers.extend(referenced);
                } else {
                    // If an expression cannot be fully walked, preserve every off group so that
                    // the content we kept does not change visibility after configuration cleanup.
                    self.retained_off_layers.extend(self.hidden_layers.iter().copied());
                }
            }
        }
        if let Some(r) = reference {
            self.ocmd_cache.insert(r, hidden);
        }
        hidden
    }

    fn xobject_layer_hidden(&mut self, doc: &Document, xobject: ObjRef, oc: &Object) -> bool {
        if oc.as_ref().is_some() {
            return self.layer_hidden(doc, oc);
        }
        if let Some(hidden) = self.xobject_oc_cache.get(&xobject) {
            return *hidden;
        }
        let hidden = self.layer_hidden(doc, oc);
        self.xobject_oc_cache.insert(xobject, hidden);
        hidden
    }

    fn collect_hidden_expression_groups(
        doc: &Document,
        expression: &Object,
        hidden_layers: &HashSet<ObjRef>,
        depth: usize,
        path: &mut HashSet<ObjRef>,
        budget: &mut usize,
        found: &mut HashSet<ObjRef>,
    ) -> bool {
        if depth > 64 || *budget == 0 {
            return false;
        }
        *budget -= 1;
        let reference = expression.as_ref();
        if let Some(r) = reference
            && !path.insert(r)
        {
            return false;
        }
        let resolved = doc.resolve(expression);
        let complete = match &*resolved {
            Object::Dict(d) if d.name(b"Type") == Some(b"OCG") => {
                if let Some(r) = reference
                    && hidden_layers.contains(&r)
                {
                    found.insert(r);
                }
                reference.is_some()
            }
            Object::Array(items) => {
                let Some(Object::Name(operator)) = items.first() else { return false };
                let operands = match operator.as_slice() {
                    b"And" | b"Or" if (2..=4097).contains(&items.len()) => items.get(1..),
                    b"Not" if items.len() == 2 => items.get(1..),
                    _ => None,
                };
                operands.is_some_and(|operands| {
                    operands.iter().all(|item| Self::collect_hidden_expression_groups(doc, item, hidden_layers, depth + 1, path, budget, found))
                })
            }
            _ => false,
        };
        if let Some(r) = reference {
            path.remove(&r);
        }
        complete
    }

    /// Evaluate an optional-content visibility expression (`/VE`) against the default config.
    /// Malformed, cyclic, or excessive expressions return `None` so the caller can fail closed.
    fn visibility_expression(
        &self,
        doc: &Document,
        expression: &Object,
        depth: usize,
        path: &mut HashSet<ObjRef>,
        budget: &mut usize,
    ) -> Option<bool> {
        if depth > 64 || *budget == 0 {
            return None;
        }
        *budget -= 1;
        let reference = expression.as_ref();
        if let Some(r) = reference
            && !path.insert(r)
        {
            return None;
        }
        let resolved = doc.resolve(expression);
        let result = match &*resolved {
            Object::Dict(d) if d.name(b"Type") == Some(b"OCG") => reference.map(|r| !self.hidden_layers.contains(&r)),
            Object::Array(items) if (2..=4097).contains(&items.len()) => {
                let operator = match items.first() {
                    Some(Object::Name(n)) => n.as_slice(),
                    _ => return None,
                };
                match operator {
                    b"And" if items.len() >= 2 => {
                        let mut visible = true;
                        for item in items.iter().skip(1) {
                            visible &= self.visibility_expression(doc, item, depth + 1, path, budget)?;
                        }
                        Some(visible)
                    }
                    b"Or" if items.len() >= 2 => {
                        let mut visible = false;
                        for item in items.iter().skip(1) {
                            visible |= self.visibility_expression(doc, item, depth + 1, path, budget)?;
                        }
                        Some(visible)
                    }
                    b"Not" if items.len() == 2 => Some(!self.visibility_expression(doc, &items[1], depth + 1, path, budget)?),
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(r) = reference {
            path.remove(&r);
        }
        result
    }

    /// Conservatively find hidden groups in an expression that could not be evaluated. Unknown
    /// syntax, cycles, and exhausted budgets count as a possible hidden reference.
    fn expression_mentions_hidden(&self, doc: &Document, expression: &Object, depth: usize, path: &mut HashSet<ObjRef>, budget: &mut usize) -> bool {
        if depth > 64 || *budget == 0 {
            return true;
        }
        *budget -= 1;
        if let Some(r) = expression.as_ref() {
            if self.hidden_layers.contains(&r) {
                return true;
            }
            if !path.insert(r) {
                return true;
            }
        }
        let resolved = doc.resolve(expression);
        let found = match &*resolved {
            Object::Array(items) => items.iter().any(|item| self.expression_mentions_hidden(doc, item, depth + 1, path, budget)),
            Object::Dict(d) if d.name(b"Type") == Some(b"OCG") => expression.as_ref().is_none_or(|r| self.hidden_layers.contains(&r)),
            Object::Dict(_) => true,
            _ => false,
        };
        if let Some(r) = expression.as_ref() {
            path.remove(&r);
        }
        found
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

/// Does this font write vertically? Only a Type0 (composite) font can: its `/Encoding` is a
/// predefined CMap name ending in `-V` (Identity-V and the like), or an embedded CMap stream
/// whose `/WMode` is 1 (ISO 32000-2 §9.7.5).
fn is_vertical(doc: &Document, font: &Dict) -> bool {
    if font.name(b"Subtype") != Some(b"Type0".as_slice()) {
        return false;
    }
    match font.get(b"Encoding").map(|e| doc.resolve(e)).as_deref() {
        Some(Object::Name(n)) => n.ends_with(b"-V"),
        Some(Object::Stream(s)) => s.dict.int(b"WMode") == Some(1),
        _ => false,
    }
}

fn fresh_name(scope: &mut Scope<'_>, xobjects: &Dict) -> Vec<u8> {
    let mut i = scope.used_names.len() + 1;
    loop {
        let n = format!("PCRedacted{i}").into_bytes();
        if !xobjects.contains(&n) && !scope.used_names.contains(&n) {
            scope.used_names.push(n.clone());
            return n;
        }
        i += 1;
    }
}

/// Drop every `replaced` resource name whose `Do` no longer appears in the rewritten `content`,
/// so a full-rewrite save stops reaching the original image or form it named. A name that some
/// surviving `Do` still draws (the same image shown again outside a mark) is kept.
pub(crate) fn prune(xo: &mut Dict, replaced: &[Vec<u8>], content: &[Vec<u8>]) {
    if replaced.is_empty() {
        return;
    }
    let joined = Pieces::join(content);
    let ops = joined.parse().ops;
    let still_drawn = |n: &[u8]| ops.iter().any(|op| op.is("Do") && op.name(0) == Some(n));
    for n in replaced {
        if !still_drawn(n) {
            xo.remove(n);
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
        let Ok(data) = s.decoded() else { return None };
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
/// left as it is for them. Retired properties are likewise unlinked only when none of the
/// holder's pages or inheriting forms names them. Fails closed when use cannot be determined.
pub(crate) fn unlink_doomed(doc: &mut Document, doomed: &[ObjRef], retired_properties: &[Object]) -> Result<(), RedactError> {
    if doomed.is_empty() && retired_properties.is_empty() {
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
    let holder_nodes: Vec<ObjRef> = holders.iter().map(|(node, _)| *node).collect();
    for (node, users) in holders {
        let Some(mut res) = doc.get(node).as_dict().and_then(|d| d.get(b"Resources")).and_then(|r| doc.resolve(r).as_dict().cloned()) else {
            continue;
        };
        let mut xo = res_dict(doc, &res, b"XObject");
        let names: Vec<Vec<u8>> = xo.iter().filter(|(_, v)| v.as_ref().is_some_and(|r| doomed.contains(&r))).map(|(n, _)| n.to_vec()).collect();
        let properties = res_dict(doc, &res, b"Properties");
        let property_names: Vec<Vec<u8>> = properties.iter().filter(|(_, v)| retired_properties.contains(v)).map(|(n, _)| n.to_vec()).collect();
        if names.is_empty() && property_names.is_empty() {
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
            res.set(b"XObject".to_vec(), Object::Dict(xo));
        }
        pruned |= prune_retired_properties(doc, &mut res, &property_names, &contents)?;
        if pruned {
            doc.update_dict(node, |d| d.set(b"Resources".to_vec(), Object::Dict(res)))?;
        }
    }
    // A surviving form can own the same old resource dictionary as a rewritten page. Its
    // unused descriptions and doomed XObjects remain reachable too, even after every page
    // holder is cleaned.
    unlink_from_forms(doc, &holder_nodes, &doomed, retired_properties)?;
    Ok(())
}

/// Retire only properties removed from a rewritten scope, preserving names still used by any
/// content sharing this holder. Different pages' streams must never be concatenated together.
fn prune_retired_properties(doc: &Document, res: &mut Dict, names: &[Vec<u8>], contents: &[Vec<Vec<u8>>]) -> Result<bool, CosError> {
    if names.is_empty() {
        return Ok(false);
    }
    let mut used = HashSet::new();
    for content in contents {
        for op in resource_ops(doc, res, content)? {
            if matches!(op.op.as_slice(), b"BDC" | b"DP")
                && let Some(name) = op.name(1)
            {
                used.insert(name.to_vec());
            }
        }
    }
    let mut properties = res_dict(doc, res, b"Properties");
    let mut pruned = false;
    for name in names {
        if !used.contains(name) {
            pruned |= properties.remove(name).is_some();
        }
    }
    if pruned {
        if properties.is_empty() {
            res.remove(b"Properties");
        } else {
            res.set(b"Properties".to_vec(), Object::Dict(properties));
        }
    }
    Ok(pruned)
}

/// Unlink retired properties and `doomed` XObjects from the resources of the forms reachable from
/// the page-tree `holders`, each unless the form's own content (or a form inheriting its
/// resources) still uses the name. Fails closed when that cannot be determined.
fn unlink_from_forms(doc: &mut Document, holders: &[ObjRef], doomed: &HashSet<ObjRef>, retired: &[Object]) -> Result<(), CosError> {
    if retired.is_empty() && doomed.is_empty() {
        return Ok(());
    }
    let mut pending: Vec<Dict> = holders
        .iter()
        .filter_map(|r| doc.get(*r).as_dict().and_then(|d| d.get(b"Resources")).and_then(|r| doc.resolve(r).as_dict().cloned()))
        .collect();
    let mut seen = HashSet::new();
    while let Some(resources) = pending.pop() {
        for (_, value) in res_dict(doc, &resources, b"XObject").iter() {
            let Some(reference) = value.as_ref() else { continue };
            if !seen.insert(reference) {
                continue;
            }
            if seen.len() > 1_000_000 {
                return Err(CosError::Syntax { offset: 0, detail: "redaction could not fully inspect shared form resources".into() });
            }
            let object = doc.get(reference);
            let Object::Stream(form) = &*object else { continue };
            if form.dict.name(b"Subtype") != Some(b"Form") {
                continue;
            }
            let Some(mut res) = form.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()) else { continue };
            let names: Vec<Vec<u8>> =
                res_dict(doc, &res, b"Properties").iter().filter(|(_, value)| retired.contains(value)).map(|(name, _)| name.to_vec()).collect();
            let mut xo = res_dict(doc, &res, b"XObject");
            let doomed_names: Vec<Vec<u8>> =
                xo.iter().filter(|(_, v)| v.as_ref().is_some_and(|r| doomed.contains(&r))).map(|(n, _)| n.to_vec()).collect();
            if names.is_empty() && doomed_names.is_empty() {
                pending.push(res);
                continue;
            }
            let data = form.decoded()?;
            let mut pruned = false;
            for name in doomed_names {
                match content_draws(doc, std::slice::from_ref(&data), &xo, &name, 0, &mut HashSet::new()) {
                    Some(true) => {}
                    Some(false) => pruned |= xo.remove(&name).is_some(),
                    None => return Err(CosError::Syntax { offset: 0, detail: "redaction could not fully inspect shared form resources".into() }),
                }
            }
            if pruned {
                res.set(b"XObject".to_vec(), Object::Dict(xo));
            }
            pruned |= prune_retired_properties(doc, &mut res, &names, &[vec![data]])?;
            if pruned {
                doc.update_dict(reference, |d| d.set(b"Resources".to_vec(), Object::Dict(res.clone())))?;
            }
            pending.push(res);
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

fn resource_ops(doc: &Document, res: &Dict, content: &[Vec<u8>]) -> Result<Vec<Op>, CosError> {
    let parsed = Pieces::join(content).parse();
    if parsed.skipped > 0 || parsed.ops.len() > 2_000_000 {
        return Err(CosError::Syntax { offset: 0, detail: "redaction could not fully inspect property resources".into() });
    }
    let mut ops = parsed.ops;
    let xobjects = res_dict(doc, res, b"XObject");
    let mut seen = HashSet::new();
    let mut cursor = 0;
    while cursor < ops.len() {
        let op = &ops[cursor];
        if op.is("Do")
            && let Some(r) = op.name(0).and_then(|n| xobjects.get(n)).and_then(Object::as_ref)
            && seen.insert(r)
            && let Object::Stream(s) = &*doc.get(r)
            && s.dict.name(b"Subtype") == Some(b"Form")
            && s.dict.get(b"Resources").is_none_or(|r| doc.resolve(r).as_dict().is_none())
        {
            let parsed = pdfcraft_content::parse(&s.decoded()?);
            if seen.len() > 2_000_000 || ops.len().saturating_add(parsed.ops.len()) > 2_000_000 || parsed.skipped > 0 {
                return Err(CosError::Syntax { offset: 0, detail: "redaction could not fully inspect inherited form resources".into() });
            }
            ops.extend(parsed.ops);
        }
        cursor += 1;
    }
    Ok(ops)
}

pub(crate) fn prune_properties(doc: &Document, res: &mut Dict, content: &[Vec<u8>]) -> Result<Vec<Object>, CosError> {
    let mut properties = res_dict(doc, res, b"Properties");
    if properties.is_empty() {
        return Ok(Vec::new());
    }
    let mut retired = Vec::new();
    let ops = resource_ops(doc, res, content)?;
    let names: Vec<Vec<u8>> = properties.iter().map(|(n, _)| n.to_vec()).collect();
    for name in names {
        if !ops.iter().any(|op| matches!(op.op.as_slice(), b"BDC" | b"DP") && op.name(1) == Some(&name))
            && let Some(value) = properties.remove(&name)
        {
            retired.push(value);
        }
    }
    if properties.is_empty() {
        res.remove(b"Properties");
    } else {
        res.set(b"Properties".to_vec(), Object::Dict(properties));
    }
    Ok(retired)
}

fn prune_form_resources(doc: &Document, res: &mut Dict, content: &[Vec<u8>]) -> Result<(), CosError> {
    let ops = resource_ops(doc, res, content)?;
    let mut xo = res_dict(doc, res, b"XObject");
    let names: Vec<Vec<u8>> = xo.iter().map(|(n, _)| n.to_vec()).collect();
    for name in names {
        if !ops.iter().any(|op| op.is("Do") && op.name(0) == Some(&name)) {
            xo.remove(&name);
        }
    }
    res.set(b"XObject".to_vec(), Object::Dict(xo));
    Ok(())
}

/// The font, writing direction and size an ExtGState's `/Font [font size]` entry sets, as `Tf`
/// would (ISO 32000-2 §8.4.5); `None` when `name` sets no font. The metrics are cached with the
/// fonts `Tf` names, under a key no resource name can take (a name cannot hold a null byte).
fn extgstate_font(
    doc: &Document,
    scope: &mut Scope<'_>,
    ext_gstates: &Dict,
    name: &[u8],
    fallback: &Rc<Metrics>,
) -> Option<(Rc<Metrics>, bool, f64)> {
    let state = doc.resolve(ext_gstates.get(name)?).as_dict().cloned()?;
    let entry = doc.resolve(state.get(b"Font")?).as_array().cloned()?;
    let [font, size] = entry.as_slice() else { return None };
    let size = doc.resolve(size).as_f64()?;
    let font_dict = doc.resolve(font).as_dict().cloned();
    let vertical = font_dict.as_ref().is_some_and(|d| is_vertical(doc, d));
    let key = [b"\0".as_slice(), name].concat();
    let metrics = match scope.fonts.get(&key) {
        Some(m) => m.clone(),
        None => {
            let m = font_dict.as_ref().map(|d| Rc::new(Metrics::from_dict(doc, d))).unwrap_or_else(|| fallback.clone());
            scope.fonts.insert(key, m.clone());
            m
        }
    };
    Some((metrics, vertical, size))
}

/// One glyph of a shown string.
struct Glyph {
    bytes: std::ops::Range<usize>,
    /// Advance in unscaled text space (before `Th`).
    advance: f64,
    hit: bool,
}

/// Process a scope's content streams (state flows from one stream to the next).
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
    process_from(doc, scope, streams, resources, gs)
}

/// [`process`] from graphics state `gs`: a form XObject starts with its caller's, text state
/// included (ISO 32000-2 §8.10.1), so text it shows without setting a font of its own is placed
/// with the caller's font, size and spacing, as the renderer places it.
fn process_from(doc: &mut Document, scope: &mut Scope<'_>, streams: &[Vec<u8>], resources: &Dict, mut gs: Gs) -> Output {
    let mut out = Output::default();
    let fallback = Rc::new(Metrics::fallback());
    let mut stack: Vec<Gs> = Vec::new();
    let (mut tm, mut tlm) = (Matrix::IDENTITY, Matrix::IDENTITY);
    let fonts_res = res_dict(doc, resources, b"Font");
    let ext_gstates = res_dict(doc, resources, b"ExtGState");
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
    out.ambiguous = parsed.ambiguous_images > 0;
    // A rewritten piece loses its comments, so keep where they began and ended: a comment that
    // ran on into the next piece still hides that piece's first line.
    let comment_edges = joined.comment_edges(&parsed);
    // Layer removal also changes enclosing replacement spans, so remember the operators before
    // any cleanup, not just the survivors of the layer pass.
    let original = parsed.ops.clone();
    if scope.layers_enabled {
        let props = res_dict(doc, resources, b"Properties");
        let (kept, removed) = strip_hidden_layers(doc, scope, parsed.ops, &props);
        parsed.ops = kept;
        if removed > 0 {
            scope.layer_blocks += removed;
            forced_change = true;
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

    let mut processed = Vec::new();
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
                        gs.vertical = font_dict.as_ref().is_some_and(|d| is_vertical(doc, d));
                        gs.font = match scope.fonts.get(name) {
                            Some(f) => f.clone(),
                            None => {
                                let m = font_dict.as_ref().map(|d| Rc::new(Metrics::from_dict(doc, d))).unwrap_or_else(|| fallback.clone());
                                scope.fonts.insert(name.to_vec(), m.clone());
                                m
                            }
                        };
                    }
                }
                b"gs" => {
                    if let Some((font, vertical, size)) = op.name(0).and_then(|n| extgstate_font(doc, scope, &ext_gstates, n, &fallback)) {
                        (gs.font, gs.vertical, gs.size) = (font, vertical, size);
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
                        // A surplus array (`[(A)] [(B)] TJ`) is malformed, but the operator still
                        // takes the one immediately before it, as the renderer and every other
                        // reader here do (#3569), so a redaction search matches what is drawn.
                        b"TJ" => op.operands.last().and_then(Object::as_array).cloned().unwrap_or_default(),
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
                            continue;
                        }
                    }
                }
                b"sh" if scope.mode == Mode::Apply => {
                    // A shading fills the current clip: clip the regions out.
                    if let Some(c) = clip_out(scope.rects, &gs.ctm, [-1e6, -1e6, 1e6, 1e6]) {
                        changed = true;
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
                        if let Some(n) = new {
                            ops.push(n);
                        }
                        continue;
                    }
                }
                _ => {}
            }
            ops.push(op);
        }
        if !path.is_empty() {
            ops.append(&mut path);
        }
        processed.push((ops, changed));
    }
    if scope.mode == Mode::Apply {
        clean_replacements(doc, resources, &original, &mut processed, scope.report);
    }
    for ((ops, changed), (comment_in, comment_out)) in processed.into_iter().zip(comment_edges) {
        out.streams.push(changed.then(|| {
            // The piece before still ends inside a comment: end it before the first operator.
            let mut bytes = if comment_in { b"\n".to_vec() } else { Vec::new() };
            bytes.extend(serialize_ops(&ops));
            // The piece ended inside a comment that hid the start of the next one: keep it open.
            if comment_out {
                bytes.push(b'%');
            }
            bytes
        }));
    }
    out
}

fn clean_replacements(doc: &Document, resources: &Dict, original: &[Op], processed: &mut [(Vec<Op>, bool)], report: &mut Report) {
    let originals: HashMap<usize, &Op> = original.iter().map(|op| (op.span.start, op)).collect();
    let unchanged: HashSet<usize> =
        processed.iter().flat_map(|(ops, _)| ops).filter(|op| originals.get(&op.span.start) == Some(op)).map(|op| op.span.start).collect();
    let mut stack = Vec::new();
    let mut affected = HashSet::new();
    for op in original {
        match op.op.as_slice() {
            b"BDC" | b"BMC" => stack.push(op.span.start),
            b"EMC" => {
                stack.pop();
            }
            _ if !unchanged.contains(&op.span.start) => affected.extend(stack.iter().copied()),
            _ => {}
        }
    }
    let properties = res_dict(doc, resources, b"Properties");
    for (ops, changed) in processed {
        for op in ops {
            if !op.is("BDC") || !affected.contains(&op.span.start) {
                continue;
            }
            let property = match op.operands.get(1) {
                Some(Object::Name(name)) => properties.get(name).map(|p| doc.resolve(p)),
                Some(p) => Some(doc.resolve(p)),
                None => None,
            };
            if let Some(mut dict) = property.and_then(|p| p.as_dict().cloned()) {
                let mut stripped = false;
                for key in [&b"ActualText"[..], b"Alt", b"E", b"T"] {
                    stripped |= dict.remove(key).is_some();
                }
                if stripped && let Some(property) = op.operands.get_mut(1) {
                    *property = Object::Dict(dict);
                    *changed = true;
                    report.replacement_text += 1;
                }
            }
        }
    }
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
        _ if s.dict.get(b"OC").is_some_and(|oc| scope.xobject_layer_hidden(doc, r, oc)) => {
            // Verify mode must count hidden XObjects too: scan uses that count to decide
            // whether the HiddenLayers pass is necessary.
            scope.layer_blocks += 1;
            if scope.mode == Mode::Verify {
                return None;
            }
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
                    let n = fresh_name(scope, xobjects);
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
                    // Sanitization removes a Form it cannot enter, so the verification scan
                    // must count that conservative removal or it may decide to skip the pass.
                    if scope.everywhere() {
                        scope.layer_blocks += 1;
                    }
                    return None;
                }
                scope.report.forms_removed += 1;
                out.replaced.push(name);
                out.doomed.push(r);
                return Some(None);
            }
            // Strict, like page content: a Form read only in part counts as undecodable.
            let data = match s.decoded_strict() {
                Ok(d) => d,
                Err(_) if scope.mode == Mode::Apply => {
                    scope.report.forms_removed += 1;
                    out.replaced.push(name);
                    out.doomed.push(r);
                    return Some(None);
                }
                Err(_) => {
                    // Apply drops an undecodable Form while sanitizing. Count it during Verify
                    // so a zero-count preflight cannot leave its potentially hidden bytes behind.
                    if scope.everywhere() {
                        scope.layer_blocks += 1;
                    }
                    return None;
                }
            };
            let own = s.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned());
            let res = own.clone().unwrap_or_else(|| resources.clone());
            let saved_fonts = std::mem::take(&mut scope.fonts);
            scope.depth += 1;
            let inner = process_from(doc, scope, std::slice::from_ref(&data), &res, Gs { ctm: fctm, ..gs.clone() });
            scope.depth -= 1;
            scope.fonts = saved_fonts;
            out.residue += inner.residue;
            out.vertical |= inner.vertical;
            out.ambiguous |= inner.ambiguous;
            out.doomed.extend_from_slice(&inner.doomed);
            out.retired_properties.extend(inner.retired_properties);
            out.tag_changes.extend(inner.tag_changes);
            out.error = inner.error.or(out.error.take());
            if scope.mode == Mode::Verify {
                return None;
            }
            let new_data = inner.streams.into_iter().next().flatten();
            if new_data.is_none() && inner.xobjects.is_empty() && inner.replaced.is_empty() {
                return None;
            }
            let mut dict = s.dict.clone();
            dict.remove(b"Length");
            let mut cleaned_res = res.clone();
            // Install rewritten nested forms before finding live property names: a new form may
            // still inherit its caller's resources, and its new Do name must resolve here.
            if !inner.xobjects.is_empty() || !inner.replaced.is_empty() {
                let mut xo = res_dict(doc, &cleaned_res, b"XObject");
                if let Some(data) = &new_data {
                    prune(&mut xo, &inner.replaced, std::slice::from_ref(data));
                }
                for (name, reference) in &inner.xobjects {
                    xo.set(name.clone(), Object::Ref(*reference));
                }
                cleaned_res.set(b"XObject".to_vec(), Object::Dict(xo));
            }
            if let Some(data) = &new_data {
                let content = std::slice::from_ref(data);
                match prune_properties(doc, &mut cleaned_res, content) {
                    Ok(retired) => out.retired_properties.extend(retired),
                    Err(error) => {
                        out.error = Some(error);
                        return None;
                    }
                }
                if let Err(error) = prune_form_resources(doc, &mut cleaned_res, content) {
                    out.error = Some(error);
                    return None;
                }
            }
            if cleaned_res != res {
                dict.set(b"Resources".to_vec(), Object::Dict(cleaned_res));
            }
            let (changed, empty) = tags::touched(doc, &res, &data, new_data.as_deref().unwrap_or(&data));
            let stream = match new_data {
                Some(d) => Stream::flate(dict, &d),
                None => Stream { dict, raw: s.raw.clone() },
            };
            let nr = doc.add(Object::Stream(stream));
            out.tag_changes.push((r, nr, changed, empty));
            let n = fresh_name(scope, xobjects);
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

/// Remove marked-content blocks of hidden layers (`/OC /name BDC … EMC`, nested blocks
/// included). Returns the remaining operators and how many blocks went.
fn strip_hidden_layers(doc: &Document, scope: &mut Scope<'_>, ops: Vec<Op>, props: &Dict) -> (Vec<Op>, usize) {
    let mut out = Vec::with_capacity(ops.len());
    // For each open marked-content block: does it hide its contents?
    let mut stack: Vec<bool> = Vec::new();
    let mut removed = 0;
    for op in ops {
        let hidden_now = stack.last().copied().unwrap_or(false);
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
                    removed += 1;
                }
                stack.push(hidden_now || this);
                if !(hidden_now || this) {
                    out.push(op);
                }
            }
            b"EMC" => {
                let was = stack.pop().unwrap_or(false);
                if !was {
                    out.push(op);
                }
            }
            _ => {
                if !hidden_now {
                    out.push(op);
                }
            }
        }
    }
    (out, removed)
}
