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
use pdfcraft_cos::{CosError, DecodeBudget, Dict, Document, MAX_PAGE_FORMS, ObjRef, Object, Stream};

use crate::{RedactError, Report, image, page_streams, tags};
use pdfcraft_fonts::pdf::Metrics;

mod layers;
mod prune;

use layers::strip_hidden_layers;
use prune::{clip_out, extgstate_font, prune_form_resources};
pub(crate) use prune::{prune, prune_extgstates, prune_patterns, prune_properties, resource_ops, unlink_doomed};

/// How deep this pass follows form XObjects. A form past it is removed rather than walked, which
/// is the fail-safe choice for a redaction -- marked content inside a form we cannot read would
/// otherwise survive -- but it also removes whatever else that form drew, marked or not. So the
/// limit has to reach as deep as anything the reader can see: the renderer follows forms to 50
/// (`MAX_NESTED_INTERPRETATION_DEPTH`) and so does Edit text's reader (`text::MAX_FORM_DEPTH`).
/// At 12, a page with a 13-deep chain lost the text of its innermost form even with nothing in it
/// marked (#3645); past 50 nothing is drawn by any of the three, so removing it costs nothing
/// visible.
pub(crate) const MAX_DEPTH: usize = 50;

// A form a reader still shows must be walkable here, or a redaction removes content that is
// visible; the build says so rather than a test, so the two limits cannot drift apart.
const _: () = assert!(MAX_DEPTH >= pdfcraft_edit::text::MAX_FORM_DEPTH);

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
    /// When the active font was set by an `/ExtGState` `/Font` entry (`gs`), the resource name of
    /// that graphics state dictionary (cleared when `Tf` sets a font).
    font_gs: Option<Vec<u8>>,
    /// Active pattern names selected by `scn` (non-stroking) and `SCN` (stroking).
    fill_pattern: Option<Vec<u8>>,
    stroke_pattern: Option<Vec<u8>>,
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
    /// New Pattern resources the rewritten content refers to: (resource name, object).
    pub patterns: Vec<(Vec<u8>, ObjRef)>,
    /// Pattern resource names dropped or replaced in this scope.
    pub replaced_patterns: Vec<Vec<u8>>,
    /// New ExtGState resources the rewritten content refers to: (resource name, object).
    pub ext_gstates: Vec<(Vec<u8>, ObjRef)>,
    /// ExtGState resource names dropped or replaced in this scope.
    pub replaced_ext_gstates: Vec<Vec<u8>>,
    /// The original objects behind `replaced`, nested scopes included. Other resource
    /// dictionaries may name them too (an inherited or shared /Resources, or the caller's when a
    /// form has none of its own): [`unlink_doomed`] removes those entries.
    pub doomed: Vec<ObjRef>,
    /// Rewritten form/image objects, including nested replacements, that may retain parent-tree keys.
    pub replacements: Vec<ObjRef>,
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

impl Output {
    pub fn has_resource_changes(&self) -> bool {
        !self.xobjects.is_empty()
            || !self.replaced.is_empty()
            || !self.patterns.is_empty()
            || !self.replaced_patterns.is_empty()
            || !self.ext_gstates.is_empty()
            || !self.replaced_ext_gstates.is_empty()
    }

    pub fn apply_resources(&self, doc: &Document, res: &mut Dict, content: &[Vec<u8>]) -> Result<(), CosError> {
        // A form with no /Resources of its own draws these names through this dictionary.
        // Read that use before any entry is removed; an unreadable form keeps the names by
        // failing the redaction rather than dropping a font or pattern it still paints.
        let inherited =
            if self.replaced_patterns.is_empty() && self.replaced_ext_gstates.is_empty() { None } else { Some(resource_ops(doc, res, content)?) };
        if !self.xobjects.is_empty() || !self.replaced.is_empty() {
            let mut xo = res_dict(doc, res, b"XObject");
            prune(&mut xo, &self.replaced, content);
            for (n, r) in &self.xobjects {
                xo.set(n.clone(), Object::Ref(*r));
            }
            res.set(b"XObject".to_vec(), Object::Dict(xo));
        }
        if !self.patterns.is_empty() || !self.replaced_patterns.is_empty() {
            let mut pat = res_dict(doc, res, b"Pattern");
            if let Some(ops) = &inherited {
                prune_patterns(&mut pat, &self.replaced_patterns, ops);
            }
            for (n, r) in &self.patterns {
                pat.set(n.clone(), Object::Ref(*r));
            }
            if pat.is_empty() {
                res.remove(b"Pattern");
            } else {
                res.set(b"Pattern".to_vec(), Object::Dict(pat));
            }
        }
        if !self.ext_gstates.is_empty() || !self.replaced_ext_gstates.is_empty() {
            let mut egs = res_dict(doc, res, b"ExtGState");
            if let Some(ops) = &inherited {
                prune_extgstates(&mut egs, &self.replaced_ext_gstates, ops);
            }
            for (n, r) in &self.ext_gstates {
                egs.set(n.clone(), Object::Ref(*r));
            }
            if egs.is_empty() {
                res.remove(b"ExtGState");
            } else {
                res.set(b"ExtGState".to_vec(), Object::Dict(egs));
            }
        }
        Ok(())
    }
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
    /// Form entries and decoded bytes left for this page, its content streams already charged:
    /// shared forms drawn from other shared forms would otherwise multiply the work with every
    /// level.
    budget: DecodeBudget,
}

impl<'a> Scope<'a> {
    /// A scope whose form walk spends `budget`, what is left of the page's allowance after its
    /// content streams were decoded.
    pub fn new(rects: &'a [[f64; 4]], mode: Mode, report: &'a mut Report, budget: DecodeBudget) -> Self {
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
            budget,
        }
    }

    /// Sanitizing visits every form XObject (not only those under a region).
    fn everywhere(&self) -> bool {
        self.hidden_text.is_some() || self.layers_enabled
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
        font_gs: None,
        fill_pattern: None,
        stroke_pattern: None,
    };
    process_from(doc, scope, streams, resources, gs)
}

/// [`process`] from graphics state `gs`: a form XObject starts with its caller's, text state
/// included (ISO 32000-2 §8.10.1), so text it shows without setting a font of its own is placed
/// with the caller's font, size and spacing, as the renderer places it.
fn process_from(doc: &mut Document, scope: &mut Scope<'_>, streams: &[Vec<u8>], resources: &Dict, gs: Gs) -> Output {
    let mut walk = Walk::new(doc, resources, gs);
    // The streams are one content stream in pieces, which may be split between any two tokens
    // (ISO 32000-2 §7.8.2): an operator's operands can end one piece and the operator start the
    // next. Parse them joined, then give each operator back to the piece its keyword is in.
    let joined = Pieces::join(streams);
    let mut parsed = joined.parse();
    walk.out.ambiguous = parsed.ambiguous_images > 0;
    // A rewritten piece loses its comments, so keep where they began and ended: a comment that
    // ran on into the next piece still hides that piece's first line.
    let comment_edges = joined.comment_edges(&parsed);
    // Layer removal also changes enclosing replacement spans, so remember the operators before
    // any cleanup, not just the survivors of the layer pass.
    let original = parsed.ops.clone();
    let mut forced_change = false;
    if scope.layers_enabled {
        let props = res_dict(doc, resources, b"Properties");
        let (kept, removed) = strip_hidden_layers(doc, scope, parsed.ops, &props);
        parsed.ops = kept;
        if removed > 0 {
            scope.layer_blocks += removed;
            forced_change = true;
            walk.any_paint_removed = true;
        }
    }
    let (pieces, rewrite) = split_into_pieces(&joined, parsed.ops, streams.len(), forced_change);
    let mut processed = Vec::new();
    for (piece, changed) in pieces.into_iter().zip(rewrite) {
        let mut done = Piece { ops: Vec::with_capacity(piece.len()), changed };
        for op in piece {
            walk.step(doc, scope, resources, op, &mut done);
        }
        if !walk.path.is_empty() {
            done.ops.append(&mut walk.path);
        }
        processed.push((done.ops, done.changed));
    }
    if scope.mode == Mode::Apply {
        walk.finish_apply(doc, scope, &mut processed);
        clean_replacements(doc, resources, &original, &mut processed, scope.report);
    }
    walk.out.streams = processed.into_iter().zip(comment_edges).map(|((ops, changed), edges)| changed.then(|| piece_bytes(&ops, edges))).collect();
    walk.out
}

/// Gives each operator to the piece its keyword is in. An operator split across pieces is written
/// whole into its keyword's piece, so every piece it spans is rewritten.
fn split_into_pieces(joined: &Pieces, ops: Vec<Op>, count: usize, forced_change: bool) -> (Vec<Vec<Op>>, Vec<bool>) {
    let mut pieces: Vec<Vec<Op>> = (0..count).map(|_| Vec::new()).collect();
    let mut rewrite: Vec<bool> = vec![forced_change; count];
    for op in ops {
        let (first, last) = joined.pieces_of(&op);
        if first < last {
            for r in rewrite.iter_mut().take(last + 1).skip(first) {
                *r = true;
            }
        }
        if let Some(p) = pieces.get_mut(last) {
            p.push(op);
        }
    }
    (pieces, rewrite)
}

/// A rewritten piece's bytes. `(comment_in, comment_out)` say whether the piece before ended
/// inside a comment, and whether this one did.
fn piece_bytes(ops: &[Op], (comment_in, comment_out): (bool, bool)) -> Vec<u8> {
    // The piece before still ends inside a comment: end it before the first operator.
    let mut bytes = if comment_in { b"\n".to_vec() } else { Vec::new() };
    bytes.extend(serialize_ops(ops));
    // The piece ended inside a comment that hid the start of the next one: keep it open.
    if comment_out {
        bytes.push(b'%');
    }
    bytes
}

/// The operators written for one piece of a scope's content, and whether they differ from the source.
struct Piece {
    ops: Vec<Op>,
    changed: bool,
}

/// The interpreter state carried across the operators of a scope.
struct Walk {
    gs: Gs,
    initial_ctm: Matrix,
    /// With each saved state, the text and text line matrices: inside a text object `q`/`Q`
    /// save and restore them too (ISO 32000-2 §9.4.1 as corrected by the PDF Association's
    /// errata, issue 368), as the renderer does.
    stack: Vec<(Gs, Matrix, Matrix)>,
    tm: Matrix,
    tlm: Matrix,
    fonts_res: Dict,
    ext_gstates: Dict,
    xobjects: Dict,
    patterns: Dict,
    painted_patterns: HashSet<Vec<u8>>,
    redacted_font_gs: HashSet<Vec<u8>>,
    surviving_font_gs: HashSet<Vec<u8>>,
    any_paint_removed: bool,
    fallback: Rc<Metrics>,
    /// The current path: its operators, its bounding box in user space, and whether it clips.
    path: Vec<Op>,
    path_box: Option<[f64; 4]>,
    clip: bool,
    out: Output,
}

impl Walk {
    fn new(doc: &Document, resources: &Dict, gs: Gs) -> Self {
        let initial_ctm = gs.ctm;
        Self {
            gs,
            initial_ctm,
            stack: Vec::new(),
            tm: Matrix::IDENTITY,
            tlm: Matrix::IDENTITY,
            fonts_res: res_dict(doc, resources, b"Font"),
            ext_gstates: res_dict(doc, resources, b"ExtGState"),
            xobjects: res_dict(doc, resources, b"XObject"),
            patterns: res_dict(doc, resources, b"Pattern"),
            painted_patterns: HashSet::new(),
            redacted_font_gs: HashSet::new(),
            surviving_font_gs: HashSet::new(),
            any_paint_removed: false,
            fallback: Rc::new(Metrics::fallback()),
            path: Vec::new(),
            path_box: None,
            clip: false,
            out: Output::default(),
        }
    }

    /// Interprets one operator, writing what replaces it to `piece`.
    fn step(&mut self, doc: &mut Document, scope: &mut Scope<'_>, resources: &Dict, op: Op, piece: &mut Piece) {
        let o = op.op.as_slice();
        if matches!(o, b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re" | b"W" | b"W*") {
            self.construct_path(op);
            return;
        }
        if matches!(o, b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n") {
            self.paint_path(scope, op, piece);
            return;
        }
        if !self.path.is_empty() {
            // A path not ended by a painting operator: pass it through.
            piece.ops.append(&mut self.path);
            self.path_box = None;
            self.clip = false;
        }
        let kept = match o {
            b"q" | b"Q" | b"cm" | b"Tf" | b"gs" => self.graphics_state(doc, scope, resources, op, piece),
            b"g" | b"rg" | b"k" | b"sc" | b"G" | b"RG" | b"K" | b"SC" | b"cs" | b"CS" | b"scn" | b"SCN" => {
                self.color_or_pattern(doc, scope, resources, op, piece)
            }
            b"BT" | b"Tc" | b"Tw" | b"Tz" | b"TL" | b"Ts" | b"Tr" | b"Td" | b"TD" | b"Tm" | b"T*" => {
                self.text_state(&op);
                Some(op)
            }
            b"Tj" | b"TJ" | b"'" | b"\"" => self.show_text(scope, op, piece),
            b"BI" => self.inline_image(scope, op, piece),
            b"sh" if scope.mode == Mode::Apply => Self::shading(&self.gs, scope, op, piece),
            b"Do" => self.draw_xobject(doc, scope, resources, op, piece),
            _ => Some(op),
        };
        piece.ops.extend(kept);
    }

    fn finish_apply(&mut self, doc: &mut Document, scope: &mut Scope<'_>, processed: &mut [(Vec<Op>, bool)]) {
        for gs_name in &self.redacted_font_gs {
            if self.surviving_font_gs.contains(gs_name) {
                continue;
            }
            let Some(gs_val) = self.ext_gstates.get(gs_name).cloned() else { continue };
            let gs_ref = gs_val.as_ref();
            let Some(mut d) = doc.resolve(&gs_val).as_dict().cloned() else { continue };
            if let Some(font_obj) = d.remove(b"Font") {
                if let Some(fr) = font_obj.as_ref() {
                    self.out.doomed.push(fr);
                }
                if let Some(fr) = doc.resolve(&font_obj).as_array().and_then(|a| a.first()).and_then(Object::as_ref)
                    && !self.fonts_res.iter().any(|(_, v)| v.as_ref() == Some(fr))
                {
                    self.out.doomed.push(fr);
                }
            }
            if let Some(gr) = gs_ref {
                self.out.doomed.push(gr);
            }
            self.out.replaced_ext_gstates.push(gs_name.clone());
            let only_type = d.iter().all(|(k, _)| k.as_slice() == b"Type");
            if only_type {
                for (ops, changed) in &mut *processed {
                    let before = ops.len();
                    ops.retain(|op| !(op.is("gs") && op.name(0) == Some(gs_name.as_slice())));
                    if ops.len() != before {
                        *changed = true;
                    }
                }
            } else {
                let nr = doc.add(Object::Dict(d));
                let n = fresh_name(scope, &self.ext_gstates);
                self.ext_gstates.set(n.clone(), Object::Ref(nr));
                self.out.ext_gstates.push((n.clone(), nr));
                for (ops, changed) in &mut *processed {
                    for op in ops {
                        if op.is("gs") && op.name(0) == Some(gs_name.as_slice()) {
                            *op = Op::new("gs", vec![Object::Name(n.clone())]);
                            *changed = true;
                        }
                    }
                }
            }
        }
        if self.any_paint_removed {
            for (ops, changed) in processed {
                let mut i = 0;
                while i < ops.len() {
                    let drop_pat = if matches!(ops[i].op.as_slice(), b"scn" | b"SCN") {
                        ops[i].operands.last().and_then(Object::as_name).filter(|n| !self.painted_patterns.contains(*n)).map(<[u8]>::to_vec)
                    } else {
                        None
                    };
                    if let Some(pname) = drop_pat {
                        if let Some(pr) = self.patterns.get(&pname).and_then(Object::as_ref) {
                            self.out.doomed.push(pr);
                        }
                        if !self.out.replaced_patterns.contains(&pname) {
                            self.out.replaced_patterns.push(pname);
                        }
                        ops.remove(i);
                        *changed = true;
                    } else {
                        i += 1;
                    }
                }
            }
        }
    }

    /// Path construction: collects the operator and grows the path's bounding box.
    fn construct_path(&mut self, op: Op) {
        let o = op.op.as_slice();
        let pts: Vec<(f64, f64)> = match o {
            b"re" => op.nums::<4>().map(|[x, y, w, h]| vec![(x, y), (x + w, y), (x, y + h), (x + w, y + h)]).unwrap_or_default(),
            b"h" | b"W" | b"W*" => Vec::new(),
            _ => op.operands.iter().filter_map(Object::as_f64).collect::<Vec<_>>().as_chunks::<2>().0.iter().map(|p| (p[0], p[1])).collect(),
        };
        for (x, y) in pts {
            let (u, v) = self.gs.ctm.apply(x, y);
            self.path_box = Some(match self.path_box {
                None => [u, v, u, v],
                Some(b) => [b[0].min(u), b[1].min(v), b[2].max(u), b[3].max(v)],
            });
        }
        self.clip |= matches!(o, b"W" | b"W*");
        self.path.push(op);
    }

    /// A painting operator ends the path; one under a region is removed (or kept as a clip).
    fn paint_path(&mut self, scope: &mut Scope<'_>, op: Op, piece: &mut Piece) {
        let o = op.op.as_slice();
        let body = std::mem::take(&mut self.path);
        let bbox = self.path_box.take();
        let clips = std::mem::replace(&mut self.clip, false);
        let hit = scope.mode == Mode::Apply && o != b"n" && bbox.is_some_and(|b| scope.hits(b));
        if !hit {
            if matches!(o, b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*")
                && let Some(p) = &self.gs.fill_pattern
            {
                self.painted_patterns.insert(p.clone());
            }
            if matches!(o, b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*")
                && let Some(p) = &self.gs.stroke_pattern
            {
                self.painted_patterns.insert(p.clone());
            }
            piece.ops.extend(body);
            piece.ops.push(op);
            return;
        }
        let b = bbox.unwrap_or_default();
        piece.changed = true;
        scope.report.paths_removed += 1;
        self.any_paint_removed = true;
        if scope.covered(b) && clips {
            // Keep the clipping, drop the painting.
            piece.ops.extend(body);
            piece.ops.push(Op::new("n", vec![]));
        }
        // Partial vector redaction must not retain underlying drawing commands
        // behind clipping (F-008). Refuse partial clip retention and remove the path.
    }

    fn graphics_state(&mut self, doc: &mut Document, scope: &mut Scope<'_>, resources: &Dict, op: Op, piece: &mut Piece) -> Option<Op> {
        match op.op.as_slice() {
            b"q" => self.stack.push((self.gs.clone(), self.tm, self.tlm)),
            b"Q" => {
                if let Some((g, m, lm)) = self.stack.pop() {
                    (self.gs, self.tm, self.tlm) = (g, m, lm);
                }
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    self.gs.ctm = Matrix(m).then(&self.gs.ctm);
                }
            }
            b"Tf" => self.select_font(doc, scope, &op),
            _ => {
                if let Some(new) = extgstate_op(doc, scope, &op, (&mut self.gs, &self.fallback), resources, &mut self.ext_gstates, &mut self.out) {
                    piece.changed = true;
                    return new;
                }
            }
        }
        Some(op)
    }

    fn color_or_pattern(&mut self, doc: &mut Document, scope: &mut Scope<'_>, resources: &Dict, op: Op, piece: &mut Piece) -> Option<Op> {
        let o = op.op.as_slice();
        match o {
            b"g" | b"rg" | b"k" | b"sc" => self.gs.fill_pattern = None,
            b"G" | b"RG" | b"K" | b"SC" => self.gs.stroke_pattern = None,
            b"cs" => {
                if op.name(0) != Some(b"Pattern") {
                    self.gs.fill_pattern = None;
                }
            }
            b"CS" => {
                if op.name(0) != Some(b"Pattern") {
                    self.gs.stroke_pattern = None;
                }
            }
            b"scn" | b"SCN" => {
                let is_fill = o == b"scn";
                if let Some(pat_name) = op.operands.last().and_then(Object::as_name).map(<[u8]>::to_vec) {
                    if is_fill {
                        self.gs.fill_pattern = Some(pat_name);
                    } else {
                        self.gs.stroke_pattern = Some(pat_name);
                    }
                    if let Some(new) = pattern_op(doc, scope, &op, (&self.gs, &self.initial_ctm), resources, &mut self.patterns, &mut self.out) {
                        piece.changed = true;
                        let updated = new.as_ref().and_then(|n| n.operands.last()).and_then(Object::as_name).map(<[u8]>::to_vec);
                        if is_fill {
                            self.gs.fill_pattern = updated;
                        } else {
                            self.gs.stroke_pattern = updated;
                        }
                        return new;
                    }
                } else if is_fill {
                    self.gs.fill_pattern = None;
                } else {
                    self.gs.stroke_pattern = None;
                }
            }
            _ => {}
        }
        Some(op)
    }

    fn select_font(&mut self, doc: &Document, scope: &mut Scope<'_>, op: &Op) {
        self.gs.font_gs = None;
        self.gs.size = op.num(1).unwrap_or(self.gs.size);
        let Some(name) = op.name(0) else { return };
        let font_dict = self.fonts_res.get(name).and_then(|f| doc.resolve(f).as_dict().cloned());
        self.gs.vertical = font_dict.as_ref().is_some_and(|d| is_vertical(doc, d));
        self.gs.font = match scope.fonts.get(name) {
            Some(f) => f.clone(),
            None => {
                let m = font_dict.as_ref().map_or_else(|| self.fallback.clone(), |d| Rc::new(Metrics::from_dict(doc, d)));
                scope.fonts.insert(name.to_vec(), m.clone());
                m
            }
        };
    }

    /// Text state and text-positioning operators.
    fn text_state(&mut self, op: &Op) {
        let gs = &mut self.gs;
        match op.op.as_slice() {
            b"BT" => {
                self.tm = Matrix::IDENTITY;
                self.tlm = Matrix::IDENTITY;
            }
            b"Tc" => gs.char_spacing = op.num(0).unwrap_or(0.0),
            b"Tw" => gs.word_spacing = op.num(0).unwrap_or(0.0),
            b"Tz" => gs.scale = op.num(0).unwrap_or(100.0) / 100.0,
            b"TL" => gs.leading = op.num(0).unwrap_or(0.0),
            b"Ts" => gs.rise = op.num(0).unwrap_or(0.0),
            b"Tr" => gs.render_mode = op.num(0).unwrap_or(0.0) as i64,
            b"Td" | b"TD" => {
                if let Some([x, y]) = op.nums::<2>() {
                    if op.is("TD") {
                        gs.leading = -y;
                    }
                    self.tlm = Matrix::translate(x, y).then(&self.tlm);
                    self.tm = self.tlm;
                }
            }
            b"Tm" => {
                if let Some(m) = op.nums::<6>() {
                    self.tlm = Matrix(m);
                    self.tm = self.tlm;
                }
            }
            _ => {
                self.tlm = Matrix::translate(0.0, -gs.leading).then(&self.tlm);
                self.tm = self.tlm;
            }
        }
    }

    /// `Tj`, `TJ`, `'` and `"`: glyphs under a region are cut out (or counted, when verifying).
    fn show_text(&mut self, scope: &mut Scope<'_>, op: Op, piece: &mut Piece) -> Option<Op> {
        let o = op.op.as_slice();
        if o == b"\"" {
            self.gs.word_spacing = op.num(0).unwrap_or(self.gs.word_spacing);
            self.gs.char_spacing = op.num(1).unwrap_or(self.gs.char_spacing);
        }
        if matches!(o, b"'" | b"\"") {
            self.tlm = Matrix::translate(0.0, -self.gs.leading).then(&self.tlm);
            self.tm = self.tlm;
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
        if self.gs.vertical
            && scope.mode == Mode::Apply
            && scope.hidden_text.is_none()
            && !scope.rects.is_empty()
            && items.iter().any(|o| matches!(o, Object::String(s) if !s.bytes.is_empty()))
        {
            self.out.vertical = true;
        }
        let (new_items, removed) = show(scope, &self.gs, &mut self.tm, &items);
        let has_surviving = new_items.iter().any(|x| x.as_string().is_some_and(|s| !s.bytes.is_empty()));
        if removed == 0 || has_surviving {
            if let Some(fg) = &self.gs.font_gs {
                self.surviving_font_gs.insert(fg.clone());
            }
            if let Some(p) = &self.gs.fill_pattern {
                self.painted_patterns.insert(p.clone());
            }
            if let Some(p) = &self.gs.stroke_pattern {
                self.painted_patterns.insert(p.clone());
            }
        }
        if removed == 0 {
            return Some(op);
        }
        if let Some(fg) = &self.gs.font_gs {
            self.redacted_font_gs.insert(fg.clone());
        }
        self.any_paint_removed = true;
        if scope.mode == Mode::Verify {
            self.out.residue += removed;
            return Some(op);
        }
        piece.changed = true;
        scope.report.glyphs += removed;
        match op.op.as_slice() {
            b"\"" => {
                piece.ops.push(Op::new("Tw", vec![num(self.gs.word_spacing)]));
                piece.ops.push(Op::new("Tc", vec![num(self.gs.char_spacing)]));
                piece.ops.push(Op::new("T*", vec![]));
            }
            b"'" => piece.ops.push(Op::new("T*", vec![])),
            _ => {}
        }
        piece.ops.push(Op::new("TJ", vec![Object::Array(new_items)]));
        None
    }

    /// An inline image under a region is removed (or counted, when verifying).
    fn inline_image(&mut self, scope: &mut Scope<'_>, op: Op, piece: &mut Piece) -> Option<Op> {
        if !scope.hits(self.gs.ctm.bbox([0.0, 0.0, 1.0, 1.0])) {
            self.painted_patterns.extend(self.gs.fill_pattern.iter().cloned());
            return Some(op);
        }
        if scope.mode == Mode::Verify {
            self.out.residue += 1;
            self.painted_patterns.extend(self.gs.fill_pattern.iter().cloned());
            return Some(op);
        }
        scope.report.images_removed += 1;
        piece.changed = true;
        None
    }

    /// A shading fills the current clip: the regions are clipped out.
    fn shading(gs: &Gs, scope: &Scope<'_>, op: Op, piece: &mut Piece) -> Option<Op> {
        let Some(c) = clip_out(scope.rects, &gs.ctm, [-1e6, -1e6, 1e6, 1e6]) else { return Some(op) };
        piece.changed = true;
        piece.ops.push(Op::new("q", vec![]));
        piece.ops.extend(c);
        piece.ops.push(op);
        piece.ops.push(Op::new("Q", vec![]));
        None
    }

    fn draw_xobject(&mut self, doc: &mut Document, scope: &mut Scope<'_>, resources: &Dict, op: Op, piece: &mut Piece) -> Option<Op> {
        let replacement = xobject(doc, scope, &op, &self.gs, resources, &mut self.xobjects, &mut self.out);
        if !matches!(replacement, Some(None)) {
            self.painted_patterns.extend(self.gs.fill_pattern.iter().cloned());
            self.painted_patterns.extend(self.gs.stroke_pattern.iter().cloned());
        }
        let Some(new) = replacement else { return Some(op) };
        piece.changed = true;
        piece.ops.extend(new);
        None
    }
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

enum StreamOutcome {
    Unchanged,
    Removed,
    Rewritten(ObjRef),
}

fn sanitize_stream_form(
    doc: &mut Document,
    scope: &mut Scope<'_>,
    (r, s): (ObjRef, &Stream),
    gs: Gs,
    override_rects: Option<&[[f64; 4]]>,
    resources: &Dict,
    out: &mut Output,
) -> StreamOutcome {
    if s.dict.get(b"OC").is_some_and(|oc| scope.xobject_layer_hidden(doc, r, oc)) {
        scope.layer_blocks += 1;
        if scope.mode == Mode::Verify {
            return StreamOutcome::Unchanged;
        }
        out.doomed.push(r);
        return StreamOutcome::Removed;
    }
    let bbox = s.dict.get(b"BBox").and_then(|b| {
        let v: Vec<f64> = doc.resolve(b).as_array()?.iter().filter_map(Object::as_f64).collect();
        (v.len() == 4).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
    });
    let active_rects = override_rects.unwrap_or(scope.rects);
    if let Some(bb) = bbox
        && !scope.everywhere()
        && !active_rects.iter().any(|rect| overlaps(*rect, gs.ctm.bbox(bb), 0.0))
    {
        return StreamOutcome::Unchanged;
    }
    if scope.depth >= MAX_DEPTH {
        if scope.mode == Mode::Verify {
            if scope.everywhere() {
                scope.layer_blocks += 1;
            }
            return StreamOutcome::Unchanged;
        }
        scope.report.forms_removed += 1;
        out.doomed.push(r);
        return StreamOutcome::Removed;
    }
    // Strict, like page content: a Form read only in part counts as undecodable.
    let data = if scope.budget.enter_form() {
        scope.budget.decode_strict(s)
    } else {
        Err(CosError::Filter(format!("the page draws more than {MAX_PAGE_FORMS} form XObjects")))
    };
    let data = match data {
        Ok(d) => d,
        // Past the page's budget nothing more is read: apply refuses the page, and
        // verification counts the form as content that may remain.
        Err(e) if scope.budget.exhausted() => {
            if scope.mode == Mode::Apply {
                out.error = Some(e);
            } else if scope.everywhere() {
                scope.layer_blocks += 1;
            } else {
                out.residue += 1;
            }
            return StreamOutcome::Unchanged;
        }
        Err(_) if scope.mode == Mode::Apply => {
            scope.report.forms_removed += 1;
            out.doomed.push(r);
            return StreamOutcome::Removed;
        }
        Err(_) => {
            if scope.everywhere() {
                scope.layer_blocks += 1;
            }
            return StreamOutcome::Unchanged;
        }
    };
    let own = s.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned());
    let res = own.clone().unwrap_or_else(|| resources.clone());
    let inner = {
        let mut sub_scope = Scope {
            rects: active_rects,
            mode: scope.mode,
            report: scope.report,
            hidden_text: scope.hidden_text,
            hidden_layers: std::mem::take(&mut scope.hidden_layers),
            layers_enabled: scope.layers_enabled,
            retained_off_layers: std::mem::take(&mut scope.retained_off_layers),
            ocmd_cache: std::mem::take(&mut scope.ocmd_cache),
            xobject_oc_cache: std::mem::take(&mut scope.xobject_oc_cache),
            layer_blocks: scope.layer_blocks,
            fonts: HashMap::new(),
            used_names: std::mem::take(&mut scope.used_names),
            depth: scope.depth + 1,
            budget: std::mem::replace(&mut scope.budget, DecodeBudget::page()),
        };
        let res_inner = process_from(doc, &mut sub_scope, std::slice::from_ref(&data), &res, gs);
        scope.hidden_layers = sub_scope.hidden_layers;
        scope.retained_off_layers = sub_scope.retained_off_layers;
        scope.ocmd_cache = sub_scope.ocmd_cache;
        scope.xobject_oc_cache = sub_scope.xobject_oc_cache;
        scope.layer_blocks = sub_scope.layer_blocks;
        scope.used_names = sub_scope.used_names;
        scope.budget = sub_scope.budget;
        res_inner
    };
    out.residue += inner.residue;
    out.vertical |= inner.vertical;
    out.ambiguous |= inner.ambiguous;
    out.doomed.extend_from_slice(&inner.doomed);
    out.retired_properties.extend(inner.retired_properties.clone());
    out.tag_changes.extend(inner.tag_changes.clone());
    out.replacements.extend_from_slice(&inner.replacements);
    out.error = inner.error.clone().or(out.error.take());
    if scope.mode == Mode::Verify {
        return StreamOutcome::Unchanged;
    }
    let new_data = inner.streams.first().cloned().flatten();
    if new_data.is_none() && !inner.has_resource_changes() {
        return StreamOutcome::Unchanged;
    }
    let mut dict = s.dict.clone();
    dict.remove(b"Length");
    let mut cleaned_res = res.clone();
    let effective_data = new_data.as_deref().unwrap_or(&data);
    let content = [effective_data.to_vec()];
    if let Err(error) = inner.apply_resources(doc, &mut cleaned_res, &content) {
        out.error = Some(error);
        return StreamOutcome::Unchanged;
    }
    if new_data.is_some() {
        match prune_properties(doc, &mut cleaned_res, &content) {
            Ok(retired) => out.retired_properties.extend(retired),
            Err(error) => {
                out.error = Some(error);
                return StreamOutcome::Unchanged;
            }
        }
        if let Err(error) = prune_form_resources(doc, &mut cleaned_res, &content) {
            out.error = Some(error);
            return StreamOutcome::Unchanged;
        }
        if let Err(error) = crate::type3::prune_type3_from_resources(doc, &mut cleaned_res, &content) {
            out.error = Some(error);
            return StreamOutcome::Unchanged;
        }
    }
    if cleaned_res != res {
        dict.set(b"Resources".to_vec(), Object::Dict(cleaned_res));
    }
    let (changed, empty) = tags::touched(doc, &res, &data, effective_data);
    let stream = match new_data {
        Some(d) => Stream::flate(dict, &d),
        None => Stream { dict, raw: s.raw.clone() },
    };
    let nr = doc.add(Object::Stream(stream));
    out.tag_changes.push((r, nr, changed, empty));
    out.replacements.push(nr);
    out.doomed.push(r);
    scope.report.forms_rewritten += 1;
    StreamOutcome::Rewritten(nr)
}

fn extgstate_op(
    doc: &mut Document,
    scope: &mut Scope<'_>,
    op: &Op,
    (gs, fallback): (&mut Gs, &Rc<Metrics>),
    resources: &Dict,
    ext_gstates: &mut Dict,
    out: &mut Output,
) -> Option<Option<Op>> {
    let name = op.name(0)?.to_vec();
    let gs_val = ext_gstates.get(&name)?.clone();
    let gs_ref = gs_val.as_ref();
    let d = doc.resolve(&gs_val).as_dict()?.clone();
    if let Some((font, vertical, size)) = extgstate_font(doc, scope, ext_gstates, &name, fallback) {
        (gs.font, gs.vertical, gs.size) = (font, vertical, size);
        gs.font_gs = Some(name.clone());
    }
    let smask_obj = d.get(b"SMask")?;
    let smask_ref = smask_obj.as_ref();
    let smask_dict = doc.resolve(smask_obj).as_dict()?.clone();
    let g_ref = smask_dict.get(b"G").and_then(Object::as_ref)?;
    let g_obj = doc.get(g_ref);
    let Object::Stream(s) = &*g_obj else { return None };
    let m = s.dict.get(b"Matrix").and_then(|m| Matrix::from_operands(doc.resolve(m).as_array()?)).unwrap_or_default();
    let fctm = m.then(&gs.ctm);
    match sanitize_stream_form(doc, scope, (g_ref, s), Gs { ctm: fctm, ..gs.clone() }, None, resources, out) {
        StreamOutcome::Unchanged => None,
        StreamOutcome::Removed => {
            if let Some(sr) = smask_ref {
                out.doomed.push(sr);
            }
            if let Some(gr) = gs_ref {
                out.doomed.push(gr);
            }
            out.replaced_ext_gstates.push(name.clone());
            let mut new_d = d;
            new_d.remove(b"SMask");
            let only_type = new_d.iter().all(|(k, _)| k.as_slice() == b"Type");
            if only_type {
                if gs.font_gs.as_deref() == Some(name.as_slice()) {
                    gs.font_gs = None;
                }
                Some(None)
            } else {
                let nr = doc.add(Object::Dict(new_d));
                let n = fresh_name(scope, ext_gstates);
                ext_gstates.set(n.clone(), Object::Ref(nr));
                out.ext_gstates.push((n.clone(), nr));
                if gs.font_gs.as_deref() == Some(name.as_slice()) {
                    gs.font_gs = Some(n.clone());
                }
                Some(Some(Op::new("gs", vec![Object::Name(n)])))
            }
        }
        StreamOutcome::Rewritten(nr) => {
            if let Some(sr) = smask_ref {
                out.doomed.push(sr);
            }
            if let Some(gr) = gs_ref {
                out.doomed.push(gr);
            }
            out.replaced_ext_gstates.push(name.clone());
            let mut new_smask = smask_dict;
            new_smask.set(b"G".to_vec(), Object::Ref(nr));
            let mut new_d = d;
            new_d.set(b"SMask".to_vec(), Object::Dict(new_smask));
            let new_gs_ref = doc.add(Object::Dict(new_d));
            let n = fresh_name(scope, ext_gstates);
            ext_gstates.set(n.clone(), Object::Ref(new_gs_ref));
            out.ext_gstates.push((n.clone(), new_gs_ref));
            if gs.font_gs.as_deref() == Some(name.as_slice()) {
                gs.font_gs = Some(n.clone());
            }
            Some(Some(Op::new("gs", vec![Object::Name(n)])))
        }
    }
}

fn pattern_rects(doc: &Document, dict: &Dict, pctm: &Matrix, gs_ctm: &Matrix, m: &Matrix, rects: &[[f64; 4]]) -> Vec<[f64; 4]> {
    let mut out = rects.to_vec();
    if gs_ctm.0 != pctm.0
        && let Some(inv_gs) = m.then(gs_ctm).invert()
    {
        for r in rects {
            let mapped = pctm.bbox(inv_gs.bbox(*r));
            if !out.contains(&mapped) {
                out.push(mapped);
            }
        }
    }
    let bbox = dict.get(b"BBox").and_then(|b| {
        let v: Vec<f64> = doc.resolve(b).as_array()?.iter().filter_map(Object::as_f64).collect();
        (v.len() == 4).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
    });
    let xstep = dict.get(b"XStep").and_then(|x| doc.resolve(x).as_f64()).unwrap_or(0.0).abs();
    let ystep = dict.get(b"YStep").and_then(|y| doc.resolve(y).as_f64()).unwrap_or(0.0).abs();
    if let Some(bb) = bbox
        && xstep > 1e-3
        && ystep > 1e-3
        && let Some(inv) = pctm.invert()
    {
        for r in rects {
            let pr = inv.bbox(*r);
            let kx0 = ((pr[0] - bb[2]) / xstep).floor() as i64;
            let kx1 = ((pr[2] - bb[0]) / xstep).ceil() as i64;
            let ky0 = ((pr[1] - bb[3]) / ystep).floor() as i64;
            let ky1 = ((pr[3] - bb[1]) / ystep).ceil() as i64;
            if kx1.saturating_sub(kx0) > 16 || ky1.saturating_sub(ky0) > 16 {
                // Bounded enumeration must fail closed, not miss distant copies of the tile.
                let whole = pctm.bbox(bb);
                if !out.contains(&whole) {
                    out.push(whole);
                }
                continue;
            }
            for kx in kx0..=kx1 {
                for ky in ky0..=ky1 {
                    if kx == 0 && ky == 0 {
                        continue;
                    }
                    let shifted =
                        [pr[0] - (kx as f64) * xstep, pr[1] - (ky as f64) * ystep, pr[2] - (kx as f64) * xstep, pr[3] - (ky as f64) * ystep];
                    if overlaps(shifted, bb, 0.0) {
                        out.push(pctm.bbox(shifted));
                    }
                }
            }
        }
    } else if let Some(bb) = bbox
        && !(xstep > 1e-3 && ystep > 1e-3)
        && !rects.is_empty()
    {
        // Tiny or invalid steps cannot be enumerated safely.
        let whole = pctm.bbox(bb);
        if !out.contains(&whole) {
            out.push(whole);
        }
    }
    out
}

fn pattern_op(
    doc: &mut Document,
    scope: &mut Scope<'_>,
    op: &Op,
    (gs, initial_ctm): (&Gs, &Matrix),
    resources: &Dict,
    patterns: &mut Dict,
    out: &mut Output,
) -> Option<Option<Op>> {
    let name = op.operands.last()?.as_name()?.to_vec();
    let r = patterns.get(&name)?.as_ref()?;
    let obj = doc.get(r);
    let Object::Stream(s) = &*obj else { return None };
    let m = s.dict.get(b"Matrix").and_then(|m| Matrix::from_operands(doc.resolve(m).as_array()?)).unwrap_or_default();
    let pctm = m.then(initial_ctm);
    let rects = pattern_rects(doc, &s.dict, &pctm, &gs.ctm, &m, scope.rects);
    match sanitize_stream_form(doc, scope, (r, s), Gs { ctm: pctm, ..gs.clone() }, Some(&rects), resources, out) {
        StreamOutcome::Unchanged => None,
        StreamOutcome::Removed => {
            out.replaced_patterns.push(name);
            Some(None)
        }
        StreamOutcome::Rewritten(nr) => {
            let n = fresh_name(scope, patterns);
            patterns.set(n.clone(), Object::Ref(nr));
            out.patterns.push((n.clone(), nr));
            out.replaced_patterns.push(name);
            let mut operands = op.operands.clone();
            if let Some(last) = operands.last_mut() {
                *last = Object::Name(n);
            }
            let op_name = if op.op == b"SCN" { "SCN" } else { "scn" };
            Some(Some(Op::new(op_name, operands)))
        }
    }
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
    let name = op.last_name()?.to_vec();
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
                    out.replacements.push(nr);
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
            match sanitize_stream_form(doc, scope, (r, s), Gs { ctm: fctm, ..gs.clone() }, None, resources, out) {
                StreamOutcome::Unchanged => None,
                StreamOutcome::Removed => {
                    out.replaced.push(name);
                    Some(None)
                }
                StreamOutcome::Rewritten(nr) => {
                    let n = fresh_name(scope, xobjects);
                    xobjects.set(n.clone(), Object::Ref(nr));
                    out.xobjects.push((n.clone(), nr));
                    out.replaced.push(name);
                    Some(Some(Op::new("Do", vec![Object::Name(n)])))
                }
            }
        }
        _ => None,
    }
}
