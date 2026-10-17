//! Redaction (architecture §11.2, execution plan M8.5–M8.7). Layer L4.
//!
//! Content is marked for redaction with Redact annotations (§12.5.6.23); `pdfcraft-annot`
//! creates them. [`apply`] then removes everything under the marks, for good:
//! - text glyphs (the rest of each line keeps its position), inline images, and paths that the
//!   marks cover; images and vectors partly under a mark lose the covered part;
//! - content inside form XObjects (rewritten as new objects, so pages sharing them keep theirs);
//! - comments, links and form fields whose rectangle overlaps a mark;
//! - document information entries whose value repeats text the marks removed (with the XMP
//!   property mirroring each entry), and XMP streams that still hold removed text where
//!   per-entry editing cannot reach — removed whole (see [`clear_marked_metadata`]);
//! - the marks themselves, replaced by boxes in their fill colour (with their overlay text)
//!   drawn into the page.
//!
//! A verification pass then re-reads every redacted page; if any glyph or inline image is still
//! under a region the whole operation fails (callers keep the previous document: fail-closed).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

pub mod codes;
mod image;
mod interp;
mod metadata;
mod overlay;
pub mod patterns;
pub mod sanitize;
mod tags;
#[cfg(test)]
mod tests;

use interp::{Mode, Scope, process};
use metadata::{KernedFragment, normalize, packet_hit};

pub type Rgb = [f64; 3];

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum RedactError {
    #[error("there are no redaction marks to apply")]
    NothingToApply,
    #[error("page {0} has content that can't be read, so it can't be redacted safely")]
    Unreadable(usize),
    #[error("redaction could not be verified: {0} item(s) were still found under the marks, so nothing was changed")]
    Residue(usize),
    /// A mark's overlay text has a character Helvetica can't draw; nothing was redacted.
    #[error("{0}")]
    Undrawable(String),
    #[error(
        "page {0} has vertical-writing text while a redaction mark is pending, and PdfCraft cannot tell whether that text lies under the mark yet, so nothing was changed"
    )]
    VerticalText(usize),
    #[error(
        "redaction appearance has invalid geometry or exceeds the 8 MiB budget; reduce the marked area, turn off repeated text or apply fewer pages at once (nothing was redacted)"
    )]
    OverlayLimit,
    #[error(transparent)]
    Edit(#[from] pdfcraft_edit::EditError),
    #[error(transparent)]
    Form(#[from] pdfcraft_forms::FormError),
    #[error(transparent)]
    Cos(#[from] pdfcraft_cos::CosError),
}

/// A redaction mark on a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    /// 0-based page index.
    pub page: usize,
    pub obj: ObjRef,
    /// The marked areas in user space (one per quadrilateral).
    pub rects: Vec<[f64; 4]>,
    /// The box colour once applied (`/IC`; none = no box).
    pub fill: Option<Rgb>,
    pub overlay: String,
    /// How the overlay text is drawn (`/DA`, `/Q`, `/Repeat`).
    pub look: pdfcraft_annot::OverlayLook,
}

/// The overlay look of a Redact annotation: font, size and colour from `/DA`, alignment from
/// `/Q`, repetition from `/Repeat`.
fn overlay_look(doc: &Document, d: &Dict) -> pdfcraft_annot::OverlayLook {
    let mut look = pdfcraft_annot::OverlayLook::default();
    if let Some(da) = d.get(b"DA").and_then(|o| doc.resolve(o).as_string().map(|s| String::from_utf8_lossy(&s.bytes).into_owned())) {
        let t: Vec<&str> = da.split_whitespace().collect();
        for (i, w) in t.iter().enumerate() {
            match *w {
                "rg" if i >= 3 => {
                    if let (Ok(r), Ok(g), Ok(b)) = (t[i - 3].parse::<f64>(), t[i - 2].parse::<f64>(), t[i - 1].parse::<f64>()) {
                        look.color = [r, g, b];
                    }
                }
                "g" if i >= 1 => {
                    if let Ok(g) = t[i - 1].parse::<f64>() {
                        look.color = [g; 3];
                    }
                }
                "Tf" if i >= 2 => {
                    look.font = pdfcraft_annot::OverlayFont::from_resource(t[i - 2].trim_start_matches('/'));
                    look.size = t[i - 1].parse::<f64>().unwrap_or(0.0).max(0.0);
                }
                _ => {}
            }
        }
    }
    look.align = d.int(b"Q").unwrap_or(1).clamp(0, 2) as u8;
    look.repeat = matches!(d.get(b"Repeat"), Some(Object::Bool(true)));
    look
}

/// What [`apply`] removed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub marks: usize,
    pub pages: usize,
    pub glyphs: usize,
    pub images_removed: usize,
    pub images_cleared: usize,
    pub paths_removed: usize,
    pub paths_clipped: usize,
    pub forms_rewritten: usize,
    pub forms_removed: usize,
    pub annotations: usize,
    pub fields: usize,
    /// Structure elements that lost alternate/actual text or emptied marked content.
    pub tags: usize,
    /// The form was an AcroForm+XFA form and its `/XFA` packets (which still held redacted field
    /// values, and from which a relayout could rebuild them) were removed.
    pub xfa_removed: bool,
    /// `/Info` entries whose value repeated text the marks removed, cleared (each with the XMP
    /// property mirroring it); the last count is the whole dictionary when it could only be
    /// removed wholesale.
    pub info_entries_cleared: usize,
    /// XMP metadata streams removed whole: they still held removed text where per-entry editing
    /// cannot reach (custom schemas, page-level packets), or could not be decoded and read.
    pub xmp_removed: usize,
    /// A metadata collection or comparison limit was exceeded. All `/Info` and catalog/page
    /// `/Metadata` entries were removed, including unrelated values, rather than risk a leak.
    pub metadata_cleared_on_limit: bool,
}

fn rect_of(doc: &Document, o: Option<&Object>) -> Option<[f64; 4]> {
    let v: Vec<f64> = doc.resolve(o?).as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
    (v.len() == 4 && v.iter().all(|x| x.is_finite())).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
}

fn color(doc: &Document, d: &Dict, key: &[u8]) -> Option<Rgb> {
    let v: Vec<f64> = doc.resolve(d.get(key)?).as_array()?.iter().filter_map(Object::as_f64).collect();
    match v.len() {
        1 => Some([v[0]; 3]),
        3 => Some([v[0], v[1], v[2]]),
        4 => Some([(1.0 - v[0]) * (1.0 - v[3]), (1.0 - v[1]) * (1.0 - v[3]), (1.0 - v[2]) * (1.0 - v[3])]),
        _ => None,
    }
}

pub(crate) fn annots_of(doc: &Document, page: &Dict) -> Vec<Object> {
    page.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default()
}

/// Every redaction mark in the document, page by page.
pub fn marks(doc: &Document) -> Vec<Mark> {
    let mut out = Vec::new();
    for (pi, p) in pdfcraft_model::pages(doc).iter().enumerate() {
        for a in annots_of(doc, &p.dict) {
            let Some(r) = a.as_ref() else { continue };
            let obj = doc.get(r);
            let Some(d) = obj.as_dict() else { continue };
            if d.name(b"Subtype") != Some(b"Redact") {
                continue;
            }
            let quads: Vec<f64> = d
                .get(b"QuadPoints")
                .and_then(|q| doc.resolve(q).as_array().map(|a| a.iter().filter_map(Object::as_f64).collect()))
                .unwrap_or_default();
            let mut rects: Vec<[f64; 4]> = quads
                .as_chunks::<8>()
                .0
                .iter()
                .map(|q| {
                    let xs = [q[0], q[2], q[4], q[6]];
                    let ys = [q[1], q[3], q[5], q[7]];
                    [
                        xs.iter().copied().fold(f64::MAX, f64::min),
                        ys.iter().copied().fold(f64::MAX, f64::min),
                        xs.iter().copied().fold(f64::MIN, f64::max),
                        ys.iter().copied().fold(f64::MIN, f64::max),
                    ]
                })
                .filter(|r| r.iter().all(|v| v.is_finite()))
                .collect();
            if rects.is_empty()
                && let Some(r) = rect_of(doc, d.get(b"Rect"))
            {
                rects.push(r);
            }
            if rects.is_empty() {
                continue;
            }
            let overlay = d.get(b"OverlayText").and_then(|t| doc.resolve(t).as_string().map(|s| s.to_text())).unwrap_or_default();
            out.push(Mark { page: pi, obj: r, rects, fill: color(doc, d, b"IC"), overlay, look: overlay_look(doc, d) });
        }
    }
    out
}

/// A page's content streams: (the references or inline objects as listed, their decoded data).
pub(crate) fn page_streams(doc: &Document, page: &Dict, index: usize) -> Result<(Vec<Object>, Vec<Vec<u8>>), RedactError> {
    let list: Vec<Object> = match page.get(b"Contents") {
        None => Vec::new(),
        Some(c) => match &*doc.resolve(c) {
            Object::Array(a) => a.clone(),
            _ => vec![c.clone()],
        },
    };
    let mut data = Vec::new();
    for o in &list {
        match &*doc.resolve(o) {
            Object::Stream(s) => data.push(s.decoded().map_err(|_| RedactError::Unreadable(index + 1))?),
            Object::Null => data.push(Vec::new()),
            _ => return Err(RedactError::Unreadable(index + 1)),
        }
    }
    Ok((list, data))
}

/// The nearest ancestor of `page` that holds `/Resources` (where an inheriting page gets them):
/// `Some(None)` when none does, `None` when the chain can't be followed (a cycle, a broken node or
/// a tree deeper than any real one), so callers can stay conservative.
fn resource_source(doc: &Document, page: ObjRef) -> Option<Option<ObjRef>> {
    let mut seen = HashSet::new();
    let mut parent = doc.get(page).as_dict().and_then(|d| d.get(b"Parent")).and_then(Object::as_ref);
    while let Some(r) = parent {
        if !seen.insert(r) || seen.len() > 4096 {
            return None;
        }
        let node = doc.get(r);
        let d = node.as_dict()?;
        if d.contains(b"Resources") {
            return Some(Some(r));
        }
        parent = d.get(b"Parent").and_then(Object::as_ref);
    }
    Some(None)
}

/// Before rewriting `selected` pages, pin the resources they inherit onto them, and retire the
/// ancestor `/Resources` they came from once no other page still inherits it, so the parent can't
/// keep a removed XObject reachable. Pages that aren't redacted are left untouched; an inherited
/// dictionary that is direct becomes one shared indirect object, never a copy per page.
fn localize_resources(doc: &mut Document, selected: &[usize]) -> Result<(), pdfcraft_cos::CosError> {
    let pages = pdfcraft_model::pages(doc);
    let inherits = |doc: &Document, p: &pdfcraft_model::Page| doc.get(p.obj).as_dict().is_some_and(|d| !d.contains(b"Resources"));
    // Ancestor → the reference its resources are shared through, and whether that reference is new.
    let mut shared: HashMap<ObjRef, (Object, bool)> = HashMap::new();
    for page in selected.iter().filter_map(|&i| pages.get(i)) {
        // What the page tree gives the page; nothing to pin if it gets nothing.
        let Some(effective) = page.dict.get(b"Resources").cloned() else { continue };
        if !inherits(doc, page) {
            continue;
        }
        let source = resource_source(doc, page.obj).flatten();
        let original = source.and_then(|s| doc.get(s).as_dict().and_then(|d| d.get(b"Resources").cloned()));
        let value = match source.filter(|_| original.as_ref() == Some(&effective)) {
            // `/Parent` agrees with the page tree: share one reference per ancestor.
            Some(source) => match shared.get(&source) {
                Some((v, _)) => v.clone(),
                None => {
                    // A direct dictionary is made indirect once, so every pinned page shares it.
                    let (v, new) = match effective {
                        Object::Ref(_) => (effective, false),
                        direct => (Object::Ref(doc.add(direct)), true),
                    };
                    shared.insert(source, (v.clone(), new));
                    v
                }
            },
            // A malformed tree whose `/Parent` disagrees: pin this page alone and retire nothing.
            None => match effective {
                Object::Ref(_) => effective,
                direct => Object::Ref(doc.add(direct)),
            },
        };
        doc.update_dict(page.obj, |d| d.set(b"Resources".to_vec(), value))?;
    }
    if shared.is_empty() {
        return Ok(());
    }
    // Ancestors some page still inherits from keep their resources (that page draws them), through
    // the shared reference; the others are retired. A page that inherits resources through a chain
    // that can't be followed might inherit from any of them, so then none is retired.
    let mut still_used: HashSet<ObjRef> = HashSet::new();
    for page in pages.iter().filter(|p| p.dict.contains(b"Resources") && inherits(doc, p)) {
        match resource_source(doc, page.obj) {
            Some(Some(source)) => {
                if shared.contains_key(&source) {
                    still_used.insert(source);
                }
            }
            _ => still_used.extend(shared.keys().copied()),
        }
    }
    for (source, (value, new)) in shared {
        if still_used.contains(&source) {
            if !new {
                continue;
            }
            doc.update_dict(source, |d| d.set(b"Resources".to_vec(), value))?;
        } else {
            doc.update_dict(source, |d| {
                d.remove(b"Resources");
            })?;
        }
    }
    Ok(())
}

/// Apply every redaction mark (or only those on `pages`, 0-based). Irreversible for the saved
/// file; callers keep the previous document for undo.
pub fn apply(doc: &mut Document, pages: Option<&[usize]>) -> Result<Report, RedactError> {
    let all_marks = marks(doc);
    let chosen: Vec<&Mark> = all_marks.iter().filter(|m| pages.is_none_or(|p| p.contains(&m.page))).collect();
    if chosen.is_empty() {
        return Err(RedactError::NothingToApply);
    }
    // Overlay text is drawn in Helvetica; applying can't be undone in the file, so text it can't
    // draw (a mark another program made) is refused before anything changes, never burnt in as `?`.
    if let Some((m, c)) = chosen.iter().find_map(|m| pdfcraft_fonts::first_undrawable(&m.overlay).map(|c| (m, c))) {
        return Err(RedactError::Undrawable(format!(
            "{}; change or clear that mark's overlay text, then apply again (nothing was redacted)",
            pdfcraft_fonts::undrawable_message(&format!("the overlay text of a redaction mark on page {}", m.page + 1), c)
        )));
    }
    let mut report = Report { marks: chosen.len(), ..Report::default() };
    // `marks` visits pages in order. Keep slices of that order for appearance generation,
    // content rewriting and verification instead of scanning all marks for every page.
    let groups: Vec<_> = chosen.chunk_by(|a, b| a.page == b.page).filter_map(|group| group.first().map(|mark| (mark.page, group))).collect();
    let by_page: Vec<_> = groups.iter().map(|(page, _)| *page).collect();
    // Budget failures must leave even a direct library caller's document untouched.
    let mut remaining = overlay::MAX_BYTES;
    let overlays = groups
        .iter()
        .map(|&(pi, page_marks)| overlay::content(page_marks, &mut remaining).map(|content| (pi, content)))
        .collect::<Result<Vec<_>, _>>()?;
    report.pages = by_page.len();
    localize_resources(doc, &by_page)?;
    let mut doomed_fields: Vec<String> = Vec::new();
    // The original image and form XObjects the rewrite drops or replaces, on any page.
    let mut doomed_xobjects: Vec<ObjRef> = Vec::new();
    let widget_owner: Vec<(ObjRef, String)> =
        pdfcraft_forms::fields(doc).into_iter().flat_map(|f| f.widgets.iter().map(|w| (w.obj, f.name.clone())).collect::<Vec<_>>()).collect();
    // The text the marks removed (maximal runs), for the metadata pass below.
    let mut fragments: Vec<String> = Vec::new();
    let mut kerned = Vec::new();
    let (mut metadata_bytes, mut metadata_limited) = (0, false);

    for &(pi, page_marks) in &groups {
        let page = pdfcraft_model::pages(doc).swap_remove(pi);
        let rects: Vec<[f64; 4]> = page_marks.iter().flat_map(|m| m.rects.iter().copied()).collect();

        // 1. Content.
        let (list, data) = page_streams(doc, &page.dict, pi)?;
        let resources = page.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
        let out = {
            let mut scope = Scope::new(&rects, Mode::Apply, &mut report);
            scope.removed_fragments = std::mem::take(&mut fragments);
            scope.kerned_fragments = std::mem::take(&mut kerned);
            scope.metadata_bytes = metadata_bytes;
            scope.metadata_limited = metadata_limited;
            let out = process(doc, &mut scope, &data, &resources, pdfcraft_content::Matrix::IDENTITY);
            scope.flush_run();
            metadata_bytes = scope.metadata_bytes;
            metadata_limited = scope.metadata_limited;
            fragments = scope.removed_fragments;
            kerned = scope.kerned_fragments;
            out
        };
        // Fail closed: the interpreter lays vertical-writing text out horizontally, so it cannot
        // tell whether those glyphs lie under a mark. Rather than report success and possibly
        // leave the text in place, refuse the whole operation (the caller keeps the previous
        // document). The error does not claim the text was under the mark. Full vertical-text
        // removal is a future enhancement.
        if out.vertical {
            return Err(RedactError::VerticalText(pi + 1));
        }
        doomed_xobjects.extend_from_slice(&out.doomed);
        let mut new_list = list.clone();
        let mut changed = false;
        for (i, new) in out.streams.into_iter().enumerate() {
            let Some(bytes) = new else { continue };
            let mut dict = match &*doc.resolve(&list[i]) {
                Object::Stream(s) => s.dict.clone(),
                _ => Dict::new(),
            };
            dict.remove(b"Length");
            // Edit ▸ Add content keeps an added item's source text in `/PCAdded`; once its glyphs
            // are redacted, keeping it would leave the redacted text in the file. The item stays
            // as plain drawn content but is no longer editable.
            dict.remove(b"PCAdded");
            // A new object: the original may be shared with other pages.
            new_list[i] = Object::Ref(doc.add(Object::Stream(Stream::flate(dict, &bytes))));
            changed = true;
        }
        if changed || !out.xobjects.is_empty() || !out.replaced.is_empty() {
            let mut res = resources.clone();
            if !out.xobjects.is_empty() || !out.replaced.is_empty() {
                let mut xo = res.get(b"XObject").and_then(|x| doc.resolve(x).as_dict().cloned()).unwrap_or_default();
                // A full save keeps whatever is still referenced: unlink the originals this page no
                // longer draws so the removed image or form bytes cannot survive in the saved file.
                let page_content: Vec<Vec<u8>> = new_list
                    .iter()
                    .map(|o| match &*doc.resolve(o) {
                        Object::Stream(s) => s.decoded().unwrap_or_default(),
                        _ => Vec::new(),
                    })
                    .collect();
                interp::prune(&mut xo, &out.replaced, &page_content);
                for (n, r) in &out.xobjects {
                    xo.set(n.clone(), Object::Ref(*r));
                }
                res.set(b"XObject".to_vec(), Object::Dict(xo));
            }
            doc.update_dict(page.obj, |d| {
                d.set(b"Contents".to_vec(), Object::Array(new_list));
                d.set(b"Resources".to_vec(), Object::Dict(res));
            })?;
            // Tags must not keep what the page no longer shows.
            let after = doc.get(page.obj).as_dict().cloned().unwrap_or_default();
            let (_, new_data) = page_streams(doc, &after, pi)?;
            let (changed_ids, empty_ids) = tags::touched(doc, &resources, &data.join(&b'\n'), &new_data.join(&b'\n'));
            report.tags += tags::clean(doc, page.obj, None, &changed_ids, &empty_ids)?;
        }

        for (old, new, changed, empty) in out.tag_changes {
            report.tags += tags::clean(doc, page.obj, Some((old, new)), &changed, &empty)?;
        }

        // 2. Annotations: the marks, and whatever lies under them (with their pop-ups).
        let annots = annots_of(doc, &doc.get(page.obj).as_dict().cloned().unwrap_or_default());
        let mut removed: Vec<ObjRef> = Vec::new();
        for a in &annots {
            let Some(r) = a.as_ref() else { continue };
            let obj = doc.get(r);
            let Some(d) = obj.as_dict() else { continue };
            let subtype = d.name(b"Subtype").unwrap_or(b"");
            if subtype == b"Redact" {
                if page_marks.iter().any(|m| m.obj == r) {
                    removed.push(r);
                }
                continue;
            }
            if subtype == b"Popup" {
                continue;
            }
            if rect_of(doc, d.get(b"Rect")).is_some_and(|b| rects.iter().any(|x| pdfcraft_content::overlaps(*x, b, 0.0))) {
                removed.push(r);
                if subtype == b"Widget" {
                    if let Some((_, name)) = widget_owner.iter().find(|(w, _)| *w == r)
                        && !doomed_fields.contains(name)
                    {
                        doomed_fields.push(name.clone());
                    }
                } else {
                    report.annotations += 1;
                }
            }
        }
        let kept: Vec<Object> = annots
            .into_iter()
            .filter(|a| {
                let Some(r) = a.as_ref() else { return true };
                if removed.contains(&r) {
                    return false;
                }
                // Pop-ups of removed annotations go too.
                let obj = doc.get(r);
                !obj.as_dict().and_then(|d| d.get(b"Parent")).and_then(Object::as_ref).is_some_and(|p| removed.contains(&p))
            })
            .collect();
        doc.update_dict(page.obj, |d| {
            if kept.is_empty() {
                d.remove(b"Annots");
            } else {
                d.set(b"Annots".to_vec(), Object::Array(kept));
            }
        })?;
    }

    // 3. Form fields with a widget under a mark (all their widgets go).
    for name in &doomed_fields {
        if pdfcraft_forms::fields(doc).iter().any(|f| &f.name == name) {
            pdfcraft_forms::delete_field(doc, name)?;
            report.fields += 1;
        }
    }

    // 3b. An XFA form mirrors its field values in the /XFA datasets packet, and a relayout
    // rebuilds fields and pages from that packet, so deleting the AcroForm field alone leaves the
    // redacted value in the file and lets a later relayout put it back. Once redactions are
    // applied the form stops being XFA: the packets, /NeedsRendering, the layout mark and the
    // rest go (pdfcraft_xfa::strip), leaving a plain already-laid-out AcroForm that neither the
    // saved file nor a relayout can restore from.
    if pdfcraft_xfa::xfa_shape(doc).is_some() {
        let r = pdfcraft_xfa::strip(doc).map_err(|e| pdfcraft_edit::EditError::Invalid(e.to_string()))?;
        report.xfa_removed = !r.packets.is_empty();
    }

    // 4. The boxes.
    for (pi, content) in overlays {
        if !content.is_empty() {
            pdfcraft_edit::stamp(doc, pi, "Redaction", content)?;
        }
    }

    // A removed image or form must not stay reachable through another resource dictionary (an
    // inherited or shared /Resources): a full save keeps whatever is still referenced.
    interp::unlink_doomed(doc, &doomed_xobjects)?;

    // 5. Verify: nothing readable may remain under a region.
    let mut residue = 0;
    for &(pi, page_marks) in &groups {
        let page = pdfcraft_model::pages(doc).swap_remove(pi);
        let rects: Vec<[f64; 4]> = page_marks.iter().flat_map(|m| m.rects.iter().copied()).collect();
        let (list, data) = page_streams(doc, &page.dict, pi)?;
        // The overlay stream (last) draws text of its own: the overlay label is allowed.
        let original: Vec<Vec<u8>> = list
            .iter()
            .zip(data)
            .filter(|(o, _)| doc.resolve(o).as_dict().and_then(|d| d.name(b"PCMark")) != Some(b"Redaction"))
            .map(|(_, d)| d)
            .collect();
        let resources = page.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
        let mut scratch = Report::default();
        let mut scope = Scope::new(&rects, Mode::Verify, &mut scratch);
        residue += process(doc, &mut scope, &original, &resources, pdfcraft_content::Matrix::IDENTITY).residue;
    }
    if residue > 0 {
        return Err(RedactError::Residue(residue));
    }
    // 6. Metadata: what the marks removed must not survive in `/Info` or XMP either (#2448).
    //    Before the full-save flag, so the previous revision's metadata leaves the file too.
    if metadata_limited {
        clear_metadata_on_limit(doc, &mut report)?;
    } else {
        clear_marked_metadata(doc, &fragments, &kerned, &mut report)?;
    }
    // The previous revision still holds the removed content: the next save must rewrite.
    doc.require_full_save();
    Ok(report)
}

/// Decoding limit for a metadata stream (XMP packets are a few kB; padding rarely takes them
/// past 100 kB — the same cap `pdfcraft_organize` reads packets with).
const MAX_METADATA: usize = 16 << 20;

/// A shared limit means earlier no-match results are no longer sufficient evidence. Retire
/// all metadata in the existing catalog/page scope; require_full_save then drops old objects.
fn clear_metadata_on_limit(doc: &mut Document, report: &mut Report) -> Result<(), RedactError> {
    report.metadata_cleared_on_limit = true;
    let count = doc.trailer().get(b"Info").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().map(Dict::len)).unwrap_or(0);
    doc.trailer_mut().remove(b"Info");
    report.info_entries_cleared += count;
    let mut owners: Vec<_> = doc.root().into_iter().collect();
    owners.extend(pdfcraft_model::pages(doc).iter().map(|p| p.obj));
    for owner in owners {
        if doc.get(owner).as_dict().is_some_and(|d| d.contains(b"Metadata")) {
            doc.update_dict(owner, |d| {
                d.remove(b"Metadata");
            })?;
            report.xmp_removed += 1;
        }
    }
    Ok(())
}

/// The metadata half of redaction (#2448): a string the marks took off the pages must not stay
/// in the document information dictionary or the XMP metadata. Runs of removed text two
/// characters or longer are matched case-insensitively with collapsed whitespace (a single clipped glyph is no evidence a
/// secret was removed, and would clear unrelated entries that merely contain that character).
///
/// Per entry first, so unrelated values survive: each `/Info` string entry whose value contains
/// a removed run — or is contained in one, for a value shorter than the run (the author of a
/// whole redacted sentence is removed text too) — goes through `pdfcraft_organize::set_info`,
/// which also removes the XMP property mirroring it and leaves the rest of the packet byte for
/// byte. Then, wholesale where per-entry editing cannot reach: every catalog and page
/// `/Metadata` stream is kept only when it decodes, reads as UTF-8, and holds no removed run or
/// part of one in decoded XML values (markup names and namespace declarations are excluded);
/// a trailer `/Info` that is a direct dictionary — the one shape an entry clear cannot edit —
/// is removed whole when it still matches. Matching errs towards clearing; text that leaves no
/// searchable run (image text, one surviving piece) needs `sanitize`'s metadata category to be
/// provably gone.
fn clear_marked_metadata(doc: &mut Document, fragments: &[String], kerned: &[KernedFragment], report: &mut Report) -> Result<(), RedactError> {
    let work = metadata::Work::new();
    // Collection enforces these before allocating; guard this boundary too, before comparison
    // normalization can expand Unicode. Optional-gap copies are bounded separately as well.
    if fragments.len() > metadata::MAX_FRAGMENTS
        || kerned.len() > metadata::MAX_FRAGMENTS
        || fragments.iter().any(|f| f.len() > metadata::MAX_RUN_BYTES)
        || kerned.iter().any(|f| f.text.len() > metadata::MAX_RUN_BYTES || f.gaps.len() > metadata::MAX_RUN_BYTES)
        || fragments.iter().fold(0usize, |n, f| n.saturating_add(f.len())) > metadata::MAX_FRAGMENT_BYTES
        || kerned.iter().fold(0usize, |n, f| n.saturating_add(f.text.len())) > metadata::MAX_FRAGMENT_BYTES
    {
        return clear_metadata_on_limit(doc, report);
    }
    let needles: Vec<_> = fragments
        .iter()
        .map(|f| KernedFragment { text: normalize(f), gaps: Vec::new() })
        .chain(kerned.iter().map(KernedFragment::normalized))
        .filter(|f| f.text.chars().take(2).count() >= 2)
        .collect();
    if needles.is_empty() {
        return Ok(());
    }
    // A value goes when it holds a removed run, or when it is itself part of one. The second
    // direction never matches a value under two characters: one shared character is no evidence
    // a secret was removed.
    let hit = |v: &str| {
        if v.len() > metadata::MAX_VALUE_BYTES || !work.charge(v.len().saturating_mul(4)) {
            work.exhausted.set(true);
            return true;
        }
        let v = normalize(v);
        v.chars().take(2).count() >= 2 && needles.iter().any(|n| n.matches_with_budget(&v, &work))
    };

    // Bound decoding a string value and building the key list before organize::info copies it.
    let oversized_info = doc
        .trailer()
        .get(b"Info")
        .map(|o| doc.resolve(o))
        .and_then(|o| {
            o.as_dict().map(|d| {
                d.len() > metadata::MAX_FRAGMENTS
                    || d.iter().any(|(key, v)| {
                        !work.charge(key.len()) || doc.resolve(v).as_string().is_some_and(|s| s.bytes.len() > metadata::MAX_VALUE_BYTES)
                    })
            })
        })
        .unwrap_or(false);
    if oversized_info {
        return clear_metadata_on_limit(doc, report);
    }

    // 1. Entry by entry: the value goes, and with it the XMP property that mirrors the entry.
    let keys: Vec<String> = doc
        .trailer()
        .get(b"Info")
        .map(|o| doc.resolve(o))
        .and_then(|o| o.as_dict().map(|d| d.iter().filter_map(|(k, _)| String::from_utf8(k.clone()).ok()).collect()))
        .unwrap_or_default();
    for key in keys {
        let matches = pdfcraft_organize::info(doc, &key).is_some_and(|v| hit(&v));
        if work.exhausted.get() {
            return clear_metadata_on_limit(doc, report);
        }
        if matches {
            if let Some(root) = doc.root()
                && let Some(meta) = doc.get(root).as_dict().and_then(|d| d.get(b"Metadata").cloned())
            {
                let within_budget = match &*doc.resolve(&meta) {
                    Object::Stream(s) => {
                        work.charge(s.raw.len())
                            && s.decoded_within(MAX_METADATA.min(work.decode_limit())).is_ok_and(|bytes| work.charge(bytes.len().saturating_mul(4)))
                    }
                    _ => false,
                };
                if !within_budget {
                    return clear_metadata_on_limit(doc, report);
                }
            }
            pdfcraft_organize::set_info(doc, &key, "").map_err(|e| pdfcraft_edit::EditError::Invalid(e.to_string()))?;
            // A trailer `/Info` that is a direct dictionary is quietly left alone (only the
            // wholesale step below can take it); count what really went.
            if pdfcraft_organize::info(doc, &key).is_none() {
                report.info_entries_cleared += 1;
            }
        }
    }

    // 2. Wholesale: a stream that still holds removed text where per-entry editing cannot reach
    //    (custom schemas, page-level packets), or that cannot be inspected at all, goes whole.
    let mut owners: Vec<ObjRef> = doc.root().into_iter().collect();
    owners.extend(pdfcraft_model::pages(doc).iter().map(|p| p.obj));
    for owner in owners {
        let Some(entry) = doc.get(owner).as_dict().and_then(|d| d.get(b"Metadata").cloned()) else { continue };
        if !work.charge(1) {
            return clear_metadata_on_limit(doc, report);
        }
        let keeps = match &*doc.resolve(&entry) {
            Object::Stream(s) => {
                // Reserve room before decoding, even for invalid UTF-8 or failed filters. A
                // packet that cannot be inspected within the remaining operation allowance
                // exhausts it; do not repeatedly decode a fresh 16 MiB on every page.
                if !work.charge(s.raw.len()) {
                    return clear_metadata_on_limit(doc, report);
                }
                let limit = MAX_METADATA.min(work.decode_limit());
                match s.decoded_within(limit) {
                    Ok(bytes) if work.charge(bytes.len().saturating_mul(4)) => String::from_utf8(bytes).is_ok_and(|text| !packet_hit(&text, &hit)),
                    _ => {
                        work.exhausted.set(true);
                        false
                    }
                }
            }
            _ => false,
        };
        if work.exhausted.get() {
            return clear_metadata_on_limit(doc, report);
        }
        if !keeps {
            doc.update_dict(owner, |d| {
                d.remove(b"Metadata");
            })?;
            report.xmp_removed += 1;
        }
    }

    // 3. The dictionary one last time: a direct trailer dictionary cannot be edited per entry,
    //    and a value that survived step 1 takes the whole `/Info` with it (the sanitize shape).
    if doc
        .trailer()
        .get(b"Info")
        .map(|o| doc.resolve(o))
        .and_then(|o| o.as_dict().map(|d| d.iter().any(|(_, v)| doc.resolve(v).as_string().is_some_and(|s| hit(&s.to_text())))))
        .unwrap_or(false)
    {
        let n = doc.trailer().get(b"Info").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().map(Dict::len)).unwrap_or(0);
        doc.trailer_mut().remove(b"Info");
        report.info_entries_cleared += n;
    }
    if work.exhausted.get() {
        return clear_metadata_on_limit(doc, report);
    }
    Ok(())
}

/// Remove redaction marks without applying them (`None` = all).
pub fn clear_marks(doc: &mut Document, pages: Option<&[usize]>) -> Result<usize, RedactError> {
    let all = marks(doc);
    let doomed: Vec<ObjRef> = all.iter().filter(|m| pages.is_none_or(|p| p.contains(&m.page))).map(|m| m.obj).collect();
    if doomed.is_empty() {
        return Err(RedactError::NothingToApply);
    }
    for p in pdfcraft_model::pages(doc) {
        let annots = annots_of(doc, &p.dict);
        let kept: Vec<Object> = annots
            .iter()
            .filter(|a| {
                let Some(r) = a.as_ref() else { return true };
                let parent = doc.get(r).as_dict().and_then(|d| d.get(b"Parent")).and_then(Object::as_ref);
                !doomed.contains(&r) && !parent.is_some_and(|p| doomed.contains(&p))
            })
            .cloned()
            .collect();
        if kept.len() != annots.len() {
            doc.update_dict(p.obj, |d| d.set(b"Annots".to_vec(), Object::Array(kept)))?;
        }
    }
    Ok(doomed.len())
}
