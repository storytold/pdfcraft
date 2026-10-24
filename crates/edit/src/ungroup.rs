//! Edit a PDF ▸ Ungroup: grouped Form artwork drawn by one `Do` in the page's own content becomes
//! ordinary page content, so its text, images and nested groups can be selected and edited one
//! by one. CAD and print exports (WSCAD, some plotter drivers) put a whole page in one Form
//! XObject, which otherwise leaves Edit a PDF with a single page-sized group.
//!
//! Painting a Form XObject is: save the graphics state, concatenate its `/Matrix`, clip to its
//! `/BBox`, run its content with its `/Resources`, restore (ISO 32000-2 §8.10.1). Ungrouping
//! writes exactly that in place of the `Do`: `q`, the matrix, the `/BBox` clip, the content and
//! `Q`. The content keeps its bytes; only resource names that clash with the page's are renamed.
//! The form object itself is left untouched, so other pages and annotations that draw it are not
//! affected.
//!
//! A form is refused, with a reason, where inlining could change how the page looks or what it
//! means: a transparency group that does more than wrap opaque content, a form that is part of the
//! structure tree (`/StructParent`, `/StructParents`), different default colour spaces, content
//! the parser had to skip, or a graphics state the content would leave unbalanced.

use std::collections::BTreeMap;

use pdfcraft_content::{Matrix, Op, Pieces, parse, write_op};
use pdfcraft_cos::{Dict, Document, Object};

use crate::EditError;

/// The largest decoded form content that is inlined.
const MAX_FORM_BYTES: usize = 64 * 1024 * 1024;
/// The most operators in one ungrouped form.
const MAX_FORM_OPS: usize = 2_000_000;
/// The deepest `q`/`BT`/marked-content nesting inside an ungrouped form.
const MAX_NESTING: usize = 1024;
/// How many fresh names are tried for one clashing resource.
const MAX_RENAME_TRIES: usize = 10_000;

/// The resource categories whose names a content stream uses (§7.8.3). `ProcSet` holds no names.
const CATEGORIES: [&[u8]; 7] = [b"ExtGState", b"ColorSpace", b"Pattern", b"Shading", b"XObject", b"Font", b"Properties"];

/// Colour space names that are operators' own, not resource names (§8.6.8, §8.9.7 for the
/// abbreviations an inline image may use).
const DEVICE_SPACES: [&[u8]; 8] = [b"DeviceGray", b"DeviceRGB", b"DeviceCMYK", b"Pattern", b"G", b"RGB", b"CMYK", b"I"];

fn invalid(message: &str) -> EditError {
    EditError::Invalid(message.into())
}

fn resolved_dict(doc: &Document, o: Option<&Object>) -> Option<Dict> {
    o.map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned())
}

/// Whether two resource values name the same thing: the same object, or equal direct values.
fn same_resource(doc: &Document, a: &Object, b: &Object) -> bool {
    match (a, b) {
        (Object::Ref(x), Object::Ref(y)) => x == y,
        (Object::Ref(_), _) | (_, Object::Ref(_)) => *doc.resolve(a) == *doc.resolve(b),
        _ => a == b,
    }
}

/// A transparency-related setting in an ExtGState that makes a group's compositing differ from
/// painting its content directly: blend modes other than Normal, soft masks, alpha below 1 and
/// alpha-is-shape (§11.3–§11.6).
fn transparent_state(doc: &Document, gs: &Dict) -> bool {
    let alpha = |key: &[u8]| gs.get(key).map(|v| doc.resolve(v)).and_then(|v| v.as_f64()).is_some_and(|a| a.is_nan() || a < 1.0);
    let blend = gs.get(b"BM").map(|v| doc.resolve(v)).is_some_and(|bm| match &*bm {
        Object::Name(n) => !matches!(n.as_slice(), b"Normal" | b"Compatible"),
        Object::Array(a) => a.first().and_then(Object::as_name).is_none_or(|n| !matches!(n, b"Normal" | b"Compatible")),
        _ => true,
    });
    let mask = gs.get(b"SMask").map(|v| doc.resolve(v)).is_some_and(|m| m.as_name() != Some(b"None"));
    let ais = gs.get(b"AIS").map(|v| doc.resolve(v)).is_some_and(|v| matches!(&*v, Object::Bool(true)));
    alpha(b"CA") || alpha(b"ca") || blend || mask || ais
}

/// Whether a group's own content composites only opaque paint: no transparent graphics state and
/// no soft-masked image among its direct resources. Nested forms stay groups of their own when
/// ungrouped, so their content does not matter here.
fn opaque_resources(doc: &Document, resources: &Dict) -> bool {
    let states = resolved_dict(doc, resources.get(b"ExtGState")).unwrap_or_default();
    if states.iter().any(|(_, gs)| resolved_dict(doc, Some(gs)).is_none_or(|gs| transparent_state(doc, &gs))) {
        return false;
    }
    let xobjects = resolved_dict(doc, resources.get(b"XObject")).unwrap_or_default();
    !xobjects.iter().any(|(_, x)| {
        resolved_dict(doc, Some(x))
            .is_some_and(|d| d.name(b"Subtype") == Some(b"Image") && (d.contains(b"SMask") || d.int(b"SMaskInData").is_some_and(|v| v != 0)))
    })
}

/// Whether a Form among `resources`' XObjects has no `/Resources` of its own, and so names its
/// resources from the content that draws it (§7.8.3). Once that content is inlined, such a Form
/// reads the page's resources instead: a renamed name would then mean the page's resource, not
/// the group's. Forms it draws in turn look their names up in the same dictionary.
fn inherits_resources(doc: &Document, resources: &Dict) -> bool {
    let xobjects = resolved_dict(doc, resources.get(b"XObject")).unwrap_or_default();
    xobjects.iter().any(|(_, x)| {
        resolved_dict(doc, Some(x)).is_some_and(|d| d.name(b"Subtype") == Some(b"Form") && resolved_dict(doc, d.get(b"Resources")).is_none())
    })
}

/// Whether the graphics state in effect at operator `target` of `ops` has transparency set by
/// `gs`, which would apply to a group as a whole but to each object once ungrouped.
fn transparent_at(doc: &Document, ops: &[Op], target: usize, states: &Dict) -> bool {
    let mut current = false;
    let mut stack = Vec::new();
    for op in ops.iter().take(target) {
        match op.op.as_slice() {
            b"q" => stack.push(current),
            b"Q" => current = stack.pop().unwrap_or(current),
            b"gs" => {
                if let Some(gs) = op.name(0).and_then(|n| resolved_dict(doc, states.get(n))) {
                    // A later state can set alpha back to 1 or the blend mode back to Normal, but
                    // a soft mask or transparency set earlier may still apply; stay cautious.
                    current |= transparent_state(doc, &gs);
                }
            }
            _ => {}
        }
    }
    current
}

/// Whether the group around a form's content changes how it looks once painted directly:
/// a knockout group, transparency inside it or applied to it, or a blending colour space other
/// than the page's.
fn group_matters(doc: &Document, group: &Dict, form_resources: &Dict, page: &Dict, page_ops: &[Op], target: usize, page_states: &Dict) -> bool {
    if group.name(b"S") != Some(b"Transparency") {
        return false;
    }
    let knockout = group.get(b"K").map(|v| doc.resolve(v)).is_some_and(|v| matches!(&*v, Object::Bool(true)));
    let space = group.get(b"CS").map(|v| doc.resolve(v));
    let page_space = resolved_dict(doc, page.get(b"Group")).and_then(|g| g.get(b"CS").map(|v| doc.resolve(v)));
    let same_space = match (&space, &page_space) {
        (None, _) => true,
        (Some(a), Some(b)) => a == b,
        (Some(a), None) => a.as_name() == Some(b"DeviceRGB"),
    };
    knockout || !same_space || !opaque_resources(doc, form_resources) || transparent_at(doc, page_ops, target, page_states)
}

/// Resource names the inlined content uses that need a new name in the page's resources.
type Renames = BTreeMap<(&'static [u8], Vec<u8>), Vec<u8>>;

/// Merge `form` resources into `page` resources. Names the page already uses for the same thing
/// are shared; names it uses for something else get a fresh name, returned in the map.
fn merge_resources(doc: &Document, page: &mut Dict, form: &Dict) -> Result<Renames, EditError> {
    let mut renames = Renames::new();
    for category in CATEGORIES {
        let Some(theirs) = resolved_dict(doc, form.get(category)) else { continue };
        if theirs.is_empty() {
            continue;
        }
        let mut ours = resolved_dict(doc, page.get(category)).unwrap_or_default();
        for (name, value) in theirs.iter() {
            match ours.get(name) {
                Some(existing) if same_resource(doc, existing, value) => {}
                None => ours.set(name.clone(), value.clone()),
                Some(_) => {
                    let fresh = (1..=MAX_RENAME_TRIES)
                        .map(|k| {
                            let mut n = name.clone();
                            n.extend_from_slice(format!("_PC{k}").as_bytes());
                            n
                        })
                        .find(|n| !ours.contains(n) && !theirs.contains(n))
                        .ok_or_else(|| invalid("the group's resource names cannot be merged into the page"))?;
                    ours.set(fresh.clone(), value.clone());
                    renames.insert((category, name.clone()), fresh);
                }
            }
        }
        page.set(category.to_vec(), Object::Dict(ours));
    }
    Ok(renames)
}

/// Whether `op` names a resource in `category` as operand `index` (§8.4.5, Table 57 and the
/// operator tables of §8–§9, §14.6).
fn resource_operand(op: &Op) -> Option<(&'static [u8], usize)> {
    let last = op.operands.len().checked_sub(1);
    match op.op.as_slice() {
        b"Tf" => Some((b"Font", 0)),
        b"Do" => Some((b"XObject", 0)),
        b"gs" => Some((b"ExtGState", 0)),
        b"sh" => Some((b"Shading", 0)),
        b"cs" | b"CS" => op.name(0).filter(|n| !DEVICE_SPACES.contains(n)).map(|_| (&b"ColorSpace"[..], 0)),
        b"scn" | b"SCN" => last.filter(|i| op.name(*i).is_some()).map(|i| (&b"Pattern"[..], i)),
        b"BDC" | b"DP" => op.name(1).map(|_| (&b"Properties"[..], 1)),
        _ => None,
    }
}

/// The inline image parameters that name a colour space resource (§8.9.7).
fn inline_space(dict: &Dict) -> Option<(&'static [u8], Vec<u8>)> {
    [&b"CS"[..], b"ColorSpace"].into_iter().find_map(|key| {
        let n = dict.get(key)?.as_name()?;
        (!DEVICE_SPACES.contains(&n)).then(|| (key, n.to_vec()))
    })
}

/// Checks the form content can stand inside the page's own: nothing skipped, no `Q` without its
/// `q`, no text object or marked content left open in a way that would swallow the page's
/// following operators. Returns what must be closed at the end (`ET`, `EMC`s, `Q`s), in order.
fn closing_ops(ops: &[Op]) -> Result<Vec<&'static str>, EditError> {
    if ops.len() > MAX_FORM_OPS {
        return Err(invalid("the group has too many operations to ungroup"));
    }
    let mut saves = 0usize;
    let mut marked = 0usize;
    let mut text = false;
    for op in ops {
        match op.op.as_slice() {
            b"q" => saves = saves.saturating_add(1),
            b"Q" => {
                saves = saves
                    .checked_sub(1)
                    .ok_or_else(|| invalid("the group restores more graphics states than it saves; it cannot be ungrouped exactly"))?
            }
            b"BT" if text => return Err(invalid("the group starts a text object inside another; it cannot be ungrouped exactly")),
            b"BT" => text = true,
            b"ET" if !text => return Err(invalid("the group ends a text object it did not start; it cannot be ungrouped exactly")),
            b"ET" => text = false,
            b"BMC" | b"BDC" => marked = marked.saturating_add(1),
            b"EMC" => {
                marked =
                    marked.checked_sub(1).ok_or_else(|| invalid("the group ends marked content it did not start; it cannot be ungrouped exactly"))?
            }
            b"Do" if text => return Err(invalid("the group draws an object inside a text object; it cannot be ungrouped exactly")),
            _ => {}
        }
        if saves > MAX_NESTING || marked > MAX_NESTING {
            return Err(invalid("the group's content is nested too deeply to ungroup"));
        }
    }
    // A form's end closes all of these implicitly; inside the page they must be closed explicitly
    // so the page's own operators after the `Do` keep their state.
    let mut out = Vec::new();
    if text {
        out.push("ET");
    }
    out.extend(std::iter::repeat_n("EMC", marked));
    out.extend(std::iter::repeat_n("Q", saves));
    Ok(out)
}

/// The form content with clashing resource names changed, other bytes as they were.
fn renamed_content(data: &[u8], ops: &[Op], renames: &Renames) -> Vec<u8> {
    if renames.is_empty() {
        return data.to_vec();
    }
    let rename = |category: &'static [u8], name: &[u8]| renames.get(&(category, name.to_vec())).cloned();
    let mut edits = Vec::new();
    for op in ops {
        let mut changed = op.clone();
        let mut any = false;
        if let Some((category, index)) = resource_operand(op)
            && let Some(fresh) = op.name(index).and_then(|n| rename(category, n))
            && let Some(slot) = changed.operands.get_mut(index)
        {
            *slot = Object::Name(fresh);
            any = true;
        }
        if let Some((dict, _)) = changed.inline.as_mut()
            && let Some((key, name)) = inline_space(dict)
            && let Some(fresh) = rename(b"ColorSpace", &name)
        {
            dict.set(key.to_vec(), Object::Name(fresh));
            any = true;
        }
        if any {
            let mut bytes = Vec::new();
            write_op(&changed, &mut bytes);
            while bytes.last().is_some_and(u8::is_ascii_whitespace) {
                bytes.pop();
            }
            edits.push((op.span.clone(), bytes));
        }
    }
    Pieces::join(&[data]).splice(edits).into_iter().next().unwrap_or_default()
}

fn numbers(doc: &Document, o: Option<&Object>) -> Option<Vec<f64>> {
    let o = doc.resolve(o?);
    o.as_array()?.iter().map(|x| doc.resolve(x).as_f64()).collect()
}

fn number(v: f64) -> Vec<u8> {
    let mut out = Vec::new();
    pdfcraft_cos::serialize(&Object::Real(v), &mut out);
    out
}

/// Ungroup Form artwork `index` (from [`crate::page_images`]) on `page`: its content replaces
/// the `Do` that draws it. Refuses images, forms drawn inside other forms and forms whose
/// content would look or mean something else on the page.
pub fn ungroup_form(doc: &mut Document, page: usize, index: usize) -> Result<(), EditError> {
    let images = crate::images::page_images(doc, page)?;
    let image = images.get(index).ok_or_else(|| EditError::Invalid(format!("page {} has no object {}", page + 1, index + 1)))?;
    if !image.is_form {
        return Err(invalid("only grouped Form artwork can be ungrouped; this is an image"));
    }
    let p = pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))?;
    let mut resources = resolved_dict(doc, p.dict.get(b"Resources")).unwrap_or_default();
    let xobjects = resolved_dict(doc, resources.get(b"XObject")).unwrap_or_default();
    let form_obj = xobjects.get(image.name.as_bytes()).cloned().ok_or_else(|| invalid("the group is no longer in the page's resources"))?;
    let resolved = doc.resolve(&form_obj);
    let Object::Stream(form) = &*resolved else {
        return Err(invalid("the group is not a stream"));
    };
    if form.dict.name(b"Subtype") != Some(b"Form") {
        return Err(invalid("only grouped Form artwork can be ungrouped"));
    }
    if form.dict.contains(b"StructParent") || form.dict.contains(b"StructParents") {
        return Err(invalid("the group is part of the document's accessibility tags; ungrouping it would break them"));
    }
    let bbox = numbers(doc, form.dict.get(b"BBox"))
        .and_then(|b| <[f64; 4]>::try_from(b).ok())
        .filter(|b| b.iter().all(|v| v.is_finite() && v.abs() <= 1e12))
        .ok_or_else(|| invalid("the group has no valid bounding box"))?;
    let matrix = match form.dict.get(b"Matrix") {
        None => Matrix::IDENTITY,
        Some(m) => numbers(doc, Some(m))
            .and_then(|v| <[f64; 6]>::try_from(v).ok())
            .filter(|v| v.iter().all(|x| x.is_finite() && x.abs() <= 1e12))
            .map(Matrix)
            .ok_or_else(|| invalid("the group has an invalid matrix"))?,
    };
    // A form without resources uses those of the content that draws it (PDF 1.1 files).
    let form_resources = resolved_dict(doc, form.dict.get(b"Resources")).unwrap_or_else(|| resources.clone());
    let defaults = |res: &Dict| {
        let spaces = resolved_dict(doc, res.get(b"ColorSpace")).unwrap_or_default();
        [&b"DefaultGray"[..], b"DefaultRGB", b"DefaultCMYK"].map(|k| spaces.get(k).map(|v| doc.resolve(v)))
    };
    if defaults(&form_resources) != defaults(&resources) {
        return Err(invalid("the group uses other default colour spaces than the page; ungrouping it would change its colours"));
    }
    let data = form.decoded_within(MAX_FORM_BYTES).map_err(|_| invalid("the group's content cannot be decoded within its size limit"))?;
    let parsed = parse(&data);
    if parsed.skipped > 0 {
        return Err(invalid("the group's content has damaged operators; ungrouping it could change the page"));
    }
    let closing = closing_ops(&parsed.ops)?;

    // The `Do` among the page's streams joined, as the image edits index it.
    let p_streams = crate::text::content_streams(doc, &p.dict)?;
    let pieces = Pieces::join(&p_streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>());
    let page_ops = pieces.parse().ops;
    let target = image.op;
    if !page_ops.get(target).is_some_and(|op| op.is("Do")) {
        return Err(invalid("the group's drawing operation could not be located"));
    }
    // A `Do` inside a text object is invalid (§9.4) and some viewers still draw it; inlining a
    // form's own text objects there would nest them.
    let in_text = page_ops.iter().take(target).fold(false, |text, op| {
        if op.is("BT") {
            true
        } else if op.is("ET") {
            false
        } else {
            text
        }
    });
    if in_text {
        return Err(invalid("the group is drawn inside a text object; it cannot be ungrouped exactly"));
    }
    let page_states = resolved_dict(doc, resources.get(b"ExtGState")).unwrap_or_default();
    if let Some(group) = resolved_dict(doc, form.dict.get(b"Group"))
        && group_matters(doc, &group, &form_resources, &p.dict, &page_ops, target, &page_states)
    {
        return Err(invalid("the group is a transparency group whose blending would change if its content were painted directly; it stays grouped"));
    }

    let renames = merge_resources(doc, &mut resources, &form_resources)?;
    if !renames.is_empty() && inherits_resources(doc, &form_resources) {
        return Err(invalid(
            "the group holds a nested group that uses the group's resources, and one of their names means something else on the page; it stays grouped",
        ));
    }
    let mut properties = None;
    if let Some(oc) = form.dict.get(b"OC") {
        // Optional content on the form: the inlined content is marked with the same membership.
        let mut props = resolved_dict(doc, resources.get(b"Properties")).unwrap_or_default();
        let existing = props.iter().find(|(_, v)| same_resource(doc, v, oc)).map(|(n, _)| n.clone());
        let name = match existing {
            Some(n) => n,
            None => {
                let fresh = (1..=MAX_RENAME_TRIES)
                    .map(|k| format!("PCOC{k}").into_bytes())
                    .find(|n| !props.contains(n))
                    .ok_or_else(|| invalid("the group's optional content cannot be named on the page"))?;
                props.set(fresh.clone(), oc.clone());
                fresh
            }
        };
        resources.set(b"Properties".to_vec(), Object::Dict(props));
        properties = Some(name);
    }

    let content = renamed_content(&data, &parsed.ops, &renames);
    let mut inline = b"q\n".to_vec();
    if matrix != Matrix::IDENTITY {
        for v in matrix.0 {
            inline.extend(number(v));
            inline.push(b' ');
        }
        inline.extend_from_slice(b"cm\n");
    }
    let [x0, y0, x1, y1] = [bbox[0].min(bbox[2]), bbox[1].min(bbox[3]), bbox[0].max(bbox[2]), bbox[1].max(bbox[3])];
    for v in [x0, y0, x1 - x0, y1 - y0] {
        inline.extend(number(v));
        inline.push(b' ');
    }
    inline.extend_from_slice(b"re W n\n");
    if let Some(name) = &properties {
        let mut bdc = Vec::new();
        write_op(&Op::new("BDC", vec![Object::name("OC"), Object::Name(name.clone())]), &mut bdc);
        inline.extend(bdc);
        inline.push(b'\n');
    }
    inline.extend(content);
    inline.push(b'\n');
    for op in closing {
        inline.extend_from_slice(op.as_bytes());
        inline.push(b'\n');
    }
    if properties.is_some() {
        inline.extend_from_slice(b"EMC\n");
    }
    inline.push(b'Q');

    let changed =
        pieces.splice([(page_ops.get(target).map(|o| o.span.clone()).ok_or_else(|| invalid("the group's drawing operation is missing"))?, inline)]);
    let contents = crate::text::rewritten_contents(doc, &p_streams, changed);
    doc.update_dict(p.obj, |d| {
        d.set(b"Contents".to_vec(), contents);
        d.set(b"Resources".to_vec(), Object::Dict(resources));
    })?;
    Ok(())
}
