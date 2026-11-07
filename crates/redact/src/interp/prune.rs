use super::{
    CosError, Dict, Document, HashMap, HashSet, MAX_DEPTH, Matrix, Metrics, ObjRef, Object, Op, Pieces, Rc, RedactError, Scope, is_vertical, num,
    overlaps, page_streams, res_dict,
};

fn resolve(doc: &Document, obj: &Object) -> Object {
    let mut cur = obj.clone();
    for _ in 0..16 {
        let Object::Ref(r) = cur else { break };
        cur = (*doc.get(r)).clone();
    }
    cur
}

fn target_ref(doc: &Document, obj: &Object) -> Option<ObjRef> {
    let mut cur = obj.as_ref()?;
    for _ in 0..16 {
        match &*doc.get(cur) {
            Object::Ref(next) => cur = *next,
            _ => return Some(cur),
        }
    }
    None
}

fn dict_name(doc: &Document, d: &Dict, key: &[u8]) -> Option<Vec<u8>> {
    d.get(key).and_then(|v| resolve(doc, v).as_name().map(<[u8]>::to_vec))
}

fn matches_retired(doc: &Document, value: &Object, retired: &[Object]) -> bool {
    retired.contains(value) || target_ref(doc, value).is_some_and(|r| retired.iter().any(|item| target_ref(doc, item) == Some(r)))
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
    let still_drawn = |n: &[u8]| ops.iter().any(|op| op.is("Do") && op.last_name() == Some(n));
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
        let Some(n) = op.last_name() else { continue };
        if n == name {
            return Some(true);
        }
        let Some(r) = xo.get(n).and_then(|v| target_ref(doc, v)) else { continue };
        let obj = doc.get(r);
        let Object::Stream(s) = &*obj else { continue };
        if dict_name(doc, &s.dict, b"Subtype").as_deref() != Some(b"Form")
            || s.dict.get(b"Resources").is_some_and(|r| resolve(doc, r).as_dict().is_some())
            || !seen.insert(r)
        {
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
    crate::type3::unlink_type3(doc)?;
    if doomed.is_empty() && retired_properties.is_empty() {
        return Ok(());
    }
    let doomed: HashSet<ObjRef> = doomed.iter().copied().collect();
    let Some(top) =
        doc.root().and_then(|root| resolve(doc, &Object::Ref(root)).as_dict().and_then(|d| d.get(b"Pages")).and_then(|p| target_ref(doc, p)))
    else {
        return Ok(());
    };
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
        let kind = dict_name(doc, d, b"Type");
        let is_pages = kind.as_deref() == Some(b"Pages") || (d.contains(b"Kids") && kind.as_deref() != Some(b"Page"));
        if is_pages {
            let kids = d.get(b"Kids").and_then(|k| resolve(doc, k).as_array().cloned()).unwrap_or_default();
            stack.extend(kids.iter().filter_map(|k| target_ref(doc, k)).map(|k| (k, holder)));
        } else if let Some(h) = holder.and_then(|i| holders.get_mut(i)) {
            h.1.push(node);
        }
    }
    let index: HashMap<ObjRef, usize> = pdfcraft_model::pages(doc).iter().enumerate().map(|(i, p)| (p.obj, i)).collect();
    let holder_nodes: Vec<ObjRef> = holders.iter().map(|(node, _)| *node).collect();
    for (node, users) in holders {
        let Some(mut res) = doc.get(node).as_dict().and_then(|d| d.get(b"Resources")).and_then(|r| resolve(doc, r).as_dict().cloned()) else {
            continue;
        };
        let mut xo = res_dict(doc, &res, b"XObject");
        let names: Vec<Vec<u8>> =
            xo.iter().filter(|(_, v)| target_ref(doc, v).is_some_and(|r| doomed.contains(&r))).map(|(n, _)| n.to_vec()).collect();
        let properties = res_dict(doc, &res, b"Properties");
        let property_names: Vec<Vec<u8>> =
            properties.iter().filter(|(_, v)| matches_retired(doc, v, retired_properties)).map(|(n, _)| n.to_vec()).collect();
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
        .filter_map(|r| doc.get(*r).as_dict().and_then(|d| d.get(b"Resources")).and_then(|r| resolve(doc, r).as_dict().cloned()))
        .collect();
    let mut seen = HashSet::new();
    while let Some(resources) = pending.pop() {
        for (_, value) in res_dict(doc, &resources, b"XObject").iter() {
            let Some(reference) = target_ref(doc, value) else { continue };
            if !seen.insert(reference) {
                continue;
            }
            if seen.len() > 1_000_000 {
                return Err(CosError::Syntax { offset: 0, detail: "redaction could not fully inspect shared form resources".into() });
            }
            let object = doc.get(reference);
            let Object::Stream(form) = &*object else { continue };
            if dict_name(doc, &form.dict, b"Subtype").as_deref() != Some(b"Form") {
                continue;
            }
            let Some(mut res) = form.dict.get(b"Resources").and_then(|r| resolve(doc, r).as_dict().cloned()) else { continue };
            let names: Vec<Vec<u8>> = res_dict(doc, &res, b"Properties")
                .iter()
                .filter(|(_, value)| matches_retired(doc, value, retired))
                .map(|(name, _)| name.to_vec())
                .collect();
            let mut xo = res_dict(doc, &res, b"XObject");
            let doomed_names: Vec<Vec<u8>> =
                xo.iter().filter(|(_, v)| target_ref(doc, v).is_some_and(|r| doomed.contains(&r))).map(|(n, _)| n.to_vec()).collect();
            if names.is_empty() && doomed_names.is_empty() {
                pending.push(res);
                continue;
            }
            let data = form.decoded()?;
            let mut pruned = prune_doomed_xobjects(doc, std::slice::from_ref(&data), &mut xo, &doomed_names)?;
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

fn prune_doomed_xobjects(doc: &Document, content: &[Vec<u8>], xo: &mut Dict, doomed_names: &[Vec<u8>]) -> Result<bool, CosError> {
    let mut pruned = false;
    for name in doomed_names {
        match content_draws(doc, content, xo, name, 0, &mut HashSet::new()) {
            Some(true) => {}
            Some(false) => pruned |= xo.remove(name).is_some(),
            None => return Err(CosError::Syntax { offset: 0, detail: "redaction could not fully inspect shared form resources".into() }),
        }
    }
    Ok(pruned)
}

/// The clip that hides everything inside each region: one even-odd clip per region (a big
/// rectangle with the region cut out), in the current user space. Returns `None` when the CTM
/// can't be inverted (nothing is painted then anyway).
pub(super) fn clip_out(rects: &[[f64; 4]], ctm: &Matrix, around: [f64; 4]) -> Option<Vec<Op>> {
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

pub(crate) fn resource_ops(doc: &Document, res: &Dict, content: &[Vec<u8>]) -> Result<Vec<Op>, CosError> {
    let parsed = Pieces::join(content).parse();
    if parsed.skipped > 0 || parsed.ops.len() > 2_000_000 {
        return Err(CosError::Syntax { offset: 0, detail: "redaction could not fully inspect property resources".into() });
    }
    let mut ops = parsed.ops;
    let xobjects = res_dict(doc, res, b"XObject");
    let mut seen = HashSet::new();
    let mut cursor = 0;
    while let Some(op) = ops.get(cursor) {
        if op.is("Do")
            && let Some(r) = op.last_name().and_then(|n| xobjects.get(n)).and_then(|v| target_ref(doc, v))
            && seen.insert(r)
            && let Object::Stream(s) = &*doc.get(r)
            && dict_name(doc, &s.dict, b"Subtype").as_deref() == Some(b"Form")
            && s.dict.get(b"Resources").is_none_or(|r| resolve(doc, r).as_dict().is_none())
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

pub(super) fn prune_form_resources(doc: &Document, res: &mut Dict, content: &[Vec<u8>]) -> Result<(), CosError> {
    let ops = resource_ops(doc, res, content)?;
    let mut xo = res_dict(doc, res, b"XObject");
    let names: Vec<Vec<u8>> = xo.iter().map(|(n, _)| n.to_vec()).collect();
    for name in names {
        if !ops.iter().any(|op| op.is("Do") && op.last_name() == Some(&name)) {
            xo.remove(&name);
        }
    }
    res.set(b"XObject".to_vec(), Object::Dict(xo));
    Ok(())
}

/// The font, writing direction and size an ExtGState's `/Font [font size]` entry sets, as `Tf`
/// would (ISO 32000-2 §8.4.5); `None` when `name` sets no font. The metrics are cached with the
/// fonts `Tf` names, under a key no resource name can take (a name cannot hold a null byte).
pub(super) fn extgstate_font(
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

/// Drop every `/Pattern` entry this scope saw selected but no surviving painting operator drew
/// with. The `scn` that selected it stays in the content, so the page names a pattern its
/// resources no longer hold; nothing surviving paints with that colour, and
/// `a_page_whose_pruned_pattern_is_still_selected_renders` pins that the page still renders and
/// that operators after the stale selector still draw. A pattern's cell is a content stream of its own which
/// this pass does not enter, so a pattern whose only painting was redacted would otherwise keep
/// its drawing -- text included -- in the saved file (#3647). A pattern some surviving paint
/// still uses, or one the scope never saw, is left alone.
/// `/Pattern` may be an indirect object like any other value, so it is resolved: reading the
/// reference itself as a dictionary found none and pruned nothing, leaving the cell in the file.
/// What is written back is a dictionary of this page's own, so pruning never reaches through a
/// `/Pattern` dictionary another page shares. The pattern objects dropped are added to `pruned`,
/// for [`unlink_pruned_patterns`].
pub(crate) fn prune_patterns(doc: &Document, res: &mut Dict, seen: &[Vec<u8>], painted: &[Vec<u8>], pruned: &mut Vec<ObjRef>) -> usize {
    let Some(mut pat) = res.get(b"Pattern").map(|p| doc.resolve(p)).and_then(|p| p.as_dict().cloned()) else { return 0 };
    let mut dropped = 0;
    for n in seen {
        if !painted.contains(n)
            && let Some(value) = pat.get(n)
        {
            // Every object along the reference chain: a name can lead to a reference-valued
            // object whose value is the cell, and the cell is what must not stay reachable.
            let mut next = value.as_ref();
            for _ in 0..MAX_DEPTH {
                let Some(reference) = next else { break };
                pruned.push(reference);
                next = match &*doc.get(reference) {
                    Object::Ref(r) => Some(*r),
                    _ => None,
                };
            }
            pat.remove(n);
            dropped += 1;
        }
    }
    if dropped > 0 {
        if pat.is_empty() {
            res.remove(b"Pattern");
        } else {
            res.set(b"Pattern".to_vec(), Object::Dict(pat));
        }
    }
    dropped
}

/// Unlink the patterns a page's rewrite pruned (`pruned`) from page-tree nodes whose
/// `/Resources` no page inherits any more: a full save keeps whatever Root still reaches, so a
/// `/Pages` node above the page that still names the cell would keep it in the file. That happens
/// when the page's nearest holder was retired but a holder further up, which it shadowed, still
/// names the pattern. A holder some page still inherits from is left alone; whatever stays
/// reachable is checked by `pattern_guard`, which refuses a retained cell it cannot prove safe.
/// When any node's `/Parent` disagrees with where the `/Kids` tree puts it (or the root has
/// one), which holder a page inherits from is ambiguous and a node may be reached only through
/// `/Parent`, so nothing is unlinked and the guard, which follows `/Parent` too, decides.
pub(crate) fn unlink_pruned_patterns(doc: &mut Document, pruned: &[ObjRef]) -> Result<(), RedactError> {
    if pruned.is_empty() {
        return Ok(());
    }
    let pruned: HashSet<ObjRef> = pruned.iter().copied().collect();
    let Some(top) = doc.root().and_then(|root| doc.get(root).as_dict().and_then(|d| d.reference(b"Pages"))) else { return Ok(()) };
    // Every node with a /Resources of its own, and whether any page inherits from it (it is the
    // nearest holder above a page, or the page itself). Cycles are cut as in `unlink_doomed`.
    let mut holders: Vec<(ObjRef, bool)> = Vec::new();
    let mut seen = HashSet::new();
    let mut consistent = true;
    let mut stack: Vec<(ObjRef, Option<ObjRef>, Option<usize>)> = vec![(top, None, None)];
    seen.insert(top);
    while let Some((node, parent, mut holder)) = stack.pop() {
        let obj = doc.get(node);
        let Some(d) = obj.as_dict() else { continue };
        // Every node's /Parent must be the node /Kids reached it from, and the root has none.
        // A node reached only through a /Parent link is then impossible here; when the links
        // disagree, the guard inspects every node they reach instead. A /Parent that is not a
        // reference (a direct dictionary, written inline by a full save) never agrees.
        let agrees = match d.get(b"Parent") {
            None => parent.is_none(),
            Some(Object::Ref(r)) => Some(*r) == parent,
            Some(_) => false,
        };
        if !agrees {
            consistent = false;
            break;
        }
        if d.contains(b"Resources") {
            holders.push((node, false));
            holder = Some(holders.len() - 1);
        }
        let is_pages = d.name(b"Type") == Some(b"Pages") || (d.contains(b"Kids") && d.name(b"Type") != Some(b"Page"));
        if is_pages {
            // Borrowed, not copied: a /Kids array can be shared by many hostile nodes.
            let kids_object = d.get(b"Kids").map(|k| doc.resolve(k));
            for kid in kids_object.as_deref().and_then(Object::as_array).into_iter().flatten().filter_map(Object::as_ref) {
                // A node reached twice (a shared /Kids entry or a cycle) has a /Parent that agrees
                // with at most one of the nodes above it.
                if !seen.insert(kid) || seen.len() > 1_000_000 {
                    consistent = false;
                    break;
                }
                stack.push((kid, Some(node), holder));
            }
            if !consistent {
                break;
            }
        } else if let Some(h) = holder.and_then(|i| holders.get_mut(i)) {
            h.1 = true;
        }
    }
    if !consistent {
        return Ok(());
    }
    for (node, inherited) in holders {
        if inherited {
            continue;
        }
        let Some(mut res) = doc.get(node).as_dict().and_then(|d| d.get(b"Resources")).and_then(|r| doc.resolve(r).as_dict().cloned()) else {
            continue;
        };
        let mut pat = res_dict(doc, &res, b"Pattern");
        let names: Vec<Vec<u8>> = pat.iter().filter(|(_, v)| v.as_ref().is_some_and(|r| pruned.contains(&r))).map(|(n, _)| n.to_vec()).collect();
        if names.is_empty() {
            continue;
        }
        for n in &names {
            pat.remove(n);
        }
        if pat.is_empty() {
            res.remove(b"Pattern");
        } else {
            res.set(b"Pattern".to_vec(), Object::Dict(pat));
        }
        // A private copy on this node: the dictionary object may be shared with other nodes.
        doc.update_dict(node, |d| d.set(b"Resources".to_vec(), Object::Dict(res)))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_cos::Stream;

    fn test_doc() -> Document {
        Document::new_empty()
    }

    #[test]
    fn unlink_doomed_prunes_chained_xobject_refs_and_indirect_types() {
        let mut doc = test_doc();
        let doomed_img = doc.add(Object::Stream(Stream::from_raw(Dict::new(), Vec::new())));
        let doomed_hop = doc.add(Object::Ref(doomed_img));

        let retired_prop = doc.add(Object::Dict(Dict::new()));
        let retired_hop = doc.add(Object::Ref(retired_prop));

        let form_subtype_ref = doc.add(Object::Name(b"Form".to_vec()));
        let mut form_xo = Dict::new();
        form_xo.set(b"ImDoomed".to_vec(), Object::Ref(doomed_hop));
        let mut form_props = Dict::new();
        form_props.set(b"PrRetired".to_vec(), Object::Ref(retired_hop));
        let mut form_res = Dict::new();
        form_res.set(b"XObject".to_vec(), Object::Dict(form_xo));
        form_res.set(b"Properties".to_vec(), Object::Dict(form_props));
        let mut form_dict = Dict::new();
        form_dict.set(b"Subtype".to_vec(), Object::Ref(form_subtype_ref));
        form_dict.set(b"Resources".to_vec(), Object::Dict(form_res));
        let form_ref = doc.add(Object::Stream(Stream::from_raw(form_dict, b"q Q".to_vec())));
        let form_hop = doc.add(Object::Ref(form_ref));

        let mut page_xo = Dict::new();
        page_xo.set(b"ImDoomed".to_vec(), Object::Ref(doomed_hop));
        page_xo.set(b"FmKeep".to_vec(), Object::Ref(form_hop));
        let mut page_props = Dict::new();
        page_props.set(b"PrRetired".to_vec(), Object::Ref(retired_hop));
        let mut page_res = Dict::new();
        page_res.set(b"XObject".to_vec(), Object::Dict(page_xo));
        page_res.set(b"Properties".to_vec(), Object::Dict(page_props));

        let page_type_ref = doc.add(Object::Name(b"Page".to_vec()));
        let contents_ref = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"/FmKeep Do".to_vec())));
        let pages_ref = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).expect("pages ref");
        let mut page_dict = Dict::new();
        page_dict.set(b"Type".to_vec(), Object::Ref(page_type_ref));
        page_dict.set(b"Parent".to_vec(), Object::Ref(pages_ref));
        page_dict.set(b"Resources".to_vec(), Object::Dict(page_res));
        page_dict.set(b"Contents".to_vec(), Object::Ref(contents_ref));
        let page_ref = doc.add(Object::Dict(page_dict));
        let page_hop = doc.add(Object::Ref(page_ref));

        let pages_type_ref = doc.add(Object::Name(b"Pages".to_vec()));
        doc.update_dict(pages_ref, |d| {
            d.set(b"Type".to_vec(), Object::Ref(pages_type_ref));
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page_hop)]));
            d.set(b"Count".to_vec(), Object::Int(1));
        })
        .expect("update pages");

        unlink_doomed(&mut doc, &[doomed_img], &[Object::Ref(retired_prop)]).expect("unlink doomed");

        let updated_page = doc.get(page_ref);
        let updated_page_res = updated_page.as_dict().and_then(|d| d.get(b"Resources")).and_then(Object::as_dict).expect("page res");
        let updated_page_xo = updated_page_res.get(b"XObject").and_then(Object::as_dict).expect("page xo");
        assert!(!updated_page_xo.contains(b"ImDoomed"), "chained ref to doomed XObject must be unlinked from page");
        assert!(updated_page_xo.contains(b"FmKeep"));
        assert!(!updated_page_res.contains(b"Properties"), "chained ref to retired property must be unlinked from page");

        let updated_form = doc.get(form_ref);
        let updated_form_res = updated_form.as_dict().and_then(|d| d.get(b"Resources")).and_then(Object::as_dict).expect("form res");
        let updated_form_xo = updated_form_res.get(b"XObject").and_then(Object::as_dict).expect("form xo");
        assert!(!updated_form_xo.contains(b"ImDoomed"), "chained ref to doomed XObject must be unlinked from surviving form");
        assert!(!updated_form_res.contains(b"Properties"), "chained ref to retired property must be unlinked from surviving form");
    }

    #[test]
    fn resource_ops_follows_inheriting_form_with_indirect_subtype_and_chained_ref() {
        let mut doc = test_doc();
        let form_subtype_ref = doc.add(Object::Name(b"Form".to_vec()));
        let mut form_dict = Dict::new();
        form_dict.set(b"Subtype".to_vec(), Object::Ref(form_subtype_ref));
        let form_ref = doc.add(Object::Stream(Stream::from_raw(form_dict, b"/OC /P1 BDC (inside) Tj EMC".to_vec())));
        let form_hop = doc.add(Object::Ref(form_ref));

        let mut xo = Dict::new();
        xo.set(b"Fm1".to_vec(), Object::Ref(form_hop));
        let mut props = Dict::new();
        props.set(b"P1".to_vec(), Object::Dict(Dict::new()));
        props.set(b"P2".to_vec(), Object::Dict(Dict::new()));
        let mut res = Dict::new();
        res.set(b"XObject".to_vec(), Object::Dict(xo));
        res.set(b"Properties".to_vec(), Object::Dict(props));

        let retired = prune_properties(&doc, &mut res, &[b"/Fm1 Do".to_vec()]).expect("prune properties");
        assert_eq!(retired.len(), 1);
        let kept_props = res.get(b"Properties").and_then(Object::as_dict).expect("kept properties");
        assert!(kept_props.contains(b"P1"), "property used inside inheriting form must be kept");
        assert!(!kept_props.contains(b"P2"), "unused property must be retired");
    }

    #[test]
    fn unlink_doomed_terminates_on_cyclic_page_tree_and_form_resources() {
        let mut doc = test_doc();
        let doomed_img = doc.add(Object::Stream(Stream::from_raw(Dict::new(), Vec::new())));
        let pages_ref = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).expect("pages ref");
        doc.update_dict(pages_ref, |d| {
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(pages_ref)]));
        })
        .expect("update cyclic kids");
        assert!(unlink_doomed(&mut doc, &[doomed_img], &[]).is_ok());
    }
}
