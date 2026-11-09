//! Edit a PDF ▸ Ungroup: draw a Form XObject's content in place of its `Do` (#2964, #3005).
//!
//! Text inside grouped Form artwork can't be edited, because editing rewrites only the page's own
//! content streams. Ungrouping copies the form's content into the page at the same place,
//! wrapped as `q <Matrix> cm <BBox> re W n … Q`, with the form's resources merged into a copy of
//! the page's under names that don't clash. Its text then edits like any other. The form object
//! itself is left alone, so another page or placement that draws it is unchanged. Each resource
//! name is read from the operand the renderer uses ([`resource_operand`]), so a surplus operand
//! before it changes neither the renaming nor the checks below.
//!
//! Drawing a form inline paints the same pixels unless the form is a transparency group whose
//! compositing differs from painting its parts one by one: a knockout group, a group that uses
//! a blend mode other than Normal inside, or a group drawn translucent, blended or under a soft
//! mask, or a group with a blending colour space (`/CS`) other than the page's: its contents
//! composite in that space and are then converted (ISO 32000-2 §11.6.6), inline they would
//! composite in the page's. Those are refused rather than changed, as is a form with resources
//! of its own whose `/DefaultGray`, `/DefaultRGB` or `/DefaultCMYK` differs from the page's, one
//! having it and the other not included (it remaps the device colours painted with it,
//! §8.6.5.6, and inline the page's resources apply), tagged form content (its structure link
//! would be lost) and content whose marked content or text objects don't balance (inlined, they
//! would change the scope of the page's own). A form in a layer (`/OC`) keeps its membership as `/OC … BDC … EMC`.
//!
//! A pattern's matrix maps pattern space to the default space of the content stream that uses it
//! (ISO 32000-2 §8.7.2): inside the form that is the form's space at the `Do`, on the page it is
//! the page's. So each pattern the form selects with `scn`/`SCN` is drawn from a copy whose
//! `/Matrix` also carries the form's placement; one that can't be copied that way is refused.
//!
//! Edit text's preparation of grouped text (`text::forms`) opens each placement with this same
//! code, and decides what to open from what [`Refusal`] and [`Ungrouped::shows_text`] say.

use std::collections::HashMap;

use pdfcraft_content::{Matrix, Op, parse};
use pdfcraft_cos::{DecodeBudget, Dict, Document, Object};

use crate::EditError;

/// The largest form content ungrouped (decoded bytes): the page's own content cap is far above
/// any real form, and a bomb must not be copied into the page. Ungroup reads the form and what it
/// draws within one [`DecodeBudget`] of this size.
pub(crate) const MAX_FORM_BYTES: usize = 64 << 20;

/// Resource categories a content stream refers to by name (ISO 32000-2 §7.8.3, Table 34).
pub(crate) const CATEGORIES: [&[u8]; 7] = [b"Font", b"XObject", b"ExtGState", b"ColorSpace", b"Pattern", b"Shading", b"Properties"];

/// The `/ColorSpace` entries that remap device colours painted with the resources they are in
/// (ISO 32000-2 §8.6.5.6).
pub(crate) const DEFAULT_SPACES: [&[u8]; 3] = [b"DefaultGray", b"DefaultRGB", b"DefaultCMYK"];

/// The most distinct patterns one ungrouped form may select; each becomes a new object.
const MAX_MOVED_PATTERNS: usize = 256;

/// The most entries of one resource category merged: a form or page with more is refused.
const MAX_RESOURCES: usize = 4096;

/// The deepest `q` nesting a form's content may have.
const MAX_SAVES: usize = 4096;

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
    /// inlining the form doesn't make it editable. `None` when something it draws can't be read,
    /// is nested too deep, draws itself, or is past the reads or the decode budget allowed.
    pub(crate) shows_text: Option<bool>,
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

/// Ungroup the form drawn by the `Do` at `page_ops[at]` (the page's streams parsed as one, the
/// indexes `page_images` gives) from the page's `resources`. `page_space` is the page group's
/// blending colour space ([`page_blend_space`]). `placement` maps the form's space to the page's
/// default space at the `Do` (its `/Matrix`, then the CTM there). The form, and the forms and
/// patterns it draws, decode within `budget`.
pub(crate) fn ungroup_ops(
    doc: &Document,
    resources: &Dict,
    page_ops: &[Op],
    at: usize,
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
    let op = page_ops.get(at).filter(|op| op.is("Do")).ok_or_else(|| refuse("it is missing"))?;
    // The renderer draws a `Do`'s last operand, and the placement was read for its first
    // (`page_images`): with more than one they can be different forms.
    if op.operands.len() != 1 {
        return Err(refuse("it is drawn with an operand too many"));
    }
    let name = op.name(0).ok_or_else(|| refuse("it is missing"))?;
    let (translucent, open) = state_at(doc, resources, page_ops, at);
    // Inline, its clip would take in the page's unfinished path and its text objects would be
    // nested in the page's.
    if open {
        return Err(refuse("it is drawn inside an unfinished path or text object"));
    }
    let xobjects = resources.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).ok_or_else(|| refuse("it is missing"))?;
    let form = xobjects.get(name).ok_or_else(|| refuse("it is missing"))?;
    let Object::Stream(s) = &*doc.resolve(form) else { return Err(refuse("it is not a Form XObject")) };
    if !is_name(doc, &s.dict, b"Subtype", b"Form") {
        return Err(refuse("it is not a Form XObject"));
    }
    let numbers = |key: &[u8], n: usize| -> Option<Vec<f64>> {
        let a = s.dict.get(key).map(|o| doc.resolve(o))?;
        let a = a.as_array()?;
        // Every entry must be a number: a short array padded out by a non-number is unreadable.
        let v: Vec<f64> = a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
        (a.len() == n && v.len() == n && v.iter().all(|x| x.is_finite())).then_some(v)
    };
    let bbox = numbers(b"BBox", 4).and_then(|b| <[f64; 4]>::try_from(b).ok()).ok_or_else(|| invalid("its bounding box is missing or unreadable"))?;
    let matrix = match s.dict.get(b"Matrix") {
        None => None,
        Some(_) => Some(Matrix(numbers(b"Matrix", 6).and_then(|m| <[f64; 6]>::try_from(m).ok()).ok_or_else(|| invalid("its matrix is unreadable"))?)),
    };
    let own = s.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned());
    if s.dict.get(b"Resources").is_some() && own.is_none() {
        return Err(invalid("its resources can't be read"));
    }
    // A form without its own resources uses the ones it is drawn with (PDF 1.1).
    let form_res = own.clone().unwrap_or_else(|| resources.clone());

    // A tagged form's content is a structure element's: drawn inline it would lose that link.
    if s.dict.get(b"StructParent").is_some() || s.dict.get(b"StructParents").is_some() {
        return Err(invalid("it is tagged content (Tags panel); edit its text in a copy without tags"));
    }
    let group_dict = s.dict.get(b"Group").map(|g| doc.resolve(g)).and_then(|g| g.as_dict().cloned());
    if s.dict.get(b"Group").is_some() && group_dict.is_none() {
        return Err(invalid("its group dictionary can't be read"));
    }
    let group = group_dict.filter(|g| is_name(doc, g, b"S", b"Transparency"));
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
    let data = budget.decode_strict(doc, s).map_err(|e| refuse(&format!("its content can't be read ({e})")))?;
    let parsed = parse(&data);
    if parsed.skipped > 0 {
        return Err(invalid("its content has parts that can't be read"));
    }
    let mut ops = parsed.ops;
    // What it draws is read for text whether it is a group or not, and for blend modes only in a
    // group: without one, blending inside draws the same inline. The form itself is being read,
    // so drawing itself again is content that can't be checked.
    let mut walk =
        BlendWalk { path: form.as_ref().into_iter().collect(), known: HashMap::new(), reads: 0, budget, text: false, group: group.is_some() };
    let unchecked = blends_inside(doc, &form_res, &ops, 0, &mut walk);
    if unchecked && group.is_some() {
        return Err(refuse("it is a transparency group with blend modes inside, or content inside that can't be checked"));
    }
    let shows_text = (!unchecked).then_some(walk.text);

    // A form's default colour space remaps its device colours (ISO 32000-2 §8.6.5.6); inline,
    // the page's resources are the current ones (§7.8.3).
    if own.is_some()
        && let Some((key, form_has)) = unshared_default_space(doc, &form_res, resources)
    {
        let key = String::from_utf8_lossy(key);
        return Err(refuse(&if form_has {
            format!("it remaps its device colours with a /{key} the page doesn't share")
        } else {
            format!("the page's /{key} would remap its device colours, which it draws as they are")
        }));
    }

    // Merge the form's resources into a copy of the page's, renaming what clashes.
    let mut merged = resources.clone();
    let mut renames: Vec<(&[u8], Renames)> = Vec::new();
    if own.is_some() {
        for cat in CATEGORIES {
            let Some(theirs) = form_res.get(cat).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()) else { continue };
            if theirs.len() > MAX_RESOURCES {
                return Err(invalid("too many resources"));
            }
            let mut mine = merged.get(cat).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
            if mine.len() > MAX_RESOURCES {
                return Err(refuse("too many resources"));
            }
            let mut map = Renames::new();
            let mut n = 0u32;
            for (key, value) in theirs.iter() {
                // A default colour space checked above is the page's, which applies inline;
                // renamed, it would be an entry nothing uses.
                if mine.get(key) == Some(value) || (cat == b"ColorSpace" && DEFAULT_SPACES.contains(&&key[..])) {
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
            // Counted after the merge: a page taken past the limit would refuse every later form,
            // even one whose resources are all the page's already.
            if mine.len() > MAX_RESOURCES {
                return Err(refuse("too many resources"));
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
    if !renames.is_empty() && ops.iter().any(|op| inherits_resources(doc, &form_res, op, budget)) {
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
        match op.op.as_slice() {
            b"q" if depth >= MAX_SAVES => return Err(invalid("its graphics states are nested too deeply")),
            b"q" => depth += 1,
            b"Q" => depth = depth.checked_sub(1).ok_or_else(|| invalid("its content restores more graphics states than it saves"))?,
            _ => {}
        }
        let slot = resource_operand(op).map_err(|Unreadable| invalid("its content selects a resource with operands that can't be read"))?;
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
    // What the copies of tiling patterns may add in all.
    let mut pattern_bytes = MAX_FORM_BYTES;
    if placement != Matrix::IDENTITY {
        let mut table = merged.get(b"Pattern").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
        let mut moved: Renames = Vec::new();
        let mut n = 0u32;
        for op in &mut ops {
            let Ok(Some((b"Pattern", i))) = resource_operand(op) else { continue };
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
                        pattern_bytes = pattern_bytes.checked_sub(s.raw.len()).ok_or_else(|| refuse("its pattern copies would be too large"))?;
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
    let [a, b, c, d] = bbox;
    let (x0, y0, x1, y1) = (a.min(c), b.min(d), a.max(c), b.max(d));
    out.push(Op::new("re", vec![num(x0), num(y0), num(x1 - x0), num(y1 - y0)]));
    out.push(Op::new("W", Vec::new()));
    out.push(Op::new("n", Vec::new()));
    out.extend(ops);
    // A path the form leaves unfinished ends with it, not in the page's content after it.
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
        // Past these sizes `ungroup_ops` refuses the form anyway. Default colour spaces are
        // never renamed (`ungroup_ops` checks them separately).
        theirs.len() <= MAX_RESOURCES
            && mine.len() <= MAX_RESOURCES
            && theirs.iter().all(|(key, value)| mine.get(key) == Some(value) || (cat == b"ColorSpace" && DEFAULT_SPACES.contains(&&key[..])))
    })
}

/// The first of `/DefaultGray`, `/DefaultRGB` and `/DefaultCMYK` that the form's and the page's
/// `/ColorSpace` resources don't share with an equal value (the same object, or the same value
/// resolved), and whether the form has it (else only the page does). A default colour space
/// remaps the device colours painted with the resource dictionary it is in (ISO 32000-2
/// §8.6.5.6, §7.8.3), so inline only the page's would apply: the form's would no longer remap
/// its colours, and the page's would start to.
fn unshared_default_space(doc: &Document, form: &Dict, page: &Dict) -> Option<(&'static [u8], bool)> {
    let spaces = |res: &Dict| res.get(b"ColorSpace").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
    let (theirs, mine) = (spaces(form), spaces(page));
    let differs = |key: &[u8]| match (theirs.get(key), mine.get(key)) {
        (None, None) => false,
        (Some(v), Some(p)) => *doc.resolve(p) != *doc.resolve(v),
        _ => true,
    };
    // The form's own entries are named first.
    let form_has = DEFAULT_SPACES.into_iter().find(|key| theirs.get(key).is_some() && differs(key)).map(|key| (key, true));
    form_has.or_else(|| DEFAULT_SPACES.into_iter().find(|key| differs(key)).map(|key| (key, false)))
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
            // A matrix is six numbers: a longer array is unreadable, not cut short.
            if nums.len() != 6 {
                return None;
            }
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
/// form drawing itself), the answers for each form with resources of its own (whether it blends
/// or can't be checked, and whether it shows text; neither depends on who draws it), how many
/// reads it has made, the decode budget they share with the form itself, whether the content
/// being walked shows text itself or through a Form XObject it draws, and whether blend modes
/// count (they do in a group).
struct BlendWalk<'a> {
    path: Vec<pdfcraft_cos::ObjRef>,
    known: HashMap<pdfcraft_cos::ObjRef, (bool, bool)>,
    reads: usize,
    budget: &'a mut DecodeBudget,
    text: bool,
    group: bool,
}

/// Whether `op` shows text (ISO 32000-2 §9.4.3, Table 107).
fn shows_text(op: &Op) -> bool {
    matches!(op.op.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"")
}

/// An operator whose operands the renderer can't read as it expects, so it skips the operator.
pub(crate) struct Unreadable;

/// Which operand of `op` names a resource, and from which category, read as the renderer reads
/// it: operands are taken from the end, so a surplus operand before them is ignored. `Do`, `gs`,
/// `sh`, `cs` and `CS` take the last operand; `Tf` a name then a size; `BDC` and `DP` a tag then
/// a name or an inline dictionary; `scn` and `SCN` numbers and the last name among them.
/// `Ok(None)` when `op` names no resource; [`Unreadable`] when the renderer would skip it, which
/// [`ungroup_ops`] refuses rather than guess at.
pub(crate) fn resource_operand(op: &Op) -> Result<Option<(&'static [u8], usize)>, Unreadable> {
    let operands = &op.operands;
    let name_at = |i: Option<usize>| i.is_some_and(|i| matches!(operands.get(i), Some(Object::Name(_))));
    let last = operands.len().checked_sub(1);
    let (cat, at): (&'static [u8], Option<usize>) = match op.op.as_slice() {
        b"Do" => (b"XObject", last),
        b"gs" => (b"ExtGState", last),
        b"sh" => (b"Shading", last),
        b"cs" | b"CS" => (b"ColorSpace", last),
        b"Tf" => {
            if !last.is_some_and(|i| matches!(operands.get(i), Some(Object::Int(_) | Object::Real(_)))) {
                return Err(Unreadable);
            }
            (b"Font", operands.len().checked_sub(2))
        }
        b"BDC" | b"DP" => {
            if !name_at(operands.len().checked_sub(2)) {
                return Err(Unreadable);
            }
            match last.and_then(|i| operands.get(i)) {
                Some(Object::Name(_)) => (b"Properties", last),
                Some(Object::Dict(_)) => return Ok(None),
                _ => return Err(Unreadable),
            }
        }
        b"scn" | b"SCN" => {
            if !operands.iter().all(|o| matches!(o, Object::Int(_) | Object::Real(_) | Object::Name(_))) {
                return Err(Unreadable);
            }
            return Ok(operands.iter().rposition(|o| matches!(o, Object::Name(_))).map(|i| (&b"Pattern"[..], i)));
        }
        _ => return Ok(None),
    };
    match at {
        Some(i) if name_at(Some(i)) => Ok(Some((cat, i))),
        _ => Err(Unreadable),
    }
}

/// Whether `op` draws a Form XObject (`Do`), Type 3 font (`Tf`) or tiling pattern (`scn`/`SCN`) from
/// `resources` that has no `/Resources` of its own, and so inherits the ones it is drawn with. A
/// form or pattern whose content names no resource inherits nothing (a filled shape, say), so it
/// needs no renaming; its content is read within `budget`. A Type 3 font's glyphs are not read,
/// and always count.
fn inherits_resources(doc: &Document, resources: &Dict, op: &Op, budget: &mut DecodeBudget) -> bool {
    let Ok(Some((cat @ (b"XObject" | b"Font" | b"Pattern"), i))) = resource_operand(op) else { return false };
    let Some(Object::Name(name)) = op.operands.get(i) else { return false };
    let Some(entry) = resources.get(cat).map(|d| doc.resolve(d)).and_then(|d| d.as_dict().and_then(|d| d.get(name).cloned())) else {
        return false;
    };
    let entry = doc.resolve(&entry);
    let (dict, stream) = match &*entry {
        Object::Stream(s) => (&s.dict, Some(s)),
        Object::Dict(d) => (d, None),
        _ => return false,
    };
    let drawn = match cat {
        b"XObject" => is_name(doc, dict, b"Subtype", b"Form"),
        b"Font" => is_name(doc, dict, b"Subtype", b"Type3"),
        // A shading pattern has no content of its own.
        _ => int_of(doc, dict, b"PatternType") == Some(1),
    };
    drawn && dict.get(b"Resources").is_none() && (cat == b"Font" || stream.is_none_or(|s| names_resources(doc, s, budget)))
}

/// Whether content stream `s` names a resource (or can't be read within `budget` to tell): device
/// colour spaces are not resources; an inline image counts, as its colour space may be one.
fn names_resources(doc: &Document, s: &pdfcraft_cos::Stream, budget: &mut DecodeBudget) -> bool {
    let Ok(data) = budget.decode_strict(doc, s) else { return true };
    let parsed = parse(&data);
    parsed.skipped > 0
        || parsed.ops.iter().any(|op| match resource_operand(op) {
            Ok(None) => op.inline.is_some(),
            Ok(Some((b"ColorSpace", i))) => {
                !matches!(op.operands.get(i), Some(Object::Name(n)) if [&b"DeviceGray"[..], b"DeviceRGB", b"DeviceCMYK", b"Pattern"].contains(&n.as_slice()))
            }
            _ => true,
        })
}

/// Whether `ops` (drawn with `resources`) blend with something other than Normal, which counts
/// only in a group (`walk.group`): a `gs` they use, or one used by a form or pattern they draw, at
/// any depth. A shading pattern blends through its own `/ExtGState`, a tiling pattern through its
/// content. A nested form or pattern that can't be read, is nested too deep, draws itself or is
/// past the reads or the budget allowed counts as blending, so the caller refuses rather than
/// guesses. An operator the renderer skips ([`Unreadable`]) draws nothing and is skipped. Sets
/// `walk.text` when `ops` show text, or a Form XObject they draw does; text in a tiling pattern's
/// cell is not the page's text and is not counted.
fn blends_inside(doc: &Document, resources: &Dict, ops: &[Op], depth: usize, walk: &mut BlendWalk<'_>) -> bool {
    let category = |cat: &[u8]| resources.get(cat).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
    let (states, xobjects, patterns) = (category(b"ExtGState"), category(b"XObject"), category(b"Pattern"));
    let group = walk.group;
    let state_blends = |g: Option<&Object>| group && g.map(|g| doc.resolve(g)).and_then(|g| g.as_dict().cloned()).is_some_and(|gs| blends(doc, &gs));
    if ops.iter().any(shows_text) {
        walk.text = true;
    }
    for op in ops {
        let Ok(Some((cat, i))) = resource_operand(op) else { continue };
        let Some(name) = op.name(i) else { continue };
        let o = match cat {
            b"ExtGState" if state_blends(states.get(name)) => return true,
            b"XObject" => xobjects.get(name),
            b"Pattern" => patterns.get(name),
            _ => continue,
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
        let Ok(data) = walk.budget.decode_strict(doc, nested) else { return true };
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

/// Whether a group blending in `cs` composites as the page does: in the page group's own space
/// when it names one, else in DeviceRGB, the space viewers composite a page in on screen.
fn same_blend_space(doc: &Document, cs: &Object, page_space: Option<&Object>) -> bool {
    let cs = doc.resolve(cs);
    match page_space {
        Some(page) => *cs == *page,
        None => cs.as_name() == Some(b"DeviceRGB"),
    }
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

/// The state at op `at` of the page's streams parsed as one: whether the graphics state is
/// translucent, blended or soft-masked there, following `q`/`Q` and `gs` (each `gs` changes only
/// the parameters it sets), which a transparency group composites as a whole; and whether a path
/// or a text object is open there.
fn state_at(doc: &Document, resources: &Dict, ops: &[Op], at: usize) -> (bool, bool) {
    let states = resources.get(b"ExtGState").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
    // Fill alpha below one, stroke alpha below one, a soft mask, a blend mode other than Normal.
    let mut stack: Vec<[bool; 4]> = Vec::new();
    let mut now = [false; 4];
    let (mut path, mut text) = (false, false);
    for op in ops.iter().take(at) {
        match op.op.as_slice() {
            b"m" | b"l" | b"c" | b"v" | b"y" | b"re" | b"h" | b"W" | b"W*" => path = true,
            b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => path = false,
            b"BT" => text = true,
            b"ET" => text = false,
            b"q" => stack.push(now),
            // A Q with nothing to restore is ignored, as renderers ignore it.
            b"Q" => now = stack.pop().unwrap_or(now),
            b"gs" => {
                let Ok(Some((_, i))) = resource_operand(op) else { continue };
                let Some(gs) = op.name(i).and_then(|n| states.get(n)).map(|g| doc.resolve(g)).and_then(|g| g.as_dict().cloned()) else {
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
    (now.iter().any(|v| *v), path || text)
}
