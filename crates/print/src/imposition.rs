use super::error::PrintError;
use super::settings::{Content, Settings};
use super::sheets::{Sheet, layout};
use super::{DecodeBudget, Dict, Document, Matrix, ObjRef, Object, SaveOptions, Stream, write_full};

// Model-derived transforms and clipping bounds need their full PDF decimal precision.
fn geometry_number(value: f64) -> String {
    let mut bytes = Vec::new();
    pdfcraft_cos::serialize(&Object::Real(value), &mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

fn n(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// The page's content streams decoded against `budget` and joined with a space as the renderer
/// joins them; streams that don't decode are left out. `None` when the budget ran out, so part of
/// the content is missing.
pub(crate) fn decoded_contents(doc: &Document, page: &Dict, budget: &mut DecodeBudget) -> Option<Vec<u8>> {
    let Some(contents) = page.get(b"Contents") else { return Some(Vec::new()) };
    let mut out = Vec::new();
    for data in budget.decode_contents(doc, contents) {
        match data {
            Ok(data) => {
                out.extend_from_slice(&data);
                out.push(b' ');
            }
            Err(_) if budget.remaining() == 0 => return None,
            Err(_) => {}
        }
    }
    Some(out)
}

/// Reduce `doc`'s pages to what [`impose`] draws of them under `content`: the annotations that
/// don't print are dropped, and with [`Content::FormFieldsOnly`] the page content too. For
/// printing pages as pictures, where a renderer draws each page as it stands.
pub fn keep_printed(doc: &mut Document, content: Content) {
    for page in pdfcraft_model::pages(doc) {
        let annots = page.dict.get(b"Annots").and_then(|a| doc.resolve(a).as_array().cloned()).unwrap_or_default();
        let kept: Vec<Object> = annots.into_iter().filter(|a| doc.resolve(a).as_dict().is_some_and(|d| prints(doc, d, content))).collect();
        // Fails only when the page object isn't a dictionary, and such a page draws nothing.
        let _ = doc.update_dict(page.obj, |d| {
            if content == Content::FormFieldsOnly {
                d.remove(b"Contents");
            }
            if d.contains(b"Annots") {
                d.set(b"Annots".to_vec(), Object::Array(kept));
            }
        });
    }
}

/// The standard annotation types of ISO 32000-2 Table 171.
const STANDARD_ANNOTATION_TYPES: [&[u8]; 28] = [
    b"Text",
    b"Link",
    b"FreeText",
    b"Line",
    b"Square",
    b"Circle",
    b"Polygon",
    b"PolyLine",
    b"Highlight",
    b"Underline",
    b"Squiggly",
    b"StrikeOut",
    b"Caret",
    b"Stamp",
    b"Ink",
    b"Popup",
    b"FileAttachment",
    b"Sound",
    b"Movie",
    b"Screen",
    b"Widget",
    b"PrinterMark",
    b"TrapNet",
    b"Watermark",
    b"3D",
    b"Redact",
    b"Projection",
    b"RichMedia",
];

/// Does annotation `d` print under `content`?
pub(crate) fn prints(doc: &Document, d: &Dict, content: Content) -> bool {
    const INVISIBLE: i64 = 1;
    const HIDDEN: i64 = 2;
    const PRINT: i64 = 4;
    let flags = d.get(b"F").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0);
    if flags & PRINT == 0 || flags & HIDDEN != 0 {
        return false;
    }
    // `/Subtype` may be an indirect reference (ISO 32000-2 §7.3.10), so resolve it first.
    let subtype_obj = d.get(b"Subtype").map(|s| doc.resolve(s));
    let subtype = subtype_obj.as_deref().and_then(Object::as_name).unwrap_or(b"");
    match subtype {
        b"Widget" => true,
        b"Link" | b"Popup" => false,
        b"Stamp" => content != Content::Document && content != Content::FormFieldsOnly,
        // ISO 32000-2 Table 167: an Invisible annotation of a type that isn't standard (Table
        // 171) is not printed even with its Print flag set.
        _ if flags & INVISIBLE != 0 && !STANDARD_ANNOTATION_TYPES.contains(&subtype) => false,
        _ => content == Content::DocumentAndMarkups,
    }
}

/// The normal appearance stream to draw for an annotation (honouring `/AS`).
pub(crate) fn appearance(doc: &Document, d: &Dict) -> Option<ObjRef> {
    let ap = doc.resolve(d.get(b"AP")?);
    let normal = ap.as_dict()?.get(b"N")?.clone();
    match &*doc.resolve(&normal) {
        Object::Stream(_) => normal.as_ref(),
        Object::Dict(states) => {
            let state = d.name(b"AS")?;
            states.get(state)?.as_ref()
        }
        _ => None,
    }
}

/// Algorithm 8.1: the matrix placing an appearance form (BBox × Matrix) onto the annotation rect.
pub(crate) fn appearance_matrix(doc: &Document, form: &Dict, rect: [f64; 4]) -> Option<Matrix> {
    let nums =
        |k: &[u8]| -> Option<Vec<f64>> { Some(doc.resolve(form.get(k)?).as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect()) };
    let bbox = nums(b"BBox").filter(|b| b.len() == 4)?;
    let m = nums(b"Matrix").filter(|m| m.len() == 6).map(|m| Matrix([m[0], m[1], m[2], m[3], m[4], m[5]])).unwrap_or_default();
    let b = m.bbox([bbox[0], bbox[1], bbox[2], bbox[3]]);
    let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
    if bw <= 0.0 || bh <= 0.0 {
        return None;
    }
    let a = Matrix([
        (rect[2] - rect[0]) / bw,
        0.0,
        0.0,
        (rect[3] - rect[1]) / bh,
        rect[0] - b[0] * (rect[2] - rect[0]) / bw,
        rect[1] - b[1] * (rect[3] - rect[1]) / bh,
    ]);
    // The Do that draws the form applies its /Matrix itself, so only the placement goes here.
    Some(a)
}

/// A Form XObject holding page `index` as it prints (user space; BBox = crop box).
/// `pages` are the document's pages, listed once by the caller: listing them for each page
/// printed would make printing quadratic in the page count.
pub(crate) fn page_form(
    doc: &mut Document,
    pages: &[pdfcraft_model::Page],
    index: usize,
    content: Content,
    promoted: &mut std::collections::HashMap<ObjRef, ObjRef>,
) -> Result<ObjRef, PrintError> {
    let page = pages.get(index).ok_or_else(|| PrintError::Invalid(format!("there is no page {}", index.saturating_add(1))))?;
    let crop = page.crop(doc);
    let mut res = page.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    let mut body = if content == Content::FormFieldsOnly {
        Vec::new()
    } else {
        decoded_contents(doc, &page.dict, &mut DecodeBudget::page())
            .ok_or_else(|| PrintError::Invalid(format!("page {} has more content than can be printed", index + 1)))?
    };
    // Wrap the page content so a state it leaves behind can't affect the annotations.
    if !body.is_empty() {
        let mut wrapped = b"q\n".to_vec();
        wrapped.extend_from_slice(&body);
        wrapped.extend_from_slice(b"\nQ\n");
        body = wrapped;
    }
    let annots = page.dict.get(b"Annots").and_then(|a| doc.resolve(a).as_array().cloned()).unwrap_or_default();
    let mut xobjects = res.get(b"XObject").and_then(|x| doc.resolve(x).as_dict().cloned()).unwrap_or_default();
    let mut k = 0usize;
    for a in annots {
        let Some(d) = doc.resolve(&a).as_dict().cloned() else { continue };
        if !prints(doc, &d, content) {
            continue;
        }
        let rect: Vec<f64> =
            d.get(b"Rect").and_then(|r| doc.resolve(r).as_array().map(|a| a.iter().filter_map(Object::as_f64).collect())).unwrap_or_default();
        if rect.len() != 4 {
            continue;
        }
        let rect = [rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3])];
        let Some(ap) = appearance(doc, &d) else { continue };
        let form = doc.get(ap).as_dict().cloned().unwrap_or_default();
        let Some(m) = appearance_matrix(doc, &form, rect) else { continue };
        let ap = if !form.contains(b"Subtype")
            && let Object::Stream(stream) = &*doc.get(ap)
        {
            // Some existing annotation appearances omit Subtype, but Do dispatch needs a Form XObject.
            // Keep the original intact: other document objects can share this appearance.
            if let Some(&cached) = promoted.get(&ap) {
                cached
            } else {
                let mut stream = stream.clone();
                stream.dict.set(b"Subtype".to_vec(), Object::name("Form"));
                let copy = doc.add(Object::Stream(stream));
                promoted.insert(ap, copy);
                copy
            }
        } else {
            ap
        };
        // Keep the page's own XObjects, including names previous print operations generated.
        // Check the growing map so each appearance also keeps the name just assigned to it.
        let name = loop {
            let name = format!("PCPrA{k}");
            k = k.checked_add(1).ok_or_else(|| PrintError::Invalid("too many annotation appearance resources".into()))?;
            if !xobjects.contains(name.as_bytes()) {
                break name;
            }
        };
        let [a0, b0, c0, d0, e0, f0] = m.0;
        body.extend(format!("q {} {} {} {} {} {} cm /{name} Do Q\n", n(a0), n(b0), n(c0), n(d0), n(e0), n(f0)).bytes());
        xobjects.set(name.into_bytes(), Object::Ref(ap));
    }
    if !xobjects.is_empty() {
        res.set(b"XObject".to_vec(), Object::Dict(xobjects));
    }
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"BBox".to_vec(), Object::Array(crop.iter().map(|v| Object::Real(*v)).collect()));
    d.set(b"Resources".to_vec(), Object::Dict(res));
    if let Some(g) = page.dict.get(b"Group") {
        d.set(b"Group".to_vec(), g.clone());
    }
    Ok(doc.add(Object::Stream(Stream::flate(d, &body))))
}

#[cfg(test)]
thread_local! {
    /// Pages listed by [`source_pages`] on this thread.
    pub(crate) static PAGES_LISTED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The source document's pages, for printing.
fn source_pages(doc: &Document) -> Vec<pdfcraft_model::Page> {
    let pages = pdfcraft_model::pages(doc);
    #[cfg(test)]
    PAGES_LISTED.with(|c| c.set(c.get() + pages.len()));
    pages
}

/// Build the print-ready PDF for `settings`. Encryption is not carried over (the file goes to a
/// printer or is the user's own copy); callers must check the print permission first.
pub fn impose(src: &Document, settings: &Settings) -> Result<Vec<u8>, PrintError> {
    let pages = source_pages(src);
    let sizes: Vec<(f64, f64)> = pages.iter().map(|p| p.display_size(src)).collect();
    let sheets = layout(&sizes, settings)?;
    let mut doc = src.clone();
    // One form per source page used (a page may appear on several sheets: posters).
    let mut forms: std::collections::HashMap<usize, ObjRef> = std::collections::HashMap::new();
    let mut promoted = std::collections::HashMap::new();
    let views: Vec<[f64; 6]> = pages.iter().map(|p| p.view_matrix(src)).collect();
    let pages_root = doc.add(Object::Null);
    let mut kids = Vec::new();
    for sheet in &sheets {
        let mut c = String::new();
        let mut xo = Dict::new();
        for (i, pl) in sheet.placed.iter().enumerate() {
            let form = match forms.get(&pl.page) {
                Some(f) => *f,
                None => {
                    let f = page_form(&mut doc, &pages, pl.page, settings.content, &mut promoted)?;
                    forms.insert(pl.page, f);
                    f
                }
            };
            let name = format!("P{i}");
            xo.set(name.clone().into_bytes(), Object::Ref(form));
            // Display space → sheet, then clip to the visible part, then user → display.
            let [a, b, cc, d, e, f] = pl.matrix.0;
            let to_display = Matrix(views[pl.page]).invert().unwrap_or_default();
            let [ua, ub, uc, ud, ue, uf] = to_display.0;
            let [x0, y0, x1, y1] = pl.clip;
            c.push_str(&format!(
                "q {} {} {} {} {} {} cm {} {} {} {} re W n {} {} {} {} {} {} cm /{name} Do Q\n",
                geometry_number(a),
                geometry_number(b),
                geometry_number(cc),
                geometry_number(d),
                geometry_number(e),
                geometry_number(f),
                geometry_number(x0),
                geometry_number(y0),
                geometry_number(x1 - x0),
                geometry_number(y1 - y0),
                geometry_number(ua),
                geometry_number(ub),
                geometry_number(uc),
                geometry_number(ud),
                geometry_number(ue),
                geometry_number(uf)
            ));
        }
        if !sheet.borders.is_empty() || !sheet.lines.is_empty() {
            c.push_str("q 0 G 0.5 w\n");
            for r in &sheet.borders {
                c.push_str(&format!("{} {} {} {} re S\n", n(r[0]), n(r[1]), n(r[2] - r[0]), n(r[3] - r[1])));
            }
            for l in &sheet.lines {
                c.push_str(&format!("{} {} m {} {} l S\n", n(l[0]), n(l[1]), n(l[2]), n(l[3])));
            }
            c.push_str("Q\n");
        }
        let contents = doc.add(Object::Stream(Stream::flate(Dict::new(), c.as_bytes())));
        let mut res = Dict::new();
        res.set(b"XObject".to_vec(), Object::Dict(xo));
        let mut p = Dict::new();
        p.set(b"Type".to_vec(), Object::name("Page"));
        p.set(b"Parent".to_vec(), Object::Ref(pages_root));
        p.set(b"MediaBox".to_vec(), Object::Array([0.0, 0.0, sheet.size.0, sheet.size.1].iter().map(|v| Object::Real(*v)).collect()));
        p.set(b"Resources".to_vec(), Object::Dict(res));
        p.set(b"Contents".to_vec(), Object::Ref(contents));
        kids.push(Object::Ref(doc.add(Object::Dict(p))));
    }
    let mut tree = Dict::new();
    tree.set(b"Type".to_vec(), Object::name("Pages"));
    tree.set(b"Count".to_vec(), Object::Int(kids.len() as i64));
    tree.set(b"Kids".to_vec(), Object::Array(kids));
    doc.set(pages_root, Object::Dict(tree));
    let mut cat = Dict::new();
    cat.set(b"Type".to_vec(), Object::name("Catalog"));
    cat.set(b"Pages".to_vec(), Object::Ref(pages_root));
    // Keep the layers' configuration, or a layer that is off in the document would print: the
    // pages' marked content still refers to its groups, and without this nothing says they are off.
    let layers = src.trailer().get(b"Root").and_then(|r| src.resolve(r).as_dict().and_then(|root| root.get(b"OCProperties").cloned()));
    if let Some(layers) = layers {
        cat.set(b"OCProperties".to_vec(), layers);
    }
    let root = doc.add(Object::Dict(cat));
    doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(root));
    doc.trailer_mut().remove(b"Info");
    if doc.trailer().contains(b"Encrypt") {
        doc.remove_encryption();
    }
    Ok(write_full(&doc, &SaveOptions::default())?)
}

/// Which source pages land on each sheet (for previews and the sheet count).
pub fn sheet_pages(sheets: &[Sheet]) -> Vec<Vec<usize>> {
    sheets.iter().map(|s| s.placed.iter().map(|p| p.page).collect()).collect()
}
