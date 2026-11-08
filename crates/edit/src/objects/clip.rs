//! Edit a PDF ▸ Copy, Cut and Paste of objects selected together (#5989): paragraphs, images,
//! Form artwork and added content, pasted as page content that draws the same.
//!
//! A copy is a content stream of its own that draws the objects and nothing else, in the page's
//! user space, with the resources it names. It replays every state operator of the page up to
//! the last object (transformations, clips, colours, ExtGStates, text state), so each object is
//! drawn in the state it had, and leaves out what paints anything else: other text, images,
//! shadings and paths (a path that clips is kept, without its paint). Each copied text-showing
//! operator is placed by its own text matrix, which is where its glyphs start, so the text
//! positioning between operators is left out. Marked content is left out too: a paste is
//! untagged and in no layer.
//!
//! A paste draws that stream on top of the page through Ungroup's merge ([`ungroup_ops`]): the
//! copy is read as Form artwork whose resources join the page's under names that don't clash,
//! moved by the offset, with the patterns it paints moved along. Its resources must already be
//! the target document's: the engine copies them over first when the copy came from another
//! document.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use pdfcraft_content::{Matrix, Op, serialize_ops};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use super::{EditableObject, MAX_COORDINATE, MAX_MOVE_OBJECTS, ObjectKind, ObjectTarget, checked_streams, editable_objects, valid_rect};
use crate::EditError;
use crate::ungroup::{CATEGORIES, DEFAULT_SPACES, page_blend_space, resource_operand, ungroup_ops};

/// How deep the copied graphics state may nest (`q`), as the page's own may.
const MAX_NESTING: usize = 1024;

/// Objects copied from one page: what [`paste_objects`] draws again.
#[derive(Clone, Debug, PartialEq)]
pub struct CopiedObjects {
    /// A content stream that draws the objects alone, in the source page's user space: its `q`
    /// and `Q` balance and its text objects close.
    pub content: Vec<u8>,
    /// The resources `content` names, as the source page has them: references are into the
    /// document the copy came from.
    pub resources: Dict,
    /// The union of the objects' boxes (user space of the source page).
    pub rect: [f64; 4],
    /// The objects as [`editable_objects`] listed them, in inventory order.
    pub objects: Vec<EditableObject>,
}

impl CopiedObjects {
    /// The text of the copied paragraphs and text boxes, one per line: what the system clipboard
    /// gets for other applications.
    pub fn text(&self) -> String {
        self.objects.iter().filter_map(|o| o.text.as_deref()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n")
    }
}

/// Copy `targets` (references from one [`editable_objects`] inventory of `page`). The document
/// is not changed.
pub fn copy_objects(doc: &Document, page: usize, targets: &[ObjectTarget]) -> Result<CopiedObjects, EditError> {
    if targets.is_empty() || targets.len() > MAX_MOVE_OBJECTS {
        return Err(EditError::Invalid("select between 1 and 1000 objects to copy".into()));
    }
    let mut inventory: BTreeMap<_, _> = editable_objects(doc, page)?.into_iter().map(|o| (o.target, o)).collect();
    let mut objects = Vec::with_capacity(targets.len());
    for target in targets.iter().collect::<BTreeSet<_>>() {
        objects
            .push(inventory.remove(target).ok_or_else(|| EditError::Invalid("a selected object no longer exists; select the objects again".into()))?);
    }
    if objects.len() != targets.len() {
        return Err(EditError::Invalid("an object was selected more than once".into()));
    }
    let indexes = |kind| objects.iter().filter(|o| o.target.kind == kind).map(|o| o.target.index).collect::<Vec<_>>();
    let streams = checked_streams(doc, page)?;
    let pieces = crate::text::joined(&streams);
    let ops = pieces.parse().ops;
    let mut keep = crate::text::paragraph_ops(doc, page, &indexes(ObjectKind::Text), false)?;
    keep.extend(crate::images::image_ops(doc, page, &indexes(ObjectKind::Image))?);
    // Added content owns its stream: everything that stream paints is the item.
    let added: Vec<_> = crate::list_added(doc).into_iter().filter(|a| a.page == page).collect();
    let owned = indexes(ObjectKind::Added)
        .into_iter()
        .map(|i| {
            added.get(i).map(|a| a.obj).ok_or_else(|| EditError::Invalid("a selected added item no longer exists; select the objects again".into()))
        })
        .collect::<Result<BTreeSet<ObjRef>, _>>()?;
    if !owned.is_empty() {
        for (i, op) in ops.iter().enumerate() {
            let stream = streams.get(pieces.pieces_of(op).0).and_then(|s| s.entry.as_ref());
            if paints(op) && stream.is_some_and(|r| owned.contains(&r)) {
                keep.insert(i);
            }
        }
    }
    let p = pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))?;
    let res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let shows = crate::text::show_starts(doc, &ops, &res)?;
    let drawn = replay(&ops, &keep, &shows)?;
    let rect = objects.iter().map(|o| o.rect).reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]).unwrap_or_default();
    Ok(CopiedObjects { resources: used_resources(doc, &res, &drawn), content: serialize_ops(&drawn), rect, objects })
}

/// Whether `op` paints (rather than sets state or builds a path).
fn paints(op: &Op) -> bool {
    matches!(op.op.as_slice(), b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"Tj" | b"TJ" | b"'" | b"\"" | b"Do" | b"sh" | b"BI")
}

/// The page's operators up to the last of `keep`, painting only `keep` (see the module docs).
/// `shows`: where each text-showing operator starts ([`crate::text::show_starts`]).
fn replay(ops: &[Op], keep: &BTreeSet<usize>, shows: &HashMap<usize, (Matrix, i64)>) -> Result<Vec<Op>, EditError> {
    let Some(&last) = keep.last() else { return Err(EditError::Invalid("the selected objects draw nothing to copy".into())) };
    let mut out = Vec::new();
    let (mut depth, mut in_text) = (0usize, false);
    let (mut path, mut clips) = (Vec::new(), false);
    for (i, op) in ops.iter().enumerate().take(last.saturating_add(1)) {
        let kept = keep.contains(&i);
        match op.op.as_slice() {
            b"q" => {
                if depth >= MAX_NESTING {
                    return Err(EditError::Invalid("the page graphics state is nested too deeply".into()));
                }
                depth += 1;
                out.push(op.clone());
            }
            // An unmatched `Q` restores nothing; the renderer ignores it too.
            b"Q" if depth > 0 => {
                depth -= 1;
                out.push(op.clone());
            }
            b"BT" if !in_text => {
                in_text = true;
                out.push(op.clone());
            }
            b"ET" if in_text => {
                in_text = false;
                out.push(op.clone());
            }
            b"Q" | b"BT" | b"ET" => {}
            b"m" | b"l" | b"c" | b"v" | b"y" | b"re" | b"h" => path.push(op.clone()),
            b"W" | b"W*" => {
                clips = true;
                path.push(op.clone());
            }
            b"n" | b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => {
                if kept || clips {
                    out.append(&mut path);
                    out.push(if kept { op.clone() } else { Op::new("n", Vec::new()) });
                }
                path.clear();
                clips = false;
            }
            b"Tj" | b"TJ" | b"'" | b"\"" => {
                // `"` sets the spacing for what follows, shown or not.
                if op.is("\"") {
                    for (name, value) in ["Tw", "Tc"].into_iter().zip(op.operands.iter().take(2)) {
                        if value.as_f64().is_some() {
                            out.push(Op::new(name, vec![value.clone()]));
                        }
                    }
                }
                match shows.get(&i) {
                    Some((tm, _)) if kept => {
                        out.push(Op::new("Tm", tm.0.map(Object::Real).to_vec()));
                        out.push(if op.is("TJ") { op.clone() } else { Op::new("Tj", op.operands.last().cloned().into_iter().collect()) });
                    }
                    // Its glyphs clip what follows, which leaving them out would not.
                    Some((_, mode)) if *mode >= 4 => {
                        return Err(EditError::Invalid(
                            "text used as a clipping path is drawn before the selected objects, so they can't be copied".into(),
                        ));
                    }
                    _ => {}
                }
            }
            // Each copied show sets its own text matrix; `TD` also sets the leading.
            b"TD" => {
                if let Some([_, y]) = op.nums::<2>() {
                    out.push(Op::new("TL", vec![Object::Real(-y)]));
                }
            }
            b"Td" | b"Tm" | b"T*" | b"BDC" | b"BMC" | b"EMC" | b"MP" | b"DP" => {}
            b"Do" | b"sh" | b"BI" => {
                if kept {
                    out.push(op.clone());
                }
            }
            _ => out.push(op.clone()),
        }
    }
    if in_text {
        out.push(Op::new("ET", Vec::new()));
    }
    out.extend(std::iter::repeat_with(|| Op::new("Q", Vec::new())).take(depth));
    Ok(out)
}

/// The entries of the page's resources `res` that `ops` name, and the default colour spaces,
/// which remap the device colours they paint (ISO 32000-2 §8.6.5.6).
fn used_resources(doc: &Document, res: &Dict, ops: &[Op]) -> Dict {
    let mut used: BTreeMap<&[u8], BTreeSet<&[u8]>> = BTreeMap::new();
    for op in ops {
        if let Ok(Some((cat, i))) = resource_operand(op)
            && let Some(Object::Name(name)) = op.operands.get(i)
        {
            used.entry(cat).or_default().insert(name.as_slice());
        }
    }
    let mut out = Dict::new();
    for cat in CATEGORIES {
        let Some(all) = res.get(cat).map(|d| doc.resolve(d)).and_then(|d| d.as_dict().cloned()) else { continue };
        let mut mine = Dict::new();
        for (key, value) in all.iter() {
            if used.get(cat).is_some_and(|u| u.contains(key.as_slice())) || (cat == b"ColorSpace" && DEFAULT_SPACES.contains(&key.as_slice())) {
                mine.set(key.clone(), value.clone());
            }
        }
        if !mine.is_empty() {
            out.set(cat.to_vec(), Object::Dict(mine));
        }
    }
    out
}

/// Paste `copied` on top of `page`, moved by `offset` (user space), as one change: its content
/// and the page's resources merged with its own. Its resources must be this document's.
pub fn paste_objects(doc: &mut Document, page: usize, copied: &CopiedObjects, offset: [f64; 2]) -> Result<(), EditError> {
    let r = copied.rect;
    if !offset.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE)
        || !valid_rect([r[0] + offset[0], r[1] + offset[1], r[2] + offset[0], r[3] + offset[1]])
    {
        return Err(EditError::Invalid("the paste position is out of range".into()));
    }
    let p = pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))?;
    crate::check_on_top(doc, &p)?;
    let res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut xobjects = res.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
    let name = (1..=xobjects.len().saturating_add(1))
        .map(|n| format!("PCPaste{n}").into_bytes())
        .find(|n| xobjects.get(n).is_none())
        .ok_or_else(|| EditError::Invalid("the page has too many resources".into()))?;
    // The copy as Form artwork, to draw inline as Ungroup does: its resources merge into the
    // page's, and patterns move with it. The form itself is never added to the document.
    let number = |v: f64| Object::Real(v);
    let mut form = Dict::new();
    form.set(b"Type".to_vec(), Object::name("XObject"));
    form.set(b"Subtype".to_vec(), Object::name("Form"));
    // Nothing of the copy is clipped: its glyphs and strokes may reach past its objects' boxes.
    form.set(b"BBox".to_vec(), Object::Array([-MAX_COORDINATE, -MAX_COORDINATE, MAX_COORDINATE, MAX_COORDINATE].map(number).to_vec()));
    form.set(b"Matrix".to_vec(), Object::Array(Matrix::translate(offset[0], offset[1]).0.map(number).to_vec()));
    form.set(b"Resources".to_vec(), Object::Dict(copied.resources.clone()));
    xobjects.set(name.clone(), Object::Stream(Stream::from_raw(form, copied.content.clone())));
    let mut with_copy = res;
    with_copy.set(b"XObject".to_vec(), Object::Dict(xobjects));
    let space = page_blend_space(doc, &p.dict);
    let (ops, mut merged, patterns) = ungroup_ops(
        doc,
        &with_copy,
        &name,
        false,
        space.as_ref(),
        Matrix::translate(offset[0], offset[1]),
        "the copied objects can't be pasted here",
    )?;
    let mut work = doc.clone();
    if let Some(mut x) = merged.get(b"XObject").and_then(Object::as_dict).cloned() {
        x.remove(&name);
        merged.set(b"XObject".to_vec(), Object::Dict(x));
    }
    if !patterns.is_empty() {
        let mut table = merged.get(b"Pattern").map(|x| work.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
        for (key, pattern) in patterns {
            table.set(key, Object::Ref(work.add(pattern)));
        }
        merged.set(b"Pattern".to_vec(), Object::Dict(table));
    }
    work.update_dict(p.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(merged)))?;
    crate::place_tagged(&mut work, &p, "Pasted", serialize_ops(&ops), false)?;
    // Keep the page's objects selectable together, as a move does.
    editable_objects(&work, page)?;
    *doc = work;
    Ok(())
}

#[cfg(test)]
#[path = "clip_tests.rs"]
mod tests;
