//! The opacity of one image or placed form: an `ExtGState` with `CA` and `ca` set by `gs` in a
//! `q … Q` of its own around that `Do` (ISO 32000-2 §8.4.5), and the opacity each image is drawn at.

use pdfcraft_content::Op;
use pdfcraft_cos::{Dict, Document, Object};

use crate::EditError;
use crate::images::{PageImage, page_images};
use crate::text::{content_streams, joined};

/// Names of the graphics states this module writes start with this.
const PREFIX: &str = "PCAlpha";
/// How many names past the first are tried when one is taken by a different state.
const MAX_NAME_TRIES: u32 = 64;
/// The deepest `q` nesting followed when reading opacity.
const MAX_DEPTH: usize = 256;
/// Opacities closer than this are the same.
const SAME: f64 = 1e-6;

/// The fill opacity (`ca`) each of [`page_images`] is drawn at, in the same order. Read from the
/// page's own `gs` operators and resources; a soft mask or a transparency group around it is
/// not counted.
pub fn image_opacities(doc: &Document, page: usize) -> Result<Vec<f64>, EditError> {
    let p = crate::images::page_of(doc, page)?;
    let ops = joined(&content_streams(doc, &p.dict)?).parse().ops;
    let states = ext_g_states(doc, &p.dict);
    let at = alpha_at(doc, &states, &ops, None);
    Ok(page_images(doc, page)?.iter().map(|i| at.get(&i.op).copied().unwrap_or(1.0)).collect())
}

/// Set image `img`'s opacity to `alpha` (0 transparent, 1 opaque). A second call replaces the
/// first rather than stacking; 1 removes it when the image is opaque without it. The value
/// already in effect is refused. A form without a transparency group fades each object it draws.
pub(crate) fn set_image_opacity(doc: &mut Document, page: usize, img: &PageImage, alpha: f64) -> Result<(), EditError> {
    if !(alpha.is_finite() && (0.0..=1.0).contains(&alpha)) {
        return Err(EditError::Invalid("the opacity must be a number from 0 to 1".into()));
    }
    let p = crate::images::page_of(doc, page)?;
    let all = content_streams(doc, &p.dict)?;
    crate::text::drawn_before_damage(&all, img.stream)?;
    let pieces = joined(&all);
    let ops = pieces.parse().ops;
    if !ops.get(img.op).is_some_and(|op| op.is("Do")) {
        return Err(EditError::Invalid("the image operation is missing".into()));
    }
    let mut res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut states = ext_g_states(doc, &p.dict);
    let ours = own_state(&ops, img.op);
    let without = alpha_at(doc, &states, &ops, ours).get(&img.op).copied().unwrap_or(1.0);
    let now = alpha_at(doc, &states, &ops, None).get(&img.op).copied().unwrap_or(1.0);
    if (alpha - now).abs() < SAME {
        return Err(EditError::Invalid(format!("the image is already at {}% opacity", (alpha * 100.0).round())));
    }
    let gs = if (alpha - 1.0).abs() < SAME && (without - 1.0).abs() < SAME {
        None
    } else {
        let name = state_name(doc, &states, alpha)?;
        let mut s = Dict::new();
        s.set(b"Type".to_vec(), Object::name("ExtGState"));
        s.set(b"CA".to_vec(), Object::Real(alpha));
        s.set(b"ca".to_vec(), Object::Real(alpha));
        states.set(name.clone().into_bytes(), Object::Dict(s));
        Some(Op::new("gs", vec![Object::name(&name)]))
    };
    let target = img.op;
    // Our scope removed whole when it holds nothing but `q gs Do Q`; anything else in it stays.
    let bare = ours.filter(|&at| gs.is_none() && at.checked_add(1) == Some(target) && ops.get(target.saturating_add(1)).is_some_and(|o| o.is("Q")));
    let changed = crate::text::splice(&pieces, &ops, |index| match (ours, &gs) {
        _ if bare.is_some_and(|at| at.checked_sub(1) == Some(index) || index == at || target.checked_add(1) == Some(index)) => (Vec::new(), false),
        (Some(at), Some(gs)) if index == at => (vec![gs.clone()], false),
        (Some(at), None) if index == at => (Vec::new(), false),
        (None, Some(gs)) if index == target => (vec![Op::new("q", vec![]), gs.clone()], true),
        (None, Some(_)) if index == target.saturating_add(1) => (vec![Op::new("Q", vec![])], true),
        _ => (Vec::new(), true),
    });
    let changed = if ours.is_none() && gs.is_some() && target.saturating_add(1) >= ops.len() { close_at_end(changed) } else { changed };
    let contents = crate::text::rewritten_contents(doc, &all, changed)?;
    res.set(b"ExtGState".to_vec(), Object::Dict(states));
    // The page gets its own copy of its resources; a dictionary other pages share stays as it is.
    doc.update_dict(p.obj, |d| {
        d.set(b"Contents".to_vec(), contents);
        d.set(b"Resources".to_vec(), Object::Dict(res));
    })?;
    Ok(())
}

/// The `Q` that closes a new `q` opened before the page's last operator, appended to the last piece.
fn close_at_end(mut pieces: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    if let Some(last) = pieces.last_mut() {
        last.extend_from_slice(b"\nQ\n");
    }
    pieces
}

/// The page's `ExtGState` resources, resolved into one dictionary (empty when there are none).
fn ext_g_states(doc: &Document, page: &Dict) -> Dict {
    page.get(b"Resources")
        .map(|r| doc.resolve(r))
        .and_then(|r| r.as_dict().and_then(|d| d.get(b"ExtGState").map(|g| doc.resolve(g))).and_then(|g| g.as_dict().cloned()))
        .unwrap_or_default()
}

/// A name for a state with this opacity: an existing one of ours with the same values, else one
/// not taken.
fn state_name(doc: &Document, states: &Dict, alpha: f64) -> Result<String, EditError> {
    let base = format!("{PREFIX}{}", (alpha * 1000.0).round() as i64);
    for k in 0..=MAX_NAME_TRIES {
        let name = if k == 0 { base.clone() } else { format!("{base}_{k}") };
        let Some(existing) = states.get(name.as_bytes()) else {
            return Ok(name);
        };
        let same = doc.resolve(existing).as_dict().is_some_and(|d| {
            let value = |key: &[u8]| d.get(key).map(|v| doc.resolve(v)).and_then(|v| v.as_f64());
            d.get(b"SMask").is_none()
                && d.get(b"BM").is_none()
                && [b"CA".as_slice(), b"ca"].iter().all(|k| value(k).is_some_and(|v| (v - alpha).abs() < SAME))
        });
        if same {
            return Ok(name);
        }
    }
    Err(EditError::Invalid("the page has too many graphics states with this name".into()))
}

/// The `gs` this module wrote for the `Do` at `target`: the operator right after the `q` of a
/// scope that holds the `Do` and paints nothing else.
fn own_state(ops: &[Op], target: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = target;
    let mut found = None;
    // Walk out from the Do: each `q` with nothing painted between it and the Do whose matching
    // `Q` also comes with nothing else painted.
    while let Some(prev) = i.checked_sub(1) {
        i = prev;
        let op = ops.get(i)?;
        if op.is("Q") {
            depth = depth.checked_add(1)?;
        } else if op.is("q") {
            if depth == 0 {
                if is_ours(ops.get(i.checked_add(1)?)?) && closes_alone(ops, i, target) {
                    found = Some(i.checked_add(1)?);
                }
            } else {
                depth -= 1;
            }
        } else if depth == 0 && paints(op) {
            break;
        }
    }
    found
}

fn is_ours(op: &Op) -> bool {
    op.is("gs") && op.operands.first().and_then(|o| o.as_name()).is_some_and(|n| n.starts_with(PREFIX.as_bytes()))
}

/// Whether the scope opened by the `q` at `open` closes with the `Do` at `target` the only paint in it.
fn closes_alone(ops: &[Op], open: usize, target: usize) -> bool {
    let mut depth = 0usize;
    for (i, op) in ops.iter().enumerate().skip(open) {
        if op.is("q") {
            depth = depth.saturating_add(1);
        } else if op.is("Q") {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return i > target;
            }
        } else if i != target && paints(op) {
            return false;
        }
    }
    false
}

fn paints(op: &Op) -> bool {
    matches!(op.op.as_slice(), b"Do" | b"Tj" | b"TJ" | b"'" | b"\"" | b"sh" | b"BI" | b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*")
}

/// The fill opacity in effect at each `Do`, by operator index, ignoring the `gs` at `skip`.
fn alpha_at(doc: &Document, states: &Dict, ops: &[Op], skip: Option<usize>) -> std::collections::BTreeMap<usize, f64> {
    let mut out = std::collections::BTreeMap::new();
    let mut stack: Vec<f64> = Vec::new();
    let mut ca = 1.0;
    for (i, op) in ops.iter().enumerate() {
        match op.op.as_slice() {
            b"q" if stack.len() < MAX_DEPTH => stack.push(ca),
            b"Q" => ca = stack.pop().unwrap_or(ca),
            b"gs" if Some(i) != skip => {
                let value = op
                    .operands
                    .first()
                    .and_then(|n| n.as_name())
                    .and_then(|n| states.get(n))
                    .map(|s| doc.resolve(s))
                    .and_then(|s| s.as_dict().and_then(|d| d.get(b"ca").map(|v| doc.resolve(v)).and_then(|v| v.as_f64())));
                if let Some(v) = value.filter(|v| v.is_finite()) {
                    ca = v.clamp(0.0, 1.0);
                }
            }
            b"Do" => {
                out.insert(i, ca);
            }
            _ => {}
        }
    }
    out
}
