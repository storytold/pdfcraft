//! Unlink unused Type 3 font glyph programs and prune exclusive graphical resources (F-011).

use std::collections::{HashMap, HashSet};

use pdfcraft_content::Pieces;
use pdfcraft_cos::{CosError, Dict, Document, ObjRef, Object};

use crate::{RedactError, page_streams};

const MAX_DEPTH: usize = 64;
const MAX_HOLDERS: usize = 1_000_000;

fn res_dict(doc: &Document, res: &Dict, key: &[u8]) -> Dict {
    res.get(key).and_then(|o| doc.resolve(o).as_dict().cloned()).unwrap_or_default()
}

/// Identifies a font in the document. Only fonts that are indirect objects are cleaned: a Type 3
/// font held directly in a resource dictionary fails closed (see [`unlink_type3`]), because its
/// glyph programs could not be told apart or rewritten reliably.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum FontKey {
    Ref(ObjRef),
}

/// Build the mapping from character code (0..=255) to glyph name in a Type 3 font.
/// Fails closed if the encoding cannot be safely resolved or parsed.
pub(crate) fn type3_code_to_glyph(doc: &Document, font_dict: &Dict) -> Result<[Option<Vec<u8>>; 256], CosError> {
    let Some(enc_obj) = font_dict.get(b"Encoding") else {
        return Err(CosError::Syntax { offset: 0, detail: "redaction could not establish safe Type 3 font cleanup: missing Encoding".into() });
    };
    let resolved_enc = doc.resolve(enc_obj);

    let mut differences = HashMap::new();
    let base_name: Option<Vec<u8>>;

    match &*resolved_enc {
        Object::Dict(enc_dict) => {
            if let Some(diffs) = enc_dict.get(b"Differences").and_then(|d| doc.resolve(d).as_array().cloned()) {
                let mut cur_code = 0i64;
                for item in &diffs {
                    match &*doc.resolve(item) {
                        Object::Int(n) => cur_code = *n,
                        Object::Name(name) => {
                            if let Ok(c) = u8::try_from(cur_code) {
                                differences.insert(c, name.clone());
                            }
                            cur_code = cur_code.saturating_add(1);
                        }
                        _ => {}
                    }
                }
            }
            if let Some(base) = enc_dict.name(b"BaseEncoding") {
                base_name = Some(base.to_vec());
            } else {
                // ISO 32000 §9.6.6.1: default base encoding for Type 3 fonts is StandardEncoding.
                base_name = Some(b"StandardEncoding".to_vec());
            }
        }
        Object::Name(name) => {
            base_name = Some(name.clone());
        }
        _ => {
            return Err(CosError::Syntax {
                offset: 0,
                detail: "redaction could not establish safe Type 3 font cleanup: invalid Encoding object".into(),
            });
        }
    }

    let base_table: Option<&'static [Option<&'static [u8]>; 256]> = match base_name.as_deref() {
        Some(b"StandardEncoding") => Some(&crate::type3_tables::STANDARD),
        Some(b"WinAnsiEncoding") => Some(&crate::type3_tables::WIN_ANSI),
        Some(b"MacRomanEncoding") => Some(&crate::type3_tables::MAC_ROMAN),
        Some(b"MacExpertEncoding") => Some(&crate::type3_tables::MAC_EXPERT),
        Some(other) => {
            return Err(CosError::Syntax {
                offset: 0,
                detail: format!(
                    "redaction could not establish safe Type 3 font cleanup: unsupported BaseEncoding: {}",
                    String::from_utf8_lossy(other)
                ),
            });
        }
        None => None,
    };

    let mut map: [Option<Vec<u8>>; 256] = std::array::from_fn(|_| None);
    for (code, entry) in map.iter_mut().enumerate() {
        let c = code as u8;
        if let Some(diff_glyph) = differences.get(&c) {
            *entry = Some(diff_glyph.clone());
        } else if let Some(table) = base_table
            && let Some(base_glyph) = table[code]
        {
            *entry = Some(base_glyph.to_vec());
        }
    }

    Ok(map)
}

fn resolve_font_in_res(doc: &Document, res: &Dict, font_name: &[u8]) -> Option<(FontKey, Dict)> {
    let fonts = res_dict(doc, res, b"Font");
    let font_obj = fonts.get(font_name)?;
    // A direct Type 3 font is refused during discovery, so none reaches the scan.
    let font_ref = font_obj.as_ref()?;
    let dict = doc.resolve(font_obj).as_dict()?.clone();
    (dict.name(b"Subtype") == Some(b"Type3")).then_some((FontKey::Ref(font_ref), dict))
}

fn resolve_extgstate_font(doc: &Document, res: &Dict, gs_name: &[u8]) -> Option<(FontKey, Dict)> {
    let extg = res_dict(doc, res, b"ExtGState");
    let state = doc.resolve(extg.get(gs_name)?).as_dict().cloned()?;
    let font_entry = state.get(b"Font").and_then(|f| doc.resolve(f).as_array().cloned())?;
    let font_obj = font_entry.first()?;
    let font_ref = font_obj.as_ref()?;
    let dict = doc.resolve(font_obj).as_dict()?.clone();
    (dict.name(b"Subtype") == Some(b"Type3")).then_some((FontKey::Ref(font_ref), dict))
}

#[derive(Default)]
struct UsedInGlyphs {
    xobjects: HashSet<Vec<u8>>,
    colorspaces: HashSet<Vec<u8>>,
    patterns: HashSet<Vec<u8>>,
    shadings: HashSet<Vec<u8>>,
    extgstates: HashSet<Vec<u8>>,
    fonts: HashSet<Vec<u8>>,
    properties: HashSet<Vec<u8>>,
}

fn collect_used_resources_in_stream(data: &[u8], used: &mut UsedInGlyphs) -> Result<(), CosError> {
    let parsed = pdfcraft_content::parse(data);
    if parsed.skipped > 0 {
        return Err(CosError::Syntax { offset: 0, detail: "redaction could not fully inspect glyph stream syntax".into() });
    }
    for op in parsed.ops {
        match op.op.as_slice() {
            b"Do" => {
                if let Some(n) = op.last_name() {
                    used.xobjects.insert(n.to_vec());
                }
            }
            b"cs" | b"CS" => {
                if let Some(n) = op.name(0) {
                    used.colorspaces.insert(n.to_vec());
                }
            }
            b"scn" | b"SCN" => {
                if let Some(n) = op.last_name() {
                    used.patterns.insert(n.to_vec());
                }
            }
            b"sh" => {
                if let Some(n) = op.last_name() {
                    used.shadings.insert(n.to_vec());
                }
            }
            b"gs" => {
                if let Some(n) = op.name(0) {
                    used.extgstates.insert(n.to_vec());
                }
            }
            b"Tf" => {
                if let Some(n) = op.name(0) {
                    used.fonts.insert(n.to_vec());
                }
            }
            b"BDC" | b"DP" => {
                if let Some(n) = op.name(1) {
                    used.properties.insert(n.to_vec());
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Removes the entries of `font_res`'s `category` sub-dictionary that `used` does not name,
/// dropping the sub-dictionary when nothing is left. Whether anything changed.
fn prune_category(doc: &Document, font_res: &mut Dict, category: &[u8], used: &HashSet<Vec<u8>>) -> bool {
    let Some(sub_obj) = font_res.get(category) else { return false };
    let mut sub = doc.resolve(sub_obj).as_dict().cloned().unwrap_or_default();
    let unused: Vec<Vec<u8>> = sub.iter().map(|(n, _)| n.to_vec()).filter(|n| !used.contains(n)).collect();
    if unused.is_empty() {
        return false;
    }
    for name in &unused {
        sub.remove(name);
    }
    if sub.is_empty() {
        font_res.remove(category);
    } else {
        font_res.set(category.to_vec(), Object::Dict(sub));
    }
    true
}

fn prune_type3_font_resources(doc: &Document, font_res: &mut Dict, used: &UsedInGlyphs) -> bool {
    let categories: [(&[u8], &HashSet<Vec<u8>>); 7] = [
        (b"XObject", &used.xobjects),
        (b"ColorSpace", &used.colorspaces),
        (b"Pattern", &used.patterns),
        (b"Shading", &used.shadings),
        (b"ExtGState", &used.extgstates),
        (b"Font", &used.fonts),
        (b"Properties", &used.properties),
    ];
    // Every category is pruned, even after an earlier one changed.
    categories.into_iter().fold(false, |changed, (category, names)| prune_category(doc, font_res, category, names) | changed)
}

struct ScanContext<'a> {
    doc: &'a Document,
    font_maps: &'a HashMap<FontKey, [Option<Vec<u8>>; 256]>,
    used_glyphs: &'a mut HashMap<FontKey, HashSet<Vec<u8>>>,
    /// A form scanned with its inherited font and the resources it draws through: the same form
    /// drawn in another context is scanned again.
    seen_forms: &'a mut HashSet<(ObjRef, Option<FontKey>, ObjRef)>,
}

fn scan_content_stream(
    ctx: &mut ScanContext<'_>,
    content: &[Vec<u8>],
    holder_ref: ObjRef,
    res: &Dict,
    initial_font: Option<FontKey>,
    depth: usize,
) -> Result<(), CosError> {
    if depth > MAX_DEPTH {
        return Err(CosError::Syntax { offset: 0, detail: "redaction exceeded max depth inspecting Type 3 font content".into() });
    }
    let joined = Pieces::join(content);
    let parsed = joined.parse();
    if parsed.skipped > 0 {
        return Err(CosError::Syntax { offset: 0, detail: "redaction could not fully inspect content stream syntax".into() });
    }

    let mut stack: Vec<Option<FontKey>> = Vec::new();
    let mut current_font = initial_font;

    for op in parsed.ops {
        match op.op.as_slice() {
            b"q" => stack.push(current_font),
            b"Q" => {
                if let Some(saved) = stack.pop() {
                    current_font = saved;
                }
            }
            b"Do" => {
                if let Some(name) = op.last_name() {
                    scan_xobject(ctx, res, holder_ref, name, current_font, depth)?;
                }
            }
            _ => apply_text_op(ctx, res, &op, &mut current_font),
        }
    }
    Ok(())
}

/// Apply an operator that changes or uses the current font: `Tf`, `gs`, and the text-showing
/// operators, which record the glyphs they draw.
fn apply_text_op(ctx: &mut ScanContext<'_>, res: &Dict, op: &pdfcraft_content::Op, current_font: &mut Option<FontKey>) {
    match op.op.as_slice() {
        b"Tf" => {
            if let Some(name) = op.name(0) {
                *current_font = resolve_font_in_res(ctx.doc, res, name).map(|(k, _)| k);
            }
        }
        b"gs" => {
            if let Some(name) = op.name(0)
                && let Some((k, _)) = resolve_extgstate_font(ctx.doc, res, name)
            {
                *current_font = Some(k);
            }
        }
        b"Tj" | b"'" | b"\"" => {
            if let Some(s) = op.operands.last().and_then(Object::as_string) {
                mark_shown(ctx, *current_font, &s.bytes);
            }
        }
        b"TJ" => {
            if let Some(arr) = op.operands.last().and_then(Object::as_array) {
                for item in arr {
                    if let Some(s) = item.as_string() {
                        mark_shown(ctx, *current_font, &s.bytes);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Record every glyph of `font` that the shown string `bytes` uses.
fn mark_shown(ctx: &mut ScanContext<'_>, font: Option<FontKey>, bytes: &[u8]) {
    let Some(fk) = font else { return };
    let Some(map) = ctx.font_maps.get(&fk) else { return };
    for b in bytes {
        if let Some(g) = &map[*b as usize] {
            ctx.used_glyphs.entry(fk).or_default().insert(g.clone());
        }
    }
}

/// Scan the Form XObject `name` drawn by a `Do` operator, once per (form, font, drawing holder).
fn scan_xobject(
    ctx: &mut ScanContext<'_>,
    res: &Dict,
    holder_ref: ObjRef,
    name: &[u8],
    current_font: Option<FontKey>,
    depth: usize,
) -> Result<(), CosError> {
    let xo = res_dict(ctx.doc, res, b"XObject");
    let Some(r) = xo.get(name).and_then(Object::as_ref) else { return Ok(()) };
    let obj = ctx.doc.get(r);
    let Object::Stream(s) = &*obj else { return Ok(()) };
    if s.dict.name(b"Subtype") != Some(b"Form") {
        return Ok(());
    }
    let form_own_res = s.dict.get(b"Resources").and_then(|ro| ctx.doc.resolve(ro).as_dict().cloned());
    // A form with resources of its own draws through them; one without draws through its caller's.
    let through = if form_own_res.is_some() { r } else { holder_ref };
    if !ctx.seen_forms.insert((r, current_font, through)) {
        return Ok(());
    }
    let data = s.decoded()?;
    match form_own_res {
        Some(form_res) => scan_content_stream(ctx, std::slice::from_ref(&data), r, &form_res, current_font, depth + 1),
        None => scan_content_stream(ctx, std::slice::from_ref(&data), holder_ref, res, current_font, depth + 1),
    }
}

/// The font object an ExtGState's `/Font` entry names (its first element), if any.
fn gs_font_object(doc: &Document, state_obj: &Object) -> Option<Object> {
    let state = doc.resolve(state_obj).as_dict()?.clone();
    let font_entry = doc.resolve(state.get(b"Font")?).as_array()?.clone();
    font_entry.into_iter().next()
}

/// Prune Type 3 fonts from a rewritten page or form's `/Resources` dictionary if the rewritten
/// content no longer draws any text using that font name.
pub(crate) fn prune_type3_from_resources(doc: &Document, res: &mut Dict, content: &[Vec<u8>]) -> Result<(), CosError> {
    let mut fonts = res_dict(doc, res, b"Font");
    if fonts.is_empty() {
        return Ok(());
    }

    // The operators of forms that draw through these resources count too: dropping a font a child
    // form still shows would leave its text without one.
    let ops = crate::interp::resource_ops(doc, res, content)?;

    let mut drawn_font_names = HashSet::new();
    for op in &ops {
        if op.is("Tf") {
            if let Some(name) = op.name(0) {
                drawn_font_names.insert(name.to_vec());
            }
        } else if op.is("gs")
            && let Some(name) = op.name(0)
        {
            let extg = res_dict(doc, res, b"ExtGState");
            if let Some(font_obj) = extg.get(name).and_then(|state_obj| gs_font_object(doc, state_obj)) {
                drawn_font_names.extend(
                    fonts
                        .iter()
                        .filter(|(_, fn_obj)| fn_obj.as_ref().is_some() && fn_obj.as_ref() == font_obj.as_ref())
                        .map(|(fn_name, _)| fn_name.to_vec()),
                );
            }
        }
    }

    let mut changed = false;
    let font_names: Vec<Vec<u8>> = fonts.iter().map(|(n, _)| n.to_vec()).collect();
    for name in font_names {
        if let Some(font_obj) = fonts.get(&name) {
            let resolved = doc.resolve(font_obj);
            if let Some(d) = resolved.as_dict()
                && d.name(b"Subtype") == Some(b"Type3")
                && !drawn_font_names.contains(&name)
            {
                fonts.remove(&name);
                changed = true;
            }
        }
    }

    if changed {
        if fonts.is_empty() {
            res.remove(b"Font");
        } else {
            res.set(b"Font".to_vec(), Object::Dict(fonts));
        }
    }

    Ok(())
}

/// Unlink unused Type 3 glyph programs from `CharProcs`, prune their exclusive resources from the
/// font's `/Resources`, and remove completely unused Type 3 fonts across all reachable resource
/// holders in the document. Fails closed if safe cleanup cannot be established.
pub(crate) fn unlink_type3(doc: &mut Document) -> Result<(), RedactError> {
    let Some(top) = doc.root().and_then(|root| doc.get(root).as_dict().and_then(|d| d.reference(b"Pages"))) else {
        return Ok(());
    };

    // Find all page-tree nodes with /Resources of their own, and the pages that draw through them.
    let mut holders: Vec<(ObjRef, Vec<ObjRef>)> = Vec::new();
    let mut seen = HashSet::new();
    let mut stack: Vec<(ObjRef, Option<usize>)> = vec![(top, None)];
    while let Some((node, mut holder)) = stack.pop() {
        if !seen.insert(node) || seen.len() > MAX_HOLDERS {
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

    let form_holders = reachable_form_holders(doc, &holders)?;

    // Discover all Type 3 fonts in holders and forms.
    let mut type3_fonts: HashMap<FontKey, Dict> = HashMap::new();
    for (node, _) in &holders {
        if let Some(res) = doc.get(*node).as_dict().and_then(|d| d.get(b"Resources")).and_then(|r| doc.resolve(r).as_dict().cloned()) {
            collect_type3_fonts(doc, &res, &mut type3_fonts)?;
        }
    }
    for (_, form_res) in &form_holders {
        collect_type3_fonts(doc, form_res, &mut type3_fonts)?;
    }

    if type3_fonts.is_empty() {
        return Ok(());
    }

    // Build code-to-glyph mappings for each Type 3 font. Fails closed on error.
    let mut font_maps: HashMap<FontKey, [Option<Vec<u8>>; 256]> = HashMap::new();
    for (key, font_dict) in &type3_fonts {
        let map = type3_code_to_glyph(doc, font_dict)?;
        font_maps.insert(*key, map);
    }

    // Scan all page contents and reachable forms to discover surviving glyph usage.
    let page_list = pdfcraft_model::pages(doc);
    let mut used_glyphs: HashMap<FontKey, HashSet<Vec<u8>>> = HashMap::new();
    let mut seen_drawn_forms = HashSet::new();

    let mut ctx = ScanContext { doc, font_maps: &font_maps, used_glyphs: &mut used_glyphs, seen_forms: &mut seen_drawn_forms };

    for (pi, p) in page_list.iter().enumerate() {
        let (_, content) = page_streams(ctx.doc, &p.dict, pi)?;
        let page_res = p
            .dict
            .get(b"Resources")
            .and_then(|r| ctx.doc.resolve(r).as_dict().cloned())
            .or_else(|| {
                holders.iter().find(|(_, users)| users.contains(&p.obj)).and_then(|(h, _)| {
                    ctx.doc.get(*h).as_dict().and_then(|d| d.get(b"Resources")).and_then(|ro| ctx.doc.resolve(ro).as_dict().cloned())
                })
            })
            .unwrap_or_default();

        scan_content_stream(&mut ctx, &content, p.obj, &page_res, None, 0)?;
    }

    // Also scan any surviving form that wasn't reached by page streams directly.
    for (form_ref, form_res) in &form_holders {
        if !ctx.seen_forms.iter().any(|(form, _, _)| form == form_ref) {
            let Object::Stream(s) = &*ctx.doc.get(*form_ref) else { continue };
            let data = s.decoded()?;
            scan_content_stream(&mut ctx, std::slice::from_ref(&data), *form_ref, form_res, None, 0)?;
        }
    }

    // For each Type 3 font: unlink unused glyphs from CharProcs and prune exclusive resources.
    for (font_key, font_dict) in &type3_fonts {
        let surviving = used_glyphs.get(font_key).cloned().unwrap_or_default();
        clean_type3_font(doc, &holders, &form_holders, font_key, font_dict, &surviving)?;
    }

    Ok(())
}

/// Every reachable Form XObject with resources of its own, with those resources, found by
/// walking the resources of the holders and of the forms they draw.
fn reachable_form_holders(doc: &Document, holders: &[(ObjRef, Vec<ObjRef>)]) -> Result<Vec<(ObjRef, Dict)>, CosError> {
    let mut form_holders: Vec<(ObjRef, Dict)> = Vec::new();
    let mut pending_res: Vec<Dict> = holders
        .iter()
        .filter_map(|(r, _)| doc.get(*r).as_dict().and_then(|d| d.get(b"Resources")).and_then(|ro| doc.resolve(ro).as_dict().cloned()))
        .collect();
    let mut seen_forms = HashSet::new();
    while let Some(res) = pending_res.pop() {
        for (_, value) in res_dict(doc, &res, b"XObject").iter() {
            let Some(reference) = value.as_ref() else { continue };
            if !seen_forms.insert(reference) {
                continue;
            }
            if seen_forms.len() > MAX_HOLDERS {
                return Err(CosError::Syntax { offset: 0, detail: "redaction exceeded max depth inspecting form resources".into() });
            }
            let object = doc.get(reference);
            let Object::Stream(form) = &*object else { continue };
            if form.dict.name(b"Subtype") != Some(b"Form") {
                continue;
            }
            if let Some(r) = form.dict.get(b"Resources").and_then(|ro| doc.resolve(ro).as_dict().cloned()) {
                form_holders.push((reference, r.clone()));
                pending_res.push(r);
            }
        }
    }
    Ok(form_holders)
}

/// Record every Type 3 font named by a resource dictionary's `/Font` and `/ExtGState` entries.
/// A Type 3 font held directly in a resource dictionary can neither be named reliably nor have its
/// glyph programs rewritten, so redaction fails closed rather than leaving its glyphs unchecked.
fn collect_type3_fonts(doc: &Document, res: &Dict, t3_fonts: &mut HashMap<FontKey, Dict>) -> Result<(), CosError> {
    let direct = || CosError::Syntax {
        offset: 0,
        detail: "redaction could not establish safe Type 3 font cleanup: a Type 3 font is not an indirect object".into(),
    };
    let mut consider = |font_obj: &Object| -> Result<(), CosError> {
        if let Some(dict) = doc.resolve(font_obj).as_dict().cloned()
            && dict.name(b"Subtype") == Some(b"Type3")
        {
            let font_ref = font_obj.as_ref().ok_or_else(direct)?;
            t3_fonts.entry(FontKey::Ref(font_ref)).or_insert(dict);
        }
        Ok(())
    };
    for (_, font_obj) in res_dict(doc, res, b"Font").iter() {
        consider(font_obj)?;
    }
    for (_, state_obj) in res_dict(doc, res, b"ExtGState").iter() {
        if let Some(font_obj) = gs_font_object(doc, state_obj) {
            consider(&font_obj)?;
        }
    }
    Ok(())
}

/// Unlink the glyph programs of one Type 3 font that no longer draw, then either remove the font
/// (when no glyph survives) or prune the resources only its surviving glyphs used.
fn clean_type3_font(
    doc: &mut Document,
    holders: &[(ObjRef, Vec<ObjRef>)],
    form_holders: &[(ObjRef, Dict)],
    font_key: &FontKey,
    font_dict: &Dict,
    surviving: &HashSet<Vec<u8>>,
) -> Result<(), RedactError> {
    let Some(char_procs_val) = font_dict.get(b"CharProcs") else {
        return Err(CosError::Syntax { offset: 0, detail: "redaction could not establish safe Type 3 font cleanup: missing CharProcs".into() }.into());
    };
    let mut char_procs = doc.resolve(char_procs_val).as_dict().cloned().ok_or_else(|| CosError::Syntax {
        offset: 0,
        detail: "redaction could not establish safe Type 3 font cleanup: CharProcs is not a dict".into(),
    })?;

    let mut procs_changed = false;
    let glyph_names: Vec<Vec<u8>> = char_procs.iter().map(|(n, _)| n.to_vec()).collect();
    for g in glyph_names {
        if !surviving.contains(&g) {
            char_procs.remove(&g);
            procs_changed = true;
        }
    }

    let FontKey::Ref(fr) = font_key;
    if char_procs.is_empty() {
        // The font has no surviving glyphs: clear its own glyph programs and resources, then
        // remove font from /Font across all holders.
        doc.update_dict(*fr, |d| {
            d.remove(b"CharProcs");
            d.remove(b"Resources");
        })?;
        return remove_font_from_holders(doc, holders, form_holders, font_key);
    }

    // A CharProcs object can be shared with another Type 3 font, so never rewrite it in place:
    // give this font its own filtered dictionary instead.
    if procs_changed {
        doc.update_dict(*fr, |d| {
            d.set(b"CharProcs".to_vec(), Object::Dict(char_procs.clone()));
        })?;
    }

    // Clean exclusive resources from the Type 3 font's /Resources.
    let Some(f_res_val) = font_dict.get(b"Resources") else { return Ok(()) };
    let mut f_res = doc.resolve(f_res_val).as_dict().cloned().unwrap_or_default();

    let mut used_in_glyphs = UsedInGlyphs::default();
    for (_, proc_val) in char_procs.iter() {
        let resolved = doc.resolve(proc_val);
        let Object::Stream(s) = &*resolved else {
            return Err(CosError::Syntax {
                offset: 0,
                detail: "redaction could not establish safe Type 3 font cleanup: CharProc is not a stream".into(),
            }
            .into());
        };
        let data = s.decoded()?;
        collect_used_resources_in_stream(&data, &mut used_in_glyphs)?;
    }

    if prune_type3_font_resources(doc, &mut f_res, &used_in_glyphs) {
        // The pruned dictionary goes into the font object: a font's /Resources object is
        // often the page's own, and rewriting that would strip the page of its resources.
        doc.update_dict(*fr, |d| {
            if f_res.is_empty() {
                d.remove(b"Resources");
            } else {
                d.set(b"Resources".to_vec(), Object::Dict(f_res.clone()));
            }
        })?;
    }
    Ok(())
}

fn remove_font_from_holders(
    doc: &mut Document,
    holders: &[(ObjRef, Vec<ObjRef>)],
    form_holders: &[(ObjRef, Dict)],
    target_key: &FontKey,
) -> Result<(), RedactError> {
    let clean_res = |doc: &Document, res: &mut Dict| -> bool {
        let mut fonts = res_dict(doc, res, b"Font");
        let mut changed = false;
        let names: Vec<Vec<u8>> = fonts.iter().map(|(n, _)| n.to_vec()).collect();
        for n in names {
            if let Some(val) = fonts.get(&n)
                && val.as_ref().map(FontKey::Ref) == Some(*target_key)
            {
                fonts.remove(&n);
                changed = true;
            }
        }
        if changed {
            if fonts.is_empty() {
                res.remove(b"Font");
            } else {
                res.set(b"Font".to_vec(), Object::Dict(fonts));
            }
        }
        changed
    };

    for (node, _) in holders {
        if let Some(mut res) = doc.get(*node).as_dict().and_then(|d| d.get(b"Resources")).and_then(|r| doc.resolve(r).as_dict().cloned())
            && clean_res(doc, &mut res)
        {
            doc.update_dict(*node, |d| {
                d.set(b"Resources".to_vec(), Object::Dict(res));
            })?;
        }
    }

    for (form_ref, _) in form_holders {
        let obj = doc.get(*form_ref);
        if let Object::Stream(s) = &*obj
            && let Some(mut res) = s.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned())
            && clean_res(doc, &mut res)
        {
            doc.update_dict(*form_ref, |d| {
                d.set(b"Resources".to_vec(), Object::Dict(res));
            })?;
        }
    }

    Ok(())
}
