//! Existing-text preparation: draw a Form XObject's content in place of its `Do` (#2964, #3005).
//! Adapted from mtlcl410's Ungroup implementation in PR #3067, revision
//! `6eafca9caa4a4d2174a395b53914bc9e689097cc` (MIT OR Apache-2.0).
//!
//! Text inside grouped Form artwork can't be edited, because editing rewrites only the page's own
//! content streams. Ungrouping copies the form's content into the page at the same place,
//! wrapped as `q <Matrix> cm <BBox> re W n … Q`, with the form's resources merged into a copy of
//! the page's under names that don't clash. Its text then edits like any other. The form object
//! itself is left alone, so another page or placement that draws it is unchanged.
//!
//! Drawing a form inline paints the same pixels unless the form is a transparency group whose
//! compositing differs from painting its parts one by one: a knockout group, a group that uses
//! a blend mode other than Normal inside, or a group drawn translucent, blended or under a soft
//! mask, or a group with a blending colour space (`/CS`) the page group doesn't name too: its
//! contents composite in that space and are then converted (ISO 32000-2 §11.6.6), inline they
//! would composite in the page's, which a page without one leaves to the PDF processor. Those
//! are refused rather than changed, as is tagged form content (its structure link would be lost)
//! and content whose marked content or text objects don't balance (inlined, they would change
//! the scope of the page's own). A form in a layer (`/OC`) keeps its membership as
//! `/OC … BDC … EMC`.
//!
//! A pattern's matrix maps pattern space to the default space of the content stream that uses it
//! (ISO 32000-2 §8.7.2): inside the form that is the form's space at the `Do`, on the page it is
//! the page's. So each pattern the form selects with `scn`/`SCN` is drawn from a copy whose
//! `/Matrix` also carries the form's placement; one that can't be copied that way is refused.

use std::collections::HashMap;

use pdfcraft_content::{Matrix, Op, parse};
use pdfcraft_cos::{DecodeBudget, Dict, Document, Object};

use crate::EditError;

/// The largest form content ungrouped (decoded bytes): the page's own content cap is far above
/// any real form, and a bomb must not be copied into the page.
const MAX_FORM_BYTES: usize = 16 << 20;

/// Resource categories a content stream refers to by name (ISO 32000-2 §7.8.3, Table 34).
const CATEGORIES: [&[u8]; 7] = [b"Font", b"XObject", b"ExtGState", b"ColorSpace", b"Pattern", b"Shading", b"Properties"];

/// The most distinct patterns one ungrouped form may select; each becomes a new object.
const MAX_MOVED_PATTERNS: usize = 256;

/// A name map for one resource category: form name → name in the page's resources.
type Renames = Vec<(Vec<u8>, Vec<u8>)>;

/// What [`ungroup_ops`] gives back.
pub(crate) struct Ungrouped {
    /// The ops that draw the form inline.
    pub(crate) ops: Vec<Op>,
    /// The page resources with the form's merged in.
    pub(crate) resources: Dict,
    /// The pattern copies to add to the document, each under the name in the merged `/Pattern`
    /// dictionary that the ops already use.
    pub(crate) patterns: Vec<(Vec<u8>, Object)>,
    /// Whether the form shows text, in its own content or in a Form XObject it draws (at any
    /// depth [`MAX_NESTING`] allows). Text inside a tiling pattern or Type 3 glyph doesn't count:
    /// inlining the form doesn't make it editable. Image-only and vector-only forms are artwork.
    pub(crate) shows_text: bool,
}

/// Why [`ungroup_ops`] left a placement grouped.
pub(crate) struct Refusal {
    pub(crate) error: EditError,
    /// The form itself is the reason (malformed, tagged, a knockout group, and so on), so every
    /// placement of it is refused the same way. Otherwise something about this placement or the
    /// page so far is: its translucency or matrix, the resources it would merge into (which grow
    /// as other groups are prepared), or the decode budget left.
    pub(crate) intrinsic: bool,
    /// The form's resources need renaming where it is drawn, and it draws something that would
    /// look them up by their old names. Names merged in for other forms can make the renaming
    /// unnecessary, which [`merges_without_renaming`] checks without reading the content.
    pub(crate) renaming: bool,
}

/// Ungroup form `name` from the page's `resources`. `translucent` says whether the graphics state
/// at the `Do` is translucent, blended or soft-masked, which a transparency group composites as a
/// whole. `page_space` is the page group's blending colour space ([`page_blend_space`]).
/// `placement` maps the form's space to the page's default space at the `Do` (its `/Matrix`,
/// then the CTM there).
pub(crate) fn ungroup_ops(
    doc: &Document,
    resources: &Dict,
    name: &[u8],
    translucent: bool,
    page_space: Option<&Object>,
    placement: Matrix,
    budget: &mut DecodeBudget,
) -> Result<Ungrouped, Refusal> {
    let refusal = |why: &str, intrinsic: bool| Refusal {
        error: EditError::Invalid(format!("this artwork can't be ungrouped: {why}")),
        intrinsic,
        renaming: false,
    };
    // Something about this placement or the page so far is the reason.
    let refuse = |why: &str| refusal(why, false);
    // The form itself is the reason: every placement of it is refused the same way.
    let invalid = |why: &str| refusal(why, true);
    let xobjects = resources.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).ok_or_else(|| refuse("it is missing"))?;
    let form = xobjects.get(name).ok_or_else(|| refuse("it is missing"))?;
    let Object::Stream(s) = &*doc.resolve(form) else { return Err(refuse("it is not a Form XObject")) };
    if !is_name(doc, &s.dict, b"Subtype", b"Form") {
        return Err(refuse("it is not a Form XObject"));
    }
    let numbers = |key: &[u8], n: usize| -> Option<Vec<f64>> {
        let a = s.dict.get(key).map(|o| doc.resolve(o))?;
        if a.as_array()?.len() != n {
            return None;
        }
        let v: Vec<f64> = a.as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
        (v.len() == n && v.iter().all(|x| x.is_finite())).then_some(v)
    };
    let bbox = numbers(b"BBox", 4).ok_or_else(|| invalid("its bounding box is missing or unreadable"))?;
    let matrix = match s.dict.get(b"Matrix") {
        None => None,
        Some(_) => Some(Matrix(
            <[f64; 6]>::try_from(numbers(b"Matrix", 6).ok_or_else(|| invalid("its matrix is unreadable"))?)
                .map_err(|_| invalid("its matrix is unreadable"))?,
        )),
    };
    let own = s.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned());
    if s.dict.get(b"Resources").is_some() && own.is_none() {
        return Err(invalid("its resources cannot be read"));
    }
    // A form without its own resources uses the ones it is drawn with (PDF 1.1).
    let form_res = own.clone().unwrap_or_else(|| resources.clone());

    // A tagged form's content is a structure element's: drawn inline it would lose that link.
    if s.dict.get(b"StructParent").is_some() || s.dict.get(b"StructParents").is_some() {
        return Err(invalid("it is tagged content (Tags panel); edit its text in a copy without tags"));
    }
    let group = s.dict.get(b"Group").map(|g| doc.resolve(g)).and_then(|g| g.as_dict().cloned()).filter(|g| is_name(doc, g, b"S", b"Transparency"));
    if s.dict.get(b"Group").is_some() && group.is_none() {
        return Err(invalid("its group dictionary cannot be read"));
    }
    if let Some(group) = &group {
        if group.get(b"K").is_some_and(|k| matches!(*doc.resolve(k), Object::Bool(true))) {
            return Err(invalid("it is a knockout transparency group"));
        }
        if translucent {
            return Err(refuse("it is a transparency group drawn translucent, blended or under a soft mask"));
        }
        // Its contents composite in its own colour space (ISO 32000-2 §11.6.6, Table 145 /CS);
        // inline they would composite in the page's.
        if let Some(cs) = group.get(b"CS")
            && !same_blend_space(doc, cs, page_space)
        {
            return Err(invalid("it is a transparency group that blends in a colour space of its own"));
        }
    }

    // Strictly: damaged data must not be copied in as a shorter form.
    let data = budget.decode_strict(s).map_err(|e| refuse(&format!("its content can't be read ({e})")))?;
    let parsed = parse(&data);
    if parsed.skipped > 0 {
        return Err(invalid("its content has parts that can't be read"));
    }
    let mut ops = parsed.ops;
    // A name is renamed at a fixed operand position below, but the renderer takes operands from
    // the end and ignores surplus ones before them: with a surplus operand the drawn name would
    // keep its old spelling while its resource moved, and draw nothing or the wrong thing. The
    // parser keeps such operands without counting them as skipped. Checked before the nested
    // walk, which can only call it unverifiable.
    if ops.iter().any(|op| takes_operands(&op.op).is_some_and(|n| op.operands.len() > n)) {
        return Err(invalid("its content gives an operator more operands than it takes"));
    }
    let shows_text = {
        // Check every form, including non-group forms, for recursive/unsupported content.
        // The form itself is being read: drawing itself again counts as blending.
        let mut walk = BlendWalk { path: form.as_ref().into_iter().collect(), known: HashMap::new(), reads: 0, budget, text: false };
        if blends_inside(doc, &form_res, &ops, 0, &mut walk) {
            return Err(refuse("it has blend modes, recursive forms, or nested content that cannot be verified"));
        }
        walk.text
    };

    // Merge the form's resources into a copy of the page's, renaming what clashes.
    let mut merged = resources.clone();
    let mut renames: Vec<(&[u8], Renames)> = Vec::new();
    if own.is_some() {
        for cat in CATEGORIES {
            let Some(theirs) = form_res.get(cat).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()) else { continue };
            if theirs.len() > 4096 {
                return Err(invalid("too many resources"));
            }
            let mut mine = merged.get(cat).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
            if mine.len() > 4096 {
                return Err(refuse("too many resources"));
            }
            let mut map = Renames::new();
            let mut n = 0u32;
            for (key, value) in theirs.iter() {
                if cat == b"ColorSpace" && matches!(key.as_slice(), b"DefaultGray" | b"DefaultRGB" | b"DefaultCMYK") && mine.get(key) != Some(value) {
                    return Err(refuse("it overrides a default colour space"));
                }
                if mine.get(key) == Some(value) {
                    continue;
                }
                let fresh = loop {
                    n = n.saturating_add(1);
                    let candidate = format!("PCUg{n}").into_bytes();
                    if mine.get(&candidate).is_none() && theirs.get(&candidate).is_none() {
                        break candidate;
                    }
                    if n == u32::MAX {
                        return Err(refuse("too many resources"));
                    }
                };
                mine.set(fresh.clone(), value.clone());
                map.push((key.to_vec(), fresh));
            }
            merged.set(cat.to_vec(), Object::Dict(mine));
            if !map.is_empty() {
                renames.push((cat, map));
            }
        }
    }
    let renamed = |cat: &[u8], name: &[u8]| -> Option<Vec<u8>> {
        renames.iter().find(|(c, _)| *c == cat).and_then(|(_, m)| m.iter().find(|(from, _)| from == name)).map(|(_, to)| to.clone())
    };
    // A nested form, Type 3 font or tiling pattern without resources of its own looks its names
    // up in the form's (the renderer chains them to whatever draws it). Drawn from the page, it
    // would find the page's, which differ wherever a name was renamed (every form resource that
    // isn't already the page's, the nested item's own entry included).
    if !renames.is_empty() && ops.iter().any(|op| inherits_resources(doc, &form_res, op)) {
        return Err(Refusal { renaming: true, ..refuse("it draws a nested form, Type 3 font or pattern that uses its resources") });
    }
    let mut depth = 0usize;
    // Marked content and text objects must balance within the form's own stream: an extra EMC
    // would close one the page opened, and one left open would take in the page's later content.
    let mut marked = 0usize;
    let mut in_text = false;
    for op in &mut ops {
        match op.op.as_slice() {
            b"BDC" | b"BMC" => marked = marked.saturating_add(1),
            b"EMC" => marked = marked.checked_sub(1).ok_or_else(|| invalid("its content ends marked content it didn't begin"))?,
            b"BT" if in_text => return Err(invalid("its content begins a text object inside another")),
            b"BT" => in_text = true,
            b"ET" if !in_text => return Err(invalid("its content ends a text object it didn't begin")),
            b"ET" => in_text = false,
            _ => {}
        }
        let slot: Option<(&[u8], usize)> = match op.op.as_slice() {
            b"Tf" | b"Do" | b"gs" | b"sh" | b"cs" | b"CS" => {
                let cat: &[u8] = match op.op.as_slice() {
                    b"Tf" => b"Font",
                    b"Do" => b"XObject",
                    b"gs" => b"ExtGState",
                    b"sh" => b"Shading",
                    _ => b"ColorSpace",
                };
                Some((cat, 0))
            }
            b"scn" | b"SCN" => op.operands.len().checked_sub(1).map(|i| (&b"Pattern"[..], i)),
            b"BDC" | b"DP" => Some((&b"Properties"[..], 1)),
            b"q" => {
                if depth >= 4096 {
                    return Err(invalid("graphics state is too deeply nested"));
                }
                depth += 1;
                None
            }
            b"Q" => {
                depth = depth.checked_sub(1).ok_or_else(|| invalid("its content restores more graphics states than it saves"))?;
                None
            }
            _ => None,
        };
        if let Some((cat, i)) = slot
            && let Some(operand) = op.operands.get_mut(i)
            && let Object::Name(n) = operand
            && let Some(to) = renamed(cat, n)
        {
            *operand = Object::Name(to);
        }
        if let Some((dict, _)) = &mut op.inline {
            for key in [&b"CS"[..], b"ColorSpace"] {
                if let Some(Object::Name(n)) = dict.get(key)
                    && let Some(to) = renamed(b"ColorSpace", n)
                {
                    dict.set(key.to_vec(), Object::Name(to));
                }
            }
        }
    }

    if marked > 0 || in_text {
        return Err(invalid("its content leaves marked content or a text object open"));
    }

    // Patterns keep their place relative to the shapes they fill: each one the form selects is
    // drawn from a copy that also carries the placement. Nested forms and patterns inside
    // patterns keep their own parents, so only this stream's selections change.
    let mut patterns: Vec<(Vec<u8>, Object)> = Vec::new();
    let mut pattern_bytes = MAX_FORM_BYTES;
    if placement != Matrix::IDENTITY {
        let mut table = merged.get(b"Pattern").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
        let mut moved: Renames = Vec::new();
        let mut n = 0u32;
        for op in &mut ops {
            if !(op.is("scn") || op.is("SCN")) {
                continue;
            }
            let Some(i) = op.operands.len().checked_sub(1) else { continue };
            let Some(Object::Name(from)) = op.operands.get(i) else { continue };
            // A name the resources don't have selects nothing, inline or not.
            let Some(entry) = table.get(from).cloned() else { continue };
            let to = match moved.iter().find(|(f, _)| f == from) {
                Some((_, to)) => to.clone(),
                None => {
                    if moved.len() >= MAX_MOVED_PATTERNS {
                        return Err(refuse("it paints with too many patterns"));
                    }
                    let copy =
                        moved_pattern(doc, &entry, placement).ok_or_else(|| refuse("it paints with a pattern whose position can't be kept"))?;
                    if let Object::Stream(s) = &copy {
                        pattern_bytes =
                            pattern_bytes.checked_sub(s.raw.len()).ok_or_else(|| refuse("its pattern copies exceed the editing budget"))?;
                    }
                    let fresh = loop {
                        n = n.saturating_add(1);
                        let candidate = format!("PCUgPat{n}").into_bytes();
                        if table.get(&candidate).is_none() {
                            break candidate;
                        }
                        if n == u32::MAX {
                            return Err(refuse("too many resources"));
                        }
                    };
                    // Reserved here; the caller replaces it with the copy's reference.
                    table.set(fresh.clone(), Object::Null);
                    patterns.push((fresh.clone(), copy));
                    moved.push((from.clone(), fresh.clone()));
                    fresh
                }
            };
            if let Some(operand) = op.operands.get_mut(i) {
                *operand = Object::Name(to);
            }
        }
        if !patterns.is_empty() {
            merged.set(b"Pattern".to_vec(), Object::Dict(table));
        }
    }

    // A form in a layer (/OC) stays in it: its content is marked with the same membership.
    let layer = match s.dict.get(b"OC") {
        None => None,
        Some(oc) => {
            let mut props = merged.get(b"Properties").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
            let existing = props.iter().find(|(_, v)| *v == oc).map(|(k, _)| k.clone());
            let name = match existing {
                Some(k) => k,
                None => {
                    let fresh = (1..=u32::MAX)
                        .map(|n| format!("PCUgOC{n}").into_bytes())
                        .find(|c| props.get(c).is_none())
                        .ok_or_else(|| refuse("too many resources"))?;
                    props.set(fresh.clone(), oc.clone());
                    merged.set(b"Properties".to_vec(), Object::Dict(props));
                    fresh
                }
            };
            Some(name)
        }
    };

    let num = |v: f64| Object::Real(v);
    let mut out = Vec::new();
    if let Some(name) = &layer {
        out.push(Op::new("BDC", vec![Object::name("OC"), Object::Name(name.clone())]));
    }
    out.push(Op::new("q", Vec::new()));
    if let Some(m) = matrix {
        out.push(Op::new("cm", m.0.iter().copied().map(num).collect()));
    }
    let [a, b, c, d] = <[f64; 4]>::try_from(bbox).map_err(|_| invalid("invalid form bounds"))?;
    let (x0, y0, x1, y1) = (a.min(c), b.min(d), a.max(c), b.max(d));
    out.push(Op::new("re", vec![num(x0), num(y0), num(x1 - x0), num(y1 - y0)]));
    out.push(Op::new("W", Vec::new()));
    out.push(Op::new("n", Vec::new()));
    out.extend(ops);
    // A form discards its unfinished current path when returning to its caller.
    out.push(Op::new("n", Vec::new()));
    // A form ends with the state it was drawn with (the Do saves and restores it).
    out.extend(std::iter::repeat_with(|| Op::new("Q", Vec::new())).take(depth));
    out.push(Op::new("Q", Vec::new()));
    if layer.is_some() {
        out.push(Op::new("EMC", Vec::new()));
    }
    Ok(Ungrouped { ops: out, resources: merged, patterns, shows_text })
}

/// Whether form `name` in `resources` would merge into them without a rename: it has resources
/// of its own, and each of their entries is already in `resources` under the same name, as
/// [`ungroup_ops`] compares them. Only dictionaries are read, never the form's content.
pub(crate) fn merges_without_renaming(doc: &Document, resources: &Dict, name: &[u8]) -> bool {
    let category = |d: &Dict, cat: &[u8]| d.get(cat).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned());
    let Some(form) = category(resources, b"XObject").and_then(|x| x.get(name).cloned()) else { return false };
    let Object::Stream(s) = &*doc.resolve(&form) else { return false };
    let Some(own) = s.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()) else { return false };
    CATEGORIES.into_iter().all(|cat| {
        let Some(theirs) = category(&own, cat) else { return true };
        let mine = category(resources, cat).unwrap_or_default();
        // Past these sizes `ungroup_ops` refuses the form anyway.
        theirs.len() <= 4096 && mine.len() <= 4096 && theirs.iter().all(|(key, value)| mine.get(key) == Some(value))
    })
}

/// A copy of pattern `entry` whose `/Matrix` maps its space to the page's default space instead
/// of the form's: its own matrix, then `placement`. `None` when it isn't a readable pattern, its
/// matrix is unreadable, the result isn't finite or a tiling pattern is too large to copy.
fn moved_pattern(doc: &Document, entry: &Object, placement: Matrix) -> Option<Object> {
    let resolved = doc.resolve(entry);
    let (mut dict, stream) = match &*resolved {
        Object::Stream(s) => (s.dict.clone(), Some(s)),
        Object::Dict(d) => (d.clone(), None),
        _ => return None,
    };
    let own = match dict.get(b"Matrix") {
        None => Matrix::IDENTITY,
        Some(m) => {
            let m = doc.resolve(m);
            if m.as_array()?.len() != 6 {
                return None;
            }
            let nums: Vec<Object> = m.as_array()?.iter().map(|x| doc.resolve(x).as_ref().clone()).collect();
            Matrix::from_operands(&nums)?
        }
    };
    let moved = own.then(&placement);
    if !moved.0.iter().all(|v| v.is_finite()) {
        return None;
    }
    dict.set(b"Matrix".to_vec(), Object::Array(moved.0.iter().map(|v| Object::Real(*v)).collect()));
    match stream {
        Some(s) if s.raw.len() > MAX_FORM_BYTES => None,
        Some(s) => Some(Object::Stream(pdfcraft_cos::Stream { dict, raw: s.raw.clone() })),
        None => Some(Object::Dict(dict)),
    }
}

/// How deep [`blends_inside`] follows nested forms; deeper ones count as blending.
const MAX_NESTING: usize = 8;

/// The most nested forms and tiling patterns [`blends_inside`] reads in all; past that the rest
/// count as blending, so a file can't make the check run for ever.
const MAX_NESTED_READS: usize = 64;

/// What [`blends_inside`] carries through its nested reads: the forms on the current path (a
/// form drawing itself), the answers for each form with resources of its own (whether it blends
/// and whether it shows text; neither depends on who draws it), how many reads it has made, and
/// whether the content being walked shows text itself or through a Form XObject it draws.
struct BlendWalk<'a> {
    path: Vec<pdfcraft_cos::ObjRef>,
    known: HashMap<pdfcraft_cos::ObjRef, (bool, bool)>,
    reads: usize,
    budget: &'a mut DecodeBudget,
    text: bool,
}

/// Whether `op` shows text (ISO 32000-2 §9.4.3, Table 107).
fn shows_text(op: &Op) -> bool {
    matches!(op.op.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"")
}

/// How many operands an operator that names a resource takes (ISO 32000-2 Annex A), its name
/// being the first of them: only for those whose name is read at a fixed position here.
fn takes_operands(op: &[u8]) -> Option<usize> {
    match op {
        b"Do" | b"gs" | b"sh" | b"cs" | b"CS" => Some(1),
        b"Tf" | b"BDC" | b"DP" => Some(2),
        _ => None,
    }
}

/// Whether `op` draws a Form XObject (`Do`), Type 3 font (`Tf`) or tiling pattern (`scn`/`SCN`) from
/// `resources` that has no `/Resources` of its own, and so inherits the ones it is drawn with.
fn inherits_resources(doc: &Document, resources: &Dict, op: &Op) -> bool {
    let (cat, i): (&[u8], usize) = match op.op.as_slice() {
        b"Do" => (b"XObject", 0),
        b"Tf" => (b"Font", 0),
        b"scn" | b"SCN" => match op.operands.len().checked_sub(1) {
            Some(i) => (b"Pattern", i),
            None => return false,
        },
        _ => return false,
    };
    let Some(Object::Name(name)) = op.operands.get(i) else { return false };
    let Some(entry) = resources.get(cat).map(|d| doc.resolve(d)).and_then(|d| d.as_dict().and_then(|d| d.get(name).cloned())) else {
        return false;
    };
    let dict = match &*doc.resolve(&entry) {
        Object::Stream(s) => s.dict.clone(),
        Object::Dict(d) => d.clone(),
        _ => return false,
    };
    let drawn = match cat {
        b"XObject" => is_name(doc, &dict, b"Subtype", b"Form"),
        b"Font" => is_name(doc, &dict, b"Subtype", b"Type3"),
        // A shading pattern has no content of its own.
        _ => int_of(doc, &dict, b"PatternType") == Some(1),
    };
    drawn && dict.get(b"Resources").is_none()
}

/// Whether `ops` blend through a used graphics state, nested form or pattern. Unreadable,
/// recursive or over-budget nested content is treated as blending, so preparation refuses it.
/// Sets `walk.text` when `ops` show text, or a Form XObject they draw does; text in a tiling
/// pattern's cell is not the page's text and is not counted.
fn blends_inside(doc: &Document, resources: &Dict, ops: &[Op], depth: usize, walk: &mut BlendWalk<'_>) -> bool {
    let category = |cat: &[u8]| resources.get(cat).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
    let (states, xobjects, patterns) = (category(b"ExtGState"), category(b"XObject"), category(b"Pattern"));
    let state_blends = |g: Option<&Object>| g.map(|g| doc.resolve(g)).and_then(|g| g.as_dict().cloned()).is_some_and(|gs| blends(doc, &gs));
    if ops.iter().any(shows_text) {
        walk.text = true;
    }
    for op in ops {
        // Read at operand 0 below; with a surplus operand the renderer draws another name, which
        // this walk can't vouch for.
        if takes_operands(&op.op).is_some_and(|n| op.operands.len() > n) {
            return true;
        }
        if op.is("gs") && state_blends(op.name(0).and_then(|n| states.get(n))) {
            return true;
        }
        let o = if op.is("Do") {
            op.name(0).and_then(|n| xobjects.get(n))
        } else if op.is("scn") || op.is("SCN") {
            op.operands.len().checked_sub(1).and_then(|i| op.name(i)).and_then(|n| patterns.get(n))
        } else {
            continue;
        };
        let Some(o) = o else { continue };
        let resolved = doc.resolve(o);
        let nested = match &*resolved {
            Object::Stream(st) if op.is("Do") && is_name(doc, &st.dict, b"Subtype", b"Form") => st,
            Object::Stream(st) if !op.is("Do") && int_of(doc, &st.dict, b"PatternType") == Some(1) => st,
            // A shading pattern.
            Object::Stream(st) if !op.is("Do") => {
                if state_blends(st.dict.get(b"ExtGState")) {
                    return true;
                }
                continue;
            }
            Object::Dict(d) if !op.is("Do") => {
                if state_blends(d.get(b"ExtGState")) {
                    return true;
                }
                continue;
            }
            _ => continue,
        };
        let r = o.as_ref();
        let drawn_form = op.is("Do");
        if let Some((known, text)) = r.and_then(|r| walk.known.get(&r)).copied() {
            if known {
                return true;
            }
            walk.text |= drawn_form && text;
            continue;
        }
        if depth >= MAX_NESTING || r.is_some_and(|r| walk.path.contains(&r)) || walk.reads >= MAX_NESTED_READS {
            return true;
        }
        walk.reads += 1;
        let Ok(data) = walk.budget.decode_strict(nested) else { return true };
        let own_resources = nested.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned());
        let has_own = own_resources.is_some();
        let own = own_resources.unwrap_or_else(|| resources.clone());
        let inner = parse(&data);
        if inner.skipped > 0 {
            return true;
        }
        if let Some(r) = r {
            walk.path.push(r);
        }
        let outer_text = std::mem::replace(&mut walk.text, false);
        let blends = blends_inside(doc, &own, &inner.ops, depth + 1, walk);
        let inner_text = walk.text;
        walk.text = outer_text || (drawn_form && inner_text);
        if let Some(r) = r {
            walk.path.retain(|x| *x != r);
            if has_own {
                walk.known.insert(r, (blends, inner_text));
            }
        }
        if blends {
            return true;
        }
    }
    false
}

/// The blending colour space of the page group (ISO 32000-2 §11.4.7): the page's transparency
/// `/Group /CS`, resolved. `None` when the page names none, and the PDF processor chooses.
pub(crate) fn page_blend_space(doc: &Document, page: &Dict) -> Option<Object> {
    let group = page.get(b"Group").map(|g| doc.resolve(g)).and_then(|g| g.as_dict().cloned()).filter(|g| is_name(doc, g, b"S", b"Transparency"))?;
    group.get(b"CS").map(|cs| (*doc.resolve(cs)).clone())
}

/// Whether a group blending in `cs` composites as the page does: only when the page group names
/// the same space. A page that names none composites in a space the PDF processor chooses
/// (ISO 32000-2 §11.4.7): DeviceRGB on screen, but often DeviceCMYK when printed, where a group
/// that blends in DeviceRGB converts its CMYK and gray colours through RGB and inline they would
/// not be. Such a group is refused rather than changed.
fn same_blend_space(doc: &Document, cs: &Object, page_space: Option<&Object>) -> bool {
    page_space.is_some_and(|page| *doc.resolve(cs) == *page)
}

/// Whether `d`'s `key` is the name `want`, directly or through an indirect object (any value may
/// be one, ISO 32000-2 §7.3.10).
fn is_name(doc: &Document, d: &Dict, key: &[u8], want: &[u8]) -> bool {
    d.get(key).is_some_and(|v| doc.resolve(v).as_name() == Some(want))
}

/// `d`'s integer `key`, directly or through an indirect object.
fn int_of(doc: &Document, d: &Dict, key: &[u8]) -> Option<i64> {
    d.get(key).and_then(|v| doc.resolve(v).as_int())
}

/// Whether an ExtGState blends with something other than Normal.
fn blends(doc: &Document, gs: &Dict) -> bool {
    let Some(bm) = gs.get(b"BM").map(|b| doc.resolve(b)) else { return false };
    let first = match &*bm {
        Object::Array(a) => a.first().map(|x| doc.resolve(x)),
        _ => Some(bm.clone()),
    };
    !first.is_some_and(|b| matches!(b.as_name(), Some(b"Normal" | b"Compatible")))
}

/// Whether the graphics state is translucent, blended or soft-masked at op `at` of the page's
/// streams parsed as one (the indexes `page_images` gives), following `q`/`Q` and `gs`. Each `gs`
/// changes only the parameters it sets.
pub(crate) fn translucent_at(doc: &Document, resources: &Dict, ops: &[Op], at: usize) -> bool {
    let states = resources.get(b"ExtGState").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
    // Fill alpha below one, stroke alpha below one, a soft mask, a blend mode other than Normal.
    let mut stack: Vec<[bool; 4]> = Vec::new();
    let mut now = [false; 4];
    for op in ops.iter().take(at) {
        match op.op.as_slice() {
            b"q" => stack.push(now),
            b"Q" => now = stack.pop().unwrap_or([false; 4]),
            b"gs" => {
                let Some(gs) = op.name(0).and_then(|n| states.get(n)).map(|g| doc.resolve(g)).and_then(|g| g.as_dict().cloned()) else {
                    continue;
                };
                let alpha = |key: &[u8]| gs.get(key).map(|v| doc.resolve(v)).and_then(|v| v.as_f64()).map(|a| a < 1.0);
                if let Some(v) = alpha(b"ca") {
                    now[0] = v;
                }
                if let Some(v) = alpha(b"CA") {
                    now[1] = v;
                }
                if let Some(m) = gs.get(b"SMask").map(|v| doc.resolve(v)) {
                    now[2] = m.as_name() != Some(b"None");
                }
                if gs.get(b"BM").is_some() {
                    now[3] = blends(doc, &gs);
                }
            }
            _ => {}
        }
    }
    now.iter().any(|v| *v)
}
