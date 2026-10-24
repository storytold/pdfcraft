//! Edit a PDF â–¸ Ungroup: draw a Form XObject's content in place of its `Do` (#2964, #3005).
//!
//! Text inside grouped Form artwork can't be edited, because editing rewrites only the page's own
//! content streams. Ungrouping copies the form's content into the page at the same place,
//! wrapped as `q <Matrix> cm <BBox> re W n â€¦ Q`, with the form's resources merged into a copy of
//! the page's under names that don't clash. Its text then edits like any other. The form object
//! itself is left alone, so another page or placement that draws it is unchanged.
//!
//! Drawing a form inline paints the same pixels unless the form is a transparency group whose
//! compositing differs from painting its parts one by one: a knockout group, a group that uses
//! a blend mode other than Normal inside, or a group drawn translucent, blended or under a soft
//! mask. Those are refused rather than changed, as is tagged form content (its structure link
//! would be lost) and content whose marked content or text objects don't balance (inlined, they
//! would change the scope of the page's own). A form in a layer (`/OC`) keeps its membership as `/OC â€¦ BDC â€¦ EMC`.
//!
//! A pattern's matrix maps pattern space to the default space of the content stream that uses it
//! (ISO 32000-2 Â§8.7.2): inside the form that is the form's space at the `Do`, on the page it is
//! the page's. So each pattern the form selects with `scn`/`SCN` is drawn from a copy whose
//! `/Matrix` also carries the form's placement; one that can't be copied that way is refused.

use std::collections::HashMap;

use pdfcraft_content::{Matrix, Op, parse};
use pdfcraft_cos::{Dict, Document, Object};

use crate::EditError;

/// The largest form content ungrouped (decoded bytes): the page's own content cap is far above
/// any real form, and a bomb must not be copied into the page.
const MAX_FORM_BYTES: usize = 64 << 20;

/// Resource categories a content stream refers to by name (ISO 32000-2 Â§7.8.3, Table 34).
const CATEGORIES: [&[u8]; 7] = [b"Font", b"XObject", b"ExtGState", b"ColorSpace", b"Pattern", b"Shading", b"Properties"];

/// The most distinct patterns one ungrouped form may select; each becomes a new object.
const MAX_MOVED_PATTERNS: usize = 256;

/// A name map for one resource category: form name â†’ name in the page's resources.
type Renames = Vec<(Vec<u8>, Vec<u8>)>;

/// What [`ungroup_ops`] gives back: the ops that draw the form inline, the page resources with
/// the form's merged in, and the pattern copies to add to the document, each under the name in
/// the merged `/Pattern` dictionary that the ops already use.
pub(crate) type Ungrouped = (Vec<Op>, Dict, Vec<(Vec<u8>, Object)>);

/// Ungroup form `name` from the page's `resources`. `translucent` says whether the graphics state
/// at the `Do` is translucent, blended or soft-masked, which a transparency group composites as a
/// whole. `placement` maps the form's space to the page's default space at the `Do` (its
/// `/Matrix`, then the CTM there).
pub(crate) fn ungroup_ops(doc: &Document, resources: &Dict, name: &[u8], translucent: bool, placement: Matrix) -> Result<Ungrouped, EditError> {
    let refuse = |why: &str| EditError::Invalid(format!("this artwork can't be ungrouped: {why}"));
    let xobjects = resources.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).ok_or_else(|| refuse("it is missing"))?;
    let form = xobjects.get(name).ok_or_else(|| refuse("it is missing"))?;
    let Object::Stream(s) = &*doc.resolve(form) else { return Err(refuse("it is not a Form XObject")) };
    if !is_name(doc, &s.dict, b"Subtype", b"Form") {
        return Err(refuse("it is not a Form XObject"));
    }
    let numbers = |key: &[u8], n: usize| -> Option<Vec<f64>> {
        let a = s.dict.get(key).map(|o| doc.resolve(o))?;
        let v: Vec<f64> = a.as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
        (v.len() == n && v.iter().all(|x| x.is_finite())).then_some(v)
    };
    let bbox = numbers(b"BBox", 4).ok_or_else(|| refuse("its bounding box is missing or unreadable"))?;
    let matrix = match s.dict.get(b"Matrix") {
        None => None,
        Some(_) => Some(Matrix(
            <[f64; 6]>::try_from(numbers(b"Matrix", 6).ok_or_else(|| refuse("its matrix is unreadable"))?)
                .map_err(|_| refuse("its matrix is unreadable"))?,
        )),
    };
    let own = s.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned());
    // A form without its own resources uses the ones it is drawn with (PDF 1.1).
    let form_res = own.clone().unwrap_or_else(|| resources.clone());

    // A tagged form's content is a structure element's: drawn inline it would lose that link.
    if s.dict.get(b"StructParent").is_some() || s.dict.get(b"StructParents").is_some() {
        return Err(refuse("it is tagged content (Tags panel); edit its text in a copy without tags"));
    }
    let group = s.dict.get(b"Group").map(|g| doc.resolve(g)).and_then(|g| g.as_dict().cloned()).filter(|g| is_name(doc, g, b"S", b"Transparency"));
    if let Some(group) = &group {
        if group.get(b"K").is_some_and(|k| matches!(*doc.resolve(k), Object::Bool(true))) {
            return Err(refuse("it is a knockout transparency group"));
        }
        if translucent {
            return Err(refuse("it is a transparency group drawn translucent, blended or under a soft mask"));
        }
    }

    // Strictly: damaged data must not be copied in as a shorter form.
    let data = s.decoded_strict_within(MAX_FORM_BYTES).map_err(|e| refuse(&format!("its content can't be read ({e})")))?;
    let parsed = parse(&data);
    if parsed.skipped > 0 {
        return Err(refuse("its content has parts that can't be read"));
    }
    let mut ops = parsed.ops;
    if group.is_some() {
        // The form itself is being read: drawing itself again counts as blending.
        let mut walk = BlendWalk { path: form.as_ref().into_iter().collect(), known: HashMap::new(), reads: 0 };
        if blends_inside(doc, &form_res, &ops, 0, &mut walk) {
            return Err(refuse("it is a transparency group with blend modes inside"));
        }
    }

    // Merge the form's resources into a copy of the page's, renaming what clashes.
    let mut merged = resources.clone();
    let mut renames: Vec<(&[u8], Renames)> = Vec::new();
    if own.is_some() {
        for cat in CATEGORIES {
            let Some(theirs) = form_res.get(cat).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()) else { continue };
            let mut mine = merged.get(cat).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
            let mut map = Renames::new();
            let mut n = 0u32;
            for (key, value) in theirs.iter() {
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
        return Err(refuse("it draws a nested form, Type 3 font or pattern that uses its resources"));
    }
    let mut depth = 0usize;
    // Marked content and text objects must balance within the form's own stream: an extra EMC
    // would close one the page opened, and one left open would take in the page's later content.
    let mut marked = 0usize;
    let mut in_text = false;
    for op in &mut ops {
        match op.op.as_slice() {
            b"BDC" | b"BMC" => marked = marked.saturating_add(1),
            b"EMC" => marked = marked.checked_sub(1).ok_or_else(|| refuse("its content ends marked content it didn't begin"))?,
            b"BT" if in_text => return Err(refuse("its content begins a text object inside another")),
            b"BT" => in_text = true,
            b"ET" if !in_text => return Err(refuse("its content ends a text object it didn't begin")),
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
                depth = depth.saturating_add(1);
                None
            }
            b"Q" => {
                depth = depth.checked_sub(1).ok_or_else(|| refuse("its content restores more graphics states than it saves"))?;
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
        return Err(refuse("its content leaves marked content or a text object open"));
    }

    // Patterns keep their place relative to the shapes they fill: each one the form selects is
    // drawn from a copy that also carries the placement. Nested forms and patterns inside
    // patterns keep their own parents, so only this stream's selections change.
    let mut patterns: Vec<(Vec<u8>, Object)> = Vec::new();
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
    let (x0, y0, x1, y1) = (bbox[0].min(bbox[2]), bbox[1].min(bbox[3]), bbox[0].max(bbox[2]), bbox[1].max(bbox[3]));
    out.push(Op::new("re", vec![num(x0), num(y0), num(x1 - x0), num(y1 - y0)]));
    out.push(Op::new("W", Vec::new()));
    out.push(Op::new("n", Vec::new()));
    out.extend(ops);
    // A form ends with the state it was drawn with (the Do saves and restores it).
    out.extend(std::iter::repeat_with(|| Op::new("Q", Vec::new())).take(depth));
    out.push(Op::new("Q", Vec::new()));
    if layer.is_some() {
        out.push(Op::new("EMC", Vec::new()));
    }
    Ok((out, merged, patterns))
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
const MAX_NESTED_READS: usize = 1024;

/// What [`blends_inside`] carries through its nested reads: the forms on the current path (a
/// form drawing itself), the answer for each form with resources of its own (its answer doesn't
/// depend on who draws it), and how many reads it has made.
struct BlendWalk {
    path: Vec<pdfcraft_cos::ObjRef>,
    known: HashMap<pdfcraft_cos::ObjRef, bool>,
    reads: usize,
}

/// Whether `ops` (drawn with `resources`) blend with something other than Normal: a `gs` they
/// use, or one used by a form or pattern they draw, at any depth. A shading pattern blends through
/// its own `/ExtGState`, a tiling pattern through its content. A nested form or pattern that can't
/// be read, is nested too deep or draws itself counts as blending, so the caller refuses rather
/// than guesses.
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

fn blends_inside(doc: &Document, resources: &Dict, ops: &[Op], depth: usize, walk: &mut BlendWalk) -> bool {
    let category = |cat: &[u8]| resources.get(cat).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
    let (states, xobjects, patterns) = (category(b"ExtGState"), category(b"XObject"), category(b"Pattern"));
    let state_blends = |g: Option<&Object>| g.map(|g| doc.resolve(g)).and_then(|g| g.as_dict().cloned()).is_some_and(|gs| blends(doc, &gs));
    for op in ops {
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
        if let Some(known) = r.and_then(|r| walk.known.get(&r)) {
            if *known {
                return true;
            }
            continue;
        }
        if depth >= MAX_NESTING || r.is_some_and(|r| walk.path.contains(&r)) || walk.reads >= MAX_NESTED_READS {
            return true;
        }
        walk.reads += 1;
        let Ok(data) = nested.decoded_strict_within(MAX_FORM_BYTES) else { return true };
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
        let blends = blends_inside(doc, &own, &inner.ops, depth + 1, walk);
        if let Some(r) = r {
            walk.path.retain(|x| *x != r);
            if has_own {
                walk.known.insert(r, blends);
            }
        }
        if blends {
            return true;
        }
    }
    false
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
