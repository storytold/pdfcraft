//! The checks: each turns what the document holds, or what the [`Walk`] found its pages draw,
//! into [`Hit`]s.

use std::collections::{BTreeMap, HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use super::walk::{ImageKind, Transparency, Walk, Where};
use super::{Fixup, Hit, Space, number, parse_version};

/// Hairlines listed one by one on a page before the rest are counted.
const MAX_LINES_PER_PAGE: usize = 50;

/// A rectangle, normalised, when `o` is four finite numbers.
pub(super) fn rect(doc: &Document, o: Option<&Object>) -> Option<[f64; 4]> {
    let o = doc.resolve(o?);
    let v: Vec<f64> = o.as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
    let [x0, y0, x1, y1] = <[f64; 4]>::try_from(v).ok()?;
    [x0, y0, x1, y1].iter().all(|v| v.is_finite()).then(|| [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)])
}

fn mm(points: f64) -> f64 {
    points * 25.4 / 72.0
}

fn name_of(n: &[u8]) -> String {
    String::from_utf8_lossy(n).into_owned()
}

/// Fonts that don't carry their program, as PDF/A's check decides it. Type 3 fonts are drawn by
/// the document itself; a CIDFont is checked through the Type 0 font that uses it.
pub(super) fn fonts(doc: &Document, w: &Walk) -> Vec<Hit> {
    let mut out = Vec::new();
    for (r, o) in crate::objects(doc) {
        let Some(d) = crate::dict_of(&o) else { continue };
        let sub = d.name(b"Subtype");
        let is_font = d.name(b"Type") == Some(b"Font") || (d.get(b"BaseFont").is_some() && sub.is_some());
        if !is_font || matches!(sub, Some(b"CIDFontType0" | b"CIDFontType2")) {
            continue;
        }
        let page = w.fonts.get(&r.num).copied();
        let kind = sub.map(name_of).unwrap_or_default();
        crate::check_font(doc, d, &mut |_clause, pattern, value, _page, _fixable| {
            out.push(Hit::new(pattern, value).on(page).object(Some(r.num)).detail("Font type", kind.clone()));
        });
    }
    out.sort_by_key(|h| (h.page.unwrap_or(usize::MAX), h.object));
    out
}

/// Colour in a family the profile refuses, where the pages draw it.
pub(super) fn colour_spaces(w: &Walk, forbid: &[Space]) -> Vec<Hit> {
    w.colours
        .iter()
        .filter(|u| forbid.iter().any(|s| u.colour.is(*s)))
        .map(|u| {
            let pattern = match u.at {
                Where::Content => "Content uses {} colour",
                Where::Image => "An image uses {} colour",
                Where::InlineImage => "An inline image uses {} colour",
                Where::Shading => "A shading uses {} colour",
            };
            Hit::new(pattern, Some(u.colour.label())).on(Some(u.page)).object(u.object).bbox(u.bbox)
        })
        .collect()
}

/// The spot colours the pages use, each once, at the first place it is used.
pub(super) fn spot_colours(w: &Walk, max: Option<usize>) -> Vec<Hit> {
    let mut first: BTreeMap<String, (usize, Option<u32>)> = BTreeMap::new();
    for u in &w.colours {
        for name in u.colour.spots() {
            first.entry(name.to_owned()).or_insert((u.page, u.object));
        }
    }
    let mut out = Vec::new();
    if let Some(m) = max
        && first.len() > m
    {
        out.push(Hit::new("The pages use {} spot colours", Some(first.len().to_string())).detail("Most spot colours", m.to_string()));
    }
    out.extend(first.into_iter().map(|(name, (page, object))| Hit::new("Spot colour {}", Some(name)).on(Some(page)).object(object)));
    out
}

/// Images drawn below the resolution their kind needs: an image object once per page, at its
/// lowest resolution there.
pub(super) fn image_resolution(w: &Walk, colour: f64, gray: f64, bitmap: f64) -> Vec<Hit> {
    let mut out: Vec<Hit> = Vec::new();
    let mut lowest: HashMap<(usize, u32), (usize, f64)> = HashMap::new();
    let mut unmeasured = HashSet::new();
    for image in &w.images {
        if image.ppi.is_nan() {
            if unmeasured.insert((image.page, image.object)) {
                out.push(Hit::new("The resolution of an image can't be measured", None).on(Some(image.page)).object(image.object));
            }
            continue;
        }
        let (least, pattern) = match image.kind {
            ImageKind::Colour => (colour, "A colour image is drawn at {} ppi"),
            ImageKind::Gray => (gray, "A gray image is drawn at {} ppi"),
            ImageKind::Bitmap => (bitmap, "A bitmap image is drawn at {} ppi"),
        };
        if image.ppi >= least {
            continue;
        }
        let hit = Hit::new(pattern, Some(format!("{:.0}", image.ppi)))
            .on(Some(image.page))
            .object(image.object)
            .bbox(Some(image.bbox))
            .detail("Pixels", format!("{} × {}", image.width, image.height))
            .detail("Placed size", format!("{} × {} mm", number(mm(image.size[0])), number(mm(image.size[1]))))
            .detail("Least resolution", format!("{} ppi", number(least)));
        // An image object drawn twice on a page is listed once, where it is drawn smallest.
        match image.object {
            Some(object) => match lowest.get(&(image.page, object)) {
                Some(&(_, ppi)) if ppi <= image.ppi => {}
                Some(&(at, _)) => {
                    if let Some(slot) = out.get_mut(at) {
                        *slot = hit;
                    }
                    lowest.insert((image.page, object), (at, image.ppi));
                }
                None => {
                    lowest.insert((image.page, object), (out.len(), image.ppi));
                    out.push(hit);
                }
            },
            None => out.push(hit),
        }
    }
    out
}

/// Strokes thinner than `min` points: the first few on each page one by one, the rest counted.
pub(super) fn hairlines(w: &Walk, min: f64) -> Vec<Hit> {
    let mut out = Vec::new();
    let mut per_page: BTreeMap<usize, usize> = BTreeMap::new();
    for s in w.strokes.iter().filter(|s| s.width < min) {
        let n = per_page.entry(s.page).or_insert(0);
        *n += 1;
        if *n > MAX_LINES_PER_PAGE {
            continue;
        }
        let hit = if s.width <= 0.0 {
            Hit::new("A line has width 0, the thinnest the printer can draw", None)
        } else {
            Hit::new("A line is {} pt wide", Some(number(s.width)))
        };
        out.push(hit.on(Some(s.page)).object(s.object).bbox(s.bbox).detail("Line width", format!("{} pt", number(s.width))));
    }
    for (page, n) in per_page {
        if n > MAX_LINES_PER_PAGE {
            out.push(Hit::new("… and {} more on this page", Some((n - MAX_LINES_PER_PAGE).to_string())).on(Some(page)));
        }
    }
    out.sort_by_key(|h| h.page);
    out
}

/// Transparency the pages use.
pub(super) fn transparency(w: &Walk) -> Vec<Hit> {
    w.transparency
        .iter()
        .map(|t| {
            let hit = match &t.kind {
                Transparency::SoftMask => Hit::new("A graphics state uses a soft mask", None),
                Transparency::Opacity => Hit::new("A graphics state sets an opacity below 1", None),
                Transparency::BlendMode(m) => Hit::new("A graphics state uses the {} blend mode", Some(m.clone())),
                Transparency::ImageSoftMask => Hit::new("An image has a soft mask", None),
                Transparency::PageGroup => Hit::new("The page is a transparency group", None),
                Transparency::FormGroup => Hit::new("A form XObject is a transparency group", None),
            };
            hit.on(Some(t.page)).object(t.object).bbox(t.bbox)
        })
        .collect()
}

/// Pages with neither a TrimBox nor an ArtBox.
pub(super) fn trim_box(pages: &[pdfcraft_model::Page]) -> Vec<Hit> {
    pages
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.dict.contains(b"TrimBox") && !p.dict.contains(b"ArtBox"))
        .map(|(i, p)| Hit::new("The page has neither a TrimBox nor an ArtBox", None).on(Some(i)).object(Some(p.obj.num)).fixup(Fixup::SetTrimBox))
        .collect()
}

/// Pages without a BleedBox, or with less bleed past the trim than `min_mm` on some side. A page
/// with no trim to measure from is left to [`trim_box`].
pub(super) fn bleed(doc: &Document, pages: &[pdfcraft_model::Page], min_mm: f64) -> Vec<Hit> {
    let mut out = Vec::new();
    for (i, p) in pages.iter().enumerate() {
        let boxed = |key: &[u8]| rect(doc, p.dict.get(key));
        let Some(trim) = boxed(b"TrimBox").or_else(|| boxed(b"ArtBox")) else { continue };
        let hit = |pattern, value| Hit::new(pattern, value).on(Some(i)).object(Some(p.obj.num));
        let Some(bleed) = boxed(b"BleedBox") else {
            out.push(hit("The page has no BleedBox", None));
            continue;
        };
        // The BleedBox counts only as far as the MediaBox reaches (ISO 32000-2 14.11.2).
        let bleed = match boxed(b"MediaBox") {
            Some(m) => [bleed[0].max(m[0]), bleed[1].max(m[1]), bleed[2].min(m[2]), bleed[3].min(m[3])],
            None => bleed,
        };
        let narrowest =
            [trim[0] - bleed[0], trim[1] - bleed[1], bleed[2] - trim[2], bleed[3] - trim[3]].into_iter().map(mm).fold(f64::INFINITY, f64::min);
        if narrowest.is_finite() && narrowest + 1e-6 < min_mm {
            out.push(
                hit("The bleed is {} mm on its narrowest side", Some(format!("{:.1}", narrowest.max(0.0))))
                    .detail("Least bleed", format!("{} mm", number(min_mm))),
            );
        }
    }
    out
}

/// An encrypted file, with its security handler.
pub(super) fn encryption(doc: &Document) -> Vec<Hit> {
    let Some(e) = doc.trailer().get(b"Encrypt") else { return Vec::new() };
    let d = doc.resolve(e).as_dict().cloned().unwrap_or_default();
    let mut handler = d.name(b"Filter").map(name_of).unwrap_or_default();
    for key in ["V", "R"] {
        if let Some(n) = d.int(key.as_bytes()) {
            handler.push_str(&format!(" {key} {n}"));
        }
    }
    vec![Hit::new("The document is encrypted", None).object(e.as_ref().map(|r| r.num)).detail("Security handler", handler.trim().to_owned())]
}

/// Where each annotation object is: its page.
fn annotation_pages(doc: &Document, pages: &[pdfcraft_model::Page]) -> HashMap<ObjRef, usize> {
    let mut out = HashMap::new();
    for (i, p) in pages.iter().enumerate() {
        let annots = p.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        for r in annots.iter().filter_map(Object::as_ref) {
            out.entry(r).or_insert(i);
        }
    }
    out
}

/// What a holder of actions (an annotation, a field, an outline item) runs that the profile
/// doesn't allow: additional actions, and an action chain with JavaScript or Launch in it.
fn holder_hits(doc: &Document, d: &Dict, object: Option<u32>, page: Option<usize>, is_annot: bool) -> Vec<Hit> {
    let mut out = Vec::new();
    let at = |hit: Hit| hit.on(page).object(object).bbox(rect(doc, d.get(b"Rect"))).fixup(Fixup::RemoveScripts);
    if d.get(b"AA").is_some() {
        let hit = if is_annot {
            Hit::new("A {} annotation has additional actions", Some(d.name(b"Subtype").map(name_of).unwrap_or_default()))
        } else if d.get(b"FT").is_some() {
            Hit::new("A form field has additional actions", None)
        } else {
            Hit::new("An object has additional actions (/AA)", None)
        };
        out.push(at(hit));
    }
    if let Some(kind) = d.get(b"A").and_then(|a| super::fixups::runs(doc, a)) {
        out.push(at(Hit::new("A {} action isn't allowed", Some(kind.into()))));
    }
    out
}

/// Document JavaScript, JavaScript and Launch actions, and additional actions: what a file does by
/// itself when it is opened, viewed or printed. Each is reported where it is held (the catalog, a
/// page, an annotation, a field, an outline item); an action object nothing refers to runs nothing
/// and isn't reported, though [`Fixup::RemoveScripts`] empties it too.
pub(super) fn scripts(doc: &Document, pages: &[pdfcraft_model::Page]) -> Vec<Hit> {
    let mut out = Vec::new();
    let root = doc.root();
    let cat = crate::catalog(doc);
    let at_root = |hit: Hit| hit.object(root.map(|r| r.num)).fixup(Fixup::RemoveScripts);
    if cat.get(b"AA").is_some() {
        out.push(at_root(Hit::new("The document has additional actions (/AA)", None)));
    }
    if let Some(names) = cat.get(b"Names").map(|n| doc.resolve(n))
        && names.as_dict().is_some_and(|n| n.get(b"JavaScript").is_some())
    {
        out.push(at_root(Hit::new("The document has document-level JavaScript", None)));
    }
    if let Some(kind) = cat.get(b"OpenAction").and_then(|open| super::fixups::runs(doc, open)) {
        out.push(at_root(Hit::new("A {} action isn't allowed", Some(kind.into()))));
    }
    let page_objects: HashSet<ObjRef> = pages.iter().map(|p| p.obj).collect();
    for (i, p) in pages.iter().enumerate() {
        if p.dict.get(b"AA").is_some() {
            out.push(
                Hit::new("Page {} has additional actions", Some((i + 1).to_string())).on(Some(i)).object(Some(p.obj.num)).fixup(Fixup::RemoveScripts),
            );
        }
        // Annotations written into the page rather than as objects of their own.
        let annots = p.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        for d in annots.iter().filter_map(Object::as_dict) {
            out.extend(holder_hits(doc, d, None, Some(i), true));
        }
    }
    let annot_page = annotation_pages(doc, pages);
    for (r, o) in crate::objects(doc) {
        if Some(r) == root || page_objects.contains(&r) {
            continue;
        }
        let Object::Dict(d) = &*o else { continue };
        let page = annot_page.get(&r).copied();
        let is_annot = d.name(b"Type") == Some(b"Annot") || page.is_some();
        out.extend(holder_hits(doc, d, Some(r.num), page, is_annot));
    }
    out
}

/// The names of an annotation's flags (ISO 32000-2 table 167) that are set.
fn flags(f: i64) -> String {
    const NAMES: [&str; 10] =
        ["Invisible", "Hidden", "Print", "NoZoom", "NoRotate", "NoView", "ReadOnly", "Locked", "ToggleNoView", "LockedContents"];
    let set: Vec<&str> = NAMES.iter().enumerate().filter(|(bit, _)| f & (1 << bit) != 0).map(|(_, n)| *n).collect();
    if set.is_empty() { "0".into() } else { set.join(", ") }
}

/// Annotations other than pop-ups that lie inside the trim (the TrimBox, else the ArtBox, else the
/// visible page).
pub(super) fn annotations_in_trim(doc: &Document, pages: &[pdfcraft_model::Page]) -> Vec<Hit> {
    let mut out = Vec::new();
    for (i, p) in pages.iter().enumerate() {
        let boxed = |key: &[u8]| rect(doc, p.dict.get(key));
        let trim = boxed(b"TrimBox").or_else(|| boxed(b"ArtBox")).unwrap_or_else(|| p.crop(doc));
        let annots = p.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        for a in &annots {
            let d = doc.resolve(a);
            let Some(d) = d.as_dict() else { continue };
            let Some(sub) = d.name(b"Subtype").filter(|s| *s != b"Popup") else { continue };
            let Some(r) = rect(doc, d.get(b"Rect")) else { continue };
            if !pdfcraft_content::overlaps(r, trim, 0.0) {
                continue;
            }
            out.push(
                Hit::new("A {} annotation lies inside the trim", Some(name_of(sub)))
                    .on(Some(i))
                    .object(a.as_ref().map(|r| r.num))
                    .bbox(Some(r))
                    .detail("Flags", flags(d.int(b"F").unwrap_or(0))),
            );
        }
    }
    out
}

/// The PDF version: the header's, or the catalog's `/Version` when that is newer.
pub(super) fn document_version(doc: &Document) -> String {
    let header = doc.version().trim().to_owned();
    let catalog = crate::catalog(doc).name(b"Version").map(name_of);
    match (parse_version(&header), catalog.as_deref().and_then(parse_version)) {
        (Some(h), Some(c)) if c > h => catalog.unwrap_or(header),
        (None, Some(_)) => catalog.unwrap_or(header),
        _ => header,
    }
}

/// The PDF version: listed, or with `max` a hit when it is newer.
pub(super) fn version(doc: &Document, max: Option<&str>) -> Vec<Hit> {
    let v = document_version(doc);
    match max {
        None => vec![Hit::new("The file is PDF {}", Some(v))],
        Some(m) => match (parse_version(&v), parse_version(m)) {
            (Some(have), Some(most)) if have > most => {
                vec![Hit::new("The file is PDF {}, newer than the profile allows", Some(v)).detail("Highest version", m.to_owned())]
            }
            _ => Vec::new(),
        },
    }
}

/// How a value the document gives is shown: as PDF writes it.
fn shown(o: &Object) -> String {
    match o {
        Object::Name(n) => format!("/{}", name_of(n)),
        Object::Bool(b) => b.to_string(),
        Object::Int(i) => i.to_string(),
        Object::Real(r) => r.to_string(),
        Object::String(s) => format!("({})", s.to_text()),
        Object::Null => "null".into(),
        _ => "…".into(),
    }
}

/// `Trapped`: in the document information as `/True` or `/False`, or in the metadata; where both
/// say it, they agree.
pub(super) fn trapped(doc: &Document) -> Vec<Hit> {
    let info_ref = doc.trailer().get(b"Info").and_then(Object::as_ref).map(|r| r.num);
    let info = doc
        .trailer()
        .get(b"Info")
        .map(|i| doc.resolve(i))
        .and_then(|i| i.as_dict().and_then(|d| d.get(b"Trapped").cloned()))
        .map(|t| doc.resolve(&t))
        .map(|t| match &*t {
            Object::Name(n) if n == b"True" => Ok(true),
            Object::Name(n) if n == b"False" => Ok(false),
            other => Err(shown(other)),
        });
    let meta = crate::metadata(doc).and_then(|x| crate::xmp::value(&x, "pdf:Trapped")).map(|t| match t.trim() {
        "True" => Ok(true),
        "False" => Ok(false),
        other => Err(other.to_owned()),
    });
    let at = |hit: Hit| hit.object(info_ref);
    match (&info, &meta) {
        (Some(Err(v)), _) => {
            vec![at(Hit::new("Trapped is {}, but it must be /True or /False", Some(v.clone()))).fixup(Fixup::SetTrapped { trapped: false })]
        }
        (None, Some(Err(v))) => {
            vec![at(Hit::new("Trapped is {}, but it must be /True or /False", Some(v.clone()))).fixup(Fixup::SetTrapped { trapped: false })]
        }
        // The key says it, the metadata's value doesn't: the fixup writes the key's to both.
        (Some(Ok(a)), Some(Err(v))) => {
            vec![at(Hit::new("Trapped is {}, but it must be /True or /False", Some(v.clone()))).fixup(Fixup::SetTrapped { trapped: *a })]
        }
        (None, None) => {
            vec![at(Hit::new("The document doesn't say whether it is trapped (Trapped)", None)).fixup(Fixup::SetTrapped { trapped: false })]
        }
        (Some(Ok(a)), Some(Ok(b))) if a != b => {
            vec![at(Hit::new("The Trapped key and the metadata's pdf:Trapped disagree", None)).fixup(Fixup::SetTrapped { trapped: *a })]
        }
        _ => Vec::new(),
    }
}

/// Every PDF/A rule the document breaks. Save as PDF/A fixes what it can; no fixup here.
pub(super) fn pdfa(doc: &Document, level: crate::Level) -> Vec<Hit> {
    let standard = match level {
        crate::Level::A2b => "ISO 19005-2",
        crate::Level::A3b => "ISO 19005-3",
    };
    crate::verify(doc, level)
        .into_iter()
        .map(|i| {
            let mut hit = Hit::new(i.pattern, i.value).on(i.page).detail("Clause", format!("{standard} {}", i.clause));
            hit.rule = Some(i.clause);
            hit
        })
        .collect()
}

/// Every PDF/X rule the document breaks, with the fixups that address the trapping, trim and
/// script rules.
pub(super) fn pdfx(doc: &Document, level: crate::pdfx::Level) -> Vec<Hit> {
    crate::pdfx::verify(doc, level)
        .into_iter()
        .map(|f| {
            let fixup = match (f.rule, f.pattern) {
                (_, "The document doesn't say whether it is trapped (Trapped)" | "Trapped is {}, but it must be /True or /False") => {
                    Some(Fixup::SetTrapped { trapped: false })
                }
                (_, "The page has neither a TrimBox nor an ArtBox") => Some(Fixup::SetTrimBox),
                ("pdfx.javascript" | "pdfx.actions", _) => Some(Fixup::RemoveScripts),
                _ => None,
            };
            let mut hit =
                Hit::new(f.pattern, f.value).on(f.page).object(f.object).detail("Clause", f.clause.to_owned()).detail("Rule", f.rule.to_owned());
            hit.rule = Some(f.rule);
            hit.fixup = fixup;
            hit
        })
        .collect()
}
